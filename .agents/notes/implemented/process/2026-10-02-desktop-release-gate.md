# Agent Note: 桌面发版校验门禁（release-gate + feed 保护防复发）

Status: implemented

## Decision

2026-10-02 连续两起"发布后才发现"的故障（Dashboard 图例/UI 缺失隐患、更新 feed 被 CLI 发布抢占导致检查更新报错）后，将发版校验机械化为非零退出门禁并接入发布工作流：

1. **`scripts/release-gate.sh`（发版前 preflight）**：
   - 版本一致性：`desktop/package.json` / `desktop/src-tauri/tauri.conf.json` / `src-tauri/Cargo.toml` 三处必须一致且等于期望版本（防漏 bump 导致的"新版本没发出去"）；
   - 前端质量：`pnpm check`（vue-tsc）+ `pnpm test`（vitest，当前 116 用例）+ `pnpm build`；
   - **产物 UI 字符串门禁**：`近 7 日用量`/`近 24 小时用量`/`实时网速`/`今日请求` 必须出现在 `desktop/dist/assets/` 构建产物中，缺失即拦——把"关键 UI（图表图例）在产物里被删/未渲染"这类回归挡在发布前。
2. **`scripts/check-update-feed.sh` 扩展 `--github-only`**：发布后（CI）只硬校验 GitHub `releases/latest` 通道；`access.ponygo.fun` 由网关主机手动 `sync-desktop-release.sh` 更新、发布时刻可能滞后，常规模式（人工）全通道严格，CI 用 GitHub-only，避免 CI 误报。
3. **工作流接线**：
   - `desktop-release.yml`：`frontend build` 后跑 `release-gate.sh "${GITHUB_REF_NAME#desktop-}"`；发布完成后跑 `check-update-feed.sh --github-only "$VER"`。
   - `cli-release.yml`：general/CLI 发布（`v*`/`cli-v*`）时自动把最新 desktop release 的 `latest.json` 复制进本次 release 并校验 GitHub latest 通道——从源头封死"CLI 发布抢占 GitHub 'latest' 导致 `releases/latest/download/latest.json` 404"（2026-10-02 事故一）。
4. **文档**：`docs/ops/DESKTOP-TROUBLESHOOTING.md` 增补"图例不显示排查 + 发版门禁"一节；根因与修复详见 existing bug-fix note `2026-10-02-github-latest-feed-shadowed-by-cli-release.md`。

## Alternatives considered

- **组件渲染测试（@vue/test-utils + happy-dom）**：能直接断言图例 DOM，但需新增前端依赖并 mock 大量 Tauri 调用，脆弱且离线不可安装；改用"产物字符串门禁"，零依赖、机械、非零退出，作为诚实的最小充分校验。
- **仅靠人工发版 checklist**：不可执行、易漏，违反"机械可查的承诺配非零退出命令"命约，否决。
- **让 `access.ponygo.fun` 也进 CI 硬门禁**：该域绕过 CI 部署、更新时机在人力手里，若作硬门禁会误杀发布，否决；保留人工全通道模式。

## Consequences

- 发版链路里任何 UI 文本缺失/版本不一致/单测失败都会红灯拦截；
- GitHub latest 通道在每次 desktop 与 general/CLI 发布后均被校验或重新保护；
- `./scripts/release-gate.sh`（无 `--feed`）在本机实测 PASS；`--feed` 模式实测在 GitHub 可达时 PASS（本机到 github.com 网络波动时按设计 fail-closed）。