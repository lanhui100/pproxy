# Agent Note: gate-cf-request-quota

Status: implemented

## Problem

gate 隧道每 WS 会话/每 TCP 连接都计 CF 请求；CF 免费档限额是**账号级** 100k 请求/天
（实测 2 小时 YouTube 可烧 43,564 次，edgetunnel issue #1419）。现有用量采集（
`quota.rs` 的 `workersInvocationsAdaptive` 全账号查询）只能看到账号总量，**无法区分
gate 与 edge 各自烧了多少**——gate 请求数隐没在总量里，逼近免费档上限时无法定位
是隧道流量还是 edge 流量所致，也无法单独对 gate 设阈值告警。

## Decision

在既有用量体系上新增 **gate 单独采集来源**（B012 最小可行增量，默认零风险）：

1. **quota.rs**：`CfCollector` 增加可选 `script_name` 过滤（`with_script_name` 构造器，
   现有 `new` 不变）；GraphQL 查询按 `scriptName` 维度区分 Worker，新增
   `cf_graphql_request_body_by_script`（变量化注入面与全账号查询同一纪律）。
2. **monitor.rs**：新增来源 `SOURCE_GATE = "gate_cf"`，metric 沿用 `requests_daily`，
   配额常量沿用账号级 100k/天；采样、落库、越线告警复用既有 tick/`evaluate_and_notify`
   链路——`/api/quota` 自动可见，阈值联动自动生效。
3. **默认关闭（关键）**：未设 `PPROXY_CF_GATE_SCRIPT_NAME` 时 gate 来源
   Disabled，行为与 B012 前完全一致（`new`/全账号查询路径零改动）；配置
   `PPROXY_CF_GATE_SCRIPT_NAME=pony-gate` 才启用独立采样。
4. **探活免计费**（edgetunnel 借鉴项的"本地应答"部分）维持现状：引擎探活经
   `probe_gate_rtt`/`probe_via_gate` 走 WS Ping 或首帧握手后即断，不建立长隧道会话，
   计入 CF 请求的仅为升级握手本身；Clash 探活同理在本地结束，不占用真实出站隧道。

## Alternatives considered

- **A. gate 自身上报请求数（worker 内计数 + 独立端点）**：落选。gate worker 无
  持久状态（isolate 生命周期短、多副本），自计数不可靠；CF GraphQL 的 scriptName
  维度是平台权威口径，零额外组件。
- **B. 把 gate 单独来源设默认开启**：落选（负向）。新增采集默认开启会改变现有
  用户（未配 GATE script 名）的 /api/quota sources 形态与告警行为，违背"默认零
  配置行为不变"纪律；显式配置才启用，明确可控。
- **C. 仅加展示不加告警联动**：落选。gate 请求数逼近免费档上限才是 B012 动机，
  只展示不告警等于继续"爆了才知道"；复用既有越线沿判定成本极低（同一 tick 链路）。
- **D. 探活改为 worker 侧本地应答（edgetunnel 反代模式测速）**：落选 v1。worker
  本地应答需引入专用探测协议与 gate-policy 交互，改动面大；现状探活只消耗升级
  握手（1 请求/次，低频），先以"独立观测 + 阈值告警"解决额度认知问题，本地应答
  留作后续增强（backlog 备注）。

## Consequences

- /api/quota 在配置 `PPROXY_CF_GATE_SCRIPT_NAME` 后出现 `gate_cf` 来源，gate 请求数
  独立可见并随隧道流量增长；越线触发告警（与 cf/vercel 同阈值口径）；
- 默认未配置时行为与 B012 前完全一致（sources 仅 cf/vercel，零新增请求）；
- 配额口径：gate 与 edge 共享账号级 100k/天（CF 免费档按账号计），gate 来源的
  告警用于提前感知隧道流量占比，不能把两个来源的配额相加；
- 机械验收：`cargo test -p pproxy-core quota`（含 by_script 用例）与
  `cargo test -p pproxy-server`（含 gate_source_disabled_by_default）非零退出；
  生产可见性靠配置后观察 /api/quota（标注靠 review）。
