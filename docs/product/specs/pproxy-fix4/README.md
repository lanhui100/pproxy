# PProxy 4问题闭环优化方案 — Spec v0.1

> 覆盖用户提出的 4 个缺陷，完成度定义为「对抗审核→实施→再审核」双闭环可交付

## 1. 背景与目标

桌面端 `pony-desktop`（Tauri2 + Rust 代理引擎）在 Windows 首次交付后，用户反馈 4 个体验/可靠性问题：

1. **系统代理残留导致离线**：开启代理后关闭再重开应用，无法连接后端管理面，疑似系统 PAC 导致管理请求被误代理到已失效的本地端口。
2. **加速名单非热更新**：`whitelist.json` 写入后引擎内 `Arc<EngineConfig>` 快照不更新，需重启应用才生效。
3. **子域未自动覆盖**：用户期望 `google.com` 自动覆盖 `one.google.com` 等任意子域（实际 Rust `whitelist::matches` 与 PAC 已支持后缀匹配，但缺少热更新与去重归一，用户感知为“逐个添加无效”）。
4. **窗口关闭语义与托盘**：期望关闭按钮仅最小化到托盘，托盘“退出”才是真退出；托盘需新增“开启/关闭代理”切换。

目标：一次迭代修复上述 4 项，前后端、引擎、系统代理三层一致，并通过两轮三路对抗审核 + 自动化验证达到可交付标准。

非目标：PAC 缓存时间可配置、Linux/macOS 系统代理、TUN 模式。

## 2. 现状根因分析

### 2.1 问题1：无法连接后端

- **PAC 错误路由**：`sysproxy::enable(Pac)` 将 `AutoConfigURL` 设为 `http://127.0.0.1:18900/pac`。PAC 的 `FindProxyForURL` 对所有 host 执行后缀匹配：`h === e || h.endsWith('.'+e)`。管理面地址若恰为 `access.example.com` / 局域网 IP，理论应 `DIRECT`，但：
  - 引擎未启动时 PAC 端口已指向死端口，PAC fetch 失败时 Windows 会按 `DIRECT` 回退还是按最后已知 PAC 行为不确定，可能短暂阻断。
  - `tauri-plugin-http` 的 `fetch` 是否尊重系统 PAC 未显式禁用——实测在 Windows 上 `reqwest` 默认会读系统代理，管理请求可能被送往 `127.0.0.1:18900`（尚未就绪/已关闭）导致 `network` 错误。
  - 应用关闭路径未还原系统代理：窗口 X 直接 `app.exit` 或进程强杀绕过了 `proxy_disable`，`cleanup_stale` 仅在下次启动时清除，中间窗口期内所有外联走死 PAC。
  - 未实现「启动自动接管 / 退出自动还原」语义。

### 2.2 问题2：非热更新

- `lib.rs::proxy_enable()` 在 spawn 时 `Arc::new(EngineConfig{whitelist: wl.clone()})` 快照；`proxy_whitelist_set` 仅写文件，引擎内 `cfg.whitelist` 永不变。
- `proxy_pac()` 读取文件、`engine` 提供 PAC 使用快照，两者数据链路分裂。

### 2.3 问题3：子域

- 引擎层 `whitelist::suffix_match` 与 PAC 的 `endsWith` 均已正确实现 `host === entry || host.endsWith('.'+entry)`，单测覆盖 `notyoutube.com` 陷阱。
- 问题本质是 **2 未热更新 + 缺少归一/去重**：用户添加 `one.google.com` 时若已存在 `google.com`，应提示或自动归一为 `google.com`，否则列表膨胀、PAC 体积增大。

### 2.4 问题4：托盘与窗口生命周期

- `tauri.conf.json` 未声明 `onClose` 拦截，`lib.rs` 无 `RunEvent::WindowEvent(CloseRequested)` 处理，`TrayIconBuilder` 仅有 `proxy_on/off/quit`，缺少「显示窗口」与“当前状态”联动。
- `quit` 已调用 `proxy_disable` + `app.exit`，但窗口 X 路径未拦截，导致“关闭即退出”而非“最小化到托盘”。

## 3. 方案设计

### 3.1 总体策略

```
┌─ 应用生命周期 ──────────────────────────────────┐
│ setup: cleanup_stale → 自动尝试 proxy_enable     │
│ window close-requested → hide (prevent_close)    │
│ tray: 显示窗口 / 启用代理 / 停用代理 / 退出(还原)│
│ on_exit/quit: proxy_disable (还原快照)           │
│                                                 │
│ 代理引擎                                         │
│  ArwLock<Vec<String>> hot whitelist              │
│  EngineConfig{ whitelist: Arc<RwLock<…>> }        │
│  handle_conn 每连接实时读锁匹配                   │
│  /pac 每请求实时读锁生成 PAC                      │
│  whitelist_set → 写文件 + 更新 RwLock + 广播 WS │
│  PAC 注入 backendHost bypass:                     │
│    if (h === backendHost || isPrivate(h)) DIRECT │
└─────────────────────────────────────────────────┘
```

### 3.2 详细设计

#### A. 接管全局（问题1）

- **启动自启**：`setup` 内 `cleanup_stale()` 后，若 `tunnel_config_load()` 齐备且 `whitelist` 非空，自动 `proxy_enable()`（失败静默，仅日志）。增加 `auto_proxy: bool` 本地偏好（`data_dir/auto_proxy.json`），默认 `true`，用户在托盘/设置可关闭自启。
- **退出还原**：
  - 拦截 `RunEvent::ExitRequested` / `RunEvent::WindowEvent(CloseRequested)` 写入还原逻辑。
  - 进程强杀兜底：保留 `cleanup_stale`，并将 PAC URL 改为带版本指纹 `http://127.0.0.1:18900/pac?v=1` 便于识别。
  - 管理请求绕过代理：
    - 方案 A（优选）：PAC 首行注入旁路——若 `host` 为管理面 host（从 `tunnel.json`/`localStorage` 推导 + 常见 `127.0.0.1/localhost/192.168.* /10.* /access.example.com`），直接 `return 'DIRECT'`。
    - 方案 B：`tauri-plugin-http` 侧对 `baseUrl` 域名禁用代理（若 plugin 支持 `proxy` 配置则设 `noProxy`；否则新增 Rust 命令 `api_proxy_bypass_fetch` 使用 `reqwest::Client::builder().no_proxy()`）。双保险：PAC + client。
- **引擎启动竞态消除**：已存在 2s 探测 + PAC 设置，保持；新增若 `TcpStream::connect` 超时则延迟 500ms 重试一次。

#### B. 热更新（问题2）

- 新增 `static WHITELIST: RwLock<Vec<String>>`（或 `OnceLock<Arc<RwLock<_>>>`），引擎与命令共享。
- `EngineConfig` 字段改为 `whitelist: Arc<RwLock<Vec<String>>>` 或保留 `Arc<EngineConfig>` 内用 `RwLock`。
- `proxy_whitelist_set`：校验→写盘→`*WHITELIST.write() = normalized_next`→`broadcast_change()`（触发系统重拉 PAC）→ 发 `proxy-whitelist-updated` 事件供前端（如需要）。
- `proxy_whitelist_get`：从 `WHITELIST` 读（若空则懒加载文件）。
- `generate_pac`：每次从 `WHITELIST` 实时生成；`engine::handle_conn` 每次连接获取读锁判定 `whitelist::matches(&host, &guard)`。
- 性能：读锁持有仅匹配期间（微秒级），写锁仅设置时；并发连接无阻塞。
- 前端 `ProxyView`：`addEntry/removeEntry` 成功后本地 `entries` 已更新，无需重启；可选监听 `proxy-whitelist-updated` 自动刷新。

#### C. 子域归一（问题3）

- **匹配层无需改**：保持 `suffix_match`（含 `rest.endsWith('.')` 防 `notyoutube.com`）。
- **写入层归一**：
  - `proxy_whitelist_set` 入参做 `normalize_host`（小写、去尾点、trim、punycode 预留）。
  - 去重：若 `new_entry` 已被现有某条后缀覆盖（`matches(new, oldList)`），则拒绝添加并提示“已由 google.com 覆盖”。
  - 归一：若 `new_entry` 是现有若干子域的父域，则移除那些子域（例如新增 `google.com` 自动移除 `mail.google.com`, `one.google.com`），保持列表最小化。
  - 批量导入时同样归一。
- **前端提示**：InfoTip 文案保持“填 example.com 会连同它的所有子域一起匹配”；添加失败时 toast 提示被覆盖原因。

#### D. 托盘与窗口语义（问题4）

- `tauri.conf.json` 保持 `windows[0]`，Rust 侧新增：
  ```rust
  .on_window_event(|win, ev| if let WindowEvent::CloseRequested{ api, ..}=ev { api.prevent_close(); win.hide().ok(); })
  ```
- 托盘菜单重构：`显示主窗口 | ── | 启用代理 | 停用代理 | ── | 退出`。`启用/停用` 项根据 `ENGINE_ON` 动态 `set_enabled`。
- `on_menu_event`：
  - `show` → `get_window("main").show+set_focus+unminimize`
  - `proxy_on/off` → 复用 `proxy_enable/disable` 并同步托盘状态
  - `quit` → `proxy_disable` + `app.exit(0)`
- 前端 `ProxyView` 的开关仍可用，但状态以 Rust `proxy_status` 为真源，切换后同步托盘。
- 单实例：保持现有行为，不新增。

### 3.3 数据与存储

- `whitelist.json`：`Vec<String>` 仍为主存储；`WHITELIST` 内存镜像。
- `tunnel.json` + keyring：不变。
- 新增 `app_config.json`（可选）：`{auto_proxy: boolean}`；默认 `true`，缺失视为 `true` 兼容老用户。

### 3.4 安全与边界

- 校验：`proxy_whitelist_set` 保持 `empty/len>253/ascii/alphanum+.-_` 检查，增加 `normalize` 后二次校验。
- PAC 注入：`generate_pac` 保持转义 `\'` 与 `\\`，新增 bypass 域名同样转义。
- `sysproxy`：Windows `RegKey` 操作保持 `KEY_SET_VALUE|KEY_QUERY_VALUE`，`broadcast_change` 保留 1000ms 超时。
- 凭据：不触及 `credential_*` 路径。
- CSP：不放宽 `connect-src 'none'`，管理请求走 `plugin-http`（Rust 侧），不走 WebView fetch。

## 4. 任务拆解

| ID | 任务 | 产出 | 依赖 |
|----|------|------|------|
| T1 | 引擎热更新：`WHITELIST RwLock` + `proxy_whitelist_*` 改写 + `/pac` 实时生成 | `engine.rs`, `lib.rs`, 单元测试 | - |
| T2 | 子域归一与校验增强：normalize、父域覆盖子域、子域被父域覆盖拒绝 | `whitelist.rs`, `lib.rs::proxy_whitelist_set`, 前端提示 | T1 |
| T3 | 系统代理接管：启动自启、退出还原、PAC backend bypass、管理请求 no_proxy | `sysproxy.rs`, `pac.rs`, `lib.rs::setup`, `client.ts` | T1 |
| T4 | 窗口/托盘语义：`CloseRequested→hide`、托盘菜单重构、状态联动 | `lib.rs`, `tauri.conf.json`（如需） | T3 |
| T5 | 前端联动：ProxyView 热更新反馈、开关状态同步、新增子域提示 | `ProxyView.vue`, `SettingsView`（可选） | T2,T4 |
| T6 | 验证与回归：单测+集成+Windows 手工清单 | 测试报告、手工验证录屏/截图 | T1-T5 |

预估：T1+T2 1天，T3 0.5天，T4 0.5天，T5 0.5天，T6 0.5天。

## 5. 风险与回滚

- **注册表写入失败**：`enable/disable` 已返回 `Result`，调用方 toast 提示但不阻塞主窗口；快照还原失败仅 warn。
- **热更新死锁**：读锁仅在匹配时持有，写锁仅在 `set` 时，`engine` 的 `handle_conn` 不在锁内做 IO。
- **PAC 误判 DIRECT 导致加速失效**：bypass 列表仅包含私网与管理面 host，白名单命中仍优先 PROXY；新增单测覆盖 `backendHost` bypass。
- **回滚**：任一 T 失败可 revert 对应 commit；`whitelist.json` 格式不变，老版本可读。

## 6. 测试与验收

### 自动化

- `cargo test -p pony_desktop_lib -- proxy::whitelist::matches` 保持 5 用例 + 新增：归一后子域被父域替代、重复添加被拒。
- `pac::generate_pac` 新增：backend bypass 行、`PROXY` 无 `DIRECT` 兜底、空列表合法。
- `engine::decide`/`handle_conn` 集成：热更新后新连接走新名单（spawn 引擎 → `proxy_whitelist_set` → 新 CONNECT 命中）。
- 前端 `vitest`：ProxyView 的 add/remove 归一提示（mock invoke）。

### 手工（Windows 实机）

1. 首次安装→打开应用→托盘/设置显示“代理已自动开启”；浏览器访问 `youtube.com` 走隧道、`baidu.com` 直连（抓包/日志验证）。
2. 应用内添加 `example.com`→不重启→新开标签访问 `sub.example.com` 立即被代理（日志 `tunneled +1`）。
3. 添加 `google.com`→再添 `one.google.com` 提示已被覆盖；反向：先 `one.google.com` 再添 `google.com` 自动合并。
4. 关闭窗口（X）→窗口隐藏、托盘仍在、代理仍有效；托盘退出→代理还原、AutoConfigURL 清除、后台 `curl` 直连不受影响。
5. 托盘“停用代理/启用代理”切换→系统代理注册表与 `proxy_status` 同步。
6. 重启应用（托盘退出后重开）→管理面可立即连接（无 network 错误），代理自动恢复。

## 7. 归档与发布

- 代码合入 `main`，版本 `0.3.12`（或按 semver）。
- 更新 `docs/product/specs/m6` 或本 spec 的 `DELIVERY.md`。
- `cargo test` + `pnpm check` + `pnpm test` 全绿。
- 发布 NSIS 并验证自更新通道。

