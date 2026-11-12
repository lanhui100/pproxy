# Agent Note: 用量账本可观测化（user usage）与删号级联清零

Status: implemented

## Problem

两件事在 2026-11-12 同时暴露，根因是同一个：**用量账本没有对外入口，也没有生命周期归属**。

1. **查不到用量**：`pproxy user list` 只显示 `quota`（配额）与到期时间，不显示已用字节。
   运维要知道某个会员还剩多少流量、是否已被熔断，只能 `ssh racknerd` 读
   `/root/.pony/gate_usage.json`，或从网关 journald 里翻 `CONNECT denied: quota exceeded`
   日志——一次查询要跨越管理机、SSH、JSON 手工解析三层。而账本本身有权威入口
   （`/clash` 的 `subscription-userinfo`），只是不对外暴露。
2. **删号不清账本**：`user rm` 只删 `users` 行 + 级联撤销令牌（B061，`2026-11-12-user-rm-disable-cascade-revoke.md`），
   不触碰账本。2026-11-12 实测：`test123` 删掉后同名重建为 5M 新号，手机始终连不上，
   网关日志 `CONNECT denied: quota exceeded uid=usr_test123 used=15613483 quota=5242880`——
   新账号继承了上一轮同名测试的 15.6MB 历史消耗，**一创建就超额**（B063）。

对商业化模型来说第 2 条是计费语义错误：会员身份的生命周期（`users` 行）与用量生命周期
（账本 `usr_<name>` 条目）脱钩，删号无法表达「这个人的账目到此为止」。

## Decision

给管理面补两个端点，并让 `user rm` 成为它们的调用方——复用 B061 已建成的管理通道
（`crates/cli/src/cmd/gate_admin.rs`，口令 + `ssh=<host>`/直连两态），不新增第二条管理面。

1. **`POST /api/admin/usage`（网关，`GATE_ADMIN_TOKEN` Bearer 鉴权）**
   - Body 可省略 = 全量；`{"uid":"usr_alice"}` = 单租户（无记录也回 0，避免「查不到」伪装成「查询失败」）
   - 响应 `{ok, cycle, billing_day, next_reset, days_left, users:{uid: bytes}}`，
     值来自 `user_used_bytes`（与 `/clash` 的 `download`、CONNECT 熔断判据**同一个 map**）
   - 鉴权抽成 `admin_authorized()`（口令为空 fail-closed + 固定时间比较），`handle_revoke` 同步改用，
     消除两处各写一遍口令比较的分叉风险
2. **`POST /api/admin/reset-usage`（网关）**：`{"uid":"usr_..."}` → 删除账本条目 + 立即 `persist()`
   （跳过 10s 周期窗口，否则进程重启会把内存旧值写回），响应体回 `before` 字节数；
   清理前先 append 一行审计 `~/.pony/usage_resets.log`（`ts= uid= before=`）——
   清零没有回滚路径，计费数据不能静默消失。`uid` 走 `[A-Za-z0-9._-]{1,128}` 白名单（该值会进入远端 shell 的 JSON body）
3. **`pproxy user usage [username]`（CLI）**：本机 `users` 表（配额/到期/启用态）× 网关账本
   （已用）合并成一张表：`quota / used / remaining / usage% / expires_at / state`，
   `state ∈ {正常, 额度耗尽, 令牌过期, 未开会员, disabled}`（按此优先级判定）。
   账本里有、用户表里没有的租户显示 `<name> (无本机记录)`——历史令牌或删号残留必须现形，
   否则那部分流量在报表里"消失"
4. **`user rm` 双级联**：撤销（断网）+ 清零（账目归零），任一失败返回 `EXIT_FAILURE`。
   `disable` **不**清零（临时停用要保留当期用量，否则恢复即等于送额度）

## Alternatives considered

- **CLI 直接 `ssh` 读 `gate_usage.json` 文件**：不新增端点、零网关改动；但账本是**内存真源 + 10s 刷盘**，
  文件值最多滞后 10s，且直接读文件绕过了权限面（谁有 SSH 谁就能读全量）。更要命的是清零必须"停服务→
  改文件→启服务"，否则被 SIGTERM 回写覆盖（2026-11-12 已踩过一次）。否决。
- **复用既有 `/api/user/profile`（用户令牌鉴权）查他人用量**：那是终端用户视角的端点，
  只返回自己；让管理员持用户令牌去查没有语义，也不该为了运维便利去签长期管理员令牌。否决。
- **把用量写进 `users` 表，让 CLI 只读本地 DB**：单一事实源最理想，但账本在网关 VPS、DB 在管理机，
  网关是刻意无状态可验签的（无 DB 通道，见 B061 的同款权衡）；每 10s 同步一次会引入新的漂移面。否决。
- **`user rm --purge-usage` 显式开关**：默认安全；但「删号留账本」没有任何合法场景——
  唯一效果是让下一位同名用户继承消耗。默认清零 + 返回非零更符合最小惊讶。否决。
- **清零时按周期归档旧值（`gate_usage_<cycle>.json` 增量）**：审计更完整，但归档文件名按周期唯一，
  同期多次清零会互相覆盖；改成 append-only 审计行（当前方案）语义更简单，也够事后对账。否决。
- **`disable` 也清零**：语义上"暂停"不该没收已用额度；恢复后应是原额度的剩余部分。否决。

## Consequences

- `pproxy user usage` 成为用量的唯一查询入口，`state` 直接回答"为什么他连不上"（额度耗尽 / 令牌过期 /
  未开会员），不必再看日志。
- 删号语义闭合：`users` 行、令牌（jti 黑名单）、用量账本三者同时归零；同名重建从 0 起算（实测 `used 0 B`）。
- 计费可审计：每次清零在网关留 `usage_resets.log` 一行，含清零前字节数。
- 管理通道扩成"通用管理调用"（`admin_post(path, body) -> (status, body)`），撤销与用量共用一套
  口令/传输/超时逻辑；`push_revoke` 降为其上的薄封装（行为与退出码不变）。
- 边界（靠 review / 后续条目）：
  - 清零对**在途连接**不做打断，之后仍会计入的新字节是真实流量（`reset` 不是"冻结账单"）；
  - `lease_bytes=500MB` 仍硬编码且网关运行时零引用（B063 residual → B064）；
  - 生产网关升级才使端点生效（本次已部署，sha256 `6bda1275…`）。
