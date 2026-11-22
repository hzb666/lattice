#include "guard.h"

#define GUARD_TAG 'gAtL'
#define GUARD_MAX_PROCESSES 1024u
#define GUARD_MAX_CLIENTS 64
#define GUARD_PORT_CONNECT 0x0001u

typedef struct _GUARD_PROCESS {
    LIST_ENTRY Link;
    PEPROCESS Process;
} GUARD_PROCESS;

typedef struct _GUARD_CLIENT {
    PFLT_PORT Port;
    PEPROCESS Owner;
} GUARD_CLIENT;

static PFLT_FILTER GuardFilter;
static PFLT_PORT GuardServerPort;
static LIST_ENTRY GuardProcesses;
static KSPIN_LOCK GuardProcessLock;
static ULONG GuardProcessCount;
static BOOLEAN GuardNotifyRegistered;
static BOOLEAN GuardStopping;

DRIVER_INITIALIZE DriverEntry;

static GUARD_PROCESS *
GuardFindProcessLocked(PEPROCESS Process)
{
    PLIST_ENTRY link;

    for (link = GuardProcesses.Flink; link != &GuardProcesses; link = link->Flink) {
        GUARD_PROCESS *entry = CONTAINING_RECORD(link, GUARD_PROCESS, Link);
        if (entry->Process == Process) {
            return entry;
        }
    }
    return NULL;
}

BOOLEAN
GuardIsEnrolled(PEPROCESS Process)
{
    KIRQL irql;
    BOOLEAN found;

    if (Process == NULL) {
        return FALSE;
    }
    KeAcquireSpinLock(&GuardProcessLock, &irql);
    found = GuardFindProcessLocked(Process) != NULL;
    KeReleaseSpinLock(&GuardProcessLock, irql);
    return found;
}

static NTSTATUS
GuardClientStatus(PEPROCESS Process)
{
    KIRQL irql;
    NTSTATUS status = STATUS_SUCCESS;

    KeAcquireSpinLock(&GuardProcessLock, &irql);
    if (GuardStopping) {
        status = STATUS_DELETE_PENDING;
    } else if (GuardFindProcessLocked(Process) != NULL) {
        status = STATUS_ACCESS_DENIED;
    }
    KeReleaseSpinLock(&GuardProcessLock, irql);
    return status;
}

static VOID
GuardFreeProcess(GUARD_PROCESS *Entry)
{
    ObDereferenceObject(Entry->Process);
    ExFreePoolWithTag(Entry, GUARD_TAG);
}

static VOID
GuardForgetProcess(PEPROCESS Process)
{
    KIRQL irql;
    GUARD_PROCESS *entry;

    KeAcquireSpinLock(&GuardProcessLock, &irql);
    entry = GuardFindProcessLocked(Process);
    if (entry != NULL) {
        RemoveEntryList(&entry->Link);
        --GuardProcessCount;
    }
    KeReleaseSpinLock(&GuardProcessLock, irql);
    if (entry != NULL) {
        GuardFreeProcess(entry);
    }
}

static NTSTATUS
GuardTrackProcess(PEPROCESS Process, PEPROCESS Caller)
{
    LARGE_INTEGER noWait = {0};
    GUARD_PROCESS *entry;
    NTSTATUS status = STATUS_SUCCESS;
    KIRQL irql;

    entry = ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(*entry), GUARD_TAG);
    if (entry == NULL) {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    ObReferenceObject(Process);
    entry->Process = Process;

    KeAcquireSpinLock(&GuardProcessLock, &irql);
    if (GuardStopping) {
        status = STATUS_DELETE_PENDING;
    } else if (Caller != NULL && GuardFindProcessLocked(Caller) != NULL) {
        status = STATUS_ACCESS_DENIED;
    } else if (GuardFindProcessLocked(Process) == NULL) {
        if (Caller != NULL &&
            KeWaitForSingleObject(Process, Executive, KernelMode, FALSE, &noWait) != STATUS_TIMEOUT) {
            status = STATUS_PROCESS_IS_TERMINATING;
        } else if (GuardProcessCount == GUARD_MAX_PROCESSES) {
            status = STATUS_QUOTA_EXCEEDED;
        } else {
            InsertTailList(&GuardProcesses, &entry->Link);
            ++GuardProcessCount;
            entry = NULL;
        }
    }
    KeReleaseSpinLock(&GuardProcessLock, irql);

    if (entry != NULL) {
        GuardFreeProcess(entry);
    }
    return status;
}

static VOID
GuardReapExitedProcesses(VOID)
{
    LARGE_INTEGER noWait = {0};
    LIST_ENTRY retired;
    PLIST_ENTRY link;
    KIRQL irql;

    InitializeListHead(&retired);
    KeAcquireSpinLock(&GuardProcessLock, &irql);
    link = GuardProcesses.Flink;
    while (link != &GuardProcesses) {
        GUARD_PROCESS *entry = CONTAINING_RECORD(link, GUARD_PROCESS, Link);
        link = link->Flink;
        if (KeWaitForSingleObject(entry->Process, Executive, KernelMode, FALSE, &noWait)
            == STATUS_SUCCESS) {
            RemoveEntryList(&entry->Link);
            InsertTailList(&retired, &entry->Link);
            --GuardProcessCount;
        }
    }
    KeReleaseSpinLock(&GuardProcessLock, irql);
    while (!IsListEmpty(&retired)) {
        link = RemoveHeadList(&retired);
        GuardFreeProcess(CONTAINING_RECORD(link, GUARD_PROCESS, Link));
    }
}

static BOOLEAN
GuardInheritedEnrollment(PPS_CREATE_NOTIFY_INFO Info)
{
    PEPROCESS creator = IoThreadToProcess(PsGetCurrentThread());
    PEPROCESS parent;
    BOOLEAN enrolled;

    if (GuardIsEnrolled(creator) || GuardIsEnrolled(PsGetCurrentProcess())) {
        return TRUE;
    }
    if (Info->ParentProcessId == Info->CreatingThreadId.UniqueProcess ||
        !NT_SUCCESS(PsLookupProcessByProcessId(Info->ParentProcessId, &parent))) {
        return FALSE;
    }
    enrolled = GuardIsEnrolled(parent);
    ObDereferenceObject(parent);
    return enrolled;
}

static VOID
GuardProcessNotify(PEPROCESS Process, HANDLE ProcessId, PPS_CREATE_NOTIFY_INFO Info)
{
    UNREFERENCED_PARAMETER(ProcessId);

    if (Info == NULL) {
        GuardForgetProcess(Process);
    } else if (NT_SUCCESS(Info->CreationStatus) && GuardInheritedEnrollment(Info)) {
        NTSTATUS status = GuardTrackProcess(Process, NULL);
        if (!NT_SUCCESS(status)) {
            Info->CreationStatus = status;
        }
    }
}

static NTSTATUS
GuardVolumeReady(HANDLE Handle)
{
    PFILE_OBJECT file;
    PFLT_VOLUME volume;
    PFLT_INSTANCE instance;
    FILE_STANDARD_INFORMATION info;
    FILE_ATTRIBUTE_TAG_INFORMATION attributes;
    FLT_FILESYSTEM_TYPE type;
    NTSTATUS status;

    status = ObReferenceObjectByHandle(Handle, FILE_READ_ATTRIBUTES, *IoFileObjectType,
                                       UserMode, (PVOID *)&file, NULL);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    if (KeAreAllApcsDisabled() || FlagOn(file->Flags, FO_CLEANUP_COMPLETE | FO_VOLUME_OPEN)) {
        ObDereferenceObject(file);
        return STATUS_INVALID_DEVICE_STATE;
    }
    status = FltGetVolumeFromFileObject(GuardFilter, file, &volume);
    if (NT_SUCCESS(status)) {
        status = FltGetFileSystemType(volume, &type);
        if (NT_SUCCESS(status)) {
            status = type == FLT_FSTYPE_NTFS
                ? FltGetVolumeInstanceFromName(GuardFilter, volume, NULL, &instance)
                : STATUS_NOT_SUPPORTED;
        }
        if (NT_SUCCESS(status)) {
            status = FltQueryInformationFile(instance, file, &info, sizeof(info),
                                             FileStandardInformation, NULL);
            if (NT_SUCCESS(status) && (!info.Directory || info.DeletePending)) {
                status = STATUS_NOT_A_DIRECTORY;
            }
            if (NT_SUCCESS(status)) {
                status = FltQueryInformationFile(instance, file, &attributes, sizeof(attributes),
                                                 FileAttributeTagInformation, NULL);
                if (NT_SUCCESS(status) && FlagOn(attributes.FileAttributes, FILE_ATTRIBUTE_REPARSE_POINT)) {
                    status = STATUS_NOT_SUPPORTED;
                }
            }
            FltObjectDereference(instance);
        }
        FltObjectDereference(volume);
    }
    ObDereferenceObject(file);
    return status;
}

static NTSTATUS
GuardConnect(PFLT_PORT Port, PVOID ServerCookie, PVOID Context, ULONG ContextSize,
             PVOID *ConnectionCookie)
{
    PEPROCESS owner = PsGetCurrentProcess();
    GUARD_CLIENT *client;
    NTSTATUS status;

    UNREFERENCED_PARAMETER(ServerCookie);
    UNREFERENCED_PARAMETER(Context);
    *ConnectionCookie = NULL;
    if (ContextSize != 0) {
        return STATUS_ACCESS_DENIED;
    }
    status = GuardClientStatus(owner);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    client = ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(*client), GUARD_TAG);
    if (client == NULL) {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    ObReferenceObject(owner);
    client->Owner = owner;
    client->Port = Port;
    *ConnectionCookie = client;
    return STATUS_SUCCESS;
}

static VOID
GuardDisconnect(PVOID ConnectionCookie)
{
    GUARD_CLIENT *client = ConnectionCookie;

    FltCloseClientPort(GuardFilter, &client->Port);
    ObDereferenceObject(client->Owner);
    ExFreePoolWithTag(client, GUARD_TAG);
}

static NTSTATUS
GuardEnrollHandle(HANDLE Handle, PEPROCESS Caller)
{
    PEPROCESS process;
    NTSTATUS status;

    status = ObReferenceObjectByHandle(Handle, PROCESS_TERMINATE, *PsProcessType,
                                       UserMode, (PVOID *)&process, NULL);
    if (NT_SUCCESS(status)) {
        GuardReapExitedProcesses();
        status = process != Caller && process != PsInitialSystemProcess
            ? GuardTrackProcess(process, Caller) : STATUS_ACCESS_DENIED;
        ObDereferenceObject(process);
    }
    return status;
}

static NTSTATUS
GuardMessage(PVOID ConnectionCookie, PVOID Input, ULONG InputSize, PVOID Output,
             ULONG OutputSize, PULONG ReturnOutputSize)
{
    const GUARD_CLIENT *client = ConnectionCookie;
    PEPROCESS caller = PsGetCurrentProcess();
    GUARD_REQUEST request;
    HANDLE handle;
    NTSTATUS status;

    UNREFERENCED_PARAMETER(Output);
    UNREFERENCED_PARAMETER(OutputSize);
    *ReturnOutputSize = 0;
    if (caller != client->Owner) {
        return STATUS_ACCESS_DENIED;
    }
    status = GuardClientStatus(caller);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    if (Input == NULL || InputSize != sizeof(request) || ((ULONG_PTR)Input & 3u) != 0) {
        return STATUS_INVALID_PARAMETER;
    }
    __try {
        RtlCopyMemory(&request, Input, sizeof(request));
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return GetExceptionCode();
    }
    if (request.Version != GUARD_PROTOCOL_VERSION) {
        return STATUS_REVISION_MISMATCH;
    }
    if (request.Operation == GUARD_OP_PROBE) {
        return request.Handle == 0 ? STATUS_SUCCESS : STATUS_INVALID_PARAMETER;
    }
    if (request.Handle == 0 || request.Handle >= (UINT64)(MAXLONG_PTR)) {
        return STATUS_INVALID_HANDLE;
    }
    handle = (HANDLE)(ULONG_PTR)request.Handle;
    switch (request.Operation) {
    case GUARD_OP_ENROLL:
        return GuardEnrollHandle(handle, caller);
    case GUARD_OP_VOLUME_READY:
        return GuardVolumeReady(handle);
    default:
        return STATUS_INVALID_DEVICE_REQUEST;
    }
}

static NTSTATUS
GuardCreatePort(VOID)
{
    UNICODE_STRING name = RTL_CONSTANT_STRING(GUARD_PORT_NAME);
    SECURITY_DESCRIPTOR descriptor;
    OBJECT_ATTRIBUTES attributes;
    ULONG aclStorage[32];
    ULONG denyStorage[SECURITY_MAX_SID_SIZE / sizeof(ULONG) + 2];
    PACL acl = (PACL)aclStorage;
    PACCESS_DENIED_ACE deny = (PACCESS_DENIED_ACE)denyStorage;
    PSID networkSid = SeExports->SeNetworkSid;
    ULONG sidLength = RtlLengthSid(networkSid);
    NTSTATUS status;

    deny->Header.AceType = ACCESS_DENIED_ACE_TYPE;
    deny->Header.AceFlags = 0;
    deny->Header.AceSize = (USHORT)(FIELD_OFFSET(ACCESS_DENIED_ACE, SidStart) + sidLength);
    deny->Mask = GUARD_PORT_CONNECT;
    status = RtlCopySid(sidLength, &deny->SidStart, networkSid);
    if (NT_SUCCESS(status)) {
        status = RtlCreateAcl(acl, sizeof(aclStorage), ACL_REVISION);
    }
    if (NT_SUCCESS(status)) {
        status = RtlAddAce(acl, ACL_REVISION, 0, deny, deny->Header.AceSize);
    }
    if (NT_SUCCESS(status)) {
        status = RtlAddAccessAllowedAce(acl, ACL_REVISION, GUARD_PORT_CONNECT,
                                        SeExports->SeAuthenticatedUsersSid);
    }
    if (NT_SUCCESS(status)) {
        status = RtlAddAccessAllowedAce(acl, ACL_REVISION, GUARD_PORT_CONNECT,
                                        SeExports->SeLocalSystemSid);
    }
    if (NT_SUCCESS(status)) {
        status = RtlCreateSecurityDescriptor(&descriptor, SECURITY_DESCRIPTOR_REVISION);
    }
    if (NT_SUCCESS(status)) {
        status = RtlSetDaclSecurityDescriptor(&descriptor, TRUE, acl, FALSE);
    }
    if (!NT_SUCCESS(status)) {
        return status;
    }
    InitializeObjectAttributes(&attributes, &name, OBJ_KERNEL_HANDLE | OBJ_CASE_INSENSITIVE,
                               NULL, &descriptor);
    return FltCreateCommunicationPort(GuardFilter, &GuardServerPort, &attributes, NULL,
                                      GuardConnect, GuardDisconnect, GuardMessage,
                                      GUARD_MAX_CLIENTS);
}

static NTSTATUS
GuardInstanceSetup(PCFLT_RELATED_OBJECTS Objects, FLT_INSTANCE_SETUP_FLAGS Flags,
                   DEVICE_TYPE VolumeDeviceType, FLT_FILESYSTEM_TYPE VolumeFilesystemType)
{
    UNREFERENCED_PARAMETER(Objects);
    UNREFERENCED_PARAMETER(Flags);
    return VolumeDeviceType == FILE_DEVICE_DISK_FILE_SYSTEM && VolumeFilesystemType == FLT_FSTYPE_NTFS
        ? STATUS_SUCCESS : STATUS_FLT_DO_NOT_ATTACH;
}

static VOID
GuardShutdown(VOID)
{
    if (GuardServerPort != NULL) {
        FltCloseCommunicationPort(GuardServerPort);
        GuardServerPort = NULL;
    }
    FltUnregisterFilter(GuardFilter);
    if (GuardNotifyRegistered) {
        NT_VERIFY(NT_SUCCESS(PsSetCreateProcessNotifyRoutineEx(GuardProcessNotify, TRUE)));
    }
    while (!IsListEmpty(&GuardProcesses)) {
        PLIST_ENTRY link = RemoveHeadList(&GuardProcesses);
        GuardFreeProcess(CONTAINING_RECORD(link, GUARD_PROCESS, Link));
    }
}

static NTSTATUS
GuardUnload(FLT_FILTER_UNLOAD_FLAGS Flags)
{
    BOOLEAN mandatory = FlagOn(Flags, FLTFL_FILTER_UNLOAD_MANDATORY) != 0;
    KIRQL irql;

    GuardReapExitedProcesses();
    KeAcquireSpinLock(&GuardProcessLock, &irql);
    if (!mandatory && GuardProcessCount != 0) {
        KeReleaseSpinLock(&GuardProcessLock, irql);
        return STATUS_FLT_DO_NOT_DETACH;
    }
    GuardStopping = TRUE;
    KeReleaseSpinLock(&GuardProcessLock, irql);
    GuardShutdown();
    return STATUS_SUCCESS;
}

static const FLT_REGISTRATION GuardRegistration = {
    .Size = sizeof(FLT_REGISTRATION),
    .Version = FLT_REGISTRATION_VERSION,
    .Flags = FLTFL_REGISTRATION_DO_NOT_SUPPORT_SERVICE_STOP,
    .OperationRegistration = GuardOperations,
    .FilterUnloadCallback = GuardUnload,
    .InstanceSetupCallback = GuardInstanceSetup,
};

NTSTATUS
DriverEntry(PDRIVER_OBJECT DriverObject, PUNICODE_STRING RegistryPath)
{
    NTSTATUS status;

    UNREFERENCED_PARAMETER(RegistryPath);
    GuardCheckPolicyContract();
    InitializeListHead(&GuardProcesses);
    KeInitializeSpinLock(&GuardProcessLock);

    status = FltRegisterFilter(DriverObject, &GuardRegistration, &GuardFilter);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    status = PsSetCreateProcessNotifyRoutineEx(GuardProcessNotify, FALSE);
    if (NT_SUCCESS(status)) {
        GuardNotifyRegistered = TRUE;
        status = FltStartFiltering(GuardFilter);
    }
    if (NT_SUCCESS(status)) {
        status = GuardCreatePort();
    }
    if (!NT_SUCCESS(status)) {
        GuardShutdown();
    }
    return status;
}
