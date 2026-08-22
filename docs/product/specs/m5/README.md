# M5 任务级 Specs — Windows 桌面端（Tauri 2）

> 上游: [ROADMAP M5](../../ROADMAP.md) + [TECH_DESIGN §2.3](../../../TECH_DESIGN.md) | 状态: 已审核（2026-08-22 对抗评审 F1-F23 → R1-R7 全部回填） | 日期: 2026-08-22
>
> 前置裁决（已定，实现不可偏离）：
> - **ADR-007**：管理面远程通道走 tailnet——服务端重绑 `PPROXY_LISTEN_ADMIN=<tailnet-ip>:8900`；Windows 入网直连。管理面公网化 + CF Access 认证记为未来拓展（非当前路线）
> - **R1 通信方案**：WebView 内原生 fetch 必被 CORS 拦截（服务端无 CORS 层，且 CSP host-source 不支持 CIDR 无法按 tailnet 网段收窄）→ **采用 `tauri-plugin-http`**：请求走 Rust 侧发出，天然绕开 CORS，权限 scope 收敛到 `http://<tailnet-ip>:8900/*`；服务端**零 CORS 改动**。api client 做双后适配器（Tauri 环境→plugin fetch，浏览器 dev→原生 fetch+MSW）
> - **构建策略**：Windows 安装包经 GitHub Actions `windows-latest` 产出（tauri-action，pin commit SHA + rust-cache），本机不做 Windows 交叉编译
> - **迭代双轨**：前端纯 Vite SPA 在 Linux 浏览器快速开发（MSW mock），Tauri 壳与安装包按里程碑节奏经 CI 构建

## 0. 现状探测结论（2026-08-22 实测）

| 项 | 实测 | 结论 |
|----|------|------|
| Node / npm / pnpm | v22.21.0 / 10.9.4 / 11.7.0 | 前端工具链就绪 |
| Rust Windows 目标 / tauri-cli / NSIS / wine | 均无 | **本机不能产出 Windows 包**，构建外置 CI（官方不支持 NSIS cross-build） |
| GitHub CLI | 已认证 `lanhui100`；**仓库未创建**（git 无 remote） | repo 引导是 M5 关键路径首任务（R5） |
| 服务器防火墙 | **ufw active**（实测） | tailnet 重绑后必须显式放行 tailscale0 接口的 8900（R2），否则验收第一步即死 |
| 服务器监控 env | `pproxy.service` unit 无任何 PPROXY_CF/VERCEL/THRESHOLD/POLL 变量 | 生产当前永不产生告警；告警闭环验收需前置准备（R4） |
| tailnet IP | <TAILNET_IP>（节点重新认证才变更，key 过期断连不改 IP） | 部署与 scope 引用此值 |
| WebView2 | Windows 10/11 系统自带 evergreen | 无需分发运行时 |

## 1. 目标与范围

Windows 桌面应用 `pony-desktop`：管理 pproxy 的 tokens/routes/usage/alerts，作为日常运维主界面。Tauri 2 + React 18 + TS + Tailwind + shadcn/ui，NSIS 安装包交付。

验收场景（ROADMAP 原文）：Windows 上安装 → 连接 dev 服务器 → 完成路由/token 管理 → 收到告警通知（R4 前置条件见 §7.2）。

不在范围内：
- 自动更新（tauri-plugin-updater，P1）；手机端（Tauri mobile，P2）
- 管理面公网化（ADR-007 未来拓展，前置 CF Access + 限速补齐）
- 多后端 profile 管理（单后端假设，Settings 改地址即覆盖）
- "CF Tunnel 状态"页签（TECH_DESIGN §2.3 所列）：隧道健康可经 127.0.0.1:19099 metrics 探测，但属范围取舍而非技术不可行——**M5 移除，登记为后续可选项**

## 2. 架构

```
┌─ pony-desktop (Windows) ─────────────────────────────┐
│  React 18 SPA（Vite + TS + Tailwind + shadcn/ui）     │
│   ├─ api client：双后适配器（Tauri→plugin-http /       │
│   │              浏览器→原生 fetch+MSW），契约=API.md   │
│   ├─ 页面×5：Dashboard/Routes/Tokens/Usage/Settings   │
│   └─ Tauri 边界：notification 插件 + keyring 命令对 +  │
│      http 插件（scope 白名单）——无其他自定义命令        │
│  Tauri 2 壳（WebView2，CSP 收敛，见 §6 安全横切）      │
└──────────────┬───────────────────────────────────────┘
               │ http://<tailnet-ip>:8900/api/*（Bearer admin，ADR-007）
               ▼
        pony-server 管理面（唯一业务状态源）
```

- **前后端契约唯一出处 = `docs/ops/API.md`**：响应形状以 **zod schema** 单一表达，MSW handler 返回前过同一 schema；CI 另设针对 dev 服务器的只读契约 smoke（node 脚本实拉真实响应过同一 schema）——双向夹住契约漂移（R6/F12）
- **业务数据不做本地缓存，全量实时拉取**（本地仅存：后端地址、轮询间隔、通知去重 id 集——cap 5000 条 FIFO，F14）
- Tauri 边界最小化：notification 插件、`credential_get/set` 命令对、http 插件（scope `http://<tailnet-ip>:8900/*`）

## 3. 构建与发布策略

**repo 引导（关键路径首任务，R5，按序执行）**：
1. `gh repo create lanhui100/pproxy --private`
2. **全历史密钥扫描**（gitleaks，针对完整 git history）——归零才允许 push；重点：config.json.bak 曾在工作区、测试脚本含 token 模式串、docs 引用凭据路径
3. push 全历史 → Actions 首跑验证 → （可选）分支保护

| 环节 | 方式 |
|------|------|
| 日常 UI 迭代 | Linux 本机 `pnpm dev`（Vite + MSW），秒级反馈 |
| 安装包 | tag push（`desktop-v*`）→ Actions windows-latest → tauri-action → NSIS 挂 GitHub Release；tauri-action pin commit SHA；Swatinem/rust-cache 缓存（冷缓存首跑 20-30min 属预期） |
| CI 触发纪律（F15） | Linux job（lint+vitest+cargo test workspace）跑 PR/push；Windows job 仅 `paths: desktop/**` 或 tag 触发，控 2× 计费分钟 |
| 仓库 | private；免费额度 2000 min/月（Windows 2× 折算），月度打包节奏充裕 |

目录布局：
```
desktop/            # Tauri 2 应用根（独立构建图，workspace Cargo.toml 不吸收）
  src-tauri/        # Rust 壳：tauri.conf.json、keyring 命令对、插件注册
  src/              # React SPA（含 api/ schema/ 页面/ 组件）
```

## 4. 页面规格（数据源全部来自现有 API）

| 页面 | 数据源 | 要点 |
|------|--------|------|
| Dashboard | `/api/health` + `/api/usage?hours=24` + `/api/alerts?unread=1` + `/api/quota` | 状态灯（health.status/db）、路由健康列表、**近 24h** requests/bytes 合计（滚动窗口语义，F16 文案）、未读告警条、quota 来源徽标（ok/error/disabled/unsupported_plan） |
| Routes | `/api/routes` CRUD + `/api/routes/{name}/test` | 添加表单（name/host/override 可选）、行内 test（latency/status）、enable/disable 开关——**PATCH 三态 double_option 序列化必须覆盖 null（清除）与字段缺席（不改）两分支**（F17）、删除二次确认 |
| Tokens | `/api/tokens` CRUD | status 徽标、创建对话框含 **expires_days 可选字段**（契约存在）、明文一次性展示 + 复制（剪贴板策略见 §6）、撤销二次确认 |
| Usage | `/api/usage?hours=N&route=&token_id=` + `/api/quota` | recharts 按 route 聚合图表、token 维度表格、quota 进度条（pct=-1 显示"未知上限"哨兵语义而非进度条） |
| Settings | 本地 + `GET /api/monitor/config`（§6） | 后端地址明文本地存、admin token 写 OS 凭据库、连接测试按钮、服务端监控配置只读展示、连接失败文案区分 DNS 失败/拒绝/超时（F10） |

错误分流表（横切，R7/F8——优先级自上而下）：

| 情形 | 判定 | 去向 |
|------|------|------|
| 网络错误（fetch reject/超时） | 非 HTTP 响应 | 页内错误横幅"无法连接后端"，**不跳 Settings**（tailnet 断线≠凭据失效） |
| 401 | HTTP 状态 | 全局拦截跳 Settings 引导重输；**豁免名单**：Settings 页自身的连接测试；**不清除 keyring 凭据**（防服务端轮换期洗掉好凭据）；并发多请求 401 去抖单次导航 |
| 其他 4xx | HTTP 状态 | `error` 字段原样上屏（API.md 固定文案契约） |
| 5xx | HTTP 状态 | 页内错误横幅 + 重试按钮 |

## 5. 通知（tauri-plugin-notification）

- 轮询 `GET /api/alerts?unread=1&limit=50`，默认 5 分钟可配；去重集 cap 5000 FIFO（F14）
- 新 id → OS 通知 `{level}: {message}`
- 平台坑兜底（F18）：前台恢复时立即刷新一次轮询；应用内未读横幅常驻兜底（Focus Assist 吞通知 / dev 裸 exe 无 AUMID toast 静默——验收用安装版规避）
- 不自动标记已读——Dashboard 显式操作（POST read 幂等）

## 6. 服务端改动与部署（R2/R3/R6/R7）

### 6.1 代码改动（crates/server，含测试）
1. **`GET /api/monitor/config`**（admin Bearer）：**手写白名单响应结构体**（仅 `threshold_pct`/`poll_interval_sec` 两字段），**禁止对 MonitorConfig 本体 derive Serialize**（其含 cf_token/vercel_token/webhook_url，F6 凭据红线）；单测断言响应键集精确等于两字段；挂入现有鉴权 router 组，错误文案沿用 ERR_* 契约
2. CI 补位：Linux job 增加 `cargo test --workspace`（服务端测试此前不在任何 CI，F6）

### 6.2 部署程序（按序，含回滚）
1. **ufw 放行**（R2/F2）：`sudo ufw allow in on tailscale0 to any port 8900 proto tcp`——缺此步 Windows 必连不上
2. **重绑监听**：`pproxy.service` 追加 `Environment=PPROXY_LISTEN_ADMIN=<TAILNET_IP>:8900` → daemon-reload → restart（数据面秒级中断，选低峰执行，CF Tunnel 侧短暂 502 属预期，F22）
3. **同步本机 CLI**：`~/.pony/config.toml` server 地址改 tailnet IP（本机进程经 tailnet IP 自访可达）
4. **Tailscale ACL 收敛**（R7/F10）：管理面对整个 tailnet 可达仅剩 Bearer 单层——ACL 限定 8900 仅 Windows 设备 IP 可访（示例入 DEPLOY.md）
5. **验证**：`ss` 断言 8900 仅绑 tailnet IP；Windows 侧连通 + 401 同体验证
6. **回滚**：删 Environment 行 + `sudo ufw delete allow in on tailscale0 to any port 8900 proto tcp` + config.toml 还原

### 6.3 m4_test.sh 参数化（R3/F3——重绑必然打挂现套件）
- admin base URL 从环境变量取（缺省 tailnet IP）；`ss` 断言改"8900 仅绑 {loopback|tailnet-ip} 其一"；四处 `curl 127.0.0.1:8900` 全部改走参数化 base
- 该改造列入 M5 变更清单；§9 回归口径 = "更新后的 m1–m4 套件通过"

### 6.4 安全横切（R7）
- **CSP**：`default-src 'self'`；因网络请求全走 plugin-http（Rust 侧），WebView **无需任何外联**，`connect-src 'none'`；禁远程内容
- **capabilities 最小授权**：仅 notification + http（scope 锁 tailnet URL）+ 自定义 keyring 命令；release 构建 `devtools` 关闭
- **admin token 卫生**：不落 localStorage/不进 console/日志/错误上报；"销毁内存副本"降格为可实现承诺——**不落盘、不进日志、卸载时置空引用**（JS 字符串不可变，过度承诺即假话）
- **剪贴板策略（F9）**：复制后 60s 自动清空并提示；Settings 提供"轮换 admin token"指引（服务端 PPROXY_ADMIN_TOKEN 注入重启）作为泄露兜底；API.md 剪贴板同步禁令对桌面端同样生效
- **dev fallback 文件模式护栏（F21）**：仅 `debug_assertions` 生效 + CI 打包前断言 release 不含该 feature + 路径固定进 .gitignore

## 7. 测试清单

### 7.1 自动化（CI）
- Linux job：eslint + tsc + vitest（下述）+ `cargo test --workspace`（§6.1）
  1. MSW 契约测试：zod schema 双端复用，handler 覆盖查询串分支（hours 边界 400、unread=1、limit>500 钳制）
  2. **契约 smoke job**：node 脚本对 dev 服务器实拉只读端点（/api/health、/api/quota、/api/monitor/config）过同一 zod schema——真值防漂移（F12）
  3. 组件测试：Tokens 创建流程（明文一次性+复制+关闭不可回看）、Routes PATCH 三态两分支、401 去抖单次导航、401 不清凭据
  4. 纯函数：Usage 聚合换算、pct=-1 哨兵渲染分支、通知去重 + cap
- Windows job（仅 desktop/** 或 tag）：cargo clippy/test（src-tauri）+ NSIS 打包

### 7.2 手动验收（Windows 实机，含前置准备）

**前置 A（网络）**：Windows 安装 Tailscale 入网 + ACL 生效验证（F23）——清单从"可达后端"开始，不再从零假设网络
**前置 B（告警来源，R4/F4）**：生产 unit 现无任何监控 env → 永不告警。两档：
- 首选：用户提供真实 CF 凭据 → unit 经 EnvironmentFile 注入（600 权限、去 export 前缀，M3-R2 口径）
- 降级（无凭据时）：unit 临时注入 dummy 凭据 + `PPROXY_CF_GRAPHQL_URL=http://127.0.0.1:18894/graphql`（复用 m3_stub）+ `PPROXY_ALERT_THRESHOLD_PCT=50` + `PPROXY_POLL_INTERVAL_SEC=60` → 确定性告警，**验收后还原 unit**；预计触发延迟上界 = 1 个轮询周期 + 通知轮询 5min ≈ 6min

验收步骤：
1. NSIS 安装（预期 SmartScreen 未签名警告，F19）→ 启动 → Settings 配置 tailnet 地址 + admin token（凭据库写入验证：重启应用免重输）
2. 五页功能走查：创建 token（含 expires_days）→ 数据面实测转发 → 撤销 → 401 复验；添加路由 → test 通过 → 禁用生效（PATCH 三态）
3. 告警闭环（前置 B 就绪后）：触发 → ≤6min 收 OS 通知 → Dashboard 已读操作闭环
4. 变体：中文/非 ASCII 安装路径安装启动（F19）
5. 卸载：NSIS 卸载应用文件干净；**凭据管理器条目 by-design 保留**（卸载器不碰凭据防误删，文档注明手动清理路径：控制面板→凭据管理器，F13 落纸判据）

## 8. 风险与缓解

| 风险 | 缓解 |
|------|------|
| Actions Windows 分钟（2× 计费，冷缓存 20-30min/次） | 仅 desktop/** 与 tag 触发 + rust-cache；月度节奏充裕 |
| GitHub 国内拉 Release 慢 | 一次性下载可接受；P1 updater 再议镜像 |
| tauri v2 + shadcn/ui 版本磨合 | 双轨隔离：UI 问题在浏览器层消化，壳问题 CI 小步验证 |
| keyring 平台差异 | Windows Credential Manager 成熟（CredWrite/CredRead）；dev fallback 按 §6.4 护栏 |
| admin 重绑影响面 | §6.3 参数化 m4 + §6.2 回滚步骤 + config.toml 同步 |
| WebView2 版本 | evergreen，不用旧版特性 |

## 9. 验收标准

- CI 全绿：Linux job（前端 lint/vitest + **cargo test workspace** + 契约 smoke）+ Windows job 产出 NSIS artifact
- **更新后的 m1/m2/m3/m4 集成套件全通过**（m4 经 §6.3 参数化改造；口径修正自"无回归"——重绑本身是对 m4 断言对象的真实变更，R3）
- Windows 实机走完 §7.2 全清单（含前置 A/B 与告警闭环或其降级口径）
- 文档同步清单（F11 全集）：API.md（+/api/monitor/config）、CURRENT.md（拓扑补 desktop + tailnet 管理通道，ADR-007 标注）、DEPLOY.md（重绑/ufw/ACL/token 轮换 runbook）、TROUBLESHOOTING.md（连接失败三分支排查）、ROADMAP M5 ✅、README 组件清单、**scripts/install.sh 过期 /stats 提示顺手修正**
- 手机 4G 实测（M4 顺延项）在本里程碑验收后一并执行（用户裁决 2026-08-22）
