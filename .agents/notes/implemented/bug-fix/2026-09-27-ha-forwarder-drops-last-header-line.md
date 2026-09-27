# Agent Note: Local HA Forwarder 转发时静默丢弃末行请求头（POST body 丢失）

Status: implemented

## Problem

`LocalHaForwarder::handle_conn` 经 `sanitize_and_inject_ticket` 重建请求头时，
`while let Some(pos) = cursor.windows(2).position(|w| w == b"\r\n")` 循环只消费
**带尾随 `\r\n`** 的行；循环结束后剩余的最后一行（无尾随 `\r\n`，因为
`find_header_end` 定位的是 `\r\n\r\n` 的起点）被直接丢弃，随后仅追加一个
`\r\n` 收尾。

后果：**每一个经 Forwarder 转发的请求都会丢掉最后一个请求头**。POST 请求中
`Content-Length` 常居末位（reqwest/hyper、curl 均如此）→ 上游（腾讯 serve →
vedge 边缘 → opencode.ai）收到没有 body 的 POST → 后端 JSON 解析不到
`model` 字段 → 返回 `{"type":"ModelError","message":"Model  is not supported"}`
（model 为空）。GET /models 无 body 不受影响，故数据面"看起来通"。

实证（2026-09-27，腾讯节点 100.105.241.39:8899 转发器）：
- `Content-Length` 居末 → 后端 401 `Model  is not supported`（body 丢失）；
- `Content-Length` 手工置于首位（被丢的变成无害的 `Proxy-Authorization`）→
  后端 403 `FreeTierError`（body 正常送达，仅因裸请求缺 opencode 工具集被拒）。

## Decision

修复 `sanitize_and_inject_ticket`（`crates/core/src/ha_forwarder.rs`）：循环结束后，
若 `cursor` 非空，把剩余末行以与循环内相同的"票证头剔除检查"逻辑写回
（追加 `\r\n`），再注入官方集群票证、追加终止 `\r\n`。新增两个单测锁定：
`sanitize_preserves_last_header_line`（末行 `Content-Length`/`X-Fake` 必须保留、
终止符完整）与 `sanitize_strips_ticket_header_anywhere_including_last_line`
（末行为伪造票证头时剔除、官方票证正确注入）。

## Alternatives considered

- **A（采用）：修正 forwarder 头部重建逻辑**。改动最小（一行循环后补齐末行），
  根因明确、有字节级实证，修复后所有经 forwarder 的请求（含 k3s Pod 的
  opencode-zen POST、桌面端转发）都受益。
- **B（否决）：绕过 forwarder，让 Pod 直连 serve**。腾讯 8899 的 serve 数据面
  已让位给 forwarder（`--local 127.0.0.1:18899`），改拓扑违背"forwarder 作
  为鉴权边界"的设计，且不修桌面端同类问题。
- **C（否决）：在 ponyllm 侧规避（强制 Content-Length 前置）**。reqwest 无法
  控制请求头顺序，且只治标不治本（GET 之外的任何末位关键头仍会丢）。

## Consequences

- 修复后腾讯链路的 POST 请求 body 正常送达上游；k3s Pod 的 muse-spark
  （opencode-zen，经 forwarder 路径路由）恢复可用（配合腾讯节点 opencode 路由
  注册与 Pod 配置修正，见 ponyllm 工作区
  `.agents/notes/implemented/bug-fix/2026-09-27-…`）。
- 需要把新二进制部署到所有运行 forwarder 的节点（腾讯 k3s 出口、
  Windows 桌面端 LocalHaForwarder），并重启服务生效。
- 验证命令（机械可查、非零退出）：见部署后在腾讯节点执行的
  `curl` 对照实验（Content-Length 居首 vs 居末的结果差异消失）；
  `cargo test -p pproxy-core ha_forwarder` exit 0。
