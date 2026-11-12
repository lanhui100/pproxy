# Agent Note: 会员时长支持秒/分钟/小时（`-d 1min`）

Status: implemented

## Problem

`pproxy user add -d` 只认天（`30`/`30d`）、自然月（`1m`/`3m`/`6m`）与自然年（`1y`）。
最细粒度是 1 天，导致**令牌到期行为无法在联调时被直接验证**：要确认「到期后订阅端点与
8443 隧道都拒绝」，只能签一个 1 天账号然后等 24 小时，或手工改网关时钟。

2026-11-12 实测复现路径：签 `-d 1d` 账号 → 到期校验代码是否真生效只能靠读代码推断，
网关日志里到期与签名错误也共用同一句 `invalid token`，无法从外部确认时刻边界。

还有一个命名陷阱：`m` 已经被「自然月」占用。若把 `m` 解释成分钟，会把生产在用的
`-d 3m`（3 个月）静默改成 3 分钟——这类静默改义是计费系统里最贵的一类 bug。

## Decision

在 `parse_expiry_spec` 中新增三个秒基单位，**不改既有含义**：

| 写法 | 含义 |
|---|---|
| `30`、`30d` | 天（不变） |
| `1m`、`3m`、`6m` | 自然月（不变） |
| `1y` | 12 个自然月（不变） |
| `30s` / `sec` / `secs` / `second(s)` | 秒 |
| `5min` / `mins` / `minute(s)` | 分钟 |
| `2h` / `hr` / `hrs` / `hour(s)` | 小时 |

1. **分钟必须写 `min`**：`m` 恒指自然月，两者不共用。错误提示与 `--help` 文案都显式写出
   这条约束，避免用户按常识写成 `-d 1m` 却拿到 1 个月。
2. **溢出报错不饱和**：抽出 `add_secs(now, n, unit_secs, hint)`，乘法或加法任一溢出即 `Err`
   （`30s` 走 `checked_mul(1)`，`u64::MAX` 秒级输入必须被拒而不是钳制）。
3. **单位表集中为常量 `UNITS`**：空输入、非法单位、非数字前缀三条错误信息共用同一份支持列表，
   避免文案与实现漂移。
4. **网关零改动**：`exp` 是 JWT 标准声明，网关侧 `verify_user_token` 只比对时间戳，
   秒级精度天然支持；本次改动只在签发侧。

## Alternatives considered

- **把 `m` 改成分钟**：否决。存量脚本/文档里的 `-d 3m` 会从 3 个月变成 3 分钟，
  是静默的计费语义破坏；且 `1y` 就再没有对偶的月单位。
- **`-d 0.0007`（小数天）**：否决。`parse::<u64>` 不支持浮点，引入浮点会让时间戳出现
  舍入误差，且与「自然月」的整数语义不一致。
- **新增独立参数 `--expires-seconds`**：否决。两套时长入口意味着两套校验、两处文档，
  且用户仍需记住「短周期走另一个 flag」；单入口 + 显式单位更省心智。
- **环境变量/隐藏 flag 开后门给测试用**：否决。不可审计，且必然被误用于生产签号。
- **仍然只保留天，用改系统时钟来测**：否决。网关是生产 systemd 服务，
  改时钟会影响 TLS 校验、令牌 iat/exp 判定与配额周期滚动，风险远大于收益。

## Consequences

- 到期验收变成 1 分钟级：`pproxy user rm X && pproxy user add X -q 1G -d 1min`，
  60 秒后即可观察 401/407，无需等待或改时钟。
- 代价是「`1m` ≠ 1 分钟」这一条反直觉约定需要持续在文档与 help 中可见；
  已同步 README、README.en、`docs/ops/DEPLOY.md` 与 `-d` 的 help 文案（same-commit）。
- 秒级令牌会让 `iat == exp - 60` 这类极端值出现在撤销表清理逻辑里；
  当前撤销表只按 `jti` 匹配，不依赖时长，无影响。

## Verification

- `cargo test -p pproxy-cli --offline` → **137 passed; 0 failed**，新增 `expiry_subday_units`
  覆盖 `30s` / `1min` / `5 min` / `2h` / `1Hr`、`1min < 1m` 的反向断言、
  `u64::MAX` 秒溢出拒绝与 `0min` 拒绝。
- 既有 `expiry_rejects_zero_and_invalid` 保持通过：`1w`、`1.5m`、`m` 仍被拒。
- 生产端到端（2026-11-12 18:25–18:26 UTC+8，`rn.ponygo.fun`）：
  - 到期前：订阅 `HTTP/2 200` + `subscription-userinfo: total=1073741824; expire=1794479169`；
    8443 隧道 `CONNECT_HTTP=200`。
  - 到期 +7 秒：订阅 `HTTP 401 Unauthorized: invalid token`；
    隧道 `HTTP/1.1 407 Proxy Authentication Required`；网关日志
    `subscription request rejected reason="invalid token"` 与
    `CONNECT denied: missing or invalid credentials`。
  - `pproxy user usage test123` → `state=令牌过期`，已用 4.2 KB 停在到期时刻。
- 待改进（已登记 B019 关联项）：网关把「签名错」与「已过期」同归为 `invalid token`
  与 `missing or invalid credentials`，排障时无法从日志区分两者。
