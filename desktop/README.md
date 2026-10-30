# pony-desktop —— Windows 桌面端（Tauri v2 + Vue 3）

桌面端是 pproxy 的图形外壳：把「接入令牌 → 隧道 → 系统代理」这条链路做成一次点击可完成的事，
并在本机跑一个**回环代理引擎**供系统代理与 PAC 使用。

- 技术栈：Vue 3 + TypeScript + Vite + Tailwind v4（`src/`）、Rust + Tauri v2（`src-tauri/`）
- 应用标识：`com.ponyjob.desktop`；可执行名 `pony-desktop`；安装包 NSIS（`SimpChinese`，`installMode: currentUser`）
- 后端（pony-server / Gate / Worker / Vercel）契约见根 `README.md` 与 `docs/ops/API.md`；
  排障记录见 `docs/ops/DESKTOP-TROUBLESHOOTING.md`

## 配置项

设置页（`src/views/SettingsView.vue`）里用户可改的项，落到本机配置（`app_config`）：

| 配置项 | 语义 | 缺省 |
|---|---|---|
| 接入令牌 / 授权码 | `pproxy-sync://` 口令、`pony-gate://` 授权码或裸 token；口令自带端点，粘贴即完成端点发现 | 空（未接入） |
| 加速模式 | `whitelist`（只代理白名单域名）/ `global`（除内网与直连名单外全走代理）；历史值 `rule`/`pac` 归一为 whitelist | `whitelist` |
| 白名单域名 | 用户增删的域名（支持别名展开）；决定 PAC 里哪些 host 走代理 | 空 |
| 服务器配置 | 自建/局域网 pproxy 的 `host:port`（如 `192.168.1.100:8899`）+ 用户名/口令——用于「不走授权码、直连自己的节点」的场景 | 空（走授权码解析出的端点） |
| 允许局域网设备接入 | 开则额外监听 `0.0.0.0:<port>`（见下）供手机/其它机器使用 | 关闭 |
| 开机自动开启代理 | 登录后自动启动引擎并接管系统代理 | 关闭 |

进程级变量（不走 UI，供脚本/运维覆盖）：

| 变量 | 作用 | 缺省 |
|---|---|---|
| `PPROXY_LAN_PORT` | 局域网接入监听端口（非法/0 回落缺省） | `8899` |

**两个监听面（别混：一个免鉴权、一个强制鉴权）**：

- `127.0.0.1:18900` —— 回环引擎，**免鉴权**，系统代理与 PAC 依赖它；处理 `CONNECT` 与
  absolute-form 请求分流。它只对本机可见，因此不做鉴权（加鉴权会打断 Windows「使用 PAC 脚本」
  这条系统级自动化链路）。
- `0.0.0.0:<PPROXY_LAN_PORT>`（缺省 `8899`）—— 局域网接入监听，**非回环来源强制 Basic 鉴权**；
  开关 = 配置键 `allow_lan_clients`（默认 false），退出/更新时主动停止监听。

## PAC 语义

系统代理走 PAC 模式：`AutoConfigURL = http://127.0.0.1:18900/pac?t=<时间戳>`。时间戳用于
刷新 WinINET 的 PAC 缓存（白名单或模式变化时强刷，否则改了规则不生效）。生成逻辑在
`src-tauri/src/proxy/pac.rs`（纯函数，可单测）。

PAC 文件返回值的四个硬口径：

1. **命中 → 单条 `PROXY 127.0.0.1:18900`，不追加 `DIRECT` 兜底。** 隧道挂了就该失败，
   而不是静默直连——`PROXY …; DIRECT` 会让用户以为在走代理、实际裸连出海。
2. **未命中（白名单模式）→ `DIRECT`。**
3. **全局模式** → 除内网地址与直连名单外一律 `PROXY 127.0.0.1:18900`。
4. **直连名单优先级最高**（含私网地址）：隧道端点自身、更新端点、核心服务域名强制 `DIRECT`——
   否则全局模式下连"下载/更新/回源"都绕海外出口，既慢又可能自断更新链路。

白名单条目在生成前做**别名展开**（`src-tauri/src/proxy/whitelist.rs`），故用户填一个域名
可能对应多条匹配形态；改动白名单/模式后若网络行为没变，先怀疑 PAC 缓存（见排障手册的急救箱：
一键清空注册表系统代理与 PAC 关联并广播系统消息）。

## 构建与发布

```bash
pnpm --prefix desktop install         # 依赖（packageManager 钉 pnpm@11.7.0）
pnpm --prefix desktop dev             # 前端热更（devUrl http://127.0.0.1:5178）
pnpm --prefix desktop check           # vue-tsc --noEmit
pnpm --prefix desktop lint            # oxlint
pnpm --prefix desktop test            # vitest
pnpm --prefix desktop smoke           # 契约冒烟（scripts/contract-smoke.mjs）
pnpm --prefix desktop build           # vue-tsc --noEmit && vite build
pnpm --prefix desktop tauri build     # NSIS 安装包（Windows）
```

- **门禁**：`.github/workflows/desktop-gate.yml` 在 `desktop/**` 变更的 push/PR 上跑前端与 Rust
  检查（**不打包**）；`desktop/scripts/check-invariants.sh <base-ref>` 是不变量门禁
  （契约层零漂移，`src-tauri` 仅白名单例外可动；需 `jq`，缺依赖 exit 3）。
- **发布**：`.github/workflows/desktop-release.yml` 仅由 `desktop-v*` tag 触发（`workflow_dispatch`
  为丢事件兜底），在 `windows-latest` 出 NSIS 安装包并挂 GitHub Release；`createUpdaterArtifacts: true`
  同时产出 `latest.json`。**构建未签名**——SmartScreen 首次提示属预期行为。
- **分发**：`scripts/publish-desktop-dist.sh` 把产物与 `latest.json` 推到
  `PPROXY_DESKTOP_DIST_DIR`（缺省 `/opt/pony-desktop-releases`），更新端点见 `tauri.conf.json`
  的 `plugins.updater.endpoints`（含 `dl.example.com` / `access.example.com` 占位，真实域名按
  `README.md` 部署清单替换）。
- 前端**不直连网络**：CSP 为 `connect-src 'none'`，所有后端交互经 Tauri 命令走 Rust 侧。

## 限制（明确不支持）

- **只出 Windows 包**（NSIS）。macOS/Linux 的 bundle target 未配置，构建脚本不产出对应安装包。
- **安装包未签名**：无法避免 SmartScreen 提示；企业分发需自行签名。
- **代理只支持 `CONNECT`（HTTPS/443）**：不提供明文 HTTP 正向代理；系统代理菜单里的
  「HTTP 代理」项不被用作入口（业务面代理 semantics 见根 README「协议边界」）。
- **回环引擎免鉴权**：本机任意进程都能用 `127.0.0.1:18900` 出网（这是系统代理链路的前提）；
  需要跨设备共享时才开局域网接入，且那条面强制 Basic 鉴权。
- **依赖 Windows 系统代理/注册表**：PAC 写入、缓存刷新、快照还原都走 WinINET 路径；非 Windows
  平台没有等价实现（也是不出别的包的原因之一）。
- **白名单模式无 DIRECT 兜底**：命中域名在隧道不可用时直接失败，不静默裸连（见 PAC 语义第 1 条）。
