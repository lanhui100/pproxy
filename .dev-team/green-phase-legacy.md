# GREEN PHASE — legacy 缺陷最终机器验收（machine record）

- 日期：2026-10-08
- 执行人：`legacy-qa`（独立 Test Agent，验收期间未触碰任何 src 代码）
- 前置：Lead 于 2026-10-08 发出【A/B/C 全部完成，L3 双路审查闭环】信号，放行 T2 绿相
- 二进制：`./target/release/pproxy`（Oct 8 11:07 构建，晚于全部 src 修改 ≤11:02，含本波次 CLI 修复）

## 1) 机器验收脚本（A/B 全 PASS，C 按 deploy-deferred 跳过）

命令：`PPROXY_BIN=./target/release/pproxy bash scripts/accept-legacy-items.sh`

```
== A) pproxy doctor CONNECT tunnel probe ==
A PASS: [pass] CONNECT tunnel probe: oauth2.googleapis.com:443 → 200
== B) pproxy status systemd ==
B PASS: systemd (pproxy [system]): active
C SKIP: [RED DEGRADED: DEPLOY-DEFERRED] 未带 --with-deploy（worker.js 标记头 + 凭据重部署未完成，跳过）
ACCEPT_EXIT=0
```

- A（item 1，CONNECT 探针缺 Host 头）：FAIL→PASS，`→ 200` 达成。
- B（item 3，systemd 单位名漂移）：FAIL→PASS，`systemd (pproxy [system]): active` 与断言格式完全一致。
- C（item 2，worker 路由标记头 + 凭据重部署）：deploy-deferred 显式跳过（无 CF 凭据，重部署不可行）。

## 2) 冻结集成测试（item 4，VERCEL_HOSTS 死回退）

命令：`cargo test -p pproxy-core --test route_default_upstream`

```
running 2 tests
test explicit_override_still_wins ... ok
test default_upstream_falls_back_to_worker ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
INTEGRATION_EXIT=0
```

- 无 override 默认 Worker（api.openai.com / opencode.ai）PASS；显式 `Some("vercel")` 仍解析 Vercel PASS。

## 3) 回归：pproxy-core 全量测试

命令：`cargo test -p pproxy-core`

```
test result: ok. 106 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.86s
     Running tests/route_default_upstream.rs ...
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
   Doc-tests pproxy_core
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
CORE_SUITE_EXIT=0
```

（1 个既有 warning：`crates/core/src/cluster.rs:106 unused_mut`，属仓库存量，与本波次修复无关，验收不改源。）

## 4) 编译门禁

命令：`cargo build --release`

```
Finished `release` profile [optimized] target(s) in 0.15s
RELEASE_BUILD_EXIT=0
```

（增量无变更 → 0.15s；二进制 Oct 8 11:07 已含全部波次修复，工程解链。）

## 结论

四项验收全绿（exit code 均为 0）。C 项维持 deploy-deferred：修复代码（worker.js 标记头 + DEPLOY.md SOP）已落盘，待有 CF 凭据重部署后以 `scripts/accept-legacy-items.sh --with-deploy` 复验。