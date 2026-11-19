# Agent Note: CLI 连通性探测放宽超时至 5s 并过滤占位符数据面域名

Status: implemented

## Problem

执行 `pproxy status` 时，连通性卡片报错：
```
│  🌐 正向出海 Google  - 出海隧道未通 (网络/DNS异常)
│  🌐 正向出海 GitHub  - 出海隧道未通 (网络/DNS异常)
```
排查发现两处缺陷：

1. **未改占位符污染探测**：`~/.pony/config.toml` 中残留开源占位域名 `data_plane = "https://edge.example.com"`。`derive_data_plane` 盲目信任非空字段，导致探针客户端将请求发往不可达的 `edge.example.com`。
2. **探测超时阈值偏窄**：`crates/cli/src/cmd/proxy_env.rs` 中探测客户端超时硬编码为 `2000ms`（2.0s）。国内到美区 VPS 往返 RTT 约 280ms，冷建连或网络抖动耗时在 2.1~2.5s 时即被判定为 `Gate 隧道超时 (上游无响应)`。

## Decision

1. **过滤占位符域名**（`crates/cli/src/config.rs`）：`derive_data_plane` 判定显式 `data_plane` 时增加 `!trimmed.contains("example.com")` 检查。若为占位符域名则自动忽略并回退到 `derive_from_server`（使用本地或服务器推导的真实端口 `8899`）。
2. **放宽探测超时至 5000ms**（`crates/cli/src/cmd/proxy_env.rs`）：将 probe client 的超时从 2000ms 提升至 5000ms，与 `doctor.rs`（`Duration::from_secs(5)`）的探测规范保持一致。
3. **补充单元测试**：在 `crates/cli/src/config.rs` 中补充包含 `https://edge.example.com` 时的自动回退测试用例。

## Alternatives considered

- **维持 2000ms 超时**：跨洋链路在 RTT 抖动时极易偶发突破 2s，造成频繁误报，用户体验差。否决。
- **直接报错退出（fail-closed）**：若检测到 `example.com` 直接抛错拒绝启动。但作为客户端工具，对未配置占位符做平滑回退体验更好，且 `server` 地址可推导出有效数据面。否决。

## Consequences

- 开源用户在未修改 `data_plane` 占位符时，CLI 连通性探测仍能正确定位到本地 `8899` 代理。
- 探测卡片容忍偶发的跨洋长尾网络波动。

## 实测收据

```bash
# 单测
cargo test -p pproxy-cli  → 142 passed; 0 failed

# 生产 CLI 状态验证
target/release/pproxy status
# ┌─ 代理连通性状态 (Connectivity) ──────────────────────
# │  🔀 反向路由网关: ✓ 7 条路由已就绪 (OpenAI/Anthropic/GitHub 等)
# │  🌐 正向出海 Google  ✓ 连通 (1217 ms)
# │  🌐 正向出海 GitHub  ✓ 连通 (1687 ms)
# └────────────────────────────────────────────────────
```
