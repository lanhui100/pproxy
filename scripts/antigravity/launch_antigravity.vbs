' =========================================================================
' Antigravity 免 TUN 静默启动脚本 (VBScript)
' 纯后台静默执行，无控制台黑框闪烁，进程级环境变量继承
' =========================================================================
Option Explicit

Dim WshShell, fso, appDir, agExe, proxyHost, proxyPort, proxyUrl, cmdLine, procEnv

Set WshShell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")

' --- 代理配置 (Pony Proxy 默认 18900，Clash 默认 7890，v2rayN 默认 10809) ---
proxyHost = "127.0.0.1"
proxyPort = "18900"
proxyUrl  = "http://" & proxyHost & ":" & proxyPort

' --- 定位 Antigravity.exe 路径 ---
appDir = fso.GetParentFolderName(WScript.ScriptFullName)
agExe = appDir & "\Antigravity.exe"

If Not fso.FileExists(agExe) Then
    agExe = WshShell.ExpandEnvironmentStrings("%LOCALAPPDATA%") & "\Programs\antigravity\Antigravity.exe"
End If

If Not fso.FileExists(agExe) Then
    MsgBox "未找到 Antigravity.exe，请检查安装路径：" & vbCrLf & agExe, vbCritical, "启动错误"
    WScript.Quit 1
End If

' --- 注入当前进程环境变量 (派生出的所有子进程/语言服务器完整继承) ---
Set procEnv = WshShell.Environment("Process")
procEnv("HTTP_PROXY")  = proxyUrl
procEnv("HTTPS_PROXY") = proxyUrl
procEnv("ALL_PROXY")   = proxyUrl
procEnv("http_proxy")  = proxyUrl
procEnv("https_proxy") = proxyUrl
procEnv("all_proxy")   = proxyUrl
procEnv("GRPC_PROXY")  = proxyUrl
procEnv("grpc_proxy")  = proxyUrl
procEnv("NO_PROXY")    = "localhost,127.0.0.1,::1,*.local"
procEnv("no_proxy")    = "localhost,127.0.0.1,::1,*.local"

' --- 构造启动命令 (同时注入 Chromium 内核代理配置) ---
cmdLine = """" & agExe & """ --proxy-server=""" & proxyUrl & """ --proxy-bypass-list=""<local>;localhost;127.0.0.1;::1"""

' --- 启动 Antigravity (正常激活 GUI 窗口，VBS 自身无控制台窗口) ---
WshShell.Run cmdLine, 1, False
