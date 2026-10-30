#!/usr/bin/env bash
# tests/governance/cases/g8_docs.sh — G8-DOCS
#
# 冻结契约（三条同时成立）：
#   a) `desktop/README.md` 存在（桌面端文档有家）；
#   b) `grep -q PPROXY_YOUTUBE_PROXY README.md` 命中（环境变量契约已记载）；
#   c) `grep -nE '待填|待补' docs/AGENTS.md` 为空（文档骨架占位已清零）。
# 文件缺失一律 FAIL（fail-closed）：缺文件 ≠ "没有占位"。

case_G8_DOCS() {
  local desktop_readme="$REPO_ROOT/desktop/README.md"
  local readme="$REPO_ROOT/README.md"
  local agents_md="$REPO_ROOT/docs/AGENTS.md"
  local n_env hits n_hits

  # a) 桌面端文档有家
  if [ ! -f "$desktop_readme" ]; then
    printf '缺 desktop/README.md（桌面端文档无家）\n'
    return 1
  fi

  # b) README.md 记载 PPROXY_YOUTUBE_PROXY
  if [ ! -f "$readme" ]; then
    printf '缺仓库根 README.md\n'
    return 1
  fi
  n_env="$(grep -cF -e 'PPROXY_YOUTUBE_PROXY' "$readme" || true)"
  if [ "${n_env:-0}" -eq 0 ]; then
    printf 'README.md 未记载 PPROXY_YOUTUBE_PROXY（grep 0 命中，期望 ≥1）\n'
    return 1
  fi

  # c) docs/AGENTS.md 无「待填/待补」占位
  if [ ! -f "$agents_md" ]; then
    printf '缺 docs/AGENTS.md（无法验证占位清零）\n'
    return 1
  fi
  hits="$(grep -nE '待填|待补' "$agents_md" || true)"
  n_hits="$(count_lines "$hits")"
  if [ "$n_hits" -ne 0 ]; then
    printf 'docs/AGENTS.md 仍含 %s 处「待填/待补」占位：%s\n' "$n_hits" "$(list_head "$hits" 3)"
    return 1
  fi

  printf 'desktop/README.md 存在；README.md 含 PPROXY_YOUTUBE_PROXY %s 处；docs/AGENTS.md 占位 %s 处\n' \
    "${n_env:-0}" "$n_hits"
  return 0
}
