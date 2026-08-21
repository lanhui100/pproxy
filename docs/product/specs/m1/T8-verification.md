# T8 — 回归验证（scripts/m1_test.sh）

> 依赖: T3 + T4 + T6 全部合入 | 最后串行 | 上游: M1 spec §5 集成测试 8 步 + 验收标准

## 1. 目标

集成测试脚本覆盖 M1 spec §5 全部 8 步 + 7 路由迁移回归断言 + M1 审核裁决新增断言（CONNECT→403、x-pony-token 不泄露、迁移后重启持久化）；**全程不触碰生产**（生产 systemd pproxy 正在 :8899/:8900 运行）。

**离线门禁子集（C-风险裁决）**：除步骤 4/5 外的全部步骤（0-3、5.5、6-11）**不依赖外网**可独立通过——本地 echo stub（§2）承载泄露断言与离线转发验证；步骤 6 离线降级断言见该步说明。外网不可达时脚本以 `M1_TEST_OFFLINE=1` 跳过 4/5 并标记 SKIP-ONLINE，其余步骤照常作为门禁。

## 2. 文件位置

- 新增 `scripts/m1_test.sh`（bash，`set -euo pipefail`）。
- 新增 `scripts/m1_echo_stub.py`（**必选**，非降级方案）：本机 `127.0.0.1:18901` 的 HTTP 服务，对任何请求返回 200 JSON：`{"url": "<?url= 参数值>", "headers": {<收到的全部请求头小写键值对>}}`——用于 x-pony-token 泄露断言（步骤 5.5）与离线转发链路验证。经临时 config 的 `upstreams.localstub` 条目成为"上游"（`EdgeClient` 直连它，不经过 CF Worker，故本机可达）。

## 3. 隔离原则（硬性）

| 项 | 生产值 | 测试值 |
|----|--------|--------|
| 数据面 | 127.0.0.1:8899 | `PPROXY_LISTEN_DATA=127.0.0.1:18999` |
| 管理面 | 127.0.0.1:8900 | `PPROXY_LISTEN_ADMIN=127.0.0.1:18900` |
| DB | ~/.pony/state.db | `PPROXY_DB=$TMPDIR/state.db` |
| config | /home/USER/pproxy/config.json | 临时目录内构造（见 §4 步骤 0） |
| echo stub | 无 | `127.0.0.1:18901`（m1_echo_stub.py） |

- 测试进程用 `cargo build` 产物 `target/debug/pproxy-server`（或 release，脚本内 `CARGO_BUILD` 变量可切），后台启动。
- **临时目录清理（S-P2-8 裁决）**：`TMPDIR=$(mktemp -d)`；脚本首行设置 `trap 'kill "$SERVER_PID" "$STUB_PID" 2>/dev/null; rm -rf "$TMPDIR"' EXIT`——正常退出、步骤失败 exit 1、Ctrl-C 一律清理临时目录与子进程。
- 脚本启动前断言测试端口未被占用（18999/18900/18901 可绑定即可）。
- 脚本结束打印 PASS/FAIL 汇总；任一步骤失败立即退出非零。

## 4. 步骤（与 M1 spec §5 一一对应，3.5/5.5/11 为裁决新增）

**步骤 0（前置）**：构造临时 config.json（完整旧格式：7 业务路由 + worker_url/worker_secret/upstreams/route_upstreams，内容复制生产 config.json 但 listen_port 改 18999），**追加测试专用项**：`upstreams.localstub = {"url": "http://127.0.0.1:18901", "secret": "test-stub"}`、`routes.echo = "echo.example.com"`、`route_upstreams.echo = "localstub"`（迁移将导入 8 行：7 业务 + echo；echo 的 target_host 为合法域名格式，实际流量经 localstub 上游直达本机 stub）。启动 `m1_echo_stub.py`。`PPROXY_CONFIG` 指向临时 config。

**步骤 1**：启动 server → 重定向 stdout/stderr 到 `$TMPDIR/server.log`，`grep ADMIN_TOKEN` 断言出现且恰一次，提取 `pony_admin_<48hex>` 存变量。

**步骤 2**：`curl -s -o /dev/null -w %{http_code} 127.0.0.1:18999/anthropic/v1/messages` → 断言 401，body 为 `{"error":"unauthorized"}`。

**步骤 3**：`curl -X POST 127.0.0.1:18900/api/tokens -H "Authorization: Bearer $ADMIN" -d '{"name":"test-dev"}'` → 断言 201、token 匹配 `^pony_[0-9a-f]{32}$`；再次同请求 → 400（name 重复）。

**步骤 3.5（CONNECT 禁用，P0-1 裁决新增）**：`curl -sS -o /dev/null -w %{http_code} --proxy http://127.0.0.1:18999 https://example.com --max-time 10` → 断言 **403**（CONNECT 被 gateway 分流拦截，非 200、非隧道建立；离线可测——403 在本机生成，无外网请求）。补充原始字节断言：`printf 'CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n' | timeout 5 nc 127.0.0.1 18999` 输出含 `403` 且连接关闭。

**步骤 4**（**外网依赖**，离线模式 SKIP-ONLINE）：带 token 请求 openai：`curl 127.0.0.1:18999/$TOKEN/openai/v1/models -H "Authorization: Bearer sk-fake"` → 断言 HTTP 401 且 body 含 `invalid_request_error`（真实上游链路验证，走 vercel 出口）。失败时输出明确提示"检查上游可达性"，不静默重试。

**步骤 5**（**外网依赖**，离线模式 SKIP-ONLINE）：带 token 请求 zen（opencode 路由，vercel 出口）——**tech-lead 2026-08-21 经生产网关实测写死**：

```bash
curl -sS --max-time 30 -X POST "127.0.0.1:18999/$TOKEN/opencode/zen/v1/chat/completions" \
  -H "Content-Type: application/json" \
  -d '{"model":"zen","messages":[{"role":"user","content":"ping"}]}'
```

断言 body 含 `CreditsError` **或** `DataPolicyError`（均为上游业务错误 = 链路通；当前 key 余额不足返回 CreditsError，已实测确认）。

**步骤 5.5（x-pony-token 泄露断言，S-P1-1 裁决新增）**：**二选一裁决：采用本地 stub 回显方案**（方案 A）——header 模式请求 echo 路由：`curl -sS 127.0.0.1:18999/echo/ping -H "X-Pony-Token: $TOKEN"` → stub 返回 200 JSON，断言其 `headers` 对象**不含 `x-pony-token` 键**（端到端证明剥离，含 EdgeClient 转发路径）。不采用"经真实上游 401 间接验证"（方案 B 无法区分"header 被剥离"与"上游忽略未知 header"，证明力不足）。此步骤同时验证 header 鉴权模式 + 离线转发链路（localstub 上游）。

**步骤 6**：`curl 127.0.0.1:18900/api/usage?hours=1 -H "Authorization: Bearer $ADMIN"` → 断言响应含 `hours`/`since_hour`/`rows`/`total` 字段且 rows 元素**无 `ts_hour` 键**（C-P2-10）；`hours=0` → 400、`hours=1000` → 400。数据断言分模式：**离线模式**断言 rows 含 route=echo 且 requests≥1（步骤 5.5 流量，本地可复现）；**在线模式**追加断言 rows 含 route=openai 与 route=opencode 的 requests≥1（bytes_in>0；bytes_out 仅断言字段存在，不设下限）。

**步骤 7**：`POST /api/routes {"name":"m1test","target_host":"api.anthropic.com"}` → 201；立即 `curl 127.0.0.1:18999/$TOKEN/m1test/v1/messages` → 非 404（在线：上游 401/405/400；离线：502 upstream_error——均证明路由已生效且进入转发）；`DELETE /api/routes/m1test` → 200；再次请求 → 404 `{"error":"unknown_route"}`。

**步骤 8**：`DELETE /api/tokens/<id>` → 200；`curl 127.0.0.1:18999/$TOKEN/echo/ping` → 401（撤销生效；用 echo 路由使离线模式同样可断言）。

**步骤 9（迁移回归）**：迁移后检查临时 DB（**C-P2-7 裁决：统一用 python3 sqlite3 模块**，不依赖 sqlite3 CLI）：

```bash
python3 -c "import sqlite3; c=sqlite3.connect('$PPROXY_DB'); print('\n'.join(f'{n} {u}' for n,u in c.execute('select name,upstream from routes')))"
```

→ 恰 8 行：7 业务路由中 openai/opencode 的 upstream=vercel、其余 5 个 worker；echo 行 upstream=localstub。`config.json` 已被重写：**含 `routes` 键为否**、`worker_url`/`worker_secret`/`upstreams`/`route_upstreams` 仍在（P0-2）、新增 `db_path`；`.bak` 存在且内容为原始（含 routes 键）。对 7 业务路由各发一次无 token 请求 → 全部 401（证明路由表加载完整，鉴权先于路由）。

**步骤 10（旧端点下线断言）**：`curl 127.0.0.1:18900/stats` → 非 200（401/404 均接受——中间件覆盖全 Router）。

**步骤 11（迁移后重启持久化，P0-2/C-风险 裁决新增）**：kill server → **同一 `PPROXY_DB` + 已重写的临时 config** 重启（步骤 8 已撤销旧 token，此处经管理 API 重新 create 一个 token）→ 断言：① 新日志**无**第二条 `ADMIN_TOKEN`（C-风险）；② `config.json` 未被再次改写（仍无 `routes` 键、`.bak` 内容不变——`AlreadyMigrated` 幂等）；③ 新 token 请求 openai：在线模式断言上游 401 `invalid_request_error`（转发仍通，凭据从重写后 config 完整恢复）；离线模式断言 502 `upstream_error`（非 401/404/503——证明 token 校验、路由解析、EdgeClient 构造全链路正常，仅外网不可达）；④ `curl $TOKEN/echo/ping`（新 token）→ 200（localstub 链路重启后仍通，离线亦可断言）。

## 5. 断言实现约定

- 统一 helper：`expect_eq actual expected label` / `expect_contains haystack needle label`，失败打印 label + 实际值后 `exit 1`。
- curl 一律 `-sS --max-time 30`。
- ADMIN_TOKEN 提取：`grep -oP 'pony_admin_[0-9a-f]{48}' "$LOG" | head -1`。
- DB 断言一律 python3 sqlite3 模块（C-P2-7），脚本内封装 `db_query "SQL"` helper。
- 脚本可重复执行（幂等）：每次全新 mktemp 目录，EXIT trap 清理（S-P2-8）。
- 离线开关：`M1_TEST_OFFLINE=1` 时步骤 4/5 打印 SKIP-ONLINE 跳过；未设置时若步骤 4 失败，提示"外网不可达时可加 M1_TEST_OFFLINE=1 跑离线门禁子集"。

## 6. 依赖任务

T3（数据面 + 环境变量覆盖 + CONNECT 403）、T4（路由迁移 + 热生效）、T6（管理 API）、T1（迁移导入 + 重写保留凭据）、T2（token）。

## 7. 单元测试清单

脚本本身无单测；等价物为脚本自检：`bash -n scripts/m1_test.sh` 语法检查纳入验收；脚本内步骤编号与本 §4 一一对应（审查对照表）。

## 8. 验收标准

- `bash scripts/m1_test.sh` 全绿退出 0，输出各步骤 PASS；`M1_TEST_OFFLINE=1 bash scripts/m1_test.sh` 在无外网环境同样退出 0（步骤 4/5 SKIP-ONLINE）。
- 生产服务不受影响：脚本运行前后 `systemctl is-active pproxy` 均 active，且 `curl 127.0.0.1:8899/` 行为不变（脚本首尾各断言一次）。
- M1 spec §5 步骤 1-8 逐条覆盖（审查对照）；§5 验收标准 4 条全部满足：
  1. curl 带 token 走通 openai/zen（步骤 4/5，在线模式）
  2. 无 token 401（步骤 2）
  3. /api/routes 增删即时生效（步骤 7）
  4. 7 路由自动迁移行为一致（步骤 9）
- 裁决新增断言全部在位：CONNECT→403（3.5）、x-pony-token 不泄露（5.5）、迁移后重启转发仍通且不重复打印 admin（11）、临时目录 trap 清理（§3）。
- 归档联动：脚本通过后，按 M1 spec §8 更新 docs/ops/API.md（管理面协议 + /stats /refresh 移除说明 + admin 恢复路径 + PPROXY_ADMIN_TOKEN 注入说明 + journal 清理提示）与 README（`/{token}/{route}/...` 新协议）。
