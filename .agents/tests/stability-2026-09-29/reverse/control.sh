#!/usr/bin/env bash
# Track B control probes, every 60s:
#   1) direct upstream probes (simulating gateway forward) -> control_direct.csv
#   2) management-side route test API                    -> control_route_test.csv
set -u
D="$(cd "$(dirname "$0")" && pwd)"
SECRET="b96ee9195c590bc9a08c0284814bf1fead74e2f626d931ab14c892effa8a5882"
ADMIN="Authorization: Bearer pony_admin_88d1b64c486f0a1cbdfbb089a5453ef6cf0b3afda0787f7a"
MGMT="http://100.95.193.103:8900"
UENC="$(python3 -c "import urllib.parse;print(urllib.parse.quote('https://httpbin.org/status/200'))")"
D_OUT="$D/control_direct.csv"
T_OUT="$D/control_route_test.csv"
LOG="$D/../logs/control_reverse.log"

[ -f "$D_OUT" ] || printf 'ts_epoch_ms,host,http_code,total_ms\n' > "$D_OUT"
[ -f "$T_OUT" ] || printf 'ts_epoch_ms,route,ok,status,latency_ms,error\n' > "$T_OUT"

csvq() { # python csv-quote helper via stdin
  python3 -c "import csv,sys;w=csv.writer(sys.stdout);w.writerow([x if x is not None else '' for x in sys.stdin.read().strip().split('|')])"
}

while :; do
  ts="$(date +%s%3N)"
  # 1) direct upstream probes
  for host in rn.ponygo.fun vedge.ponygo.fun edge.ponygo.fun; do
    res="$(curl -s -m 20 -o /dev/null -w '%{http_code} %{time_total}' \
      -H "X-Proxy-Secret: $SECRET" "https://$host/api/proxy?url=$UENC" 2>/dev/null)"
    code="${res%% *}"; total="${res##* }"
    [ -z "$code" ] && code=000
    [ -z "$total" ] && total=20.0
    printf '%s,%s,%s,%s\n' "$ts" "$host" "$code" "$total" >> "$D_OUT"
  done
  # 2) management route test API
  for route in openai anthropic opencode xai github bai opencode-cf; do
    resp="$(curl -s -m 12 -X POST "$MGMT/api/routes/$route/test" -H "$ADMIN" 2>/dev/null)"
    if [ -z "$resp" ]; then
      printf '%s,%s,curl_fail,,,,curl_empty\n' "$ts" "$route" >> "$T_OUT"
      continue
    fi
    line="$(printf '%s' "$resp" | python3 -c "
import json,sys,csv,io
d=json.load(sys.stdin)
e=str(d.get('error') or '').replace('\n',' ').replace('\r',' ')
buf=io.StringIO()
csv.writer(buf).writerow([str(d.get('ok','')),str(d.get('status','')),str(d.get('latency_ms','')),e])
sys.stdout.write(buf.getvalue().strip())
" 2>/dev/null)"
    if [ -z "$line" ]; then
      printf '%s,%s,parse_fail,,,\n' "$ts" "$route" >> "$T_OUT"
    else
      printf '%s,%s,%s\n' "$ts" "$route" "$line" >> "$T_OUT"
    fi
  done
  echo "[$(date +%H:%M:%S)] control cycle done ts=$ts" >> "$LOG"
  sleep 60
done
