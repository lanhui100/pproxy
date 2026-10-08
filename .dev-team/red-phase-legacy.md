# RED PHASE — legacy 缺陷验收测试冻结（machine record）

- 日期：2026-10-08
- 执行人：`legacy-qa`（独立 Test Agent，未触碰任何 src 代码）
- 任务：pproxy 仓库 `/home/dm/pproxy`（Rust workspace）legacy 三缺陷红相测试冻结

## 冻结的测试交付物（本文件写域：crates/core/tests/、scripts/、.dev-team/）

| 文件 | 用途 |
|---|---|
| `crates/core/tests/route_default_upstream.rs` | 集成测试：item 4（VERCEL_HOSTS 死回退）契约 = 无 override 默认 Worker、显式 override 仍生效 |
| `scripts/accept-legacy-items.sh` | 机器验收：A=doctor CONNECT 探针 200；B=status system 级 active；C=部署后路由 test（deploy-deferred 跳过） |
| `.dev-team/red-phase-legacy.md` | 本记录 |

## 红相执行输出（机器证据原文）

### A) `bash scripts/accept-legacy-items.sh`（无参数）→ EXIT=1

```
== A) pproxy doctor CONNECT tunnel probe ==
[fail] CONNECT tunnel probe: oauth2.googleapis.com:443 → HTTP 400: HTTP/1.1 400 Bad Request
ACCEPT-FAIL: A: CONNECT 探针未达 [pass] ... → 200（当前为 FAIL）
```

→ A 断言 FAIL（exit 1）：缺 Host 头导致网关回 400。

### B) `pproxy status` 的 systemd 行 + B 断言独立验证 → FAIL

```
systemd (pproxy-server [user]): inactive
```

B 断言（`grep -E '^systemd \((pproxy|pproxy-server) \[system\]\): active$'`）退出码 = 1（FAIL）：
系统级 pproxy.service 实为 active（`systemctl is-active pproxy.service` = active，enabled），
但旧 status 只查 `--user` 作用域 → 误报 inactive（item 3：systemd 单位名漂移）。

### C) 部署后路由 test → 显式跳过（deploy-deferred）

脚本无 `--with-deploy` 时打印：
`C SKIP: [RED DEGRADED: DEPLOY-DEFERRED] 未带 --with-deploy（worker.js 标记头 + 凭据重部署未完成，跳过）`
本环境无 CF 凭据，重部署不可行（deploy-deferred），C 不执行。

## 集成测试红相运行结果（需如实说明）

`cargo test -p pproxy-core --test route_default_upstream` → **PASS（2/2）**：

```
running 2 tests
test default_upstream_falls_back_to_worker ... ok
test explicit_override_still_wins ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
EXIT=0
```

**为什么红相不红（重要发现，非伪造）**：`git status` 显示 Executor 的实现**已落在工作区**
（未提交，git 写主 = Lead）：

```
 M crates/cli/src/cmd/doctor.rs   ← item 1：connect_request() 已加 Host 头（含单测）
 M crates/cli/src/cmd/service.rs  ← item 3：非 root 补查 system 作用域
 M crates/cli/src/cmd/upgrade.rs  ← item 3 同族：upgrade 守护检测补查 system 作用域
 M crates/core/src/route.rs       ← item 4：VERCEL_HOSTS 已移除，默认恒 Worker
?? .dev-team/red-evidence/
?? crates/core/tests/
?? scripts/accept-legacy-items.sh
```

因此 **源码级 item 4 已是绿相**（pick_upstream 无 override 返回 Worker，显式 "vercel" 仍 Vercel），
冻结测试即回归门禁，当前直接通过。二进制级 A/B 仍红，是因为 `~/.local/bin/pproxy` 是
**旧版已装二进制**（未 rebuild/install），并非源码缺陷。

## 关键状态结论（发 Lead）

1. 冻结测试就位；A/B 脚本断言当前红相成立（A=400，B=inactive，均 exit 1 验证）。
2. 源码级修复已在工作区（未提交）；item 4 集成测试已绿（回归门禁生效）。
3. 二进制级 A/B 需**重建并安装新 pproxy 二进制**（Executor/Lead 步骤）后才会转绿；
   C 维持 deploy-deferred（无 CF 凭据）。
4. 待 Lead 确认后进入绿相：回归 `cargo test -p pproxy-core` + `bash scripts/accept-legacy-items.sh` + `cargo build --release`。