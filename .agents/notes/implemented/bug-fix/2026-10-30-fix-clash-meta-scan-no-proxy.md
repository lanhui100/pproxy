# Agent Note: 手机 Clash Meta 扫码导入后无法出网（订阅声明了不存在的 CONNECT 数据面）

Status: implemented

## Problem

桌面端「一键生成 Clash Meta 配置 + 扫码订阅」流程中，手机扫码**能成功添加配置**，但激活 Mihomo /
Clash Meta 后**任何站点都不走代理**，用户的判断是"代理没启动"。

订阅链路本身是通的（`GET https://rn.ponygo.fun/api/clash?token=<任意值>` → 200 + 完整 YAML），
所以故障不在"导入"，而在**下发的配置指向了一个并不存在的服务能力**。

### 根因（生产实测证据链）

订阅 YAML 的出站节点声明为：

```yaml
- name: "Pony-Tunnel"
  type: http
  server: rn.ponygo.fun
  port: 443
  tls: true
  headers:
    Authorization: "Bearer <token>"
```

即"rn.ponygo.fun:443 上有一个 HTTPS 正向代理，用 Bearer 鉴权"。逐段证伪：

1. **443 后面没有 CONNECT**。`rn.ponygo.fun` 直连 A 记录 `192.210.231.8`；443 由 nginx 终结 TLS
   后反代到 `127.0.0.1:3101`（`pproxy-gate-server`）。实测：
   ```
   $ printf 'CONNECT www.google.com:443 HTTP/1.1\r\nHost: ...\r\n\r\n' \
       | openssl s_client -quiet -connect rn.ponygo.fun:443 -servername rn.ponygo.fun
   HTTP/1.1 400 Bad Request
   Server: nginx
   ```
   nginx 的 `http` 核心不转发 CONNECT；而 `crates/gate-server` 全 crate **零处 CONNECT 实现**
   （`grep -rc CONNECT crates/gate-server/src/*.rs` → 0/0/0），只有 `/ws` 首帧协议
   （`{"host","port"}`）与 `/proxy?url=` 反向网关。`crates/gate-server/README.md:8`
   声称"接收客户端 CONNECT/WS 隧道流量"，与代码不符——文档先行、实现缺位。
2. **Bearer 是无效凭据**。CONNECT 数据面（`crates/core/src/auth.rs::verify_connect_credentials`）
   只接受 `Proxy-Authorization: Basic` 与 `X-Pony-Token`；`Authorization: Bearer` 仅用于 `/ws`
   的 WS Upgrade 握手。即便 443 上真有 CONNECT，Bearer 也只会换来 407。
3. **局域网兜底同样死的**。无公网隧道时下发 `<lan_ip>:8899`，而桌面端引擎硬编码
   `127.0.0.1:18900`（`desktop/src-tauri/src/proxy/engine.rs`），且对 CONNECT **零鉴权**；
   用户机器上 8899 根本没有监听 → 手机连过去 Connection Refused。
4. **"扫码添加成功"是假信号**。订阅端点不校验 token（`?token=probe` 亦返回 200），
   导入成功只证明"YAML 下载成功"，与端点是否可用完全无关。
5. **分流把绝大多数流量直连掉了**。规则尾部 `MATCH,DIRECT`，只有硬编码的几十个域名走 PROXY。
   即使前四条都修好，用户仍会感觉"没代理"。

## Decision

**承认"数据面能力 = 契约"**：先补齐 CONNECT 数据面，再让配置生成器指向它，最后让分发链路
不再产出与能力不符的配置。

1. **新增标准 HTTP CONNECT 正向代理监听**（`crates/gate-server/src/connect_proxy.rs`）：
   裸 TCP、`CONNECT host:port HTTP/1.1`、首部上限 16 KiB、首字节/首部各 30 s 窗口、空闲 300 s、
   绝对上限 86400 s；鉴权接受 `Proxy-Authorization: Basic`（user 或 pass 任一等于令牌）与
   `X-Pony-Token`，令牌校验复用 `/ws` 同一套（tunnel token sha256 / `usr_live_` 验签 + 撤销表）；
   目标 ACL 仅 443 + 合法 host；凭据缺失/错误 → 407 + `Proxy-Authenticate`，ACL 拒绝 → 403，
   目标建连失败 → 502。**未设 `GATE_CONNECT_LISTEN` 则不启动**（fail-closed）。
2. **公网入口 nginx `stream` 8443 → 127.0.0.1:3102**。不复用 443：nginx 的 http 核心不转发
   CONNECT，443 上的 `server` 块无法承载该语义；8443 走 `stream` 纯 L4 转发 + TLS 终结，
   既有 443 路由面一行未改。
3. **三处生成器同口径**（desktop `build_clash_meta_yaml` / cli `generate_clash_yaml_mode` /
   gate-server `generate_clash_meta_yaml`）：`port: 8443` + `username`/`password`（同一令牌），
   **删掉** `headers.Authorization: Bearer`；令牌为空时 fail-fast，不产出无凭据配置。
4. **桌面端新增受控 LAN 监听**：`0.0.0.0:<PPROXY_LAN_PORT|8899>`，仅在"允许局域网设备接入"
   打开时启动；**非回环来源强制 Basic 鉴权**（否则 407），回环来源保持既有的免鉴权行为
   （保护系统代理 / PAC 链路）。LAN 侧同时提供 `/clash.yaml` 订阅端点，否则局域网扫码必 404。
5. **订阅端点鉴权**：`/clash` 与 `/api/clash` 是同一 handler，令牌缺失 → 401 `missing token`，
   无效 → 401 `invalid token`。宁可在导入阶段显式失败，也不要再产出一个"导入成功但不可用"的假成功。
6. **兜底分流改为** `- GEOIP,CN,DIRECT` 紧接 `- MATCH,PROXY`；探活域与内网 CIDR 仍全部前置 DIRECT。

## Alternatives considered

- **A（只在 443 上让 nginx 转发 CONNECT）**：被拒。nginx `http` 核心不支持 CONNECT 方法转发
  （实测就是它回 400）；要复用 443 只能上 `stream_ssl_preread` 按 SNI 分流，而 443 已被
  `rn.ponygo.fun` 的 http server 块占用，两者不能共存于同一 `listen`。改独立 8443 是唯一
  不动既有 443 语义的做法。
- **B（改用 `X-Pony-Token` 自定义头而非 Basic）**：被拒为唯一方案。Mihomo 的 `type: http` 出站
  对自定义 `headers` 的注入点与 CONNECT 语义不保证一致，而 `username`/`password` 是 Clash 侧
  一等公民字段（必然转成 `Proxy-Authorization: Basic`）。最终 Basic 为契约主通道，
  `X-Pony-Token` 保留为兼容通道（两条都在测试里覆盖）。
- **C（只修 LAN 模式，不做公网 CONNECT）**：被拒。UI 已在承诺"公网蜂窝任意 Wi-Fi 扫码即用"，
  只修 LAN 等于把谎话写得更详细；且 LAN 模式要求手机与电脑同网，不满足主场景。
- **D（把桌面引擎 18900 直接绑 0.0.0.0）**：被拒。桌面引擎对 CONNECT 零鉴权，绑 LAN 等于在
  用户 Wi-Fi 上开一个无凭据出海出口（任何人可白嫖其隧道与流量）。因此是为 LAN **新增**一个
  强制鉴权的独立监听，而不是放宽既有回环监听的绑定。
- **E（默认仍 `MATCH,DIRECT`，只补文档告知"仅白名单域名走代理"）**：被拒。产品语义是"出海代理"，
  未匹配即直连的默认会让绝大多数站点绕过代理，与用户预期直接冲突；国内流量改由
  `GEOIP,CN,DIRECT` 兜住，代价可控。
- **F（拉高 gate 令牌并发上限/换运营商线路来"缓解"）**：被拒。属于用容量掩盖协议缺失，
  与本故障无因果关系。
- **G（把 deploy 产物直接写进 `/etc/nginx/conf.d/`）**：被拒。Ubuntu 官方包把
  `include conf.d/*.conf` 放在 `http {}` **内部**，`stream {}` 放进去会 `unknown directive`。
  实际落 `/etc/nginx/stream-connect/` + `nginx.conf` 顶层 include，并把 `libnginx-mod-stream`
  安装（本机为 `--with-stream=dynamic` 且原本未安装该包）与 `ufw allow 8443/tcp` 写成部署硬步骤。

## 对抗审查后的加固（三路 L3 独立审查 → 3 个 BLOCKING）

首版实现经三路独立对抗审查（并发/资源、安全/协议、方案取舍）后**全部判 FAIL**，暴露出三个
真实缺陷，已修并复核：

1. **首部与首包同段时丢字节**（两路独立命中）：`read_head` 在首个 read 命中 `\r\n\r\n` 时
   返回截断后的 head，同包剩余字节（管线化 CONNECT、随首包发出的 TLS ClientHello）被静默丢弃，
   `leftover` 转发成为死代码。改为整段返回 + 统一 `split_head()` 切分，leftover 交上游。
   收据：首部 + 真实 ClientHello（517 B）同一次 `write` → 上游回 `160303007a` ServerHello。
2. **目标 ACL 可被编码绕过 → SSRF**（三路独立命中）：原实现是**字符串前缀黑名单**
   （只挡 `10.`/`127.`/`169.254.`/`192.168.`/`::1`），`0.0.0.0`、十进制 `2130706433`、
   十六进制 `0x7f000001`、IPv4-mapped `[::ffff:127.0.0.1]`、`[::]` 全部放行并真的拨到回环。
   改为**解析后判定**：IP 字面量（含十进制/十六进制/八进制/mapped）按解析结果判公网；
   域名先 `lookup_host` 取全部地址、任一非公网即 403；最终**只对已校验的 `IpAddr` 建连**
   （不对域名二次解析，封 DNS rebinding 窗口）。收据：生产 8443 上四种编码全部 `403`。
3. **组自检 URL 与数据面能力不符**：三处生成器写 `http://cp.cloudflare.com/generate_204`（明文 80），
   而 CONNECT 只放行 443 → Clash 对节点做的健康检查恒失败，面板上节点长期显示超时。
   改用 `https://www.gstatic.com/generate_204`。

同时修掉若干 ADVISORY：`run_server`（CLI `pproxy gate-server` 路径）原先不启动 CONNECT 监听
（与 binary 路径能力分叉、env 静默忽略）→ 模块移入 lib 并在 `run_server` 内 `spawn_from_env`；
桌面 LAN 会话加 idle 300 s / 绝对 86400 s 并在关闭开关时取消在飞会话；accept 单次出错不再
永久退出；二进制首包不再经 `from_utf8_lossy` 改写；令牌比较改 `constant_time_eq`；
订阅响应加 `Cache-Control: no-store`。其余 ADVISORY 登记为 `backlog/backlog.md` B016。

## 验证（物理收据）

- 本机（契约 G1/G2/G3）：`curl -x http://127.0.0.1:3102 -U tok:tok .../generate_204` → 204；
  去凭据 → `CONNECT tunnel failed, response 407`；`CONNECT example.com:80` → `403 acl_denied`；
  ACL 编码绕过族（`0.0.0.0` / `2130706433` / `0x7f000001` / `[::ffff:127.0.0.1]` / `[::]`）→ 全部 403。
  `cargo test -p pproxy-gate-server` → 17 passed；`cargo test -p pproxy-cli clash` → 7 passed。
- 生产（G4/G5）：`curl -x https://rn.ponygo.fun:8443 -U <token>:<token>` → google 204、
  youtube 200、`ipinfo.io/ip` 返回出口 `192.210.231.8`；订阅端点无 token / `?token=probe` → 401，
  有效 token → 200 且 YAML 为 `port: 8443` + `username`/`password` + 结尾 `- MATCH,PROXY`
  + `url: "https://www.gstatic.com/generate_204"` + 响应头 `cache-control: no-store`；
  生产 8443 上四种 ACL 编码变体全部 403。
- **真客户端端到端**：以 `Mihomo Meta v1.19.2` 直接加载生产下发的 YAML 并作为手机等价客户端：
  配置加载成功、`Mixed proxy listening at 127.0.0.1:7890`，经其访问 google → 204、
  `ipinfo.io/ip` → `192.210.231.8`（即 VPS 出口），日志 `match Match using PROXY[Pony-Tunnel]`
  证明兜底规则已真正走隧道。
- 独立验收套件：`tests/acceptance/`（L2-T + L2-AT，含半截首部、24 KiB 单行首部、20 并发 Barrier、
  harness 自检与变异敏感性/负控制）。

## 附带影响与残留

- `/clash` `/api/clash` 从"匿名可拉"变为"必须带有效令牌"：任何未带 token 的外部消费方会从 200
  变 401。仓库内唯一消费方是桌面端（已带 token），无回归。
- `pip`/Clash 侧的 `allow-lan: false` 保持不变：那是客户端自身 inbound 设置，与上游节点无关。
- 配额超限在 CONNECT 路径回 `403 + X-Pony-Reason: quota_exceeded`（未新增 402），与数据面既有
  错误码风格一致。
- CLI `run()` 仍硬拒显式非 443 的 `tunnel_gate_url`（该守卫针对 `/ws` 端点，不在本次范围）。
