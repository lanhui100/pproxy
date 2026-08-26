#!/usr/bin/env bash
# auto-sync-desktop-release.sh — 检测到新的 desktop-v* tag 时自动同步分发目录。
#
# WHY：v0.3.7/v0.3.8 发布后曾因漏跑 scripts/sync-desktop-release.sh，
# 线上更新源滞留 0.3.6，已装客户端「检查更新」永远提示已是最新（2026-08-26 事故）。
# 本脚本由 pony-dsk-sync.timer 周期调用作兜底；手动发布流程不变。
#
# 幂等：以分发目录内 .synced-tag 标记文件判断是否需要动作。
set -euo pipefail
cd "$(dirname "$0")/.."

git fetch --tags --quiet origin
NEWEST=$(git tag -l 'desktop-v*' | sort -V | tail -1)
[[ -n "$NEWEST" ]] || { echo "[auto-sync] 无 desktop-v* tag，跳过"; exit 0; }

DIST="${PPROXY_DESKTOP_DIST_DIR:-/home/USER/pony-desktop-releases}"
MARKER="$DIST/.synced-tag"

if [[ -f "$MARKER" && "$(cat "$MARKER")" == "$NEWEST" ]]; then
  echo "[auto-sync] 最新 tag $NEWEST 已同步，无事可做"
  exit 0
fi

echo "[auto-sync] 发现未同步 tag：$NEWEST → 开始同步"
scripts/sync-desktop-release.sh "$NEWEST"
mkdir -p "$DIST"
echo "$NEWEST" > "$MARKER"
echo "[auto-sync] 完成：$NEWEST"
