#!/usr/bin/env bash
# sync-desktop-release.sh — 将指定 tag 的 desktop release 资产同步到本地分发目录，
# 供 pproxy /dsk/ 端点向 tailnet 内的桌面端提供自更新（M5 拓展）。
#
# 用法：scripts/sync-desktop-release.sh desktop-v0.2.0
# 前提：gh 已认证（lanhui100）；分发目录默认 $HOME/pony-desktop-releases
set -euo pipefail

TAG="${1:?usage: $0 <tag> 例: desktop-v0.2.0}"
DEST="${PPROXY_DESKTOP_DIST_DIR:-$HOME/pony-desktop-releases}"

[[ "$TAG" =~ ^desktop-v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "tag 形如 desktop-v0.2.0"; exit 2; }

mkdir -p "$DEST"
echo "[sync] $TAG -> $DEST"
gh release download "$TAG" --repo lanhui100/pproxy --dir "$DEST" --clobber

# 改写 latest.json 内的资产地址：私仓 GitHub 直链对 updater 不可达（404），
# 指向 HTTPS 公开分发端点 access.ponygo.fun/dsk/（网关公开路由，M6 迁移；
# 旧 MagicDNS http 地址仅 tailnet 内可达且与 updater 端点域名不一致，已弃用）
DIST_BASE="${PPROXY_DIST_BASE:-https://access.ponygo.fun}"
python3 - "$DEST/latest.json" "$DIST_BASE" <<'PY'
import json, sys, os
path = sys.argv[1]
dist_base = sys.argv[2].rstrip("/")
d = json.load(open(path))
for plat in d.get("platforms", {}).values():
    url = plat.get("url", "")
    name = url.rsplit("/", 1)[-1]
    plat["url"] = f"{dist_base}/dsk/{name}"
json.dump(d, open(path, "w"), indent=2)
print("latest.json urls rewritten to", dist_base)
PY

echo "[sync] 内容清单："
ls -la "$DEST"
REQUIRED="latest.json"
if [[ ! -f "$DEST/$REQUIRED" ]]; then
  echo "WARN: 缺少 $REQUIRED（updater 无法发现更新）——确认 release 构建时 TAURI_SIGNING_PRIVATE_KEY secrets 已配置"
  exit 1
fi

# ---- 历史旧版本轮转淘汰策略（防磁盘撑爆） ----
# 默认保留最近 KEEP_VERSIONS 个版本（默认 3 个）
KEEP_VERSIONS="${KEEP_VERSIONS:-3}"
python3 - "$DEST" "$KEEP_VERSIONS" <<'PY'
import sys, os, glob, re

dest = sys.argv[1]
keep = int(sys.argv[2])

def parse_semver(filename):
    m = re.search(r'(\d+)\.(\d+)\.(\d+)', filename)
    if not m:
        return (0, 0, 0)
    return tuple(map(int, m.groups()))

pattern = os.path.join(dest, "*_x64-setup.exe")
exes = glob.glob(pattern)
exes.sort(key=lambda f: parse_semver(os.path.basename(f)), reverse=True)

for old_exe in exes[keep:]:
    try:
        os.remove(old_exe)
        print(f"[clean] 淘汰旧版安装包: {os.path.basename(old_exe)}")
    except OSError as e:
        print(f"WARN: 无法删除 {old_exe}: {e}")
    sig = old_exe + ".sig"
    if os.path.isfile(sig):
        try:
            os.remove(sig)
            print(f"[clean] 淘汰旧版签名: {os.path.basename(sig)}")
        except OSError:
            pass

all_files = os.listdir(dest)
current_exes = set(os.path.basename(f) for f in exes[:keep])
for f in all_files:
    if f.endswith("_x64-setup.exe.sig"):
        base_exe = f[:-4]
        if base_exe not in current_exes:
            try:
                os.remove(os.path.join(dest, f))
                print(f"[clean] 移除孤立签名: {f}")
            except OSError:
                pass
PY

echo "[sync] 完成 ✓"

