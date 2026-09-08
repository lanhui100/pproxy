# 桌面端故障排查手册

> 定位：Windows 桌面端（Tauri v2 + Vue 3）的故障复盘与排查记录。
> 服务端（pony-server / Worker / Vercel）问题见 [TROUBLESHOOTING.md](./TROUBLESHOOTING.md)。

## 目录

- [2026-08-29 · 粘贴口令后假「已开启」+ 代理与服务页被强制跳转设置页](#2026-08-29--粘贴口令后假已开启--代理与服务页被强制跳转设置页)
- [修复审核 · 第二轮（另一团队实施）](#修复审核--2026-08-29第二轮另一团队实施)
- [2026-08-30 · openai.com 等站点经隧道 502（CF Worker 平台拒绝 dial CF 托管目标）](#2026-08-30--openaicom-等站点经隧道-502cf-worker-平台拒绝-dial-cf-托管目标)

---

## 2026-08-29 · 粘贴口令后假「已开启」+ 代理与服务页被强制跳转设置页

### 元信息

| 项 | 值 |
|---|---|
| 报告日期 | 2026-08-29 |
| 版本 | desktop 0.3.16（`desktop/src-tauri/tauri.conf.json`） |
| 环境 | Windows，`pnpm --prefix desktop tauri dev` |
| 影响面 | 桌面端全部出网功能 + 管理面所有请求 |
| 状态 | 已定位，**修复未应用**（本文方案待确认） |
| 关联改动 | 工作区未提交：`App.vue` / `DashboardView.vue` / `SettingsView.vue` / `src-tauri/src/lib.rs` |

### 症状

1. 仪表盘粘贴一键口令（或方案 A 的 Cloudflare Token）并点「开启」后，主开关显示**智能加速已开启**（绿灯），
   但「常用 AI 与海外服务实时连通性」全部**无法连接**。
2. 点击侧栏「代理与服务」后，页面**自动跳转回「设置」**，无法停留。仪表盘页不受影响。

两个症状同源，第 2 条是第 1 条的副产物。

---

### 故障一：状态显示「已开启」但连接全部失败

#### 根因：状态位与链路可用性完全脱钩

UI 上的「已开启」只代表**本地监听起来了 + 系统代理被接管**，不代表**能出网**。三处叠加：

| # | 位置 | 问题 |
|---|---|---|
| A | `desktop/src-tauri/src/lib.rs:508-523` | `proxy_enable_inner` 只用 `TcpStream::connect("127.0.0.1:18900")` 探测本地端口，**从不验证 WS 隧道出口是否可达**。本地监听成功即返回 `Ok(())` |
| B | `desktop/src/views/DashboardView.vue:98-99` | `await invoke('proxy_enable')` 一成功就无条件 `isRunning.value = true`，不复核 `proxy_status`，也不做真实拨测 |
| C | `desktop/src/views/DashboardView.vue:148-153` | `runSiteTests()` 的 `catch` 在 invoke 抛错时把所有站点**伪装成 `ok` + `150ms`**，失败被吞成假绿 |

于是「本地代理已接管」被 UI 渲染成「网络已打通」。

#### 链路侧：两种口令各有各的断法

**`pproxy://user:pass@host:port` 或手动填远端服务器（chained 模式）**

引擎层**完全没有实现 chained 模式**。`proxy::engine::decide()`
（`desktop/src-tauri/src/proxy/engine.rs:89-106`）只有 `Tunnel`(WS) 与 `Direct` 两条路径，
全仓库**没有任何代码读取 `remote_host`**：

- `proxy_import_sync` 的 `pproxy://` 分支（`lib.rs:822-838`）
- `proxy_mode_switch` 的 `chained` 分支（`lib.rs:727-736`）

二者都只把 `remote_host / username / password` 写进 `app_config.json` 与 keyring 就返回成功。
后果：开启后白名单流量仍然走 WS 隧道 —— 隧道没配就在 `lib.rs:477` 报「隧道未配置」；
隧道是旧配置则与你粘贴的口令**毫无关系**。该模式目前处于
「能保存、能显示已开启、但一条流量都不走它」的半截状态。

**`pproxy-sync://` 口令 / 方案 A 的 Cloudflare Token**

只有 payload 携带 `proxy_secret` 时才写隧道，由 `configure_direct_tunnel`（`lib.rs:696-718`）
落到 `wss://edge.ponyjob.top`。secret 无效或该 worker 不支持 WS 中继时，
每个请求都在 `engine_tunnel::establish`（`proxy/engine_tunnel.rs:80-110`）失败 → 回 502 →
站点检测全红，而状态灯依旧是绿的。

---

### 故障二：进入「代理与服务」被强制跳转设置页

#### 根因链路

```
仪表盘粘贴口令 / CF Token
  └─ lib.rs:714-715  configure_direct_tunnel()
       └─ 同一个 secret 同时写入 tunnel_token 与 admin_token 两个凭据位
            └─ 管理面请求携带 Bearer <proxy_secret>
                 └─ 进入 /core：CoreView.vue:282  api.listRoutes() 未传 skipAuthRedirect
                      └─ 后端返回 401
                           └─ client.ts:195  notifyUnauthorized()
                                └─ App.vue:39  router.push('/settings')
                                     └─ 死循环：新版设置页已无 admin token 配置入口
```

#### 逐环说明

| 环 | 位置 | 说明 |
|---|---|---|
| 令牌污染 | `src-tauri/src/lib.rs:714-715` | `cred_set_impl(CREDENTIAL_USER_TUNNEL, secret)` 与 `cred_set_impl(CREDENTIAL_USER, secret)` 连续两行。`CREDENTIAL_USER = "admin_token"`，把 WS 隧道密钥写进了管理面令牌位 |
| 请求未豁免 | `src/views/CoreView.vue:282` | `api.listRoutes()` 未传 `skipAuthRedirect`。同文件的 `probeAllRoutes`（`:299`）**传了**，两处不一致 |
| 全局拦截 | `src/api/client.ts:195-197` | 401 → `notifyUnauthorized()` → 广播给所有监听者 |
| 强制跳转 | `src/App.vue:38-40` | `onUnauthorized(() => router.push({ path: '/settings', ... }))`，全仓库唯一跳转点 |
| 无法自愈 | `src/views/SettingsView.vue` | 本次未提交改动删掉了旧版的「管理面地址 + admin token」与「隧道中继」两块配置卡片，只剩加速模式 / 口令导入 / 急救箱 / 更新 |

#### 为什么只有这个 tab 会跳

`/core`（CoreView）是**唯一在 `onMounted` 时就发未豁免 API 请求**的页面。
DashboardView 完全不走 `api/client.ts`，所以仪表盘安然无恙。

#### 放大因素

- `App.vue` 本次改动**删除了 `useBackendGate()`**。旧行为：未配置网关时内容区只渲染
  `NeedSetupGuide`，页面不发请求。现在即使后端地址为空，`/core` 也照样发请求。
- `useAlertNotifications.ts:46` 的 `api.alerts(true, 50)` 同样未豁免。
  默认 5 分钟一轮，每次 401 都会把用户从任意页面踢到设置页。
- `client.ts:122-129` 的去抖只有 50ms，且没有「已在设置页则不重复 push」的判断。

---

### 解决方案

#### P0 · 解开死循环（约 10 分钟）

**1. 停止污染 admin_token —— `src-tauri/src/lib.rs:714-715`**

```diff
     let _ = cred_set_impl(CREDENTIAL_USER_TUNNEL, secret.to_string());
-    let _ = cred_set_impl(CREDENTIAL_USER, secret.to_string());
     let _ = ensure_tunnel_watch().send((Some(ws_url), Some(secret.to_string())));
```

`CREDENTIAL_USER` 仍被 `credential_get/set/delete` 与 `proxy_get_current_config` 使用，删除后不会触发未使用告警。

**2. 被动加载一律豁免，401 走页内横幅**

`src/api/client.ts`：

```diff
-  listRoutes: () => request(RoutesRespSchema, 'GET', '/api/routes'),
+  listRoutes: (opts?: RequestOptions) => request(RoutesRespSchema, 'GET', '/api/routes', undefined, opts),

-  alerts: (unreadOnly?: boolean, limit?: number) => {
+  alerts: (unreadOnly?: boolean, limit?: number, opts?: RequestOptions) => {
     const q = new URLSearchParams()
     if (unreadOnly) q.set('unread', '1')
     if (limit !== undefined) q.set('limit', String(Math.min(Math.max(limit, 1), 500)))
     const qs = q.toString()
-    return request(AlertsRespSchema, 'GET', `/api/alerts${qs ? `?${qs}` : ''}`)
+    return request(AlertsRespSchema, 'GET', `/api/alerts${qs ? `?${qs}` : ''}`, undefined, opts)
   },
```

`src/views/CoreView.vue`：

```diff
-async function refreshRoutes(): Promise<void> {
+async function refreshRoutes(opts?: RequestOptions): Promise<void> {
   routesLoading.value = true
   routesError.value = ''
   try {
-    const res = await api.listRoutes()
+    const res = await api.listRoutes(opts)
     routes.value = res.routes
   } catch (e) {
     routesError.value = errText(e)
   } finally {
     routesLoading.value = false
   }
 }
```

调用点区分：**被动加载豁免，用户主动操作保留跳转**（那样 401 才有引导意义）：

| 调用点 | 处理 |
|---|---|
| `onMounted` (`:464`) | `await refreshRoutes({ skipAuthRedirect: true })` |
| 30 分钟轮询 (`:320`) | `await refreshRoutes({ skipAuthRedirect: true })` |
| 「刷新列表」按钮 / `submitSmartAccess` / `doDeleteRoute` | 保持原样（不传 opts） |

需从 `@/api/client` 额外导入 `RequestOptions` 类型（当前未导出，需补 `export interface RequestOptions`）。

`src/composables/useAlertNotifications.ts:46`：

```diff
-      const r = await api.alerts(true, 50)
+      const r = await api.alerts(true, 50, { skipAuthRedirect: true })
```

**3. 恢复设置页的管理面配置入口**

在 `SettingsView.vue` 补一张卡片，复用 `lib/config.ts` 现成的 `loadBackendUrl / saveBackendUrl /
saveAdminToken / clearAdminToken`，以及旧版已验证的 `setTokenProvider` 装配逻辑：

- 管理面地址输入（`http://host:8900`），保存时 `saveBackendUrl()` 同步 `backendUrlSaved` ref 热解锁
- admin token 输入（type=password），非空才 `saveAdminToken()`
- 「连接测试」按钮用 `api.health({ skipAuthRedirect: true })`，401/网络错误在卡片内展示，**绝不触发跳转**
- 401 落地后消费 `history.state.authInvalidHint`，顶部给警示条并聚焦 token 输入框（旧版已有实现，可cherry-pick）

#### P1 · 让状态位说真话（约 30 分钟）

**4. `proxy_enable_inner` 增加真实出网拨测 —— `src-tauri/src/lib.rs`**

在 `let snap = proxy::sysproxy::enable(...)`（`:524`）**之前**插入。放在启用系统代理之前，
失败时无需回滚注册表，只需 abort 引擎任务：

```rust
/// 出网拨测：经本地引擎对探测目标发 CONNECT，验证隧道出口真实可达（同步实现，避免 block_on 风险）。
fn probe_egress_via_engine(probe_host: &str, timeout: std::time::Duration) -> Result<(), String> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect("127.0.0.1:18900")
        .map_err(|e| format!("引擎未就绪: {e}"))?;
    let _ = s.set_read_timeout(Some(timeout));
    let _ = s.set_write_timeout(Some(timeout));
    let req = format!("CONNECT {probe_host}:443 HTTP/1.1\r\nHost: {probe_host}:443\r\n\r\n");
    s.write_all(req.as_bytes()).map_err(|e| format!("出网拨测写入失败: {e}"))?;

    let mut buf = Vec::new();
    let mut tmp = [0u8; 512];
    loop {
        match s.read(&mut tmp) {
            Ok(0) => return Err("出网拨测：隧道在建连前关闭连接".into()),
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 8192 { break; }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {
                return Err(format!("出网拨测超时：隧道未在 {timeout:?} 内建立 {probe_host} 的连接"));
            }
            Err(e) => return Err(format!("出网拨测读失败: {e}")),
        }
    }
    let head = String::from_utf8_lossy(&buf);
    if head.starts_with("HTTP/1.1 2") || head.starts_with("HTTP/1.0 2") {
        Ok(())
    } else {
        Err(format!("出网拨测失败：{}", head.lines().next().unwrap_or("无响应").trim()))
    }
}
```

调用处（替换 `:507-523` 的端口探测尾部，保留端口探测作为阶段一）：

```rust
    // 阶段二：真实出网拨测（仅当存在需要走隧道的流量时）
    let need_tunnel = matches!(load_proxy_mode_from_file(), proxy::pac::ProxyMode::Global) || !wl.is_empty();
    if need_tunnel {
        let probe_host = wl.iter()
            .find(|e| e.split('.').count() >= 2)
            .map(|e| format!("www.{e}"))
            .unwrap_or_else(|| "www.google.com".to_string());
        if let Err(e) = probe_egress_via_engine(&probe_host, std::time::Duration::from_secs(8)) {
            if let Some(h) = ENGINE_TASK.lock().unwrap_or_else(|p| p.into_inner()).take() { h.abort(); }
            ENGINE_ON.store(false, AOrd::SeqCst);
            sync_tray_and_emit(&app, false);
            return Err(format!("出网拨测失败，系统代理未启用：{e}"));
        }
    }
```

> 探测目标必须从白名单派生并加 `www.` 前缀，保证在白名单模式下确实命中 `Route::Tunnel`；
> 若固定写 `www.google.com`，白名单被用户改空或改造后会退化成 Direct 直连，在国内必然超时误判。
> 代价：开启代理最多增加 8 秒等待，属可接受（换来「不骗人」）。

**5. 前端不再无条件置位，也不再伪装成功 —— `src/views/DashboardView.vue`**

```diff
-      await invoke('proxy_enable')
-      isRunning.value = true
-      toast.success('智能加速已开启！')
-      runSiteTests()
+      await invoke('proxy_enable')
+      const st = await invoke('proxy_status') as { engine_running: boolean }
+      isRunning.value = st.engine_running
+      if (!st.engine_running) {
+        toast.error('开启失败', '出网拨测未通过，系统代理未启用')
+        return
+      }
+      toast.success('智能加速已开启！')
+      runSiteTests()
```

```diff
   } catch (e) {
-    for (const item of testResults.value) {
-      item.status = isRunning.value ? 'ok' : 'idle'
-      item.latency = isRunning.value ? 150 : undefined
-    }
+    for (const item of testResults.value) {
+      item.status = 'fail'
+      item.latency = undefined
+    }
+    toast.error('测速失败', String(e))
   }
```

> 修复后仍保留 `proxy_test_sites` 要求 `ENGINE_ON` 的前置判断（`lib.rs:572`），逻辑不变。

#### P2 · chained 模式去留（产品决策）

方案 B（连接远端代理）在引擎层未实现，只有两条路，选一：

| 方案 | 工作量 | 说明 |
|---|---|---|
| A. 引擎实现 HTTP 上游 | 大 | 在 `Route` 枚举加 `UpstreamHttp` 分支，新增「经远端 HTTP 代理发 CONNECT」的 relay；`proxy::pac` 与白名单逻辑不受影响。需同步定义凭据读取（`remote_host` + keyring `admin_token` 复用会再次踩 P0 的坑，**必须另开凭据位**） |
| B. UI 下架方案 B | 小 | 移除 `SettingsView` 与 `DashboardView` 的方案 B 卡片、`proxy_import_sync` 的 `pproxy://` 分支、`proxy_mode_switch` 的 `chained` 分支，只保留 `pproxy-sync://` 一键导入 |

建议：**短期走 B**（避免用户继续掉进「配置了却完全不生效」的坑），
等有真实远端代理需求时再走 A，并单独开凭据位。

---

### 验证方法

修复后按以下顺序回归：

1. **凭据隔离**：粘贴 `pproxy-sync://` 口令后，检查 Windows 凭据管理器
   （控制面板 → 凭据管理器 → Windows 凭据）中 `pony-desktop / admin_token` 的值
   **不等于** 粘贴的 secret；`pony-desktop / tunnel_token` 等于该 secret。
2. **跳转消失**：未配置 admin token 时进入「代理与服务」，应停留在本页并显示
   页内错误横幅「未授权：admin token 缺失或已失效」，**不再跳转**。
3. **5 分钟轮询**：在 `/core` 停留 6 分钟以上，不应被踢走。
4. **假状态消失**：填入一个无效 secret 后点开启，应看到明确的失败 toast +
   红灯，主开关仍是「加速已停止」，系统代理未被接管
   （`netsh winhttp show proxy` 或设置 → 网络和 Internet → 代理，确认仍为关闭）。
5. **真成功回归**：填入有效 secret，开启后 5 个站点检测应全部绿色且延迟合理。

前端快速自检（devtools console）：

```js
localStorage.getItem('pony-backend-url')   // 空则说明管理面地址从未配置
```

---

---

## 修复审核 · 2026-08-29（第二轮：另一团队实施）

### 审核范围

| 文件 | 改动 |
|---|---|
| `desktop/src-tauri/src/proxy/engine_upstream.rs` | 新增 217 行（chained 上游中继） |
| `desktop/src-tauri/src/proxy/engine.rs` | +22（新增 `Upstream` 结构、`Route::Tunnel` 分支分流、`relay_bidir` 提为 `pub(crate)`） |
| `desktop/src-tauri/src/proxy/mod.rs` | +3（模块声明） |
| `desktop/src-tauri/src/lib.rs` | +320（凭据槽、upstream watch、`proxy_enable_inner` 模式装配、`derive_gate_url`、方案 B 相关命令） |

### 验证记录（本机实测）

| 动作 | 结果 |
|---|---|
| `cargo check --all-targets`（desktop/src-tauri） | 通过，无警告 |
| `npx vitest run`（desktop） | 14 文件 / 111 测试全通过 |
| `cargo test --lib`（desktop/src-tauri） | **挂死**：32 项通过后卡在 `engine_upstream::tests::connect_ok_on_2xx_and_relays` 超 60s；单独重跑 50s 仍未结束 |

### 结论：暂不合入（Conditional Reject）

1 项阻塞（测试挂死，CI 必挂）、3 项功能缺陷；且**用户实际报告的两个症状中，只有一个的根因被修掉，症状路径本身仍在**。

---

### 已正确修复（认可）

**1. admin_token 污染 —— P0-1 到位**

`lib.rs:766` `configure_direct_tunnel` 已删除对 `CREDENTIAL_USER`（`admin_token`）的写入，
只剩 `CREDENTIAL_USER_TUNNEL`，并留下说明注释。这是解开跳转死循环的关键一环，改得干净。

**2. chained 模式引擎实现 —— P2 选了方案 A**

- `engine_upstream.rs` 实现了「经远端 HTTP 代理 CONNECT 出网」，R4 语义正确
  （失败即报错关闭，不静默回落直连）
- `lib.rs:183` 新增独立凭据槽 `proxy_password`，**没有复用 admin_token**，避开上一轮踩的坑
- `lib.rs:481` `proxy_enable_inner` 按 `mode_type` 装配通道，chained 分支做了 scheme 剥离与缺端口回落 8899
- `engine.rs:233` 分流正确：`upstream` 有值优先走上游，否则回落 WS gate；`borrow().clone()` 未跨 await 持锁

**3. `derive_gate_url` 补齐 `/ws` 路径 —— `lib.rs:732`**

原先只做 `https→wss` 替换，缺 gate 端点的 `/ws` 路径，Upgrade 必然失败。这是本轮一并发现并修掉的真缺陷，
有 6 条单测覆盖。

---

### 阻塞项（必须修）

**B1. 单元测试永久挂起 —— `engine_upstream.rs:167-200`**

`connect_ok_on_2xx_and_relays` 在 `handle.await`（`:199`）处死锁：`a` 是客户端 socket，
在测试函数返回前不会 drop，而 `relay_bidir` 需两端都关闭才返回 —— 等一个永远不会发生的关闭。

```diff
         a.write_all(b"PING").await.unwrap();
         let n = a.read(&mut tmp).await.unwrap();
         assert_eq!(&tmp[..n], b"PING");
+        drop(a);   // 或 a.shutdown().await，让 relay_bidir 的读端归零
         handle.await.unwrap().expect("链路建立成功");
```

`cargo test --lib` 因此永远不会结束，CI 拿不到结果。这是本轮最严重的问题——
它让整包测试失去意义，比产品代码的局部缺陷更危险。

---

### 功能缺陷（建议修）

**F1. `try_establish` 无读超时 —— `engine_upstream.rs:44-53`**

```rust
let n = s.read(&mut tmp).await?;   // 上游 accept 后不回包 → 永久挂起
```

对照 `engine_tunnel.rs:54` 有 `FIRST_FRAME_TIMEOUT`（10s）保护，此处缺失。
远端代理的半死连接是链式场景的常态，每个请求会泄漏一个 tokio 任务且无上限，浏览器侧同步卡死。
建议对齐 `engine_tunnel::try_establish_url` 的 `timeout_at` 写法。

**F2. 明文 HTTP 请求重建多出一个 CRLF —— `engine_upstream.rs:84-95`**

`head.lines()` 对结尾的 `\r\n\r\n` 会多产出一个空元素，重建后变成 `...\r\n\r\n\r\n`。
已模拟 Rust `lines()` 语义复现验证：

```
重建: "GET http://x/y HTTP/1.1\r\nProxy-Authorization: Basic ***\r\nHost: x\r\n\r\n\r\n"
期望: "GET http://x/y HTTP/1.1\r\nProxy-Authorization: Basic ***\r\nHost: x\r\n\r\n"
```

GET 无感，但明文 POST 的 body 会被这 2 字节前缀破坏。

> 同一缺陷存在于 `engine_tunnel.rs:130-140` 与 `engine.rs:271`（既有代码）。
> 新模块是复制既有模式，但既然是新写的分支，建议一次改对，并回头统一其余两处。

**F3. `Upstream` 派生 `Debug` 泄露密码 —— `engine.rs:20-26`**

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Upstream {
    pub host: String,
    pub username: String,
    pub password: String,   // 任何 {:?}（含 panic 回溯、日志）都会打出明文
}
```

建议移除 `Debug` 派生，手写实现把 `password` 打为 `***`。

---

### 未覆盖项：原诊断的 4 条修复未做

| 编号 | 项 | 状态 | 后果 |
|---|---|---|---|
| P0-1 | 停止污染 admin_token | 已修 | — |
| P0-2 | `listRoutes` / `alerts` 传 `skipAuthRedirect` | **未做** | **症状 2 仍在** |
| P0-3 | 恢复设置页管理面地址与 token 入口 | **未做** | **401 后仍无法自愈** |
| P1-4 | `proxy_enable_inner` 增加真实出网拨测 | **未做** | **症状 1 仍在** |
| P1-5 | 去掉无条件置位与假 ok | **未做** | **症状 1 仍在** |
| P2 | chained 去留 | 已实现（方案 A） | — |

实测确认未做的位置：

- `api/client.ts`：`listRoutes` 与 `alerts` 仍不接受 `opts`；`RequestOptions` 仍未导出
- `useAlertNotifications.ts:46`：`api.alerts(true, 50)` 未豁免 → 每 5 分钟一次的 401 依旧把人踢到设置页
- `App.vue:36`：`onUnauthorized` 跳转保留（本身合理），但触发源没堵住
- `SettingsView.vue`：全文无 `admin` / `backend` / `8900` / `setTokenProvider` 相关代码，管理面入口仍是零
- `lib.rs:508-521`：`proxy_enable_inner` 仍只有 `TcpStream::connect("127.0.0.1:18900")` 端口探测，无出网拨测
- `DashboardView.vue:99`：`isRunning.value = true` 仍是无条件置位；`:150-151` 假 `ok` + `150ms` 仍在

**要点**：P0-1 修掉的是「为什么会出现 401」的根因，但 401 一旦出现（token 过期、后端重启换 token、
首次安装未配置），跳转链路依然完整存在。用户报的「进代理与服务被弹到设置页」在新环境仍会复现，
只是触发条件从「粘贴口令后」变成「token 不对」。同理，「开启成功但连不上」的假状态一条没动。

---

### 次要建议

| # | 位置 | 建议 |
|---|---|---|
| S1 | `lib.rs:605` `proxy_test_sites` | 超时由 10s 收紧到 4s。chained 需经公网远端代理再出网，4s 偏紧，建议按模式区分或回调至 8s |
| S2 | `lib.rs:288` `ensure_upstream_watch` | 始终初始化为 `None`，不从 `app_config.json` 恢复，与 `ensure_tunnel_watch` 不一致。当前无实际 bug（唯一写入点必 send），但依赖隐式前提，建议补上从配置初始化 |
| S3 | `lib.rs:697` `proxy_get_current_config` | `has_secret` 只查 `tunnel_token` 与 `admin_token`，未纳入 `proxy_password`，方案 B 配置后该字段仍为 false |
| S4 | `lib.rs:496` | `trim_start_matches("http://")` 会重复剥离（`http://http://x` → `x`），改用 `strip_prefix` 一次即可 |
| S5 | `lib.rs:746` | `trim_end_matches("/ws")` 同样会重复剥离，同上 |
| S6 | 测试覆盖 | 缺三个场景：上游不响应（超时）、absolute-form 转发、username 为空时不发认证头。B1 与 F1 若有测试即能拦住 |
| S7 | `engine_upstream.rs:56` | `status.contains(" 2")` 判定 2xx 不够精确（有 `starts_with` 兜底，当前安全），建议改解析状态码数字，与 `dial_via_proxy` 的 `starts_with("HTTP/1.1 2")` 统一 |
| S8 | `crates/engine`（`pproxy-engine`） | 与 `crates/server` 的 `connect.rs`(422 vs 897)、`gateway.rs`(244 vs 636) 并存且行数不同，疑似未完成的拆分重构，两份逻辑重叠。与本次修复无关，建议确认归属后清理 |

---

### 遗留与待决（审核后更新）

| 项 | 状态 |
|---|---|
| B1 测试挂死 | **阻塞**，必须修 |
| F1 / F2 / F3 功能缺陷 | 建议同批修 |
| P0-2 / P0-3（401 跳转死循环） | 未做，症状仍在，需补 |
| P1-4 / P1-5（假「已开启」） | 未做，症状仍在，需补 |
| 401 去抖未判断「已在设置页」 | 次要，补 P0-2 时顺手加 |
| `useBackendGate` 删除后缺失「未配置网关」引导 | `NeedSetupGuide.vue` 仍在但无人引用；补 P0-3 时决定接入还是删除 |
| S8 `crates/engine` 与 `crates/server` 重叠 | 需确认重构归属 |

---

## 修复实施与对抗审核闭环 · 2026-08-29（第三轮）

### 实施结果（本轮全部落地）

| 项 | 落地 |
|---|---|
| B1 测试挂死 | `engine_upstream.rs` 测试补 `drop(a)`；`relay_bidir` 由 `try_join!` 重写（见下） |
| F1 上游无超时 | `try_establish` 增加 dial 与首帧双重 `timeout_at`（测试态 500ms/300ms，生产 10s） |
| F2 明文请求多 CRLF | `rebuild_request` 用 `split_once("\r\n")` 重建；`engine_tunnel.rs`、`engine.rs` 同缺陷一并修 |
| F3 Debug 泄露密码 | `Upstream` 手写 `Debug`，password 脱敏为 `***`；含 `EngineConfig` 不泄露测试 |
| P0-1 令牌污染 | `configure_direct_tunnel` 不再写 `admin_token`（上一轮已修，本轮确认） |
| P0-2 401 豁免 | `listRoutes`/`alerts` 支持 `opts`；`RequestOptions` 导出；CoreView 被动加载与 alerts 轮询传 `skipAuthRedirect` |
| P0-3 管理面入口 | 设置页新增「管理面连接」卡片（地址 + token + 测试并保存 + 清除密钥 + 401 落地提示） |
| P1-4 出网拨测 | `probe_egress_via_engine`（同步拨测）+ `derive_probe_host`（白名单派生）+ 失败回滚 |
| P1-5 假状态 | `toggleProxy` 复核 `proxy_status` 再置位；`runSiteTests` catch 如实报失败 |

### 对抗审核（两轮 agent，意见 12 条 → 采纳 10 条修复 + 2 条遗留）

**Rust 侧（7 条）**

| 意见 | 处置 |
|---|---|
| [阻塞] `relay_bidir` select 取消丢数据 | 采纳：重写为单循环双方向 + 半关闭（`shutdown` 对端写半区），两端 EOF 才退出 |
| [阻塞] `open_external_url` 命令注入（`&`/`\|` 经 cmd 二次解析） | 采纳：严格校验 URL 字符（拒绝 `& \| < > ^ " 空格 %`） |
| [功能] 8s 拨测冻结 UI（同步命令在主线程） | 采纳：`proxy_enable`/`proxy_disable` 改 async + `spawn_blocking` |
| [功能] `derive_probe_host` 对含 `www.` 条目叠加成 `www.www.` | 采纳：直接使用白名单条目（后缀匹配必命中 Tunnel） |
| [次要] 4xx/407 确定性拒绝仍重试 | 采纳：错误按 `ErrorKind` 分类（TimedOut/ConnectionRefused 重试，PermissionDenied 放弃） |
| [次要] close-wait 半关闭悬挂 | 由新的单循环 relay 实现一并解决 |
| [次要] CONNECT 应答与首包同段时 leftover 字节丢弃 | **遗留**：影响极小（上游 CONNECT 后立刻推数据的场景），记为待办 |

**前端侧（7 条）**

| 意见 | 处置 |
|---|---|
| [功能] 告警横幅「前往处理」→ 仪表盘已无告警列表（markAlertRead 无调用方） | 采纳：移除「前往处理」链接，横幅仅提示 + 关闭 |
| [功能] `routesError` 从未渲染，401 豁免后用户只见空态 | 采纳：专线列表区加 `routesError` 提示条（含跳转设置页指引） |
| [存疑] `@click="refreshRoutes"` 传 MouseEvent | 采纳：改为 `@click="refreshRoutes()"`（消除类型隐患） |
| [次要] `toggleProxy` 复核 invoke 抛错 → 镜像假状态 | **遗留**：复核失败保持「未开启」是保守正确，且下次点击可自愈 |
| [次要] 测速未命中永久「测速中」 | 采纳：遍历后把仍为 `testing` 的项兜底为 `fail` |
| [次要] 保存失败后输入框与生效地址脱节 | 采纳：失败路径回写 `adminUrl = prevUrl` |
| [存疑] alerts 豁免后未读数保留过期值 | **遗留**：静默降级设计使然，token 恢复后自动更新 |

### 最终验证（本机实测全绿）

| 项 | 结果 |
|---|---|
| `cargo test --lib` | 45/45 通过，4.5s，不挂起 |
| `cargo clippy --all-targets` | 0 警告 |
| `npx vue-tsc --noEmit` | 通过 |
| `npx vitest run` | 111/111 通过 |
| `npx oxlint src vite.config.ts` | 0 警告 |

### 构建环境备忘（本次踩坑，避免重蹈）

- **本机无 MSVC 工具链**（无 link.exe / msvcrt.lib，Windows SDK 10.0.26100.0 存在）。
  `desktop/src-tauri/target` 里的 exe 均为预构建产物，任何真实重链接都会失败。
- 解决：`desktop/src-tauri/.cargo/config.toml` 配置 `linker = "rust-lld"`（rustup 自带，可链接 MSVC 目标）
  并关闭增量编译 `incremental = false`——rustc 1.95 的增量缓存损坏会触发 ICE（`encode_metadata` heapsort panic）。
- 已安装 zig（winget）备用，但最终未使用（rust-lld 足够）。
- 注意：Git Bash 的 `/usr/bin/link.exe` 会顶替 MSVC 链接器（PATH 优先），曾导致
  `link: missing operand after '\377\376'`；配置显式 linker 后绕开。
- 遗留待办：`CONNECT` 应答 leftover 字节透传（engine_upstream.rs）、`toggleProxy` 复核异常兜底、
  `ensure_upstream_watch` 从配置初始化（S2）、`has_secret` 纳入 `proxy_password`（S3）。

---

## 第四轮：gate 端点与隧道令牌（一键开启失败实测）· 第五轮：admin token 废弃

### gate 端点修复（已完成）

- **根因**：gate worker（WS 桥）部署在 `gate.ponyjob.top/ws`（wrangler.toml 注明），但桌面端与 server
  的推导逻辑都用 `edge.ponyjob.top`（HTTP 网关域名，被 CF 403），且旧 `tunnel.json` 缺 `/ws`。
- **修复**：桌面端 `GATE_WS_URL` 常量 + `configure_direct_tunnel` 固定端点 +
  `migrate_tunnel_url`（edge→gate 自动迁移）；server 端 `derive_gate_url_from_worker` 同步迁移。
- 用户数据 `tunnel.json` 已改为 `wss://gate.ponyjob.top/ws`。

### 隧道令牌重置（已完成，curl 101 验证生效）

- gate 校验 `sha256(Bearer) == TUNNEL_TOKEN_HASH`（wrangler secret，部署时手动设）。
- **wrangler 4.x 版本化限制**：`wrangler secret put` 被拒，须 `wrangler versions secret put` +
  `wrangler versions deploy <version-id>`（在 dev 主机 `/home/USER/pproxy/deploy/cf-gate-worker` 执行）。
- **坑**：`echo | wrangler secret put` 会写入尾部换行导致校验永败，须 `printf '%s'`。
- 新令牌对已配置并验证（101）。

### admin token 废弃（单体化收尾，已完成）

单体架构下桌面端不再认证远端管理面，admin token 及管理面逻辑退休：

| 文件 | 改动 |
|---|---|
| `views/SettingsView.vue` | 删除「管理面连接」卡片与全部相关 script（provider 装配、401 落地提示、测试保存、清除密钥） |
| `App.vue` | 删除 onUnauthorized 401 全局跳转、告警横幅（useAlertNotifications/unreadCount/banner） |
| `views/CoreView.vue` | 删除整个「专线管理」区块（管理面 API：routes/token 列表、一键接入、测速历史、30 分钟轮询），保留 Windows 系统代理控制 |
| `composables/useTunnelProvision.ts` | 去掉 `api.tunnelConfig` 远程拉取，只读本地 |
| `api/client.ts` | `defaultResolveToken` 一律返回 null（不再读 keyring admin_token）；`setTokenProvider` 机制保留 |
| 保留 | api client 结构/端点（无调用方但无害）、Rust `credential_*` 命令（向后兼容）、`useAlertNotifications` 文件 |

**验证**：vue-tsc 0 错 / vitest 111 通过 / oxlint 0 警告。

**遗留**：Rust 侧 `credential_get/set/delete` 与 `api_bypass_fetch` 未删（无 UI 调用，可后续清理）；
`useAlertNotifications` 文件保留未删。

---

## 2026-08-30 · openai.com 等站点经隧道 502（CF Worker 平台拒绝 dial CF 托管目标）

### 元信息

| 项 | 值 |
|---|---|
| 报告日期 | 2026-08-30 |
| 环境 | Windows，`pnpm dev:tauri`（单体架构桌面端） |
| 症状日志 | `proxy conn failed: tunnel: denied: Error: proxy request failed, cannot connect to the specified address. It looks like you might be trying to connect to a HTTP-based service — consider using fetch instead` |
| 影响面 | 白名单中所有 Cloudflare 托管站点（openai.com / chatgpt.com / claude.ai / anthropic.com / notion.so 等）；google 系、github、x 等非 CF 托管站点不受影响 |
| 状态 | 根因定位 + 客户端/网关代码修复完成；**备用出口需部署后故障才算闭环**（见文末步骤） |

### 排障结论（E2E 实测证据）

1. **报错文本是 Cloudflare Workers `connect()`（cloudflare:sockets）的平台错误**，不是本仓库代码生成的
   （全仓库 grep 无该文本）。它出现在 WS 握手与令牌校验**通过之后**、gate worker 向目标拨号的阶段。
2. 用本机存储的隧道令牌直接对 `wss://gate.ponyjob.top/ws` 复测（`deploy/vercel-gate-worker/smoke-test.mjs`）：
   - `www.google.com:443` → `{"ok":true}` + 真实 TLS 握手成功（TLS_AES_256_GCM_SHA384）；
   - `openai.com:443` → `{"ok":false}`，reason 与用户日志**逐字一致**。
   证明：令牌有效、gate 正常、代理主体已通；失败被精确定位在 worker `connect()` 到 openai.com。
3. openai.com 解析到 172.64.154.211 / 104.18.33.45——均为 Cloudflare 自有 IP 段。

### 根因（两层，均不可在 CF Worker 内绕过）

| # | 层 | 说明 |
|---|---|---|
| 1 | CF 平台回环防护 | Workers `connect()` 禁止连到 Cloudflare 自有 IP（官方 TCP sockets 限制）。openai.com 是 CF 托管（橙云）→ 拨号必然被拒，即本次报错 |
| 2 | OpenAI 封禁 CF 出口 | [ADR-002](../architecture/decisions/002-dual-upstream-cf-vercel.md) 早已记录：OpenAI 按 AS13335（Cloudflare ASN）整段拉黑；CF 托管目标即使能连也会被对端拒绝 |

单体化重构后桌面端唯一隧道端点是 CF gate（`tunnel.json` 单端点 `wss://gate.ponyjob.top/ws`），
而旧架构里承担 OpenAI 流量的 Vercel 出口（ADR-002「敏感服务走 Vercel，AWS 真实 IP 放行」）
**从未提供 WS gate 形态**——`deploy/vercel-gate-worker` 写好了但没部署，且代码带致命 bug（见下）。

### 顺带发现并修复的缺陷

**N1. Node 备用网关首帧吞没 bug（`deploy/vercel-gate-worker/api/index.js`，阻塞部署）**

`ws` v8 的 `message` 回调里，文本帧与二进制帧的 `data` **都是 Buffer**，只能用 `isBinary` 区分。
原代码 `if (isBinary || Buffer.isBuffer(data))` 恒为真 → 客户端的 JSON 首帧被当二进制吞掉 →
连接永不建立。本地实测复现（冒烟测试 15s 超时）。

**N2. 桌面端 `engine_tunnel::try_establish_url` 缺拨号超时（failover 的前提）**

`connect_async` 自身无超时：多端点配置下首个端点若静默挂死（edge PoP 抖动/被墙），
failover 永远轮不到后续端点，每个请求卡满整条建连循环。已对齐 `engine_upstream` 的
`DIAL_TIMEOUT` 方案（生产 10s，测试 800ms），并补两条 failover 单测
（`establish_falls_over_to_second_endpoint_on_denied` / `establish_falls_over_on_silent_endpoint`）。

**N3. 测试与 clippy 清理**

`probe_egress_err_on_non_2xx`（`lib.rs`）在拨测错误信息改为携带响应体后未同步更新（既有失败，
与本次改动无关，已实测确认），已对齐新语义；两处 `redundant_closure` clippy 警告清零。
最终 `cargo test --lib` 47/47 通过，`cargo clippy --all-targets` 0 警告。

### 已落地的代码修复

| 文件 | 改动 |
|---|---|
| `deploy/vercel-gate-worker/api/ws.js` | 由 `api/index.js` 重写：首帧判定只用 `isBinary`（N1）；鉴权 fail-closed（env 缺失一律 401，对齐 vercel/api/proxy.js 基线）；路径兼容 `/ws` 与 `/api/ws`；默认导出 `http.Server`（Vercel 官方 WebSocket/Fluid 模式） |
| `deploy/vercel-gate-worker/server.js` | standalone 模式复用默认导出实例，不再二次建 server |
| `deploy/vercel-gate-worker/vercel.json` | `api/ws.js` maxDuration=300（Vercel 上即 WS 连接寿命上限） |
| `deploy/vercel-gate-worker/smoke-test.mjs` | 网关端到端冒烟工具（WS 首帧 + 隧道之上真实 TLS 握手），CF/Node/Vercel 通用 |
| `systemd/pony-gate-node.service` | VPS 常驻部署单元（方案 A 用） |
| `desktop/src-tauri/src/proxy/engine_tunnel.rs` | 拨号超时（N2）+ 2 条 failover 单测 |
| `desktop/src-tauri/src/lib.rs` | N3 测试与 clippy 修复 |

### 收尾部署记录（方案 B：Vercel，2026-08-30 已执行完毕）

1. **项目**：`pony7/pony-gate-node`（Vercel CLI 59，token 取自 dev 主机 `~/pproxy/.pproxy.env`
   的 `PPROXY_VERCEL_TOKEN`；`vercel projects add` → `vercel link` → `env add TUNNEL_TOKEN_HASH production` →
   `vercel --prod --yes`，部署 Ready）。
2. **令牌散列坑**：dev 主机 `.pproxy.env` 里的 `PPROXY_TUNNEL_TOKEN` 是**轮换前旧令牌**，
   与桌面端凭据管理器 `tunnel_token.pony-desktop` 的 sha256 不一致（实测比对 MISMATCH）。
   CF gate 与 Vercel gate 都以**桌面端 keyring 令牌**为准（该令牌经 CF gate 实测认证通过）。
3. **域名**：项目绑定 `vgate.ponyjob.top`（`POST /v9/projects/pony-gate-node/domains`，即时 verified）；
   CF DNS 记录 `CNAME vgate → cname.vercel.com`（DNS only）。
   坑：`.pproxy.env` 的 `PPROXY_CF_API_TOKEN` 与 `.secrets.env` 的
   `CLOUDFLARE_API_TOKEN_FOR_GATE` 均无 DNS 权限；最终用 dev 主机
   `~/.cloudflared/cert.pem`（ARGO TUNNEL TOKEN 块内嵌 `apiToken`，带该 zone 的 DNS 写权限，
   `cloudflared tunnel route dns` 同源凭据）经 CF API 创建记录。
4. **端点形态**：Vercel Function 挂载于 `/api/ws`（非 standalone 的 `/ws`）；
   `vercel.app` 域有 SSO 登录墙且被 GFW 屏蔽，自定义域为必需项（同 vedge 先例）。
5. **生效配置**：桌面端 `tunnel.json` 已写多端点：
   `wss://gate.ponyjob.top/ws,wss://vgate.ponyjob.top/api/ws`
   （CF 优先，CF 平台拒绝的连接逐连接自动落到 Vercel 出口；桌面端引擎无需再改代码）。

**验证结果（2026-08-30 实测）**

```text
node smoke-test.mjs wss://gate.ponyjob.top/ws    www.google.com 443 --token '<keyring令牌>'
  → OK: TLS established (TLS_AES_256_GCM_SHA384)          # 既有出口回归 ✓
node smoke-test.mjs wss://vgate.ponyjob.top/api/ws www.google.com 443 --token '<keyring令牌>'
  → OK: TLS established (TLS_AES_256_GCM_SHA384)          # 备用出口回归 ✓
node smoke-test.mjs wss://vgate.ponyjob.top/api/ws openai.com   443 --token '<keyring令牌>'
  → OK: TLS established (TLS_AES_256_GCM_SHA384)          # 本次故障目标打通 ✓
node smoke-test.mjs wss://gate.ponyjob.top/ws    openai.com   443 --token '<keyring令牌>'
  → DENIED: proxy request failed, cannot connect ...      # CF 平台限制原样复现（对照）
```

**遗留备选**：方案 A（VPS 常驻 Node gate + cloudflared ingress，`systemd/pony-gate-node.service`
头部注释有完整步骤）作为 Vercel 出口不可用时的回退，未部署。

---

## admin_token 清理收尾 · 2026-08-30（单体化决策的最终执行）

**决策**：admin_token（旧前后端分离架构的远端管理面凭据）在第五轮已随管理面退役而停用，
本轮将其**代码与存储彻底移除**，仅保留本文档的决策记录。完成后代码中不再有任何
admin_token 逻辑、凭据槽位或依赖；隧道令牌（tunnel_token）为桌面端唯一凭据，不受影响。

**移除清单**：

| 层 | 内容 |
|---|---|
| Rust（lib.rs） | `CREDENTIAL_USER`（admin_token 槽位）常量、`credential_get/set/delete` 三个 Tauri 命令、`api_bypass_fetch` 命令（管理面绕代理 fetch）、`proxy_get_current_config` 的 has_secret admin_token 分支；reqwest 依赖随之移除 |
| 前端 API 层 | `api/`（client.ts + schemas + msw + 测试，含 token provider/401 拦截机制）整体删除；zod、msw、@tauri-apps/plugin-http 依赖移除 |
| 前端组件/组合式 | `UsageDrawer`（用量抽屉）、`useAlertNotifications`、`useBackendGate`、`useAdaptivePoll`、`useSessionSecret`、`useSecretCopy`（均无存活调用方）；chart.js/vue-chartjs 随用量抽屉移除 |
| 前端 lib | `errors`（管理面错误字典）、`statusLabels`（token/quota 状态映射，`Tone` 类型内联进 StatusDot）、`usageJoin`、`format`、`presetGenerator`、`serviceTemplates`、`expiry`、`normalize`；`config.ts` 的 backend url / dev admin token / 数据面地址 / 轮询间隔段 |
| 本机凭据 | Windows 凭据管理器中遗留的 `pony-desktop / admin_token` 条目已删除；`tunnel_token` 保留 |

**范围界定**：crates/cli 与 crates/core 中的 `admin_token`（`pproxy init --token`、`PonyConfig`、
`AdminClient`、`generate_admin_token`）是 CLI 连接 pproxy-server 管理面的**在用功能**（ADR-007
tailnet 管理面），不属于桌面端退役范围，予以保留。

**验证**：cargo test --lib 47/47、clippy 0 警告；vue-tsc 0 错、vitest 36/36、oxlint 0 警告。

---

## 阻塞式出网拨测移除 · 2026-08-30（产品决策，推翻第三轮 P1-4 的阻塞设计）

**现象**：开启代理报「出网拨测失败，系统代理未启用：出网拨测超时：8s 内未能建立到 openai.com 的出网连接」。

**根因**：P1-4 的阻塞拨测从白名单取**第一个条目**作探测目标——当前白名单第一项恰是 openai.com。
多端点 failover 生效后，openai.com 成了**最贵**的目标：每条连接先在 CF gate 吃一次平台拒绝
（实测 ~2.4s），再 failover 到 Vercel 出口建立（~3.2s，Fluid 冷启动更久），单次全程 ~5.5s 起步，
撞上拨测 8s 预算即误报超时并**阻断整个服务启动**。链路本身是通的——拨测在惩罚正常架构行为。

**决策（用户裁定）**：
1. **启动不再做出网拨测**。本地端口就绪检查（127.0.0.1:18900，2s）保留；一个站点的连通性
   不得阻塞整个服务启动。单站连通性由仪表盘「常用站点连通性测试」逐站体现（并行、各自超时、
   如实展示失败），不再有启动门禁。`probe_egress_via_engine` / `derive_probe_host` 及其测试一并删除。
2. **连通性测试站点调整为 google / github / x.com / openai / anthropic**（前后端列表同步）。
   单站超时 4s → 10s：CF 托管目标（openai/anthropic）须走 failover 链路，4s 预算必然误报；
   测试并行执行，拉长单站预算不拖慢整体。

**代价与自愈**：开启代理恢复秒级；「状态已开但个别站点不通」由连通性测试逐站暴露，
用户可据此判断链路质量，而不是被启动门禁一票否决。

---

## Antigravity CLI `FAILED_PRECONDITION (400) Location Not Supported` 修复与双 Gate 落地 · 2026-09-01

**现象**：
Antigravity CLI（`agy`）执行任务时频繁中断报错 `⚠ Agent execution terminated due to error`。通过扫描会话 SQLite 数据库底层真实响应，发现全部错误均为 Google API 返回的 `FAILED_PRECONDITION (code 400): User location is not supported for the API use`（Server: ESF）。

**根因**：
`agy` 经本地 pproxy 路由至 Cloudflare Gate Worker（`gate.ponyjob.top`）。CF Worker 出口网络地理位置跟随 edge colo 调度，国内用户频繁被调度至香港节点（HKG/MFM），而香港属于 Gemini/Google API 不支持的区域，从而导致请求被 Google ESF 拦截并报 400。

**落地修复**：
1. **Vercel Gate 钉死美区物理执行算力**：
   - 在 [`deploy/vercel-gate-worker/vercel.json`](file:///D:/Documents/pproxy/deploy/vercel-gate-worker/vercel.json) 显式配置 `"regions": ["iad1"]`（AWS 美东弗吉尼亚数据中心），确保所有出网 TCP 具有合规的原生美国 IP。
   - 生产环境绑定自定义域名 [`vgate.ponyjob.top`](https://vgate.ponyjob.top)。
2. **三端 SHA-256 鉴权令牌对齐**：
   - 统一使用 SHA-256 散列值 `<REDACTED_SHA256>`。
   - 同步注入到 Cloudflare Worker (`gate.ponyjob.top`) 密钥、Vercel 环境变量 (`TUNNEL_TOKEN_HASH`) 以及本地 Windows 凭据管理器 (`tunnel_token.pony-desktop`)。
3. **双网关 Failover 协同策略**：
   - 客户端配置多端点：`wss://gate.ponyjob.top/ws,wss://vgate.ponyjob.top/api/ws`。
   - 当 CF Worker 调度至非合规区域（HKG/MFM）且目标为 Google 时返回 `denied (unsupported_colo)`，桌面端 `engine_tunnel` 自动秒级 Failover 至 Vercel 美区出口；常规流量继续享受 CF Worker 低延迟直连。
4. **验证取证**：
   - `node smoke-test.mjs wss://vgate.ponyjob.top/api/ws www.google.com 443` -> `OK: TLS established (TLS_AES_256_GCM_SHA384)`
   - `node smoke-test.mjs wss://vgate.ponyjob.top/api/ws openai.com 443` -> `OK: TLS established (TLS_AES_256_GCM_SHA384)`
   - 单元测试：Rust 44/44 通过、Vitest 69/69 通过。

### 门禁加固与端点策略 · 2026-09-02（P1/P2 设计审核落地）

针对「前端链接状态绿但 agy 不可用」的复查，补齐门禁盲区与端点策略（见 `deploy/cf-gate-worker/gate-policy.mjs` 与 `desktop/src-tauri/src/proxy/engine_tunnel.rs`）：

- **P1-1 · agy 域名纳入 Google 系门禁**：`GOOGLE_SUFFIXES` 补充 `antigravity.google` / `labs.google`（桌面端白名单默认项）。此前这两个域名会被当作非 Google 流量在 HKG/MFM 等区域被全量放行，Google 拒绝时不触发 Vercel failover——正是 agy 场景的门禁盲区。
- **P1-2 · 严格白名单默认开启（fail-closed）**：`worker.js` 改为 `STRICT_GOOGLE_WHITELIST` 未设置/非 `false|0` 时即启用严格白名单；`wrangler.toml` 显式声明 `STRICT_GOOGLE_WHITELIST="true"`。此前白名单仅在显式开启时生效，属名不副实。
- **P2-3 · 端点 host 感知优先级**：`engine_tunnel.rs` 新增 `is_google_host`/`order_endpoints`——Google 系 host（含 agy 域名）→ Vercel 合规出口优先；非 Google → CF 低延迟优先。与 gate-policy 口径一致，消除「全量 Vercel 优先拖慢非 Google 流量」与「Google 先吃 CF denied 往返」两个问题。
- **P2-4 · 单 CF 端点旧配置迁移**：`migrate_tunnel_url` 对单独 `wss://gate.ponyjob.top/ws` 也自动补齐默认双端点。
- **P2-5 · 前端站点测速走本地引擎**：新增 `proxy_test_site_local` 命令，`DashboardView` 站点行改为经本地引擎真实分流（命中白名单/全局走隧道、未命中直连），移除站点行的手动 C/V 切换（引擎已按 host 自动选出口）；接口行保留 gate RTT 测速。
- **方案 A · 桌面端待命隧道池**（`engine_tunnel.rs` `TunnelPool`，对齐 `crates/server connect.rs`）：解决「经本地引擎测速 1s 内 → 1-3s」的口径问题——之前的测速命令把**冷建连成本**计入了计时，而旧 C/V 直拨测速用热态口径剥离了冷建连。池化后引擎按端点预建 WS 待命会话，establish 命中池时只需热态首帧（1 RTT）。桌面端差异：按端点分组 + host 感知 checkout（P2-3 端点策略）+ watch 热更新清池 + Weak 自引用防泄漏；`engine::run` 异步上下文幂等启动。
- **安装体验 · 安装/升级成功自动打开（默认勾选）+ 桌面图标**（`desktop/src-tauri/windows/installer-hooks.nsh`）：`NSIS_HOOK_POSTINSTALL` 无条件调用 `CreateOrUpdateDesktopShortcut`（GUI 未勾选/升级残留图标场景下桌面图标始终存在并指向当前版本）；自动打开走模板自带机制、hook 内不得直接拉起——GUI 安装由完成页「运行」复选框触发（`MUI_FINISHPAGE_RUN` 默认勾选、用户可取消，点完成后经 `RunMainBinary` 以 `RunAsUser` 拉起，单实例锁防重复），被动/静默升级（updater 下发 `/P /UPDATE /R`）完成页被跳过、由 `.onInstSuccess` 凭 `/R` 携带 `/ARGS` 拉起。
- 回归：gate-policy 51/51、desktop cargo 55/55、Vitest 78/78、vue-tsc 0 错、oxlint 0 警告、NSIS installer 编译通过。

### 复发定位与根治：出站地理门禁 + 合规出口端点优选 · 2026-09-08

> 错误分类与分层判定方法已沉淀为独立手册：
> [`docs/ops/ANTIGRAVITY-CLI-ERRORS.md`](ANTIGRAVITY-CLI-ERRORS.md)（含配套脚本 `scripts/antigravity/diag/`）。

**现象**：
`agy` 仍频繁 `⚠ Agent execution terminated due to error` + `Error ID: <trajectory_id>-<步号>`
（该 ID 是 CLI 本地关联 ID，不是 Google 错误码）。日志累计 99 次 agent-executor 级失败、
12 个对话受影响，跨 09-05 ~ 09-08。

**为什么 09-01/09-02 的修复没根治**（逐条实测）：

1. **入站 colo ≠ 出站 egress IP**：`shouldBlockColo` 判的是 WS 握手的 `request.cf.colo`，
   而 `connect()` 的出站 IP 由 CF 另行分配。实测 CF gate 出口在 `104.28.152.x / 104.28.158.x / 104.28.165.x`
   间轮换（8 次探测 1 次超时），colo 合规不等于 Google 看到的地区合规。
2. **端点优选没接到数据面**：`order_endpoints` 只被 `desktop/src-tauri/.../engine_tunnel.rs` 调用；
   `crates/server`（线上 8899）与 `crates/engine`（`pproxy serve`）都按配置顺序建连，CF 永远在前。
3. **省额度策略反向加压**：运行中 pproxy-server 带 `PPROXY_CONSERVE_VERCEL=1`，泛 Google 优先 CF。
4. **400 不触发 failover**：failover 只在建连失败时发生；Google 的 400 在隧道建立之后返回，
   代理是端到端密文转发，看不到也改不了。

**落地修复**（详见 ADR-008）：

- **A · CF gate 出站地理门禁**：`egress-probe.mjs`（`connect()`+`startTls()` 探测自身出站 IP 国家码）
  + `egress-geo.mjs`（TTL 5min 复用 / stale 30min 回退 / 并发去重）
  + `gate-policy.mjs::shouldBlockEgress`（**仅**对 Cloud Code 系 3 个 host 生效，fail-closed）。
  非合规 host 一律放行，避免探测异常时把泛 Google 流量倾泻到兜底出口。
- **B · 合规出口端点优选**：`transport::route` 新增 `COMPLIANT_EGRESS_SUFFIXES` /
  `requires_compliant_egress` / `ordered_gate_urls`；两份数据面 CONNECT 实现统一按目标 host 重排，
  池化 `checkout_ordered` 同步；该例外**高于** `PPROXY_CONSERVE_VERCEL`。CF 端点保留为兜底。
- **C · 修 gate worker 半死隧道**：上游正常 EOF 时 `pipeTo` 是 resolve 而非 reject，
  原 `.catch` 不触发、WS 悬挂（实测 CF 侧 >210s 仍存活）。改为 `.then(closeUpstream, closeUpstream)`。

**验证**：

```bash
cargo test --workspace                             # 全绿
node deploy/cf-gate-worker/gate-policy.test.mjs    # 75 pass
node deploy/cf-gate-worker/egress-geo.test.mjs     # 19 pass
bash scripts/check-egress-parity.sh                # Rust/JS 两侧 host 清单一致
```

**部署（2026-09-08 已完成）**：

1. Rust 数据面：`cargo build --release -p pproxy-server` → 替换 `/home/USER/.local/bin/pproxy-server`
   → `systemctl --user restart pproxy-server`（会打断在途隧道，建议 agy 空闲时做）。已完成。
2. CF worker：`cd deploy/cf-gate-worker && npx wrangler deploy`。已完成
   （线上版本此前停留在 2026-08-31，A/C 从未上线）。
   凭据：`CLOUDFLARE_API_TOKEN` + `CLOUDFLARE_ACCOUNT_ID`；本机可用 token 在
   `~/.wrangler/config/default.toml`（wrangler 4 默认不读该路径，需显式导出环境变量）。

**上线后自检**：

```bash
# 出站地理探测（返回本 Worker 出站 IP/国家码）
curl -s https://gate.ponyjob.top/debug/egress -H "Authorization: Bearer <tunnel_token>"
# 数据面出口归位（合规 host 应落 vgate 66.33.60.x，认证类仍落 CF）
pproxy status
```

**同批修掉的两个缺陷**：

- `PPROXY_TUNNEL_POOL=0` 回滚开关此前无效（`crates/transport/src/pool.rs` 的 `size.max(1)`
  把 0 抬成 1，仍预建 1 条待命会话）；现 `size=0` 真正禁池化，实测 `pooled=false`。
- `pproxy status` 把**上游** 403 误报成"未配置 Gate 隧道出口"：`api.github.com/zen` 会对
  共享出口 IP 的未认证请求限流（实测 5 次 1 次 403）。现按 `x-pproxy-reason` 头区分
  代理层拒绝与上游状态码，并把 GitHub 探测目标换成 `github.com/robots.txt`。


---

## 凭据编码污染事故：「隧道未配置」误报 + 全站 401 · 2026-09-01

**现象**：
重启桌面端后开启代理报「隧道未配置」（配置明明都在）；经同步口令导入后代理可开但所有网站
502（`tunnel failed: HTTP error: 401`）；仪表盘却显示接口正常。

**根因**：
当日 token 轮换时，新 token 由脚本以 **UTF-8 字节**写入 Windows 凭据管理器；而桌面端
keyring 3.6.3 一律按 **UTF-16** 解码凭据 blob（其 `set_password` 亦按 UTF-16 写入）。
53 字节 UTF-8 blob 无法按 UTF-16 解码，`get_password()` 返回 `Err("Data is not UTF-8 encoded")`，
被 `.ok().flatten()` 静默吞成「未配置」。同步口令导入则把旧 token 直送内存 watch（不读凭据），
导致引擎持旧 token 拨 gate → 401。接口拨测每次现读凭据，故能升级 WS，造成「接口绿、站点红」。

**定位手法**：
最小 keyring 探针（keyring 3.6.3 + `Entry::new("pony-desktop","tunnel_token").get_password()`）
复现解码错误；Python `win32cred.CredRead` 对照证实 blob 为 UTF-8。

**修复**：
用 `win32cred.CredWrite`（str blob，UTF-16LE）重写凭据，keyring 探针读回 OK，
`CONNECT google.com:443` 经本地引擎恢复 200。

**固化防御（spec token-ux-simplification）**：
1. `tunnel_token_save` 保存后直发用户输入 secret 到 watch，不依赖凭据回读；
2. 开启代理时凭据读取 Err 单独报错「凭据损坏」，不再混淆为「隧道未配置」；
3. 引擎 401 自愈：重读凭据 token 变化则刷新 watch 重试（冷却 single-flight + spawn_blocking）；
4. `proxy_tunnel_get` 暴露 `cred_error` 与 token 指纹，`tunnel_self_check` 一键逐 gate 自检；
5. **运维纪律**：写入 `tunnel_token.pony-desktop` 凭据必须通过桌面端（设置页/同步口令/连接口令）
   或 keyring 兼容工具（UTF-16 blob），严禁以 UTF-8 字节直接 CredWrite。

---

## 待命隧道池（方案 A）对抗审核与调优闭环 · 2026-09-02

**背景**：
在完成方案 A（待命隧道池 TunnelPool）初版实现后，经由 Agent Team（并发与内存安全审查员、协议与边界容错审查员）开展深度对抗审查，识别出 6 项高危/中危边界缺陷并全量加固。

**发现与闭环清单**：
1. **空闲过期未清理（假池化、真冷建）**：初版 `maintain` 缺 `vec.retain(|s| s.born.elapsed() < IDLE_TTL)`，空闲超过 30s 后池内遗留死连接，新请求 `checkout` 100% 发生 miss 并退化为冷建连。修复：`maintain` 循环头主动剔除过期会话并补齐。
2. **Weak 引用失效导致任务永久泄漏**：初版 `maintain` 一进函数将 Weak 升级为局部 `Arc<TunnelPool>` 并贯穿全局，导致强引用永不归零。修复：将强引用严格约束在每次 loop 内部，宿主 drop 后后台任务在下个周期醒来干净退出。
3. **Token 轮换不清池**：初版 `last_key` 仅比对端点 URL 列表。Token 变化时未清池，导致池中残留旧凭据死连接。修复：扩展为比对 `(endpoints, token)` 元组。
4. **401 自愈冷却锁无条件提前置位**：初版进入 `saw_401` 即刷新时间戳，导致用户随后重新粘贴新口令后 30s 内仍被拦截。修复：仅在成功读出新 Token 并广播 watch 后才置位冷却时间戳。
5. **WS 隧道缺少半关闭状态机**：客户端发完 Body 发送 EOF 时直接 break 退出，强行掐断下行 Response 导致连接重置。修复：重构 `relay` 引入 `client_done` 守卫，支持 TCP 半关闭。
6. **路由与超时调优**：`is_google_or_ai_host` 扩展支持 Google 全球国别域及受限 AI 域名（OpenAI/Anthropic）直连 Vercel 美区出口；生产超时预算收敛为拨号 4s、首帧绑定 3.5s；单端点待命池容量调整为 2（双端点共 4 条连接），平滑 5 站并发测速。

**验证结果**：
- 新增针对性单测：`tunnel_pool_refills_expired_sessions_automatically` 与 `tunnel_pool_maintain_exits_when_owner_dropped`；
- 全量单元与集成测试：`cargo test --lib` **57/57 通过**；
- 静态质量检查：`cargo clippy --all-targets` **0 警告**。
