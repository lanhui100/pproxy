# 性能专项：请求响应 1-2s → <1s 优化（收口记录）

日期：2026-02（会话记录）｜ 分级：B（标准完整路径，简化执行）｜ 状态：Done

## 归因结论

本地热路径（token verify / route resolve / usage 计数）经核查均为纯内存微秒级，
**不是**延迟来源。主因经定量分析定位：

1. **CONNECT 隧道冷建连 ≈7 RTT**（TCP 1 + TLS 2 + WS Upgrade 1 + 首帧 1 + 隧道内目标 TLS 2），
   跨洲 250ms RTT 下 ≈1.75s —— 与"普遍 1-2s"定量吻合（主因）。
2. 入站 socket 无 TCP_NODELAY（hyper 手动 serve_connection / tokio-tungstenite 均不设），
   Nagle + delayed-ACK 每次交互 40-200ms。
3. Vercel proxy.js 响应流每 chunk 一次 setImmediate 让出，且无背压。
4. reqwest 连接池空闲 90s 后冷 TLS 握手（~2-3 RTT），仅影响稀疏流量。

已排除/删除项（对抗审核结论）：pool.rs 排序、edge.rs Url 缓存、relay 缓冲区放大
（微秒级噪声或死代码）；CF worker 请求体流式化（与 redirect:follow 冲突，307 会挂）。

## 变更清单

| 文件 | 变更 |
|---|---|
| `crates/server/src/connect.rs` | 新增 `TunnelPool` 待命 WS 池（预建 upgrade-only 会话，checkout 时首帧绑定目标；establish ~5 RTT→1 RTT）；establish 拆分 `connect_ws`/`bind_target`；connect_ws 10s 超时；establish_ms/pooled 计时日志；3 个新测试；修复 1 个预存陈旧断言 |
| `crates/server/src/gateway.rs` | 入站 accept 后 `set_nodelay(true)`；`GatewayState.tunnel` 类型改为 `Option<Arc<TunnelPool>>` |
| `crates/server/src/main.rs` | TunnelPool 装配 + `PPROXY_TUNNEL_POOL=0` 回滚开关；edge 保活任务（`PPROXY_EDGE_KEEPALIVE=1`，45s，含 Vercel 计费提示） |
| `crates/engine/src/server.rs` | 入站 accept 后 `set_nodelay(true)` |
| `crates/core/src/edge.rs` | 新增 `keepalive_ping`（读尽 body 归还连接；不走 execute 避免重试逻辑） |
| `deploy/vercel/api/proxy.js` | 去 per-chunk setImmediate；drain/close 竞速背压；`flushHeaders()`（SSE 首 token） |

## 审核记录

- 计划阶段：2 独立 reviewer 对抗审核（均有条件通过）。采纳：保活读尽 body、
  保活挂载点移至装配处+env 开关、删微优化项、CF 流式 body 放弃、flushHeaders 替代删 yield。
- 代码阶段：reviewer A 完整审查（有条件通过，无 P0）。采纳：proxy.js drain/close 竞速（P2 必修）、
  测试 accept/upgrade 计数竞态（P2 必修，改 idle_len 断言）、connect_ws 超时（P2）、
  TTL 50s→30s（P1）、失败日志补 pooled 字段、池 env 开关、keepalive 成本提示。
  未采纳：非阻塞 poll 探测死会话（复杂度>收益，TTL 30s + Network 兜底已覆盖）。
- reviewer B 两次启动失败（工具层面），按降级处理：其审核维度（失败路径/资源）已由
  计划阶段 reviewer B + 代码 reviewer A 的对应条目覆盖。

## 测试证据

- `cargo test -p pproxy-server -p pproxy-core -p pproxy-engine`：129 passed / 0 failed
  （含新增：池预建、池化路径 200、死会话 Network 兜底）。
- `node --check deploy/vercel/api/proxy.js` 通过。
- clippy 无新增告警。
- 预存失败（与本次无关，未修复）：`pproxy-cli` `sync_roundtrip_url_safe_and_replay_protection`
  ——固定 nonce 撞本机持久化重放缓存，环境依赖；`from_pool_config_derives_zero_config_tunnel`
  陈旧断言已顺带修复（edge→gate 重定向为既有预期行为）。

## 残余风险 / 后续项

- 池会话 checkout 后恰被 gate 回收的 TOCTOU 不可消除，由 Network 重试兜底（最坏 = 旧行为 +400ms）。
- 池对 CF gate 常驻 2 条 WS（每 ~30s 轮换），有持续连接时长计费；可用 `PPROXY_TUNNEL_POOL=0` 回退。
- engine crate 的 CONNECT 协议（header 声明目标）与已部署 gate worker（首帧声明）分叉，
  属预存技术债，本次未触及；engine 侧用户若走 CONNECT 需先修协议再谈池化。
- 验收建议：部署后对同一目标对比 `establish_ms` 日志（pooled=true 应 ≈1 RTT）与
  端到端 curl 计时；若目标站本身响应 >1s（如 LLM 首 token），代理侧优化无法压缩该部分。
