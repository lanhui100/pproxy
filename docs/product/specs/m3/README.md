# M3 任务级 Specs — 监控与告警

> 上游: [ROADMAP M3](../../ROADMAP.md) + [TECH_DESIGN §2.1](../../../TECH_DESIGN.md) | 状态: 已实现归档（2026-08-22，R1-R5 全部落地；离线 stub 口径验收达标，生产真实凭据注入待 R2 手动步骤） | 日期: 2026-08-21
>
> 审核裁决摘要：R1 测试注入补 dummy 凭据（无 PPROXY_CF_API_TOKEN 等三变量则来源 disabled，stub 永不被触达）；R2 手动验收补 systemd unit `EnvironmentFile=` 修改步骤（现状 unit 无 EnvironmentFile，restart 不会让新变量到达进程）；R3 quota=-1 时 pct 落 -1、判定跳过 pct<0（DDL pct NOT NULL 必须有值）；R4 mark_read 复用三态幂等 outcome（对齐 AlreadyRevoked 先例：已读重复标记 200）；R5 Channel 分发用具体类型（async fn in trait 非 dyn-compatible，不引 async-trait，P1 多渠道再评估）。

## 0. 现状探测结论（2026-08-21 实测，spec 的前提）

| 项 | 实测 | 结论 |
|----|------|------|
| Vercel `/v1/usage`（team_4TaI…，Hobby plan） | `{"error":{"code":"plan_upgrade_required"}}` | **官方用量 API 被 Pro 计划门控**，Hobby 不可用 |
| VERCEL_TOKEN（.secrets.env） | v9/projects 正常列出 pproxy-edge | token 有效，仅 usage 端点被门控 |
| `api.cloudflare.com` 外网可达性 | 403 Missing Authorization（网络通） | 可达；**缺 CF_API_TOKEN 与 accountTag**（.secrets.env 无 CF 凭据） |
| dev 服务器出网 | api.vercel.com / api.cloudflare.com 均可达 | 在线轮询可行，凭据是唯一缺口 |

## 1. 目标与范围

服务端监控轮询器（tokio interval，1h）：上游限额采集 → `quota_snapshots` 落库 → 阈值越线生成 `alerts` → webhook 通知。管理面补齐 `/api/alerts` 真实实现与 `/api/quota` 快照查询，作为 M5 Dashboard 数据源。顺带清 M1 债务中的保留策略项（usage_hourly 30 天 / quota_snapshots 90 天清理）。

不在范围内：审计日志（M1 债务登记"M3 一并考虑"——**驳回**：当前无管理操作消费方，等 M5 GUI 有真实写操作面时一并设计）；per-token 限速（P1）；CLI 告警子命令（Dashboard 直接消费 API，CLI 不加层）。

验收场景（ROADMAP 原文）：人为调低阈值触发告警；Dashboard 数据源就绪（`/api/quota` + `/api/alerts` 非空且结构稳定）。

## 2. 架构（高内聚低耦合）

```
crates/core/src/
  quota.rs        # QuotaSource trait + CLOUDFLARE/Vercel 两个实现 + 解析纯函数
  alert.rs        # AlertEvaluator（越线判定，纯逻辑）+ Channel trait + WebhookChannel
crates/server/src/
  monitor.rs      # 轮询任务装配：tick → 采集 → 落库 → 评估 → 通知；来源健康状态
```

- core 不依赖 server；quota/alert 只暴露纯函数与 trait，HTTP 客户端经构造注入（复用 reqwest）。
- 落库方法全部在 store 层新增（见 §5），monitor.rs 只编排不写 SQL。
- 轮询任务生命周期仿 main.rs 第 6 步 usage flush task：`tokio::spawn` + interval，panic 不拖垮主服务（join_error 忽略重建）。

## 3. 配置与凭据

**全部走环境变量，config.json 零改动**（凭据不入盘原则，与 `.secrets.env` 运维模式一致）：

| 变量 | 含义 | 缺省行为 |
|------|------|---------|
| `PPROXY_CF_API_TOKEN` | CF API Token（需 Account.Analytics:Read） | 缺失 → CF 来源 disabled（启动 info 一行，非错误） |
| `PPROXY_CF_ACCOUNT_TAG` | CF account id | 同上 |
| `PPROXY_VERCEL_TOKEN` | Vercel token | 缺失 → Vercel 来源 disabled |
| `PPROXY_VERCEL_TEAM_ID` | team scope 必带 | 缺失 → 单用户口径尝试 |
| `PPROXY_ALERT_WEBHOOK_URL` | 告警 webhook POST 地址 | 缺失 → 不发外部通知（仅落库） |
| `PPROXY_ALERT_THRESHOLD_PCT` | 告警阈值百分比 | 默认 80 |
| `PPROXY_POLL_INTERVAL_SEC` | 轮询间隔秒 | 默认 3600（测试注入小值） |
| `PPROXY_CF_GRAPHQL_URL` | **仅测试**：CF GraphQL 端点覆盖 | `https://api.cloudflare.com/client/v4/graphql` |
| `PPROXY_VERCEL_API_BASE` | **仅测试**：Vercel API base 覆盖 | `https://api.vercel.com` |

## 4. 采集规格

### 4.1 CF Workers（免费额度 100,000 请求/日）
GraphQL（workersInvocationsAdaptive，account 维度，当日 UTC 窗口）：

```graphql
query($accountTag: String!, $since: Date!, $until: Date!) {
  viewer { accounts(filter: {accountTag: $accountTag}) {
    workersInvocationsAdaptive(
      filter: { date_geq: $since, date_leq: $until }, limit: 10000,
      orderBy: [date_ASC]) {
      sum { requests }
      dimensions { date }
    } } }
}
```

- used = 当日各行 sum(requests) 求和；quota = 100000（常量 `CF_DAILY_REQUEST_QUOTA`）；metric = `requests_daily`
- Bearer `PPROXY_CF_API_TOKEN`；响应 `errors` 数组非空 → 该次采集失败（warn，不影响另一来源）
- 10s 超时独立 client；失败不重试（下个 tick 自然重试）

### 4.2 Vercel（Hobby 降级口径）
`GET /v1/usage?from=<ISO>&to=<ISO>[&teamId=]`：
- **实测 Hobby 返回 `plan_upgrade_required`** → 来源标记 `unsupported_plan`，启动后首个 tick warn 一次，此后静默 skip（不再刷日志）。这是**能力探测降级**，不是错误路径；未来升级 Pro 后自动恢复工作，代码零改动。
- 可用时取 bandwidth / function invocations 两指标（字段名以实际响应为准，解析函数单测钉住）；Hobby 无官方上限常量可依 → `quota=-1` 表示"未知上限"，`pct=-1` 同为哨兵（DDL REAL NOT NULL 必须有值），告警判定跳过 pct<0 的快照（仅记录不评估）。

## 5. 数据模型（表已建，本里程碑只加方法）

store 层新增：

```
upsert_quota_snapshot(ts, upstream, metric, used, quota, pct)   // INSERT OR REPLACE
latest_quota_snapshots() -> Vec<Row>                            // 每 (upstream,metric) 取最新一条
insert_alert(ts, level, message) -> i64
list_alerts(unread_only: bool, limit: u32) -> Vec<Row>
mark_alert_read(id) -> MarkReadOutcome                          // 三态幂等：Marked/AlreadyRead/NotFound（对齐 RevokeOutcome 先例）
prune_usage_before(cutoff_hour) -> usize                        // M1 债务：保留 30 天
prune_quota_before(cutoff_ts) -> usize                          // 保留 90 天
```

- `quota_snapshots.ts` = 采样时刻的 UTC 小时地板（hour_floor 复用），PK `(ts, upstream, metric)` 天然支持每小时一行进度序列。
- `alerts.level` ∈ {warning, critical}：pct ≥ threshold → warning；pct ≥ 95 → critical。

## 6. 告警判定（alert.rs，纯函数可单测）

输入：本次快照 pct、上次快照 pct（内存 HashMap<(upstream,metric), f64>）、阈值。
- **越线沿触发**：last < T 且 now ≥ T → 告警一次；持续超阈值不重复发。回落再越线 → 再次告警。
- 重启丢失内存态 → 可能重复告警一次，可接受（登记为已知行为）。
- message 人话格式：`vercel bandwidth at 83.2% (used/limit)`；不含凭据。
- 触发即 `insert_alert` 落库 + 逐渠道投递；webhook 投递失败仅 warn（告警已在库，不丢不重试）。

## 7. Webhook 渠道（P1 抽象的最小实现）

```
struct WebhookChannel { url: Option<String>, client: reqwest::Client }
impl WebhookChannel { async fn send(&self, alert: &AlertRecord) -> Result<(), String>; }
```

- **不用 trait 分发**（R5）：`async fn` in trait 非 dyn-compatible，`Vec<Box<dyn Channel>>` 编不过；单实现期直接持有具体类型，P1 真出现第二渠道时再评估 enum 分发或 async-trait。
- POST JSON: `{event:"quota_alert", level, message, ts}`；15s 超时；2xx 即成功；非阻塞 spawn（不拖慢轮询主流程）。

## 8. 管理 API 扩展（API.md 同步更新）

| 端点 | 方法 | 行为 |
|------|------|------|
| `/api/alerts` | GET | **替换现有空实现**：`?unread=1&limit=50`（limit ≤500）→ `{alerts:[{id,ts,level,message,read_at}]}`，倒序 |
| `/api/alerts/{id}/read` | POST | 标记已读 `{read:true}`；重复标记 200（幂等）；404 not_found |
| `/api/quota` | GET | `{snapshots:[{upstream,metric,used,quota,pct,ts}], sources:[{name,state,last_ok}]}` —— snapshots 为每键最新值；sources 反映采集器健康（ok/disabled/error/unsupported_plan） |

鉴权沿用 admin Bearer 中间件（router 挂载位置同现有组）。

## 9. 测试清单

单元测试（`#[cfg(test)]`，纯函数优先）：
1. alert 判定：首次越线触发、持续不重发、回落再触发、critical 分档
2. CF GraphQL 响应解析（正常/errors 数组/空行集）；Vercel usage 解析与 plan_upgrade_required 识别
3. store 方法：upsert/latest/mark_read/prune（tempfile 内存库，仿 store/tests.rs）
4. webhook body 序列化形状

集成验证（scripts/m3_test.sh，全隔离同 M2 模式：临时 HOME/DB/config + 端口 18896/18895/18894 区分既有脚本）：
1. 本地起两个 stub：CF GraphQL stub（返回当日 requests 使 pct=85%）+ webhook 接收 stub（POST 落盘文件）；`PPROXY_CF_GRAPHQL_URL`/`PPROXY_VERCEL_API_BASE`/`PPROXY_POLL_INTERVAL_SEC=2` + **dummy 凭据 `PPROXY_CF_API_TOKEN=test` `PPROXY_CF_ACCOUNT_TAG=test` `PPROXY_VERCEL_TOKEN=test`**（R1：缺凭据则来源 disabled，stub 永不被触达）注入
2. server 启动 → 轮询 2 个周期后：`GET /api/quota` 含 cf 条目 pct≈85、sources.cf=ok；Vercel stub 返回 plan_upgrade_required → sources.vercel=unsupported_plan（降级口径断言）
3. `GET /api/alerts` 非空且含 warning 级条目（阈值默认 80 < 85）
4. webhook stub 收到 POST（文件存在且含 `"event":"quota_alert"`）
5. 持续轮询不重复告警（alerts 条数稳定为 1）
6. `POST /api/alerts/{id}/read` → read_at 非空；unread=1 过滤生效
7. 鉴权：无 token 访问 /api/quota → 401
8. prod_guard 断言生产 pproxy active（首尾）

## 10. 验收标准

- `cargo test --workspace` 全绿；`M1_TEST_OFFLINE=1 bash scripts/m1_test.sh` 与 `bash scripts/m2_test.sh` 无回归
- `bash scripts/m3_test.sh` 全绿退出 0（离线 stub 口径，无需真实 CF/Vercel 凭据）
- 手动验收：生产环境注入真实 CF 凭据——`/etc/systemd/system/pproxy.service` 追加 `EnvironmentFile=/home/USER/pproxy/.secrets.env`（R2：现状 unit 无 EnvironmentFile，仅 restart 不会让新变量到达进程；.secrets.env 需去掉 export 前缀或改用逐项 Environment=）+ `sudo systemctl daemon-reload && sudo systemctl restart pproxy` → `/api/quota` 出现真实数据；在此之前以 stub 口径验收通过即视为达标
- API.md 更新提交（/api/alerts 移除"预留"标注、新增 /api/quota）
