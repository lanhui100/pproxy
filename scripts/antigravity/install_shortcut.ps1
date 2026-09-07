# =========================================================================
# 自动化安装/刷新 Antigravity 免 TUN 代理启动快捷方式
# =========================================================================
$ErrorActionPreference = "Stop"

$appDir = "$env:LOCALAPPDATA\Programs\antigravity"
$agExe = "$appDir\Antigravity.exe"
$vbsPath = "$appDir\launch_antigravity.vbs"
$batPath = "$appDir\start_antigravity.bat"

if (-not (Test-Path $agExe)) {
    Write-Error "未找到 Antigravity.exe: $agExe"
    exit 1
}

# 复制脚本到程序目录
$currentDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (Test-Path "$currentDir\launch_antigravity.vbs") {
    Copy-Item "$currentDir\launch_antigravity.vbs" -Destination $vbsPath -Force
    Copy-Item "$currentDir\start_antigravity.bat" -Destination $batPath -Force
    Write-Host "已同步启动脚本至: $appDir" -ForegroundColor Green
}

# 创建桌面快捷方式
$desktop = [Environment]::GetFolderPath("Desktop")
$shortcutPath = "$desktop\Antigravity (Proxy).lnk"

$wsh = New-Object -ComObject WScript.Shell
$sc = $wsh.CreateShortcut($shortcutPath)
$sc.TargetPath = "wscript.exe"
$sc.Arguments = "`"$vbsPath`""
$sc.WorkingDirectory = $appDir
$sc.IconLocation = "$agExe,0"
$sc.Description = "Antigravity (免 TUN 代理增强模式)"
$sc.Save()

Write-Host "桌面快捷方式已就绪: $shortcutPath" -ForegroundColor Cyan
