# Agent Note: 网关验签公钥改为多公钥双信任（密钥轮换窗口）

Status: implemented

## Problem

B021 的表层现象是「dev 用 `pproxy user add -q` 签出的令牌，生产网关回 401 signature error」，
最初判断是「生产配错了公钥，换成 dev 公钥即可，在用的旧令牌本就是测试账号」。按此方案切换后
**实测推翻了「旧令牌是测试账号」这个前提**：

1. 切换后 2 分钟内 `journalctl -u pony-gate-rust` 出现 **62 次** `signature error`，
   而切换前 3 天累计仅 **2 次**——说明有一把**在用的旧令牌**被这次切换打死。
2. 该令牌是 `~/.pony/config.toml` 与 `.pproxy.env` 的 `PPROXY_TUNNEL_TOKEN`
   （`sub=usr_dm`、`quota=100GB`、`exp=2027-10-02`），用候选公钥逐一验签确认：
   **`9e20e627…` 验通、`e10ab1ce…` 验不通**。
3. 由此定位真实成因：`9e20e627…` 是 **dev 在 2026-10-03 轮换前**的旧公钥。
   `~/.pony/cluster_signing_key.hex` 的 mtime 正是 2026-10-03 15:56，`keygen` 覆写私钥且不留备份
   （`user.rs::keygen` 直接 `truncate(true)`），因此**旧私钥已不可恢复**；
   而生产一直配着旧公钥，于是：旧令牌能过、2026-10-03 之后签发的新令牌全过不去。
4. 关键约束：`ServerConfig.verifier` 只能挂**一把**公钥，「换钥匙」必然让另一批令牌全量失效——
   要么打死在用旧令牌（含 100GB 租户令牌），要么新签发的会员全部 401。

即：真正的缺陷不是「配错公钥」，而是**验签只支持单公钥，没有密钥轮换窗口**。

## Decision

1. `USER_VERIFYING_KEY` 支持**逗号分隔的多公钥**，顺序即信任优先级：
   `ServerConfig.verify_user_token` 先试主公钥（首项），再依次试历史公钥（`legacy_verifiers`）；
   首个验签通过即返回，全部失败回最后一个错误。全部未配置时返回 `None`，调用方按「未启用验签」处理。
2. 解析函数 `parse_verifying_keys`（`crates/gate-server/src/lib.rs`）对非法项**跳过并记 error 日志**，
   不因单个坏公钥让整台网关失去验签能力；两条启动路径（`main.rs` 二进制、`lib.rs::run_server`）
   统一走 `verifiers_from_env()`，避免能力分叉。
3. 生产实际配置为「dev 新公钥在前 + 旧公钥在后」：
   `USER_VERIFYING_KEY=e10ab1ce…,9e20e627…`，一次切换同时满足
   「CLI 新签发令牌可用」与「在用旧令牌不失效」。
4. 撤销表（`revoked_tokens.txt`）、配额熔断、订阅头口径均不变——本次只动验签入口。

## Alternatives considered

- **只切到 dev 公钥（原方案）**：实现最省（改一行 env），但实测会打死在用的 `usr_dm` 租户令牌
  （62 次/2 分钟验签失败、数据面重连抖动），且下次再轮换还会重演。否。
- **找回 `9e20e627…` 的私钥**：dev 私钥已被覆写且无备份，生产侧只有公钥（`/opt/pproxy` 下无任何私钥文件），
  dm-windows 因 SSH 22 不可达无法验证。以「已丢失」处置。
- **让 CLI 支持 fork 旧密钥签发**：等于把已泄露风险面（多重私钥）常驻在管理机，且旧私钥本就不存在。否。
- **强制全量重签**（`user rm` + `user add` 重建）：需同时改 `~/.pony/config.toml` 与 `.pproxy.env`
  两处凭据并重启本机服务，且用量账本按 `sub` 归集会断档；在双信任已能零中断覆盖的前提下属于多余风险。否。
- **引入 JWKS / 密钥版本号（kid）**：长期正确方向，但需要令牌格式与撤销链路一起演进，
  超出「让 CLI 签发的链接当场可用」的最小闭包。留作 B059 的演进项。

## Consequences

- 轮换从「一把钥匙全量切换」变成「追加新公钥 → 重签/等待旧令牌自然过期 → 摘掉旧公钥」三步，
  每步都可回滚，窗口内新旧令牌并存。
- 遗留风险：`9e20e627…` 私钥若在某处仍存在，可继续伪造有效令牌（撤销表只能按 `jti`/`sub` 拉黑）。
  旧令牌最晚 `exp=2027-10-02` 自然到期；摘除该公钥需新签租户令牌（B059 跟踪）。
- 安全边界不变：私钥仍只在管理机（0600），生产只持公钥、只能验签不能签发。

## Verification

| 断言 | 命令 | 结果 |
|---|---|---|
| 全工作区编译 | `cargo check --workspace --all-targets --offline` | exit 0 |
| 网关单测（含双信任 + 多公钥解析 2 条新用例） | `cargo test -p pproxy-gate-server --offline` | 21 passed / 8 passed（CONNECT 集成） |
| 生产二进制与本地构建一致 | `sha256sum` 两侧 | `7a8d802e…` 逐字相同 |
| 双公钥均加载 | `journalctl -u pony-gate-rust \| grep 'Verifier enabled'` | primary=`e10ab1ce…`、legacy=`9e20e627…` 各一行 |
| 旧令牌仍可用 | `curl -o /dev/null -w '%{http_code}' 'http://127.0.0.1:3101/clash?token=<usr_dm>'` | `200` |
| 新令牌可用 | 同上，token 来自 `pproxy user add qa_probe -q 1G -d 1m` | `200` |
| 无令牌拒绝 | `curl ... /clash` | `401` |
| 切换后无验签失败 | `journalctl --since <重启时刻> \| grep -c 'signature error'` | `0`（切换瞬间的同口径计数为 62） |
| 订阅头为真实口径 | `curl -sD - 'https://rn.ponygo.fun/clash?token=…' \| grep subscription-userinfo` | `upload=0; download=0; total=1073741824; expire=1797057111` |
| 数据面链路可用 | `curl -x 'https://<token>:<token>@rn.ponygo.fun:8443' https://example.com` | `http=200`，随后订阅头 `download=12386`（计量随流量增长） |

回滚：`cp -a /opt/pproxy/.gate-server.env.bak-<ts> /opt/pproxy/.gate-server.env && systemctl restart pony-gate-rust`；
二进制同理有 `pproxy-gate-server.bak-<ts>`（本次已实际执行过一次回滚验证：回滚后验签失败计数归 0、旧令牌 `200`）。
