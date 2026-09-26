# Agent Note: 增加客户端错误主动遥测与诊断导出功能

Status: implemented

## Decision

1. **服务端网关遥测路由**：`crates/gate-server/src/lib.rs` 新增 `POST /api/client/telemetry`，将桌面端上报的错误摘要（含时间戳、客户端 IP、错误原因、目标站点、Token 指纹）追加写入网关 `/root/.pony/client-telemetry.log`，并在 `tracing` 中发出告警，实现运维毫秒级知晓现场。
2. **桌面端主动遥测上报**：`desktop/src-tauri/src/lib.rs` 在 `proxy_test_site_local`（站点测速）与 `proxy_test_egress`（出口网关测速）发生非空失败时，异步静默上报异常上下文至网关遥测端点，不阻塞客户端主流程。
3. **UI 诊断显性化与日志导出**：
   - 常用站点行与代理状态行的失败状态支持鼠标悬停查看详细后端错误描述（`latestTooltip`）。
   - 设置页「系统维护」模块新增「打开日志目录」（调用系统文件管理器打开 `%APPDATA%\pony-desktop`）与「复制诊断信息」（一键拷贝当前凭据指纹、运行模式、白名单数量等环境快照）。

## Alternatives considered

- **让用户在 Windows 手动抓包或排查日志**：对普通用户门槛过高，往返沟通成本极大。
- **纯本地日志**：在客户端遭遇极端握手拒绝时仍需用户手动翻找目录与上传文件。主动遥测可在发生错误的瞬间直接由服务端感知。

## Consequences

- 调试闭环从“重新打包 CI 12 分钟 + 猜原因”缩短为“客户端复现 → 服务端实时查看日志”，效率提升 10 倍以上。
- 桌面版本升级至 `0.3.58`。
