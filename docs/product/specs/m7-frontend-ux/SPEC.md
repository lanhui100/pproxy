# M7 · 桌面前端 UX 打磨 — 诊断与改造方案（SPEC v2）

> 状态: v2 —— 已吸收对抗审核 R1 三路报告（UX 16 条 / ENG 13 条 / SEC+GOV 12 条），裁决见 §11
> 范围: `desktop/src` + src-tauri 白名单例外（§4）| 分支: `feat/ux-polish`（worktree）
> 定位声明: 本 spec 仅覆盖 ROADMAP M7 的**前端切片**；模板库后端化/config export 全覆盖/部署文档/全链路回归不在本 spec。

## 0. 背景与目标

Pony Proxy 桌面端当前 UI 是按管理 API 形状直接铺出的"工程面板"。本次打磨为**面向用户的
傻瓜式操作 + 极简视觉**。可证伪口径：

1. 五步主旅程（连后端 →建设备密钥 → 复制接入配置 → 加服务 → 看用量）每步有唯一显性下一步。
2. 全界面中文；英文枚举/术语零直出（专有名词除外）；含 OS 通知与 toast 文案。
3. 每页具备 加载 / 出错重试 / 空 / 未配置 / 401 五态（§8）。
4. **生成的接入 base_url 必须指向数据面地址**（端口 ≠ 管理面端口，或经推导函数单测钉住）。

## 1. 诊断（v1 结论维持，摘要）

全局：语言混杂(G1)/无首启引导(G2)/错误反馈生硬(G3)/无加载态(G4)/默认 zinc 高密度+Google Fonts 死代码(G5)/枚举数值直出(G6)。
分页：总览术语化无行动入口(D1-D4)；密钥有效期裸数字、明文弹窗无接入示例(T1-T4)；路由手填三技术字段、test 按钮工程风(R1-R4)；用量 24h/168h/720h 与英文表头(U1-U3)；设置四卡混杂、测试连接隐式保存(S1-S5)。
R1 补充确诊：Tauri http scope 只放行 3 个开发者地址（首启必死）；管理面(8900)/数据面(8899)双地址概念前端零感知；明文弹窗误触即永久丢失；服务端英文错误串原样上屏；轮询间隔改动不热生效。

## 2. 设计原则

1. 一屏一事：每页一个主任务；主操作唯一显著按钮。
2. 说人话：全中文；状态一律「彩点+中文词」（全枚举映射表见 §9.2，未知值灰点+"未知"）；
   数字人性化（`fmtBytes/fmtCount/fmtRelative`）。
3. 引导优先：未配置态是全局门槛（§3.1）；空态必给下一步动作；帮助文案必须真实可执行（R1 教训：假命令比没有文案更糟）。
4. 渐进披露：专业参数收进折叠/预设档。
5. 反馈分级（防 toast 噪音）：破坏性/不可逆结果与**一切失败** → toast；有行内状态变化的成功 → 仅行内反馈；
   后台静默成功不打扰。危险操作保留二次确认。
6. 极简视觉：中性灰阶 + 单强调色；语义色仅绿/黄/红；扁平细边框、无重阴影；系统字体栈；
   8pt 节奏；动效仅限状态反馈 150–200ms。**不做暗色模式**（v2 裁决：砍掉，理由见 §11-A19）。

## 3. 改造方案

### 3.1 壳层（地基组 F1）

- 导航：`总览 / 服务 / 设备密钥 / 用量统计 / 设置`（PRD 心智："给每台设备发钥匙""一键添加服务"；
  页内标题副注保留 token 字样兼容旧文档）。品牌区「Pony Proxy · 个人代理网关」。
  徽标条件改按 `item.to` 匹配并数据化（nav 项加 `badge?: 'alerts'|'update'`）——禁止按 label 文案匹配。
- 窗口标题改 `Pony Proxy`（仅 tauri.conf.json `app.windows[0].title` 单字段，白名单见 §4）。
- **全局未配置门槛**：baseUrl 为空 ⇒ 五个页面统一渲染共享组件 `NeedSetupGuide`
  （说明 + CTA 进设置向导），**不发任何请求**；已配置但不可达才走各自错误态。
- 401 全局拦截保持跳设置页；提示用 vue-router `state` 或内存 flag 承载（**禁用 query**），
  固定文案「登录凭据无效，请在下方重新粘贴 admin token」，不含动态详情。
- 不加暗色开关；`.dark` tokens 保留不动（无 UI 入口即死代码，无害）。

### 3.2 共享基建

**F1（表现层）**：`lib/format.ts`(+vitest)、`components/common/{PageHeader,StatusDot,EmptyState,
ConfirmDialog,SkeletonCard,SkeletonTable,ToastHost}.vue`、`composables/useToast.ts`（模块级单例，
success/info 3s、error 6s 自动消失，aria-live polite）、`components/common/NeedSetupGuide.vue`。
接口签名冻结于 §9.1。

**F2（行为层）**：
- `lib/config.ts`：新增数据面地址存取（key `pony-data-plane-url`）；轮询间隔改为模块级响应式 ref
  （`loadPollIntervalMin` 允许 0 往返，去掉 `Math.max(1,…)` 钳制；0=不自动轮询）。
- `lib/urls.ts`(+vitest)：`deriveDataPlane(adminUrl)` —— 同 scheme+host、端口换 8899，
  语义对齐 crates/cli `derive_from_server`（含非法输入返回 null 的分支）。
- `lib/errors.ts`(+vitest)：已知服务端错误串字典 → 中文（invalid token name / invalid expires_days /
  cannot revoke admin / invalid target host / invalid route name / bad_request / unauthorized /
  not_found 等，实施时对照 crates/server/src/api.rs 全量补齐），视图层经 `errText(e)` 消费，
  未命中回退原文。client.ts 保持零改动。
- `composables/useAlertNotifications.ts`：watch 轮询 ref 变更即时 stop/start（热生效）；0 ⇒ 执行一次
  初始 pollOnce 但不建 interval；OS 通知标题改「用量告警」、级别前缀中文（严重/警告，复用 format 映射）。
- `composables/useSecretCopy.ts`：一切承载秘密的复制唯一入口——新复制取消旧 60s 定时器再重设；
  关闭对话框取消全部待清任务；toast 只报「已复制（60 秒后自动清空剪贴板）」不含复制内容。
- `src-tauri/capabilities/default.json`：http scope 放开为通配（覆盖任意 http/https 主机与端口；
  实施时按 Tauri v2 URL pattern 语法核实最宽合法写法并在注释记录依据）。CSP 不动。
- `scripts/check-invariants.sh`、`scripts/check-zh.mjs`（门禁脚本，见 §6）。

### 3.3 设置 · 连接向导（页面组 C）

一屏三步卡（逐步打勾）：
1. **管理面地址**：帮助文案「形如 `http://100.x.x.x:8900`，需为其他设备可达的 IP（127.0.0.1 仅限本机）」。
2. **数据面地址（选填）**：留空按 `deriveDataPlane` 自动推导（同主机、端口 8899），展示推导结果预览；
   走公网入口填 `https://access.ponyjob.top`。帮助注明：管理面无需公网可达。
3. **admin token**：帮助文案（真实路径，已核实 CLI 无任何打印/生成命令）：
   「在**服务器**上查看：① 部署时注入的环境变量 `PPROXY_ADMIN_TOKEN`；
   ② 或执行 `journalctl -u pproxy | grep ADMIN_TOKEN`（首次启动仅打印一次）；
   ③ 或服务器 `~/.pony/config.toml` 的 `admin_token` 字段。日志已丢失则设置变量后重启服务端。」
4. 单一原子按钮 **「测试并保存」**：运行时 swap baseUrl/token provider → `api.health({skipAuthRedirect:true})`
   → 成功：落盘 localStorage+凭据库，toast「已连接并保存」；失败：**finally 中恢复原 providers**
   （并发窗口对齐 visibilitychange 轮询），行内错误三分支文案保留，localStorage 与 keyring 均不写。
   无独立"仅测试"按钮。
5. 「告警通知」档位 chips：标准（5 分钟）/ 安静（15 分钟）/ 手动（0，标注「不再自动通知告警」）——
   点击即生效并持久化（无独立保存按钮），依赖 F2 热生效机制。
6. 「软件更新」卡保留；原「服务端监控配置」卡删除，其信息并入告警通知卡一行只读小字：
   「服务端告警阈值 {{threshold_pct}}%，配额每 X 小时轮询一次」。

### 3.4 总览（页面组 A）

- PageHeader + 刷新；三卡重构：**服务状态**（大彩点+运行正常/异常+活跃设备数）、
  **近 24 小时流量**（请求数人性化+↑↓字节）、**上游额度**（进度条摘要，点击进用量页；
  空态文案「暂无额度数据（未启用上游监控或暂不支持）」）。
- 「我的接入」卡：展示完整可复制 base_url 形态 `http://<主机>:8899/<令牌>/<服务>`
  （一律使用**数据面地址**；令牌明文不可得时如实提示到设备密钥页新建，模板含 `<令牌>` 占位符时
  占位符高亮 + 角标说明，复制时 toast 明示「内容含占位符，需替换后使用」）。
- 告警区：中文级别徽章、相对时间、critical 置顶、「全部标为已读」——实现契约：以 limit=500 重拉
  后循环 markAlertRead，按钮 busy，toast 报「已读 N/M」，M<N 提示部分失败可重试。
- 路由健康列表化（名称+启用彩点+上游小字，禁用置灰）；未配置走全局门槛卡。

### 3.5 设备密钥（页面组 B）

- 创建对话框：有效期档位 chips（永久 / 30 天 / 90 天 / 自定义天数）；名称说明「建议用设备名，如 my-laptop」。
- 明文一次性弹窗升级「接入配置」：打开即自动写剪贴板（入 useSecretCopy，60s 自清）；
  Tab=`通用 / Anthropic / OpenAI`（**v-if 实现，DOM 同时只存在一个片段**）；
  每 Tab 内容：大号 base_url（数据面地址拼接）一键复制 + 密钥单独复制 +
  折叠段「环境变量示例」给 bash(`export`) 与 PowerShell(`$env:`) 双版本
  （措辞与 crates/cli/src/export.rs 对齐）。警示：「明文仅此一次展示；片段含明文令牌，请勿截图外发」。
  **关闭防护**：未复制过时拦截 Esc/遮罩关闭 → 二次确认「尚未复制，关闭后将无法再次查看，需要重新创建」；
  关闭即清引用与待清定时器（纪律：不落盘、不进日志、60s 清剪贴板）。
- 表格：状态中文徽章（active 启用/expired 已过期/revoked 已撤销）；`__admin__` 行显示
  「系统管理员（内置）」操作列「—」；过期列「永不过期」；最后使用相对时间；空态 CTA「创建第一个设备密钥」。

### 3.6 服务（页面组 B）

- 添加改两段式：上部**模板网格**（常量表 `lib/serviceTemplates.ts`，前端常量、与服务端解耦；
  行 `{name, label, target_host}`），下部折叠「手动添加（高级）」（override 上游下拉收于此）。
  模板表（name 必须满足服务端校验 `^[a-z][a-z0-9_-]{0,63}$` 且非 `pony_` 前缀；实施时逐条
  web 核验 target_host 并在表内注释出处，核验不了的不上线并记录）：

  | name | label | target_host（待核验） |
  |------|-------|----------------------|
  | openai | OpenAI | api.openai.com |
  | anthropic | Anthropic | api.anthropic.com |
  | gemini | Gemini | generativelanguage.googleapis.com |
  | github | GitHub | github.com |
  | x | X (Twitter) | api.twitter.com |
  | openrouter | OpenRouter | openrouter.ai |
  | groq | Groq | api.groq.com |
  | mistral | Mistral | api.mistral.ai |
  | xai | xAI | api.x.ai |
  | hf | Hugging Face | huggingface.co |

  （Facebook 不做模板——PRD 无此条；存量 facebook 路由仍作为普通行正常管理。
  PRD 提到的 zen 服务无法给出可信 host，不上线，记录于交付报告。）
  上游决策预期与 crates/core VERCEL_HOSTS 自动规则一致，模板不传 override。
- 创建失败反馈必须在 Dialog 内部 error 区渲染，表单不清空、Dialog 不关。
- 列表列：名称 / 目标 / 上游（CF Worker/Vercel 出口，中文徽标）/ 状态（Switch）/ 连通性 / 操作。
  「测速」结果：`正常 · 123ms` / `失败：<原因>`；失败态附内联动作「切换线路 ▾（CF Worker/Vercel，
  PATCH override_upstream）+ 重测」，切换成功自动重测一次。
- 删除确认统一 ConfirmDialog，文案补影响说明；空态 CTA「添加服务」。

### 3.7 用量统计（页面组 A）

- 时间档 segmented：近 24 小时 / 近 7 天 / 近 30 天。
- 表头中文：请求次数 / 上行流量 / 下行流量 / 设备密钥；join 纯函数（随 format.ts 测）：
  `tokenMap.get(id)?.name ?? '#'+id`，revoked 显示「名字（已撤销）」；`Promise.all([usage,quota,listTokens])`
  并行，每次刷新重取不缓存。
- 图表：静态 hex 强调色常量（亮色定值即可，无暗色切换），tooltip 中文化；禁读 CSS 变量进 canvas。
- 额度进度：pct≥0 正常渲染；pct=-1 文案「无固定上限，已用 N」。空数据给说明文案。

### 3.8 设置页其余

见 §3.3。401 跳转落地向导卡并高亮 token 输入框（state/内存 flag）。

## 4. 非目标与白名单例外

- 冻结：`api/client.ts`、`api/schemas.ts` 端点与契约；Rust 代码；updater 链路；productName/bundle。
- **白名单例外（仅此两处可动 src-tauri）**：① `capabilities/default.json` 的 http scope 通配放开
  （现状 3 条硬编码地址会拦死所有新用户的首启，属 P0 缺陷；CSP 不放宽，两者分开评审）；
  ② `tauri.conf.json` 的 `app.windows[0].title` 单字段。两处 diff 均列入验收核对。
- 不引入 i18n 框架/新状态库/图表库更换/第三方 toast·skeleton·test-utils 依赖；
  vitest 保持 node 环境（可测逻辑全部抽纯函数，不做组件挂载测试）。
- 不做多语言、移动端适配、暗色模式。

## 5. 风险与回归控制

| 风险 | 控制 |
|------|------|
| 视图重构碰坏取数逻辑 | api.* 调用序列等价迁移；R2 审核逐页核对 |
| 明文密钥安全承诺退化 | useSecretCopy 唯一入口；关闭清引用；60s 自清；§6 安全纪律复核项 |
| 401 豁免链路 | client.ts 零改动；Settings skipAuthRedirect 保留 |
| 测试连接失败污染全局 providers | §3.3-4 finally 恢复算法（P0 契约，实施不得偏离）|
| 轮询 0 值语义 | config 允许 0 往返 + composable 0 不建 interval + 热生效，三点均单测 |
| chart.js 回归 | register 列表不动；颜色为静态常量 |
| scope 放开的安全性 | 个人工具威胁模型（无第三方内容渲染，URL 仅来自用户设置项）；CSP 不变；R2 安复审 |
| Windows WebView 兼容 | 不用容器查询/subgrid 等新特性 |

## 6. 测试与验收门禁

1. `pnpm check && pnpm lint && pnpm test && pnpm build` 全绿；现有 26 测试不许删改断言。
2. 新增单测（node 环境，全部纯函数）：format 四件套、deriveDataPlane（含非法输入）、errors 字典、
   轮询归一化（0 往返/档位映射）、usage join、告警级别映射。
3. **不变量门禁** `scripts/check-invariants.sh`：`git diff --exit-code <base> -- desktop/src/api/client.ts
   desktop/src/api/schemas.ts` 零差异；tauri.conf.json 除 title 外零差异；capabilities 仅 scope 数组变化。
4. **中文扫描门禁** `scripts/check-zh.mjs`：扫描范围 = 五视图模板文本节点与插值字面量 + composables/lib
   内面向用户字符串（toast/通知/错误字典命中表）；内联专有名词白名单（Anthropic/OpenAI/PowerShell/
   bash/token/base_url/admin token/CF Worker/Vercel 等）；输出违规清单，非零退出。
5. 五态核查表（§8）逐格填**代码证据（file:line）**；第二双眼由 R2 对抗审核承担。
6. 主旅程走查脚本（五步任务单）写入本文档附录供真人验收；自动化环境无法真实多设备联调，
   该项如实标记「待用户验收」，不以自评代替。
7. 安全纪律复核：密钥流不落盘不进日志；剪贴板 60s 自清覆盖所有承载秘密的复制点（枚举清单）；
   向导文案与 CLI 实测一致；scope 变更仅限白名单。

## 7. 实施编排（agent team）

1. **F1 地基·表现层** 与 **F2 地基·行为层** 并行（文件所有权不相交，§3.2）→ 各自跑门禁。
2. **A（总览+用量）/ B（设备密钥+服务）/ C（设置向导）** 并行，冻结在 §9 接口卡版本上；
   页面组禁改他人文件、禁 git 写操作；需要新共享件时上报主控裁决。
3. 主控合流跑全量门禁 → **对抗审核 R2**（新鲜三视角：UX 终审 / 回归风险 / 安全纪律）
   → 修复循环直至通过 → 交付报告。

### Commit 策略（支撑回滚，§10）

F1、F2、A、B、C 各自独立 commit 序列；主控在每组完成后立即提交，保证按组 revert 可行。

## 8. 五态核查表（实施组填报，R2 复核）

| 页面 | 加载 | 错误(可重试) | 空(CTA) | 未配置(壳层门槛) | 401 |
|------|------|--------------|---------|------------------|-----|
| 总览 | ✓ SkeletonCard×3+告警骨架 DashboardView:151-158 | ✓ 整页红条+重试 :163-168 / 旧数据横幅 :170-176 | ✓ 路由空 EmptyState→/routes :289-299；告警空灰字 :257 | ✓ App.vue 壳层统一 NeedSetupGuide | n/a（壳层拦截跳设置）|
| 服务 | ✓ SkeletonTable RoutesView | ✓ 横幅可重试 | ✓ 「还没有服务路由」→添加 | ✓ 同上 | 同上 |
| 设备密钥 | ✓ SkeletonTable TokensView | ✓ Dialog 内 errText :106-109/:343 | ✓ 「创建第一个设备密钥」CTA | ✓ 同上 | 同上 |
| 用量统计 | ✓ 双卡+表骨架 UsageView:138-144 | ✓ :149-154/:156-162 | ✓ 图表 :170 与明细 :206 EmptyState | ✓ 同上 | 同上 |
| 设置(向导) | ✓ monitorConfig 静默加载 | ✓ 测试三分支文案+errText | n/a | ✓ 向导本身即入口 | ✓ authInvalidHint 消费 SettingsView:84-92 |

附加固定条目：创建密钥失败在 Dialog 内可见 ✓(Tokens:343)／添加服务失败在 Dialog 内可见 ✓(Routes 两段式表单区)／
明文弹窗误触被拦截 ✓(Tokens:204-213+:496-505)／测速失败出现切换线路+重测 ✓(Routes:169-189,:292-312)／
全部已读 busy+计数 toast ✓(Dashboard:103-121)

## 9. 接口卡（冻结版，页面组据此开发）

### 9.1 组件与组合式签名

```ts
// lib/format.ts
fmtBytes(n: number): string          // B→KB→MB→GB；<10 保 2 位小数否则 1 位；非法 → '—'
fmtCount(n: number): string          // ≥10000 → 'x.x 万'；其余千分位
fmtRelative(ts: number, now?: number): string // 刚刚/N分钟前/N小时前/昨天/超2天回退 fmtDate
fmtDate(ts: number): string
fmtDateTime(ts: number): string
alertLevelLabel(level: 'warning'|'critical'): '警告'|'严重'

// lib/urls.ts
deriveDataPlane(adminUrl: string): string | null

// lib/errors.ts
errText(e: unknown): string          // 包装 errorMessage(e)，api 类先查字典

// composables/useToast.ts
useToast(): { success(msg); error(msg); info(msg); dismiss(id) } // 模块级单例队列

// composables/useSecretCopy.ts
useSecretCopy(): { copySecret(text: string, opts?: { placeholderHint?: boolean }): Promise<void>;
                   releaseAll(): void }   // 对话框关闭时调用

// lib/config.ts （既有 + 新增）
loadBackendUrl(): string; saveBackendUrl(url: string): void
loadDataPlaneUrl(): string; saveDataPlaneUrl(url: string): void   // 新
pollIntervalMin: Ref<number>       // 新：响应式，0 合法
normalizePollMin(v: number): number // 新：0 保留，负/NaN→5，其余 clamp ≥1
saveAdminToken/clearAdminToken/isTauri 不变

// components/common props
PageHeader { title: string; subtitle?: string } + slot actions
StatusDot { tone: 'ok'|'warn'|'error'|'muted'|'accent'; label: string }
EmptyState { title: string; description?: string } + slot actions
ConfirmDialog { open: boolean; title: string; description?: string;
                confirmText?: string; destructive?: boolean; busy?: boolean }
               emits update:open / confirm
NeedSetupGuide { }            // CTA 内部 RouterLink 到 /settings
SkeletonTable { rows?: number }

// App.vue nav 数据结构
{ to, label, icon, badge?: 'alerts'|'update' }[]
```

### 9.2 枚举 → 中文映射（StatusDot/徽章唯一出口，纯函数 + 单测）

```
QuotaSourceState: ok→运行正常  disabled→已停用  error→异常  unsupported_plan→套餐不支持
TokenStatus:      active→启用  expired→已过期  revoked→已撤销
AlertLevel:       warning→警告  critical→严重
上游:             worker→CF Worker  vercel→Vercel 出口
自由字符串（health.status/db 等）: 灰点 + 原文小字（不猜语义）
```

## 10. 回滚

- 按组 revert：F1/F2/A/B/C 独立 commit 序列（§7）；页级回滚互不牵连，地基组回滚需连带页面组。
- 触发条件：连续两轮修复仍未过门禁；任一 P0 级安全回归（密钥泄漏路径/凭据库误写/scope 越权）。
- 回滚后必须重跑全量门禁并在交付报告记录。

## 11. R1 裁决记录（要点）

- **采纳**：UX-01~14、ENG-1~12、SEC/GOV-1~6 中全部条文级修改；其中关键裁决：
  scope 通配放开（UX-01，含安全性论证入 §5）；数据面双地址+推导函数（SEC-1/UX-02）；
  「测试并保存」单一原子动作+finally 恢复（UX-04+ENG-2）；轮询 0 全链路契约（ENG-1/GOV-3/UX-05）；
  接口卡冻结（ENG-11）；砍暗色模式（UX-11b，极简与非目标精神，消除半成品风险）；
  监控配置降级为一行小字（GOV-2）；Facebook 除名/zen 暂缓（GOV-1/UX-10/ENG-5）；
  明文弹窗防呆+自动复制（UX-08/SEC-4）；测速失败给切换线路动作（UX-09）；
  PowerShell 双版本片段（UX-12）；OS 通知中文化（UX-13/ENG-12）；toast 分级（UX-14）。
- **部分采纳/偏差声明**：GOV-4 的"真人计时走查"以走查脚本+待用户验收替代（环境限制，如实标注）；
  UX-15 导航命名采纳「服务/设备密钥」但保留 token 副注；ENG-7 选方案 a（纯函数化，不加 test-utils）；
  ENG-13 以专用脚本实现可判定扫描（§6.4）。
- **拒绝**：无整条拒绝项；UX-15 的激进命名（去掉"统计"等）未采纳，保守处理。

## 附录 A · 主旅程走查脚本（待用户验收，SPEC §6.6）

> 自动化环境无法真实多设备联调；以下五任务供真人按序走查并计时。
> 硬判据（PRD §5）：T3 ≤ 1 分钟；T2 复制后可直接发起一次成功请求。

| # | 任务 | 步骤 | 通过判据 |
|---|------|------|----------|
| T1 | 连接网关 | 打开应用→总览出现「先连接你的网关」引导→去连接→填管理面地址（数据面留空）→粘贴 admin token→「测试并保存」 | toast「已连接并保存」；总览三卡出数据 |
| T2 | 新设备复制接入配置 | 设备密钥页→创建（档位选 30 天）→弹窗自动复制→切 Anthropic Tab→复制 base_url 片段 | 粘贴出的 URL 端口为数据面(8899/公网)；60s 后剪贴板自清 |
| T3 | 添加一个新服务 | 服务页→添加→模板网格点选（如 Groq）→确认上游自动决策→创建 | 创建成功且出现在列表，全程 ≤1 分钟 |
| T4 | 测速与线路切换 | 该服务行点「测速」→观察结果；若失败点「切换线路」→另一线路→自动重测 | 结果以彩点+中文呈现；切换后自动重测一次 |
| T5 | 看用量与告警处理 | 用量统计切「近 7 天」→看图与明细令牌名列；回总览对告警「全部标为已读」 | 表头全中文、令牌显示名字非 id；toast 报「已读 N/M」 |
| T6 | scope 放行真机冒烟（R2-SEC-1 兜底） | 在 Tauri 打包产物中分别请求管理面 `:8900/api/health` 与数据面 `:8899` 各一次 | 两请求均成功返回，无 scope 拦截报错 |

## 12. R2 对抗审核裁决与修复记录

R2 三路（回归 ENG / 安全 SEC / UX 终审）共提出 4×P0、5×P1、16×P2。裁决与处置：

### P0（全部修复）
| 编号 | 问题 | 处置 |
|------|------|------|
| SEC-1 | capabilities `http://*` 不匹配非默认端口（:8899/:8900 被拦），白名单改动未达成目的 | 实证矩阵（urlpattern 0.3.0 同版本复现）裁决：改 `http://*:*` + `https://*:*`；description 记录依据（tauri#12734 维护者背书、plugins-workspace#2131、scope.rs 仅补 pathname/search/hash）。ENG 附记③的相反判断被实证否定 |
| UX-1/ENG-3 | useBackendGate computed 零响应式依赖永不失效 → 首启保存后门槛死锁 | config.ts 新增响应式源 `backendUrlSaved`，saveBackendUrl 写路径同步；gate 改 computed 消费；新增回归钉测试 |
| ENG-1 | 有效期档位 d30/d90 缺 expires_days → 创建永久密钥 | 新增 lib/expiry.ts 纯函数 + 五分支形状测试；TokensView 消费 |
| ENG-2 | readPollMin 把缺失 key 当 0 → 新装用户默认不自动轮询 | 仅显式存储值参与往返；缺失/空串回落默认 5；config.test.ts 三态钉住 |

### P1（同批修复）
ENG-4 告警时间秒当毫秒（×1000）；ENG-5 EmptyState #actions 插槽错配；ENG-6 deriveDataPlane
IPv6/非数字尾段对齐 Rust；ENG-7 补轮询单测（config.test.ts）；UX-2 cf 枚举映射缺失；
UX-3 连接成功下一步指引；UX-4 自动复制对象标注；SEC-2/ENG-10 useSecretCopy epoch 代际防在途竞态。

### P2（择要同批，其余显式延后）
已修：ENG-8 负数→5；UX-5 失败反馈统一 toast；UX-7①②③；UX-8 禁用原因提示；UX-9 模板覆盖防护；
UX-10 last_ok 上下文；UX-12 空态 CTA+占位符中文化+「目标域名」；SEC-6 general Tab 注释式示例对齐 CLI；
SEC-3/ENG-9 keyring 失败回滚快照+独立文案；SEC-5 清除凭据文案如实化。
**显式延后项已于清账批次全部处理**（见 §13）：UX-6 占位符记法统一（polish 提交）；
UX-11 Chip 抽象/ui-card 统一/StatusDot size prop；SEC-4 check-invariants 换 jq 语义比对；
SEC-5 残余（清除凭据结果如实反馈）。scope 真机冒烟保留为附录 A 用户验收步骤 T6。

### 接口卡偏差声明（ENG-11）
useSecretCopy 实际签名为 `copySecret(text, opts?: { onCopied?: () => void })`，替代冻结卡
`{ placeholderHint }`：调用方自行拼装 toast（含「60 秒后自动清空」固定文案），功能等效、职责更清晰。
§9.1 以本声明为准。另 §9.1 补充：EmptyState 内容必须置于具名插槽 #actions（无默认插槽出口）。

## 13. 技术债清账批次

| 债项 | 处置 | 落点 |
|------|------|------|
| UX-11 视觉单一出口 | 新建 Chip.vue（选中态档位按钮唯一出口）；StatusDot 增加 size prop 替代 DOM 穿透 hack；Dashboard×4/Usage×2 手写卡统一 ui/card（Card py-4+Content px-4 等价原 p-4，未叠加避免双倍内边距） | components/common/{Chip,StatusDot}.vue + 四视图 |
| SEC-4 门禁脚本可信度 | check-invariants.sh ②③改 jq -S 语义比对：tauri.conf del(.app.windows[0].title) 后全等；capabilities 纯 {url} 条目折叠 {} + unique（豁免 url 数组增删改排序）+ 走私键必现形；缺 jq exit 3 / 坏 ref exit 2 / JSON 解析失败即违规。自测矩阵证明旧两例绕过现均被指认、负例不误伤 | scripts/check-invariants.sh |
| SEC-5 残余 | clearAdminToken 返回 boolean；forgetToken 成功才清 hasStoredToken（步骤③打勾不失真），失败 toast.error 给人工兜底路径 | lib/config.ts + SettingsView |
