#!/bin/bash
# measure.sh — pproxy 稳定性测试 轨道C：物理网络路径质量测量（顺序、自完备、可中断续跑）
# 输出：network/raw/<host>.txt（每端点原始日志）+ network/run.out（总日志）+ network/DONE 标记
set +e
umask 002
BASE=/home/dm/pproxy/.agents/tests/stability-2026-09-29/network
RAW=$BASE/raw
mkdir -p "$RAW"
LOG=$BASE/run.out
{
echo "=== measure.sh start $(date '+%F %T %Z') ==="
echo "resolver: $(grep -m1 nameserver /etc/resolv.conf)"

PUBLIC_HOSTS="rn.ponygo.fun vedge.ponygo.fun edge.ponygo.fun gate.ponygo.fun vgate.ponygo.fun daily-cloudcode-pa.googleapis.com opencode.ai api.openai.com api.anthropic.com"

# stats <valuefile> -> "avg p50 p95 max n"（NA 当无数据）
stats() {
  local f=$1
  if [ ! -s "$f" ]; then echo "NA NA NA NA 0"; return; fi
  sort -n "$f" | awk -v n=NR '{a[n]=$1;s+=$1} END{
    if(n==0){print "NA NA NA NA 0";exit}
    i50=int((n-1)*0.5); i95=int((n-1)*0.95)
    printf "%.1f %.1f %.1f %.1f %d\n", s/n, a[i50], a[i95], a[n-1], n
  }'
}

measure_host() {
  local H=$1
  local F=$RAW/$H.txt
  local dnsV=$RAW/.tmp_dns_$H.txt tcpV=$RAW/.tmp_tcp_$H.txt tlsV=$RAW/.tmp_tls_$H.txt httpV=$RAW/.tmp_http_$H.txt
  : > "$dnsV"; : > "$tcpV"; : > "$tlsV"; : > "$httpV"
  {
    echo "##### $H @ $(date '+%F %T') #####"
    echo "----- DNS local resolver (dig +short +time=3) x5 -----"
    for i in 1 2 3 4 5; do
      t0=$(date +%s%N)
      r=$(dig +short +time=3 +tries=1 "$H" 2>&1)
      t1=$(date +%s%N); ms=$(( (t1-t0)/1000000 ))
      echo "run$i ${ms}ms ips=[$(echo "$r" | tr '\n' ' ')]" >> "$F"
      echo "$ms" >> "$dnsV"
    done
    echo "----- DNS @114.114.114.114 x3 (对照) -----"
    for i in 1 2 3; do
      t0=$(date +%s%N)
      r=$(dig +short +time=2 +tries=1 @114.114.114.114 "$H" 2>&1)
      t1=$(date +%s%N); ms=$(( (t1-t0)/1000000 ))
      echo "114run$i ${ms}ms ips=[$(echo "$r" | tr '\n' ' ')]" >> "$F"
    done
    echo "----- DNS @8.8.8.8 x3 (对照) -----"
    for i in 1 2 3; do
      t0=$(date +%s%N)
      r=$(dig +short +time=2 +tries=1 @8.8.8.8 "$H" 2>&1)
      t1=$(date +%s%N); ms=$(( (t1-t0)/1000000 ))
      echo "88run$i ${ms}ms ips=[$(echo "$r" | tr '\n' ' ')]" >> "$F"
    done
    echo "----- TCP :443 x15 (bash /dev/tcp) -----"
    for i in $(seq 1 15); do
      t0=$(date +%s%N)
      if timeout 6 bash -c "exec 3<>/dev/tcp/$H/443" 2>/dev/null; then
        t1=$(date +%s%N); ms=$(( (t1-t0)/1000000 ))
        echo "run$i ok ${ms}ms" >> "$F"; echo "$ms" >> "$tcpV"
      else
        t1=$(date +%s%N); ms=$(( (t1-t0)/1000000 ))
        echo "run$i FAIL(${ms}ms)" >> "$F"
      fi
    done
    echo "----- TLS handshake x10 (openssl s_client) -----"
    for i in $(seq 1 10); do
      t0=$(date +%s%N)
      out=$(timeout 12 openssl s_client -connect "$H:443" -servername "$H" </dev/null 2>&1)
      t1=$(date +%s%N); ms=$(( (t1-t0)/1000000 ))
      ok=$(echo "$out" | grep -c "BEGIN CERTIFICATE")
      echo "run$i ${ms}ms cert=$ok" >> "$F"
      if [ "$ok" -gt 0 ]; then echo "$ms" >> "$tlsV"; else echo "  (no cert -> handshake fail)" >> "$F"; fi
    done
    echo "----- CERT (showcerts x509 -noout) -----"
    timeout 12 openssl s_client -connect "$H:443" -servername "$H" -showcerts </dev/null 2>/dev/null \
      | openssl x509 -noout -subject -issuer -dates 2>/dev/null >> "$F"
    echo "----- HTTP x10 (curl -m 15 direct --noproxy) -----"
    for i in $(seq 1 10); do
      r=$(curl --noproxy '*' -s -m 15 -o /dev/null -w "%{http_code} %{time_total}" "https://$H/" 2>&1)
      code=${r% *}; tot=${r##* }
      echo "run$i code=$code total=${tot}s" >> "$F"
      echo "$tot" >> "$httpV"
    done
    echo "----- ICMP ping -c 10 -W 2 -----"
    ping -c 10 -W 2 "$H" >> "$F" 2>&1
    if echo "$H" | grep -qvE '^[0-9.]+$'; then
      echo "----- mtr -r -c 10 (wide) -----"
      timeout 90 mtr -rwb -c 10 "$H" >> "$F" 2>&1
    else
      echo "----- (tailnet IP: skip mtr) -----" >> "$F"
    fi
    echo "RESULT_HOST $H dns=$(stats "$dnsV") tcp=$(stats "$tcpV") tls=$(stats "$tlsV") http=$(stats "$httpV")"
  } >> "$F" 2>&1
  rm -f "$dnsV" "$tcpV" "$tlsV" "$httpV"
}

measure_node() {
  local H=100.105.241.39
  local F=$RAW/$H.txt
  local tcpV=$RAW/.tmp_tcp_$H.txt; : > "$tcpV"
  {
    echo "##### tailnet node $H @ $(date '+%F %T') #####"
    echo "----- tailscale status (确认链路) -----"
    tailscale status 2>/dev/null | grep -E "tencent|100.105.241.39" | head -2
    echo "----- tailscale ping x3 -----"
    timeout 20 tailscale ping -c 3 "$H" 2>&1 | head -6
    echo "----- ping -c 20 -W 2 -----"
    ping -c 20 -W 2 "$H" >> "$F" 2>&1
    # TCP 端口选择：8899 优先，其次 22
    PORT=8899
    if ! timeout 4 bash -c "exec 3<>/dev/tcp/$H/$PORT" 2>/dev/null; then PORT=22; fi
    echo "----- TCP :$PORT x20 (bash /dev/tcp) -----"
    for i in $(seq 1 20); do
      t0=$(date +%s%N)
      if timeout 5 bash -c "exec 3<>/dev/tcp/$H/$PORT" 2>/dev/null; then
        t1=$(date +%s%N); ms=$(( (t1-t0)/1000000 ))
        echo "run$i ok ${ms}ms (port $PORT)" >> "$F"; echo "$ms" >> "$tcpV"
      else
        t1=$(date +%s%N); ms=$(( (t1-t0)/1000000 ))
        echo "run$i FAIL(${ms}ms port $PORT)" >> "$F"
      fi
    done
    echo "RESULT_HOST $H tcp=$(stats "$tcpV")"
  } >> "$F" 2>&1
  rm -f "$tcpV"
}

measure_egress() {
  local E=$RAW/egress.txt
  {
    echo "##### egress identity @ $(date '+%F %T') #####"
    echo "----- local proxy -x 127.0.0.1:8899 -> ipinfo.io/json x3 -----"
    for i in 1 2 3; do
      echo "--- run$i ---"
      curl -s -m 12 -x http://127.0.0.1:8899 https://ipinfo.io/json; echo
    done
    echo "----- rn.ponygo.fun /api/proxy -> ipinfo.io/json x3 -----"
    local U="https://rn.ponygo.fun/api/proxy?url=https%3A%2F%2Fipinfo.io%2Fjson"
    for i in 1 2 3; do
      echo "--- run$i ---"
      curl --noproxy '*' -s -m 15 "$U" -H "X-Proxy-Secret: b96ee9195c590bc9a08c0284814bf1fead74e2f626d931ab14c892effa8a5882"; echo
    done
    echo "----- vedge.ponygo.fun /api/proxy x2 (已知 402 DEPLOYMENT_DISABLED) -----"
    local U2="https://vedge.ponygo.fun/api/proxy?url=https%3A%2F%2Fipinfo.io%2Fjson"
    for i in 1 2; do
      echo "--- run$i ---"
      curl --noproxy '*' -s -m 15 "$U2" -H "X-Proxy-Secret: b96ee9195c590bc9a08c0284814bf1fead74e2f626d931ab14c892effa8a5882"; echo
    done
    echo "----- edge/gate 是否存在 /api/proxy（单次探测）-----"
    for h in edge gate; do
      echo "--- $h ---"
      curl --noproxy '*' -s -m 10 "https://$h.ponygo.fun/api/proxy?url=https%3A%2F%2Fipinfo.io%2Fjson" -H "X-Proxy-Secret: b96ee9195c590bc9a08c0284814bf1fead74e2f626d931ab14c892effa8a5882" -w " [HTTP=%{http_code} t=%{time_total}s]"; echo
    done
  } > "$E" 2>&1
  # 兜底检查：若 ipinfo 全部失败，补 ipify/ipapi 样本
  if ! grep -q '"ip"' "$E"; then
    echo "ipinfo 不可达，改用兜底 API" >> "$E"
    for i in 1 2 3; do
      curl -s -m 12 -x http://127.0.0.1:8899 "https://api.ipify.org?format=json"; echo; done >> "$E" 2>&1
    for i in 1 2 3; do
      curl -s -m 12 -x http://127.0.0.1:8899 "https://ipapi.co/json/"; echo; done >> "$E" 2>&1
  fi
  echo "RESULT egress done"
}

# ===== 主流程 =====
for H in $PUBLIC_HOSTS; do
  measure_host "$H"
  sleep 1
done
measure_node
measure_egress
echo "=== measure.sh DONE $(date '+%F %T %Z') ==="
date +%s > "$BASE/DONE.txt"
} 2>&1 | tee -a "$LOG"
