# Agent Note: 订阅短码——把终端二维码砍到四分之一（/c/<code>）

Status: implemented

> 前序：`implemented/feature/2026-11-13-user-add-silent-secrets-and-qr-fit.md`（纠错级别降到 L、
> 去掉静默区，实测 71 列 × 35 行）。客户反馈「还是太大，要当前的四分之一」，本条是第二次收敛。

## Problem

终端 Unicode 二维码的宽度**等于模块数**，而模块数只由负载字节数决定（`EcLevel::L` 已是最低纠错级别）。
订阅 URL 内嵌 `usr_live_` 令牌（约 340–380 字节）→ 二维码版本 13 → 69 列 × 35 行。
在该口径下「再小」只有三条路，前两条已经被前序决策走完并在实现中封顶：

1. 降纠错级别 → 已到 L（再低不存在）；
2. 去静默区 → 已按 76 列自适应；
3. **缩短负载** → 未做，是唯一还剩的手段。

换句话说：不缩短 URL，就不可能把二维码再缩小——把它「画小一点」在终端里没有语义
（字符格是离散的，缩放只会降低扫码率）。

## Decision

**在网关侧引入 8 字符订阅短码，把二维码负载从 ~390 字节压到 32 字节。**

1. **负载形态**：`https://<host>/c/<code>`（32 字节）→ 二维码版本 2 → **33 列 × 17 行**，
   面积 561 格，是长令牌形态（69 × 35 = 2415 格）的 **23%**。
   短码字母表为 `23456789abcdefghjkmnpqrstuvwxyz`（30 符号，剔除 0/o/1/i/l/u 等易混字符），
   8 位空间 30^8 ≈ 6.5e11。
2. **短码不是第二套鉴权**：`/c/<code>` 与 `/clash?token=<code>` 都只是「还原成真实令牌」的入口，
   还原后仍走 `validate_subscription_token`（Ed25519 验签 + 撤销表 + 到期 + 额度）。
   YAML 里下发的 `username`/`password` 依旧是长令牌，订阅头依旧是真实用量。
3. **签发与存储**：`POST /api/admin/shortlink`（`GATE_ADMIN_TOKEN` Bearer，与撤销/用量同一管理通道）
   接收 `{"token":"usr_live_..."}`，返回 `{"ok":true,"code":"...","url_path":"/clash?token=..."}`。
   表落 `$HOME/.pony/short_links.json`（`GATE_SHORTLINK_FILE` 可覆盖），原子写（tmp + rename），
   启动按空表兜底（文件缺失/损坏不阻塞网关）。
4. **生命周期与令牌同源**：`exp` 取令牌 `exp`，解析时顺手清理过期条目；`/api/user/revoke`
   按 `jti`/`sub` 级联删除短码，因此 `user rm|disable|revoke` 之后短码立即 401——
   短码不能成为已撤销令牌的复活通道。
5. **CLI 无感接入**：`user add` 签发令牌后在同一次运行里申请短码，成功则打印 `/c/<code>`
   形态的订阅地址与深链，失败（未配管理通道）退回长链接并在 stderr 说明原因，退出码不变。
6. **多一条路由的理由**：`/clash?token=<code>` 是 42 字节（版本 3，37 列），`/c/<code>` 是 32 字节
   （版本 2，33 列）。为省这 10 字节加一条**复用同一 handler** 的路径，比在新路由里复制鉴权逻辑安全得多。

## Alternatives considered

- **把短码塞进 `/clash?token=`**（42 字节，二维码 37 列，29% 面积）：改动最小，
  但离「四分之一」还差 4 个百分点，且客户已经就尺寸反馈过两轮。否决。
- **缩短短码到 6 位**（配合 `/c/` 仍可进版本 2 的下限）：空间降到 30^6 ≈ 7.3e8，
  在公网无速率限制的前提下偏弱。否决——8 位 + 32 字节已经是版本 2 的容量上限（字节模式 32 字节），
  继续缩码并不换来更小的二维码，只换来更弱的空间。
- **用二维码 alphanumeric 模式**（同容量下版本更低）：需要全大写负载（含 `TOKEN=` 与路径），
  而 base64url 令牌大小写敏感；即使短码全大写，也要给 `/clash` 与 `token` 加大小写别名路由，
  把稳定性换给尺寸。否决。
- **把令牌本身编码得更紧凑**（CBOR/定长字段 + 截断签名）：能省到 ~150 字节，仍停在版本 9（61 列），
  且截断签名是安全性降级。否决。
- **二维码改渲染成 PNG 文件让用户自己打开**：脱离「终端扫码即用」的交付路径。否决。
- **在终端里对二维码降采样/缩放**：终端没有图像缩放语义，降采样直接毁扫码率（前序已否决）。

## Verification

```console
$ cargo test -p pproxy-cli -p pproxy-gate-server --offline
test result: ok. 142 passed   # pproxy-cli
test result: ok. 29 passed    # pproxy-gate-server lib（含 test_short_code_resolves_to_real_token）
test result: ok. 8 passed     # connect_proxy 集成
# exit code 0

$ cargo test -p pproxy-cli --offline test_render_qr_short_code_is_quarter_area -- --nocapture
qr: long 69x35 = 2415, short 33x17 = 561 (23%)
# exit code 0 —— 断言：短码面积 ≤ 长链接的 1/3，且宽度 ≤ 35 列
```

短码语义的机器判据（`test_short_code_resolves_to_real_token`）：
`/clash?token=<code>` 与 `/c/<code>` 都回 200，YAML 中 `username` 为**真实长令牌**；
未知短码 401；`POST /api/user/revoke` 撤销 `jti` 后短码表清空且同一短码 401。

文档 same-commit：`README.md` §5 建号输出与「订阅短码与令牌等权」条、`README.en.md` 对应条目、
`docs/ops/DEPLOY.md` 新增 `GATE_SHORTLINK_FILE` 与「订阅短码（二维码尺寸）」小节。

## Consequences / 残余风险

- **短码是 bearer 凭据**：8 字符等于该用户全部订阅权限（与令牌等价），泄露后果与泄露订阅 URL 相同，
  `user revoke|rm|disable` 即止损。它不会扩大权限面，但把「凭据看起来很短」这件事变成了诱导性风险，
  已在 README 明示。
- **公网枚举**：`/c/<code>` 无速率限制（网关既有能力里没有 limiter），30^8 空间下盲猜不可行，
  但缺少失败审计。已登记 backlog。
- **短码表是网关本地状态**：多实例部署时各实例表独立（当前生产单实例）。若将来横向扩展，
  需把短码表挪到共享存储，否则短码只在签发命中的实例上生效。已登记 backlog。
