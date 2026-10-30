#!/usr/bin/env bash
# tests/acceptance/cases_meta.sh — 断言灵敏度自检（防止"永远绿的测试"）。
#
# 反向控制（negative control）是 L2-AT 的必要配对：正向用例全绿只能说明"没报错"，
# 还要证明**断言本身会咬人**。本文件对断言器做变异注入与负控制：
#   META-ASSERT-SENSITIVITY  在 gate 产出的真 YAML 上做 3 处变异（port 443 / 尾部
#                            MATCH,DIRECT / 插回 Bearer 死配置），断言 yaml_contract.py
#                            对每种变异都必须非零退出，且对原始 YAML 必须零退出。
#   META-AT1-NEGCTRL         起一个"永不回包"的哑服务，断言 raw_probe.py halfopen
#                            在预算内**必须报红**（closed=0 → rc=1），证明 AT1 判定
#                            不是恒真。
#
# 纪律：变异全部发生在 $WORKDIR 副本上，绝不写仓库文件。

META_YAML="$WORKDIR/meta-base.yaml"

case_META_ASSERT_SENSITIVITY() {
  if [ ! -s "$SUB_BODY" ]; then
    record META-ASSERT-SENSITIVITY 1 "无可用 YAML 基线（依赖 G5c 红相），无法做变异灵敏度自检"
    return
  fi
  cp "$SUB_BODY" "$META_YAML"

  local bad="" base_rc=0 mut_rc
  run_helper "$HELPERS/yaml_contract.py" fields --file "$META_YAML" --token "$TOKEN" >/dev/null || base_rc=$?
  [ "$base_rc" = "0" ] || bad="$bad 基线应零退出(rc=$base_rc)"

  # 变异 1：port 8443 → 443（原缺陷形态）
  sed 's/^\([[:space:]]*port:\)[[:space:]]*8443[[:space:]]*$/\1 443/' "$META_YAML" >"$WORKDIR/meta-port.yaml"
  mut_rc=0; run_helper "$HELPERS/yaml_contract.py" fields --file "$WORKDIR/meta-port.yaml" --token "$TOKEN" >/dev/null || mut_rc=$?
  [ "$mut_rc" != "0" ] || bad="$bad 变异port443未被捕获"

  # 变异 2：插回 Bearer 死配置
  { cat "$META_YAML"; printf '    headers:\n      Authorization: "Bearer %s"\n' "$TOKEN"; } >"$WORKDIR/meta-bearer.yaml"
  mut_rc=0; run_helper "$HELPERS/yaml_contract.py" fields --file "$WORKDIR/meta-bearer.yaml" --token "$TOKEN" >/dev/null || mut_rc=$?
  [ "$mut_rc" != "0" ] || bad="$bad 变异Bearer未被捕获"

  # 变异 3：尾部 MATCH,PROXY → MATCH,DIRECT（原缺陷形态）
  sed 's/^\([[:space:]]*-[[:space:]]*\)MATCH,PROXY[[:space:]]*$/\1MATCH,DIRECT/' "$META_YAML" >"$WORKDIR/meta-match.yaml"
  mut_rc=0; run_helper "$HELPERS/yaml_contract.py" rules --file "$WORKDIR/meta-match.yaml" >/dev/null || mut_rc=$?
  [ "$mut_rc" != "0" ] || bad="$bad 变异MATCH,DIRECT未被捕获"

  if [ -z "$bad" ]; then
    record META-ASSERT-SENSITIVITY 0 "R2：基线零退出 + port443/Bearer/MATCH,DIRECT 三处变异全部被断言咬住"
  else
    record META-ASSERT-SENSITIVITY 1 "断言灵敏度不足:${bad}"
  fi
}

case_META_AT1_NEGCTRL() {
  local port pid rc=0 out
  port=$(free_port)
  python3 - "$port" <<'PY' >/dev/null 2>&1 &
import socket, sys, time
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1])))
s.listen(4)
try:
    conn, _ = s.accept()
    time.sleep(5.0)          # 故意不关闭、不回包（半死连接对照）
    conn.close()
except OSError:
    pass
finally:
    s.close()
PY
  pid=$!
  local start now
  start=$(date +%s%N)
  while :; do
    timeout 1 bash -c "exec 3<>/dev/tcp/127.0.0.1/$port" 2>/dev/null && break
    now=$(date +%s%N)
    [ $(((now - start) / 1000000)) -ge 3000 ] && break
    sleep 0.1
  done

  out=$(run_helper "$HELPERS/raw_probe.py" halfopen --connect-host 127.0.0.1 --connect-port "$port" \
    --mode none --budget 2) || rc=$?
  kill "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null

  if [ "$rc" != "0" ]; then
    record META-AT1-NEGCTRL 0 "负控制生效：哑服务（不关闭/不回包）在 2s 预算内被判 closed=0 → helper 非零退出 | $out"
  else
    record META-AT1-NEGCTRL 1 "负控制失效：哑服务未回包却被判通过（AT1 判定恒真，说明对抗断言无效） | $out"
  fi
}
