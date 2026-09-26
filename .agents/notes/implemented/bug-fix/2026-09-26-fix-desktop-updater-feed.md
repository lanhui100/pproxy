# Agent Note: 修复桌面端自更新检查与分发链路故障

Status: implemented

## Decision

1. **解决服务端 /dsk/latest.json 404 与分发目录缺失问题**：
   - 线上部署默认分发目录为 `/opt/pony-desktop-releases`，由于软链接未就绪且 `PPROXY_DESKTOP_DIST_DIR` 未显式配置，导致公网 CF Tunnel 入口 `https://access.ponygo.fun/dsk/latest.json` 返回 404（从而报 `error sending request / not found`）；
   - 在 `crates/server/src/dsk.rs` 中增加容错与回退：若 `dist_dir()` 读取失败，自动尝试回退读取 `$HOME/pony-desktop-releases`，避免因软链接或配置未注入引发分发服务 404；
   - 在主机建立 `/opt/pony-desktop-releases -> /home/dm/pony-desktop-releases` 稳定软链接并重启 `pproxy` 服务；
   - 更新 `scripts/sync-desktop-release.sh` 改写逻辑，将 `latest.json` 内的产物下载地址统一指向 `https://access.ponygo.fun/dsk/`（支持 `PPROXY_DIST_BASE` 覆盖）。
2. **桌面端自更新检查双通道容错**：
   - 在 `desktop/src/composables/useUpdater.ts` 中增强容错：若本地加速引擎在跑，优先经系统代理通道检查；若检查异常，自动 fallback 直连重试一次，消除因本地代理临时端口或链路切换造成的网络检查中断；
   - 在 `desktop/src-tauri/src/proxy/pac.rs` PAC 旁路白名单列表中补齐 `access.ponygo.fun` 与 `dl.ponygo.fun`，确保更新检查流量在 PAC 模式下始终直连。

## Alternatives considered

- **仅依赖 GitHub 官方 Releases 作为单点**：国内网络环境直接拉取 GitHub raw 或 releases 存在 DNS 污染和握手超时风险，因此必须保障 `access.ponygo.fun/dsk/` 和 GitHub Releases 双重可达。

## Consequences

- `curl -i https://access.ponygo.fun/dsk/latest.json` 稳定返回 200 OK 并下发最新版本；
- 产物二进制下载地址 `https://access.ponygo.fun/dsk/Pony.Proxy_0.3.57_x64-setup.exe` 响应 200 OK；
- 桌面端单测与 `cargo test` 全部通过。
