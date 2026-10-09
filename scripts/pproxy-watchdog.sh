#!/usr/bin/env bash
# pproxy accept 队列看门狗（wave-pproxy-accept-loop 契约 §4，ADR：fix-accept-loop-saturation-hang）
#
# 检测：数据面 LISTEN 套接字 accept 队列深度（Recv-Q）≥ 阈值（默认 64 = backlog 128 的
# 50%）连续 SAMPLES 次采样（间隔由 systemd timer 触发，默认 5s）→ 触发
# `systemctl restart pproxy`（单元 pproxy.service 已 Restart=always + RestartSec=5）。
#
# 判据数值冻结于契约 §4：PPROXY_WATCHDOG_THRESHOLD(默认 64) / PPROXY_WATCHDOG_SAMPLES(默认 3)
# / PPROXY_WATCHDOG_INTERVAL_S(默认 5，timer 周期，脚本自身不 sleep) / PPROXY_WATCHDOG_COOLDOWN_S(默认 60)。
#
# 前置守卫：仅当 pproxy 进程存活时检测（不掩盖崩溃）；触发后 60s 冷却再武装（防重启循环）。
# 测试接缝：--dry-run / PPROXY_WATCHDOG_DRYRUN=1 时打印将执行的 restart 命令而不执行。
# 日志：trace_id=watchdog-<run_ts> / event=detect|restart|rearmed / recv_q / samples（JSON 兼容 key=value）。
set -uo pipefail

PORT="${PPROXY_WATCHDOG_PORT:-8899}"
THRESHOLD="${PPROXY_WATCHDOG_THRESHOLD:-64}"
SAMPLES="${PPROXY_WATCHDOG_SAMPLES:-3}"
COOLDOWN_S="${PPROXY_WATCHDOG_COOLDOWN_S:-60}"
STATE_DIR="${PPROXY_WATCHDOG_STATE_DIR:-/tmp}"
STRIKES_FILE="$STATE_DIR/pproxy-watchdog.strikes"
COOLDOWN_FILE="$STATE_DIR/pproxy-watchdog.cooldown"
RUN_TS=$(date +%s)

# env 非法值回落默认（契约 §4：判据可注入，非法回落）
is_pos_int() { [[ "$1" =~ ^[0-9]+$ ]] && [ "$1" -gt 0 ]; }
is_pos_int "$THRESHOLD" || { THRESHOLD=64; }
is_pos_int "$SAMPLES" || { SAMPLES=3; }
is_pos_int "$COOLDOWN_S" || { COOLDOWN_S=60; }

DRYRUN=0
[ "${1:-}" = "--dry-run" ] && DRYRUN=1
[ "${PPROXY_WATCHDOG_DRYRUN:-0}" = "1" ] && DRYRUN=1

log() { echo "trace_id=watchdog-$RUN_TS $*"; }

# ── 前置守卫 1：pproxy 服务存活才检测（不掩盖崩溃）─────────────────────────────
if ! systemctl is-active --quiet pproxy 2>/dev/null; then
    log "event=skip reason=service_inactive"
    exit 0
fi

# ── 前置守卫 2：触发后冷却窗口内不再检测（防重启循环）────────────────────────
if [ -f "$COOLDOWN_FILE" ]; then
    age=$(( $(date +%s) - $(stat -c %Y "$COOLDOWN_FILE" 2>/dev/null || echo 0) ))
    if [ "$age" -lt "$COOLDOWN_S" ]; then
        log "event=cooldown remaining=$(( COOLDOWN_S - age ))"
        exit 0
    fi
    rm -f "$COOLDOWN_FILE"
fi

# ── 探针：LISTEN 行 Recv-Q（accept 队列深度）─────────────────────────────────
recv_q=$(ss -ltnH "sport = :$PORT" 2>/dev/null | awk '$1 == "LISTEN" { print $2; exit }')
if [ -z "${recv_q:-}" ]; then
    log "event=skip reason=no_listener port=$PORT"
    exit 0
fi
# 非数字兜底
is_pos_int "$recv_q" || recv_q=0

# ── 判据：连续 SAMPLES 次 Recv-Q ≥ THRESHOLD → restart ───────────────────────
strikes=0
[ -f "$STRIKES_FILE" ] && strikes=$(cat "$STRIKES_FILE" 2>/dev/null || echo 0)
is_pos_int "$strikes" || strikes=0

if [ "$recv_q" -ge "$THRESHOLD" ]; then
    strikes=$((strikes + 1))
    echo "$strikes" > "$STRIKES_FILE"
    log "event=detect recv_q=$recv_q samples=$strikes threshold=$THRESHOLD port=$PORT"
    if [ "$strikes" -ge "$SAMPLES" ]; then
        log "event=restart recv_q=$recv_q samples=$strikes threshold=$THRESHOLD"
        rm -f "$STRIKES_FILE"
        touch "$COOLDOWN_FILE"
        if [ "$DRYRUN" = 1 ]; then
            echo "DRY-RUN: would execute: sudo -n systemctl restart pproxy"
            exit 0
        fi
        # 部署机已配 NOPASSWD sudo（任务 T3 前提）；root/直接可执行场景兜底直调
        if sudo -n systemctl restart pproxy 2>/dev/null || systemctl restart pproxy 2>/dev/null; then
            log "event=restarted recv_q=$recv_q samples=$strikes"
        else
            log "event=restart_failed recv_q=$recv_q samples=$strikes"
            exit 1
        fi
        exit 0
    fi
else
    # 队列回落：重新武装（strikes 清零）
    [ -f "$STRIKES_FILE" ] && rm -f "$STRIKES_FILE"
    log "event=rearmed recv_q=$recv_q samples=0 threshold=$THRESHOLD"
    exit 0
fi
