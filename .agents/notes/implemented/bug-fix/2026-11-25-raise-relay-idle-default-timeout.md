# Agent Note: 上调 CONNECT relay 默认空闲超时至 300s（避免 LLM 长思考 SSE 断流）

Status: implemented

## Problem

生产排查（2026-11-25，Antigravity CLI 会话 `28098853-550c-42d0-92a6-096a98e1b980`）：大上下文（~270k tokens）与高思考档位请求经 `pproxy` CONNECT 隧道访问 `daily-cloudcode-pa.googleapis.com:443` 时，在 prefill 或思维链静默窗口超过 30s 后频繁触发 `pproxy_server::connect` 强断（`reason="relay_idle_timeout"`），导致客户端连续收到 9 次 `stream reading error: unexpected EOF` 后会话中断。

根因：`crates/server/src/connect.rs` 将 `RELAY_IDLE_DEFAULT_MS` 设为 `30_000`（30s），源于旧记录 `.agents/notes/implemented/bug-fix/2026-10-09-fix-accept-loop-saturation-hang.md` 按普通外部调用 3s × 10 取值；而出口侧 `crates/gate-server/src/connect_proxy.rs` 的 `IDLE_TIMEOUT` 早已为 `300s`（5 min），入口侧 30s 默认阈值显著短于长上下文 LLM SSE 的首包前与思考停顿窗口。

## Decision

1. 将 `crates/server/src/connect.rs` 的 `RELAY_IDLE_DEFAULT_MS` 从 `30_000`（30s）上调为 `300_000`（300s / 5 min），与 `crates/gate-server/src/connect_proxy.rs` 的 `IDLE_TIMEOUT = 300s` 对齐。
2. 保留 `PPROXY_RELAY_IDLE_TIMEOUT_MS` / `PPROXY_RELAY_ABSOLUTE_TIMEOUT_MS` 环境变量覆盖语义与 6h 绝对上限不变，继续保证死隧道（CLOSE-WAIT / 双向静默）在有界时间内释放 permit。
3. 在 `crates/server/src/connect.rs` 增加单测校验默认空闲超时为 300s（`300_000` ms），并同步更新 `crates/server/README.md` 缺省值表及回链 `.agents/notes/implemented/bug-fix/2026-10-09-fix-accept-loop-saturation-hang.md`。

## Alternatives considered

- **A 仅在 systemd drop-in 中覆盖 `PPROXY_RELAY_IDLE_TIMEOUT_MS=300000`、不改源码默认值**：本机服务可立即缓解，但重装、换机部署或直接运行 `pproxy serve` 时会回归 30s 默认断流，维护成本转移给运维环境；否决。
- **B 移除 `relay_idle_timeout`、仅保留 6h 绝对上限**：会导致死隧道最长持 permit 达 6 小时，重新暴露 `2026-10-09` 256 并发许可耗尽饱和风险；否决。
- **C 折中设为 60s 或 120s**：在 270k+ 上下文叠加高思考停顿场景下仍可能触发误杀（实测单次 prefill + 思考静默可逼近/超过 60–90s），且与 `gate-server` 既有 300s 空闲预算不一致；否决。

## Consequences

- 大上下文 LLM SSE 在 300s 内的首包等待与思考停顿不再被 `pproxy` 误杀，且无需依赖外部 systemd 环境变量覆盖即可默认生效。
- 死隧道在 5 分钟双向无字节窗口或 6 小时绝对上限内仍可确定释放 permit（`PPROXY_MAX_CONNECTIONS=256` 配合 fast-fail 503 与看门狗兜底仍有效）。
- 部分取代 `.agents/notes/implemented/bug-fix/2026-10-09-fix-accept-loop-saturation-hang.md` 中 `RELAY_IDLE_DEFAULT_MS` 默认 30s 的取值，其余 accept 循环与有界生命周期语义保持不变。
- 机器验证：`bash .agents/skills/write-adr/verify-note.sh` 与 `cargo test -p pproxy-server` 均需 exit 0。
