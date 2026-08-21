# M1 Specs 对抗审核记录

> 审核日期: 2026-08-21 | 审核者: code-reviewer（架构/质量）∥ security-auditor（安全/对抗），独立并行
> 结论: 两路均为「有条件通过」→ 按下表裁决修订后进入实现

## 汇总

| 来源 | P0 | P1 | P2 |
|------|----|----|----|
| code-reviewer | 2 | 12 | 14 |
| security-auditor | 2 | 4 | 10 |

两路 P0 高度重合（CONNECT 绕过鉴权、迁移后上游凭据丢失），交叉印证成立。

## 裁决（P0）

| # | 意见 | 裁决 | 采纳 |
|---|------|------|------|
| S-P0-1 | CONNECT 完全绕过鉴权 = 免认证任意 TCP 跳板（内网 SSRF/端口扫描） | **采纳**。M1 直接禁用 CONNECT（返回 403），理由：① 池已停用，CONNECT 仅剩直连兜底，而直连兜底正是攻击面；② 上游 `?url=` 模式本无法承载 CONNECT，CURRENT.md 确认无消费方；③ M4 公网化后此洞为致命。删除 T3 中"保留 CONNECT relay"设计，Pool/relay 模块不再被 server 引用。T8 增加断言：CONNECT → 非 200 | ✅ |
| S-P0-2 / C-P0-1 | 迁移删除 worker_url/worker_secret/upstreams 且不落库 → 重启即全路由 503 | **采纳（方案 a）**。config.json 重写仅删除 `routes` 键，保留 worker_url/worker_secret/upstreams/route_upstreams（它们就是基础设施项，与 listen 同类，且含密钥本就不应入 git——见 S-P1-2 联动处理）。T8 增加步骤 11：同一 DB + 已重写 config 重启后转发仍通 | ✅ |
| C-P0-2 | `update_route` 的 `Option<String>` 无法区分"字段缺席"与"null 清除"，T1/T6 契约冲突 | **采纳**。改为 `Option<Option<String>>` + serde `double_option`：缺席=不改，null=清除，Some=设置。T1/T4/T6 三处同步 | ✅ |

## 裁决（P1）

| # | 意见 | 裁决 | 采纳 |
|---|------|------|------|
| S-P1-1 / C-P1-1 | `X-Pony-Token` 会透传到上游（token 明文泄露给第三方 API） | **采纳**。转发前从 header map 删除 `x-pony-token`（两种鉴权模式都删）；T3/T8 加断言 | ✅ |
| S-P1-2 | 真实 worker_secret 随 config.json 入 git | **采纳**。`.gitignore` 加 `config.json`，入库 `config.example.json`（secret 占位符）。T1 执行 | ✅ |
| S-P1-3 | admin token 明文入持久 journald 且无轮换 | **采纳**。支持 `PPROXY_ADMIN_TOKEN` 环境变量注入（跳过生成）；首启打印仍保留（引导必需，单用户场景可接受），归档文档写明 journal 清理命令 | ✅ |
| S-P1-4 / C-P1-2 | "pony_ 前缀被正则天然排除"是错误断言 | **采纳**。T4 §6 显式增加 `name.starts_with("pony_") → InvalidName`；修正 T3 措辞 | ✅ |
| C-P1-3 | `insert_token(is_admin)` 参数无处落地 | **采纳**。删除参数，admin 语义仅由 `name=="__admin__"` 约定 | ✅ |
| C-P1-4 | `revoke_token -> bool` 无法区分 404/200 语义 | **采纳**。返回 `enum RevokeOutcome { Revoked, AlreadyRevoked, NotFound }`，T1/T2/T6 同步 | ✅ |
| C-P1-5 | RouteTable 缺 edges 注入 / `effective_upstream` 不存在 / `pick_upstream` 私有 | **采纳**。`new(store, edges)`；`pick_upstream` 改 pub；新增 `pub effective_upstream`；上游未配置时 test_route 返回 `Ok(ok:false)` | ✅ |
| C-P1-6 | flush 失败合并回 live 的 `and_modify` 丢数据（entry 必 vacant） | **采纳**。改 `and_modify(...).or_insert_with(...)`；测试断言"flush 失败后 live 非空" | ✅ |
| C-P1-7 | 400 错误 body 契约自相矛盾（§3.2 vs §4） | **采纳（以 §4 为准）**。§3.2 改为"固定文案，禁止透传 Display" | ✅ |
| C-P1-8 | T6 偏离上游 M1 spec（token 列表不返回前 8 位）未声明 | **采纳**。T6 加显式偏离声明，归档时同步修正上游 M1 spec §3.5 | ✅ |
| C-P1-9 | Store 签名同步/异步三处矛盾 | **采纳（同步签名）**。Store 全部同步 `pub fn`，调用方（T2 touch 路径、T5 flush）自行 `spawn_blocking`；修正 README §3.4 | ✅ |
| C-P1-10 | token name 唯一性无落地，Duplicate 无产生路径 | **采纳**。部分唯一索引 `UNIQUE(name) WHERE revoked_at IS NULL`（撤销后可复用名）；T1/T2 测试同步 | ✅ |
| C-P1-11 | health 需 `SELECT 1` 但 Store 无 ping 方法 | **采纳**。T1 增加 `pub fn ping()` | ✅ |
| C-P1-12 | T8 步骤 5 zen 无具体 curl 命令 | **采纳（已实测）**。tech-lead 于 2026-08-21 经生产网关实测：路径为 `/zen/v1/chat/completions`，POST 返回 401 `CreditsError`（当前 key 余额不足但链路通）。T8 写死该命令，断言放宽为"body 含 CreditsError 或 DataPolicyError（均为上游业务错误=链路通）" | ✅ |

## 裁决（P2，择优采纳）

| # | 意见 | 裁决 |
|---|------|------|
| S-P2-1 | verify 隔离 admin 行（admin token 不得作数据 token） | 采纳 |
| S-P2-2 | DB 文件权限 0700/600 | 采纳 |
| S-P2-3 | token name 字符集白名单 `^[a-zA-Z0-9._-]{1,64}$` | 采纳 |
| S-P2-4 | `expires_days` 限 1..=3650 + checked_add | 采纳（与 C-P2-8 同） |
| S-P2-5 | revoke 后 reload 失败 → fail-closed 返回 Err | 采纳 |
| S-P2-6 / C-P2-6 | test_route 用独立 10s timeout client | 采纳 |
| S-P2-7 | PATCH override_upstream 校验 worker/vercel | 采纳 |
| S-P2-8 | T8 临时目录 trap 清理 | 采纳 |
| S-P2-9 | 删除 `(\*\.)?` 通配前缀（与 C-P2-1 同） | 采纳 |
| S-P2-10 | 错误日志禁记完整 URL（query 可能含上游 API key）；SQL 禁 format! 拼接写入 T1 | 采纳 |
| S-P2-额外 | 管理面绑定非回环时启动 warn | 采纳 |
| C-P2-2 | test_route 注释残缺重写 | 采纳 |
| C-P2-3 | 删除 UsageTrackerHandle 死代码声明 | 采纳 |
| C-P2-4 | `Store::open` 的 bool 返回值：保留，用于首启日志 "created new db" | 采纳（注明用途） |
| C-P2-5 | record_request 时序改为 body 读取完成后 | 采纳 |
| C-P2-7 | sqlite3 CLI 改 python3 sqlite3 模块 | 采纳 |
| C-P2-9 | PATCH 移除 `upstream` 字段（展示列不可改） | 采纳 |
| C-P2-10 | /api/usage 响应用独立 DTO，不含 ts_hour=0 哨兵 | 采纳 |
| C-P2-11 | query_usage 补"含 since_hour 当小时"注释 | 采纳 |
| C-P2-12 | README 删去 0.0.0.0:8899 | 采纳 |
| C-P2-13 | 节流窗口引用常量 | 采纳 |
| C-P2-14 | T3 指定唯一方案：accept 循环 + peek 分流 + http1::Builder，删除 axum::serve 分支 | 采纳 |
| C-P2-额外 | usage_hourly 保留策略记录为已知债务（M3 处理） | 采纳（写入 README 已知债务节） |
| C-P2-额外 | T8 定义离线可过门禁子集（步骤 0-3、6-10） | 采纳 |
| S-P1-额外 | M1 加全局并发连接上限（简单 Semaphore，如 256） | 采纳（轻量实现，非完整限速框架） |
| S-P2-额外 | verify/revoke 竞态窗口一致性级别注释 | 采纳 |
| S-风险 | 管理操作审计日志 | 驳回（M1 范围外，M3 告警体系一并考虑） |
| C-风险 | CONNECT 冒烟测试 | 随 P0-1 裁决改为"CONNECT → 403"断言 |
| C-风险 | T8 门禁依赖外部网络 | 已由离线子集裁决覆盖 |
| C-风险 | 迁移中断态测试（AlreadyMigrated + config 未重写） | 采纳（T1 §8 补一项） |

## 修订执行

修订由 planner 按上表更新 T1-T8 specs 与 README，修订要点：
1. T1：config.json 仅删 routes 键；.gitignore + config.example.json；部分唯一索引；ping()；同步签名；DB 权限；SQL 参数化纪律；迁移中断态测试
2. T2：删除 is_admin 参数；RevokeOutcome；name 白名单；expires 校验；verify 隔离 admin；PPROXY_ADMIN_TOKEN 注入；节流常量化；竞态窗口注释；fail-closed reload
3. T3：CONNECT → 403；x-pony-token 转发前删除；pony_ 前缀措辞修正；唯一实现路径（http1::Builder）；record_request 时序；并发上限 Semaphore；错误日志禁整 URL
4. T4：pony_ 前缀显式校验；RouteTable::new(store, edges)；pick_upstream pub；effective_upstream；删除通配前缀；test_route 注释重写
5. T5：or_insert_with 修复；删除死代码声明
6. T6：double_option PATCH；错误文案统一 §4；偏离声明；PATCH 移除 upstream 字段；usage 独立 DTO；admin 撤销保护已有
7. T8：CONNECT→403 断言；重启持久化步骤 11；x-pony-token 泄露断言；zen 命令写死（/zen/v1/chat/completions，CreditsError|DataPolicyError）；python3 查 DB；trap 清理；离线门禁子集
8. README：同步签名修正；0.0.0.0 删除；已知债务节；并发上限约定

## 实现期裁决（F 系列，2026-08-21）

实现完成后由 code-reviewer ∥ security-auditor 对代码二次对抗审核，修复记录于 git 提交 `233b7b0`（F1/F3/F4/F5/F7）与 `bbf86cc`（F8）。

| # | 发现 | 修复 |
|---|------|------|
| F1 | wrangler.toml workers_dev 开启 + PROXY_SECRET 明文入 vars | 关闭 workers_dev；secret 改 wrangler secret |
| F3 | 数据面 serve_connection 无 header 读超时（慢连接占满 Semaphore） | hyper-util auto::Builder + header_read_timeout 30s + TokioTimer |
| F4 | 迁移导入未过 name/host 校验，脏数据直接入库 | 逐条 validate_name/validate_host，非法项跳过不阻断 |
| F5 | CONNECT 403 响应 content-length 硬编码 | 由 body 实际长度计算 |
| F7 | main.rs 空 if 死代码块 | 删除 |
| **F8** | **routes 表双列语义断裂**：T1 §6.1.6 原 spec 让迁移把 route_upstreams 绑定写入 `upstream` 列（创建时快照列，仅展示用途），而 resolve/pick_upstream 实际读取 `override_upstream` 列 → 迁移后绑定全部失效（openai/zen 落 Worker 而非 Vercel） | ① Upstream 枚举扩展 `Named(String)` 变体承载任意已配置上游名；② 迁移改写 override_upstream 列（upstream 列留 NULL），绑定值经格式白名单校验；③ parse_upstream 接受任意非空值归一为枚举，pick_upstream 加空串守卫回退 host 规则；④ create/update 校验统一 valid_override——"worker"\|"vercel" 恒合法（worker 由 config.worker_url 提供不经 upstreams map）、空串拒绝、其余须在 edges 表键中。T1 §6.1.6 / T4 §2-§8 已同步修订 |

## 验收结论

- 单测：`cargo test --workspace --tests` 55 passed / 1 ignored（网络用例）
- 集成：`M1_TEST_OFFLINE=1 bash scripts/m1_test.sh` 52 项断言全绿退出 0（步骤 4/5 在线用例因本机外网不可达按 T8 §1 离线子集口径跳过）
- 生产服务 systemd pproxy 全程 active（脚本首尾断言）
