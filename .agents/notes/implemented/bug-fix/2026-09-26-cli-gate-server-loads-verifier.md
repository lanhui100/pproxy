# Agent Note: CLI gate-server 子命令加载多租户验签器与撤销表

Status: implemented

## Decision

`pproxy gate-server` CLI 子命令路径（`crates/cli/src/main.rs` → `pproxy_gate_server::run_server`）
现在与独立 binary（`crates/gate-server/src/main.rs`）同口径：从环境读取
`USER_VERIFYING_KEY` 构造多租户 Ed25519 `TokenVerifier`，从 `GATE_ADMIN_TOKEN`
读取管理口令，启动时经 `load_revoked_tokens` 预加载 `~/.pony/revoked_tokens.txt`
到内存黑名单。实现位置：`crates/gate-server/src/lib.rs::run_server`。

## Alternatives considered

- **改 systemd 改调独立 binary**：`main.rs` 路径本就正确，但需改 RackNerd
  现有 `pony-gate-rust.service`（`ExecStart=/opt/pproxy/bin/pproxy gate-server`），
  还要把该 binary 纳入发布物。治标且引入部署变更，拒绝。
- **让 `run_server` 加 `verifier` 参数由 CLI 传**：需改 CLI 参数解析与所有调用点，
  但 env 注入本就是文档既定口径（`docs/ops/DEPLOY.md` §2：systemd `Environment=`），
  加参数与文档口径分叉，拒绝。
- **保持现状（仅独立 binary 支持多租户）**：systemd 实际跑的就是 CLI 子命令路径，
  等于生产永远 verifier=None，`usr_live_` 全 401。拒绝。

## Consequences

- systemd 无需改动：重启即生效，`USER_VERIFYING_KEY` 照旧从
  `/opt/pproxy/.gate-server.env` 的 `EnvironmentFile` 注入。
- 单令牌 `gate_` 回退路径不变（`verifier=None` 时仍走 `TUNNEL_TOKEN_HASH` 校验）。
- 配套验证命令：`cargo test -p pproxy-gate-server`（`test_gate_user_token_auth_flow`）。

---

## Follow-up: 桌面默认端点收敛到唯一多租户出口（同日实施）

`desktop/src-tauri/src/lib.rs`：`DEFAULT_TUNNEL_URLS`/`GATE_WS_URL` 收敛为
`wss://rn.ponygo.fun/ws`（唯一启用 `USER_VERIFYING_KEY` 的出口）；含
`example.com` 占位或 `ponyjob.top` 单令牌域名的旧配置经 `migrate_tunnel_url`
统一收敛；`resolve_gate_url_for_iface("vercel"/"cf")` 回退同样指向 rn；
无 token 拨测 TCP 目标改为 `rn.ponygo.fun:443`。理由：Vercel/CF Node gate
仅认 `TUNNEL_TOKEN_HASH`，任何把 `usr_live_` 路由到它们的默认都会 401，
端点优先级再正确也无意义。UI 不展示端点（维持用户"填 token 即用"体验）。
