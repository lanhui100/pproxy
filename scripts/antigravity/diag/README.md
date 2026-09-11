# antigravity/diag — Antigravity CLI 故障分层诊断工具

配套手册：`docs/ops/ANTIGRAVITY-CLI-ERRORS.md`。

设计目标：把"CLI 弹的 UI 文案"还原成"哪一层、什么码"，避免在错误的层上排查。
所有脚本**不内置任何密钥**：令牌一律从环境变量或本机既有配置文件读取。

## 脚本

| 脚本 | 作用 | 依赖 |
|---|---|---|
| `extract_error_payload.py` | 从会话库抠出底层完整响应（HTTP 码 / TraceID / JSON error / reason / error_number） | 无 |
| `cloudcode_probe.py` | 用 CLI 令牌直调 Cloud Code 各接口，输出错误类别 | 无 |
| `pproxy_egress_sampler.py` | 采样 pproxy→各出口的字节增量，判定某次请求实际走了哪条出口 | 无 |
| `gate_exitip.mjs` | 经 gate 隧道量出站 IP/国家（证明 Google 看到的来源地区） | `ws`（仓库内已有） |
| `gate_bind_probe.py` | 探测 gate 对某 host 的 bind 判定（含 `unsupported_egress:*`） | `websockets` |

## 典型用法

```bash
# 0) 先看真实错误（永远第一步）
python3 scripts/antigravity/diag/extract_error_payload.py

# 1) 区分"代理层"还是"Google 层"：直调后端看错误类别
python3 scripts/antigravity/diag/cloudcode_probe.py

# 2) 出口是否合规
node scripts/antigravity/diag/gate_exitip.mjs                       # 经 CF gate
node scripts/antigravity/diag/gate_exitip.mjs wss://vgate.example.com/api/ws
curl -s https://gate.example.com/debug/egress -H "Authorization: Bearer $PPROXY_TUNNEL_TOKEN"

# 3) 门禁是否生效（负向验证）
python3 scripts/antigravity/diag/gate_bind_probe.py

# 4) 失败瞬间到底走了哪条出口（期间触发一次失败）
python3 scripts/antigravity/diag/pproxy_egress_sampler.py 300
```

## 令牌来源

| 令牌 | 位置 | 用途 |
|---|---|---|
| CLI OAuth | `~/.gemini/antigravity-cli/antigravity-oauth-token` | `cloudcode_probe.py` |
| 隧道令牌 | 环境变量 `PPROXY_TUNNEL_TOKEN` 或 `~/.pony/config.toml` 的 `tunnel_token` | gate 相关脚本 |

## 判读要点

- **连接存在 ≠ 流量经过**：pproxy 会同时为 CF 与 Vercel 预建待命会话，
  必须看字节增量（`pproxy_egress_sampler.py`）。
- **同一条隧道上部分接口成功、部分失败** → 大概率不是链路问题，而是接口级校验。
- **`Error ID` 不是错误码**：它是 `<轨迹UUID>-<步号>`。
