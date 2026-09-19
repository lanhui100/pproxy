#!/usr/bin/env bash
# pproxy 复发自检（2026-09-19 加固，ADR：compliant-egress-host-skips-cf-pool）
#
# 覆盖三类哨兵，任一硬失败非零退出（供 systemd timer / 告警消费）：
#   1. 端口归属：8899/8900 若被监听，必须属于 systemd 的 pproxy.service，
#      绝不允许 nohup/裸起实例（P0-2 复发）。
#   2. 合规出口计数：统计最近窗口内 `tunnel established host=...pooled=false`
#      的 Cloud Code 系冷建连次数（= Vercel Fluid 调用次数，Vercel 有限额，
#      P1-6 可观测性）；超出阈值硬失败。
#   3. journal 哨兵：Address already in use（端口战争/循环崩溃）与
#      no_compliant_egress（配置漂移 → 合规 host fail-closed）为硬失败；
#      vercel collect failed 为已知 Hobby 降级，仅提示不置 FAIL。
#
# 用法: bash scripts/self-check-pproxy.sh [--verbose]
set -uo pipefail
cd "$(dirname "$0")/.."

VERBOSE=0
[ "${1:-}" = "--verbose" ] && VERBOSE=1
FAIL=0

# ---- 1. 端口归属 ----
for port in 8899 8900; do
    # 匹配 ":$port" 且后随非数字或行尾（避免 :88990 之类误命中 IPv6）
    pid=$(ss -ltnp 2>/dev/null | awk -v p=":$port([^0-9]|\$)" '$4 ~ p { match($0, /pid=([0-9]+)/, m); print m[1]; exit }')
    if [ -n "${pid:-}" ]; then
        cgroup=$(cat "/proc/$pid/cgroup" 2>/dev/null | tr '\n' ' ')
        case "$cgroup" in
            *"/system.slice/pproxy.service"*)
                [ "$VERBOSE" = 1 ] && echo "OK  :$port 属 systemd pproxy.service (pid=$pid)"
                ;;
            *)
                echo "FAIL: :$port 被 pid=$pid 占用，cgroup=$cgroup —— 非 systemd 实例！"
                FAIL=1
                ;;
        esac
    else
        [ "$VERBOSE" = 1 ] && echo "OK  :$port 空闲"
    fi
done

# ---- 2. 合规出口（Vercel Fluid）冷建连计数 ----
# Cloud Code 系 host 走 vgate 冷建连（pooled=false）= 每次 Vercel Fluid 调用。
# 阈值：600/小时（10 次/分钟）——超过视为异常流量尖峰（正常 agent 循环远低于此）。
WINDOW_MIN=${PPROXY_SELFCHECK_WINDOW_MIN:-60}
THRESHOLD=${PPROXY_SELFCHECK_THRESHOLD:-600}
count=$(journalctl -u pproxy.service --since "${WINDOW_MIN} min ago" --no-pager 2>/dev/null \
    | grep -c "tunnel established host=\(daily-cloudcode-pa\|cloudcode-pa\|cloudaicompanion\)\.googleapis\.com.*pooled=false" || true)
[ "$VERBOSE" = 1 ] && echo "INFO: 近 ${WINDOW_MIN}min 合规出口冷建连 = ${count} (阈值 ${THRESHOLD})"
if [ "$count" -gt "$THRESHOLD" ]; then
    echo "FAIL: 近 ${WINDOW_MIN}min 合规出口冷建连 ${count} 超阈值 ${THRESHOLD} —— Vercel Fluid 额度可能被烧穿"
    FAIL=1
fi

# ---- 3. journal 哨兵 ----
# `connect denied: no tunnel route` 是 allowlist 防火墙的正常拒绝（设计行为），
# 不计入失败；`Restart=always` 循环中的 "Address already in use" 出现即端口战争
# 或崩溃循环（本次事故现场），必须告警；`no_compliant_egress` 是合规 host
# 配置漂移到无 Vercel 端点（fail-closed），必须告警。
HARD_FAIL_PATTERNS=(
    "Address already in use"
    "no_compliant_egress"
)
for pat in "${HARD_FAIL_PATTERNS[@]}"; do
    hits=$(journalctl -u pproxy.service --since "${WINDOW_MIN} min ago" --no-pager 2>/dev/null | grep -c "$pat" || true)
    if [ "$hits" -gt 0 ]; then
        echo "FAIL: 近 ${WINDOW_MIN}min journal 出现 ${hits} 次 '$pat'"
        FAIL=1
    elif [ "$VERBOSE" = 1 ]; then
        echo "OK  : 无 '$pat'"
    fi
done
# vercel collect failed：Hobby 计划采集降级（已知现状，P1-6），仅提示不置 FAIL
vc_hits=$(journalctl -u pproxy.service --since "${WINDOW_MIN} min ago" --no-pager 2>/dev/null | grep -c "vercel collect failed" || true)
if [ "$vc_hits" -gt 0 ]; then
    echo "WARN: 近 ${WINDOW_MIN}min vercel collect failed ${vc_hits} 次（Hobby 采集降级，Vercel 用量不可观测）"
fi

if [ "$FAIL" -eq 0 ]; then
    echo "SELF-CHECK PASS"
else
    echo "SELF-CHECK FAIL"
fi
exit "$FAIL"
