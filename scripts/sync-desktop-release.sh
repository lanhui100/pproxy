#!/usr/bin/env bash
# sync-desktop-release.sh — 将指定 tag 的 desktop release 资产同步到本地分发目录，
# 供 pproxy /dsk/ 端点向 tailnet 内的桌面端提供自更新（M5 拓展）。
#
# 用法：scripts/sync-desktop-release.sh desktop-v0.2.0
# 前提：gh 已认证（lanhui100）；分发目录默认 /home/USER/pony-desktop-releases
set -euo pipefail

TAG="${1:?usage: $0 <tag> 例: desktop-v0.2.0}"
DEST="${PPROXY_DESKTOP_DIST_DIR:-/home/USER/pony-desktop-releases}"

[[ "$TAG" =~ ^desktop-v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "tag 形如 desktop-v0.2.0"; exit 2; }

mkdir -p "$DEST"
echo "[sync] $TAG -> $DEST"
gh release download "$TAG" --repo lanhui100/pproxy --dir "$DEST" --clobber

echo "[sync] 内容清单："
ls -la "$DEST"
REQUIRED="latest.json"
if [[ ! -f "$DEST/$REQUIRED" ]]; then
  echo "WARN: 缺少 $REQUIRED（updater 无法发现更新）——确认 release 构建时 TAURI_SIGNING_PRIVATE_KEY secrets 已配置"
  exit 1
fi
echo "[sync] 完成 ✓"
