#!/bin/bash
# 兄弟项目代理隔离应急工具
#
# 用法: source scripts/mitigate-sibling-proxy.sh [on|off]
#   on  - 临时关闭代理环境变量（保存原值供恢复）
#   off - 恢复之前保存的代理环境变量
#
# 也可用等效的 CLI 命令:
#   eval "$(pproxy env suspend)"   # 保存并清除
#   eval "$(pproxy env resume)"    # 恢复

PPROXY_SAVED_FILE="${PPROXY_SAVED_FILE:-/tmp/.pproxy-saved-env}"

case "${1:-}" in
    on)
        cat > "$PPROXY_SAVED_FILE" <<SAVED_EOF
export http_proxy="${http_proxy:-}"
export https_proxy="${https_proxy:-}"
export no_proxy="${no_proxy:-}"
export HTTP_PROXY="${HTTP_PROXY:-}"
export HTTPS_PROXY="${HTTPS_PROXY:-}"
export NO_PROXY="${NO_PROXY:-}"
SAVED_EOF
        unset http_proxy https_proxy no_proxy
        unset HTTP_PROXY HTTPS_PROXY NO_PROXY
        echo "✓ 代理环境变量已临时清除（保存至 $PPROXY_SAVED_FILE）"
        echo "  之后执行: source $0 off"
        ;;
    off)
        if [ -f "$PPROXY_SAVED_FILE" ]; then
            source "$PPROXY_SAVED_FILE"
            rm -f "$PPROXY_SAVED_FILE"
            echo "✓ 代理环境变量已恢复"
        else
            echo "⚠ 没有找到保存的代理环境变量 ($PPROXY_SAVED_FILE)"
        fi
        ;;
    *)
        echo "用法: source $0 [on|off]"
        echo "  on  - 临时关闭代理并保存原值（供兄弟项目启动）"
        echo "  off - 恢复之前保存的代理环境变量"
        echo ""
        echo "也可用等效的 CLI 命令:"
        echo "  eval \"\$(pproxy env suspend)\"  # 保存并清除"
        echo "  eval \"\$(pproxy env resume)\"   # 恢复"
        ;;
esac