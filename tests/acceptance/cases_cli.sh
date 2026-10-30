#!/usr/bin/env bash
# tests/acceptance/cases_cli.sh — 契约 C3/C6 的 CLI 生成器口径（crates/cli/src/cmd/clash.rs）。
#
# 覆盖门禁：
#   G6-cli           `pproxy clash` 隧道模式输出：port 8443 / username: <token> /
#                    password: <token>，且不含 Authorization: "Bearer（C3）
#   G7-cli           同输出：尾部 `- MATCH,PROXY`、GEOIP,CN,DIRECT 在其前、
#                    探活域与内网 CIDR 排在所有 PROXY 规则之前（C6）
#   G6-cli-failfast  token 缺失/空白 → 生成器非零退出（fail-fast），
#                    且绝不产出 `username: ""` 这类空凭据配置（C3 末条）
#
# 隔离纪律：用临时 HOME 承载 config.toml / clash.yaml，不读写用户真实 ~/.pony；
# fixture 的 admin 端点指向 127.0.0.1:1（立即 ECONNREFUSED），确保零外部依赖。
#
# 豁免：桌面端 `build_clash_meta_yaml`（W3）属 Tauri GUI crate，无法在无显示环境
# 构建出可运行二进制，故此处不做跨构建图验证 —— 其口径改由 cases_desktop_static.sh
# 做源码级断言，理由登记在 tests/acceptance/README.md 豁免表。

CLI_HOME="$WORKDIR/home-cli"
CLI_OUT="$WORKDIR/cli-clash.out"
CLI_ERR="$WORKDIR/cli-clash.err"

_cli_write_config() {
  local home="$1" gate_url="${2:-wss://gate.accept.test/ws}"
  mkdir -p "$home/.pony"
  cat >"$home/.pony/config.toml" <<EOF
server = "http://127.0.0.1:1"
admin_token = "accept-fixture-admin"
tunnel_gate_url = "$gate_url"
EOF
}

# 生成一次 CLI 输出；结果落到 $CLI_OUT（stdout）与 ~/.pony/clash.yaml（若生成器写了）
_cli_generate() {
  local home="$1"
  shift
  _cli_write_config "$home"
  ( cd "$REPO_ROOT" && HOME="$home" timeout 90 "$CLI_BIN" clash --url-only "$@" \
      >"$CLI_OUT" 2>"$CLI_ERR" )
  return $?
}

# 口径断言载体：优先取生成器落盘的 clash.yaml（同一生成器的物理产物），否则用 stdout
_cli_yaml_source() {
  local home="$1"
  if [ -s "$home/.pony/clash.yaml" ]; then printf '%s' "$home/.pony/clash.yaml"; return 0; fi
  if [ -s "$CLI_OUT" ] && grep -q '^proxies:' "$CLI_OUT"; then printf '%s' "$CLI_OUT"; return 0; fi
  return 1
}

case_G6_cli() {
  local rc=0 yaml problems="" detail rc_fields=0
  _cli_generate "$CLI_HOME" --token "$TOKEN" || rc=$?
  if [ "$rc" -ne 0 ]; then
    record G6-cli 1 "pproxy clash 退出码 ${rc}（期望 0），stderr: $(head -c 200 "$CLI_ERR" | tr '\n' ' ')"
    return
  fi
  if ! yaml=$(_cli_yaml_source "$CLI_HOME"); then
    record G6-cli 1 "生成器未产出 YAML：stdout/stderr 均为空或非 YAML（stdout_head='$(head -c 120 "$CLI_OUT" | tr '\n' ' ')'）"
    return
  fi
  grep -qE '^[[:space:]]*port:[[:space:]]*8443[[:space:]]*$' "$yaml" || problems="$problems port!=8443"
  grep -qE "^[[:space:]]*username:[[:space:]]*\"?$TOKEN\"?[[:space:]]*\$" "$yaml" || problems="$problems username-missing"
  grep -qE "^[[:space:]]*password:[[:space:]]*\"?$TOKEN\"?[[:space:]]*\$" "$yaml" || problems="$problems password-missing"
  grep -qiE 'Authorization:[[:space:]]*"?[[:space:]]*Bearer' "$yaml" && problems="$problems Bearer-forbidden"
  detail=$(run_helper "$HELPERS/yaml_contract.py" fields --file "$yaml" --token "$TOKEN") || rc_fields=$?
  if [ -z "$problems" ] && [ "$rc_fields" = "0" ]; then
    record G6-cli 0 "C3 口径（cli 生成器）: $detail"
  else
    record G6-cli 1 "C3 口径不符（cli 生成器），problems='${problems:-无}' | $detail"
  fi
}

case_G7_cli() {
  local yaml detail rc=0
  if ! yaml=$(_cli_yaml_source "$CLI_HOME"); then
    record G7-cli 1 "生成器未产出 YAML（依赖 G6-cli 的红相），C6 无法验证"
    return
  fi
  detail=$(run_helper "$HELPERS/yaml_contract.py" rules --file "$yaml") || rc=$?
  if [ "$rc" = "0" ]; then
    record G7-cli 0 "C6 分流（cli 生成器）: $detail"
  else
    record G7-cli 1 "C6 分流不符（cli 生成器）: $detail"
  fi
}

case_G6_cli_failfast() {
  local fresh="$WORKDIR/home-cli-failfast" rc=0
  rm -rf "$fresh"
  # 无 --token、config.toml 无 tunnel_token、HOME 干净（无历史 clash.yaml 可复用）
  _cli_generate "$fresh" || rc=$?
  local bad=""
  for f in "$fresh/.pony/clash.yaml" "$CLI_OUT"; do
    [ -f "$f" ] || continue
    grep -qE 'username:[[:space:]]*""' "$f" && bad="$bad empty-username@$(basename "$f")"
    grep -qE 'password:[[:space:]]*""' "$f" && bad="$bad empty-password@$(basename "$f")"
  done
  if [ "$rc" -ne 0 ] && [ -z "$bad" ]; then
    record G6-cli-failfast 0 "token 缺失 → 退出码 ${rc}（fail-fast），且未产出空凭据 YAML"
  else
    record G6-cli-failfast 1 "token 缺失时期望非零退出且无空凭据，实测 rc=${rc} 空凭据='${bad:-无}' stderr: $(head -c 160 "$CLI_ERR" | tr '\n' ' ')"
  fi
}
