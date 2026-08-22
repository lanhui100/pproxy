# Pony Proxy — 里程碑

> 原则：每个里程碑结束时可验证、可演示；后端先行，客户端跟进。

## M1 后端强化（数据面鉴权 + 管理面 + 存储）— ✅ 已完成（2026-08-21）
- [x] token 模型（哈希存储、路径鉴权 `/{token}/{route}/...`、header 兼容）
- [x] 管理面 REST API：tokens/routes/usage/health CRUD
- [x] SQLite 落地（tokens/routes/usage_hourly），config.json 迁移导入
- [x] per-route/per-token 请求与字节计数（实时内存 + 每小时落库）
- [x] 路由自动上游选择 + override
- [x] 现有 7 路由迁移验证（集成脚本 m1_test.sh 全绿；离线模式 52 断言）
**验收**：curl 带 token 走通 openai/zen；无 token 401；`/api/routes` 可增删路由并即时生效
（在线步骤 4/5 因 dev 服务器外网不可达按 T8 §1 离线子集口径验收）

## M2 CLI — ✅ 已完成（2026-08-21）
- [x] pony status / start / stop / restart（systemd 直调，不经 shell）
- [x] pony route list/add/rm/test/enable/disable（--all 并发实测）
- [x] pony token create/list/revoke（明文仅创建时打印一次）
- [x] pony usage / doctor / config export（7 服务模板）
- [x] ~/.pony/config.toml（0600）+ 退出码契约 0/1/2/3；cargo install 待发布仓库后补
**验收**：纯 CLI 完成 Gemini 添加 → 生成 token → 导出配置 → doctor 通过
（集成脚本 m2_test.sh 隔离环境全绿：12 步 26 断言；workspace 测试 23+55 全绿）

## M3 监控与告警 — ✅ 已完成（2026-08-22）
- [x] CF GraphQL 限额轮询（Workers 10万/天，GraphQL 变量注入面隔离）
- [x] Vercel /v1/usage 轮询（Hobby `plan_upgrade_required` → unsupported_plan 降级，quota=-1/pct=-1 哨兵）
- [x] quota_snapshots 落库 + 80% 越线沿告警（≥95 critical 分档；回落再越线重发）
- [x] webhook 通知（R5 具体类型单实现，非阻塞 spawn，失败仅 warn）
- [x] 管理 API：/api/alerts 真实实现（unread/limit≤500）+ /api/alerts/{id}/read（R4 三态幂等）+ /api/quota（snapshots + sources 健康）
- [x] M1 债务清理：usage_hourly 30 天 / quota_snapshots 90 天保留策略
**验收**：人为调低阈值触发告警；Dashboard 数据源就绪
（集成脚本 m3_test.sh 离线 stub 口径全绿：7 步 23 断言——cf pct≈85/sources 健康口径/告警不重发/read 幂等/401/prod_guard；workspace 测试 80+23 全绿零警告，m1/m2 无回归。生产真实凭据注入属 R2 手动步骤，待用户提供 CF_API_TOKEN/accountTag 后按 systemd EnvironmentFile 步骤启用）

## M4 公网入口（CF Tunnel）— 预计 0.5 天
- [ ] cloudflared 安装 + tunnel 配置（access.ponyjob.top → :8899）
- [ ] 外网手机（热点）实测数据面 + 国内可达性
**验收**：手机 4G 网络下 SDK 经 access.ponyjob.top 调用 zen 成功

## M5 Windows 桌面端（Tauri 2）— 预计 3-4 天
- [ ] 脚手架：Tauri 2 + React + shadcn/ui，管理 API client
- [ ] Dashboard / Routes / Tokens / Usage / Settings 五页
- [ ] 用量图表（recharts）+ 限额进度条
- [ ] 系统通知（告警轮询）
- [ ] NSIS 安装包
**验收**：Windows 上安装 → 连接 dev 服务器 → 完成路由/token 管理 → 收到告警通知

## M6 打磨与模板库 — 预计 1-2 天
- [ ] 服务模板库（Gemini/OpenRouter/Groq/Mistral/xAI 一键导入）
- [ ] config export 全服务覆盖
- [ ] README + 部署文档 + 截图
- [ ] 全链路回归 + doctor 套件
**验收**：新用户从零到可用 < 10 分钟（按文档）

## 总计：约 8-9 个工作日

## 排序依据
后端(M1)是唯一状态源必须先行 → CLI(M2)验证管理 API 完备性 → 监控(M3)为 GUI 提供数据 → Tunnel(M4)解锁移动场景 → GUI(M5)承载全部能力 → 模板(M6)降低使用门槛。
