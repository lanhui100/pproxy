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

# 3. 准备下载二进制
DOWNLOAD_BASE="${PPROXY_DOWNLOAD_BASE:-https://get.ponyjob.top/dist}"
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
    echo -e "\n${YELLOW}[WARN] 主下载源连接失败，尝试从 GitHub Releases 镜像下载...${RESET}"
    GITHUB_URL="https://github.com/lanhui100/pproxy/releases/latest/download/${BINARY_NAME}"
    if ! curl -fSL $CURL_PROGRESS "$GITHUB_URL" -o "$TMP_FILE"; then
        echo -e "${RED}[ERROR] 二进制下载失败，请检查网络连接或代理设置。${RESET}"
        exit 1
    fi
fi

chmod +x "$TMP_FILE"
mv "$TMP_FILE" "${INSTALL_DIR}/pproxy"
echo -e "✓ 二进制已安装至: ${GREEN}${INSTALL_DIR}/pproxy${RESET}"

# 4. PATH 环境变量自适应注入（非 Root 用户）
if [ "$IS_ROOT" -eq 0 ]; then
    PATH_UPDATED=0
    if [[ ":$PATH:" != *":$HOME/.local/bin:"* ]]; then
        for RC in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
            if [ -f "$RC" ]; then
                if ! grep -q 'export PATH="$HOME/.local/bin:$PATH"' "$RC"; then
                    echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$RC"
                    PATH_UPDATED=1
                fi
            fi
        done
        if [ "$PATH_UPDATED" -eq 1 ]; then
            echo -e "✓ 已将 ${INSTALL_DIR} 自动写入 Shell 配置文件 (.bashrc/.zshrc)"
        fi
    fi
fi

# 5. Systemd 守护进程自适应配置（仅在系统真实运行 Systemd 时启用）
if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
    mkdir -p "$SYSTEMD_DIR"
    SERVICE_FILE="${SYSTEMD_DIR}/pproxy.service"

    if [ "$IS_ROOT" -eq 1 ]; then
        cat <<EOF > "$SERVICE_FILE"
[Unit]
Description=Pony Proxy Standalone Daemon
After=network.target

[Service]
Type=simple
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
        systemctl enable pproxy || true
        systemctl restart pproxy || true
        echo -e "✓ 已配置并启动 Systemd 系统服务 (0.0.0.0:8899 共享模式)"
    else
        cat <<EOF > "$SERVICE_FILE"
[Unit]
Description=Pony Proxy User Daemon
After=network.target

[Service]
Type=simple
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
        systemctl --user enable pproxy || true
        systemctl --user restart pproxy || true
        echo -e "✓ 已配置并启动 Systemd 用户服务 (127.0.0.1:8899 本机自用)"
    fi
else
    echo -e "ℹ 未检测到运行中的 Systemd 环境（如 Docker 容器），跳过守护进程注册。"
    echo -e "  可使用前台命令直接启动: ${BLUE}pproxy serve${RESET}"
fi

# 6. 完成引导
echo -e "\n${BOLD}${GREEN}════════════════════════════════════════════════════════════════${RESET}"
echo -e "${BOLD}${GREEN}🎉 Pony Proxy 安装成功！${RESET}"
echo -e "${BOLD}${GREEN}════════════════════════════════════════════════════════════════${RESET}\n"

echo -e "${BOLD}常用快捷命令：${RESET}"
echo -e "  1. 开启终端代理:     ${BLUE}eval \"\$(pproxy on --eval)\"${RESET}"
echo -e "  2. 关闭终端代理:     ${BLUE}eval \"\$(pproxy off --eval)\"${RESET}"
echo -e "  3. 创建代理用户:     ${BLUE}pproxy user add <username>${RESET}"
echo -e "  4. 跨端配置同步:     ${BLUE}pproxy sync import \"<同步口令>\"${RESET}"
echo -e "  5. 代理健康诊断:     ${BLUE}pproxy doctor${RESET}"
echo -e "  6. 服务状态查看:     ${BLUE}pproxy status${RESET}\n"
