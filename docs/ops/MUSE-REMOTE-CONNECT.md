# Muse 远程主机连接指南

本指南说明如何在 dev 主机及 ~/pproxy 工作区中连接并操作远程 muse 主机（htch-runtime）。

## 1. 架构说明

- **目标主机**：muse.ponygo.fun（内部用户 dmin）
- **网络拓扑**：目标主机位于 NAT 之后，无公网直接入站 IP，通过 Cloudflare Tunnel + 转发器暴露出站隧道。
- **客户端接入**：通过 cloudflared access ssh 作为 SSH ProxyCommand 建立端到端加密通道。
- **认证方式**：Ed25519 纯密钥认证（私钥位置：~/.ssh/vm-ssh-key）。

## 2. 常用操作命令

### 交互式登录
`ash
ssh muse
`

### 远程执行单条命令
`ash
ssh muse 'uname -a && whoami && uptime'
`

### 文件传输 (SCP)
`ash
# 上传文件到 muse
scp ./local-file.txt muse:~/

# 从 muse 下载文件
scp muse:~/remote-file.txt ./
`

### 端口转发
若需将远程 muse 上的服务（例如端口 8080）映射到 dev 本地端口：
`ash
ssh -L 8080:127.0.0.1:8080 -N muse
`

## 3. SSH 配置详情 (~/.ssh/config)

`ssh-config
Host muse
  HostName muse.ponygo.fun
  User admin
  IdentityFile ~/.ssh/vm-ssh-key
  ProxyCommand /usr/local/bin/cloudflared access ssh --hostname %h
  ServerAliveInterval 60
  ServerAliveCountMax 3
  StrictHostKeyChecking accept-new
`
