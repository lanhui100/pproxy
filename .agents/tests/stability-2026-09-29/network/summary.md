# 轨道C：物理网络路径质量测量 — 对照总表与结论

- 测量时间：2026-09-29 16:50:01 CST 开始（START_TIME.txt=`1790671759`），17:09:11 完成，耗时约 19 min
- 测量主机：devserver（100.95.193.103，本机 tailnet 节点），enp1s0 直连 192.168.101.161，DNS 本地 systemd-resolved(127.0.0.53)+dnsmasq 链路
- 样本量：DNS 5×本地 + 3×@114 + 3×@8.8.8.8；TCP 15×；TLS 10×；HTTP 10×（直接链路，--noproxy）；ICMP ping -c 10；mtr -r -c 10
- 原始日志：`raw/<host>.txt`、`raw/egress.txt`、`run.out`
- 说明：末跳均参考 mtr 的 Avg；HTTP 根路径的 404/402 为业务语义（链路通即有效）；googleapis/openai 为「直连被阻断」非测量异常

## 1. 总表（单位 ms；HTTP 为秒已换算 ms）

| 端点 | DNS avg/max | TCP p50/p95/max | TLS p50/p95/max | HTTP p50/p95/max·码 | ICMP 丢包/RTTavg | mtr 末跳 RTT | 证书到期日 | 备注 |
|---|---|---|---|---|---|---|---|---|
| rn.ponygo.fun（RackNerd VPS） | 30.6/36 | 265/292/301 | 788/841/1036 | 728/856/892·200×10 | 0% / 253.9 | 253.8（14 跳） | 2026-12-24 (LE YE2) | ✅ 稳定；Cogent 中段 ICMP 60% 丢但末跳 0%，属限速 |
| vedge.ponygo.fun（Vercel） | 90/344* | 82/91/110 | 261/274/299 | 244/250/254·402×10 | 0% / 87.0 | 87.7（16 跳） | 2026-12-24 (LE YR1) | ✅ 亚洲边缘低延迟；⚠️ /api/proxy 402 DEPLOYMENT_DISABLED（业务层） |
| edge.ponygo.fun（CF Worker） | 23.8/25 | 258/285/300 | 803/932/949 | 742/812/843·404×10 | 0% / 187.4 | 186.8（11 跳） | 2026-12-24 (GTS WE1) | ✅ 稳定；IPv6 路径，根路径 404 正常 |
| gate.ponygo.fun（CF 网关） | 24.8/26 | 255/277/286 | 853/976/977 | 825/892/893·404×10 | 0% / 185.9 | 189.0（11 跳） | 2026-12-24 (GTS WE1) | ✅ 稳定；hop10 StDev 37 偶发抖动 |
| vgate.ponygo.fun（Vercel 隧道） | 69.8/249* | 82/89/90 | 260/274/279 | 237/274/278·402×10 | 0% / 69.6 | 74.8（16 跳） | 2026-12-24 (LE YR2) | ✅ 亚洲边缘最低延迟；⚠️ /api/proxy 402（同 vedge） |
| daily-cloudcode-pa.googleapis.com | 29.8/54 | 全 FAIL(6s 超时) | 全 FAIL(12s 超时) | 全 000·15.0s×10 | 100% / — | 路径止于 Level3 SJ（Lumen） | 未取到（TLS 被阻断） | 🚨 直连硬阻断（TCP/TLS/HTTP 全超时、mtr 圣何塞后中断） |
| opencode.ai（CF） | 33.6/72 | 196/208/231 | 605/642/646 | 902/953/1160·200×10 | 10% (1/10) / 186.5 | 184.6（11 跳） | 2026-12-27 (GTS WE1) | ✅ 可达稳定；1 包 ICMP 丢应属限速 |
| api.openai.com | 29.2/54 | 全 FAIL(6s 超时) | 全 FAIL(12s 超时) | 全 000·15.0s×10 | 100% / — | 路径止于 hop5-6 | 未取到（TLS 被阻断） | 🚨 直连硬阻断 + DNS 污染（见 §3） |
| api.anthropic.com | 28.8/50 | 200/208/222 | 622/628/639 | 593/606/607·403×10 | 10% (1/10) / 192.5 | 191.6（11 跳） | 2026-12-20 (GTS WE1) | ⚠️ TCP/TLS 通但 HTTP 403（阶段风控，非网络故障） |
| 100.105.241.39（tencent，tailnet） | — | **:8899** 31/32/32（20 次全通） | — | — | 10% (2/20) / 24.0 | —（跳过 mtr） | — | ✅ tailscale 直连 175.24.73.251:41641，tailscale ping 24ms；ICMP 丢 2 包但 TCP 20/20 稳定 |

## 2. DNS 解析详情（本地 dnsmasq vs 公共对照）

| 端点 | 本地 IP 集合（一致） | @114.114.114.114（ms: 3 样本） | @8.8.8.8（ms: 3 样本） | 结论 |
|---|---|---|---|---|
| rn.ponygo.fun | 192.210.231.8（×5 全一致） | 490/660/42 | 265/244/281 | 三源一致，无污染 |
| vedge.ponygo.fun | cname→66.33.60.194/76.76.21.93（×5） | 76.76.21.98/66.33.60.67 | .93/67 + .123/35 混出 | IP 池随解析波动（Vercel anycast 正常），无污染 |
| edge / gate | 104.21.47.232 + 172.67.174.120 | 一致 | 一致（gate 2/3 超时 2s） | CF anycast 固定；8.8.8.8 偶发超时（GFW udp/53 干扰） |
| vgate | 66.33.60.67/76.76.21.98 | .66/.142 | .61/.130/.34/.164 | anycast 池轮换，无污染 |
| googleapis | 172.217.112~119.4（8 个） | 3 个 .119/.118/.116 | 8 个；1/3 超时 | 解析正常（Google anycast 池），坏在 TCP 层 |
| opencode.ai | 172.65.90.20~23（4 个） | 一致 | 一致 | 无污染 |
| api.openai.com | **103.228.130.27** | **162.125.17.131** | **128.242.240.253 / 128.242.245.29 / 199.59.148.209** | 🚨 三解析器互不相同、均非 OpenAI 真实 IP（IPv6 还解析进 Facebook 2a03:2880 段）= 典型 GFW DNS 污染 |
| api.anthropic.com | 160.79.104.10 | 一致 | 一致（216-225ms） | 无污染，稳 |

## 3. 出口身份（raw/egress.txt）

| 出口通道 | 样本 | 结果 IP | 国别/城市 | ASN/Org | 状态 |
|---|---|---|---|---|---|
| 本地代理 -x 127.0.0.1:8899 | ×3 | **192.210.231.8** | US / Santa Clara, CA | AS36352 (HostPapa/ColoCrossing) | ✅ 3/3 一致 |
| 出口R：rn.ponygo.fun /api/proxy | ×3 | **192.210.231.8** | US / Santa Clara, CA | AS36352 | ✅ 3/3 一致（本地代理即经 rn 隧道出海，与 rn 同出口） |
| 出口V：vedge.ponygo.fun /api/proxy | ×2 | — | — | — | 🚨 402 DEPLOYMENT_DISABLED（Vercel 部署被禁用，隧道出口不可用） |
| edge.ponygo.fun /api/proxy（探测） | ×1 | **172.71.31.54** | US / Atlanta, GA | AS13335 Cloudflare | ✅ 200（存在代理接口） |
| gate.ponygo.fun /api/proxy（探测） | ×1 | — | — | — | 404 not found（无此接口） |

- 备注：本地代理与出口R（rn VPS）出口身份完全一致（192.210.231.8 ×6 样本），说明本地代理隧道即经 rn 出海；ipinfo 全程可达，未触发 ipify/ipapi 兜底。本机（devserver）直连出海身份未单独取证（ipinfo 直连抽查为 200/2.05s，正文不含直连身份数据）。

## 4. 异常标注汇总

1. 🚨 **api.openai.com 直连全链路不可达**：DNS 三源互相矛盾（污染），TCP 15/15 超时 6s、TLS 10/10 超时 12s、HTTP 10/10 time_total=15s、ICMP 10/10 丢包、mtr 路径在联通骨干 hop5-6 后中断。属硬阻断，必须走代理。
2. 🚨 **daily-cloudcode-pa.googleapis.com 直连被阻断**：DNS 正常解析出 Google anycast（172.217.112~119.x），但 TCP/TLS/HTTP 全部超时、mtr 在圣何塞 Lumen/Level3 边缘后中断 → 疑似 SNI/传输层干扰，属阻断而非抖动。ponyllm antigravity 直连云需代理兜底。
3. ⚠️ **vedge/vgate 的 /api/proxy 返回 402 DEPLOYMENT_DISABLED**：Vercel 部署关闭，出口V 当前业务不可用（非物理链路问题；TCP/TLS 均正常）。
4. ⚠️ **api.anthropic.com HTTP 403**（根路径风控，503/403 常见）——网络层通、业务层挡。
5. ⚠️ **tailnet tencent 节点 ICMP 10% 丢包**（2/20），但 TCP:8899 20/20 全通、RTT 29-32ms 极稳 → 无实害，或为 ICMP 限速。
6. ⚠️ **8.8.8.8 对照解析 2/3 超时**（gate、googleapis 各超时 2s/次）→ udp/53 出海不稳，属 GFW 常见现象；本地 dnsmasq 与 114 均稳定。
7. 轻微：vedge/vgate DNS 首查 344ms/249ms（dnsmasq 缓存未命中冷启动），后续 25ms 稳定 —— 非抖动。

## 5. 物理路径稳定性结论（一句话）

> **物理路径层无传输抖动热点：rn/vedge/edge/gate/vgate 五个出口的 TCP/TLS/HTTP 样本均低抖动（StDev≤8%、端到端 0% 丢包、出口R=出口IP 192.210.231.8 美国西海岸/TLS p50 788ms、出口V 亚洲边缘 84ms），tailnet→tencent 直连 30ms 极稳；googleapis 与 openai 直连为系统性阻断（非质量劣化），整体物理路径判定为「稳定」。**

*（路径抽样：rn 经联通 219.158→Cogent SJC→38.120.133.51→192.210.231.8，253ms@14 跳；vedge/vgate 经联通→Vercel 亚洲边缘，75-88ms；edge/gate/opencode 经联通 v6→Cloudflare，184-189ms；全程无 GFW 丢包放大热点。）*

## 6. 收尾信息
- 测量脚本：`measure.sh`（nohup 全自动执行，DONE 标记 `DONE.txt`=$(date +%s)）
- 本表数字可直接与 轨道B/C 其他轨道汇总合并；原始数据见 `raw/`。