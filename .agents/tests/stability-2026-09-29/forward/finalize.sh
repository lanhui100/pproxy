#!/usr/bin/env bash
# Track-A finalize (self-contained; survives agent turn interruptions).
# 1) Waits for the probe window: START_TIME + 25min (~17:10), extend to +30min if any
#    host CSV has <150 data rows; hard cap +32.5min.
# 2) Watchdog: if probe.sh dies before the window ends, relaunch it (remaining time).
# 3) Dumps journal window, runs finalize_analyze.py -> summary.md / tunnel-pool-window.json / journal-errors.txt
set -u
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOGS="${DIR}/../logs"
mkdir -p "$LOGS"
START=$(cat "$DIR/START_TIME.txt")
WINDOW=1500 EXTEND=1800 HARD=1950
TARGET=$(( START + WINDOW ))

log() { echo "$(date '+%F %T') $*" >> "$LOGS/finalize.log"; }

min_lines() {
  local m=99999999 n
  for h in cloudcode google204 github opencode; do
    n=$(( $(wc -l < "$DIR/probe_$h.csv" 2>/dev/null || echo 0) - 1 ))
    [ "$n" -lt "$m" ] && m=$n
  done
  echo "$m"
}

probe_alive() { pgrep -f 'bash ./probe.sh' >/dev/null 2>&1; }

watchdog() {
  if ! probe_alive; then
    log "watchdog: probe.sh not alive -> relaunching (remaining window)"
    ( cd "$DIR" && nohup ./probe.sh >>run.out 2>>run.err & )
  else
    log "watchdog: probe.sh alive, no relaunch"
  fi
}

log "finalize started (START=$START target=$TARGET hard=$((START+HARD)))"
ml=0
while :; do
  now=$(date +%s)
  ml=$(min_lines)
  if [ "$now" -ge "$TARGET" ]; then
    if [ "$ml" -ge 150 ]; then log "window done: past target, ml=$ml"; break; fi
    if [ "$now" -ge $(( START + EXTEND )) ]; then log "window done: extended cap reached, ml=$ml"; break; fi
  fi
  if [ "$now" -ge $(( START + HARD )) ]; then log "window done: hard cap, ml=$ml"; break; fi
  last=$ml
  sleep 45
  ml=$(min_lines)
  if [ "$ml" -le "$last" ]; then watchdog; fi
done

log "dumping journal window (since @$START)"
journalctl -u pproxy.service --no-pager --since "@$START" > "$LOGS/journal-window.txt" 2>> "$LOGS/finalize.log"
log "journal lines: $(wc -l < "$LOGS/journal-window.txt")"

log "running finalize_analyze.py"
python3 "$DIR/finalize_analyze.py" "$DIR" "$LOGS" "$START" >> "$LOGS/finalize.log" 2>&1
rc=$?
log "finalize_analyze exit=$rc"

if [ "$rc" -eq 0 ] && [ -s "$DIR/summary.md" ]; then
  echo "FINALIZE_DONE" > "$DIR/FINALIZE_DONE"
  log "FINALIZE_DONE: summary.md ready"
else
  log "FINALIZE_FAILED rc=$rc"
  exit 1
fi