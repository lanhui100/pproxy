# S004 — 隧道 token 反复丢失修复规约

## 1. 背景与目标

同任务卡 T004。解决三类失效：重启后丢（本地分叉/毒化 watch）、一段时间后丢（内存冻结 + 服务端轮换不可自愈）、双出口齐挂误诊（混合 kind 被覆盖、探针口径分裂、前端误报可用）。

## 2. 方案总览

### Rust 持久化（`desktop/src-tauri/src/lib.rs`）

1. `data_dir()` 去静默 temp 回退：`APPDATA` 缺失时 Windows 直接 Err（调用方显错），并在启动日志打印一次绝对路径；`LOCALAPPDATA` 备选经实测后再定（本批先显错不断链：保留 `temp_dir` 仅当显式 `PONY_DESKTOP_ALLOW_TEMP_DIR=1`）。
2. 新增进程内 `CRED_LOCK`；`cred_set` fallback 改 tmp+rename；与 `meta` 同序写；失败若 keyring 已写成功返回“半写”分类错误（`half-written:keyring-ok,fallback-<err>`）。
3. 分叉仲裁引入新鲜度：比较 `cred_meta.last_write_ts`，fallback 更新鲜且来源为人工写入（`tunnel_token_save|connect_code_import|configure_direct_tunnel`）时不覆盖、只告警并保留多代 `.bak.<ts>`；`self_heal_diverged` 只在 keyring 更新鲜时覆盖。
4. `tunnel_token_save` 照抄 `configure_direct_tunnel` 回读校验，失败禁广播 watch。
5. `tunnel_config_load` 拆分为 `tunnel_url_load()` + `tunnel_token_load()` 独立返回；`ensure_tunnel_watch` 播种与各广播点对 Transient（keyring 瞬时错）保留旧值 + 脏标记，`tunnel.json` 缺失时 url 回退 `DEFAULT_TUNNEL_URLS` 而非整体 `(None,None)`；保留 `tunnel_config_load()` 做兼容壳。
6. `cred_delete` keyring 删除 Err 上抛；fallback 先 `.bak` 再删；新增原子命令 `tunnel_config_set{url?,secret?}`（一次校验、一次落盘、一次 watch），旧命令保留兼容。
7. `app_config_set` 改 tmp+rename。
8. 认证码/口令输入统一 trim（trim 后的值参与落盘、广播、指纹），杜绝首尾空白导致 hash mismatch。

### 引擎与池

9. `crates/transport/src/pool.rs`：`IdleSession` 加 `token_fp8` 标签；`checkout` 按 `(urls, expected_fp8)` 匹配，失配视为 miss；`maintain` 遇 401 连续 N 次熔断停建 + `log` 通道统一 + endpoint/fp8 打点；加 hit/miss/过期/熔断计数；`invalidate()` 同步排空。
10. `desktop/.../engine_tunnel.rs`：自愈 `send` 后同步 `pool.invalidate()`；磁盘无新值时返回明确 `needs-reinput` 分类错误（含 mem_fp8/disk_fp8/双端点逐端点错误），不再静默 502；401 日志补 egress+端点+fp8；保留双端点错误（首 401 + 末错），不只留 `last_err`。
11. 探针统一：`proxy_test_iface` 改用 `probe_via_gate` 打固定探针 host（`www.google.com:443`），`tunnel_self_check` gates 同时跑 Upgrade（沿用 `probe_gate_rtt`）与 bind（含首帧）两针并分别上报 `kind_upgrade/kind_bind` 兼容旧 `kind` 字段。

### 前端（`desktop/src`）

12. `provisionTunnel/localTunnelReady` 改有效性口径：`url && hasToken && !credError`；常态区展示 `credWinner/diverged` 徽标（不只自检区）。
13. 自检渲染 `kind`：`auth401→令牌无效请重贴；denied→门禁拒绝非token错；timeout/closed→网络；no_token→未配置`；`credError` 非空仍允许自检。
14. `clearTunnelToken` 改抛错并展示 `cleared` 时间线；URL 独立编辑后强制自检提示。
15. dev 通道加“开发模拟·恒绿”水印语义（`dev-mock` token + 自检标注 mock）。
16. 新增 `tunnelConfigSet`（调后端原子命令），`saveTunnelConfig/import` 切到它；失败后必 `refreshTunnel()` 并提示“端点可能已变更”。

### 文档

17. `docs/ops/TROUBLESHOOTING.md` 补隧道 401 章节（症状→自检三值→双冒烟→轮换时间线→H1/H2 分流），索引 09-01 编码事故与 `collect-401-evidence.mjs`。

## 3. 任务拆解与并行边界

- A（Rust 持久化）：2-8，可独立于 B/C 先行。
- B（引擎/池）：9-11，依赖 A 的拆分加载接口（先定函数签名再并行）。
- C（前端）：12-16，依赖 A 的原子命令与 kind 字段（先定契约再并行）。
- D（文档）：17，全程可并行。

## 4. 风险与回滚

- keyring/凭据行为变更风险：保留旧命令兼容壳；分叉仲裁默认仍 keyring 优先，仅在 fallback 明确更新鲜且人工来源时告警不覆盖。
- 回滚：按文件 revert；`tunnel.json/.dat/.bak/meta` 均保留多代，不删用户数据。

## 5. 测试计划

- Rust：拆分加载三态、原子写入撕裂、回读校验失败禁广播、分叉多代 bak、新鲜度反向告警、池 token 标签失配、熔断计数、双错保留、自愈 needs-reinput。
- 前端：有效性口径、kind 文案映射、原子命令失败刷新、dev-mock 标注。
- 全量：`cargo test --workspace`、`cargo clippy --all-targets`、`pnpm test/check/lint`、`contract-smoke`（若可跑）。

## 6. 验收标准

同任务卡 T004 五条。

## 7. 审核记录

- 待实现后双 reviewer 对抗审核（正确性/边界），意见逐条采纳/不采纳留痕。
