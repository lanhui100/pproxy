# tests/acceptance — pproxy 出海链路独立验收套件（Wave 4 / qa）

合同唯一依据：`.dev-team/contract.md`（C1/C2/C3/C4/C5/C6 + 门禁 G1..G8）。
本目录只写测试，不含业务实现；断言一律以合同条款为锚，未自行发明标准。

## 一键运行

```bash
bash tests/acceptance/run.sh                 # 全量：构建 -> 自建 fixture -> 全部用例
bash tests/acceptance/run.sh '^G5'           # 调试：只跑匹配用例（不做完备性校验）
ACCEPT_SKIP_BUILD=1 bash tests/acceptance/run.sh
ACCEPT_KEEP=1 bash tests/acceptance/run.sh   # 保留临时工作区（排障）
```

输出协议（机器可读）：

```
[PASS] <case-id> <描述 + 实测值>
[FAIL] <case-id> <描述 + 实测值>
[SKIP] <case-id> <物理不可达原因>     ← 仅覆盖缺口，须在本文件豁免表登记
... ACCEPT_EXIT=<0|1>                 ← 末行，且进程退出码与之一致
```

失败即非零退出；未执行的必需用例会被 `MISSING-CASE` 兜底判红（不存在"漏跑=放行"）。

## 零外部凭据的 fixture 自建

`run.sh` 自己起一个 gate-server，全部凭据本地生成：

| 环境变量 | 值 | 用途 |
| --- | --- | --- |
| `HOST` / `PORT` | `127.0.0.1:<随机空闲端口>` | axum 管理/订阅面（C5） |
| `GATE_CONNECT_LISTEN` | `127.0.0.1:<随机空闲端口>` | C1 CONNECT 数据面开关 |
| `TUNNEL_TOKEN_HASH` | `sha256(accept-fixture-token-9f2c)` | C1/C5 令牌校验路径 |
| `PROXY_SECRET` / `GATE_ADMIN_TOKEN` | 本地 fixture 串 | 避免网关起不来 |
| `HOME` | 临时目录 | 隔离 `~/.pony/revoked_tokens.txt` 等副作用 |

CLI 用例同样用临时 `HOME` 承载 `config.toml` / `clash.yaml`，`admin` 端点指向
`127.0.0.1:1`（立即 ECONNREFUSED），因此**不需要任何外网凭据**。
`curl` 一律带 `--noproxy ''`，防止宿主 `no_proxy` 让回环目标绕过代理造成假绿。

## 覆盖矩阵

| 用例 ID | 合同条目 | 断言要点 |
| --- | --- | --- |
| `HARNESS-SELFTEST` | —— | 套件自检：辅助脚本非零退出必须被透传（防 false-green） |
| `G1` | C1 / G1 | Basic(user=token) 经 gate CONNECT 出网，目标 `generate_204` → 204 |
| `G1b` | C1 | `X-Pony-Token` 同权限出网 → 204 |
| `G2` | C1 / G2 | 无凭据 → 407 且带 `Proxy-Authenticate: Basic realm="pproxy"` |
| `G2b` | C1 | 错误凭据 → 407（防"任意凭据放行"） |
| `G3` | C1 / G3 | `example.com:80` → 403（仅 443 放行） |
| `C1-ACL` | C1 | `127.0.0.1:443` → 403（`is_valid_host` 拒回环/私网） |
| `G4` | C2 / G4 | 生产端到端 `https://rn.ponygo.fun:8443` → `https://www.gstatic.com/generate_204` = 204（**默认 SKIP**，仅设 `PPROXY_E2E_TOKEN` 时执行） |
| `C1-BOUNDED` | C1（演进） | 不可达/被拒目标（`192.0.2.1:443`、`192.88.99.1:443`）→ 码 ∈ {403,502} **且** ≤30s 内返回（性质是"不无限挂起"） |
| `C1-ACL-ENCODINGS` | C1（演进） | ACL 编码绕过族 12 变体全部 403：`0.0.0.0`、`2130706433`、`0x7f000001`、`[::ffff:127.0.0.1]`、`[::]`、`[0:0:0:0:0:0:0:1]`、`100.64.0.1`、`[fe80::1]`、`10.`、`172.16.`、`192.168.`、`[fc00::1]` |
| `G5a` | C5 / G5 | `/api/clash?token=probe` → 401 + body `invalid token` |
| `G5b` | C5 | `/clash` 缺令牌 → 401 + body `missing token` |
| `G5c` | C5 | 有效令牌 → 200 + `subscription-userinfo` / `profile-update-interval` / `content-disposition` |
| `G5d` | C5 | `/clash` 别名端点同口径 → 200 |
| `HEALTH-gate` | C3（演进） | 组自检 `url: "https://www.gstatic.com/generate_204"`，全文无 `http://cp.cloudflare.com` |
| `G6-gate` | C3 / G6 | gate 生成器：`port: 8443`、`username`/`password`=请求令牌、无 `Authorization: Bearer` |
| `G7-gate` | C6 / G7 | 尾部 `- MATCH,PROXY`、`GEOIP,CN,DIRECT` 紧邻其前、5 个探活域+内网 CIDR 在全 PROXY 之前 |
| `G6-cli` | C3 / G6 | CLI 生成器同口径（`~/.pony/clash.yaml` 或 stdout 物理产物） |
| `HEALTH-cli` | C3（演进） | CLI 组自检 URL 同口径 |
| `G7-cli` | C6 / G7 | CLI 分流尾部同口径 |
| `G6-cli-failfast` | C3 末条 | token 缺失 → 非零退出，且绝不产出 `username: ""` 空凭据 |
| `G8-static` | C4 / G8 | 桌面端源码级：8899 / `PPROXY_LAN_PORT` / `0.0.0.0` / 407+质询头 / 回环豁免 / lib.rs 接线 / `allow-lan: false` |
| `G6-desktop-static` | C3 | 桌面端 `build_clash_meta_yaml` 函数体内同口径 + 生成器返回 `Result`（fail-fast） |
| `HEALTH-desktop-static` | C3（演进） | 桌面端 `build_clash_meta_yaml` 体内组自检 URL 同口径 |
| `G7-desktop-static` | C6 | 同函数体内规则尾部与探活域顺序 |
| `AT1a` | L2-AT 超时 | 半截首部后静默 → 服务端在 30s+5s margin 内有界关闭 |
| `AT1b` | L2-AT 超时 | 连上后零字节 → 同上（首字节窗口） |
| `AT2a` | L2-AT 畸形 | 单行 24 KiB 请求行 → 4xx/关闭，进程存活，RSS 增量 < 64 MiB |
| `AT2b` | L2-AT 畸形 | 总首部 ~40 KiB → 同上 |
| `AT3` | L2-AT 竞态 | 固定 20 并发 × 3 轮（`Barrier` 同步起跑）→ 无死锁、全部有界返回、压后仍 204 |
| `META-ASSERT-SENSITIVITY` | L2-AT 反向控制 | 对真 YAML 做 port443 / Bearer / MATCH,DIRECT 三处变异，断言必须变红 |
| `META-AT1-NEGCTRL` | L2-AT 反向控制 | 对"永不回包"哑服务，`AT1` 判定必须报红（证明判定非恒真） |

## 豁免与未覆盖项（如实登记）

| 项 | 状态 | 理由 |
| --- | --- | --- |
| `G4` 公网 `rn.ponygo.fun:8443` 端到端 | **条件执行（默认 SKIP）** | 仅在设置环境变量 `PPROXY_E2E_TOKEN` 时运行；令牌只存于内存与 `curl -K -`（stdin），不落盘、不打印、不进 argv。未设置时输出 `[SKIP] G4 需 PPROXY_E2E_TOKEN`，不计入 FAIL |
| `C2` nginx `stream` 块 / `nginx -t` | 未覆盖 | 该配置在 VPS 上，仓库内无 `deploy/nginx/`；本地无 root 无法执行 `nginx -t` |
| `G8` 运行态（`ss -tlnp` / 非回环 407） | 降级为源码级 | 桌面端为 Tauri GUI crate，无显示环境不可运行；且单机回环 fixture 的源 IP 恒为 127.0.0.1，C4 明确"回环免鉴权"，非回环路径不可构造 |
| 桌面端运行时 YAML | 降级为源码级 | Tauri 构建图外（任务书：qa 不跨构建图验证桌面端，由 W3 自证） |
| C1 配额记账（`user_used_bytes` 按 `claims.sub` 累计） | 未覆盖 | 需 `usr_live_` 自包含令牌 + Ed25519 签发密钥 fixture；属 Wave 5 可扩展项 |
| Clash 客户端真实加载/分流行为 | 未覆盖 | 需 Clash Meta 客户端环境，属端到端外环，超出本套件边界 |

## 断言器（helpers/）

| 文件 | 作用 |
| --- | --- |
| `helpers/raw_probe.py` | `halfopen` / `oversize` / `concurrent` 协议级探针；阈值由用例传入，退出码表达"是否取得合同要求的观察" |
| `helpers/yaml_contract.py` | `fields`（C3）/ `rules`（C6）YAML 结构断言，输出单行实测明细 |
| `helpers/desktop_static.py` | 作用域限定在 `fn build_clash_meta_yaml` 体内，避免无关 `Authorization` 误判 |

## 纪律

- 固定迭代次数 + 固定超时预算，无随机 sleep、无概率重试；轮询仅用于"等服务拉起"。
- 每条断言失败必须带实测值与片段；辅助脚本退出码必须透传（`run_helper`，有自检用例）。
- 变异/负控制全部发生在临时目录副本上，绝不写仓库文件；套件自建进程在 `EXIT` trap 中回收。

## 契约演进记录（2026-10-30 Lead 裁决，本套件已迁移）

1. **C1 ACL 补充条款**：CONNECT 目标必须是**解析后的公网 IP**；loopback / private / link-local /
   unspecified / multicast / CGNAT `100.64.0.0/10` / 文档段（`192.0.2.0/24`、`198.51.100.0/24`、
   `203.0.113.0/24`、`2001:db8::/32`）/ `fc00::/7` / `fe80::/10` 一律 `403`。
   背景：L3 安全审查 BLOCKING —— 旧实现是字符串前缀黑名单，`0.0.0.0`、`2130706433`、
   `0x7f000001`、`[::ffff:127.0.0.1]` 等编码可绕过内网拦截。
   黑盒钉：`C1-ACL`（回环）+ `C1-ACL-ENCODINGS`（12 变体）。
2. **`C1-502` → `C1-BOUNDED`**：文档段目标现按新 ACL 返回 403，502 不再是唯一正确码；
   断言改为「状态码 ∈ {403,502} 且 ≤30s 内返回」——钉的性质是**有界返回**，不是具体码。
3. **组自检 URL**：三处生成器的 `proxy-groups[].url` 必须为
   `https://www.gstatic.com/generate_204`，全文不得出现 `http://cp.cloudflare.com`
   （原明文 80 会让所有节点健康检查恒红）；订阅 200 响应必须带 `cache-control: no-store`。
