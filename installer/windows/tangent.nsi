; Tangent — Windows installer.
;
; Built from macOS with `makensis` (brew install makensis); scripts/build-installer.sh
; passes SRCDIR, OUTFILE and VERSION on the command line.
;
; Two components, either or both:
;   Standalone -> $PROGRAMFILES64\Tangent
;   VST3       -> $COMMONFILES64\VST3\Tangent.vst3
;
; $COMMONFILES64 is "C:\Program Files\Common Files", so that resolves to
; C:\Program Files\Common Files\VST3 — the location in the VST3 specification
; and the one every host scans without being configured. Writing anywhere else
; is the difference between a plugin that appears and one the user has to go
; looking for.
;
; The VST3 is a BUNDLE DIRECTORY, not a loose DLL:
;   Tangent.vst3\Contents\x86_64-win\Tangent.vst3
; That nesting is correct and is what the spec describes. Hosts do accept a
; bare DLL, but only the bundle can carry the licence files alongside it.

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"

!ifndef VERSION
  !error "VERSION not defined (pass -DVERSION=x.y.z)"
!endif
!ifndef SRCDIR
  !error "SRCDIR not defined"
!endif
!ifndef OUTFILE
  !error "OUTFILE not defined"
!endif

Name "Tangent ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$PROGRAMFILES64\Tangent"
InstallDirRegKey HKLM "Software\Tangent" "InstallDir"
; Both destinations are under Program Files, so this needs admin. Asking up
; front is better than failing halfway through with a file-copy error.
RequestExecutionLevel admin
SetCompressor /SOLID lzma

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Tangent"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "CompanyName" "ganten"
VIAddVersionKey "FileDescription" "Tangent installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "MIT (application) / GPL-3.0-or-later (plugin)"

!define MUI_ABORTWARNING
!define MUI_COMPONENTS_TEXT_TOP "Tangent comes in two pieces. Install either, or both."
!define MUI_FINISHPAGE_RUN "$INSTDIR\tangent.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open Tangent now"
!define MUI_FINISHPAGE_TEXT "Your DAW will find the plugin the next time it scans. It appears as Tangent, by ganten.$\r$\n$\r$\nHold H in either one to see every keyboard shortcut."

!insertmacro MUI_PAGE_LICENSE "${SRCDIR}\LICENSE.txt"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Tangent application" SEC_APP
  SectionIn 1
  SetOutPath "$INSTDIR"
  File /r "${SRCDIR}\app\*.*"

  CreateDirectory "$SMPROGRAMS\Tangent"
  CreateShortCut "$SMPROGRAMS\Tangent\Tangent.lnk" "$INSTDIR\tangent.exe"
  CreateShortCut "$SMPROGRAMS\Tangent\Uninstall Tangent.lnk" "$INSTDIR\Uninstall.exe"

  WriteRegStr HKLM "Software\Tangent" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tangent" \
    "DisplayName" "Tangent"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tangent" \
    "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tangent" \
    "Publisher" "ganten"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tangent" \
    "DisplayIcon" "$INSTDIR\tangent.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tangent" \
    "UninstallString" "$INSTDIR\Uninstall.exe"
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tangent" \
    "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tangent" \
    "NoRepair" 1
SectionEnd

Section "Tangent VST3 plugin" SEC_VST3
  SectionIn 1
  ; A stale bundle from a previous version is the classic way to end up with a
  ; host loading last month's plugin: the directory is overwritten file by
  ; file, so anything that used to be there and is not any more simply stays.
  RMDir /r "$COMMONFILES64\VST3\Tangent.vst3"
  SetOutPath "$COMMONFILES64\VST3\Tangent.vst3"
  File /r "${SRCDIR}\Tangent.vst3\*.*"
SectionEnd

Section -Uninstaller
  ; Always written, even when only the plugin was chosen, so there is always a
  ; way back out. $INSTDIR exists either way because SetOutPath created it.
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_APP} \
    "The standalone app. Opens its own window and connects to a MIDI keyboard by itself."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_VST3} \
    "The plugin, installed where every DAW looks. It reads the notes on the track it is on and produces no audio, so put it on a MIDI or instrument track."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "Tangent needs 64-bit Windows."
    Abort
  ${EndIf}
FunctionEnd

Section "Uninstall"
  RMDir /r "$COMMONFILES64\VST3\Tangent.vst3"
  Delete "$SMPROGRAMS\Tangent\Tangent.lnk"
  Delete "$SMPROGRAMS\Tangent\Uninstall Tangent.lnk"
  RMDir "$SMPROGRAMS\Tangent"
  RMDir /r "$INSTDIR"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tangent"
  DeleteRegKey HKLM "Software\Tangent"
  ; Settings are deliberately NOT removed. They live in %USERPROFILE%\.config\ivory
  ; and include taught chord names and a supporter key, none of which the user
  ; asked to throw away by uninstalling a program.
SectionEnd
