# Agent Note: 彻底修复待命池僵死连接假活与超时不对齐导致的 Connection Error

Status: implemented

## Problem

在首轮修复数据面 Token 分离后，生产环境在调用 `gemini-3.8-flash`（目标域 `daily-cloudcode-pa.googleapis.com`）时，仍然偶发连接超时与 `Connection error`。深入挖掘日志后发现系统性假死与误报：

1. **Gate 服务端与客户端待命池生命周期契约倒挂**：
   - Gate 侧（`crates/gate-server/src/lib.rs`）等待首帧目标声明硬编码为 10 秒（`timeout(10s, ws_receiver.next())`），空闲 10 秒未声明即单方面 Close；
   - 客户端连接池（`crates/transport/src/pool.rs`）默认空闲 TTL（`DEFAULT_IDLE_TTL`）配置为 30 秒；
   - 导致空闲超过 10s 但不足 30s 的池化会话在对端早已关闭，客户端取出时 100% 成为死连接。
2. **死连接卡满首帧超时与吞异常掩盖问题**：
   - 取出死连接后发送目标声明，因对端已关闭或不回复，客户端硬等待 `FIRST_FRAME_TIMEOUT`（5 秒）直至超时报错 `gate bind failed ... error=network: first-frame timeout`；
   - `crates/server/src/connect.rs` 在 `bind_res.is_err()` 时静默吞掉错误并降级冷建连，外层日志仍误报 `pooled = true`（如 `establish_ms=1505 pooled=true` 实际是假池化 + 冷建连）；
   - 叠加跨国冷建连（1.5s），使得单次建连耗时达 6.5 秒以上，直接击穿上游调用方（ponyllm）4 秒的 TTFB 预算，导致调用方抛出 `Connection error`。
3. **待命池容量过窄（`POOL_SIZE = 2`）**：
   - 并发突发或重试时连接池瞬间被抽空，所有后续请求退化为 1.5s~2s 的跨洋冷建连。

## Decision

1. **Gate 服务端生命周期放宽**：
   - `crates/gate-server/src/lib.rs` 等待首帧超时从 10 秒提升至 60 秒（`Duration::from_secs(60)`），给客户端待命池充分的留存窗口。
2. **客户端连接池生命周期安全对齐**：
   - `crates/transport/src/pool.rs` 将 `DEFAULT_IDLE_TTL` 设为 25 秒（严格小于 Gate 的 60 秒，物理上杜绝取出对端已超时的会话）；
   - `POOL_SIZE` 由 2 提升至 4，增强突发并发吸收能力。
3. **超时预算与异常处理真实化**：
   - `crates/transport/src/proto.rs` 将 `FIRST_FRAME_TIMEOUT` 从 5000ms 放宽至 8000ms（与 `DIAL_TIMEOUT = 8000ms` 对齐），防止跨国网络抖动误杀；
   - `crates/server/src/connect.rs` 修正 `used_pool` 统计：池化绑定失败回退冷建连时显式记录 warn 日志，并将 `used_pool` 置为 `false`，彻底消除假指标。
4. **即时部署与生效**：
   - 重新编译 release 二进制，将 `pproxy-gate-server` 部署并热重启至 RackNerd VPS；
   - 本机热重启 `pproxy.service`。

## Alternatives considered

- **A（仅缩减客户端连接池 TTL 至 8 秒）**：虽能避开 Gate 10 秒超时，但会造成连接池频繁重建（每 8 秒轮换），浪费跨国握手资源，且遇到网络抖动仍易踩中 10 秒边界；改由 Gate 放宽至 60s 根治。
- **B（禁用连接池，全量走冷建连）**：每次请求都需承受 1.5s~2s 跨洋握手，严重降低大模型首 Token 响应速度，违背待命池消除冷建连开销的设计初衷。

## Consequences

- 彻底根除 `error=network: first-frame timeout` 假死告警；
- 空闲后重新发起请求时，连接池真实命中，建连耗时从 1500ms~5000ms 下降至 240ms~290ms；
- 6 并发及空闲唤醒测试均稳定在 0.8s~2.3s 内返回，不再超时；
- `gemini-3.8-flash` 真实端到端推理请求成功返回。
