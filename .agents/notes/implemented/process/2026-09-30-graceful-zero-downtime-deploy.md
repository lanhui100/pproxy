# Agent Note: graceful-zero-downtime-deploy

Status: implemented

## Problem

各组件部署 SOP 零下线能力不齐：Rust 集群已有 `pproxy cluster upgrade` 逐节点滚动
（HA Forwarder 吸收窗口），但 **CF gate/edge Worker 的 SOP 只有 `wrangler deploy`
直接替换**——旧版本即刻下线、无灰度、失败即事故；且"本机 server 单节点重启"
未标注窗口与接替机制，容易误宣称"零下线"。需求：所有组件部署必须明确"零下线
机制 + 窗口内流量由谁接替"，并落成可执行的 SOP。

## Decision

建立**优雅不下线部署总则**并逐组件落实（决策随本次变更落地）：

1. **总则**（docs/ops/DEPLOY.md §0）：任何部署动作不得让服务整体下线；每步先回答
   "窗口内流量谁来接"。四组件零下线机制：
   - CF Worker（gate/edge）：wrangler 4 版本化灰度（见下）；
   - Vercel：平台版本化部署（新 deployment 就绪后自动切流），零窗口；
   - Rust 集群节点：`pproxy cluster upgrade` 滚动升级（已有 ROLLING-UPGRADE.md）；
   - Rust 单机（无 peer）：如实标注"秒级窗口、靠客户端 failover 吸收"，不宣称零下线。
2. **CF Worker 部署 SOP 改写**（DEPLOY.md §更新 CF Worker）：禁止裸 `wrangler deploy`
   直接替换；改为 `wrangler versions upload`（线上不动）→ `versions deploy @5/@50/@100`
   按比例灰度 → 异常 `wrangler rollback <上一版本ID>`。wrangler.toml 同步注释纪律。
3. **边界诚实**：单机无 peer 时服务端无法消除重启窗口（无 HA Forwarder 吸收）；
   真正服务端零下线的前置是配集群 peer 后滚动升级——文档明示，不虚报能力。
4. **验证口径**：灰度窗口至少包含一次真实 WS 隧道会话（仅 curl /debug 不足以证明
   WS 桥正常），并给出离线打包校验 `wrangler versions upload --dry-run`（非零退出）。

## Alternatives considered

- **A. 维持 `wrangler deploy` 直接替换 + 靠客户端 failover 兜底**：落选。直接替换
  无灰度观察期，新版本缺陷即全量事故；客户端 failover 只在两端（CF+Vercel）都配好
  时有效，且掩盖服务端质量信号。版本化灰度成本低（wrangler 4 原生支持），收益是
  "先小流量验证再全量"的安全边际。
- **B. 引入独立灰度平台/流量网关（如 Argo/cloudflared 分流）**：落选。pproxy 已有
  双上游（CF+Vercel）客户端侧 failover，再加一层网关属过度设计；wrangler versions
  的原生百分比切流已满足"优雅不下线"，无需新增组件。
- **C. 单机也宣称"零下线"（systemctl restart 后探活即算）**：落选（负向）。
  无 HA Forwarder 吸收时重启窗口真实存在，宣称零下线是虚假承诺；文档按真实能力
  标注边界，并在需要时指引先配集群。
- **D. 各组件 SOP 分散维护、不立总则**：落选。缺总则则"零下线"无统一验收口径，
  新组件接入时无判断框架；§0 总则表给出四组件机制与窗口接替的单一事实源。

## Consequences

- gate/edge Worker 部署从"直接替换"升级为"灰度-观察-全量-回滚"，服务零下线；
- 部署 SOP 有统一总则与逐组件机制表，新组件接入有据可依；
- 单机节点的窗口边界被诚实标注（不再可能误宣称零下线），需要时先配集群；
- 验证命令可机械执行：`wrangler versions upload --dry-run`（离线非零退出）、
  灰度窗口含一次真实 WS 会话（标注靠 review/运维执行）；
- 部署文档与 wrangler.toml 注释同提交（本决策随 DEPLOY.md/wrangler.toml 落地）。
