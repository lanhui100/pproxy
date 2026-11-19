# Agent Note: rn VPS nginx 连接容量墙——worker_connections 与 reload 风暴

Status: implemented

> 前置与关系：本条是
> [`testing/2026-11-13-connect-capacity-baseline.md`](../testing/2026-11-13-connect-capacity-baseline.md)
> 里被显式推迟的那一步落地。那条记录以「生产行为变更，需要独立决策、回滚方案与告警」为由
> **否决了**在同一轮里顺手调 `worker_connections` / `nf_conntrack_max`，只交付了测量口径与基线。
> 本条接续该待办，给出独立决策与机器收据。两条不构成 supersession：那条仍是容量测量口径的权威，
> 本条是配置变更的权威。

## Problem

2026-11-13 排查 pproxy 出海长尾时，rn（192.210.231.8）的 `/var/log/nginx/error.log` 出现：

```
2026/11/13 04:08:xx [alert] 768 worker_connections are not enough
2026/11/13 10:30:08 [alert] 768 worker_connections are not enough while connecting to upstream,
                                client: 127.0.0.1, server: 0.0.0.0:8443, upstream: "127.0.0.1:3102"
```

当日累计 98 条。分三处根因，且**前两处此前被误判过**：

### 1. worker_connections 768 是 Ubuntu 默认值，与机型无关

`/etc/nginx/nginx.conf` 沿用发行版默认：`worker_processes auto; worker_connections 768;`。
`nproc=1` → 实际只有 1 个 worker，768 就是**全机总量**。stream 与 http 共享该配额，
每条 CONNECT 占 2 个连接（client + upstream）→ 理论上限 384 条。

### 2. 报错全部来自本机压测，不是线上流量

98 条报错的 `client` 字段全量统计：82 条 `127.0.0.1`、16 条为空。即**全部由回环压测触发**，
没有一条来自真实公网用户。线上 `/ws` refill 失败的峰值时段（CST 20–22 点，730 条）
对应 UTC 12–15 点，而该时段 rn 侧 `error.log` **除 `ssl_stapling` 警告外零报错**。

**因此「nginx 连接数耗尽导致 refill 失败」这个因果链不成立**，本轮实测将其证伪。
refill 失败的真实特征是 `dial timeout after 8s`（客户端侧建连超时），rn 侧无任何痕迹。

### 3. reload 风暴：每 5 分钟一次无条件 reload

`ponygo-pull.timer` 每 5 分钟拉 dist 并**无条件** `systemctl reload nginx`。当日 182 次 reload。
每次 reload fork 新 worker，老 worker 因持有长连接（`/ws` 隧道、8443 CONNECT）滞留，
实测累积到 8 个 `worker process is shutting down` 并存，每个 RSS ~18MB —— 在 458MB / swap 已用
229MB 的机器上是实打实的内存压力。而 `/var/www/ponygo/current` 的 mtime 是 11-08，
说明这些 reload **全部是无谓抖动**（静态文件没变）。

## Decision

三处一起改，都在 rn 上以脚本幂等落地（备份 `nginx.conf.bak-cap-*` / `.bak-grace-*` /
`ponygo-pull.bak-*`）：

1. `worker_connections 768 → 2048`，并新增 `worker_rlimit_nofile 8192`
   （原 worker 继承的软限制是 1024，不抬 fd 上限则连接数抬高后会被 fd 卡住）。
2. `nf_conntrack_max 4096 → 16384`，落 `/etc/sysctl.d/99-pproxy-conntrack.conf` 持久化
   （此前无任何 sysctl 持久化文件，重启即回 4096）。
3. `ponygo-pull` 的 reload 改为**内容变化才触发**：`rsync --itemize-changes | wc -l`，
   0 项变化则跳过；同时加 `worker_shutdown_timeout 30s` 给老 worker 一个硬上限
   （此前无上限，长连接可让老 worker 滞留 30min+）。

## Alternatives considered

- **只调 `worker_connections`**：能消掉报错，但 98 条报错本就全来自压测，线上无感；
  不解决 reload 风暴带来的内存压力，等于只修了「不痛的那一处」。否决为单点方案。
- **调大 `worker_processes` 到 2–4**：`nproc=1`，多 worker 在单核上只会增加上下文切换，
  且每个 worker 各占 768 配额并不等于总量翻倍（stream 每条连接仍要 client+upstream 两个）。
  收益不如直接抬 `worker_connections`，否决。
- **把 `ponygo-pull.timer` 周期从 5min 放宽到 30min**：降低 reload 频率但不消除无谓 reload，
  且牺牲发布时效；用「内容不变不 reload」能同时保住时效与零抖动，是更优解。
- **给 nginx 加 `MemoryMax`**：458MB 机身已吃紧，设上限会让 nginx 在峰值被 OOM kill
  而不是降级；容量问题应靠减少无谓 reload + 合理配额解决，否决。
- **提高 `DIAL_TIMEOUT`（8s）以掩盖 refill 失败**：那会把「建连失败」变成「建连更慢地失败」，
  且掩盖的是客户端侧跨境链路质量问题，与 nginx 容量无关。不在本轮范围。

## Consequences

机器证据（全部在 rn 上实测）：

- 容量：`200/400/600/1000` 并发 CONNECT 压测，**全部 ok=N fail=0**（改动前 400 并发即
  336 成功 + 64 失败）。`grep -c "worker_connections are not enough"` 停在 98，无新增。
- conntrack：1000 并发时 `nf_conntrack_count=3412 / max=16384`（21%），远低于 80% 阈值；
  改动前同样负载会顶到 4096 上限。
- reload 抑制：手动触发 `ponygo-pull.service`，日志出现
  `[pull] dist 无变化 → 跳过 reload`，nginx 未 reload。
- 优雅退出：`systemctl reload nginx` 后逐 5s 采样，老 worker 在 **+30s 由 5 个降到 4 个**，
  证明 `worker_shutdown_timeout 30s` 生效（此前无上限）。
- 功能未回归：`curl https://127.0.0.1/ -k` → 200；`nginx -t` syntax ok。

**边界与遗留（靠 review / 后续）**：

- 4 个改动前就存在的 `shutting down` worker 仍持有历史长连接，`worker_shutdown_timeout`
  对它们不追溯（新配置只对新 reload 产生的 worker 生效），会在连接自然结束时消失。
- **refill 失败（`dial timeout after 8s`）本轮未解决，且已证明与 nginx 容量无关**。
  实测 dev→rn 的 TLS 握手 p50=0.79s、p90=2.3s、p99=6.5s，有 3/100 超过 5s ——
  8s 的 `DIAL_TIMEOUT` 对含 TLS + WSS 首帧的建连偏紧，但这属于跨境链路质量问题。
- **测量口径教训**：本机 `HTTPS_PROXY=127.0.0.1:8899` 全程生效，此前多轮 curl 实际
  经自家代理回环，测到的是「代理→rn」而非「dev→rn」。直连必须显式
  `curl --noproxy '*'`（且 `*` 要加引号，否则被 shell 展开成当前目录文件列表）。
  本记录里所有「直连」数据均已用引号形式重测。
- 上述配置**只在 rn 一台机器上手工落地**，未进仓库；`deploy/nginx/stream-connect.conf`
  只覆盖 8443 server 块，不含 `worker_*` 全局指令。若要可复现，需补一份
  `deploy/nginx/nginx.conf.capacity` 片段并在 DEPLOY.md 里引用——本轮未做。
