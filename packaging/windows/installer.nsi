; NSIS installer for VisualHub. Build with:
;   makensis -DVERSION=0.1.0 packaging/windows/installer.nsi
!ifndef VERSION
  !define VERSION "0.1.0"
!endif

Name "VisualHub ${VERSION}"
OutFile "..\..\dist\VisualHub-${VERSION}-setup.exe"
InstallDir "$PROGRAMFILES64\VisualHub"
InstallDirRegKey HKLM "Software\VisualHub" "InstallDir"
RequestExecutionLevel admin
Unicode true
SetCompressor /SOLID lzma

Icon "visualhub.ico"
UninstallIcon "visualhub.ico"

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "VisualHub"
  SetOutPath "$INSTDIR"
  File "..\..\target\release\visualhub.exe"
  File "visualhub.ico"
  File "..\..\src\ui\LICENSE-SCHIST"

  WriteRegStr HKLM "Software\VisualHub" "InstallDir" "$INSTDIR"
  ; Add/Remove Programs entry.
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\VisualHub" \
    "DisplayName" "VisualHub"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\VisualHub" \
    "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\VisualHub" \
    "Publisher" "Astrid Gealer"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\VisualHub" \
    "UninstallString" "$INSTDIR\uninstall.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\VisualHub" \
    "DisplayIcon" "$INSTDIR\visualhub.ico"

  CreateShortcut "$SMPROGRAMS\VisualHub.lnk" "$INSTDIR\visualhub.exe" "" "$INSTDIR\visualhub.ico"
  WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\visualhub.exe"
  Delete "$INSTDIR\visualhub.ico"
  Delete "$INSTDIR\LICENSE-SCHIST"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$SMPROGRAMS\VisualHub.lnk"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "Software\VisualHub"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\VisualHub"
SectionEnd
