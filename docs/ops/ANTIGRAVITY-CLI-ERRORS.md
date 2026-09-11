# Antigravity CLI（agy）错误图谱与分层诊断手册

> **适用范围**：本机（`devserver`）上 `agy` 经 pproxy 隧道访问 Google Cloud Code 后端
> （`daily-cloudcode-pa.googleapis.com`）时出现的各类报错。
>
> **数据来源**：`~/.gemini/antigravity-cli/log/cli-*.log`（68 个日志文件，2026-09-02 → 09-08）、
> `~/.gemini/antigravity-cli/conversations/*.db`（含底层完整响应载荷与 TraceID）、pproxy 侧实测。
>
> **配套工具**：`scripts/antigravity/diag/`（见 §4）。
>
> **相关记录**：`docs/architecture/decisions/008-cloud-code-compliant-egress.md`（出口合规修复）、
> `docs/ops/DESKTOP-TROUBLESHOOTING.md`（2026-09-08 章节）。

---

## 0. 一页速查

| 看到的错误 | 归因层 | 一句话判定 | 处置 |
|---|---|---|---|
| `400 FAILED_PRECONDITION: User location is not supported` | **L4 Google 地区/资格** | 出口已是美国仍报 → 账号侧 | 改账号地区关联；或换模型（若该模型不触发） |
| `503 UNAVAILABLE: No capacity available for model X` | **L4 Google 容量** | `MODEL_CAPACITY_EXHAUSTED`，与网络无关 | 等/换模型/换时段；报 bug |
| `403 You do not have a valid license` | **L4 许可** | 请求形态/凭据不对 | 检查是否为自造请求；CLI 正常会话不会出现 |
| `403 PERMISSION_DENIED: Verify your account to continue` | **L1 账号验证** | 账号需要验证 | 完成 Google 账号验证 |
| `401 Unauthorized` / `You are not logged into Antigravity` | **L1 认证** | 令牌源缺失 | 重新登录；不影响已认证会话 |
| `proxyconnect tcp: dial tcp 127.0.0.1:8899: connection refused` | **L2 本地代理** | 代理没启动 | `pproxy on` / 重启 `pproxy-server` |
| `... : EOF` | **L2/L3 隧道** | 连接被中途切断 | 看 pproxy 日志与出口自检；本手册 §2.5 |
| `context canceled` / `executor is not currently running` | **L1 CLI 内部** | 请求被取消（多为重试/中断） | 一般忽略 |
| `serializer encountered non-tool step ... CANCELED` | **L1 CLI 内部** | 序列化告警 | 忽略 |
| `Failed to parse skill file ... frontmatter` | **L1 工作区** | SKILL.md YAML 头写错 | 修文件（本机：`.agents/skills/governance-review/SKILL.md`） |

---

## 1. 分层模型：先定位层，再下结论

```
agy (L1 客户端/认证)
  │  http_proxy=http://127.0.0.1:8899
  ▼
pproxy 数据面 (L2 本地代理)          ← CONNECT 白名单 / 端口 / 池化
  │  WS 隧道（Bearer token）
  ▼
gate 隧道出口 (L3)                    ← CF gate / Vercel(vgate) gate
  │  connect() 出站
  ▼
daily-cloudcode-pa.googleapis.com (L4 Google 后端)  ← 地区 / 容量 / 许可 / 账号资格
```

**核心原则**：同一条隧道上的请求，如果一部分成功、一部分失败，那么失败**通常不在 L2/L3**——
先看失败请求的**接口**与**错误码语义**，再决定查哪一层。

---

## 2. 错误清单（按归因层）

### 2.1 L4 · 地区/资格：`400 FAILED_PRECONDITION`

**日志形态**

```
E errorreport.go:224] agent executor error: calling model:
  FAILED_PRECONDITION (code 400): User location is not supported for the API use.
```

**底层载荷**（会话库原文）

```json
HTTP 400 Bad Request
TraceID: 0x6fe09037874aadc5
Headers: { ..., "Server":["ESF"], "X-Cloudaicompanion-Trace-Id":["6fe09037874aadc5"], ... }
{"error":{"code":400,"message":"User location is not supported for the API use.","status":"FAILED_PRECONDITION"}}
```

**统计**：240 次，首见 09-05 12:59，末见 09-08 20:18。

**判定（关键）**：
1. 先确认出口不是本地 ISP：`pproxy status` 与 §4 的出口探针。若出口已是**美国机房 IP** 仍报此错 → 不是 L2/L3。
2. 看**同一隧道**的其它接口是否成功。实测：`loadCodeAssist` / `fetchAvailableModels` 成功而
   `streamGenerateContent` 失败 → 说明是**接口级**校验，不是链路问题。
3. 社区实证：美国出口（两个不同 ASN）+ 账号地区已是 United States 仍报同一错误
   （[官方论坛](https://discuss.ai.google.dev/t/all-gemini-models-fail-with-false-user-location-is-not-supported-in-antigravity-ai-studio-and-claude-work-normally/178242)）。

**处置**：
- 长期：改 Google 账号的国家/地区关联（https://policies.google.com/country-association-form ），
  全程保持目标地区出口，等 Google 邮件确认。
- 短期：换模型族试（历史上 Claude 曾可绕过；但 09-08 当天 Claude 撞上 503，见 §2.2）。
- 向 Google 报 bug 时附：Trajectory ID、TraceID、UTC 时间。

### 2.2 L4 · 容量：`503 UNAVAILABLE` / `MODEL_CAPACITY_EXHAUSTED`

**日志形态**

```
E run.go:371] Run: attempt 1 failed (UNAVAILABLE (code 503): No capacity available for model
  claude-opus-4-6-thinking on the server), retrying in 1s
E errorreport.go:224] calling model: UNAVAILABLE (code 503): No capacity available for model ...
```

**底层载荷**

```json
{"error":{"code":503,
  "message":"No capacity available for model claude-opus-4-6-thinking on the server",
  "status":"UNAVAILABLE",
  "details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo",
    "reason":"MODEL_CAPACITY_EXHAUSTED",
    "domain":"cloudcode-pa.googleapis.com",
    "metadata":{"error_number":"2010","model":"claude-opus-4-6-thinking"}}]}}
```

**统计**：80 次，首见 09-05 15:25，末见 09-08 20:20。**跨模型**出现：
`gemini-2.5-flash-lite`（最多，含输入意图识别）、`gemini-3.1-flash-lite`、
`claude-sonnet-4-6`、`claude-opus-4-6-thinking`。

**判定**：`reason=MODEL_CAPACITY_EXHAUSTED` 是 Google 侧容量池打满的**语义化错误**，
与出口、隧道、代理无关。判据：同一模型在不同小时的成功率波动，且换模型后错误码随之改变。

**处置**：等待/重试；换非 thinking 版本；避开高峰。社区同题：
[claude-opus-4-5-thinking 503](https://discuss.ai.google.dev/t/unavailable-code-503-no-capacity-available-for-model-claude-opus-4-5-thinking-on-the-server/119410/2)、
[Ultra 套餐 503 持续一周](https://discuss.ai.google.dev/t/google-antigravity-http-503-model-capacity-exhausted-on-ultra-plan-for-1-week/143349)。

### 2.3 L4 · 许可与账号验证

| 错误 | 含义 | 备注 |
|---|---|---|
| `403 You do not have a valid license of this product` | 请求被判定为需要企业许可 | 手写探针请求（缺 CLI 的请求形态）会命中；正常 CLI 会话不出现 |
| `403 PERMISSION_DENIED: Verify your account to continue.` | 账号需验证 | 121 次，末见 09-05 |
| `loadCodeAssist` 响应里的 `ineligibleTiers[].reasonCode=UNSUPPORTED_CLIENT` | free-tier 已不再支持该客户端 | 属账号/客户端资格变更，参考用 |

### 2.4 L1 · 认证与令牌

| 错误 | 次数 | 说明 |
|---|---|---|
| `error getting token source: You are not logged into Antigravity` | 4781 | 后台组件（experiments 轮询、quota）拿不到令牌源；**已认证会话仍可正常对话**，属噪声 |
| `failed to fetch user info: 401 Unauthorized` | 19 | 同上 |
| `Cache(...): Singleflight refresh failed: error getting token source` | 若干 | 上述噪声的缓存层表现 |

**判定**：若**模型调用本身**成功，这类错误可忽略；若全部失败且伴随 401，才需重新登录。

### 2.5 L2/L3 · 代理与隧道

| 错误 | 次数 | 归因 | 处置 |
|---|---|---|---|
| `proxyconnect tcp: dial tcp 127.0.0.1:8899: connect: connection refused` | 173 | 代理未启动（`pproxy off` 或服务停了） | 启动代理/服务 |
| `Post "…/v1internal:<接口>": EOF` | **34978** | 连接中途被切断 | 见下方专项 |
| `dial tcp [2001:4860:...]:443: i/o timeout` | 若干 | **未设代理时的直连**（IPv6） | 确认 `http_proxy` 已设 |

**EOF 专项（最大一类，值得单独看）**

按接口分布：`fetchAvailableModels` 19647、`loadCodeAssist` 11845、`fetchUserInfo` 1864、
`listExperiments` 82、`streamGenerateContent` 仅 2。首见 09-02，末见 09-08。

按小时（09-08）：13 时 1547 → 14 时 1034 → 15 时 101 → 16 时 7 → 17 时 64 → 18 时 161 → 19 时 6 → 20 时 1。

**要点**：
- EOF 集中在**元数据接口**，模型推理接口几乎不出现 → 更像短连接/重试路径被切断，而非长流问题。
- 09-08 18:58 / 19:18 两次代理升级（含 gate worker 上游 EOF 关闭修复）之后，19–20 时降到个位数；
  但 15 时之前也已明显下降，**不能单独归因于该修复**，需要更长观察窗口（见 §7）。

### 2.6 L1 · CLI 内部噪声（一般忽略）

| 错误 | 次数 | 说明 |
|---|---|---|
| `serializer encountered non-tool step … CORTEX_STEP_STATUS_CANCELED` | 388 | 会话被取消时的序列化告警 |
| `executor is not currently running` | 21 | 重试/中断竞态 |
| `error during input detection model call: … context canceled` | 50 | 用户中断或重试导致 |
| `Failed to parse skill file … frontmatter: yaml: line 2` | 若干 | 本机 `.agents/skills/governance-review/SKILL.md` YAML 头语法错，**需修文件** |
| `skipping component during resolution: empty component: prompt section "mcp_servers"` | 若干 | 未配置 MCP，正常提示 |

---

## 3. 判定决策树（照着走）

```
报错发生在 agy
├─ 错误码 400 location ────────────────► 先量出口（§4.2）；出口=美国仍报 → L4 账号/地区（§2.1）
├─ 错误码 503 no capacity ─────────────► L4 容量（§2.2），与网络无关，等待/换模型
├─ 错误码 403 license/permission ──────► 区分：自造请求（正常）还是账号验证（§2.3）
├─ 401 / not logged in ────────────────► 看模型调用是否成功：成功=噪声（§2.4）
├─ proxyconnect refused ───────────────► L2：代理/服务未启动
├─ EOF ────────────────────────────────► 看接口：元数据接口=§2.5 专项；模型接口=查隧道寿命
└─ 其它 ───────────────────────────────► 先 grep 日志归类（§4.1），再进对应小节
```

**通用第一步（永远先做）**：把"底层真实响应"抠出来，而不是只看 CLI 的
`Agent execution terminated due to error.` 与 `Error ID`（后者只是
`<trajectory_id>-<步号>` 的本地关联 ID，不是错误码）。

```bash
# 从会话库直接看完整载荷（含 TraceID / HTTP 头 / JSON error）
python3 scripts/antigravity/diag/extract_error_payload.py ~/.gemini/antigravity-cli/conversations
```

---

## 4. 诊断工具箱

脚本位于 `scripts/antigravity/diag/`（详见该目录 README）：

| 脚本 | 用途 | 依赖 |
|---|---|---|
| `extract_error_payload.py` | 从会话库提取底层完整错误载荷 | 无（stdlib） |
| `cloudcode_probe.py` | 用 CLI 令牌直调 Cloud Code 各接口，打印错误类别 | 无（stdlib） |
| `pproxy_egress_sampler.py` | 采样 pproxy 到各出口的字节增量，判定某次请求走了哪条出口 | 无（stdlib） |
| `gate_exitip.mjs` | 经 gate 隧道量出站 IP/国家 | `ws` |
| `gate_bind_probe.py` | 探测 gate 对某 host 的 bind 判定（含 `unsupported_egress:*`） | `websockets` |

### 4.1 日志归类（先做）

```bash
cd ~/.gemini/antigravity-cli/log
grep -h "errorreport.go\|run_command_handler.go" *.log \
 | sed -E 's/.*(errorreport|run_command_handler)\.go:[0-9]+\] //' \
 | sed -E 's/0x[0-9a-f]+/TRACE/g; s/[0-9a-f-]{36}/UUID/g' \
 | sort | uniq -c | sort -rn | head -20
```

### 4.2 出口核验（判 L2/L3 是否清白）

```bash
pproxy status                                   # 正向出海连通性
curl -s https://gate.example.com/debug/egress \
  -H "Authorization: Bearer $TUNNEL_TOKEN"      # CF gate 自身出站 IP/国家/是否合规
python3 scripts/antigravity/diag/pproxy_egress_sampler.py 60   # 实时看流量落在哪条出口
```

### 4.3 排除"绕过代理"

```bash
for p in $(pgrep -x agy); do ss -tnp | grep "pid=$p," | awk '{print $5}' | grep -v '^127\.'; done
# 输出为空 = 只连本地代理，没有直连泄漏
```

---

## 5. 已知"看着像故障其实不是"

1. **`Error ID: <uuid>-<N>`** —— 本地关联 ID（轨迹 + 步号），不是错误码。同一轨迹重试时 N 递增。
2. **`You are not logged into Antigravity`（数千次）** —— 后台组件噪声，不影响已认证会话。
3. **`api.github.com/zen` 返回 403** —— GitHub 对**共享出口 IP** 的未认证限流；隧道本身正常
   （CONNECT 已 200）。CLI 的 `pproxy status` 曾把它误报成"未配置 Gate 隧道出口"，已于 0.3.38 修。
4. **`403 You do not have a valid license`** —— 手写 curl 探针会命中，正常 CLI 会话不会。
5. **`empty component: prompt section "mcp_servers"`** —— 未配置 MCP 的正常提示。
6. **`serializer encountered non-tool step`** —— 取消操作时的告警。

---

## 6. 实战时间线（2026-09-08）

| 时间 | 事件 | 结论 |
|---|---|---|
| 上午 | agy 反复 `400 location`，99+ 次 / 12 个会话 | 底层为 Google 地区限制 |
| 15:46–19:18 | 排查 pproxy：发现数据面固定 CF 优先、入站 colo 门禁管不到出站 IP、gate worker 上游 EOF 不关闭 WS | 三项均已修（ADR-008） |
| 19:18 | 数据面升级 0.3.38；19:0x–19:1x CF worker 重新部署（此前线上版本停在 08-31，A/C 从未上线） | 出口合规门禁生效 |
| 19:34 / 20:01 / 20:15 | **仍报 400 location** | 实测出口已是 vgate `3.88.192.43`（US/AS14618）→ 排除出口 |
| 19:45–20:16 | 字节差分采样确认模型流量（单次上行 1.7–4 MB）走 vgate；agy 无直连泄漏 | 链路清白 |
| 20:20 | 切 Claude 后报 `503 MODEL_CAPACITY_EXHAUSTED` | 第二个 Google 侧故障 |

**最终归因**：两类错误都在 L4（Google 侧），L1–L3 已逐层排除。

---

## 7. 未决 / 待观察

1. **EOF 是否随 gate worker 的"上游 EOF 关闭"修复而下降** —— 需要 ≥24h 的对比窗口
   （09-08 19–20 时已降到个位数，但 15 时前也已下降，无法单独归因）。
2. **账号地区关联修改后的效果** —— 若执行 `country-association-form`，记录修改时间与前后
   location 错误频次。
3. **503 容量是否与时段/模型相关** —— 建议按小时统计各模型成功率，形成"可用窗口"参考。
4. **`governance-review/SKILL.md` frontmatter 解析失败** —— 待修（工作区文件，非本仓库）。
5. **`/usr/local/bin/agy`（1.1.22，root 所有）** —— 陈旧副本，PATH 优先级低于 `~/.local/bin/agy`
   （1.1.27），无害；如需清理需 sudo。

---

## 附录 A：一次性把证据收齐

```bash
# 1) 底层载荷（含 TraceID）
python3 scripts/antigravity/diag/extract_error_payload.py ~/.gemini/antigravity-cli/conversations 20
# 2) 错误归类与频次
cd ~/.gemini/antigravity-cli/log && grep -h "errorreport.go" *.log | sed -E 's/.*go:[0-9]+\] //' | sort | uniq -c | sort -rn | head
# 3) 出口与链路
pproxy status && pproxy doctor
# 4) 失败瞬间的出口归属
python3 scripts/antigravity/diag/pproxy_egress_sampler.py 300   # 期间触发一次失败
```

## 附录 B：向 Google 报 bug 时给什么

- Trajectory ID（会话库里的 UUID）与 Error ID
- `X-Cloudaicompanion-Trace-Id` / `TraceID`
- UTC 时间戳（会话库 Header 里的 `Date`）
- 模型 id（如 `claude-opus-4-6-thinking`）与 `error_number`（容量类为 `2010`）
- 出口国家/ASN（用于自证"不是地区问题"）
