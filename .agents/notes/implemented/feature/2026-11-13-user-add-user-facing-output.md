# Agent Note: `user add` 屏幕输出改为面向用户——凭据默认遮挡

Status: implemented

> 前序：`implemented/feature/2026-11-12-user-membership-provisioning-and-real-subscription-headers.md`
> 定下了「一条命令闭环」的**内容**（自然月时长、订阅深链、配额同源落库）；本条只改它的**呈现**。

## Problem

`pproxy user add -q …` 是交付给「客户」看的最后一块界面，但它的措辞是给工程师看的：

- 抬头写「✓ 商业化多租户自包含令牌签发成功 (Ed25519 签名)」——既暴露签名算法（选型属内部信息），
  也把「商业化多租户」这类架构词摆到客户面前；
- 正文混入 `公钥校验 <hex>`、`用户 ID`、`(局域网 Basic / 订阅客户端校验用)`——
  公钥对租户毫无用处，用户 ID 是内部主键；
- **密码与令牌默认明文打印**。共享屏幕、录屏、截图发给客户支持时，凭据直接随截图外泄；
  更糟的是订阅地址本身就内嵌令牌，一次截图等于交出账号。
- 帮助文案（`--help`）同样是术语：「商业化自包含令牌：周期总配额」「最大并发连接数」。
  客户不知道「并发连接数」是什么，只知道「几台设备能同时用」。

## Decision

1. **抬头与正文换成人话**：`✓ 用户开通成功`；字段改为 `用户名 / 身份 / 每月流量 / 有效期至 /
   同时在线 / 登录密码 / 用户令牌`。删除「公钥校验」「用户 ID」「Ed25519」「自包含」等字样。
2. **新增 `--show-secrets`（默认关闭）**：登录密码与用户令牌默认打印
   `已遮挡（创建时加 --show-secrets 才会显示）`。遮挡口径收敛到单一纯函数
   `secret_display(value, show_secrets)`，两个分支（令牌模式 / Basic 模式）共用，便于单测。
3. **公钥永不显示**：`add` 里签名器的验签公钥绑定改为 `_vk_hex`，不再出现在任何用户可见路径。
   （`user keygen` 是唯一例外——公钥就是它的产出物，没有公钥该命令无意义。）
4. **订阅地址与二维码保持默认显示**，并在其后标注「订阅地址内含上面的用户令牌，等同账号凭据」。
   理由见下节：`add` 不可重放，若一并遮挡，管理员必须 `rm` 再重建才能拿到交付物。
5. **三步上手 + 前置约束**：`1 装 App → 2 扫码导入 → 3 开代理开关`，
   并把「扫码前请先关闭手机上正在运行的其他 VPN / 代理，否则订阅下载会失败」
   放在二维码之前——这是实测中 `tls handshake timeout` 最常见的客户端成因。
6. **帮助文案去术语**：`--quota` → 「每月流量额度（如 50G、100M）」；
   `--max-conns` → 「最多几台设备同时在线（默认 3）」；`--admin` → 「开通为管理员（默认普通用户）。
7. **数值按量级显示，不写死单位**（验收时抓到的回归）：`每月流量` 原先固定两位 GB，
   5M 额度被格式化成 `0.00 GB`，客户会读成「没给流量」；改为 `render::human_bytes`
   走 B/KB/MB/GB 自适应。`有效期至` 原先固定 `{} 天`，分钟级试用显示 `0 天`；
   改为 `duration_label`（天/小时/分钟/秒），并补两条单测锁住这两个口径。

## Alternatives considered

- **全部默认遮挡（含订阅地址与二维码）**：语义最一致，但 `user add` 对同一用户名不可重放
  （已存在即报错，防止两套互不知情的凭据）。默认遮挡会让「建号」与「拿到交付物」必须分两步，
  管理员只能 `rm` 后重建，反而制造停机。否决。
- **默认明文、只加 `--quiet` 之类的关掉开关**：默认不安全，与「防截图泄露」的初衷相反。否决。
- **新增 `pproxy user show <name>` 子命令读取凭据**：正解方向（凭据可按需再取，`add` 就能全遮挡），
  但需要新增一条从 DB/网关读回明文令牌的路径（令牌是不可逆签发的，重发即产生新 jti，
  还要处理旧 jti 失效），属于独立话题，本期只登记 backlog，不在本次改动里扩散。
- **按字段分组打印（公钥/内部字段折叠）**：终端没有折叠语义，实现成本高于直接删。否决。

## Verification

```console
$ cargo test -p pproxy-cli -p pproxy-core -p pproxy-gate-server
test result: ok. 140 passed; 0 failed  (pproxy-cli)
test result: ok. 110 passed; 0 failed; 1 ignored  (pproxy-core)
test result: ok. 22 passed; 0 failed  (pproxy-gate-server)
# exit code 0
```

新增单测 `cmd::user::tests::secret_display_hides_by_default`：断言默认输出含「已遮挡」且
**不含明文凭据**；`--show-secrets` 时反之。
另加 `duration_label_covers_subday_and_long_lived` 与 `quota_line_uses_human_bytes_not_always_gigabytes`，
分别锁住「分钟级试用显示 0 天」和「5M 显示 0.00 GB」两个回归。

人工取证（临时用户验证后已 `user rm` 清理）：

```console
$ pproxy user add uicheck -q 5G -d 1m -c 3
  登录密码:   已遮挡（创建时加 --show-secrets 才会显示）
  用户令牌:   已遮挡（创建时加 --show-secrets 才会显示）

$ pproxy user add uicheck2 -q 1G -d 30 --show-secrets
  登录密码:   2cd00c2a3788f5e7
  用户令牌:   usr_live_eyJqdGki…
```
