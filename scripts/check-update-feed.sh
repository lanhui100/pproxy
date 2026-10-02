#!/usr/bin/env bash
# check-update-feed.sh — 机械校验桌面更新 feed 通道可达且版本匹配。
# 用法：scripts/check-update-feed.sh 0.3.62
# 用法：scripts/check-update-feed.sh            # 不传版本则只校验可达与 JSON 合法
# 用法：scripts/check-update-feed.sh --github-only [0.3.62]
#   --github-only：只校验 GitHub latest 通道（发版后 CI 用；access.ponygo.fun 由网关主机
#   手动 sync-desktop-release.sh 更新，发布时刻可能仍是旧版，不能作为 CI 硬门禁）。
# 背景：2026-10-02 因 CLI 发布 v0.3.57 抢占 GitHub "latest"，releases/latest/download/latest.json
#       曾 302 → v0.3.57（无 feed）→ 404，导致桌面端检查更新报 error sending request。
#       本脚本在发布流程跑一次，防止同类回归（见 .agents/notes/implemented/bug-fix/2026-10-02-...）。
set -euo pipefail

GH_ONLY=0
declare -a POS
for a in "$@"; do
  [[ "$a" == "--github-only" ]] && GH_ONLY=1 || POS+=("$a")
done
EXPECTED="${POS[0]:-}"
REPO="lanhui100/pproxy"
FEED_URLS=(
  "https://github.com/${REPO}/releases/latest/download/latest.json"
  "https://access.ponygo.fun/dsk/latest.json"
)
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

checked=0
for url in "${FEED_URLS[@]}"; do
  if (( GH_ONLY )) && [[ "$url" != "${FEED_URLS[0]}" ]]; then
    echo "SKIP $url（--github-only）"
    continue
  fi
  if curl -fsSL -m 30 -o "$TMP/feed.json" "$url" 2>/dev/null; then
    version="$(python3 -c "import json,sys; d=json.load(open('$TMP/feed.json')); print(d.get('version',''))" 2>/dev/null || true)"
    [[ -n "$version" ]] || fail "$url 返回的 JSON 无 version 字段"
    checked=$((checked + 1))
    echo "OK  $url  -> version=$version"
    if [[ -n "$EXPECTED" && "$version" != "$EXPECTED" ]]; then
      fail "$url 版本 $version != 期望 $EXPECTED"
    fi
    url_plat="$(python3 -c "import json,sys; d=json.load(open('$TMP/feed.json')); print(next(iter(d.get('platforms',{}).values()))['url'])" 2>/dev/null || true)"
    if [[ -n "$url_plat" ]]; then
      curl -fsSI -m 30 -o /dev/null "$url_plat" || fail "feed 内产物 URL 不可达: $url_plat"
      echo "OK  产物URL -> $url_plat"
    fi
  else
    echo "WARN $url 不可达（跳过；国内网络直连 CF 或 GitHub 不通时属预期）" >&2
  fi
done

# 至少 GitHub 通道必须可达——它是桌面端 updater 的第一端点（2026-10-02 修复目标）
if curl -fsSL -m 30 -o "$TMP/gh.json" "${FEED_URLS[0]}" 2>/dev/null; then
  : # 已在循环内校验
else
  fail "GitHub latest feed 通道不可达：${FEED_URLS[0]}"
fi

echo "PASS: 更新 feed 校验完成（GitHub 通道 ✓，可选通道 $((checked - 1)) 个）"