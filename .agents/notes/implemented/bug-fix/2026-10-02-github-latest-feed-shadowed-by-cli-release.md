# Agent Note: GitHub `releases/latest` 更新端点被 CLI 发布抢占（桌面自更新报错修复）

Status: implemented

> 相关旧条：[2026-09-26-fix-desktop-updater-feed.md](2026-09-26-fix-desktop-updater-feed.md)（服务端 /dsk 分发 404 故障，已修复；本条的 GitHub latest 被抢占是与旧故障叠加的新根因，未 supersede 旧条）。

## Decision

1. **根因**：桌面端 updater 第一端点 `https://github.com/lanhui100/pproxy/releases/latest/download/latest.json` 依赖 GitHub "latest" 语义（最近创建的、非 draft、非 prerelease 的 Release）。2026-09-30 发布 CLI `v0.3.57`（该 Release 只含 pproxy CLI 二进制、无 latest.json 资产），把 "latest" 位子占掉，导致该端点 302 → `v0.3.57/latest.json` → 404。更新器逐个端点尝试，最终在最后一个端点 `access.ponygo.fun/dsk/latest.json` 失败时报 `error sending request`。
2. **临时修复（本次落地，无需用户重装）**：将 `desktop-v0.3.62` 的 `latest.json` 与 `Pony.Proxy_0.3.62_x64-setup.exe.sig` 原样上传到 `v0.3.57` Release（`gh release upload v0.3.57 --repo lanhui100/pproxy --clobber`），恢复 `releases/latest/download/latest.json` 通道。feed 内容保持 0.3.62，exe 指向 `desktop-v0.3.62` 的 GitHub 资产，全链路 GitHub 可达。
3. **防复发（机械校验）**：新增 `scripts/check-update-feed.sh <version>`，以非零退出断言最新 feed 可用且版本匹配，接入后续发布流程前自检。
4. **治本方向（后续 0.3.63 打包时执行，另行记录）**：updater endpoints 改为「固定 tag 的 GitHub feed + access.ponygo.fun」双通道，避免依赖 `releases/latest` 语义；或将 CLI 发布与 desktop feed 解耦。
5. **已知边界（客户端侧）**：`access.ponygo.fun` 从服务器侧实测 200/可达；用户机器报 `error sending request` 属客户端建连失败（DNS/被阻断/系统代理指向 18900 而隧道未开），且 `pac.rs` 将 updater 域名列入直连 bypass（隧道开着也绕行直连），国内直连 Cloudflare 不稳定时仍可能报错——此项仅能由治本方向缓解，非本次临时修复范围。

## Alternatives considered

- **改 `releases/latest` 为固定 tag 端点**：需要重打包客户端才生效，无法解决现有 0.3.62 用户，故本次只做服务器端资产补位。
- **给 v0.3.57 加空 latest.json**：可能让 CLI 下载者误入 desktop feed，不如直接放与 desktop-v0.3.62 一致的完整 feed。
- **删除/降权 CLI v0.3.57**：破坏 CLI 发布，不可取。

## Consequences

- `curl -L https://github.com/lanhui100/pproxy/releases/latest/download/latest.json` → 200，`version: 0.3.62`，exe URL 指向 `desktop-v0.3.62` 资产；
- `v0.3.57` Release 资产含 `latest.json` + `Pony.Proxy_0.3.62_x64-setup.exe.sig`（已验证）；
- 0.3.62 客户端在可访问 GitHub 的网络下「检查更新」恢复可用。