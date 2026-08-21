# M2 任务级 Specs — CLI

> 上游: [ROADMAP M2](../../ROADMAP.md) + [TECH_DESIGN §2.2](../../../TECH_DESIGN.md) | 状态: 已审核（2026-08-21 对照 API.md 与 M1 实现修正） | 日期: 2026-08-21
>
> M2 单任务单 crate，无并行波次；本 README 即完整 spec（无 T 编号拆分）。
>
> 审核裁决摘要：R1 删除虚构的 test-upstream 端点（服务端无此 API，CLI 不发明）；R2 reqwest 带 rustls-tls 支持远程 https server；R3 export 改 `--route/--token` 双参数（`--token-name` 无法反查明文属伪需求）；R4 集成测试全隔离（临时 HOME+随机端口），不碰生产 pproxy；R5 时间戳本地化在 CLI 层做（不违反 core 无 chrono 约定）；R6 `--upstream` 值域含 F8 Named 绑定（透传服务端校验）。

## 1. 目标与范围

`pony` CLI（Rust + clap，同 workspace 新增 `crates/cli`）：管理 API 的命令行客户端。**纯客户端**——不直连 SQLite、不含业务逻辑，一切状态经管理面 REST API（:8900）读写。

服务端契约以 [docs/ops/API.md](../../../ops/API.md) 为准（M1 已实现：admin Bearer 中间件全端点鉴权、错误体固定文案、PATCH double_option 三态）。CLI 不做任何本地状态缓存——每次调用即真实读 API，无失效一致性问题。

验收场景（ROADMAP 原文）：纯 CLI 完成添加新服务（如 Gemini）→ 生成 token → 导出配置 → doctor 通过。

## 2. crate 结构（高内聚低耦合）

```
crates/cli/
  Cargo.toml        # bin name = "pony"
  src/
    main.rs         # 入口：解析 → 执行 → 退出码；禁止业务逻辑
    config.rs       # ~/.pony/config.toml 读写 + admin token 存取
    client.rs       # AdminClient：reqwest 封装全部 /api/* 调用（唯一 HTTP 出口）
    cmd/
      mod.rs        # 命令分发
      service.rs    # status / start / stop / restart
      route.rs      # route list/add/rm/test/enable|disable
      token.rs      # token create/list/revoke
      usage.rs      # usage 报表渲染
      doctor.rs     # 全路由体检
      export.rs     # config export <service>
```

依赖：`clap = { version = "4", features = ["derive"] }`、`toml = "0.8"`、workspace 复用 `serde/serde_json/anyhow/futures`；reqwest 各 crate 自行声明（不在 workspace.dependencies）：CLI 用 `reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }`——带 TLS 使 `--server https://…` 远程配置不致运行时才炸（R2）。目录 `dirs` 不引入：home 目录用 `std::env::var("HOME")` 兜底 `~` 展开。

## 3. 连接配置（config.rs）

路径 `$HOME/.pony/config.toml`（HOME 缺失时回落 `/root` 或报错退出 2——取 `env::var("HOME")`，无则用当前用户 home 目录推导失败即退出 2）：

```toml
server = "http://127.0.0.1:8900"   # 管理面 base URL
admin_token = "pony_admin_..."      # 明文存本地，文件权限 0600
# 可选覆盖（缺省用 server 推导 :8899）
data_plane = "http://127.0.0.1:8899"
```

- 首次运行任何命令（除 init）若无此文件 → stderr 提示引导 + 退出码 2：
  `config not found: ~/.pony/config.toml — run 'pony init --server <url> --token <admin_token>'`
- `pony init` 子命令：写入 config.toml（已存在则拒绝，`--force` 覆盖）；创建后 `chmod 600`
- admin token 来源优先级：`--token` 参数 > `PONY_ADMIN_TOKEN` 环境变量 > config.toml（env 注入避免落盘；`--token` 仅 init 与全局 `--token` flag 生效处使用）
- **安全**：config.toml 权限校验——读到的模式含 group/other 位时 warn（不阻断）；错误输出永不回显 token 明文（只显示前 10 位 + …）
- data_plane 推导规则：显式配置优先；否则取 server URL 的 scheme+host、端口替换为 8899（非 http/https scheme 或无法解析 → 退出码 2）

## 4. 命令规格（精确到行为）

通用约定：所有 API 调用失败 → stderr 人话错误（HTTP 状态 + body error 字段，body 非 JSON 时显示原文截断 200 字节）+ 退出码 1；连接失败（connect/timeout 类 reqwest 错误）→ 提示 `pony-server 未运行? (systemctl status pproxy)` + 退出码 3。表格输出用简单对齐（列宽取各行最大宽），不引第三方 table 库。全局 flag `--token <t>` 覆盖 admin token 来源（优先级最高）；`--server <url>` 覆盖 config 的 server。

### 4.1 pony init
`pony init --server <url> [--token <t>] [--force]`
写 `~/.pony/config.toml`；`--token` 缺省提示从 `PONY_ADMIN_TOKEN` 或首启日志获取。成功后打印文件路径。

### 4.2 服务开关（service.rs）— 仅本机可用
- `pony status`：GET /api/health → 渲染 status/db/tokens_active/routes 表；追加本机 systemd 状态（`systemctl is-active pproxy` 子进程，非 Linux 或 systemctl 不存在则跳过该行）
- `pony start|stop|restart`：仅封装 `sudo systemctl start|stop|restart pproxy`（子进程透传 exit code；远程模式 P2 再评估，当前直接报"仅支持本机"；非 Linux 同样直接报错退出 1）

### 4.3 路由（route.rs）
- `pony route list`：GET /api/routes → 表格 name/target_host/effective_upstream/override_upstream/enabled/created_at（enabled=false 标记 `[disabled]` 后缀；时间戳渲染为本地时区 `%Y-%m-%d %H:%M`——格式化在 CLI 做，core 无 chrono 约定不受影响）
- `pony route add <name> <target_host> [--upstream <u>]`：POST /api/routes（`--upstream` 缺省不传 override 字段；值域 worker|vercel|已配置上游名由服务端校验，CLI 原样透传）；成功打印 201 + 响应 upstream 决策结果（用户可立刻看到自动选择 vs override）
- `pony route rm <name>`：DELETE /api/routes/{name}；200 `{deleted:true}` 打印已删除，404 时报错退出 1（不做交互确认——个人工具，rm 语义即删）
- `pony route test <name> [--all]`：
  - 单路由：POST /api/routes/{name}/test → 打印 ok/status/latency_ms/error
  - `--all`：GET /api/routes 取 enabled 列表 → 并发 POST 各自 test（futures join_all），逐行打印；任一 ok=false 最终退出码 1
- `pony route enable|disable <name>`：PATCH /api/routes/{name} `{"enabled": true|false}`

### 4.4 token（token.rs）
- `pony token create <name> [--expires-days N]`：POST /api/tokens → **明文一次性高亮打印** + 提示"仅此一次，请立即保存"；同时打印可直接粘贴的 base_url 片段 `{data_plane}/{token}`
- `pony token list`：GET /api/tokens → 表格 id/name/status/created_at/expires_at/last_used_at（时间戳本地化同 §4.3）
- `pony token revoke <id>`：DELETE /api/tokens/{id}；admin 行被拒（400 cannot revoke admin）原样透传提示

### 4.5 用量（usage.rs）
`pony usage [--hours 24] [--route <name>] [--token-id <id>]`
GET /api/usage（hours 上限 720 由服务端校验，CLI 透传 400 错误）；渲染 rows 表格 + total 汇总行；字节人性化（KB/MB/GB）。

### 4.6 doctor（doctor.rs）
`pony doctor [--probe-token <t>]`
顺序执行、汇总报告，任一环节失败不中断后续环节：
1. GET /api/health → status/db
2. GET /api/routes → 路由数、disabled 数
3. 对全部 enabled 路由并发 POST test（同 route test --all；无 enabled 路由记 SKIP）
4. 数据面抽样：有 `--probe-token <t>` 且存在 enabled 路由时，发 `GET {data_plane}/{probe_token}/{首条 enabled 路由}/`，按 HTTP 状态判 pass/fail（401 也算 fail——token 无效）；否则注明 SKIP
输出末尾汇总：`X passed, Y failed, Z skipped`；failed>0 退出码 1。

### 4.7 配置导出（export.rs）
`pony config export <service> [--route <name>] [--token <plaintext>]`

内置模板表（service → 路由名/env 前缀）：

| service | route 名（默认=service 名） | env 前缀 |
|---------|-------------|----------|
| anthropic | anthropic | ANTHROPIC_ |
| openai | openai | OPENAI_ |
| opencode | opencode | OPENCODE_ |
| google/github/x/facebook | 同名 | （无标准 env 约定，仅输出 BASE_URL 注释示例） |

行为：
- `--route`：路由名缺省等于 service 名；显式给定则用之（服务名与路由名不一致的场景）
- token 明文来源两种模式：
  - 默认：打印占位 `<your-pony-token-here>` + 创建命令提示 `pony token create <name>`（管理 API 列表无明文、无法反查——安全裁决，见 §6）
  - `--token <明文>`：显式传入嵌入输出（用户从创建时的保存处取）
- 输出格式：shell env 片段（`export X=...`），stdout only；示例：
  ```
  export ANTHROPIC_BASE_URL=http://127.0.0.1:8899/<pony_xxx>/anthropic
  export ANTHROPIC_API_KEY=<your-upstream-key>
  ```
- service 不在模板表 → stderr 列出可用项退出码 1

## 5. 退出码契约

| 码 | 含义 |
|----|------|
| 0 | 成功 |
| 1 | 操作失败（API 4xx/5xx、路由测试有 fail、service 不在模板表） |
| 2 | 本地配置缺失/非法 |
| 3 | 管理面不可达 |

clap 参数错误自身退出 2（clap 默认），与本契约 2 冲突可接受（同为"本地输入问题"类）。

## 6. 安全约束（强制）

- token/admin token 明文只在两个时刻出现：init 写入、token create 打印；其余任何输出（list/export/doctor/error）不得回显完整明文
- export 的 `--token` 显式传入属用户主动行为，允许嵌入输出（输出前 stderr 一行提示"明文已嵌入输出，注意终端记录"）
- config.toml 0600；client.rs 的 reqwest 不跟随非同 host 重定向（redirect policy limited 到同 host）
- 所有请求 15s 超时；route test/doctor 因服务端内部有 10s 探测超时，CLI 侧对 test 类调用放宽到 30s
- `pony status` 等命令的 systemd 子进程调用不透传 shell（直接 Command::new("systemctl")，无注入面）

## 7. 测试清单

单元测试（crates/cli/src 内 `#[cfg(test)]`；渲染/格式化函数为纯函数直接测，不引 HTTP mock 库）：
1. config 解析：合法 toml、缺 server、缺 admin_token、权限位检测 warn
2. 渲染函数：tokens/routes/usage 表格对齐、字节人性化（1023B→1023B、1024B→1.0 KB）、status 派生标记
3. export 模板：anthropic/openai 正确输出、未知 service 列出可用项、`<your-pony-token-here>` 占位
4. 错误分类：reqwest 状态码→退出码 1、连接拒绝→3 的映射纯函数测试

集成验证（scripts/m2_test.sh，仿 m1_test.sh 模式：临时 HOME + 随机端口 + 环境变量注入，不碰生产状态）：
- 临时目录作 HOME（隔离 `~/.pony/config.toml`）→ 启动临时 server（随机 PPROXY_LISTEN_DATA/PPROXY_LISTEN_ADMIN + PPROXY_DB/PPROXY_CONFIG 指向临时路径）+ echo stub
- 流程断言（逐步校验退出码与输出子串）：
  1. 无配置时 `pony status` → 退出码 2 且 stderr 含 "config not found"
  2. `pony init --server http://127.0.0.1:$ADMIN_PORT --token $ADMIN_TOKEN` → 退出码 0，config.toml 权限 600
  3. `pony status` → 退出码 0，输出含 routes/tokens_active
  4. `pony route add gemini generativelanguage.googleapis.com` → 201 输出含 effective upstream 决策
  5. `pony route list` → 含 gemini 行
  6. echo stub 路由 `pony route add stub <stub_host> --upstream localstub`（测试 config 需带 `upstreams.localstub` 段，同 m1_test.sh:80；`--upstream` 传已配置上游名走 F8 Named 绑定语义）→ `pony route test stub` → ok=true
  7. `pony route disable stub` → `route test --all` 不再包含 stub 行；`route enable stub` 恢复
  8. `pony token create ci-token` → 明文捕获（正则 `pony_[0-9a-f]{32}`）→ 用该明文对数据面 `/{token}/stub/` curl 断言非 401（数据面链路闭环）
  9. `pony token list` → 含 ci-token active
  10. 数据面发几个请求后 `pony usage --hours 1` → total.requests ≥ 已发数
  11. `pony config export openai --token <捕获的明文>` → 输出含 `export OPENAI_BASE_URL=` 与该明文；`config export nosuch` → 退出码 1
  12. `pony doctor --probe-token <明文>` → "failed: 0"；人为停掉临时 server 后 `pony status` → 退出码 3
- 结束清理：杀临时进程、删临时目录（trap）

## 8. 验收标准

- `cargo test --workspace` 全绿（含 cli 单元测试）
- `M1_TEST_OFFLINE=1 bash scripts/m1_test.sh` 仍全绿（无回归）
- `bash scripts/m2_test.sh` 全绿退出 0
- 手动验收：纯 CLI 完成 Gemini 添加 → token 生成 → export → doctor 通过（dev 服务器外网不可达，按 T8 §1 离线子集口径：stub 兜底语义验证）
