# CF 平台风控 SOP（gate/edge Worker 生存手册）

> 研究背景：edgetunnel 社区实证 CF 正重点风控"Worker 代理"（封号/滥用报告/1011）。
> pproxy 的 gate/edge Worker 长期跑在 CF 免费档，必须按本文纪律运营。
> 配套决策：`.agents/notes/implemented/feature/2026-09-30-gate-multi-egress-fallback.md`
> （P0-1 多出口兜底）与 `.agents/notes/implemented/process/2026-09-30-graceful-zero-downtime-deploy.md`。
> 属于 B013（部署形态 + 风控 SOP 落点）。本文档为运维 how-to，验收靠 review。

## 0. 一句话结论

**"能用就别动"**：在跑的 Worker 不要频繁重部署、不要开无关端点、不要主动上报
可疑行为。降低风控触发 = 少暴露指纹 + 账号隔离 + 出事有预案。伪装页/反指纹
（B011）与多出口兜底（P0-1）是前置防线，本文讲账号与流程层。

---

## 1. 部署形态评估（Pages vs Workers）

| 维度 | Workers 部署 | Pages 部署 |
|---|---|---|
| 耐封性 | 社区实证易收滥用报告（edgetunnel #1554） | **Pages 明显更耐封**（社区最推荐） |
| 部署方式 | `wrangler versions upload/deploy`（版本化灰度，见 DEPLOY.md） | 上传 zip / Pages+GitHub |
| 成本 | 免费档 100k 请求/天（账号级） | 同 Workers 免费档（同一额度体系） |
| 自定义域 | `routes` custom_domain | Pages custom domain |

**现状**：gate 当前为 Workers（`wrangler.toml` routes custom_domain `gate.ponygo.fun`）。
**迁移触发条件**（满足任一才评估迁移，不主动折腾）：
1. gate 收到 abuse 邮件或账号被标记（见 §3）；
2. 同一账号 Worker 被封（1011 无法恢复）；
3. 计划重建小号并顺带搬 Pages。

**迁移步骤**（触发后执行，零下线）：新建 Pages 项目 → 绑定同名自定义域 → 用
`wrangler pages` 或 zip 上传同一 `worker.js` + gate-policy 等模块 → `/debug` 验证 →
走优雅下线验证清单（DEPLOY.md §更新 CF Worker 的验证口径：含一次真实隧道会话）→
旧 Workers 保留不删（回退用，字段测试确认稳定后再下线）。

---

## 2. 账号隔离（小号部署，防一锅端）

- **主要用途多账号隔离**：同一个 CF 主账号不要同时跑可被识别为代理的 Worker 与
  重要真实业务；代理 Worker 建议单独小号（或至少单独账号 Workerd 配额相互独立）。
- **工作负载分离**：gate（隧道桥）与 edge（数据面）归属不同 Worker 名，便于分别
  观测（B012 gate_cf 来源）与分别处置（封一个不影响另一个，客户端 failover 接管）。
- **凭据隔离**：每个账号独立 CF API Token（最小权限：仅目标 Worker 的编辑权限），
  轮换/吊销互不影响；`.secrets.env` 分账号存，禁共用 token。
- **配额是账号级**：CF 免费档 100k 请求/天按账号计，gate 与 edge 共享；隔离账号
  才真正隔离额度（B012 ADR 口径：同账号内不可相加，跨账号才独立）。

---

## 3. 收到 abuse 邮件怎么办（SOP）

> 触发：CF 发来滥用/ToS 警告邮件，或账号状态页出现警告。
> 原则：**冷静、不狡辩、不硬刚**；按时间线留痕；优先保住 Vercel/VPS 出口不受牵连。

1. **不删证据**：截图邮件全文 + 时间戳 + 关联的 Worker 名/域名，归档到事故记录
   （`docs/ops/` 或私有笔记，不发公开渠道）。
2. **立刻止血**：
   - 是"滥用报告"（某个目标站点的投诉）→ 检查 gate/edge 是否代理了该目标；
     若白名单配置了该 host，临时移除（`PPROXY_TUNNEL_ALLOWLIST` 或路由级）；
   - 是"账号级警告"→ 评估是否保留该账号：账号里没有不可替代资源时，**弃号最干净**
     （删除 Worker / 转移自定义域后弃用），避免连坐其他组件。
3. **48h 处理窗口**：CF 通常给整改期；期限内把"整改动作 + 时间"回复到邮件，
   态度配合但**不承诺"停止代理服务"这种无法兑现的措辞**（模板见下）。
4. **客户端不中断**：gate 被封 → 桌面端 engine failover 到 Vercel/VPS 出口
   （既有 `denied` failover 语义），用户无感；速度损失可接受，等新号迁移。
5. **事后动作**：
   - 按 §4 域名轮换或 §1 Pages 迁移到新小号；
   - 复盘触发因素：是新目标域名？新部署形态？还是探活流量异常？写入本次事故记录。

**回复模板（配合姿态，不承诺具体服务形态）**：
> 已收到通知并完成整改：移除了关联端点、降低外部可达请求、核查访问策略。
> 如有进一步问题请通过工单联系，我方会尽快配合处理。

---

## 4. 域名轮换步骤（Worker 被标记 / 域名被 SNI 阻断时）

> 触发：域名被 GFW 阻断（国内 timeout）、CF 标记、或 abuse 后弃号。
> pproxy 客户端侧 endpoint 可配（`PPROXY_TUNNEL_GATE_URL` / 桌面端授权码），
> 轮换=换端点在客户端重录，分钟级窗口（B015 后续做订阅 HOST 自动对准，先手工）。

```bash
# 1) 新域名绑定（旧域保留勿删，DNS 生效前回退用）
#    dashboard：Worker → Settings → Domains & Routes → Add custom domain
#    或 wrangler.toml routes pattern + wrangler versions deploy（见 DEPLOY.md）

# 2) 等 DNS 生效（dig +short <新域> 返回 CF 任播 IP；约 1-5 分钟）

# 3) 验证新域全链路
curl -s https://<新域>/debug | head -c 120          # 200 且含 fallback 字段
# 真实隧道会话（客户端连一次 wss://<新域>/ws）——仅 /debug 不够

# 4) 客户端切换：桌面端重录授权码 / server 更新 PPROXY_TUNNEL_GATE_URL
# 5) 切换确认 24h 后，旧域可删（绝不刚切就删，留回退）
```

纪律：一次换一个域；换完 24h 观察再用下一个域（避免连续换域触发新风控）。

---

## 5. "能用就别动"纪律（防手痒）

- 生产 Worker 无变更需求时**不重部署、不点 dashboard、不跑无关脚本**；
- 变更统一走优雅不下线 SOP（DEPLOY.md §0/§更新 CF Worker：versions 灰度 + 观察 + 回滚）；
- 探活/测速低频执行（探活只消耗升级握手 1 请求/次，不做高频拨测轰炸）；
- 新端点（如诊断接口）只加在 Bearer 保护下，不开放匿名。

---

## 6. 机器可验清单（非零退出命令）

| 检查 | 命令 |
|---|---|
| gate 存活 + 伪装页生效 | `curl -s -o /dev/null -w '%{http_code}' https://gate.ponygo.fun/` 应 200（欢迎页） |
| /debug 正常 | `curl -s https://gate.ponygo.fun/debug` 含 `fallback` 字段 |
| 新域 DNS 已生效 | `dig +short <新域> \| grep -q '^104\.\|^172\.\|^173\.'`（CF 任播段） |
| 风控迹象日志 | `journalctl -u pproxy -g 'unsupported_colo\|abuse\|1011' -n 20`（人工判读） |

> 本 SOP 文档为运维承诺：机械可查项配了命令；"账号隔离/弃号时机/回复措辞"等
> 判断项标注靠 review/人工。