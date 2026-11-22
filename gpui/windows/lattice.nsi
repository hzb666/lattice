Target amd64-unicode
!include LogicLib.nsh
!include WinVer.nsh
!include x64.nsh
!include "${PAYLOAD_INCLUDE}"

!define InstallRoot "$PROGRAMFILES64\Lattice GPUI"
!define ProductKey "Software\Microsoft\Windows\CurrentVersion\Uninstall\LatticeGPUI"
!define DriverName "LatticeAgentGuard"
!define SourceInf "$INSTDIR\driver\${DriverName}.inf"
!define Shortcut "$SMPROGRAMS\Lattice GPUI.lnk"

Name "Lattice GPUI"
OutFile "${SETUP_OUT}"
InstallDir "${InstallRoot}"
RequestExecutionLevel admin
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles
LoadLanguageFile "${NSISDIR}\Contrib\Language files\English.nlf"
LoadLanguageFile "${NSISDIR}\Contrib\Language files\SimpChinese.nlf"

LangString Unsupported ${LANG_ENGLISH} "Lattice Agent Guard requires x64 Windows 11 version 24H2 or later."
LangString Unsupported ${LANG_SIMPCHINESE} "Lattice Agent Guard 需要 Windows 11 24H2 或更高版本（x64）。"
LangString Occupied ${LANG_ENGLISH} "Uninstall the existing Lattice GPUI release before installing this one. Remove an existing standalone Agent Guard through its original installer."
LangString Occupied ${LANG_SIMPCHINESE} "请先卸载现有的 Lattice GPUI，再安装此版本。单独安装的 Agent Guard 请通过原安装程序卸载。"
LangString SetupFailed ${LANG_ENGLISH} "Setup did not complete. If Lattice GPUI appears in Installed apps, uninstall it there before retrying."
LangString SetupFailed ${LANG_SIMPCHINESE} "安装未完成。如果 Lattice GPUI 出现在“已安装的应用”中，请先卸载再重试。"
LangString RemoveFailed ${LANG_ENGLISH} "Removal did not complete. Close Lattice GPUI and its Agent, then retry uninstall. Setup details show where removal stopped."
LangString RemoveFailed ${LANG_SIMPCHINESE} "卸载未完成。请关闭 Lattice GPUI 及其 Agent 后重试。详情中有停止位置。"
LangString RestartNeeded ${LANG_ENGLISH} "Restart Windows to finish the driver change."
LangString RestartNeeded ${LANG_SIMPCHINESE} "请重启 Windows 以完成驱动程序变更。"

Var PublishedInf
Var StoreInf

!macro StopWith Message
  MessageBox MB_OK|MB_ICONSTOP "$(${Message})" /SD IDOK
  SetErrorLevel 1
  Quit
!macroend

!macro RestartRequired
  SetRebootFlag true
  SetErrorLevel 3010
  MessageBox MB_OK|MB_ICONINFORMATION "$(RestartNeeded)" /SD IDOK
!macroend

!macro CheckDirectory Path
  System::Call 'kernel32::GetFileAttributesW(w "${Path}") i.r0 ?e'
  Pop $1
  ${If} $0 == -1
    ${If} $1 != 2
    ${AndIf} $1 != 3
      Goto unsafe_paths
    ${EndIf}
  ${Else}
    IntOp $0 $0 & 0x410
    ${If} $0 != 0x10
      Goto unsafe_paths
    ${EndIf}
  ${EndIf}
!macroend

!macro QueryGuard
  System::Call 'advapi32::OpenSCManagerW(p 0, p 0, i 1) p.r2 ?e'
  Pop $0
  ${If} $2 != 0
    System::Call 'advapi32::OpenServiceW(p r2, w "${DriverName}", i 4) p.r3 ?e'
    Pop $0
    ${If} $3 != 0
      System::Call '*(i 0, i 0, i 0, i 0, i 0, i 0, i 0) p.r4'
      System::Call 'advapi32::QueryServiceStatus(p r3, p r4) i.r5 ?e'
      Pop $0
      ${If} $5 != 0
        System::Call '*$4(i, i.r1)'
        StrCpy $0 0
      ${EndIf}
      System::Free $4
      System::Call 'advapi32::CloseServiceHandle(p r3)'
    ${EndIf}
    System::Call 'advapi32::CloseServiceHandle(p r2)'
  ${EndIf}
!macroend

!macro LocateDriverStoreInf
  System::Call 'setupapi::SetupGetInfDriverStoreLocationW(w "$PublishedInf", p 0, p 0, w .r1, i ${NSIS_MAX_STRLEN}, p 0) i.r0'
!macroend

Function .onInit
  ${IfNot} ${IsNativeAMD64}
  ${OrIfNot} ${AtLeastBuild} 26100
    !insertmacro StopWith Unsupported
  ${EndIf}
  StrCpy $INSTDIR "${InstallRoot}"
  SetRegView 64
  SetShellVarContext all
FunctionEnd

Section
  ReadRegStr $0 HKLM "${ProductKey}" "UninstallString"
  StrCmp $0 "" 0 occupied
  IfFileExists "${Shortcut}" occupied
  !insertmacro QueryGuard
  StrCmp $0 1060 0 occupied
  System::Call 'kernel32::CreateDirectoryW(w "$INSTDIR", p 0) i.r0'
  StrCmp $0 0 occupied

  ClearErrors
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  IfErrors registration_failed
  WriteRegStr HKLM "${ProductKey}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  IfErrors registration_failed
  WriteRegStr HKLM "${ProductKey}" "DisplayName" "Lattice GPUI"
  IfErrors setup_failed

  SetOverwrite try
  !insertmacro InstallPayload
  IfErrors setup_failed

  System::Call 'setupapi::SetupCopyOEMInfW(w "${SourceInf}", p 0, i 1, i 8, w .r1, i ${NSIS_MAX_STRLEN}, p 0, p 0) i.r0 ?e'
  Pop $2
  DetailPrint "SetupCopyOEMInf: $0 ($2)"
  StrCmp $0 0 setup_failed
  StrCpy $PublishedInf $1
  ClearErrors
  WriteRegStr HKLM "${ProductKey}" "PublishedInf" "$PublishedInf"
  IfErrors setup_failed
  !insertmacro LocateDriverStoreInf
  StrCmp $0 0 setup_failed
  WriteRegStr HKLM "${ProductKey}" "DriverStoreInf" "$1"
  IfErrors setup_failed
  System::Call 'newdev::DiInstallDriverW(p $HWNDPARENT, w "${SourceInf}", i 0, *i 0 r1) i.r0 ?e'
  Pop $2
  DetailPrint "DiInstallDriver: $0 ($2)"
  StrCmp $0 0 setup_failed
  StrCmp $1 0 check_guard_start pending_start

  check_guard_start:
    !insertmacro QueryGuard
    StrCmp $0 0 0 setup_failed
    StrCmp $1 4 install_shortcut
    StrCmp $1 2 pending_start
    StrCmp $1 1 0 setup_failed
    ClearErrors
    ExecWait '"$SYSDIR\fltmc.exe" load ${DriverName}' $0
    IfErrors setup_failed
    DetailPrint "fltmc load: $0"
    !insertmacro QueryGuard
    StrCmp $0 0 0 setup_failed
    StrCmp $1 4 install_shortcut
    StrCmp $1 2 0 setup_failed

  pending_start:
    SetRebootFlag true

  install_shortcut:
    SetOutPath "$INSTDIR\bin"
    CreateShortcut "${Shortcut}" "$INSTDIR\bin\lattice-gpui.exe"
    IfErrors setup_failed
    IfRebootFlag 0 done
    !insertmacro RestartRequired
    Goto done

  occupied:
    !insertmacro StopWith Occupied
  registration_failed:
    Delete "$INSTDIR\Uninstall.exe"
    RMDir "$INSTDIR"
  setup_failed:
    !insertmacro StopWith SetupFailed
  done:
SectionEnd

Function un.onInit
  SetRegView 64
  SetShellVarContext all
  StrCmp $INSTDIR "${InstallRoot}" 0 remove_failed
  ReadRegStr $0 HKLM "${ProductKey}" "UninstallString"
  StrCmp $0 '"$INSTDIR\Uninstall.exe"' ready
  remove_failed:
    !insertmacro StopWith RemoveFailed
  ready:
FunctionEnd

Section "Uninstall"
  !insertmacro CheckDirectory "$INSTDIR"
  !insertmacro CheckPayloadDirectories
  ReadRegStr $PublishedInf HKLM "${ProductKey}" "PublishedInf"
  StrCmp $PublishedInf "" remove_files
  IfFileExists "$PublishedInf" 0 missing_driver
  ReadRegStr $StoreInf HKLM "${ProductKey}" "DriverStoreInf"
  !insertmacro LocateDriverStoreInf
  StrCmp $0 0 remove_failed
  StrCmp $StoreInf $1 0 remove_failed
  !insertmacro QueryGuard
  StrCmp $0 1060 remove_driver
  StrCmp $0 0 0 remove_failed
  StrCmp $1 1 remove_driver
  ClearErrors
  ExecWait '"$SYSDIR\fltmc.exe" unload ${DriverName}' $0
  IfErrors remove_failed
  DetailPrint "fltmc unload: $0"
  StrCmp $0 0 0 remove_failed

  remove_driver:
    System::Call 'newdev::DiUninstallDriverW(p $HWNDPARENT, w "$PublishedInf", i 0, *i 0 r1) i.r0 ?e'
    Pop $2
    DetailPrint "DiUninstallDriver: $0 ($2)"
    StrCmp $0 0 remove_failed
    StrCmp $1 0 removed_driver
    !insertmacro RestartRequired
    Goto removed_driver
  missing_driver:
    !insertmacro QueryGuard
    StrCmp $0 1060 removed_driver remove_failed
  removed_driver:
    ClearErrors
    DeleteRegValue HKLM "${ProductKey}" "PublishedInf"
    IfErrors remove_failed
  remove_files:
    SetOutPath "$SYSDIR"
    ClearErrors
    !insertmacro RemovePayloadFiles
    IfErrors remove_failed
    IfFileExists "${Shortcut}" 0 +2
    Delete "${Shortcut}"
    IfErrors remove_failed
    Delete "$INSTDIR\Uninstall.exe"
    IfErrors remove_failed
    DeleteRegKey HKLM "${ProductKey}"
    !insertmacro RemovePayloadDirectories
    RMDir "$INSTDIR"
    Goto done

  unsafe_paths:
    DetailPrint "Unsafe installation directory."
  remove_failed:
    !insertmacro StopWith RemoveFailed
  done:
SectionEnd
