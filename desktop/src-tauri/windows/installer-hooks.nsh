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

  ; 安装/升级成功后自动打开：依赖模板自带机制，此处不得直接拉起——
  ; GUI 安装由完成页 "运行 Pony Proxy" 复选框触发（MUI_FINISHPAGE_RUN，
  ; 未定义 NOTCHECKED 即默认勾选，用户可取消；点完成后经 RunMainBinary
  ; 以 RunAsUser 拉起，单实例锁防重复）；
  ; 被动/静默升级（updater 下发 /P /UPDATE /R）完成页被跳过，由模板
  ; .onInstSuccess 凭 /R 携带 /ARGS 拉起。此处若无条件拉起，既无视用户
  ; 取消勾选，又会在更新模式下与 /R 路径双重启动（且本次拉起丢 /ARGS）。
  ; 注意：函数在模板末尾定义，NSIS 允许前向引用（模板自身第 701 行即前向调用）。
  Call CreateOrUpdateDesktopShortcut
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Cover direct-uninstall paths too (template uninstall section only deletes
  ; "${PRODUCTNAME}.lnk").
  !insertmacro _PPROXY_REMOVE_LEGACY_LNK "$DESKTOP\pony-desktop.lnk"
  !insertmacro _PPROXY_REMOVE_LEGACY_LNK "$SMPROGRAMS\pony-desktop.lnk"
!macroend
