# Agent Note: YouTube 流量自动分流至 muse 专属中继出口

Status: implemented

## Problem

正向代理隧道（CONNECT）此前全部流量（含 YouTube 流媒体）默认由 RackNerd VPS（出口R）出海：
1. 出口R 具有 **500 GB/月**的硬件硬顶（实测已消耗超 44%），YouTube 视频流量持续增长容易耗尽月额度或因高并发触发 429；
2. 尝试全量改走 Cloudflare Workers 免费网关（出口C）面临每日 100,000 请求数账号硬顶及长连接大文件吞吐受限（0.02–1.45 Mbps 伴随 1006 断流）；
3. 远程内网机器 `muse`（`htch-runtime`）在放行 YouTube 家族白名单后，具备稳定的 YouTube/GoogleVideo 出海能力，且经由 Cloudflare Access SSH 隧道与本机互通，不经过 RackNerd，不消耗 VPS 流量。

## Decision

1. **识别层（`crates/transport/src/route.rs`）**：
   - 增加 `is_youtube_host` 判定函数，识别 `youtube.com`、`googlevideo.com`、`ytimg.com`、`youtu.be`、`youtube-nocookie.com`、`ggpht.com` 及其子域。
2. **数据面分流（`crates/server/src/connect.rs` 与 `crates/engine/src/connect.rs`）**：
   - 在处理 CONNECT 请求时，若命中 `is_youtube_host` 且配置了环境变量 `PPROXY_YOUTUBE_PROXY`，则绕过标准待命连接池，直接建立指向该专用代理的 TCP 隧道（支持 SOCKS5 与 HTTP CONNECT）；
   - 若专用代理握手失败，平滑降级（fallback）至原有正向隧道通道，确保业务不中断。
3. **隧道保活守护（`scripts/muse-tunnel-watchdog.sh`）**：
   - 本地运行轻量级看门狗脚本，检测到本地 `127.0.0.1:10880` 异常时，通过 `ssh -o ServerAliveInterval=15 -N -f -L 127.0.0.1:10880:198.19.0.1:3128 muse` 自动拉起端到端隧道。
4. **环境注入**：
   - `.pproxy.env` 声明 `PPROXY_YOUTUBE_PROXY=http://127.0.0.1:10880`。

## Alternatives considered

1. **让 YouTube 走 Cloudflare Workers（出口C）**：落选。实测 CF 免费版长连接转发大文件易遇 CPU 限制/1006 异常断开，且每天 100k 请求数容易被视频切片耗尽进而波及 API 网关。
2. **在 muse 上搭建公开公网 SOCKS5 服务**：落选。muse 处于严格隔离的容器沙箱内，无 root 权限，无公网入站 IP。
3. **仅靠本地 PAC/客户端分流**：落选。无法实现 `pproxy` 整体网络出口对移动端、开发终端及各种客户端的透明无感分流。

## Consequences

- YouTube 流量（含视频分段、首页、缩略图）全部自动漂移至 `muse` 出口，端到端耗费 RackNerd 流量归零；
- 非 YouTube 流量（OpenAI、Claude、开发 API）完全维持原有的 NativeVps 连接池机制，无任何行为漂移；
- 验收标准：单测 `cargo test -p pproxy-transport route::tests::test_is_youtube_host` 通过；实测 `curl -x http://127.0.0.1:8899 https://www.youtube.com/` 返回 200，日志确认命中 `routing youtube host to dedicated proxy`。
