# M4 任务级 Specs — 公网入口（CF Tunnel）

> 上游: [ROADMAP M4](../../ROADMAP.md) + [TECH_DESIGN §2.4](../../../TECH_DESIGN.md) | 状态: 已审核（2026-08-22 对抗评审 F1-F20 → R1-R6 全部回填） | 日期: 2026-08-22
>
> 方案裁决（2026-08-22 会话确认）：**纯 CF Tunnel**。Tailscale 替代方案已评估并否决——大陆无可控 DERP 中继、手机端需装客户端入网、运营商 4G 直连质量不可控；本机既有 tailnet 仅作个人运维通道，不写入架构文档（ADR-006 记录否决结论时同样禁止写入 tailnet 标识等细节）。管理面公网化维持 P2 再评估。
>
> 审核裁决摘要：**R1** 公网闭环判定改三重断言（精确状态码+响应体特征+`server: cloudflare` 头）并强制对照负例（停隧道必现 5xx）；**R2** 凭据命令一律 dm 身份执行+属主权限断言，unit 改名 `pony-tunnel.service` 规避 `cloudflared service install` 覆盖回归，unit 规格补齐 Restart/RestartSec/network-online；**R3** 平台限制显式登记（免费层 ~100s 边缘超时→524、~100MB 请求体上限、协议钉死 http2、WARP 共存红线、明文过 CF 披露）；**R4** e2e token 流程 trap EXIT 强制 revoke+撤销后 401 复验+零回显，m4_test.sh 定性为生产变更型脚本须幂等可重跑；**R5** 威胁模型重写（token 入 path 暴露渠道列举、熵验证、"鉴权机制不变但暴露面扩大并接受"），限速缺口显式风险接受；**R6** §6 定位 smoke 并注明 vantage 差异，恢复窗口 10s→60s，补 8899 loopback/metrics 显式 loopback/环境幂等断言。

## 0. 现状探测结论（2026-08-22 实测，spec 的前提）

| 项 | 实测 | 结论 |
|----|------|------|
| 生产数据面监听 | `127.0.0.1:8899`（pproxy-server pid 实测 LISTEN） | cloudflared 与 server 同机，origin 直接回环可达 |
| 生产管理面监听 | `127.0.0.1:8900` | 保持 loopback；ingress 只配 8899 即天然隔离 |
| cloudflared 二进制 | 未安装；`~/.cloudflared/` 不存在 | 从零安装 + 全新凭据链 |
| 本机 WARP 客户端 | **`warp-svc.service` active（当前 Disconnected）** | 共存红线见 R3：WARP connect 会把 cloudflared 出站长连接卷入 WARP 隧道，运维上两者互斥；m4_test.sh 加非阻断观测项 |
| sudo | `sudo -n` NOPASSWD 可用 | apt 安装与 unit 安装可脚本化 |
| access.ponyjob.top | 当前 DNS 无任何记录（实测 dig 为空）；ADR-003 已预留该子域名 | `route dns` 可干净创建；部署前置预检断言 DNS 为空（防 dashboard 占位记录冲突） |
| CF 凭据 | `.secrets.env` 无 CF_API_TOKEN | tunnel 创建需用户授权配合（§3 配合点） |
| 国内可达性先例 | edge/vedge.ponyjob.top 经 CF 边缘国内正常（ADR-002/003 口径） | tunnel 走同一 CF 边缘网络，可达性风险最低 |
| 系统 | Ubuntu 24.04 noble | apt 安装用 signed-by keyring 方式（无 apt-key） |

## 1. 目标与范围

把生产数据面经 Cloudflare Tunnel 暴露到公网：`https://access.ponyjob.top/{token}/{route}/...`。cloudflared 以 systemd 服务常驻，仅转发数据面 ：8899；TLS 由 CF 边缘终结；服务器不开任何入站端口。

验收场景（ROADMAP 原文）：手机 4G 网络下 SDK 经 access.ponyjob.top 调用 zen 成功。

不在范围内：
- **管理面公网化**——维持 loopback-only，P2 再评估；
- **Tailscale 替代方案**——文首裁决，已否决；
- per-token 限速——**显式风险接受**（见 §8）：token 熵 128bit 不可枚举 + 个人网关流量模型下接受裸奔窗口，出现真实滥用再升级；
- 多 hostname / 暴露其他端口（单 ingress 规则）。

## 2. 架构

```
[手机 4G SDK]
   │ https://access.ponyjob.top/{token}/{route}/...
   ▼
Cloudflare 边缘（TLS 终结，免费层基础防护）
   │ CF Tunnel 出站长连接——协议钉死 http2（TCP），不用默认 QUIC/UDP：
   │ 大陆运营商 UDP 劣化正是否决 Tailscale 的同源教训（R3）
   ▼
pony-tunnel.service (cloudflared, User=pproxy, metrics 绑 127.0.0.1:19099)
   │ http://127.0.0.1:8899（ingress 唯一规则）
   ▼
pproxy-server 数据面（行为与局域网完全一致）
```

- **管理模式 locally-managed**：ingress 写本地 `config.yml` 且模板入库（deploy/cloudflared/），git 可追溯。
- **unit 改名 `pony-tunnel.service`**（R2）：规避与官方 `cloudflared service install` 生成的同名 unit 冲突——后者以 root 运行且无加固，手滑执行会静默覆盖降级。DEPLOY 文档同步写明禁用 `service install` 子命令。
- cloudflared 崩溃不影响 pproxy 本身：systemd 自动拉起；隧道中断时局域网路径始终可用。
- core/server 代码零改动——纯部署里程碑，代码库产出为部署资产（模板/unit/脚本）与文档。

## 3. 配置与凭据

| 项 | 位置 | 说明 |
|----|------|------|
| cloudflared 安装 | `/usr/bin/cloudflared` | pkg.cloudflare.com apt 源（signed-by keyring 方式，noble 兼容；需 sudo） |
| origin 证书 | `~/.cloudflared/cert.pem` | `cloudflared tunnel login` 浏览器授权一次（属主 dm，0600） |
| tunnel 凭据 | `~/.cloudflared/tunnel-pony-access.json` | `cloudflared tunnel create pony-access` 生成（0600，**永不入库**） |
| ingress 配置 | `~/.cloudflared/config.yml` ← 模板 `deploy/cloudflared/config.yml` | 唯一规则 access.ponyjob.top → `http://127.0.0.1:8899`；`protocol: http2`；`metrics: 127.0.0.1:19099`（避开业务端口与 m1-m3 测试段） |
| DNS 路由 | CNAME `<tunnel-id>.cfargotunnel.com` | `tunnel route dns pony-access access.ponyjob.top`；**前置预检**：dig 该域名须为空 |
| systemd unit | `systemd/pony-tunnel.service` ← `/etc/systemd/system/` | 规格：User=pproxy、After/Wants=network-online.target、Restart=always、RestartSec=5、NoNewPrivileges=true、ProtectSystem=full、ProtectHome=read-only、PrivateTmp=true |

**执行身份纪律（R2）**：sudo 仅限 apt 安装与 unit 拷贝两步；login/create/route dns 渲染一律 **dm 身份**执行——sudo 执行会把 `~/.cloudflared` 全部生成 root 属主，unit(User=pproxy) 读 0600 root 凭据启动即崩且表象是无限重启循环而非权限报错。部署后 `stat` 断言属主 dm 权限 600。

**用户配合点（凭据缺口，类比 M3-R2）**：`cloudflared tunnel login` 浏览器授权一次；或提供 API Token（Zone.DNS:Edit + Account.Cloudflare Tunnel:Edit）。二选一，实现开始前提供。

## 4. 安全规格（威胁模型，R5 重写）

**鉴权机制不变，暴露面扩大且显式接受**：

- 此前数据面 token 物理上不出机器；公网化后 token 进入 URL path，新增泄露渠道包括：CF 边缘访问日志、手机端 SDK 日志/crash 上报/剪贴板/浏览器历史。缓解事实：数据 token 为 `pony_` + 32 hex = **128bit 熵**（M1 生成机制），暴力枚举不可行；API.md 外部访问节写明「token 等同密码，禁入客户端持久日志」。
- 无速率控制窗口期：per-token 限速维持 P1，本里程碑**显式接受**该缺口（个人流量模型 + 熵论证）；出现真实滥用时的升级路径 = CF 免费层 rate limiting rule（dashboard 操作、零代码改动）。
- 最小暴露面：ingress 单规则仅 8899；测试断言 **8899 与 8900 双双监听 127.0.0.1**（R6 补 8899，防未来 bind 改动导致公网直连绕过隧道）。
- 凭据纪律：cert.pem/凭据 JSON 0600 dm 属主不入库不进日志；unit 无 secret（凭据经文件路径引用）；e2e 测试 token 全程零回显（R4）。
- PROXY_SECRET 仍只在服务器内存，隧道不改变该性质。
- **明文过第三方披露（R3）**：TLS 在 CF 终结，LLM prompt/completion 对 Cloudflare 可见——个人项目接受的取舍，ADR-006 显式记录。

## 5. 部署步骤规格（实现清单，按依赖顺序）

1. **安装 cloudflared**（sudo）：添加 pkg.cloudflare.com keyring + apt 源 → `apt install cloudflared`。
2. **预检**：`dig +short access.ponyjob.top` 必须为空（防占位记录使 route dns 失败）；`warp-cli status` 观测记录（须 Disconnected）。
3. **授权 + 建隧道（dm 身份）**：login（§3 配合点二选一）→ `tunnel create pony-access` → 记录 tunnel-id。
4. **落配置（dm 身份）**：渲染 `~/.cloudflared/config.yml`（替换 tunnel-id）→ `tunnel route dns pony-access access.ponyjob.top` → `stat` 断言凭据链 dm/600。
5. **systemd 化（sudo 仅此步）**：安装 `pony-tunnel.service` → daemon-reload → enable --now → is-active 断言 + `systemctl show -p User` 断言 dm。
6. **`scripts/m4_test.sh`** 全绿。
7. **门禁**：m1/m2/m3 回归 + workspace 测试确认不受影响（零代码改动）。

## 6. 测试清单（scripts/m4_test.sh）

> **定位声明（R6）**：本清单是**服务端 smoke**——请求从服务器发起，路径为「服务器→CF 边缘→隧道→服务器」自环，vantage 与手机 4G（另一运营商、另一干扰面）不等价；真实验收锚点是 §7 手动项，不得以本清单全绿推导 M4 达成。
> **定性声明（R4）**：本脚本为**生产变更型脚本**（创建/撤销 token、启停隧道），必须幂等可重跑；首尾 prod_guard 断言生产 pproxy active；不新开监听端口（metrics 19099 为只读观测）。

1. prod_guard 断言生产 pproxy active（首尾）
2. `pony-tunnel.service` active 且 enabled；`systemctl show -p User` = dm
3. DNS 双解析器回归（ADR-003 口径）：`dig @223.5.5.5` 与系统解析均返回 CF 边缘记录
4. 公网闭环三重断言（R1）：
   - 无 token → 精确 `401` + body 恰为 `{"error":"unauthorized"}` + 响应头含 `server: cloudflare`
   - 临时 e2e token + 不存在路由 → 精确 `404` + body 含 `"error":"unknown_route"`——这是网关层确定性输出，只有请求真正穿透隧道抵达 pproxy 才可能产生（替代被否决的"非 401/404 即通过"弱判据，F13 FATAL：CF 隧道故障时的 502/524/530 同样满足弱判据）
   - header 模式 `X-Pony-Token` 同样可用
   - **对照负例**：临时 stop 隧道 → 同一请求命中 `5xx` 且不再含 unknown_route → start 后正例恢复（证明判据真能分辨通/不通）
5. e2e token 纪律（R4）：trap EXIT 强制 revoke（异常退出也兜底）；revoke 后用原 token 复验 401 闭环；token 全程变量引用零 stdout 回显
6. 管理面/监听隔离：`ss` 断言 **8899 与 8900 均 127.0.0.1**
7. 隧道韧性：restart 后 **60s 轮询窗口**内恢复 active 且三重断言重新通过（10s 过硬易 flaky，F15）
8. 凭据权限：cert.pem/凭据 JSON 属主 dm 权限 600
9. 环境幂等终检：DNS route 存在、e2e token 已清（列表不含）、隧道 enabled
10. （非阻断观测）`warp-cli status` 输出记录进日志，Disconnected 时 warn 提示红线

## 7. 手动验收（用户执行）

- 手机（4G/热点，非家庭 WiFi）配置 base_url 实调成功——ROADMAP 原文口径
- **streaming 长请求场景（R3/F4）**：流式对话持续 >30s 不断流（验证 CF 边缘对流式的放行；同时确认非流式 >100s 请求会 524——平台限制，文档明示不受支持）
- （可选）国内蜂窝网络延迟体感记录，供归档备注

## 8. 风险与缓解

| 风险 | 缓解 |
|------|------|
| **CF 免费层平台限制**（R3 新增）：边缘等待 ~100s 即 524、请求体 ~100MB 上限，Tunnel 不豁免 | API.md 外部访问节明示「>100s 非流式请求不受支持」；长任务走 streaming；§7 手动验收覆盖 |
| **WARP 共存冲突**（R3/F1）：warp-cli connect 会把 cloudflared 出站连接卷入 WARP 隧道 | 运维红线写入部署文档：本机 WARP 与 pony 隧道互斥；§6.10 观测项 |
| CF Tunnel 免费额度政策变化 | TECH_DESIGN 已登记兜底：局域网模式始终可用；卸载程序见 §9 回滚步骤 |
| access 子域名命中新的国内 DNS 过滤词 | ADR-003 中性命名预判 + §6.3 双解析器回归门 |
| 协议劣化（QUIC/UDP 被运营商干扰） | 模板钉死 `protocol: http2`（TCP），不依赖隐式默认（F5） |
| cloudflared 单点故障 | systemd Restart=always/RestartSec=5 + §6.7 恢复断言；pproxy 局域网路径不受影响 |
| 公网扫描/滥用 | token 128bit 熵不可枚举 + 401 同体；限速缺口**显式风险接受**（§1/§4），滥用升级路径 = CF rate limiting rule |
| 用户授权流程阻塞 | 与 M3-R2 同性质：服务端自动化部分先行，「待授权」状态挂起 |

## 9. 验收标准

- `bash scripts/m4_test.sh` 全绿退出 0，且**连续重跑两次均绿**（生产变更型脚本的幂等口径）
- m1/m2/m3 集成测试无回归
- 代码库产出仅部署资产与文档：`deploy/cloudflared/config.yml` 模板、`systemd/pony-tunnel.service`、`scripts/m4_test.sh`、API.md 外部访问节（含平台限制）、ROADMAP M4 ✅、CURRENT.md 拓扑加隧道分支、**ADR-006**（CF Tunnel 选型 + Tailscale 否决结论 + 明文过 CF 取舍；不写 tailnet 细节）、**docs/ops/DEPLOY.md 回滚章节**（删 route dns → tunnel delete → 摘除 unit → 验证局域网路径完好——公网入口必须有明确关闭程序，F20③）
- 手动验收 §7 由用户完成并在 ROADMAP 归档行标注
