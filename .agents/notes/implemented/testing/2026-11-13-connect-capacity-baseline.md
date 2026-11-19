# Agent Note: rn VPS CONNECT 并发容量基线以带凭据水位采样为准

Status: implemented

## Problem

`backlog/tasks.jsonl` 的 B071 把 400 并发压测结论写成「nginx `worker_connections=768` 把上限卡在约 336 条 CONNECT，与 CPU/内存无关」，又把 `nf_conntrack_max=4096` 换算成 650–700 并发上限。两个数字都缺少内存、swap、丢包与 OOM 的同步水位证据，且 B071 的「与 CPU/内存无关」是在未采集这些维度的情况下下的判断。用户追问「这台机器最大并发上线时是 650–700 吗」时，现有证据不足以回答；继续沿用这两个数字会把估算值伪装成容量实测值。

## Decision

新增 `scripts/capacity-probe.py` 作为可复跑的 CONNECT 容量水位探针，并在**不修改生产配置**的前提下，对 rn VPS 本机回环 `127.0.0.1:8443` 做带凭据的 400 并发 CONNECT 采样。探针从 `/proc` 同步采集 `MemAvailable`/`MemFree`/`SwapFree`、`nf_conntrack_count`、nginx 与 gate-server RSS、探针自身 RSS、TCP established、`/proc/net/dev` 丢包、load1；以 2 秒间隔贯穿建连、持连与释放后 9 秒；`MemAvailable < 40 MB` 或 `SwapFree < 40 MB` 触发提前释放护栏。凭据只经 `PROBE_TOKEN` 环境变量传入，不落盘、不进日志；并发数与持连时长经 `PROBE_N`/`PROBE_HOLD` 注入。

本次实测口径与结果（生产机 1 核 / 458 MB RAM / 512 MB swap，`worker_connections=768`、`nf_conntrack_max=4096` 保持不变）：

| 观测项 | 基线 | 400 并发峰值/谷值 | 判定 |
|---|---|---|---|
| CONNECT 结果 | — | 366 成功 / 34 失败 | 34 条失败对应 nginx error.log 新增 34 条 `768 worker_connections are not enough while connecting to upstream`；容量墙确认为 nginx 连接槽，而非认证或上游 DNS |
| `nf_conntrack_count` | 203 | 1685（净增 1482，≈4.05 条/发起连接） | 未接近 4096；`conntrack` 不是本次 400 并发的瓶颈 |
| `MemAvailable` | 283948 kB | 141436 kB（下降 142512 kB ≈139 MB） | 尚余 138 MB，未触发 40 MB 护栏 |
| `SwapFree` | 281784 kB | 274780 kB（仅下降 7004 kB ≈6.8 MB） | 空闲态已用约 243 MB swap 属既有状态；压测没有造成显著新增换出 |
| nginx+gate RSS | 125760 kB | 144840 kB（+19080 kB ≈18.6 MB） | 服务端每发起连接约 47.7 kB，远小于 Backlog 估算的 32 KB×2 缓冲口径 |
| 探针自身 RSS | 22764 kB | 145264 kB（+122.5 MB） | 压测客户端与服务端同机，绝大部分内存增量来自探针自身；这使内存结论偏保守（真实客户端在异地时服务端侧更宽裕） |
| established | 77 | 1909 | 每条成功隧道对应多路本地/上游连接，与 conntrack 净增一致 |
| 网卡丢包 / OOM / conntrack full | 612 / 0 / 0 | 612 / 0 / 0 | 全程零增量，`dmesg` 无 OOM/conntrack full |

机器可验证承诺：`python3 -m py_compile scripts/capacity-probe.py` 通过；单连接冒烟 `PROBE_N=1 PROBE_HOLD=1` 得 1 成功 / 0 失败、状态行 `HTTP/1.1 200 Connection Established`；400 并发完整采样退出码 0，nginx error.log 增量 34 条且全为 worker_connections 告警，OOM 计数与网卡丢包计数零增量。

结论边界写入事实：**336 只是 2026-11-13 早先一轮压测的观测值，366 是本轮同配置下的观测值**，两者差异说明「扣掉 443 常态连接后余约 336」不是可复现的固定常数；`worker_connections=768` 才是当前确定的第一道容量墙。650–700 仍只是把 `conntrack` 净增率外推到 4096 的估算上限，且没有考虑 `worker_connections`、nginx worker `Max open files=1024`、以及客户端与服务端同机时的探针内存占比；**本轮没有放宽 nginx/conntrack，因此不宣称实测过 650–700，也不宣称内存一定不会在该区间先崩**。要回答放宽后的真实上限，需按 `backlog` B071 的待办另行决策并逐级加压；那属于生产配置变更，不在本条内实施。

## Alternatives considered

- **直接把 B071 的 336 改成 366、把 650–700 写成「实测上限」**：否决。单轮不同结果恰恰证明 336 是环境相关观测值；650–700 是线性外推，没有放宽第一道墙的实测，写成实测会制造第二个失准事实。
- **为了让数字更准，本轮顺手调高 `worker_connections` 与 `nf_conntrack_max` 再压 600/700**：否决。这是生产行为变更，需要独立决策、回滚方案与告警，且本轮目标是「在改配置前先拿到内存基线」，不是完成 B071 全部待办。
- **用无凭据的 400 并发采样近似资源水位**：否决。实测无凭据轮 400 次全为 `407 Proxy Authentication Required`，只产生短连接与约 21 MB 内存波动，不建立上游隧道，无法代表真实 CONNECT 持连的资源占用。
- **只记录成功数/失败数与 nginx 错误日志，不采集内存与 swap**：否决。那正是 B071 原有证据缺口，无法回答用户追问的「内存会不会先崩」。
- **把探针内存增量归因成服务端容量消耗**：否决。探针与 nginx/gate 同机，probe self RSS 单独采样，正文明确区分「服务端 +18.6 MB」与「客户端 +122.5 MB」，避免用总内存下降误判生产容量。

## Consequences

- `scripts/capacity-probe.py` 成为后续逐级加压的统一采样口径；`PROBE_TOKEN`/`PROBE_N`/`PROBE_HOLD` 是它的调用契约。
- B071 的事实表述需按本条校正：第一道墙是 nginx 连接槽，观测值随常态连接波动；conntrack 上限是外推值而非实测；内存/swap 在 400 并发下有充足余量，但 650–700 区间仍未经实测。
- 靠 review：探针把 TLS 校验关掉仅用于本机回环压测（`server_hostname` 仍为生产域名），不可直接复用为公网安全探测；凭据传递方式是否满足具体运维环境的安全要求需人工确认。
