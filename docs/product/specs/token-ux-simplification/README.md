# spec：授权码傻瓜化（连接口令 + 指纹自检 + 健壮性修复）

## 1. 背景

2026-09-01 事故（详见 DESKTOP-TROUBLESHOOTING）暴露三类问题：

1. **凭据编码事故**：外部脚本以 UTF-8 写入 Windows 凭据，keyring 3.6.3 按 UTF-16 解码失败 →
   `get_password()` 报错 → 开启代理时报误导性「隧道未配置」；经同步口令导入的内存旧 token → gate 401。
2. **设置页概念混乱**：方案 A 存在两个输入框（「加速授权码 (Cloudflare Token)」与「隧道令牌」），
   实际写入**同一凭据槽** `tunnel_token`；且「获取授权码」链接指向 Cloudflare API Tokens 页面——
   tunnel_token 与 Cloudflare API Token 完全无关，严重误导。
3. **无自愈**：gate 返回 401 时引擎只用内存缓存 token 重试，凭据更新后必须重启应用。

## 2. 目标

- 用户只面对**一个授权码概念**：方案 A = 加速授权码（隧道令牌），方案 B = 远端代理密码，文案清晰区分。
- 支持**单一连接口令**：一个字符串同时携带 gate URL + token，粘贴即完成方案 A 全部配置。
- 展示本机 token 指纹（SHA-256 前 8 位）+ 一键三端连通自检。
- 凭据读取失败、401 两类故障自愈或给出准确报错。

## 3. 范围与非目标

**范围**：桌面端（`desktop/src-tauri/src/lib.rs`、`proxy/engine_tunnel.rs`、`src/views/SettingsView.vue`、
`src/lib/config.ts`）、运维脚本 `deploy/gen-connect-code.mjs`、文档。

**非目标**：不改 gate 端（CF/Vercel worker）协议；不动方案 B 认证逻辑；不引入服务端配置分发。

## 4. 方案

### 4.1 连接口令格式（新增，长期有效）

```
pony-gate://<base64url-no-pad(JSON)>
JSON: {"u": "<gate url，逗号分隔多端点>", "t": "<tunnel_token 明文>"}
```

- 无加密、无过期、无防重放（与同步口令 pproxy-sync:// 定位不同：同步口令用于设备间一次性迁移；
  连接口令是管理员签发的长期配置载体，机密性与 token 本身等同）。
- 生成工具：`deploy/gen-connect-code.mjs <url> <token>` 输出口令。

### 4.2 后端（Rust）

1. **`tunnel_token_save` 直发 watch**：保存后用用户输入的 secret 直接
   `ensure_tunnel_watch().send((url, Some(secret)))`，不再依赖凭据回读（对齐 `configure_direct_tunnel` 既有模式）。
2. **凭据自检**：`proxy_tunnel_get` 增加 `cred_error: Option<String>`——`get_password` 返回 Err 时
   透传「凭据损坏或编码不兼容，请重新粘贴授权码」类提示（不再静默吞错为"未配置"）。
3. **401 自愈**：`engine_tunnel::establish` 全部端点 401 失败时，调用 `crate::tunnel_config_load()`
   重读凭据；若 token 变化则更新 watch 并重试一轮。仅在「确为 401 且重读后 token 不同」时重试，防死循环。
4. **新命令 `tunnel_connect_code_import(code)`**：解析 pony-gate:// 口令 → 校验 URL（复用
   `validate_tunnel_url`）→ 写 tunnel.json + 凭据 → 直发 watch。返回 fingerprint。
5. **新命令 `tunnel_self_check()`**：返回 `{ fingerprint, cred_ok, cred_error, gates: [{name, url, ok, ms, error}] }`。
   fingerprint = 本机 token SHA-256 前 8 位 hex；gates 复用 `probe_gate_rtt` 逐端点实测（WS 升级成功即证明
   该端哈希与本机 token 一致，比 /debug 的 {set:bool} 更强）。

### 4.3 前端（SettingsView.vue）

1. **方案 A 合并为单一卡片**：
   - 删除「更新加速授权码 (Cloudflare Token)」输入框与 Cloudflare 外链；
   - 删除高级区重复的「隧道令牌」输入框（保留隧道端点 URL 高级编辑，默认折叠语义不变）；
   - 主输入框：「连接口令 / 加速授权码」，接受 `pony-gate://` 口令（一键完成）或裸 token（仅更新令牌，
     端点沿用默认双 gate）。裸 token 走 `tunnel_token_save`；口令走 `tunnel_connect_code_import`。
2. **授权码指引 InfoTip**（复用既有 `components/common/InfoTip.vue`）：hover 展示
   「什么是加速授权码、如何获取（向服务提供方/部署管理员索取；运维侧用 gen-connect-code.mjs 生成）」。
3. **已保存状态区**：显示指纹 `sha256:xxxx…`、凭据健康状态（cred_error 时红字提示并引导重贴）、
   「自检」按钮触发 `tunnel_self_check` 逐 gate 展示结果。
4. **方案 B**：密码框标签改为「远端代理密码（连接你自己的代理服务器，与加速授权码无关）」，
   同样配 InfoTip 说明获取途径（pproxy user add / sync export）。

### 4.4 错误文案修正

- 「隧道未配置」细分为：缺 URL / 缺 token / 凭据读取失败（携带具体 cred_error）三种准确提示。

## 5. 影响面与依赖

- `engine_tunnel.rs` 增加对 `crate::` 两个函数的调用（需 `pub(crate)`）；无协议变化。
- 前端仅 SettingsView.vue 与 lib/config.ts；InfoTip 组件复用。
- 既有行为兼容：裸 token、旧同步口令、pproxy:// 口令全部照常工作。

## 6. 风险与回滚

| 风险 | 缓解 |
|---|---|
| 401 自愈导致凭据被并发反复重写 | 仅「token 变化」才重试一次；watch send 幂等 |
| 口令明文载体被截屏外泄 | 文案提示与 token 同级机密；口令仅本机粘贴 |
| 自检命令被滥用探测 | 仅本机 localhost 前端调用，无新攻击面 |
| 回滚 | git revert 单提交；不涉及持久化格式变更 |

## 7. 测试计划

1. Rust 单测：pony-gate:// 解析（合法/坏 base64/坏 JSON/缺字段/非法 URL）；401 自愈逻辑（mock watch + 双端点）；
   fingerprint 计算。
2. Vitest：前端口令/裸 token 判定逻辑（抽纯函数 `parseGateInput`）。
3. `cargo test`（desktop）全量 + `pnpm vitest run` 全量。
4. 人工验证：真机粘贴 pony-gate:// 口令 → 开启代理 → google.com 通；篡改凭据编码复现旧事故 →
   设置页出现准确 cred_error 提示。

## 8. 验收标准

1. 设置页方案 A 只剩一个授权码输入框，文案无「Cloudflare Token」字样与外链。
2. pony-gate:// 口令粘贴即完成 URL+token 配置并即时生效（不重启）。
3. 设置页可见本机 token 指纹与逐 gate 自检结果。
4. 凭据 UTF-8 污染场景下，开启代理报「凭据损坏」类准确提示而非「隧道未配置」。
5. 凭据外部更新后，引擎 401 自愈重连成功（不重启应用）。
6. 全部自动化测试通过。

## 9. 审核记录

审核 A（架构/边界，子代理）：有条件通过，P0×2 / P1×6。审核 B（安全/UX）因子代理基础设施故障降级为编排者自审。
采纳汇总：

| 意见 | 决定 | 落实 |
|---|---|---|
| P0-1 401 无法用字符串判定 | **采纳** | `try_establish_url` 在 tungstenite `Http` 错误 status==401 时打稳定标记 `tunnel_auth_401`，establish 只认标记 |
| P0-2 自愈竞态/清场/读放大 | **采纳** | 三条不变式：load 为 None 不回写；send 前与当前 watch 快照比较；进程内 30s 冷却 single-flight；keyring 读走 `spawn_blocking` |
| P1-1 错误三分与 helper 矛盾 | **部分采纳** | 开启路径直接调 `cred_get_impl` 区分凭据错误（已落地），`tunnel_config_load` 签名保持不变式 |
| P1-2 pony-gate:// 投毒入口 | **采纳** | 连接口令强制 `wss://`；JSON 支持可选 `v` 字段；非官方域名的端点导入后前端 toast 警示 |
| P1-3 裸 token 的 URL 终态 | **采纳** | 裸 token 保存时：已有合法 URL 则沿用；无/失效 URL 则自动写入默认双 gate 端点并落盘 |
| P1-4 与既有口令入口冲突 | **采纳** | `proxy_import_sync` 识别 `pony-gate://` 前缀直接转发 `tunnel_connect_code_import`；同步口令覆盖端点为既有行为，文档注明 |
| P1-5 fingerprint 二义性 | **采纳** | 定义为「凭据回读 token 的 SHA-256 hex 前 8 字符（4 字节）」；回读失败时 fingerprint=null 且 cred_error 展示 |
| P1-6 持久 401 无前端事件 | **不采纳** | engine 无 AppHandle 通道，成本高；设置页/首页自检已覆盖发现路径，列为后续项 |
| P2-1 分层破坏（engine 回调 crate） | **不采纳** | 自愈被 401 标记严格门控，既有引擎单测不触发；注入回调重构收益不抵成本，留作技术债 |
| P2-3 仪表盘拨测吞凭据错误 | **采纳** | `proxy_test_egress` 在凭据读取 Err 时返回失败并携带 cred_error，不再回落 TCP 假绿 |
| P2-4 回滚叙述 | **采纳** | 回滚按多提交 revert + 旧版对多端点 URL 兼容（split 逻辑 ≥0.3.20 已具备） |
| P3-3 gen-connect-code.mjs 不存在 | **采纳** | 随本任务一并交付，含输入校验与输出样例 |

代码审核：前端子代理交付完整审查（P1×2/P2/P3），采纳并修复：
- P1-1 loadTunnelConfig snake_case→camelCase 显式映射（含回归单测 mapTunnelConfig×3；顺带修复预存的 hasToken 字段漂移 bug）；
- P1-2 官方域名判定改为 URL hostname 精确匹配（仿冒子串测试已补）；
- P2 atob→TextDecoder UTF-8 解码；非官方端点导入只保留警示 toast 不叠加成功且不自动开代理；错误兜底 `?? '未知错误'`；
- P3 口令 v 版本字段校验（缺省 v1，不兼容版本拒绝）；自检 ms 仅 number 时渲染。
后端子代理中途失败，关键点由编排者在非退回前自审并复核（401 自愈守卫、凭据错误透传）。门禁：cargo test 47/47、vitest 78/78、vue-tsc 通过。
