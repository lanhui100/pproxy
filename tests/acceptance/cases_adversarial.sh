#!/usr/bin/env bash
# tests/acceptance/cases_adversarial.sh — L2-AT 对抗破坏注入（契约 C1 的鲁棒性边界）。
#
# 三类确定性注入（无随机、无概率、无 sleep 断言）：
#   AT1a 半死连接：发半截首部后静默      → 服务端必须在 30s 首部窗口 + 5s margin 内关闭
#   AT1b 零字节静默：连上后一字节不发    → 必须在 30s 首字节窗口 + 5s margin 内关闭
#   AT2a 单行 > 16 KiB（24 KiB 请求行）  → 必须拒绝（4xx 或关闭），进程不崩、RSS 不失控
#   AT2b 总首部 > 16 KiB（400 × 100B）   → 同上
#   AT3  固定 20 并发 × 3 轮（Barrier）  → 无死锁，全部在预算内返回（成功或明确拒绝）
#
# 断言载体：helpers/raw_probe.py（协议级观测），阈值由本文件显式传入，
# 观察值原样写入 [PASS]/[FAIL] 明细，便于复现。

AT_BUDGET_HALFOPEN="${AT_BUDGET_HALFOPEN:-35}"   # 契约 30s 窗口 + 5s margin
AT_BUDGET_OVERSIZE="${AT_BUDGET_OVERSIZE:-20}"   # 畸形首部拒绝预算（远小于 30s 窗口）
AT_BUDGET_CONCURRENT="${AT_BUDGET_CONCURRENT:-40}"
AT_CONCURRENCY="${AT_CONCURRENCY:-20}"
AT_ROUNDS="${AT_ROUNDS:-3}"

_proc_rss_kb() { awk '/VmRSS/{print $2}' "/proc/$GATE_PID/status" 2>/dev/null; }
_proc_alive() { kill -0 "$GATE_PID" 2>/dev/null; }

_adv_precondition() { [ "$CONNECT_UP" = "1" ] && return 0; return 1; }

case_AT1a() {
  if ! _adv_precondition; then record AT1a 1 "CONNECT 监听未就绪（$CONNECT_HOST:$CONNECT_PORT）：C1 未实现，无法注入"; return; fi
  local out rc=0
  out=$(run_helper "$HELPERS/raw_probe.py" halfopen \
    --connect-host "$CONNECT_HOST" --connect-port "$CONNECT_PORT" \
    --mode partial --budget "$AT_BUDGET_HALFOPEN") || rc=$?
  if [ "$rc" = "0" ] && _proc_alive; then
    record AT1a 0 "半截首部后静默 → 服务端有界关闭 | $out"
  else
    record AT1a 1 "半截首部后静默未被有界关闭或进程已死(proc_alive=$(_proc_alive && echo 1 || echo 0)) | $out"
  fi
}

case_AT1b() {
  if ! _adv_precondition; then record AT1b 1 "CONNECT 监听未就绪（$CONNECT_HOST:$CONNECT_PORT）：C1 未实现，无法注入"; return; fi
  local out rc=0
  out=$(run_helper "$HELPERS/raw_probe.py" halfopen \
    --connect-host "$CONNECT_HOST" --connect-port "$CONNECT_PORT" \
    --mode none --budget "$AT_BUDGET_HALFOPEN") || rc=$?
  if [ "$rc" = "0" ] && _proc_alive; then
    record AT1b 0 "零字节静默（首字节窗口）→ 服务端有界关闭 | $out"
  else
    record AT1b 1 "零字节静默未被有界关闭或进程已死(proc_alive=$(_proc_alive && echo 1 || echo 0)) | $out"
  fi
}

case_AT2a() {
  if ! _adv_precondition; then record AT2a 1 "CONNECT 监听未就绪（$CONNECT_HOST:$CONNECT_PORT）：C1 未实现，无法注入"; return; fi
  local before after out rc=0
  before=$(_proc_rss_kb)
  out=$(run_helper "$HELPERS/raw_probe.py" oversize \
    --connect-host "$CONNECT_HOST" --connect-port "$CONNECT_PORT" \
    --mode line --budget "$AT_BUDGET_OVERSIZE") || rc=$?
  after=$(_proc_rss_kb)
  local delta="?"
  if [ -n "$before" ] && [ -n "$after" ]; then delta=$((after - before)); fi
  # 越界/OOM 哨兵：RSS 增量 < 64 MiB 且进程存活
  if [ "$rc" = "0" ] && _proc_alive && { [ "$delta" = "?" ] || [ "$delta" -lt 65536 ]; }; then
    record AT2a 0 "单行24KiB → 拒绝（4xx/关闭）且进程存活、rss_delta=${delta}KiB | $out"
  else
    record AT2a 1 "单行24KiB 未被拒绝或进程异常(proc_alive=$(_proc_alive && echo 1 || echo 0), rss_delta=${delta}KiB) | $out"
  fi
}

case_AT2b() {
  if ! _adv_precondition; then record AT2b 1 "CONNECT 监听未就绪（$CONNECT_HOST:$CONNECT_PORT）：C1 未实现，无法注入"; return; fi
  local before after out rc=0
  before=$(_proc_rss_kb)
  out=$(run_helper "$HELPERS/raw_probe.py" oversize \
    --connect-host "$CONNECT_HOST" --connect-port "$CONNECT_PORT" \
    --mode headers --budget "$AT_BUDGET_OVERSIZE") || rc=$?
  after=$(_proc_rss_kb)
  local delta="?"
  if [ -n "$before" ] && [ -n "$after" ]; then delta=$((after - before)); fi
  if [ "$rc" = "0" ] && _proc_alive && { [ "$delta" = "?" ] || [ "$delta" -lt 65536 ]; }; then
    record AT2b 0 "总首部~40KiB → 拒绝（4xx/关闭）且进程存活、rss_delta=${delta}KiB | $out"
  else
    record AT2b 1 "总首部~40KiB 未被拒绝或进程异常(proc_alive=$(_proc_alive && echo 1 || echo 0), rss_delta=${delta}KiB) | $out"
  fi
}

case_AT3() {
  if ! _adv_precondition; then record AT3 1 "CONNECT 监听未就绪（$CONNECT_HOST:$CONNECT_PORT）：C1 未实现，无法注入"; return; fi
  local out rc=0
  out=$(run_helper "$HELPERS/raw_probe.py" concurrent \
    --connect-host "$CONNECT_HOST" --connect-port "$CONNECT_PORT" \
    --target-host "$AT3_TARGET_HOST" --target-port 443 \
    --token "$TOKEN" --n "$AT_CONCURRENCY" --rounds "$AT_ROUNDS" --budget "$AT_BUDGET_CONCURRENT") || rc=$?
  # 竞态后进程仍须可服务：立即单发一次 CONNECT，确认监听未死
  local code
  code=$(curl -s --noproxy '' -o /dev/null -m "$HTTP_T" -w '%{http_code}' \
    -x "$PROXY_URL" -U "$TOKEN:$TOKEN" "$G1_TARGET" 2>/dev/null)
  if [ "$rc" = "0" ] && _proc_alive && [ "$code" = "204" ]; then
    record AT3 0 "并发 ${AT_CONCURRENCY}×${AT_ROUNDS} 无死锁，全部有界返回，压测后单发仍 204 | $out"
  else
    record AT3 1 "并发竞态异常（post_check='${code:-<curl-error>}' proc_alive=$(_proc_alive && echo 1 || echo 0)） | $out"
  fi
}
