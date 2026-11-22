#include "guard.h"

#define GUARD_NORMALIZED (FLT_FILE_NAME_NORMALIZED | FLT_FILE_NAME_QUERY_DEFAULT)
#define GUARD_WRITE_ACCESS (FILE_WRITE_DATA | FILE_APPEND_DATA | FILE_WRITE_EA | \
                            FILE_WRITE_ATTRIBUTES | FILE_DELETE_CHILD | DELETE | \
                            WRITE_DAC | WRITE_OWNER | ACCESS_SYSTEM_SECURITY | \
                            GENERIC_ALL | GENERIC_WRITE | MAXIMUM_ALLOWED)

static BOOLEAN
GuardExtensionProtected(UNICODE_STRING Component)
{
    static const UNICODE_STRING extensions[] = {
        RTL_CONSTANT_STRING(L".bib"), RTL_CONSTANT_STRING(L".cdx"),
        RTL_CONSTANT_STRING(L".cdxml"), RTL_CONSTANT_STRING(L".mnova"),
        RTL_CONSTANT_STRING(L".mnjs"), RTL_CONSTANT_STRING(L".mnl"),
        RTL_CONSTANT_STRING(L".mnlt"),
    };
    USHORT index;

    for (index = Component.Length / sizeof(WCHAR); index != 0; --index) {
        if (Component.Buffer[index - 1] == L'.') {
            Component.Buffer += index - 1;
            Component.Length -= (USHORT)((index - 1) * sizeof(WCHAR));
            Component.MaximumLength = Component.Length;
            for (index = 0; index < RTL_NUMBER_OF(extensions); ++index) {
                if (RtlEqualUnicodeString(&Component, &extensions[index], TRUE)) {
                    return TRUE;
                }
            }
            break;
        }
    }
    return FALSE;
}

static BOOLEAN
GuardPathAllowed(UNICODE_STRING Path)
{
    const UNICODE_STRING chem = RTL_CONSTANT_STRING(L".chem");
    USHORT count = Path.Length / sizeof(WCHAR);
    USHORT start = 1;
    USHORT index;

    if (count == 0 || Path.Length % sizeof(WCHAR) != 0 || Path.Buffer[0] != L'\\') {
        return FALSE;
    }
    while (count > 1 && Path.Buffer[count - 1] == L'\\') {
        --count;
    }
    for (index = start; index <= count; ++index) {
        UNICODE_STRING component;

        if (index < count && Path.Buffer[index] != L'\\') {
            if (Path.Buffer[index] == L':' || Path.Buffer[index] == UNICODE_NULL) {
                return FALSE;
            }
            continue;
        }
        component.Buffer = Path.Buffer + start;
        component.Length = (USHORT)((index - start) * sizeof(WCHAR));
        component.MaximumLength = component.Length;
        if (RtlEqualUnicodeString(&component, &chem, TRUE) ||
            (index == count && GuardExtensionProtected(component))) {
            return FALSE;
        }
        start = index + 1;
    }
    return TRUE;
}

VOID
GuardCheckPolicyContract(VOID)
{
#if DBG
    const UNICODE_STRING bib = RTL_CONSTANT_STRING(L"\\assets\\a.BiB");
    const UNICODE_STRING chem = RTL_CONSTANT_STRING(L"\\state\\.CHEM\\cache\\a.txt");
    const UNICODE_STRING chemDirectory = RTL_CONSTANT_STRING(L"\\state\\.chem\\");
    const UNICODE_STRING ads = RTL_CONSTANT_STRING(L"\\paper.tex:extra");
    const UNICODE_STRING tex = RTL_CONSTANT_STRING(L"\\state\\.chemistry\\paper.tex");

    NT_ASSERT(!GuardPathAllowed(bib));
    NT_ASSERT(!GuardPathAllowed(chem));
    NT_ASSERT(!GuardPathAllowed(chemDirectory));
    NT_ASSERT(!GuardPathAllowed(ads));
    NT_ASSERT(GuardPathAllowed(tex));
#endif
}

static BOOLEAN
GuardNameAllowed(PFLT_FILE_NAME_INFORMATION Info)
{
    UNICODE_STRING path;

    if (!NT_SUCCESS(FltParseFileNameInformation(Info)) || Info->Volume.Length == 0 ||
        Info->Volume.Length >= Info->Name.Length || Info->Stream.Length != 0) {
        return FALSE;
    }
    path.Buffer = Info->Name.Buffer + Info->Volume.Length / sizeof(WCHAR);
    path.Length = Info->Name.Length - Info->Volume.Length;
    path.MaximumLength = path.Length;
    return GuardPathAllowed(path);
}

static BOOLEAN
GuardCanQuery(VOID)
{
    return KeGetCurrentIrql() == PASSIVE_LEVEL && !KeAreAllApcsDisabled() &&
           IoGetTopLevelIrp() == NULL;
}

static BOOLEAN
GuardQueryOpenFile(PCFLT_RELATED_OBJECTS Objects, PFILE_STANDARD_INFORMATION Info)
{
    if (!GuardCanQuery() || Objects->FileObject == NULL ||
        FlagOn(Objects->FileObject->Flags, FO_VOLUME_OPEN | FO_CLEANUP_COMPLETE)) {
        return FALSE;
    }
    return NT_SUCCESS(FltQueryInformationFile(Objects->Instance, Objects->FileObject,
                                              Info, sizeof(*Info), FileStandardInformation, NULL)) &&
           !Info->DeletePending && (Info->Directory || Info->NumberOfLinks == 1);
}

static BOOLEAN
GuardWritableName(PFLT_CALLBACK_DATA Data, PCFLT_RELATED_OBJECTS Objects,
                  PFILE_STANDARD_INFORMATION Info)
{
    PFLT_FILE_NAME_INFORMATION name;
    BOOLEAN allowed;

    if (Objects->Transaction != NULL || !GuardQueryOpenFile(Objects, Info) ||
        !NT_SUCCESS(FltGetFileNameInformation(Data, FLT_FILE_NAME_NORMALIZED |
                                             FLT_FILE_NAME_QUERY_FILESYSTEM_ONLY, &name))) {
        return FALSE;
    }
    allowed = GuardNameAllowed(name);
    FltReleaseFileNameInformation(name);
    return allowed;
}

static BOOLEAN
GuardNameCanMutate(PFLT_CALLBACK_DATA Data, PCFLT_RELATED_OBJECTS Objects,
                   PFLT_FILE_NAME_INFORMATION Name, BOOLEAN AllowDirectory)
{
    OBJECT_ATTRIBUTES attributes;
    IO_STATUS_BLOCK ioStatus = {0};
    FILE_STAT_INFORMATION info;
    ULONG flags = OBJ_KERNEL_HANDLE | OBJ_DONT_REPARSE;
    NTSTATUS status;

    if (!GuardCanQuery()) {
        return FALSE;
    }
    if (!FlagOn(Data->Iopb->OperationFlags, SL_CASE_SENSITIVE)) {
        flags |= OBJ_CASE_INSENSITIVE;
    }
    InitializeObjectAttributes(&attributes, &Name->Name, flags, NULL, NULL);
    status = FltQueryInformationByName(Objects->Filter, Objects->Instance, &attributes,
                                      &ioStatus, &info, sizeof(info), FileStatInformation, NULL);
    if (status == STATUS_OBJECT_NAME_NOT_FOUND || status == STATUS_NO_SUCH_FILE) {
        return TRUE;
    }
    return status == STATUS_SUCCESS && ioStatus.Information >= sizeof(info) &&
           !FlagOn(info.FileAttributes, FILE_ATTRIBUTE_REPARSE_POINT) &&
           (FlagOn(info.FileAttributes, FILE_ATTRIBUTE_DIRECTORY) ? AllowDirectory :
            info.NumberOfLinks == 1);
}

static FLT_PREOP_CALLBACK_STATUS
GuardDeny(PFLT_CALLBACK_DATA Data, NTSTATUS Status)
{
    Data->IoStatus.Status = Status;
    Data->IoStatus.Information = 0;
    return FLT_PREOP_COMPLETE;
}

static FLT_PREOP_CALLBACK_STATUS
GuardCreate(PFLT_CALLBACK_DATA Data, PCFLT_RELATED_OBJECTS Objects, PVOID *CompletionContext)
{
    const FLT_PARAMETERS *parameters = &Data->Iopb->Parameters;
    ULONG options = parameters->Create.Options;
    ULONG disposition = options >> 24;
    PFLT_FILE_NAME_INFORMATION name;

    if (FlagOn(Data->Iopb->OperationFlags, SL_OPEN_PAGING_FILE) ||
        FlagOn(options, FILE_DELETE_ON_CLOSE) ||
        (Objects->FileObject != NULL && FlagOn(Objects->FileObject->Flags, FO_VOLUME_OPEN))) {
        return GuardDeny(Data, STATUS_ACCESS_DENIED);
    }
    if (parameters->Create.SecurityContext == NULL) {
        return GuardDeny(Data, STATUS_ACCESS_DENIED);
    }
    if (disposition == FILE_OPEN &&
        !FlagOn(parameters->Create.SecurityContext->DesiredAccess, GUARD_WRITE_ACCESS)) {
        return FLT_PREOP_SUCCESS_NO_CALLBACK;
    }
    if (disposition > FILE_OVERWRITE_IF || FlagOn(options, FILE_OPEN_BY_FILE_ID) ||
        Objects->Transaction != NULL || !GuardCanQuery() ||
        !NT_SUCCESS(FltGetFileNameInformation(Data, GUARD_NORMALIZED, &name))) {
        return GuardDeny(Data, STATUS_ACCESS_DENIED);
    }
    if (!GuardNameAllowed(name) ||
        !GuardNameCanMutate(Data, Objects, name,
                           disposition != FILE_SUPERSEDE && disposition < FILE_OVERWRITE)) {
        FltReleaseFileNameInformation(name);
        return GuardDeny(Data, STATUS_ACCESS_DENIED);
    }
    *CompletionContext = name;
    return FLT_PREOP_SUCCESS_WITH_CALLBACK;
}

static FLT_POSTOP_CALLBACK_STATUS
GuardPostCreate(PFLT_CALLBACK_DATA Data, PCFLT_RELATED_OBJECTS Objects, PVOID CompletionContext,
                FLT_POST_OPERATION_FLAGS Flags)
{
    PFLT_FILE_NAME_INFORMATION name = CompletionContext;
    PFLT_FILE_NAME_INFORMATION tunneled = NULL;
    FILE_STANDARD_INFORMATION info;

    if (!FlagOn(Flags, FLTFL_POST_OPERATION_DRAINING) &&
        NT_SUCCESS(Data->IoStatus.Status) && Data->IoStatus.Status != STATUS_REPARSE &&
        (!NT_SUCCESS(FltGetTunneledName(Data, name, &tunneled)) ||
         (tunneled != NULL && !GuardNameAllowed(tunneled)) ||
         !GuardQueryOpenFile(Objects, &info))) {
        if (!FlagOn(Objects->FileObject->Flags, FO_HANDLE_CREATED)) {
            FltCancelFileOpen(Objects->Instance, Objects->FileObject);
            GuardDeny(Data, STATUS_ACCESS_DENIED);
        }
    }
    if (tunneled != NULL) {
        FltReleaseFileNameInformation(tunneled);
    }
    FltReleaseFileNameInformation(name);
    return FLT_POSTOP_FINISHED_PROCESSING;
}

static FLT_PREOP_CALLBACK_STATUS
GuardRename(PFLT_CALLBACK_DATA Data, PCFLT_RELATED_OBJECTS Objects, PVOID *CompletionContext)
{
    const FLT_PARAMETERS *parameters = &Data->Iopb->Parameters;
    const FILE_RENAME_INFORMATION *rename = parameters->SetFileInformation.InfoBuffer;
    const ULONG header = FIELD_OFFSET(FILE_RENAME_INFORMATION, FileName);
    ULONG size = parameters->SetFileInformation.Length;
    PFLT_FILE_NAME_INFORMATION name;

    if (!FLT_IS_IRP_OPERATION(Data) || rename == NULL || size < header ||
        rename->FileNameLength == 0 || rename->FileNameLength % sizeof(WCHAR) != 0 ||
        rename->FileNameLength > size - header ||
        (rename->RootDirectory != NULL && FltGetRequestorProcess(Data) != PsGetCurrentProcess()) ||
        !NT_SUCCESS(FltGetDestinationFileNameInformation(Objects->Instance, Objects->FileObject,
                    rename->RootDirectory, (PWSTR)rename->FileName, rename->FileNameLength,
                    GUARD_NORMALIZED, &name))) {
        return GuardDeny(Data, STATUS_ACCESS_DENIED);
    }
    if (!GuardNameAllowed(name) || !GuardNameCanMutate(Data, Objects, name, FALSE)) {
        FltReleaseFileNameInformation(name);
        return GuardDeny(Data, STATUS_ACCESS_DENIED);
    }
    *CompletionContext = name;
    return FLT_PREOP_SYNCHRONIZE;
}

static FLT_POSTOP_CALLBACK_STATUS
GuardPostRename(PFLT_CALLBACK_DATA Data, PCFLT_RELATED_OBJECTS Objects, PVOID CompletionContext,
                FLT_POST_OPERATION_FLAGS Flags)
{
    PFLT_FILE_NAME_INFORMATION name = CompletionContext;
    PFLT_FILE_NAME_INFORMATION tunneled = NULL;

    UNREFERENCED_PARAMETER(Objects);
    if (!FlagOn(Flags, FLTFL_POST_OPERATION_DRAINING) && NT_SUCCESS(Data->IoStatus.Status) &&
        (!NT_SUCCESS(FltGetTunneledName(Data, name, &tunneled)) ||
         (tunneled != NULL &&
          (KeGetCurrentIrql() != PASSIVE_LEVEL || !GuardNameAllowed(tunneled))))) {
        KdPrintEx((DPFLTR_IHVDRIVER_ID, DPFLTR_ERROR_LEVEL,
                   "LatticeAgentGuard: rename destination could not be verified after tunneling\n"));
    }
    if (tunneled != NULL) {
        FltReleaseFileNameInformation(tunneled);
    }
    FltReleaseFileNameInformation(name);
    return FLT_POSTOP_FINISHED_PROCESSING;
}

static BOOLEAN
GuardCanDeleteDirectory(PCFLT_RELATED_OBJECTS Objects)
{
    FILE_ATTRIBUTE_TAG_INFORMATION info;

    return NT_SUCCESS(FltQueryInformationFile(Objects->Instance, Objects->FileObject,
                          &info, sizeof(info), FileAttributeTagInformation, NULL)) &&
           !FlagOn(info.FileAttributes, FILE_ATTRIBUTE_REPARSE_POINT);
}

static FLT_PREOP_CALLBACK_STATUS
GuardSetInformation(PFLT_CALLBACK_DATA Data, PCFLT_RELATED_OBJECTS Objects, PVOID *CompletionContext)
{
    FILE_STANDARD_INFORMATION target;
    FILE_INFORMATION_CLASS operation = Data->Iopb->Parameters.SetFileInformation.FileInformationClass;

    if (operation == FilePositionInformation || operation == FileIoPriorityHintInformation) {
        return FLT_PREOP_SUCCESS_NO_CALLBACK;
    }
    if (!GuardWritableName(Data, Objects, &target)) {
        return GuardDeny(Data, STATUS_ACCESS_DENIED);
    }
    switch (operation) {
    case FileBasicInformation:
    case FileAllocationInformation:
    case FileEndOfFileInformation:
        return FLT_PREOP_SUCCESS_NO_CALLBACK;
    case FileDispositionInformation:
    case FileDispositionInformationEx:
        if (!target.Directory || GuardCanDeleteDirectory(Objects)) {
            return FLT_PREOP_SUCCESS_NO_CALLBACK;
        }
        break;
    case FileRenameInformation:
    case FileRenameInformationEx:
        if (!target.Directory) {
            return GuardRename(Data, Objects, CompletionContext);
        }
        break;
    }
    return GuardDeny(Data, STATUS_ACCESS_DENIED);
}

static BOOLEAN
GuardReadOnlySection(ULONG Protection)
{
    switch (Protection & 0xffu) {
    case PAGE_NOACCESS:
    case PAGE_READONLY:
    case PAGE_WRITECOPY:
    case PAGE_EXECUTE:
    case PAGE_EXECUTE_READ:
    case PAGE_EXECUTE_WRITECOPY:
        return TRUE;
    default:
        return FALSE;
    }
}

static FLT_PREOP_CALLBACK_STATUS
GuardPreOperation(PFLT_CALLBACK_DATA Data, PCFLT_RELATED_OBJECTS Objects, PVOID *CompletionContext)
{
    const FLT_PARAMETERS *parameters = &Data->Iopb->Parameters;
    UCHAR operation = Data->Iopb->MajorFunction;
    FILE_STANDARD_INFORMATION target;

    *CompletionContext = NULL;
    if (!GuardIsEnrolled(FltGetRequestorProcess(Data)) &&
        (Data->Thread == NULL || !GuardIsEnrolled(IoThreadToProcess(Data->Thread))) &&
        (operation != IRP_MJ_ACQUIRE_FOR_SECTION_SYNCHRONIZATION ||
         !GuardIsEnrolled(PsGetCurrentProcess()))) {
        return FLT_PREOP_SUCCESS_NO_CALLBACK;
    }
    if (FLT_IS_FASTIO_OPERATION(Data)) {
        return FLT_PREOP_DISALLOW_FASTIO;
    }
    if (operation == IRP_MJ_ACQUIRE_FOR_SECTION_SYNCHRONIZATION) {
        // shortcut: shared writable sections are denied until mapping targets can be checked safely.
        if (parameters->AcquireForSectionSynchronization.SyncType == SyncTypeCreateSection &&
            !GuardReadOnlySection(parameters->AcquireForSectionSynchronization.PageProtection)) {
            return GuardDeny(Data, STATUS_INSUFFICIENT_RESOURCES);
        }
        return FLT_PREOP_SUCCESS_NO_CALLBACK;
    }
    switch (operation) {
    case IRP_MJ_CREATE:
        return GuardCreate(Data, Objects, CompletionContext);
    case IRP_MJ_SET_INFORMATION:
        return GuardSetInformation(Data, Objects, CompletionContext);
    case IRP_MJ_WRITE:
        if (GuardWritableName(Data, Objects, &target) && !target.Directory) {
            return FLT_PREOP_SUCCESS_NO_CALLBACK;
        }
        break;
    case IRP_MJ_FILE_SYSTEM_CONTROL:
        if (Data->Iopb->MinorFunction == IRP_MN_USER_FS_REQUEST &&
            parameters->FileSystemControl.Common.FsControlCode == FSCTL_GET_REPARSE_POINT) {
            return FLT_PREOP_SUCCESS_NO_CALLBACK;
        }
        break;
    }
    return GuardDeny(Data, STATUS_ACCESS_DENIED);
}

const FLT_OPERATION_REGISTRATION GuardOperations[] = {
    { IRP_MJ_CREATE, 0, GuardPreOperation, GuardPostCreate, NULL },
    { IRP_MJ_WRITE, FLTFL_OPERATION_REGISTRATION_SKIP_PAGING_IO, GuardPreOperation, NULL, NULL },
    { IRP_MJ_PREPARE_MDL_WRITE, 0, GuardPreOperation, NULL, NULL },
    { IRP_MJ_SET_INFORMATION, 0, GuardPreOperation, GuardPostRename, NULL },
    { IRP_MJ_SET_EA, 0, GuardPreOperation, NULL, NULL },
    { IRP_MJ_SET_SECURITY, 0, GuardPreOperation, NULL, NULL },
    { IRP_MJ_SET_QUOTA, 0, GuardPreOperation, NULL, NULL },
    { IRP_MJ_SET_VOLUME_INFORMATION, 0, GuardPreOperation, NULL, NULL },
    { IRP_MJ_FILE_SYSTEM_CONTROL, 0, GuardPreOperation, NULL, NULL },
    { IRP_MJ_DEVICE_CONTROL, 0, GuardPreOperation, NULL, NULL },
    { IRP_MJ_ACQUIRE_FOR_SECTION_SYNCHRONIZATION, 0, GuardPreOperation, NULL, NULL },
    { IRP_MJ_OPERATION_END, 0, NULL, NULL, NULL },
};
