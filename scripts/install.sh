#!/usr/bin/env bash
# ==============================================================================
# Pony Proxy (pproxy) — Linux 生产级一键自动化安装脚本
# 支持架构: x86_64 (amd64), aarch64 (arm64)
# 支持环境: Root / 非 Root 普通用户 / Docker 容器 / Systemd
# 用法: curl -fsSL https://get.ponyjob.top/install.sh | bash
# ==============================================================================

set -euo pipefail

# 颜色与样式
BOLD="\033[1m"
GREEN="\033[32m"
BLUE="\033[34m"
YELLOW="\033[33m"
RED="\033[31m"
RESET="\033[0m"

echo -e "\n${BOLD}${BLUE}╔════════════════════════════════════════════════════════════════╗${RESET}"
echo -e "${BOLD}${BLUE}║             Pony Proxy (pproxy) Linux 一键安装程序             ║${RESET}"
echo -e "${BOLD}${BLUE}╚════════════════════════════════════════════════════════════════╝${RESET}\n"

# 1. 系统与架构探测
OS="$(uname -s)"
if [ "$OS" != "Linux" ]; then
    echo -e "${RED}[ERROR] 本脚本仅支持 Linux 系统。当前系统: $OS${RESET}"
    exit 1
fi

ARCH="$(uname -m)"
case "$ARCH" in
    x86_64|amd64)
        TARGET_ARCH="x86_64-unknown-linux-musl"
        BINARY_NAME="pproxy-linux-amd64"
        ;;
    aarch64|arm64)
        TARGET_ARCH="aarch64-unknown-linux-musl"
        BINARY_NAME="pproxy-linux-arm64"
        ;;
    *)
        echo -e "${RED}[ERROR] 不受支持的 CPU 架构: $ARCH (仅支持 x86_64 与 aarch64)${RESET}"
        exit 1
        ;;
esac

echo -e "✓ 系统架构检测: ${GREEN}${OS} (${ARCH})${RESET}"

# 2. 确定安装路径与权限模式
if [ "$(id -u)" -eq 0 ]; then
    INSTALL_DIR="/usr/local/bin"
    IS_ROOT=1
    SYSTEMD_DIR="/etc/systemd/system"
    echo -e "✓ 权限模式: ${GREEN}Root 权限 (安装至 $INSTALL_DIR)${RESET}"
else
    INSTALL_DIR="$HOME/.local/bin"
    IS_ROOT=0
    SYSTEMD_DIR="$HOME/.config/systemd/user"
    echo -e "✓ 权限模式: ${YELLOW}非 Root 用户 (安装至 $INSTALL_DIR)${RESET}"
fi

mkdir -p "$INSTALL_DIR"

# 3. 准备获取二进制（优先检测本地已有构建产物，否则从 Release 下载）
LOCAL_TARGET_BIN="$(dirname "$0")/../target/release/pproxy"
if [ -f "$LOCAL_TARGET_BIN" ]; then
    echo -e "\n${BOLD}检测到本地构建二进制，直接安装...${RESET}"
    cp "$LOCAL_TARGET_BIN" "${INSTALL_DIR}/pproxy"
    chmod +x "${INSTALL_DIR}/pproxy"
    echo -e "✓ 二进制已就地安装至: ${GREEN}${INSTALL_DIR}/pproxy${RESET}"
elif [ -f "./target/release/pproxy" ]; then
    echo -e "\n${BOLD}检测到本地构建二进制，直接安装...${RESET}"
    cp "./target/release/pproxy" "${INSTALL_DIR}/pproxy"
    chmod +x "${INSTALL_DIR}/pproxy"
    echo -e "✓ 二进制已就地安装至: ${GREEN}${INSTALL_DIR}/pproxy${RESET}"
else
    GITHUB_RELEASE_BASE="https://github.com/lanhui100/pproxy/releases/latest/download"
    DOWNLOAD_BASE="${PPROXY_DOWNLOAD_BASE:-$GITHUB_RELEASE_BASE}"
    DOWNLOAD_URL="${DOWNLOAD_BASE}/${BINARY_NAME}"
    TMP_FILE="$(mktemp /tmp/pproxy.XXXXXX)"
    trap 'rm -f "$TMP_FILE"' EXIT INT TERM

    echo -e "\n${BOLD}正在下载 Pony Proxy 二进制...${RESET}"
    echo -e "来源: ${BLUE}${DOWNLOAD_URL}${RESET}"

    # TTY 检测：交互式终端显示动态 ANSI 进度条 (-#)，非交互式使用静默模式
    if [ -t 1 ]; then
        CURL_PROGRESS="-#"
    else
        CURL_PROGRESS="-s"
    fi

    if ! curl -fSL $CURL_PROGRESS "$DOWNLOAD_URL" -o "$TMP_FILE"; then
        echo -e "\n${YELLOW}[WARN] 主下载源连接失败，尝试从 CDN 备用镜像源下载...${RESET}"
        CDN_URL="https://get.ponyjob.top/dist/${BINARY_NAME}"
        if ! curl -fSL $CURL_PROGRESS "$CDN_URL" -o "$TMP_FILE"; then
            echo -e "${RED}[ERROR] 二进制下载失败，请检查网络连接或从 GitHub Releases 手动下载。${RESET}"
            exit 1
        fi
    fi

    chmod +x "$TMP_FILE"
    mv "$TMP_FILE" "${INSTALL_DIR}/pproxy"
    echo -e "✓ 二进制已安装至: ${GREEN}${INSTALL_DIR}/pproxy${RESET}"
fi

# 4. PATH 环境变量与 Shell Wrapper 极速函数注入
WRAPPER_BLOCK='# Pony Proxy Shell Integration
if command -v pproxy >/dev/null 2>&1; then
    pproxy() {
        case "$1" in
            on)
                # 按需拉起后台守护进程（不强占开机自启）
                systemctl --user is-active --quiet pproxy-server 2>/dev/null || systemctl --user start pproxy-server 2>/dev/null || true
                eval "$(command pproxy on --eval)"
                ;;
            off)
                eval "$(command pproxy off --eval)"
                ;;
            *)
                command pproxy "$@"
                ;;
        esac
    }
fi'

for RC in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
    if [ -f "$RC" ]; then
        if [ "$IS_ROOT" -eq 0 ] && [[ ":$PATH:" != *":$HOME/.local/bin:"* ]]; then
            if ! grep -q 'export PATH="$HOME/.local/bin:$PATH"' "$RC"; then
                echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$RC"
            fi
        fi
        if ! grep -q '# Pony Proxy Shell Integration' "$RC"; then
            echo -e "\n$WRAPPER_BLOCK" >> "$RC"
        fi
    fi
done
echo -e "✓ 已为当前用户注入极速 Shell 包装函数 (支持直接输入 pproxy on 自动拉起并注入环境)"

# 5. Systemd 守护进程单元注册（按需手动唤醒，默认不设置开机自启）
if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
    mkdir -p "$SYSTEMD_DIR"
    SERVICE_FILE="${SYSTEMD_DIR}/pproxy-server.service"

    if [ "$IS_ROOT" -eq 1 ]; then
        cat <<EOF > "$SERVICE_FILE"
[Unit]
Description=Pony Proxy Admin & Data Gateway Daemon
After=network.target

[Service]
Type=simple
EnvironmentFile=-/etc/pproxy/.pproxy.env
EnvironmentFile=-/root/.pony/.pproxy.env
ExecStart=${INSTALL_DIR}/pproxy serve --listen 0.0.0.0:8899
Restart=always
RestartSec=3
LimitNOFILE=65535
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF
        systemctl daemon-reload || true
        echo -e "✓ 已注册 Systemd 系统服务 (按需运行，未设置开机自启)"
    else
        cat <<EOF > "$SERVICE_FILE"
[Unit]
Description=Pony Proxy Admin & Data Gateway Daemon
After=network.target

[Service]
Type=simple
EnvironmentFile=-%h/.pony/.pproxy.env
ExecStart=${INSTALL_DIR}/pproxy serve --listen 127.0.0.1:8899
Restart=always
RestartSec=3
LimitNOFILE=65535
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=default.target
EOF
        systemctl --user daemon-reload || true
        echo -e "✓ 已注册 Systemd 用户服务 (按需运行，未设置开机自启)"
    fi
else
    echo -e "ℹ 未检测到运行中的 Systemd 环境（如 Docker 容器），跳过守护进程注册。"
fi

# 6. 完成引导
echo -e "\n${BOLD}${GREEN}════════════════════════════════════════════════════════════════${RESET}"
echo -e "${BOLD}${GREEN}🎉 Pony Proxy 安装成功！${RESET}"
echo -e "${BOLD}${GREEN}════════════════════════════════════════════════════════════════${RESET}\n"

echo -e "${BOLD}核心操作指南：${RESET}"
echo -e "  1. 开启终端代理:     ${BLUE}pproxy on${RESET}   (自动拉起后台服务并注入当前 Shell，附带实时测速)"
echo -e "  2. 关闭终端代理:     ${BLUE}pproxy off${RESET}  (就地清除当前 Shell 代理环境变量)"
echo -e "  3. 代理状态与测速:   ${BLUE}pproxy status${RESET}"
echo -e "  4. 全路由诊断体检:   ${BLUE}pproxy doctor${RESET}"
echo -e "  5. 跨端加密配置同步: ${BLUE}pproxy sync export${RESET}\n"
echo -e "💡 请执行 ${GREEN}source ~/.bashrc${RESET} 或重新打开终端以使 pproxy on 快捷函数立即生效。\n"
