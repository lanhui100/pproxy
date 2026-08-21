# M1 Spec: 后端强化（鉴权 + 管理面 + 存储）

> 状态: 已实现（2026-08-21，任务级 specs 见 [m1/](m1/README.md)） | 优先级: P0 | 预计: 1.5 天 | 依赖: 无

## 1. 背景与目标

当前 pproxy-server 数据面无鉴权（仅绑 127.0.0.1）、路由硬编码于 config.json、无用量观测。M1 将其升级为 Pony Proxy 的后端基座：

1. 多设备 token 鉴权（数据面 + 管理面分离）
2. 路由白名单动态管理（SQLite，热生效）
3. per-route/per-token 用量计数与查询
4. 管理面 REST API（CLI/GUI 的唯一入口）

## 2. 范围

**做**：token 模型、路径鉴权、SQLite 存储、config.json 迁移导入、路由 CRUD、自动上游选择 + override、用量计数（内存实时 + 每小时落库）、管理 API、admin token 引导、7 路由迁移回归。

**不做**（后续里程碑）：CF/Vercel 限额轮询与告警（M3）、CLI（M2）、CF Tunnel（M4）、限速（P1，表结构预留）。

## 3. 详细设计

### 3.1 Token 模型

```
格式:    pony_<32hex>（16 字节随机）
存储:    SHA-256(token) 落库，明文仅创建响应返回一次
字段:    id, name, token_hash UNIQUE, created_at, expires_at?, revoked_at?, last_used_at
admin:   pony_admin_<48hex>，首个 admin 首次启动生成 → 打印日志 + 落库（name="__admin__"）
```

### 3.2 数据面鉴权

```
主:  GET /{token}/{route}/{path}?{query}
辅:  Header X-Pony-Token: pony_...（路径不含 token 段时）
失败: 401 {"error":"unauthorized"}
```
- token → 内存缓存（HashMap<hash, TokenRow>，启动全量加载 + 管理面变更时刷新）
- 校验：哈希比对 + 未撤销 + 未过期；命中更新 last_used_at（节流 60s 写库）

### 3.3 路由引擎

- 路由表迁入 SQLite `routes` 表；启动加载 + 管理面变更热生效
- 自动上游选择：`api.openai.com | opencode.ai → vercel`，其余 → `worker`；`override_upstream` 可覆盖
- config.json 迁移：首次启动检测旧格式 → 导入 routes 表 → config.json 仅保留 listen/bind/log 级别

### 3.4 用量计数

- 内存：`DashMap<(route, token_id), (AtomicU64 req, bytes_in, bytes_out)>`
- bytes_in = 请求 body 长度；bytes_out = 响应流式累计
- 每小时 tokio interval 聚合落库 `usage_hourly`（UPSERT），落库后清零
- 查询 API 聚合内存 + 库中数据

### 3.5 管理面 API（:8900，Bearer admin_token）

| 端点 | 方法 | 说明 |
|------|------|------|
| /api/tokens | POST | `{name, expires_days?}` → 明文 token（仅此一次） |
| /api/tokens | GET | 列表（脱敏：不返回 token_hash/明文前缀——C-P1-8 偏离修订：仅 name/状态/过期，防前缀穷举） |
| /api/tokens/{id} | DELETE | 撤销（软删 revoked_at） |
| /api/routes | GET / POST | 列表 / 新增 `{name, target_host, override_upstream?}` |
| /api/routes/{name} | PATCH / DELETE | 改 upstream/enabled / 删除 |
| /api/routes/{name}/test | POST | 连通性实测（经上游请求 target 首页，返回状态码/耗时） |
| /api/usage | GET | `?hours=24&route=&token_id=` 聚合报表 |
| /api/health | GET | 服务/上游可达性摘要 |
| /api/alerts | GET | 预留（M3 写入） |

### 3.6 SQLite（rusqlite bundled + WAL）

路径：`~/.pony/state.db`
表：tokens / routes / usage_hourly / quota_snapshots(预留) / alerts(预留)
访问：`Mutex<Connection>`（个人低并发足够），写操作 spawn_blocking。

## 4. 任务拆解

| # | 任务 | 产出 | 依赖 |
|---|------|------|------|
| T1 | 存储层 | store.rs：schema 初始化、迁移导入 config.json | - |
| T2 | token 模块 | token.rs：生成/哈希/校验/CRUD + 内存缓存 | T1 |
| T3 | 数据面鉴权 | main.rs 路径解析改造 + 401 | T2 |
| T4 | 路由引擎 | route.rs：动态加载、自动上游、热生效 | T1 |
| T5 | 用量计数 | usage.rs：计数器 + 小时落库 + 查询聚合 | T1 |
| T6 | 管理 API | api.rs：3.5 全部端点（admin 中间件） | T2 T4 T5 |
| T7 | admin 引导 | 首启生成 + 日志打印 | T2 |
| T8 | 回归验证 | 7 路由迁移 + 集成测试脚本 | T3 T4 T6 |

## 5. 测试与验收

**单元测试**：token 生成唯一性/哈希校验/过期撤销逻辑；上游自动选择；路径解析（token 段/无 token/header 模式）。

**集成测试**（脚本 `scripts/m1_test.sh`）：
1. 启动 server → 日志出现 admin token
2. 无 token 请求 `/anthropic/v1/messages` → 401
3. 管理 API 创建 token → 明文返回一次
4. 带 token 请求 openai（假 key）→ 401 invalid_request_error（链路通）
5. 带 token 请求 zen → DataPolicyError（链路通）
6. `/api/usage` 显示上述请求计数
7. 管理 API 新增路由 → 立即生效；删除 → 404
8. 撤销 token → 请求 401

**验收标准**（ROADMAP M1）：
- curl 带 token 走通 openai/zen
- 无 token 401
- /api/routes 增删即时生效
- 旧 config.json 7 路由自动迁移，行为与迁移前一致

## 6. 风险与回滚

| 风险 | 缓解 |
|------|------|
| 路径 token 为 breaking 变更（旧 base_url 失效） | 单用户可控；迁移说明写 README；config.json 原样保留 |
| SQLite 并发写锁 | WAL + Mutex + 低频写（小时聚合）；个人场景足够 |
| token 泄露 | 可撤销 + 过期时间 + 哈希存储（库泄露不暴露明文） |
| 迁移逻辑缺陷 | 首次导入前备份 config.json → config.json.bak；导入幂等 |

**回滚**：git tag `pre-m1`；`git checkout pre-m1 && cargo build --release && systemctl restart pproxy` 即回旧版（SQLite 文件不影响旧版运行）。

## 7. 审查与门禁

- 三路审查：实现后自审 → 独立 reviewer（安全视角：token 处理/注入/信息泄露）→ 第二审查（架构/测试覆盖）
- 安全审查必做（token/鉴权/外部输入进 URL 拼装——SSRF 检查：target_host 白名单校验，禁止内网地址）
- 测试门禁：单元 + 集成全绿方可归档

## 8. 归档

完成后：spec 状态改"已实现"→ ROADMAP 勾选 M1 → 更新 docs/ops/API.md（管理面协议）与 README。
