#!/usr/bin/env bash
# ==============================================================================
# pproxy-ops Skill 一键自动化安装程序 (macOS / Linux 跨平台)
# 用法:
#   curl -fsSL https://raw.githubusercontent.com/lanhui100/pproxy/master/scripts/install-skill.sh | bash
# ==============================================================================

set -euo pipefail

BOLD="\033[1m"
GREEN="\033[32m"
BLUE="\033[34m"
YELLOW="\033[33m"
RESET="\033[0m"

echo -e "\n${BOLD}${BLUE}╔════════════════════════════════════════════════════════════════╗${RESET}"
echo -e "${BOLD}${BLUE}║       PProxy Management Skill (pproxy-ops) 安装程序            ║${RESET}"
echo -e "${BOLD}${BLUE}╚════════════════════════════════════════════════════════════════╝${RESET}\n"

# 1. 检测目标环境技能存储目录
# 支持标准 Agent 技能根目录：~/.agents/skills 与 Claude Desktop / Code 规范 ~/.claude/skills
TARGET_DIRS=()

if [ -d "$HOME/.agents/skills" ]; then
    TARGET_DIRS+=("$HOME/.agents/skills/pproxy-ops")
fi

if [ -d "$HOME/.claude/skills" ]; then
    TARGET_DIRS+=("$HOME/.claude/skills/pproxy-ops")
fi

# 若均未探测到，默认创建标准的 ~/.agents/skills/pproxy-ops
if [ ${#TARGET_DIRS[@]} -eq 0 ]; then
    TARGET_DIRS+=("$HOME/.agents/skills/pproxy-ops")
fi

RAW_SKILL_URL="https://raw.githubusercontent.com/lanhui100/pproxy/master/.agents/skills/pproxy-ops/SKILL.md"

for dest in "${TARGET_DIRS[@]}"; do
    echo -e "正在安装技能至: ${YELLOW}${dest}${RESET} ..."
    mkdir -p "$dest"
    
    # 若在本地仓库目录中执行，优先复制本地最新版本；否则自 GitHub 拉取
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd || true)"
    REPO_ROOT="$(cd "$SCRIPT_DIR/.." 2>/dev/null && pwd || true)"
    LOCAL_SKILL="$REPO_ROOT/.agents/skills/pproxy-ops/SKILL.md"
    
    if [ -f "$LOCAL_SKILL" ]; then
        cp -f "$LOCAL_SKILL" "$dest/SKILL.md"
        echo -e "  ✓ 已自本地仓库同步: ${GREEN}$dest/SKILL.md${RESET}"
    else
        if command -v curl >/dev/null 2>&1; then
            curl -fsSL "$RAW_SKILL_URL" -o "$dest/SKILL.md"
        elif command -v wget >/dev/null 2>&1; then
            wget -qO "$dest/SKILL.md" "$RAW_SKILL_URL"
        else
            echo "错误: 缺少 curl 或 wget，无法下载技能文件" >&2
            exit 1
        fi
        echo -e "  ✓ 已从 GitHub 远端拉取安装: ${GREEN}$dest/SKILL.md${RESET}"
    fi
done

echo -e "\n${BOLD}${GREEN}✓ pproxy-ops 技能已成功就绪！${RESET}"
echo -e "AI 助理现在可识别以下场景："
echo -e "  • 客户端环境管理:  ${YELLOW}pproxy on / off / status / env suspend / resume${RESET}"
echo -e "  • 分布式集群组网:  ${YELLOW}pproxy cluster token-create / join / status${RESET}"
echo -e "  • 商业多租户配额:  ${YELLOW}pproxy user keygen / add / revoke${RESET}"
echo -e "  • 移动端/Clash:    ${YELLOW}pproxy clash${RESET}\n"
