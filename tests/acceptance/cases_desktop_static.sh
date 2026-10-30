#!/usr/bin/env bash
# tests/acceptance/cases_desktop_static.sh — 桌面端（W3）源码级契约断言。
#
# ============================ 豁免理由（必读） ============================
# 桌面端是 Tauri GUI crate（desktop/src-tauri，WebKit/GTK 依赖），本机无显示环境、
# 且不在 workspace 构建图内，无法产出可 headless 运行的二进制；而 G8 的原始判据
# （`ss -tlnp | grep :8899` + 非回环来源 CONNECT → 407）要求真实 GUI 进程 + 真实
# 非回环来源，单机回环 fixture 无法构造（源 IP 恒为 127.0.0.1，C4 明确回环免鉴权）。
# 因此桌面端三项降级为**源码级契约断言**（机器可查、失败非零退出）：
#   G8-static           C4：8899 缺省端口 / PPROXY_LAN_PORT / 0.0.0.0 绑定 /
#                       407 + Proxy-Authenticate / 回环豁免分支 / lib.rs 接线 / allow-lan:false
#   G6-desktop-static   C3：build_clash_meta_yaml 函数体内 port:8443 / username / password /
#                       无 Bearer 死配置 / 返回 Result（fail-fast）
#   G7-desktop-static   C6：同函数体内规则尾部与探活域顺序
# 免责边界：源码级断言只能证明"声明存在"，不能证明"运行行为正确"；运行态由 W3 自证
# （任务书明确 qa 不跨构建图验证桌面端）。
# ========================================================================

DESKTOP_SRC="$REPO_ROOT/desktop/src-tauri/src"
DESKTOP_LIB="$DESKTOP_SRC/lib.rs"
DESKTOP_ENGINE="$DESKTOP_SRC/proxy/engine.rs"

_grep_src() { # <pattern> [file|dir] — 默认扫描整个 src 树
  local pat="$1" file="${2:-$DESKTOP_SRC}"
  if [ -d "$file" ]; then
    grep -rqE -- "$pat" "$file" --include='*.rs'
  else
    grep -qE -- "$pat" "$file" 2>/dev/null
  fi
}

case_G8_static() {
  local missing=""
  [ -d "$DESKTOP_SRC" ] || { record G8-static 1 "桌面端源码目录缺失：$DESKTOP_SRC"; return; }

  # C4-1 缺省端口 8899 + env 覆盖 PPROXY_LAN_PORT
  _grep_src 'LAN_DEFAULT_PORT[[:space:]]*:[[:space:]]*u16[[:space:]]*=[[:space:]]*8899' "$DESKTOP_ENGINE" || missing="$missing 缺省端口常量8899"
  _grep_src 'PPROXY_LAN_PORT' "$DESKTOP_SRC" || missing="$missing env-PPROXY_LAN_PORT"
  # C4-2 LAN 监听绑定 0.0.0.0（非 127.0.0.1）
  _grep_src '0\.0\.0\.0' "$DESKTOP_ENGINE" || missing="$missing 绑定0.0.0.0"
  # C4-3 非回环强制 Basic；失败 407 + 标准质询头
  _grep_src '407 Proxy Authentication Required' "$DESKTOP_ENGINE" || missing="$missing 407状态行"
  _grep_src 'Proxy-Authenticate: Basic realm=' "$DESKTOP_ENGINE" || missing="$missing Proxy-Authenticate头"
  # C4-4 回环来源免鉴权分支必须存在（不得把回环也拦成 407）
  _grep_src 'is_loopback\(\)' "$DESKTOP_ENGINE" || missing="$missing 回环判定"
  # C4-5 设置项门禁：LAN 监听必须由 lib.rs 接线启动（无接线 = 永不监听）
  _grep_src 'LanListener' "$DESKTOP_LIB" || missing="$missing lib.rs-LanListener接线"
  # C4-6 allow-lan 必须保持 false（Clash 自身 inbound 设置，与上游节点无关）
  _grep_src 'allow-lan:[[:space:]]*false' "$DESKTOP_LIB" || missing="$missing allow-lan=false"

  if [ -z "$missing" ]; then
    record G8-static 0 "C4 源码级契约齐备（8899/PPROXY_LAN_PORT/0.0.0.0/407+质询头/回环豁免/LanListener 接线/allow-lan:false）"
  else
    record G8-static 1 "C4 源码级契约缺口:${missing}（运行态豁免，见文件头理由）"
  fi
}

case_G6_desktop_static() {
  [ -f "$DESKTOP_LIB" ] || { record G6-desktop-static 1 "桌面端 lib.rs 缺失：$DESKTOP_LIB"; return; }
  local detail rc=0
  detail=$(run_helper "$HELPERS/desktop_static.py" fields --file "$DESKTOP_LIB") || rc=$?
  if [ "$rc" = "0" ]; then
    record G6-desktop-static 0 "$detail"
  else
    record G6-desktop-static 1 "$detail（运行态豁免：Tauri 构建图外，见文件头理由）"
  fi
}

case_G7_desktop_static() {
  [ -f "$DESKTOP_LIB" ] || { record G7-desktop-static 1 "桌面端 lib.rs 缺失：$DESKTOP_LIB"; return; }
  local detail rc=0
  detail=$(run_helper "$HELPERS/desktop_static.py" rules --file "$DESKTOP_LIB") || rc=$?
  if [ "$rc" = "0" ]; then
    record G7-desktop-static 0 "$detail"
  else
    record G7-desktop-static 1 "$detail（运行态豁免：Tauri 构建图外，见文件头理由）"
  fi
}
