# 冻结契约（Wave 1–4，Lead 一票冻结，波内不得就地修改）

目标：让「扫码导入的 Clash Meta 配置」指向一个真实存在、真实鉴权、真实出网的端点。

## C1 — 公网 CONNECT 数据面（gate-server）

- 新增独立裸 TCP 监听，地址取 env `GATE_CONNECT_LISTEN`；**未设置则不启动**（fail-closed）。
  部署缺省值：`127.0.0.1:3102`。
- 协议：标准 HTTP/1.1 正向代理 `CONNECT host:port HTTP/1.1`。
  - 首部读取上限 **16 KiB**；首字节等待窗口 **30 s**；首部读取窗口 **30 s**。
  - 鉴权（二者任一即可）：
    - `Proxy-Authorization: Basic base64(user:pass)`，`user` 或 `pass` 任一等于有效令牌即通过；
    - `X-Pony-Token: <token>`。
  - 令牌校验：`sha256(token)` 等于配置的 tunnel token hash **或** 以 `usr_live_` 前缀经用户令牌验签并通过撤销表。
  - 目标 ACL：**仅 `port == 443`** 且 `is_valid_host(host)`（host 只允许 `[A-Za-z0-9.-]` 与 IPv6 字面量）；否则 `403`。
  - 成功：先回 `HTTP/1.1 200 Connection Established\r\nProxy-Agent: pproxy-gate\r\n\r\n`，随后双向裸字节中继。
  - 失败码：凭据缺失/错误 → `407 Proxy Authentication Required`（带 `Proxy-Authenticate: Basic realm="pproxy"`）；
    ACL 拒绝 → `403`；目标建连失败/超时 → `502`。
- 空闲超时 300 s；单连接绝对上限沿用进程级配置；不得泄漏任务（连接结束即释放）。
- 与 `/ws` 一致按 `claims.sub` 累计 `user_used_bytes` 配额。

## C2 — nginx 公网入口

- 新增 `stream` 块：`listen 8443 ssl;` + 复用 `/etc/letsencrypt/live/rn.ponygo.fun/{fullchain,privkey}.pem`
  → `proxy_pass 127.0.0.1:3102;`，`proxy_timeout 86400s;`。
- 既有 443 `server` 块**不得改动语义**；`nginx -t` 必须 exit 0；生效方式 `systemctl reload nginx`（零停机）。

## C3 — Clash 隧道模式 YAML（三处生成器同口径：desktop `build_clash_meta_yaml` / cli `generate_clash_yaml_mode` / gate-server `generate_clash_meta_yaml`）

```yaml
proxies:
  - name: "Pony-Tunnel"
    type: http
    server: <gate_host>
    port: 8443
    tls: true
    sni: <gate_host>
    skip-cert-verify: false
    username: "<token>"
    password: "<token>"
```
- **禁止**输出 `headers.Authorization: Bearer <token>`（CONNECT 路径不接受 Bearer，输出即为死配置）。
- token 缺失/空白 → 不生成隧道节点，生成器返回错误（fail-fast），严禁产出空凭据配置。

## C4 — 局域网模式 YAML（桌面端）

- 桌面端新增 LAN CONNECT 监听：`0.0.0.0:<PPROXY_LAN_PORT>`，缺省 `8899`；仅在设置项「允许局域网设备接入」为真时启动。
- 非回环来源 CONNECT **强制** Basic 鉴权（`username`/`password` 任一等于隧道令牌）；失败 `407`。
  回环来源行为**保持不变**（免鉴权，避免破坏系统代理/PAC 链路）。
- YAML：`server: <lan_ip>`、`port: 8899`、`username: "<token>"`、`password: "<token>"`、无 `tls`。
- `allow-lan` 保持 `false`（这是 Clash 客户端自身的 inbound 设置，与上游节点无关）。

## C5 — 订阅端点鉴权（gate-server `/clash` `/api/clash`）

- 令牌缺失或校验失败 → `401`（body 明示 `missing token` / `invalid token`）。
- 校验通过 → `200` + YAML，出站节点凭据即该请求令牌。
- 保留 `subscription-userinfo` / `profile-update-interval` / `content-disposition` 响应头。

## C6 — 分流默认行为（三处生成器同口径）

- 规则尾部改为 `- GEOIP,CN,DIRECT` 紧接 `- MATCH,PROXY`（原 `MATCH,DIRECT` 使绝大多数流量直连，体感「代理没生效」）。
- 探活/连通性域（`connectivitycheck.gstatic.com`、`clients3.google.com`、`msftconnecttest.com`、
  `captive.apple.com`、`cp.cloudflare.com`）与内网 CIDR **必须**排在所有 `PROXY` 规则之前，保持 DIRECT。

## 验收门禁（物理收据）

| ID | 断言 | 命令 |
| --- | --- | --- |
| G1 | gate CONNECT 可出网 | `curl -x http://127.0.0.1:3102 -U tok:tok -sS -o /dev/null -w '%{http_code}' https://www.google.com/generate_204` → `204` |
| G2 | 无凭据 407 | 同上去掉 `-U` → curl 报 `CONNECT tunnel failed, response 407` |
| G3 | 非 443 拒绝 | CONNECT `example.com:80` → `403` |
| G4 | 公网 8443 端到端 | `curl -x https://rn.ponygo.fun:8443 -U <token>:<token> ...` → `204` |
| G5 | 订阅鉴权 | `?token=probe` → `401`；有效 token → `200` |
| G6 | 三处 YAML 一致 | 三生成器输出均含 `port: 8443`、`username:`、`password:`，且无 `Authorization: "Bearer` |
| G7 | 分流默认 | 三处输出均以 `- MATCH,PROXY` 结尾，且探活域规则在其之前 |
| G8 | LAN 监听 | 桌面端开启后 `ss -tlnp \| grep :8899` 命中 `0.0.0.0:8899`；无凭据 CONNECT → `407` |
