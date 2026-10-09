# pproxy accept 队列看门狗（deploy/watchdog/）

> 契约：`.dev-team/contracts/2026-10-09-pproxy-accept-loop.md` §4（wave-pproxy-accept-loop）
> 实现：`scripts/pproxy-watchdog.sh`；本目录为 systemd 单元与安装说明（写域：deploy/）。

## 作用

数据面 accept 循环根治（`gateway.rs serve_data_plane` 永不阻塞）之外的**兜底保险**：
若未来任何原因导致 accept 队列再次饱和（Recv-Q ≥ 64 = backlog 128 的 50%）连续 3 次采样
（间隔 5s），看门狗触发 `systemctl restart pproxy` 整进程回收（内核 accept 队列、
已持有 socket/permit 随进程退出释放；单元 Restart=always + RestartSec=5 已存在）。
触发后 60s 冷却再武装，防重启循环。

## 判定（契约 §4 冻结值，均可 env 注入）

| 变量 | 默认 | 说明 |
|---|---|---|
| `PPROXY_WATCHDOG_THRESHOLD` | 64 | LISTEN 套接字 Recv-Q（accept 队列深度）阈值 |
| `PPROXY_WATCHDOG_SAMPLES` | 3 | 连续命中采样次数 → 触发 restart |
| `PPROXY_WATCHDOG_INTERVAL_S` | 5 | timer 触发周期（系统单元 OnUnitActiveSec） |
| `PPROXY_WATCHDOG_COOLDOWN_S` | 60 | 触发后冷却（秒），冷却内不检测 |
| `PPROXY_WATCHDOG_PORT` | 8899 | 数据面监听端口 |
| `PPROXY_WATCHDOG_STATE_DIR` | /tmp | strikes/cooldown 状态文件目录 |
| `PPROXY_WATCHDOG_DRYRUN` | 0 | 1 = 打印 restart 命令不执行（测试接缝） |

探针：`ss -ltnH 'sport = :8899'` 解析 LISTEN 行 Recv-Q。
前置守卫：仅当 `systemctl is-active pproxy` 存活时检测（不掩盖崩溃）。
日志：`trace_id=watchdog-<run_ts> / event=detect|restart|rearmed|cooldown / recv_q / samples`。

## 安装

```bash
sudo cp deploy/watchdog/pproxy-watchdog.service deploy/watchdog/pproxy-watchdog.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now pproxy-watchdog.timer
```

依赖：dm 用户对 `systemctl restart pproxy` 具备 sudo NOPASSWD（部署机已配置）。

## 验证（机器命令）

```bash
# 判据→动作映射（dry-run，不真重启）
bash scripts/pproxy-watchdog.sh --dry-run
# 当前队列健康时输出: event=rearmed recv_q=<n> samples=0  exit=0

# timer 状态
systemctl list-timers pproxy-watchdog.timer
# 最近一次运行日志
journalctl -u pproxy-watchdog.service --since '10 min ago'
```

## 回滚

```bash
sudo systemctl disable --now pproxy-watchdog.timer
sudo rm /etc/systemd/system/pproxy-watchdog.{service,timer}
sudo systemctl daemon-reload
```