# Agent Note: saturation_contract 日志解析误读 ANSI 转义码

Status: implemented

## Problem

`crates/server/tests/saturation_contract.rs` 的
`over_limit_fast_fail_and_recovery_contract` 自 2026-10-09 引入起从未在 CI 通过：
`captured_sem_capacity()` 返回 `Some(0)`，断言期望 `Some(2)`。

真实原因不在实现。`gateway.rs` 的 fast-fail 日志确实输出了 `sem_capacity=2`，
但测试的 `install_log_capture()` 没有关闭 ANSI，捕获缓冲区里字段名后的字节是：

```
sem_capacity\x1b[0m\x1b[2m=\x1b[0m\x1b[2m2
             ↑
```

`captured_sem_capacity()` 用 `position(|c| c.is_ascii_digit())` 找"字段名后的第一个
数字"，命中的是 `\x1b[0m` 中的 `0`，于是解析出 `0`。

影响：CI 从 2026-10-10 起连续 9 次失败，且每次只有这一个测试失败；最后一次绿色
CI 是 2026-10-09T08:00Z，早于测试提交（13:56Z），所以该测试**从未绿过**。

## Decision

在 `install_log_capture()` 的 tracing subscriber 上显式 `.with_ansi(false)`，
并同步删除断言里过期的"红相判据：当前实现无饱和 fast-fail 事件日志"提示
（实现早已落地，该提示会把人引向错误方向）。

断言强度不变：仍然要求日志中 `sem_capacity == INJECTED_LIMIT`，只是让解析器读到
真正的数字。

## Alternatives considered

- **只加 `NO_COLOR=1` 到 CI**：能让流水线变绿，但本地 `cargo test` 仍红，且掩盖
  同类解析缺陷；不是修复。
- **让 `captured_sem_capacity()` 先剥离 ANSI 再扫描**：可行，但每个字段解析都要
  重复处理；在写入端关闭着色更彻底。
- **改实现去适配解析器（例如去掉 `=` 两侧空格）**：ANSI 序列与格式无关，改了也
  没用；且让实现为测试缺陷让路，方向错误。
- **修改断言期望值为 0**：直接削弱契约，禁止。

## Consequences

- `cargo test --workspace` 恢复绿色；`saturation_contract` 首次真正验证 env 注入。
- 测试文件头部的"当前实现必须红"叙述已改为当前态描述；契约原文
  `.dev-team/contracts/2026-10-09-pproxy-accept-loop.md` 未随仓库提交，契约数值
  内联在测试常量中，文件头已注明。
- 机械验收：`NO_COLOR=1 cargo test -p pproxy-server --test saturation_contract`
  与不带 `NO_COLOR` 的同一命令均通过（后者证明修复不依赖外部环境）。
