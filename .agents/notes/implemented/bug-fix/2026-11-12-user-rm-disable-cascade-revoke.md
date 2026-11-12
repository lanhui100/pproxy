# Agent Note: user rm/disable 级联撤销——删号停用必须等于断网

Status: implemented

## Problem

`pproxy user rm <name>` 与 `pproxy user disable <name>` 此前只改本地 SQLite（`users` 表），
不触碰网关的撤销黑名单。令牌是 Ed25519 **自包含**的：签名对、未过期、不在黑名单，网关就放行。
结果是「删号/停用」与「断网」脱钩——2026-11-12 实测同一张 `usr_live_` 令牌在 `user rm` 之后仍能
`200` 拉订阅、继续走 8443 隧道直到 `exp` 到期（会员默认 30 天、最长 1 年）。

放大器有两个：

1. **推送通道只服务于 `user revoke`**：热更新逻辑内联在 `crates/cli/src/cmd/user.rs` 的
   `revoke()` 里，`rm`/`disable` 各自独立实现，没有共用件可调。
2. **生产管理端点是死的**：`/opt/pproxy/.gate-server.env` 无 `GATE_ADMIN_TOKEN`，
   `handle_revoke` 对空口令配置一律 `401`；且网关 `HOST=127.0.0.1`（`PORT=3101`），
   管理机 dev 无法直连，只能人工 `ssh` 改 `revoked_tokens.txt` 再重启——一次停用要两分钟且易漏。

对商业化模型而言这是账目漏洞：额度按 `sub`（`usr_<name>`）记账，账号已经不存在了，
额度仍在被消耗；`disable` 的语义（临时停机）也无法兑现。

## Decision

把「撤销」收敛成一条可复用的管理通道，并让删除/停用成为它的调用方：

1. **`crates/cli/src/cmd/gate_admin.rs`（新模块，单一职责：把标识符塞进网关黑名单）**
   - 口令来源：`GATE_ADMIN_TOKEN` 环境变量 → `~/.pony/gate_admin.conf`（0600）的 `token=`
   - 传输路径：`ssh=<host>`（经既有运维 SSH 通道在网关本机 `curl`）→ 否则 `url=`/`GATE_ADMIN` 直连
     默认 `http://127.0.0.1:3101`
   - 口令与 JSON body 走 **stdin**，不进 `ssh` 命令行，规避 `ps` / `/proc` 窥屏
   - 标识符白名单（`[A-Za-z0-9._-]{1,128}`）作为纵深防御，再叠一层 JSON 转义
2. **撤销标识符按 jti 精确选择**（`revocation_identifiers`）：`users.jti` 有值 → 撤销该 jti；
   `NULL`/空白（老库 legacy 行）→ 回落 `usr_<name>` 并显式告警「同名重建需先摘除」。
   选 jti 而非 sub 是为了不连坐：同名用户重建后新 jti 立即可用（实测 200）。
3. **`rm` / `disable` 级联**：删除/停用前先 `UserService::get_user` 取快照（删完就无从查 jti），
   成功后调用同一 `enforce_revocation`；任一环节失败（远端未生效）返回 `EXIT_FAILURE`，
   让「停用没生效」成为脚本/CI 可查的事实，而不是一行容易被忽略的日志。
4. **生产配置一次：** 生成 32 字节随机口令写入 `/opt/pproxy/.gate-server.env`
   （`GATE_ADMIN_TOKEN=…`，0600，留 `.bak-<ts>`），本机写 `~/.pony/gate_admin.conf`
   （`token=…` + `ssh=racknerd`），重启网关生效。

## Alternatives considered

- **撤销 sub（`usr_<name>`）而非 jti**：一次覆盖该用户名下所有令牌，最"彻底"；
  但同名重建会被永久挡住，除非再加一个 unrevoke 端点（多一条可被滥用的写路径）。
  jti 已经是 DB 真源，撤销粒度没有损失，否决。
- **网关改成读 DB 判令牌有效性**：语义最正（DB 即真相），但网关在独立 VPS、无 DB 通道，
  且每握手一次远程查询会把可用性绑在 DB 上；当前架构刻意让网关无状态可验签，否决。
- **CLI 直接把 `revoked_tokens.txt` scp 到 VPS**：省掉管理端点，但覆盖会丢掉网关运行期
  自己追加的条目（如未来的自动封禁），且内存黑名单仍需重启；否决。
- **暴露 3101 到公网并用 Bearer 保护**：省掉 SSH 一跳，但把管理面暴露到互联网，
  与「管理通道只走运维通道」相反；否决。
- **`user disable` 用 sub 撤销、`enable` 自动 unrevoke**：语义最贴合"临时停机"，
  代价是引入 unrevoke 端点 + 并发语义（重新启用与撤销竞态）；本轮以 jti 撤销 + 提示重签收口，
  需要"真·暂停/恢复"时另立条目（见 B061 的 residual）。

## Consequences

- `user rm` / `user disable` 现在默认就断网：网关新握手（WS / Profile / CONNECT）返回 401，
  实测同一令牌 `/clash` 由 `200` → `401`，网关日志留 `subscription: token revoked` /
  `CONNECT: user token revoked` 两跳。
- 退出码语义变得更严格：撤销未生效时 `rm`/`disable`/`revoke` 返回非零（删除/停用本身已完成，
  故文案明确区分「已删除」与「撤销未完成」）。
- **作用边界**：撤销只拦新握手，已在传输中的长连接不中断（靠连接超时自然收敛）；
  这不属于本轮范围，已写入 CLI 回显与下文 residual。
- `enable` 不会复通旧令牌——必须重签（`user rm` 后重建，或待 B019 的 `renew` 落地）；
  CLI 在该命令上直接提示，避免"启用了却上不了网"的误判。
- 生产可运维性：管理口令与通道集中在两处（VPS env + 本机 0600 conf），
  换通道只改一个 `ssh=`/`url=` 键；口令不进 shell 历史、不进进程表。
- residual（仍在 B061）：`disable` 的真·暂停/恢复需要 unrevoke 端点；在途连接需要主动断开
  或更短的连接 TTL；`crates/server` 的 `/clash` 无鉴权回吐磁盘令牌（B019 已登记）未在本轮收口。
