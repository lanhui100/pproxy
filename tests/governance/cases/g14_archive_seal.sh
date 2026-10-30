#!/usr/bin/env bash
# tests/governance/cases/g14_archive_seal.sh — G14-ARCHIVE-SEAL（Wave-2 追加）
#
# 冻结契约：
#   a) `.meta/gates/archive-seal.sh` 支持 `--check`（存在 manifest 时 exit 0）与 `--write`；
#   b) 在**临时副本**里篡改 `archived/*.md` 后 `--check` 必须非零；
#   c) manifest 落 `.agents/notes/archived/SHA256SUMS`。
#
# 隔离纪律：所有篡改/删除/重建都在仓库外镜像副本（$TMPROOT/g14 + git init）里做，
#   绝不写被验收仓库。镜像内先校正基线（--check 必须 0），再做篡改与回环实验：
#     基线 --check=0 → 篡改 archived/*.md → --check≠0 → 还原 + 删 manifest → --write=0
#     → 重建的 manifest 非空 → 再 --check=0（回环一致）。
#   若镜像内基线就不为 0，说明脚本绑定原仓库绝对路径、无法在检出态自证 → 如实判 FAIL 并给出实测。

SEAL_REL='.meta/gates/archive-seal.sh'
SEAL_MANIFEST_REL='.agents/notes/archived/SHA256SUMS'

case_G14_ARCHIVE_SEAL() {
  local seal="$REPO_ROOT/$SEAL_REL" manifest="$REPO_ROOT/$SEAL_MANIFEST_REL"
  local mirror base_rc tamper_rc write_rc n_man sealed target n_sealed

  if [ ! -f "$seal" ]; then
    printf '缺 %s（归档封条门不存在，archived 决策可被静默改写）\n' "$SEAL_REL"
    return 1
  fi
  if [ ! -f "$manifest" ]; then
    printf '缺 %s（manifest 未落盘，封条无基准）\n' "$SEAL_MANIFEST_REL"
    return 1
  fi
  n_man="$(count_lines "$(cat "$manifest")")"
  if [ "$n_man" -lt 1 ]; then
    printf '%s 为空（0 条封条条目）\n' "$SEAL_MANIFEST_REL"
    return 1
  fi

  # 1) 真实仓库：--check 必须放行
  run_cmd "$CMD_T" bash "$SEAL_REL" --check
  if [ "$CAP_RC" -ne 0 ]; then
    printf 'bash %s --check 在真实仓库应 exit 0，实得 rc=%s：%s\n' \
      "$SEAL_REL" "$CAP_RC" "$(oneline "$CAP_OUT")"
    return 1
  fi

  # 2) 仓库外镜像副本（保留相对目录结构 + git 索引，兼容 cwd/git 两种寻址方式）
  mirror="$TMPROOT/g14"
  rm -rf "$mirror"
  mkdir -p "$mirror/.agents" || {
    printf '无法创建镜像副本目录 %s\n' "$mirror"
    return 1
  }
  cp -a "$REPO_ROOT/.meta" "$mirror/.meta" || {
    printf '镜像副本复制 .meta 失败\n'
    return 1
  }
  cp -a "$REPO_ROOT/.agents/notes" "$mirror/.agents/notes" || {
    printf '镜像副本复制 .agents/notes 失败\n'
    return 1
  }
  git -C "$mirror" init -q >/dev/null 2>&1
  git -C "$mirror" add -A >/dev/null 2>&1
  # 造一个基线提交：部分实现在 --check 里取 HEAD 侧内容或判 git 干净度，
  # 无 HEAD 会把"实现正常"误报成失败 → 镜像必须具备与真实仓库等价的 HEAD 状态。
  git -C "$mirror" -c user.email=governance@test -c user.name=governance \
    commit -qm "governance-mirror-baseline" >/dev/null 2>&1 || true

  run_cmd_in "$mirror" "$CMD_T" bash "$SEAL_REL" --check
  base_rc="$CAP_RC"
  if [ "$base_rc" -ne 0 ]; then
    printf '镜像副本内 --check 基线应 0（脚本似绑定原仓库状态/绝对路径，检出态无法自证），实得 rc=%s：%s\n' \
      "$base_rc" "$(oneline "$CAP_OUT")"
    return 1
  fi

  sealed="$(find "$mirror/.agents/notes/archived" -maxdepth 1 -type f -name '*.md' 2>/dev/null | sort)"
  n_sealed="$(count_lines "$sealed")"
  # 优先篡改真正的归档决策（非 AGENTS.md 索引）
  target="$(printf '%s\n' "$sealed" | grep -v '/AGENTS\.md$' | head -n1 || true)"
  if [ -z "$target" ]; then target="$(printf '%s\n' "$sealed" | head -n1 || true)"; fi
  if [ -z "$target" ]; then
    printf 'archived/ 下无 *.md 可篡改（封条无覆盖面；manifest=%s 条）\n' "$n_man"
    return 1
  fi

  cp "$target" "$TMPROOT/g14-pristine.md" || {
    printf '备份 %s 失败\n' "${target#"$mirror/"}"
    return 1
  }
  printf '\n篡改标记 %s\n' "$(date +%s%N)" >>"$target"

  run_cmd_in "$mirror" "$CMD_T" bash "$SEAL_REL" --check
  tamper_rc="$CAP_RC"
  if [ "$tamper_rc" -eq 0 ]; then
    printf '篡改 %s 后 --check 仍 exit 0（封条未检出篡改，形同虚设）\n' "${target#"$mirror/"}"
    return 1
  fi

  # 3) --write 支持 + 回环：还原内容、删除 manifest、--write 重建、再 --check 必须 0
  cp "$TMPROOT/g14-pristine.md" "$target"
  rm -f "$mirror/$SEAL_MANIFEST_REL"
  run_cmd_in "$mirror" "$CMD_T" bash "$SEAL_REL" --write
  write_rc="$CAP_RC"
  if [ "$write_rc" -ne 0 ]; then
    printf 'bash %s --write 应 exit 0，实得 rc=%s：%s\n' "$SEAL_REL" "$write_rc" "$(oneline "$CAP_OUT")"
    return 1
  fi
  if [ ! -f "$mirror/$SEAL_MANIFEST_REL" ]; then
    printf '--write 未在 %s 生成 manifest（写端与读端路径不一致）\n' "$SEAL_MANIFEST_REL"
    return 1
  fi
  run_cmd_in "$mirror" "$CMD_T" bash "$SEAL_REL" --check
  if [ "$CAP_RC" -ne 0 ]; then
    printf '--write 重建 manifest 后 --check 应 0（回环不一致），实得 rc=%s：%s\n' \
      "$CAP_RC" "$(oneline "$CAP_OUT")"
    return 1
  fi

  printf 'manifest %s 条（archived *.md %s 个）；真实仓库 --check rc=0；篡改(%s)后 rc=%s 被检出；--write 重建后 --check rc=0\n' \
    "$n_man" "$n_sealed" "${target#"$mirror/"}" "$tamper_rc"
  return 0
}
