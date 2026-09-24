# pproxy-cli

`pproxy` 命令行工具入口，负责 CLI 参数解析、环境代理控制（on / off / env / status）、后台服务启停与运维诊断。

## 配置与使用

- 主要入口文件：`src/main.rs`
- 交互命令：
  - `pproxy on` / `pproxy off`：切换系统/终端环境代理配置。
  - `pproxy status`：检查本地代理及多节点出口连通性。
  - `pproxy env`：打印或导出代理环境变量。

## 依赖关系

依赖 `pproxy-core`（配置与状态类型）与 `pproxy-server` / `pproxy-engine`。
