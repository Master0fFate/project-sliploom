Unicode true
!include "MUI2.nsh"

Name "Sliploom"
OutFile "${PACKAGE_PATH}"
InstallDir "$LOCALAPPDATA\Programs\Sliploom"
RequestExecutionLevel user
SetCompressor /SOLID lzma

VIProductVersion "${PRODUCT_VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "Sliploom"
VIAddVersionKey /LANG=1033 "FileDescription" "Sliploom test installer"
VIAddVersionKey /LANG=1033 "FileVersion" "${PRODUCT_VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductVersion" "${PRODUCT_VERSION}"

!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Sliploom"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Install"
  SetShellVarContext current
  SetRegView 64
  SetOutPath "$INSTDIR"
  File "${APP_PATH}"
  File "${FONT_NOTICE}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortCut "$SMPROGRAMS\Sliploom.lnk" "$INSTDIR\seam-desktop.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "Sliploom"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\seam-desktop.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '$\"$INSTDIR\Uninstall.exe$\" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  SetRegView 64
  SetOutPath "$TEMP"
  IfFileExists "$INSTDIR\seam-desktop.exe" 0 remove_metadata
  ClearErrors
  Delete "$INSTDIR\seam-desktop.exe"
  IfErrors 0 remove_metadata
  MessageBox MB_OK|MB_ICONEXCLAMATION "Close Sliploom before uninstalling." /SD IDOK
  SetErrorLevel 1
  Abort

  remove_metadata:
  Delete "$INSTDIR\Roboto-OFL.txt"
  Delete "$INSTDIR\Uninstall.exe"
  Delete "$SMPROGRAMS\Sliploom.lnk"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  RMDir "$INSTDIR"
SectionEnd
