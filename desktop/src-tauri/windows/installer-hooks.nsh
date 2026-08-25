; Pony Proxy NSIS installer hooks (tauri v2 bundle > windows > nsis > installerHooks)
;
; Background: releases up to v0.3.0 used productName "pony-desktop", so old installs
; left "%USERPROFILE%\Desktop\pony-desktop.lnk" and
; "%APPDATA%\Microsoft\Windows\Start Menu\Programs\pony-desktop.lnk" behind.
; The stock tauri template only migrates "${PRODUCTNAME}.lnk" whose TARGET changed
; (OldMainBinaryName migration); it never removes shortcuts named after an OLD product
; name, so those stale .lnk survive upgrades AND uninstalls forever.
;
; Fix: remove legacy-named shortcuts after install and before uninstall, but ONLY when
; the shortcut target really points at the legacy install
; (%LOCALAPPDATA%\pony-desktop\pony-desktop.exe) so unrelated user files named alike
; are never touched. UnpinShortcut first so taskbar/start pins don't turn into ghosts.

!macro _PPROXY_REMOVE_LEGACY_LNK LNK_PATH
  Push $0
  !insertmacro IsShortcutTarget "${LNK_PATH}" "$LOCALAPPDATA\pony-desktop\pony-desktop.exe"
  Pop $0
  ${If} $0 = 1
    DetailPrint "Removing legacy shortcut: ${LNK_PATH}"
    !insertmacro UnpinShortcut "${LNK_PATH}"
    Delete "${LNK_PATH}"
  ${EndIf}
  Pop $0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; New shortcuts already created by the template above this point.
  !insertmacro _PPROXY_REMOVE_LEGACY_LNK "$DESKTOP\pony-desktop.lnk"
  !insertmacro _PPROXY_REMOVE_LEGACY_LNK "$SMPROGRAMS\pony-desktop.lnk"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Cover direct-uninstall paths too (template uninstall section only deletes
  ; "${PRODUCTNAME}.lnk").
  !insertmacro _PPROXY_REMOVE_LEGACY_LNK "$DESKTOP\pony-desktop.lnk"
  !insertmacro _PPROXY_REMOVE_LEGACY_LNK "$SMPROGRAMS\pony-desktop.lnk"
!macroend
