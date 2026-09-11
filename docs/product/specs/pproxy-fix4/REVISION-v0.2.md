# PProxy Fix4 对抗审核整改 — Spec v0.2 修订纪要

> 基于 2026-08-27 三路对抗审核（架构/安全/Windows UX）共 14 条意见，采纳全部 P0 阻断 + 多数 P1，修订后进入实施。

## 采纳策略

- **全部 P0 阻断 6 项必改**：锁选型、管理面 no_proxy 双保险、PAC bypass 推导、快照持久化、托盘退出全路径、auto_proxy 默认 false
- **P1 5 项并行整改**：PAC JS 转义加固、归一化二次校验、bypass 优先级钉死、单实例、事件驱动状态同步
- **P2 3 项随 T5 交付**：气球提示、合并撤销、菜单层级

不采纳：无（仅对“是否默认 true”原提案做了反向修订，完全采纳审核意见改为首次询问）

---

## 修订对比（v0.1 → v0.2）

### 1. 热更新并发模型（架构 I-1 / I-4 + 安全 5）

- **v0.1**：`static WHITELIST: RwLock<Vec<String>>`，类型未钉死，临界区包含 `format!` 与 `broadcast`。
- **v0.2**：
  - 统一 `tokio::sync::watch::channel(Vec<String>)` 为唯一内存真源（`OnceLock<watch::Sender>`），文件仅持久化。读方 `receiver.borrow().clone()` 后立即 `drop` 再做 `matches/generate_pac`，写方 `send` 在锁外。
  - 替代方案：若 watch 在 Tauri 同步命令上下文引入运行时依赖，则退化为 `parking_lot::RwLock` + `clone-then-drop` 契约，并加 `clippy::await_holding_lock` / `#[deny(clippy::await_holding_lock)]`。
  - 约束：`proxy_whitelist_set` 顺序固定为 `validate→normalize→write tmp+rename→watch.send→broadcast_change`（广播在锁/通道发送后）。
  - 新增单测：持锁跨 `await` 编译失败反例、50并发 CONNECT + 1 set 无超时。

### 2. 启动/退出时序与管理面双保险（架构 I-2 + 安全 1/3/4）

- **v0.1**：`setup: cleanup_stale → 自动 proxy_enable (默认 true, 静默)`，管理面走 PAC，未持久化快照。
- **v0.2**：
  - 时序重排：`cleanup_stale(持久化快照还原) → init_watch_from_file → engine.bind().await 成功 → sysproxy::enable(Pac) → broadcast → emit("proxy-ready")`。前端 `client.ts` 在 `proxy-ready` 前的管理请求强制走 `no_proxy` 通道。
  - 新增 Rust 命令 `api_bypass_fetch`（或 `tauri-plugin-http` scope 强制 `noProxy`）内部 `reqwest::Client::builder().no_proxy().build()`，管理面凭据请求一律走此通道，PAC 仅作第二道保险。
  - 快照持久化：`data_dir/sysproxy_snapshot.json`（含 `proxy_enable/proxy_server/autoconfig_url/pid/ts`），`enable` 前写盘，`disable/cleanup_stale` 读盘还原，崩溃后下次启动按快照还原而非简单 delete。
  - PAC URL 匹配改为 `starts_with("http://127.0.0.1:18900/pac")` 兼容 `?v=hash` 指纹，回读校验 `get_value` + 重试一次 `broadcast_change`（检查 `SendMessageTimeoutA` 返回）。
  - 错误不再 `let _ =` 丢弃，托盘与前端 toast 显式提示。

### 3. PAC bypass 清单形式化（架构 I-3 + 安全 2/3）

- **v0.1**：`if (h===backendHost||isPrivate(h)) DIRECT`，推导源不全、私网判定用后缀。
- **v0.2**：
  - 抽 `collect_bypass_hosts() -> BTreeSet<String>`：解析 `tunnel.json url`、`localStorage pony-backend-url`（前端启动时 `invoke('proxy_bypass_hosts')` 注入）、`tauri.conf.json plugins.updater.endpoints`，各取 `Url::parse().host_str().to_ascii_lowercase()` 去重，`access.example.com` 仅 fallback。
  - PAC 模板：`DIRECT` 优先级最高（`isPlainHostName / localhost / isPrivateHost / bypassSet` 均 `return 'DIRECT'` 后才进入 whitelist 循环）；私网走严格前缀/范围（`10.`, `192.168.`, `172.16-31.`, `127.`, `::1`, `fc00:`, `fe80:`），域名走 `h===e || h.endsWith('.'+e)`，IP 不走后缀；陷阱单测 `172.17.5.1 / 192.168.evil.com / notyoutube.com`。
  - 注入点归一化二次校验（regex `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)*$` 且 `len<=253`），拒绝 `_ * : / ? #`；bypass 列表仅信任 `tunnel.json` 解析出的 host，禁止 `localStorage` 任意拼串。
  - PAC 生成统一走 `serde_json::to_string` 转义（覆盖 `\b\f\n\r\t\u2028\u2029`），手写 replace 废弃；`generate_pac` 单测含 JS parse 校验。
  - 约束：白名单禁止包含管理面 host（`proxy_whitelist_set` 若 `matches(backendHost, newList)` 则拒绝）。

### 4. auto_proxy 默认策略（Windows UX #2）

- **v0.1**：`auto_proxy.json` 默认 `true`，缺失视为 true。
- **v0.2**：改为 **首次询问**。`setup` 中若 `app_config.json` 无 `auto_proxy` 字段，不自动 `proxy_enable`，而是前端弹确认对话框“检测到可自动开启系统代理以修复连接问题，是否开启？[开启并记住] [暂不开启]” + `下次不再询问` 复选框。老用户迁移：`whitelist.json` 非空但 `app_config` 缺失视为首次询问。设置页提供开关可随时关闭。自动接管仅在用户显式确认后生效。

### 5. 托盘与窗口语义（架构 I-5 + 安全 4 + Windows UX #1/#3/#5）

- **v0.1**：双 item `启用/停用` + `set_enabled`，`CloseRequested→hide` 未作区分。
- **v0.2**：
  - 窗口：`on_window_event` 仅对 `CloseRequested`（用户 X/Alt+F4/任务栏关闭）`prevent_close + hide` 并 500ms 内 `TrayIcon::show_balloon("已最小化到托盘…")`（仅首次），`RunEvent::ExitRequested` / `WM_QUERYENDSESSION` 不 prevent，保证关机可 `proxy_disable`。
  - 托盘：单项切换 `系统代理: 已启用 [✓]`（checked 态）替代双 item；`display` 菜单置顶 `显示主窗口` 并支持图标双击/单击唤起；`proxy_enable/disable` 成功后 `app.emit("proxy-status-changed", {on: bool})` + `tray.set_checked()`；前端 `ProxyView` 监听事件并在 `windowFocus` 时 `invoke('proxy_status')` 轮询兜底。
  - 单实例：引入 `tauri-plugin-single-instance`，第二实例 focus 第一实例后立即退出，禁止其执行 `cleanup_stale`。
  - 看门狗：P2 可选，不纳入本迭代。

### 6. 子域归一交互（Windows UX #4）

- **v0.1**：静默覆盖/合并，文案技术化。
- **v0.2**：拒绝时 `one.google.com 已包含在 google.com 中，无需重复添加`；合并时弹确认 `添加 google.com 将合并并移除 2 个子域 (mail.google.com, …)，是否继续？[合并][取消]`，或执行后 `Toast: 已合并为 google.com [撤销 5s]`；批量导入完成后 `Toast: 已归一 N 条为 M 条`。

## 更新后的任务拆解（T1-T6 保持，验收标准追加）

- T1 必须满足 I-1/I-4 的 watch/临界区约束，单测含并发。
- T3 必须满足 I-2/I-3 的双保险与 bypass 推导单测 + `no_proxy` 抓包验证。
- T4 必须满足 I-5 + UX #1/#3/#5 的事件驱动与首次气球。
- T2/T5 追加归一化二次校验与文案。

## 放行标准（进入实施）

- 本修订已闭环全部 P0 阻断，剩余 P1 随实现并行，二审改为“代码审查时复核”而非阻塞开发。

