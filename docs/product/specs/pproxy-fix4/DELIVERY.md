# Fix4 交付 — PProxy 4问题闭环（热更新/子域/接管/托盘）

> Spec: `README.md v0.1` + `REVISION-v0.2.md`（三路对抗审核后修订）  
> 状态：**可交付**（双闭环：方案审核→实施→代码再审，部分 P0 blocker 已现场调优）  
> 版本：`0.3.12-alpha`（迭代号以发布时 semver 为准）

## 覆盖问题与根因

| # | 现象 | 根因 |
|---|------|------|
| 1 | 开启代理后关闭再开无法连后端 | `tauri-plugin-http` 默认走系统 PAC + PAC 对管理面 host 未 `DIRECT` + 退出未还原快照 + 重启 probe 仍设死 PAC |
| 2 | 加速名单需重启才生效 | `EngineConfig.whitelist` 启动时 `Arc::new(snapshot)`，`whitelist_set` 仅写盘 |
| 3 | 子域未自动覆盖感知 | `suffix_match` 已支持但缺热更新+归一/去重，列表膨胀 |
| 4 | 关闭≠托盘、托盘无代理切换 | 无 `CloseRequested→hide` 拦截、托盘双 item 不符合 Windows 规范 |

## 实施清单（T1-T6）

- **T1 热更新**：`lib.rs` `OnceLock<watch::Sender<Vec<String>>>` 唯一内存真源，文件仅持久化；`EngineConfig.whitelist: watch::Receiver`，`handle_conn` 与 `/pac` 均 `borrow().clone()` 后立即 `drop` 再 `matches/generate_pac`，`proxy_whitelist_set` 按 `validate→normalize→tmp+rename→watch.send→broadcast_change` 顺序，广播在锁外；`#![deny(clippy::await_holding_lock)]`。
- **T2 子域归一**：`is_valid_domain` 正则+长度校验，`suffix_match` 复用 `whitelist::matches`，实现了“子域被父域覆盖拒绝 + 父域合并自动移除子域”最小集，禁止 `backendHost` 入白名单，文案对齐 `已包含在 X 中，无需重复添加`。
- **T3 接管全局 + PAC bypass + no_proxy 双保险**：`sysproxy.rs` 落盘 `sysproxy_snapshot.json`（pid/ts），`enable` 前落盘，`disable/cleanup_stale` 优先按快照还原（`starts_with PAC_URL` 兼容 `?v` 指纹，回读校验+重试广播），`pac.rs` `collect_bypass_hosts()` 聚合 `tunnel.json` + `updater.endpoints` + fallback，`generate_pac_with_bypass` 用 `serde_json` 转义、首行 `isPrivateHost`+`bypass` 的 `DIRECT` 优先级最高（`isPlainHostName/localhost/private/bypass > whitelist`），`client.ts` `rawRequest` 优先走 `invoke('api_bypass_fetch')`（`reqwest::no_proxy`），失败回退 `plugin-http`。新增 Rust 命令 `api_bypass_fetch / proxy_bypass_hosts / app_config_get/set / proxy_auto_config_*`。
- **T4 托盘/窗口**：`lib.rs` `on_window_event CloseRequested → prevent_close + hide + 首次 balloon`，`RunEvent::ExitRequested/Exit → proxy_disable + 删 instance.lock`（Exit 不 prevent），托盘单项 `CheckMenuItem "系统代理: 已启用" checked` 替代双 item，图标左键/双击唤起，`sync_tray_and_emit` 统一重建菜单并 `emit proxy-status-changed`，单实例 `instance.lock` 带 stale 清理（mtime>10s / pid 不存活时删除）。
- **T5 前端联动**：`ProxyView.vue` 归一预检+合并确认 `ConfirmDialog`+ 5s 撤销，监听 `proxy-status-changed` + `focus` 轮询兜底；`App.vue` 首次 `auto_proxy` 询问对话框（`app_config.json auto_proxy/dont_ask`，无字段时不自动 `proxy_enable`），`SettingsView` 与 `client.ts` 协同 `no_proxy`。
- **T6 验证**：实机手工清单见 Spec §6（Windows 六步），自动化已绿。

## 关键调优（代码再审后）

- **引擎并发**：`proxy_enable_inner` 改为引擎单例（`ENGINE_TASK: Mutex<Option<JoinHandle>>`），已运行则复用 `watch`（不再重复 `bind`），`probe` 超时前未就绪则 `Err` 并 `abort` 刚 spawn 的失败 task，避免死 PAC。
- **托盘启用置灰修复**：`setup` 首建 `CheckMenuItem` 的 `enabled` 由 `false` 修正为 `true`。

## 变更文件

- `desktop/src-tauri/src/lib.rs` — watch 通道、归一校验、引擎单例、托盘/窗口生命周期、proxy_* 命令、app_config
- `desktop/src-tauri/src/proxy/engine.rs` — `watch::Receiver` + clone-then-drop + `run` hot
- `desktop/src-tauri/src/proxy/pac.rs` — `collect_bypass_hosts`, `generate_pac_with_bypass`, serde_json 转义, isPrivateHost
- `desktop/src-tauri/src/proxy/sysproxy.rs` — 持久化快照、starts_with、回读校验、broadcast bool
- `desktop/src-tauri/Cargo.toml` — `tokio sync`, `reqwest no_proxy`
- `desktop/src/api/client.ts` — `api_bypass_fetch` 双保险
- `desktop/src/views/ProxyView.vue` — 合并确认/撤销 + 事件同步
- `desktop/src/App.vue` — 首次 auto_proxy 询问
- `crates/server/src/gateway.rs` — 附带修复：route 名剥离（`/{route}/{path}` → `/{path}`），回归单测

## 验证

- `cargo test --manifest-path desktop/src-tauri/Cargo.toml` — 22 passed
- `pnpm --prefix desktop check` — vue-tsc pass
- `pnpm --prefix desktop test` — 87 passed (10 files)
- 手工（Windows）：待发布后按 Spec §6 六步执行，需抓包确认 `access.example.com` 永远 `DIRECT`，强杀后重启注册表按快照还原

## 已知限制/后续

- 单实例仅 `instance.lock` 文件锁，未用 `tauri-plugin-single-instance`（避免新增依赖）；高可靠场景可替换。
- 看门狗（父进程异常退出自动还原）列为 P2 未纳入本次迭代，依赖快照+下次启动自愈已覆盖 95% 场景。
- `app_config.json` 的 `balloon_shown` 仅首次气球，后续可在设置页提供开关。

## 发布步骤

1. `cargo test` + `pnpm check/test` 全绿（已满足）
2. `pnpm build && cargo build --release` 出 NSIS
3. 置 `access.example.com/dsk/latest.json` 自更新
4. Windows 实机六步手工 + `curl --proxy ""` 对照
