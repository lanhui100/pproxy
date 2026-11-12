# Agent Note: 会员开通一条命令闭环（自然月时长 / 订阅深链 / 配额同源落库）与订阅头真实化

Status: implemented

## Problem

运营给单个会员开通账号时，`pproxy user add -q 50G` 只签发一把 Ed25519 令牌，另外三件事全靠手工：

1. **时长不可直述**：`-d` 只吃天数（`u32`）。"1 个月 / 3 个月 / 半年 / 1 年" 必须自己换算成 30/90/180/365，
   而按固定天数算的"1 个月"在 2 月与 8 月长度不同，会员到期日会与实际售卖口径漂移。
2. **拿不到可直接交付的凭据**：命令打印令牌与 Basic 密码，但不打印 Clash 订阅链接。
   服务端 `/clash?token=usr_live_...` 早已实现（验签 + 撤销表 + 按呈现令牌合成 YAML），
   CLI 侧却从未拼过这个 URL，`pproxy clash` 在隧道模式下反而只导出本地文件。
   于是每次开通都要人工拼 `https://<gate域名>/clash?token=<抄来的 token>` —— 抄错一位就是"链接打不开"。
3. **表里没有这个人**：`-q` 分支不写 `users` 表，`user list` / `rm` / `disable` 看不到该会员；
   而写表的旧路径（`UserService::create_user`）又不带额度参数。
   两条路径互斥，导致"额度可管"与"账号可管"不能同时成立，`jti` 与用户名也没有任何落盘关联。
4. **额度不可感知**：网关 `/clash` 的 `subscription-userinfo` 是硬编码
   `upload=0; download=0; total=107374182400; expire=0`，客户端永远显示"100G 总量 / 0 已用"。
   50G 额度用尽后用户只看到连接失败，看不到"额度已用完，次月 1 日重置"，客诉全部落到运营。

## Decision

1. **时长语法 `parse_expiry_spec`（`crates/cli/src/cmd/user.rs`）**：`30`/`30d` = 自然日，`1m`/`3m`/`6m` = 自然月，
   `1y` = 12 个自然月；大小写不敏感、允许单位前后空白；`0` 与非数字直接报错并打印支持列表。
   月运算走 `add_months_utc`（Howard Hinnant 民用历算法手写，不引入 `chrono`，与 `usage.rs::compute_billing_cycle` 同风格），
   **月末溢出钳制**到目标月最后一天（1 月 31 日 + 1m → 2 月 28/29 日），保证"一个月"在任意月份都可成交。
   旗标 `-d/--expires` 从 `u32` 改为字符串，并保留 `visible_alias = "expires-days"`：旧脚本传 `--expires-days 30`
   仍按 30 天解释，不静默失效。
2. **订阅链接随签发一起交付**：`subscription_links(host, token, username)` 产出
   `https://<host>/clash?token=<usr_live_...>` 与 `clash://install-config?url=<RFC3986 百分号编码>&name=PonyProxy-<user>`，
   并用 `clash::render_qr` 打印二维码（载荷是订阅 URL 本身，不是全量 YAML，避免高密度扫不出）。
   网关域名取 `config.tunnel_gate_url`，复用 `clash.rs` 的 fail-fast 口径：非 443 端口不生成链接并说明原因——
   订阅端点只经 nginx 443 暴露，生成 8443 的链接等于发一张打不开的卡。
3. **配额/到期/jti 同源落库（单一真源）**：`-q` 分支改为先 `UserService::create_user_ex` 写 `users`，
   成功后才签发令牌；用户名已存在则直接失败并提示先 `user rm`，避免"表里一套密码 + 手里一套令牌"的凭据漂移。
   `users` 表新增可空列 `quota_bytes` / `jti` / `max_conns` / `role`（`store::USER_MEMBER_COLUMNS` 单一清单，
   `SCHEMA_SQL` 与老库 `PRAGMA table_info` 幂等 `ALTER` 共用），`USER_COLUMNS` 常量统一所有 SELECT 的列序，
   消除"加列漏改某条 SELECT 导致索引错位"的整类缺陷。`None` 语义 = 未开通会员（老 Basic 用户不受影响）。
4. **订阅头返回真实值**：`subscription_userinfo_header(used, quota, expire)` 按令牌 claims 与
   `user_used_bytes` 水位合成，`upload` 恒为 0（现行计量把上下行合并记在 download 侧，不伪造口径）。
   管理员按 Clash 约定 `total=0`（不限额展示）；单口令兼容模式展示 VPS 整机基线与该账本周期聚合用量。
5. **不引入新依赖、不改协议**：仍是标准 HTTP CONNECT + Clash YAML，手机端无需换客户端。

## Alternatives considered

- **给 `-d` 加一套预设枚举（`--monthly`/`--quarterly`/…）**：把"数量 + 单位"拆成两个旗标组合爆炸，
  且无法表达"3 天试用"；字符串语法一个解析器覆盖全部，且能与 `-q 50G` 的既有风格对齐。
- **月数按 30 天硬乘**：实现最省，但 1 月 31 日开通的"1 个月"会员会在 3 月 2 日过期，
  与用户理解的"下月同日"不符；改为自然月 + 月末钳制。
- **放宽 `tunnel_gate_url` 端口校验、任何端口都生成链接**：省一次报错，但 8443 只服务 CONNECT 数据面，
  订阅端点不在其上，用户拿到的是 100% 打不开的链接；宁可在签发时报错并给出原因。
- **`-q` 只签令牌、把落库放到后续 `user sync` 命令**：签发与落库分两步，中间失败即产生"令牌有效但表里无此人"
  的幽灵账号，且额度无法被 `user list` 审计；改为同一次调用内先落库后签发（失败即整体不成立）。
- **给 `users` 建独立 `memberships` 表**：更"规范化"，但本项目的会员属性就是"用户名 + 额度 + 到期 + jti"一维，
  新增表要引入外键与联表，收益仅是形式；`users` 加可空列在老库迁移上是幂等 `ALTER`，成本更低。
- **订阅头继续返回整机 500GB 基线**：客户端会显示"已用 0 / 500G"，与 50G 熔断阈值不一致，
  用户永远看不到自己额度耗尽；改为按 claims 真实值。
- **把 `subscription-userinfo` 的 upload 拆出真实上行**：需要计量侧改为双向分别记账（口径变更 + 账本格式变更），
  超出本次范围；合并记为 download 不误导总和。

## Consequences

- 一条命令完成"开通会员 + 交付订阅材料"：`pproxy user add alice -q 50G -d 3m` 输出配额、到期时间、
  订阅链接、`clash://` 深链与二维码；`user list` 同时显示 `50.00G` 与该会员身份，`rm`/`disable`/`revoke` 全部对同一身份生效。
- 到期语义从"固定天数"变为"自然月/年 + 月末钳制"；`--expires-days` 作为兼容别名保留，旧脚本不破。
- 令牌 `exp` 与售卖时长同源；跨月续费仍需重签（`set-quota`/`renew` 子命令属未做项，见 backlog B019）。
- 订阅链接内嵌 `usr_live_` 长期令牌：泄露上限 = 该用户当期额度被耗尽 + 出口 IP 被当开放代理，不构成横向越权；
  服务端 `/clash` 的另一条无鉴权路由（`crates/server/src/gateway.rs`，直接回吐磁盘 `~/.pony/clash.yaml` 明文令牌）
  仍待治理，已登记 backlog B016 第 6 项与 B019。
- 机械验收（全部非零退出即失败）：
  `cargo test --offline -p pproxy-core -p pproxy-cli`（新增 `expiry_month_end_clamping` / `expiry_units_and_whitespace` /
  `percent_encode_rfc3986_component` / `subscription_links_shape` / `subscription_host_requires_gate_on_443` /
  `quota_display_supports_legacy_none` 与 store 迁移幂等用例）；
  `cargo test --offline -p pproxy-gate-server`（订阅头断言 `download`/`total`/`expire` 与 claims + 水位一致）。
- 端到端实证：临时 `HOME` 下 `pproxy user add alice -q 50G -d 3m` → 到期 2027-02-12（自 2026-11-12），
  `user list` 显示 `50.00G / user / 2027-02-12`，订阅 URL 与深链含 `https://gate.ponygo.fun/clash?token=...`。
