#!/usr/bin/env bash
# Self-contained finalize: wait for the 25-min window to end, freeze probes,
# then aggregate and write reverse/summary.md. No agent round needed.
set -u
D="$(cd "$(dirname "$0")" && pwd)"
L="$(cd "$D/.." && pwd)/logs"
START_MS="$(head -n1 "$D/START_TIME.txt")"
END_MS=$((START_MS + 25*60*1000 + 30*1000))   # start + 25min + 30s grace

now_ms() { date +%s%3N; }

echo "[finalize] window start=$START_MS end=$END_MS now=$(now_ms) waiting..." 
while [ "$(now_ms)" -lt "$END_MS" ]; do
  sleep 15
done
echo "[finalize] window over, freezing probes"
date +%s%3N > "$D/END_TIME.txt"
date +"%Y-%m-%d %H:%M:%S %Z" >> "$D/END_TIME.txt"

# freeze: stop our probe/control processes (match exact script paths only)
pkill -f "$D/probe.sh" 2>/dev/null
pkill -f "$D/control.sh" 2>/dev/null
sleep 2

echo "[finalize] sample counts:"
wc -l "$D/probe_routes.csv" "$D/control_direct.csv" "$D/control_route_test.csv"

python3 "$D/analyze.py" > "$L/finalize_reverse.log" 2>&1
rc=$?
echo "[finalize] analyze.py rc=$rc"
tail -n 5 "$L/finalize_reverse.log"
echo "[finalize] done. summary.md written at $(date +%H:%M:%S)"
