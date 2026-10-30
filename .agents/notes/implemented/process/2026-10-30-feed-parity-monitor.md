# Agent Note: 桌面 feed 双通道改用每日异步巡检，不塞进发版闸门

Status: implemented

## Decision

新增 `.github/workflows/feed-parity.yml`：每天 02:30 UTC（北京 10:30）与手动 dispatch 时，取最新 `desktop-v*` tag 的版本号，跑 `scripts/check-update-feed.sh <version>`，要求 GitHub latest 与 access 分发两个通道都等于该版本、且 feed 内产物 URL 可达；不对齐即 workflow 失败（GitHub 通知到人）。发版流程 `desktop-release.yml` 保持只跑 `release-gate.sh <ver>`（preflight）与 `check-update-feed.sh --github-only <ver>`（GitHub 通道），**不**加 `release-gate.sh --feed`。

## Alternatives considered

- **把 `release-gate.sh --feed` 接进 `desktop-release.yml` 发版后步**：判据最强，但 access 通道由网关主机 15 分钟 timer 异步 sync，发版瞬间必然可能滞后 → 要么永久红，要么 `continue-on-error`（等于假门禁）。否决。
- **在发版后 sleep/重试等待 access 收敛（如 15–30 分钟）**：会把发版 workflow 拖长且仍依赖 timer 抖动；失败时也分不清「同步没跑」还是「发版包坏了」。否决。
- **完全不管，只留文档**：2026-10-02 的事故（CLI 发布抢占 GitHub latest）就是「没人定期看」的产物；文档不产生通知。否决。
- **每日巡检（采纳）**：与「发布时刻无硬门禁」的真实约束一致，把「最终对齐」变成有通知的机械判据；代价是发现滞后最多延迟一天（可接受：滞后窗口内老版本仍可用，updater 只在有新版时提示）。

## Consequences

- `feed-parity` 每日一次网络调用（两个 feed + 产物 HEAD），失败即 issue 通知；公开仓 GitHub Actions 免费额度足够。
- 发版人仍须在发版后确认 access 已同步（`sync-desktop-release.sh` 由 timer 自动跑）；本 workflow 兜底「最终一致」，不替代发版前 preflight。
- 判据唯一真源仍是 `scripts/check-update-feed.sh`；workflow 只做「取最新 tag + 调用」，不复制判据。
