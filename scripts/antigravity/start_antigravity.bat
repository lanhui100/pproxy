@echo off
chcp 65001 >nul
title Antigravity Launcher
cd /d "%~dp0"

:: =========================================================================
:: Antigravity 免 TUN 模式启动脚本 (BAT)
:: =========================================================================
:: 核心原理:
:: Antigravity 底层基于 Electron + Language Server (gRPC/Node) 架构。
:: 默认不走系统代理，通常需要全局 TUN 网卡模式接管所有流量。
:: 本脚本通过在启动主进程前注入代理环境变量与 Chromium 命令行参数，
:: 使得主进程与派生的所有子服务（含语言模型通信进程）均强制走本地代理。
:: =========================================================================

:: [代理配置] - 可根据你使用的代理客户端修改端口:
:: Pony Proxy: 18900 / 8899
:: Clash / Mihomo / Clash Verge: 7890
:: v2rayN: 10809 (HTTP) / 10808 (SOCKS)
set PROXY_HOST=127.0.0.1
set PROXY_PORT=18900
set PROXY_URL=http://%PROXY_HOST%:%PROXY_PORT%

echo [Antigravity Launcher] 正在注入代理环境变量: %PROXY_URL%

:: 1. 注入通用 HTTP/HTTPS 代理环境变量 (涵盖大小写，供 CLI、Node、Git 识别)
set HTTP_PROXY=%PROXY_URL%
set HTTPS_PROXY=%PROXY_URL%
set ALL_PROXY=%PROXY_URL%
set http_proxy=%PROXY_URL%
set https_proxy=%PROXY_URL%
set all_proxy=%PROXY_URL%

:: 2. 注入 gRPC 代理环境变量 (Google 语言服务及模型底层通信关键)
set GRPC_PROXY=%PROXY_URL%
set grpc_proxy=%PROXY_URL%

:: 3. 绕过本地回环与局域网 (防止本地 IPC 通信死锁)
set NO_PROXY=localhost,127.0.0.1,::1,*.local
set no_proxy=localhost,127.0.0.1,::1,*.local

:: 4. 定位 Antigravity.exe 路径
set "AG_EXE=%LOCALAPPDATA%\Programs\antigravity\Antigravity.exe"

if not exist "%AG_EXE%" (
    echo [错误] 未在以下路径找到 Antigravity.exe:
    echo "%AG_EXE%"
    pause
    exit /b 1
)

:: 5. 启动 Antigravity 桌面端 (注入 Chromium 代理参数)
echo [Antigravity Launcher] 正在启动主程序...
start "" "%AG_EXE%" --proxy-server="%PROXY_URL%" --proxy-bypass-list="<local>;localhost;127.0.0.1;::1" %*
