#!/usr/bin/env bash
# Forward-chain stability probe (Track A)
# Probes 4 real hosts through the local pproxy data plane 127.0.0.1:8899 (CONNECT tunnel pool).
# Output: per-host CSV in same dir -> probe_<id>.csv
# CSV rows: epoch_ms,host,http_code,connect_ms,appconnect_ms,total_ms,size_bytes
#   (failure => http_code=000, time fields=0 per spec; raw rc + real elapsed logged to ../logs/probe-failures.log)
set -u
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOGS="${DIR}/../logs"
mkdir -p "$LOGS"
PROXY="http://127.0.0.1:8899"
TIMEOUT=20
DURATION_S=1500          # ~25 min
START=$(date +%s)

# id|display_hostname|url
HOSTS=(
  "cloudcode|daily-cloudcode-pa.googleapis.com|https://daily-cloudcode-pa.googleapis.com/"
  "google204|www.google.com|https://www.google.com/generate_204"
  "github|api.github.com|https://api.github.com/"
  "opencode|opencode.ai|https://opencode.ai/"
)

for h in cloudcode google204 github opencode; do
  [ -s "$DIR/probe_${h}.csv" ] || echo "epoch_ms,host,http_code,connect_ms,appconnect_ms,total_ms,size_bytes" > "$DIR/probe_${h}.csv"
done

probe_one() {
  local id="$1" name="$2" url="$3"
  local epoch_ms rc line rest code conn ac total size conn_ms ac_ms total_ms
  epoch_ms=$(date +%s%3N)
  line=$(curl -x "$PROXY" -m "$TIMEOUT" -sS -o /dev/null \
      -w '|%{http_code}|%{time_connect}|%{time_appconnect}|%{time_total}|%{size_download}' \
      "$url" 2>>"$LOGS/curl-errors.log")
  rc=$?
  rest="${line#|}"
  code="${rest%%|*}"; rest="${rest#*|}"
  conn="${rest%%|*}"; rest="${rest#*|}"
  ac="${rest%%|*}";   rest="${rest#*|}"
  total="${rest%%|*}"; rest="${rest#*|}"
  size="${rest%%|*}"
  if [ "$rc" -ne 0 ]; then
    # keep a machine-readable failure record with the real rc and measured elapsed
    echo "FAIL epoch_ms=$epoch_ms rc=$rc host=$name code=$code connect=$conn appconnect=$ac total=$total size=$size" >> "$LOGS/probe-failures.log"
    code=000; conn=0; ac=0; total=0; size=0
  fi
  conn_ms=$(awk -v v="$conn" 'BEGIN{printf "%d", v*1000}')
  ac_ms=$(awk -v v="$ac"   'BEGIN{printf "%d", v*1000}')
  total_ms=$(awk -v v="$total" 'BEGIN{printf "%d", v*1000}')
  echo "${epoch_ms},${name},${code},${conn_ms},${ac_ms},${total_ms},${size}" >> "$DIR/probe_${id}.csv"
}

round=0
while [ $(( $(date +%s) - START )) -lt "$DURATION_S" ]; do
  round=$((round+1))
  for entry in "${HOSTS[@]}"; do
    IFS='|' read -r id name url <<< "$entry"
    probe_one "$id" "$name" "$url"
  done
  echo "$(date +%s%3N),round=$round,elapsed=$(( $(date +%s) - START ))s" >> "$LOGS/probe-progress.log"
  sleep 2
done
echo "probe done: rounds=$round elapsed=$(( $(date +%s) - START ))s end=$(date +%s%3N)" >> "$LOGS/probe-progress.log"