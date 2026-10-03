# Agent Note: 管理员 Token 豁免出海网关字节配额熔断

Status: implemented

## Problem

`crates/gate-server/src/lib.rs` 原先只用 `is_admin` 豁免并发 `max_conns`，握手阶段仍对所有 `usr_live_` claims 执行 `used >= quota_bytes` → HTTP 402，双向流式阶段也统一按 `claims.quota_bytes` 熔断。结果 role=admin/name=admin 的管理员账号仍会在字节用量达到阈值后被 402/4402 阻断，与运维预期“管理员不限额”不一致。

## Decision

- 新增 `is_admin_claims` / `quota_limit` 两个判定入口：`role == "admin"`、`name == "admin"` 或 `name` 以 `admin_` 开头视为管理员。
- 握手阶段、`/api/user/profile` 状态、流式上下的 `quota_up` / `quota_down` 统一经 `quota_limit`：管理员返回 `None`，即不做 402 与 WS 4402 字节熔断；普通用户维持原 `claims.quota_bytes` 限额。
- 管理员并发限制继续保持现有豁免语义；普通用户并发规则不变。

## Alternatives considered

- **只在签发时给 admin 配置一个极大 `quota_bytes`**：不改代码，部署简单，但会把“不限额”混入租户账本；真实超大值在内存累计器/任意 u64 边界上仍非语义化的不限额，且运营视图会继续显示“quota”。
- **按 `sub`/`name` 硬编码一批免配额账号**：命中当前用户最快，但会把运维名单散进代码，轮换难、测试难；`role=admin` 已有语义通道，应收口到 token claims。
- **让 admin 只豁免握手 402、仍保留流式 4402**：实现更小，但会让连接已通过鉴权后再被踢掉，表现为随机中断，难排查；配额语义应在连接建立前后一致。

## Consequences

- 管理员 token 不再因字节累计触发握手 402 或 WS 4402；普通用户配额语义不变。
- `used_bytes` 对 admin 仍可被累计用于观测，但不再作为熔断条件。
- 机械验收：`cargo test -p pproxy-gate-server` 新增 `test_admin_token_ignores_quota_limit` 与原 `test_gate_user_token_auth_flow` 均通过。
