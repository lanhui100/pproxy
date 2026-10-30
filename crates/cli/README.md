# pproxy-cli

`pproxy` 命令行入口：参数解析 → 子命令执行 → 退出码。管理面 API 是唯一状态源，本 crate 只有
客户端逻辑与本地文件（`render.rs` 负责输出形态，`client.rs` 封装管理面调用）。

## 配置项

- `~/.pony/config.toml`（0600，`config.rs`）：管理面地址、admin token、隧道配置；写盘时同步导出
  `~/.pony/.pproxy.env`。
- 全局旗标（任意子命令可用）：`--server` 覆盖管理面 base URL、`--data-plane` 覆盖数据面 base URL
  （`export`/`doctor` 用）、`--token` 覆盖 admin token。admin token 的解析顺序（按代码实况）：
  `--token` > `PPROXY_ADMIN_TOKEN` > `PONY_ADMIN_TOKEN` > `config.toml`。
- 命令读取的环境变量：`PPROXY_EDGE_URL` / `PPROXY_VERCEL_URL`、`PPROXY_CLUSTER_KEY` /
  `PPROXY_CLUSTER_NODE_ID` / `PPROXY_CLUSTER_PEERS`、`PPROXY_CLIENT_TOKEN`、`PPROXY_CF_API_TOKEN`、
  `PPROXY_VERCEL_TOKEN`、`PPROXY_DEPLOY_DIR`、`PPROXY_LISTEN_ADMIN`、`PONY_DIST_URL`（自更新源）、
  `PONY_GITHUB_TOKEN`、`PPROXY_TUNNEL_GATE_URL` / `PPROXY_TUNNEL_TOKEN` / `PPROXY_TUNNEL_TOKEN_DATA` /
  `PPROXY_TUNNEL_ALLOWLIST`。

## 语义

- **命令面** = `src/cmd/` 下 17 个文件（16 个命令模块 + `mod.rs` 注册），顶层子命令见 `main.rs` 的
  `Command` 枚举：`serve`、`ha-forwarder`、`init`、`status`、`on` / `off` / `env`、`route`、`token`、
  `usage`、`doctor`、`user`、`cluster`、`sync`、`clash`、`client`、`deploy`、`migrate`、`gate-server`、
  `config`、`upgrade`（别名 `update`）。
- **退出码契约**（`main.rs` 顶部常量，四档）：`0` 成功 / `1` 执行失败（`Msg` 类错误）/ `2` 本地配置错误
  （`config.toml` 读取或格式问题）/ `3` 管理面连接不可达（`cmd/service.rs` 把 `ApiError::Connection`
  映射为该档，其他子命令的同类错误仍按 `1` 报）。错误统一 `error: …` 写 stderr。
- **交互语义**：`init --interactive` 逐项询问（token/密钥类输入不回显）；`on --eval` / `off --eval` 只把
  `export` / `unset` 语句写到 stdout，需调用方 `eval` 才生效；`off` 另有就地清理当前 shell 代理变量的路径；
  `clash --qr` 额外渲染二维码（`qrcode`）。非交互路径一律不读 stdin。
- **状态归属**：`serve` 拉起的是同进程内嵌引擎（`pproxy-engine`），管理面/数据面协议见
  `docs/ops/API.md`；拓扑与出口分层见 `docs/architecture/CURRENT.md`。

## 限制

- 提示输入是阻塞式 `stdin` 行读，**没有 TTY 检测**：管道/CI 里跑 `init --interactive` 会用空行顶掉默认值，
  自动化场景应改用 `init --server … --token …` 显式参数。
- 退出码只分四档，不按子命令细分失败原因（原因看 stderr 文本）。
- 不持有任何服务端状态：SQLite、路由表、配额都在服务端；CLI 只读写本地配置与调用 API。
- `sync` 的口令是自包含密文（ChaCha20-Poly1305），不校验对端版本——跨代兼容靠 review。

## 扩展点

- 新子命令：`main.rs` 的 `Command` 变体 + `cmd/<name>.rs` + `cmd/mod.rs` 注册（三处同一提交）。
- 新管理端点：`client.rs` 加方法，`crates/server/src/api.rs` 加 handler；协议文档同步到 `docs/ops/API.md`。
- 输出形态统一走 `render.rs`（表格/颜色/静默开关），不要在各命令里各自拼字符串。
- 加命令后回头核对本节命令面清单与根 `README.md` 文档索引是否仍准确（否则就是 same-commit 欠账）。
