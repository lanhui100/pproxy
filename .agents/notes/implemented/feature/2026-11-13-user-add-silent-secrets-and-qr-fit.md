# Agent Note: `user add` 输出二次收敛——凭据行整行不出现、二维码压进 80 列

Status: implemented

> 前序：`implemented/feature/2026-11-13-user-add-user-facing-output.md`（把输出换成人话、
> 凭据默认「已遮挡」、二维码保持默认显示）。本条只修它落地后暴露的两个观感缺陷。

## Problem

前一条把凭据从「明文」改成「已遮挡（创建时加 --show-secrets 才会显示）」，实测交付时仍然刺眼：

1. **遮挡提示本身就是噪音**。客户拿到的是开通结果，不是 CLI 用法说明书；
   两行 `已遮挡（创建时加 --show-secrets 才会显示）` 既暴露了内部开关的存在，
   又把交付界面占据两行——而真正要交付的东西（订阅地址 + 二维码）反而被挤下去。
2. **二维码占满屏幕**。订阅 URL 内嵌 `usr_live_` 令牌，负载约 340–380 字节；
   旧实现用 `EcLevel::M` + 默认 4 模块静默区 + 2 空格缩进，实测 **81 列 × 39 行**，
   在 80 列终端里整行折行，二维码糊成不可扫的一片。

## Decision

1. **默认整行不出现**：未加 `--show-secrets` 时，`登录密码` / `用户令牌` 两行**根本不打印**，
   也不打印任何「已遮挡」提示。口径收敛为纯函数 `cmd::user::secret_lines(pass, token, show)`：
   返回 `Vec<String>`（空 = 这一屏压根不提凭据），密码路径（遗留 Basic 开通入口）传 `None` 不硬塞令牌行。
   订阅地址下方的提醒同时改为「下方订阅地址内含**你的**用户令牌」——原来说「上面的用户令牌」，
   而上面已经不再有那一行。
2. **二维码三管齐下瘦身**：`render_qr` 改用 `EcLevel::L`（屏幕没有物理磨损，7% 冗余够用，
   版本直降 2 级）；保留 `Dense1x2` 折半行高；按 `QR_MAX_COLS = 76` 自适应决定是否保留静默区——
   先按带静默区渲染，超宽才去掉白边。实测 **71 列 × 35 行**，80 列终端不折行。
3. **验收可机械化**：新增单测 `cmd::clash::tests::test_render_qr_fits_80_cols_for_subscription_url`
   （380 字节真实负载 → 宽 ≤ 76、高 ≤ 45、宽 ≥ 41）与
   `cmd::user::tests::secret_lines_absent_by_default`（默认返回空、开启后 2 行、仅密码路径 1 行）。

## Alternatives considered

- **保留遮挡但缩短提示文案**（如 `已遮挡`）：仍然占两行、仍然提示了一个客户用不到的开关。否决。
- **默认把密码/令牌折叠成「按需再取」的子命令**（`user show <name>`）：方向正确，
  但令牌不可逆（重发即新 jti，旧 jti 需处理失效），属独立话题，本期不扩散。沿用前一条的 backlog 判断。
- **二维码改为在终端内等比缩放/分块打印**：终端没有图像缩放语义，跨终端宽度不可控，
  只能靠减少模块数（纠错级别）与减少边距（静默区）达成。否决「降采样」路线——会直接毁掉扫码率。
- **二维码默认不打印，只给 URL**：URL 有 300+ 字符，手输必错，扫码是最省事的交付路径。否决。
- **按 `COLUMNS` 环境变量动态选纠错级别**：`COLUMNS` 在非交互/CI 里常缺失，
  同一负载会渲出两种二维码，反而难做回归断言。改用「76 列上限 + 是否保留静默区」的二值自适应。

## Verification

```console
$ cargo test -p pproxy-cli
test result: ok. 141 passed; 0 failed; 0 ignored
# exit code 0

$ cargo build --release -p pproxy-cli   # 安装到 ~/.local/bin/pproxy
$ pproxy user add uicheck3 -q 1G -d 1h > /tmp/add3.txt
$ grep -cE '登录密码|用户令牌|已遮挡' /tmp/add3.txt
1        # 仅命中「下方订阅地址内含你的用户令牌」这句提醒，凭据行已消失
# 二维码块实测：35 行 × 71 列（旧实现 39 行 × 81 列）
$ pproxy user add uicheck4 -q 1G -d 1h --show-secrets | grep -E '登录密码|用户令牌'
  登录密码:   fc3e10ee41aec691
  用户令牌:   usr_live_<TOKEN>
$ pproxy user rm uicheck3 && pproxy user rm uicheck4   # 临时用户已清理
```

文档 same-commit：`README.md` §5「建号输出面向客户」条、`README.en.md` 同名条目同步改为
「默认不显示，`--show-secrets` 才列出密码与令牌」。
