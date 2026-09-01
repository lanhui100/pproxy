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

  ; 更新桌面图标：模板只在 silent/passive 或完成页勾选时创建桌面快捷方式，
  ; 这里无条件补齐——保证 GUI 未勾选、升级后旧图标残留等场景下桌面图标
  ; 始终存在且指向当前版本 exe（Pony Proxy.lnk）。
  ; 注意：函数在模板末尾定义，NSIS 允许前向引用（模板自身第 701 行即前向调用）。
  Call CreateOrUpdateDesktopShortcut

  ; 安装成功自动启动应用：交互式安装完成即拉起（单实例锁保证与完成页
  ; "Run" 复选框重复启动互斥，无重复进程）。静默/自动更新路径不在此拉起，
  ; 由 tauri 更新器自行重启，避免双重启动冲突。
  ${IfNot} ${Silent}
    nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" ""
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Cover direct-uninstall paths too (template uninstall section only deletes
  ; "${PRODUCTNAME}.lnk").
  !insertmacro _PPROXY_REMOVE_LEGACY_LNK "$DESKTOP\pony-desktop.lnk"
  !insertmacro _PPROXY_REMOVE_LEGACY_LNK "$SMPROGRAMS\pony-desktop.lnk"
!macroend
