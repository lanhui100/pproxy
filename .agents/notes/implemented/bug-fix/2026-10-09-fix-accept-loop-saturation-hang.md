# Agent Note: 根治数据面 accept 循环 Semaphore 卡死（并发饱和 fast-fail + CONNECT relay 有界生命周期 + 看门狗）

Status: implemented

## Problem

生产事故（2026-10-09）：ponyllm 网关对 pproxy-host:8899 全部 10s connect timeout → 503
upstream_unavailable（muse / space-bunny / antigravity 全波及）。物理证据：内核 accept
队列 recv-Q 129/128、330 个 CLOSE-WAIT 连接。

根因（`crates/server/src/gateway.rs` serve_data_plane，修复前 L463-479）：
accept 循环内同步 `sem.acquire_owned().await`（`MAX_CONCURRENT_CONNECTIONS=256`，L29）。
CONNECT relay（`connect.rs` relay → `transport relay_bidir_ws`）持 permit 至结束且**无超时**；
死隧道（gate worker 静默 + 客户端静默，或 CLOSE-WAIT 半死连接）占满 256 个 permit 后，
accept 循环停在 acquire → 内核 accept 队列溢出 → 新连接全部挂死。

契约/基准：`.dev-team/contracts/2026-10-09-pproxy-accept-loop.md`（L2-P 冻结）与
`.dev-team/nfr-baseline-pproxy-accept-loop.json`；红相测试（L2-T/L2-AT，commit 4db3050）。

## Decision

1. **accept 循环永不阻塞**（契约 §1/§2）：`serve_data_plane` 改为
   `try_acquire_owned()`——成功 → spawn 持有 permit 的任务；失败 → **原地立即**
   写 `HTTP/1.1 503 Service Unavailable` + `connection: close` + `x-pproxy-reason:
   saturation` 后关闭（不 spawn、不排队、不获取许可）。accept() 持续排空内核队列。
   并发上限经 `PPROXY_MAX_CONNECTIONS` env 注入（契约 §2：env > 常量；非法/0 → 回落
   256 并 warn 含 raw；常量保留为 fallback）。
   日志（契约 §1 条款）：`trace_id / reason=saturation / conn_id / sem_used /
   sem_capacity / action=close_503`（trace_id = 连接级合成 `conn_<seq>_<unix_ms>`）。

2. **CONNECT relay 有界生命周期**（契约 §3）：`crates/transport/src/relay.rs` 新增
   `relay_bidir_ws_bounded(..., idle, absolute) -> Result<Option<RelayDeadlineHit>>`——
   `idle` = 双向连续无字节窗口（活动即重置），`absolute` = 单次 relay 总时长硬界；
   命中 → 强制双端断连并返回命中类别。既有 `relay_bidir_ws` 原样委托（None,None），
   engine/desktop 调用方零改动。`connect.rs relay` 包装调用：默认空闲 30s / 绝对 6h，
   env `PPROXY_RELAY_IDLE_TIMEOUT_MS` / `PPROXY_RELAY_ABSOLUTE_TIMEOUT_MS` 注入
   （非法/0 回落默认并 warn）。超时命中结构化日志：
   `trace_id / conn_id / reason=relay_idle_timeout|relay_absolute_timeout / elapsed_ms`。
   死隧道因此必然释放 permit（取最早者：relay 完成/空闲超时/绝对上限/客户端断开/服务停止）。

3. **peek 首字节窗口 30s**（`handle_conn`）：半开连接（connect 后不发任何字节）不得
   无限期占 permit——对齐既有 CONNECT 首行接收 30s 预算（F3 既有策略，契约 §3 不改动项
   的语义延伸，非新增契约面）。

4. **看门狗兜底**（契约 §4）：`scripts/pproxy-watchdog.sh` + `deploy/watchdog/`
   systemd 单元（service + timer，5s 周期，模式对齐 `pproxy-self-check.timer`）。
   判据：LISTEN 套接字 Recv-Q ≥ 64（backlog 128 的 50%）连续 3 次采样 →
   `sudo systemctl restart pproxy`；仅 pproxy 存活时检测；触发后 60s 冷却再武装；
   `--dry-run` / `PPROXY_WATCHDOG_DRYRUN=1` 测试接缝；阈值/次数/间隔/冷却均 env 可注入。
   部署见 `deploy/watchdog/README.md`（安装/验证/回滚命令）。

不改动（契约 §3 禁越界 + 防夹带重构）：CONNECT 首行 30s 超时、establish 重试
（MAX_ATTEMPTS=5）、502 tunnel_failed 语义、进程级日志格式（json 迁移登记为既有债项）、
admin 端点。

## Alternatives considered

- **A（选定）try_acquire + 原地 503/关闭**：饱和连接亚毫秒级确定终止（契约上界 3000ms、
  测试硬界 4000ms），accept 循环零阻塞；0 许可占用、0 spawn、0 排队 → 无任务/句柄堆积。
  代价：饱和窗口内的连接被快速拒绝（业务上正确——重试语义归客户端/ponyllm 网关池）。
  L2-AT 的 out-of-limit 断言 `served <= 上限` 与"并发上限约束同时持 permit 数"语义匹配
  （重冻结版已校准）。
- **B permit 获取移入 spawned 任务（无限排队）**：accept 循环放行所有连接，任务在其内
  `acquire_owned()` 排队。代价：任务数/打开 FD 无界增长（每排队连接持 1 socket），
  饱和被掩盖成"全部连接都在等"——内存/FD 耗尽风险，且契约 C2 明确拒绝（排队 = 无快速
  终止，测试红）。收益（0 503）无法抵消资源风险，否决。
- **C 背压/拒连（维持现状"排队即天然背压"）**：现状即事故形态——acquire 停转 → 内核
  accept 队列溢出（129/128）→ 未排队的连接直接挂死（connect timeout），比显式 503
  更糟（客户端无法区分"慢"与"永久挂"）。结论：背压只能作用于应用层（显式 503），
  绝不能停转内核 accept。
- **D relay 仅绝对整窗 timeout（30s 无空闲感知）**：活跃流（SSE/长下载）>30s 被误杀，
  违反"不打断活跃流"（契约 §3 空闲判定）。必须空闲（活动重置）+ 绝对（总时长硬界）
  双界，否决单一整窗。
- **E 在 server 侧复制 relay 循环加超时**：需把 `relay_bidir_ws` 的空闲感知逻辑复制进
  connect.rs——违反"别重复造轮子"与 NFR"复用既有 relay_bidir_ws 关闭语义"；选 transport
  有界变体（additive 新函数，既有函数零改动），成本是触达 crates/transport（本任务写域
  边界外的一次最小加法，随 diff 审计）。
- **F 看门狗只告警不自动重启**：事故证明人肉 SLO 不达标（恢复靠人工发现）；`pproxy.service`
  已 Restart=always，自动重启是该单元的既有恢复姿态；60s 冷却 + --dry-run 测试接缝
  控制循环与误触发风险。选自动重启。

## Consequences

- 数据面 accept 队列永不被 permit 获取卡停；饱和连接快速 503/关闭；恢复语义：任一
  permit 释放后下一连接立即受理（契约验收断言）。
- 死隧道（CLOSE-WAIT / 双向静默）在空闲 30s / 绝对 6h 内必然释放 permit，杜绝
  "256 死隧道 = 全站饱和"再次发生。
- 默认值对存量长连接的影响：单 relay 最长 6h；空闲超 30s 的隧道被回收（活跃流因
  双向字节活动不触发空闲界；Ping/Pong 保活流量计入活动）。
- 新增 env 面：`PPROXY_MAX_CONNECTIONS` / `PPROXY_RELAY_IDLE_TIMEOUT_MS` /
  `PPROXY_RELAY_ABSOLUTE_TIMEOUT_MS`（契约 §2，均有非法值回落 + warn）；
  看门狗 `PPROXY_WATCHDOG_*` 一组。
- 机器验证（非零退出命令）：
  - `cargo test -p pproxy-server` → 三集成二进制全绿 + lib 46 passed，Exit 0
    （saturation_contract / relay_dead_tunnel / adversarial 重冻结版，收据
    `.dev-team/exec-pproxy-evidence.log`）；
  - `bash .agents/skills/write-adr/verify-note.sh` → Exit 0；
  - `bash scripts/pproxy-watchdog.sh --dry-run` → Exit 0（recv_q=0 → rearmed）。
- 相关既有决策：`implemented/bug-fix/2026-10-09-fix-tunnel-pool-dead-session-and-timeout.md`
  （待命池/首帧超时生命周期）；本条补齐 relay 有界生命周期与 accept 循环根治，二者
  同属"连接生命周期治理"族。