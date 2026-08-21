# Pony Proxy — 里程碑

> 原则：每个里程碑结束时可验证、可演示；后端先行，客户端跟进。

## M1 后端强化（数据面鉴权 + 管理面 + 存储）— 预计 1.5 天
- [ ] token 模型（哈希存储、路径鉴权 `/{token}/{route}/...`、header 兼容）
- [ ] 管理面 REST API：tokens/routes/usage/health CRUD
- [ ] SQLite 落地（tokens/routes/usage_hourly），config.json 迁移导入
- [ ] per-route/per-token 请求与字节计数（实时内存 + 每小时落库）
- [ ] 路由自动上游选择 + override
- [ ] 现有 7 路由迁移验证（doctor 全绿）
**验收**：curl 带 token 走通 openai/zen；无 token 401；`/api/routes` 可增删路由并即时生效

## M2 CLI — 预计 1 天
- [ ] pony status / start / stop / restart
- [ ] pony route list/add/rm/test
- [ ] pony token create/list/revoke
- [ ] pony usage / doctor / config export
- [ ] cargo install 分发 + ~/.pony/config.toml
**验收**：纯 CLI 完成添加新服务（如 Gemini）→ 生成 token → 导出配置 → doctor 通过

## M3 监控与告警 — 预计 1 天
- [ ] CF GraphQL 限额轮询（Workers 10万/天）
- [ ] Vercel /v1/usage 轮询（带宽/函数时长）
- [ ] quota_snapshots 落库 + 80% 告警生成
- [ ] webhook 通知（P1 渠道抽象）
**验收**：人为调低阈值触发告警；Dashboard 数据源就绪

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
