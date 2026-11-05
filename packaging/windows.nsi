Unicode True
!include "MUI2.nsh"
!include "LogicLib.nsh"
!ifndef VERSION
!define VERSION "0.1.0"
!endif
!ifndef BINARY
!error "BINARY must point to the release executable"
!endif
!ifndef LICENSE_FILE
!define LICENSE_FILE "${__FILEDIR__}/../LICENSE"
!endif
!ifndef OUTPUT
!define OUTPUT "Sidekick-Setup.exe"
!endif
Name "Sidekick"
OutFile "${OUTPUT}"
InstallDir "$LOCALAPPDATA\HostAgent"
RequestExecutionLevel user
SetCompressor /SOLID lzma
!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\host-agent.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch Sidekick (configure startup in the tray)"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Sidekick" SEC_MAIN
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  SetOverwrite try
  ClearErrors
  File /oname=host-agent.exe "${BINARY}"
  IfErrors 0 +3
    MessageBox MB_OK|MB_ICONSTOP "Quit Sidekick from its tray menu before installing an update."
    Abort
  File /oname=LICENSE "${LICENSE_FILE}"
  ; Clean up shortcuts from the previous display name during an upgrade.
  Delete "$SMPROGRAMS\Host Agent\Host Agent.lnk"
  Delete "$SMPROGRAMS\Host Agent\Uninstall.lnk"
  RMDir "$SMPROGRAMS\Host Agent"
  CreateDirectory "$SMPROGRAMS\Sidekick"
  CreateShortcut "$SMPROGRAMS\Sidekick\Sidekick.lnk" "$INSTDIR\host-agent.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortcut "$SMPROGRAMS\Sidekick\Uninstall.lnk" "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HostAgent" "DisplayName" "Sidekick"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HostAgent" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HostAgent" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HostAgent" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HostAgent" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HostAgent" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  Delete "$INSTDIR\host-agent.exe"
  IfFileExists "$INSTDIR\host-agent.exe" 0 +3
    MessageBox MB_OK|MB_ICONSTOP "Quit Sidekick from its tray menu before uninstalling."
    Abort
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "dev.HostAgent.HostAgent"
  Delete "$SMPROGRAMS\Sidekick\Sidekick.lnk"
  Delete "$SMPROGRAMS\Sidekick\Uninstall.lnk"
  RMDir "$SMPROGRAMS\Sidekick"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HostAgent"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  ; User config and pairing identity are deliberately retained.
SectionEnd
