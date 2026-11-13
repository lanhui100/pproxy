# Agent Note: CLI 测试的 HOME 竞态——并行测试共享进程级环境变量

Status: implemented

## Problem

2026-11-13，PR #5 的 CI `server-tests` 失败在一条与该提交无关的用例上：

```
thread 'cmd::sync::tests::sync_roundtrip_url_safe_and_replay_protection' panicked at
crates/cli/src/cmd/sync.rs:426:9: assertion failed: res1.is_ok()
```

该提交只改 `cmd/user.rs` 的展示层与 `main.rs` 的参数声明，`sync` 一字未动；本地
`cargo test`（含 `--test-threads=16`、全新 HOME）连续 12 轮不复现，说明这是**依赖线程交错**的竞态，
不是逻辑回归。

根因是进程级全局状态被并行测试共享：`cmd/proxy_env.rs` 的测试用
`std::env::set_var("HOME", tmp_dir)` 改写 **进程级** `HOME`，而 Rust 测试默认在同一进程内并行
（CI 2 核的交错顺序与本地不同）。`import_internal` 的落盘路径全部由 `HOME` 派生——
`~/.pony/config.toml`（`config_path()`）与 `~/.pony/state.db`（`default_db_path()`）——
在改写窗口内，并发用例可能读到**另一个**临时目录，于是首次导入的落盘结果与预期不一致，
断言只留下 `is_ok()` 一句，看不出是哪一步错了。

原有防护不足：`proxy_env` 模块内确有 `HOME_LOCK`，但它只互斥"改写者之间"，
**不挡读者**；跨模块的读者（`sync` / `config` / `store`）完全在锁外。守的对象错了。

## Decision

把 `HOME` 的串行化提到 crate 级单一出口 `crates/cli/src/test_support.rs`
（`#[cfg(test)]` 挂载于 `main.rs`）：

1. `with_home(dir, f)`：全局 `Mutex` 内设置 `HOME`，结束后恢复原值（含"原本没有 `HOME`"的情形），
   锁中毒时 `into_inner()` 继续——测试辅助不该因为别处的 panic 变成二次失败。
2. `with_temp_home(f)`：临时目录 + 独占窗口，供"需要真实落盘但不得污染真实 HOME"的用例。
3. `proxy_env` 删除私有的 `HOME_LOCK`/`with_home`，改用共享件，模块语义不变；
   `sync_roundtrip_url_safe_and_replay_protection` 把两次 `import_internal` 放进 `with_temp_home`，
   并把首条断言改成携带错误文本，下同类失败能在 CI 日志里直接看到原因而不是一句 `is_ok()`。

## Alternatives considered

- **CI 加 `--test-threads=1`**：一行修好所有竞态，但把并行测试的收益全部丢掉（CI 时长翻倍），
  更糟的是把"测试依赖进程级环境变量"这一结构问题合法化，本地跑法仍是并行的——行为分叉，否决。
- **给该用例加重试（失败后 sleep 再试 N 次）**：把不确定性变成统计，CI 会偶发变慢且永不暴露真实缺口，否决。
- **只在 `sync` 用例里加一把自己的锁**：挡不住 `proxy_env` 的改写（锁不是同一把），等于没修，否决。
- **只让 `sync` 用例不落盘（把 `import_internal` 里的 config/db 路径参数化）**：语义最好，
  但要穿透 CLI 调用链改签名，超出"修一条 CI 红"的最小闭包；已写入 Consequences 作为后续选项。
- **`serial_test` 之类的 dev-dependency**：引入第三方依赖解决 5 行辅助函数的问题，
  与"零依赖优先"的项目惯例相悖，否决。

## Consequences

- 竞态消除的机器证据：`cwd=crates/cli` + 全新 `HOME` + `--test-threads=16`，**12 轮 0 失败**；
  `cargo test -p pproxy-cli` → `138 passed; 0 failed`。
- 用例副作用不再落到真实 `HOME`：`sync` 用例的 `config.toml` 与 `state.db` 全部写在 `tempdir` 内，
  跑测试不再改写开发者本机配置——这是比"修好 CI"更实际的收益。
- **边界（靠 review）**：共享锁只能保护"经 `with_home` 改写"的路径；
  仍存在直接读 `HOME` 而不加锁的用例，约定已写进模块文档。
  若要彻底封闭，需把 `HOME` 依赖改为显式参数注入（上条备选），本轮不做。
- 同批仍未收口的相邻事实：`cmd::deploy::tests::resolve_proxy_secret_missing_error` 依赖
  `cwd` 下是否存在 `.secrets.env`（`cargo test` 的 cwd 是包根，故 CI 通过、手工跑二进制会红）；
  与本次竞态同源（隐式环境依赖），不在本轮删除范围内，留待后续。
