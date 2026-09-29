#!/usr/bin/env bash
# Track B main probe: 7 reverse-gateway routes, one probe per route every 3s.
# Output: reverse/probe_routes.csv  (ts_epoch_ms,route,http_code,total_ms)
set -u
D="$(cd "$(dirname "$0")" && pwd)"
TOKEN="$(cat "$D/../test-token.txt")"
OUT="$D/probe_routes.csv"

declare -A RPATH=(
  [openai]=v1/models
  [anthropic]=v1/messages
  [opencode]=zen/v1/models
  [xai]=v1/models
  [github]=zen/v1/models
  [bai]=v1/models
  [opencode-cf]=zen/v1/models
)

probe_one() {
  local route="$1" path="$2"
  while :; do
    local ts code total res
    ts="$(date +%s%3N)"
    res="$(curl -s -m 20 -o /dev/null -w '%{http_code} %{time_total}' \
      "http://127.0.0.1:8899/$TOKEN/$route/$path" 2>/dev/null)"
    code="${res%% *}"; total="${res##* }"
    [ -z "$code" ] && code=000
    [ -z "$total" ] && total=20.0
    printf '%s,%s,%s,%s\n' "$ts" "$route" "$code" "$total" >> "$OUT"
    sleep 3
  done
}

[ -f "$OUT" ] || printf 'ts_epoch_ms,route,http_code,total_ms\n' > "$OUT"
for route in openai anthropic opencode xai github bai opencode-cf; do
  probe_one "$route" "${RPATH[$route]}" &
done
wait
