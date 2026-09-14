# T004 — 桌面端隧道 token 反复丢失（重启/一段时间后重输，双出口齐挂）

- 状态：Done（双 reviewer 回改完成，全量门禁绿；v0.3.48 发版中）
- 复杂度：B（跨 Rust 凭据持久化 / 引擎 / 池 / 前端链路 / 运维取证）
- 创建：2026-09-14
- 负责人：主会话 orchestrator + 4 路诊断小队（已完成诊断）

## 背景

桌面端正确输入隧道 token 后，一段时间后或重启后需重新输入，否则 CF + Vercel 双出口齐失败。
四路并行诊断已完成（凭据持久化 / 引擎自愈池 / 前端链路 / 服务端轮换），结论见本次最终答复。

## 目标

1. 重启后不再因本地分叉/毒化 watch 丢失 token。
2. 服务端轮换后客户端给出明确“请重输”而非静默 502。
3. 自检/探针口径统一，`kind` 可区分 auth401 / denied / timeout。
4. 前端不再把“有值”报成“可用”。

## 范围 / 非目标

- 范围：`desktop/src-tauri/src/lib.rs` 凭据与 watch、`desktop/src-tauri/src/proxy/engine_tunnel.rs`、自愈、`crates/transport` 池/探针、`desktop/src` 前端链路、`docs/ops/TROUBLESHOOTING.md`。
- 非目标：服务端 hash 轮换流程本身（只补文档与取证口径）；chained 模式；更新分发。

## 验收标准（可验证）

1. `cargo test -p pony-desktop` 通过（含新增用例：拆分加载、原子写入、分叉多代 bak、回读校验、池 token 标签、双错保留）。
2. `cargo test --workspace` 通过；`cargo clippy --all-targets` 0 警告。
3. `desktop` 侧 `pnpm test`（vitest 102+ 新增通过）、`vue-tsc --noEmit` 0 错、`oxlint` 0 警告。
4. 重启复现：keyring 瞬时锁 / tunnel.json 缺失时引擎不被毒化为 `(None,None)`，UI 与引擎口径一致。
5. 轮换复现：磁盘无新值时 401 路径给出明确重输提示并保留双端点错误，不静默。

## spec

`docs/dev-team/specs/S004-tunnel-token-reloss.md`

## 审核记录

- 诊断轮：4 路子智能体独立诊断（结论/P0-P3/假设/边界/修复/取证），编排器交叉复核 tungstenite `Http` Display 与 `AUTH_401_MARKER` 口径。
- 实现后：2 路独立 code reviewer（正确性/边界）+ 测试门禁。

## Next Action / Resume Hint

- 按 spec P0 分批实现 → 双 reviewer → 全量门禁。
- Resume：读本卡 + spec + `git status`，继续未完成批次。
