<#
.SYNOPSIS
    pproxy 构建产物与缓存清理脚本
.DESCRIPTION
    清理根目录与 desktop/src-tauri 的双构建图 target 目录，并显示释放空间。
#>
[CmdletBinding()]
param(
    [switch]$IncludeNodeModulesCache
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot

function Get-DirSizeMB ($path) {
    if (Test-Path -LiteralPath $path) {
        $measure = Get-ChildItem -LiteralPath $path -Recurse -Force -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum
        if ($measure.Sum) {
            return [math]::Round($measure.Sum / 1MB, 2)
        }
    }
    return 0
}

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  pproxy 构建产物清理工具" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

$rootTarget = Join-Path $projectRoot "target"
$desktopTarget = Join-Path $projectRoot "desktop\src-tauri\target"

$rootSizeBefore = Get-DirSizeMB $rootTarget
$desktopSizeBefore = Get-DirSizeMB $desktopTarget
$totalBeforeMB = $rootSizeBefore + $desktopSizeBefore

Write-Host "[1/3] 正在清理根目录 Rust 产物 (当前: $rootSizeBefore MB)..." -ForegroundColor Yellow
Push-Location $projectRoot
try {
    cargo clean
} finally {
    Pop-Location
}

Write-Host "[2/3] 正在清理桌面端 Rust 产物 (当前: $desktopSizeBefore MB)..." -ForegroundColor Yellow
$desktopTauriDir = Join-Path $projectRoot "desktop\src-tauri"
Push-Location $desktopTauriDir
try {
    cargo clean
} finally {
    Pop-Location
}

if ($IncludeNodeModulesCache) {
    $viteCache = Join-Path $projectRoot "desktop\node_modules\.vite"
    if (Test-Path -LiteralPath $viteCache) {
        Write-Host "[可选] 正在清理 Vite 缓存..." -ForegroundColor Yellow
        Remove-Item -LiteralPath $viteCache -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "[3/3] 统计清理结果..." -ForegroundColor Green
Write-Host "----------------------------------------"
Write-Host ("已释放构建缓存: {0:N2} MB ({1:N2} GB)" -f $totalBeforeMB, ($totalBeforeMB / 1024)) -ForegroundColor Green

$drive = Get-PSDrive D -ErrorAction SilentlyContinue
if ($drive) {
    $freeGB = [math]::Round($drive.Free / 1GB, 2)
    Write-Host ("当前 D 盘剩余可用空间: {0} GB" -f $freeGB) -ForegroundColor Cyan
}
Write-Host "========================================" -ForegroundColor Cyan
