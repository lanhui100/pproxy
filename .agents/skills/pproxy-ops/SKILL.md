---
name: pproxy-ops
description: |
  PProxy 智能出海代理与分布式集群运维助手。支持 macOS 与 Linux 平台。
  触发场景：
  - 代理环境管理："开启代理"、"关闭代理"、"挂起代理"、"恢复代理"、"查看代理状态" (pproxy on/off/status/env)
  - 分布式集群组网："生成入网令牌"、"加入集群"、"查看集群大盘" (pproxy cluster token-create/join/status)
  - 商业多租户配额："生成密钥对"、"签发用户令牌"、"撤销令牌" (pproxy user keygen/add/revoke)
  - 手机/第三方客户端生态："生成 Clash 配置"、"手机扫码代理" (pproxy clash)
  - 服务启停与自升级："启动本地网关"、"一键自升级" (pproxy serve/upgrade)
---

# pproxy-ops — PProxy 代理与分布式集群运维技能

本技能为 `pproxy` 的全生命周期管理提供标准化操作程序，支持 **macOS** 与 **Linux** 两大平台。

---

## 快速导航

| 领域 | 核心命令 (CLI) | 说明 |
| :--- | :--- | :--- |
| **客户端代理控制** | `pproxy on` / `pproxy off` / `pproxy status` | 自动配置系统代理及环境变量，智能内网绕过 |
| **临时挂起/恢复** | `eval "$(pproxy env suspend)"` / `resume` | 调试/构建时临时直连，恢复即时回填 |
| **多租户令牌签发** | `pproxy user add <user> -q 50G -d 30 -c 3` | 基于 Ed25519 签名，支持配额与并发限制 |
| **令牌吊销/封禁** | `pproxy user revoke <token_or_username>` | 全网黑名单落盘并热更新阻断 |
| **集群零接触扩容** | `pproxy cluster token-create` / `join -t ...` | 自动同步出海端点配置与公钥，自启服务 |
| **集群状态监控** | `pproxy cluster status` | 实网并发探测全网节点延迟与在线状态 |
| **移动端/Clash 接入**| `pproxy clash` | 导出包含探活免计费的 YAML 配置并在终端打印二维码 |

---

## 1. 客户端环境代理管理 (macOS / Linux)

### 1.1 开启代理 (一键出海)
自动探测内网环境，注入 `http_proxy`、`https_proxy` 与 `all_proxy`，并自动派生 `no_proxy`。
```bash
# 激活当前 Shell 环境代理
eval "$(pproxy on --eval)"

# 或直接开启（持久化至 ~/.pony/proxy.env）
pproxy on
```

### 1.2 查看状态与外网连通性
```bash
pproxy status
```

### 1.3 临时挂起与恢复 (编译/测试直连必备)
避免因为出海代理干扰内网探测：
```bash
# 临时挂起代理（保存现场）
eval "$(pproxy env suspend)"

# 恢复代理环境
eval "$(pproxy env resume)"
```

### 1.4 关闭环境代理
```bash
eval "$(pproxy off --eval)"
# 或清理持久化配置
pproxy off --hard
```

---

## 2. 分布式集群组网与备灾运维 (Cluster Mesh)

### 2.1 种子节点签发入网令牌 (Zero-Touch Token)
自动打包本机已生效的出海端点（如 RackNerd VPS / Vercel）及验签公钥，生成防篡改加入令牌：
```bash
# 签发 30 分钟有效的加入令牌
pproxy cluster token-create -m 30

# 若指定对外内网 IP (例如 Tailscale IP)
pproxy cluster token-create -s 100.95.193.103:8899 -m 30
```

### 2.2 工作节点一键入网自启 (Zero-Touch Bootstrap)
在全新机器上只需粘贴令牌，系统将自动自愈装配出海网关并在后台拉起双模服务：
```bash
pproxy cluster join -t "<JOIN_TOKEN>" --auto-start
```

### 2.3 查看全集群分布式大盘
并发探测所有已知对等节点的实际连通性与延迟：
```bash
pproxy cluster status
```

---

## 3. 商业多租户配额与凭据管理 (Ed25519)

### 3.1 管理机生成非对称密钥对
私钥严格保存在管理机 `~/.pony/cluster_signing_key.hex`（0600 权限），公钥配置至网关：
```bash
pproxy user keygen
```

### 3.2 离线签发多租户令牌
```bash
# 签发 50GB 流量、30 天有效、最大 3 并发的个人商业令牌
pproxy user add alice -q 50G -d 30 -c 3

# 短参数极速模式
pproxy user add bob -q 20G -d 15 -c 2
```
用户在客户端输入生成的 `usr_live_...` 令牌即可直显配额进度条与到期时间。

### 3.3 实时撤销令牌 / 封禁用户
```bash
# 按令牌废止
pproxy user revoke "usr_live_xxxx"

# 按用户名一键封禁全部历史令牌
pproxy user revoke alice
```
配置 `GATE_ADMIN_TOKEN` 环境变量时，将自动调用 `POST /api/user/revoke` 热更新网关内存黑名单，秒级 401 拦截。

---

## 4. 本地网关自启动与高可用守护 (Local HA Forwarder)

### 4.1 独立拉起网关服务
```bash
# 前台运行（独占 127.0.0.1:8899）
pproxy serve

# 开启局域网共享模式（绑定 0.0.0.0:8899，供手机/平板/局域网接入）
pproxy serve --lan
```

### 4.2 启用 Local HA Forwarder (永不断线高可用分发桩)
指定集群候选对等节点启动：
```bash
PPROXY_CLUSTER_PEERS="100.97.143.121:8899,100.105.241.39:8899" pproxy serve
```
- 本地对外固定入口 `127.0.0.1:8899` 由独立守护进程常驻监听（提供给本地大模型网关 ponyllm、IDE 插件）；
- 本地主引擎崩溃或重启时，**0 延时自动漂移至远程备灾节点**，业务完全不断线！

---

## 5. 手机端与第三方生态支持 (Clash Meta)

一键生成标准 Clash Meta / Mihomo 配置文件与终端二维码：
```bash
pproxy clash
```
打开手机 Clash 扫描终端打印的二维码，即可享受自动分流出海与系统探活免计费保护。

---

## 6. 故障排查与自检 (Doctor)

```bash
# 运行全链路自检诊断
pproxy doctor

# 自动自升级至最新稳定版本
pproxy upgrade
```
