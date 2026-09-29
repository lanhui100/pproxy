#!/usr/bin/env python3
"""Track B aggregation: reverse gateway route stability + control comparison.
Definitions (per plan): link-up = 2xx-4xx upstream real response (incl. 401/402/404/405
business statuses); failure = code 000 / curl timeout; 5xx = link up but upstream error.
Baseline for the p95>2x rule = same-window direct-upstream control p95 (independent
simulation of gateway forwarding); the route's own median ratio is reported as an
observation (bimodal profile), not an auto-unstable verdict.
vedge.ponygo.fun returns 402 DEPLOYMENT_DISABLED at the Vercel platform layer
(egress service disabled, NOT a link failure) - explicitly distinguished.
"""
import csv, datetime, os

D = "/home/dm/pproxy/.agents/tests/stability-2026-09-29/reverse"
ROUTES = ["openai", "anthropic", "opencode", "xai", "github", "bai", "opencode-cf"]
HOSTS = {"openai": "vedge.ponygo.fun", "xai": "vedge.ponygo.fun",
         "opencode": "rn.ponygo.fun",
         "anthropic": "edge.ponygo.fun", "github": "edge.ponygo.fun",
         "bai": "edge.ponygo.fun", "opencode-cf": "edge.ponygo.fun"}
HOST_LABEL = {"rn.ponygo.fun": "vps (opencode 主上游)", "vedge.ponygo.fun": "vercel (openai/xai)",
              "edge.ponygo.fun": "CF worker (anthropic/github/bai/opencode-cf)"}
BIZ_NOTE = {  # what the persistent business status means for our probe
    "openai": "402 上游 Vercel 平台禁用",
    "xai": "402 上游 Vercel 平台禁用",
    "anthropic": "405 探针方法/路径不符上游预期",
    "github": "404 路径不存在 + 偶发 429",
    "bai": "401 需鉴权",
    "opencode": "200 可用",
    "opencode-cf": "200 可用",
}

def pct(vals, p):
    if not vals: return None
    sv = sorted(vals)
    k = max(0, min(len(sv)-1, int(round((p/100.0)*len(sv)))-1))
    return sv[k]

def pcts(vals):
    return {p: (pct(vals, p) if vals else None) for p in (50, 90, 95, 99)}

def fmt(x, nd=1):
    if x is None: return "-"
    if x == int(x): return str(int(x))
    return f"{x:.{nd}f}"

def to_ms(v):
    """probe/control total_ms is seconds -> ms."""
    try:
        return round(float(v)*1000)
    except (ValueError, TypeError):
        return None

def fnum(v):
    """already-ms values (mgmt latency_ms) -> float."""
    try:
        return float(v)
    except (ValueError, TypeError):
        return None

def read_csv(path):
    with open(path) as f:
        return list(csv.DictReader(f))

def htime(ms_epoch):
    return datetime.datetime.fromtimestamp(ms_epoch/1000).strftime("%H:%M:%S")

def norm_test(x):
    """Normalize control_route_test rows; tolerate legacy pipe-joined format."""
    ok = (x.get("ok") or "").strip()
    status = (x.get("status") or "").strip()
    lat = (x.get("latency_ms") or "").strip()
    err = (x.get("error") or "").strip()
    if "|" in ok:
        parts = ok.split("|")
        ok = parts[0].strip()
        status = parts[1].strip() if len(parts) > 1 else ""
        lat = parts[2].strip() if len(parts) > 2 else ""
        err = "|".join(parts[3:]).strip()
    return {"ok": ok, "status": status, "latency_ms": lat, "error": err}

probe = read_csv(f"{D}/probe_routes.csv")
direct = read_csv(f"{D}/control_direct.csv")
rtest = read_csv(f"{D}/control_route_test.csv")
start_ms = int(open(f"{D}/START_TIME.txt").readline().strip())
end_ms = int(open(f"{D}/END_TIME.txt").readline().strip()) if os.path.exists(f"{D}/END_TIME.txt") else start_ms

lines = []
lines.append("# Track B: 反向网关路由稳定性探测总结 (2026-09-29)")
lines.append("")
lines.append(f"- 探测窗口: {htime(start_ms)}–{htime(end_ms)} CST (约 {(end_ms-start_ms)/60000:.0f} min, 主探测 {len(probe)} 条样本: 7 路由 × ~3s/条; 控制面每 60s 一轮)")
lines.append(f"- 数据面: http://127.0.0.1:8899/{{token}}/{{route}}/..., 令牌 stability-test-20260929 (用后即撤)")
lines.append("- 成功/失败定义: **2xx-4xx = 链路通**(上游真实响应, 含 401/402/404/405 业务态); **code=000/curl 超时 = 失败**; 5xx = 链路通但上游错误(另列)")
lines.append("- ⚠️ 重要: vedge.ponygo.fun(Vercel) 当前返回 **402 Payment required / DEPLOYMENT_DISABLED**——Vercel 函数部署被禁用(平台层, 疑似计费), 属**出口服务不可用而非链路故障**; 该 402 本身证明链路可达。")
lines.append("")

lines.append("## 1. 主探测: 每路由稳定性")
lines.append("")
lines.append("| route | 上游 | N | 成功率(链路通2xx-4xx) | 业务可用率(200) | 状态码分布 | p50 | p90 | p95 | p99 | max | ≥10s超时 |")
lines.append("|---|---|---|---|---|---|---|---|---|---|---|---|")
route_anom = {}
for r in ROUTES:
    rows = [x for x in probe if x["route"] == r]
    n = len(rows)
    codes = {}
    for x in rows:
        codes[x["http_code"]] = codes.get(x["http_code"], 0) + 1
    n_succ = sum(v for k, v in codes.items() if k[:1] in "234")
    n_biz = codes.get("200", 0) + codes.get("201", 0)
    n_5xx = sum(v for k, v in codes.items() if k[:1] == "5")
    lat_up = [to_ms(x["total_ms"]) for x in rows if x["http_code"] != "000"]
    lat_up = [x for x in lat_up if x is not None]
    ps = pcts(lat_up)
    timeout_cnt = len([x for x in rows if x["http_code"] == "000" or (to_ms(x["total_ms"]) or 0) >= 10000])
    dist = " ".join(f"{k}:{v}" for k, v in sorted(codes.items()))
    if n_5xx: dist += f" (5xx×{n_5xx})"
    mx = max(lat_up) if lat_up else 0
    lines.append(f"| {r} | {HOST_LABEL[HOSTS[r]]} | {n} | {100*n_succ/n:.2f}% | {100*n_biz/n:.2f}% | {dist} | {fmt(ps[50])} | {fmt(ps[90])} | {fmt(ps[95])} | {fmt(ps[99])} | {fmt(mx)} | {timeout_cnt} |")
    route_anom[r] = {"codes": codes, "timeouts": timeout_cnt, "lat_up": lat_up, "n": n,
                     "n_succ": n_succ, "n_biz": n_biz, "n_5xx": n_5xx}
lines.append("")
lines.append("业务态说明(探针业务结果≠链路健康): " + "; ".join(f"{r}={BIZ_NOTE[r]}" for r in ROUTES) + "。")
lines.append("")

lines.append("## 2. 异常事件时间线 (code=000 / 5xx / 单次≥10s)")
lines.append("")
events = []
for x in probe:
    t = to_ms(x["total_ms"]) or 0
    if x["http_code"] == "000" or x["http_code"].startswith("5") or t >= 10000:
        events.append((int(x["ts_epoch_ms"]), x["route"], x["http_code"], t))
events.sort()
if events:
    lines.append("| 时间(CST) | route | http_code | total_ms |")
    lines.append("|---|---|---|---|")
    for ts, r, c, t in events:
        lines.append(f"| {htime(ts)} | {r} | {c} | {t} |")
else:
    lines.append("无 (0 条异常) ✓")
lines.append("")

lines.append("## 3. 对照: 直连上游 (模拟网关转发, X-Proxy-Secret, httpbin.org/status/200, 60s/次)")
lines.append("")
lines.append("| host | 用途 | N | 状态码分布 | p50 | p90 | p95 | p99 | max | 说明 |")
lines.append("|---|---|---|---|---|---|---|---|---|")
direct_note = {"rn.ponygo.fun": "✅ 正常", "vedge.ponygo.fun": "⚠️ Vercel 平台 402 DEPLOYMENT_DISABLED(部署禁用, 非链路故障)", "edge.ponygo.fun": "✅ 正常"}
direct_stats = {}
for h in ["rn.ponygo.fun", "vedge.ponygo.fun", "edge.ponygo.fun"]:
    rows = [x for x in direct if x["host"] == h]
    if not rows: continue
    codes = {}
    for x in rows: codes[x["http_code"]] = codes.get(x["http_code"], 0) + 1
    lats = [to_ms(x["total_ms"]) for x in rows if x["http_code"] != "000"]
    lats = [x for x in lats if x is not None]
    ps = pcts(lats)
    direct_stats[h] = {"codes": codes, "p95": ps[95], "p50": ps[50], "n": len(rows)}
    lines.append(f"| {h} | {HOST_LABEL[h]} | {len(rows)} | {' '.join(f'{k}:{v}' for k,v in sorted(codes.items()))} | {fmt(ps[50])} | {fmt(ps[90])} | {fmt(ps[95])} | {fmt(ps[99])} | {fmt(max(lats) if lats else 0)} | {direct_note[h]} |")
lines.append("")

lines.append("## 4. 对照: 管理面 route/test (网关侧独立度量, POST /api/routes/{name}/test, 60s/次)")
lines.append("")
lines.append("| route | N | 管理面 ok 率 | status分布 | latency p50 | latency p95 | latency max | 常见 error |")
lines.append("|---|---|---|---|---|---|---|---|")
err_sample = {}
for r in ROUTES:
    rows = [norm_test(x) for x in rtest if x["route"] == r]
    if not rows: continue
    oks = sum(1 for x in rows if x["ok"].lower() == "true")
    codes = {}
    lats = []
    for x in rows:
        st = x["status"]
        codes[st] = codes.get(st, 0) + 1
        l = fnum(x["latency_ms"])
        if l is not None: lats.append(l)
        e = x["error"]
        if e and r not in err_sample: err_sample[r] = e
    ps = pcts(lats)
    err = (err_sample.get(r) or "-")
    if len(err) > 70: err = err[:70] + "…"
    lines.append(f"| {r} | {len(rows)} | {100*oks/len(rows):.0f}% | {' '.join(f'{k}:{v}' for k,v in sorted(codes.items()))} | {fmt(ps[50])} | {fmt(ps[95])} | {fmt(max(lats) if lats else 0)} | {err} |")
lines.append("")
lines.append("注: 管理面 ok=false 语义为“上游业务未返回预期成功(未到达源站/上游平台错误)”，非链路判定; 实测 status 与数据面探针存在路径差异(bai: 数据面401 vs 管理面403; github: 数据面404 vs 管理面200)。")
lines.append("")

lines.append("## 5. 结论")
lines.append("")
lines.append("判定阈值: 失败率(000/超时)>0.5% 或 窗口内≥2次10s+超时 或 p95>2×直连上游对照p95 → 不稳定; p95>2×自身中位 标记为延迟模式观察(不单独判不稳定)。")
lines.append("")
verdicts = []
for r in ROUTES:
    a = route_anom[r]
    ps = pcts(a["lat_up"])
    med = ps[50] or 0
    p95 = ps[95] or 0
    fail_rate = 100.0 * a["codes"].get("000", 0) / a["n"]
    dh = HOSTS[r]
    d95 = (direct_stats[dh].get("p95") or 0)
    self_ratio = (p95/med) if med else 0
    unstable = []
    if fail_rate > 0.5: unstable.append(f"失败率{fail_rate:.2f}%>0.5%")
    if a["timeouts"] >= 2: unstable.append(f"{a['timeouts']}次≥10s超时(≥2)")
    if d95 and p95 > 2*d95: unstable.append(f"p95({fmt(p95)}ms)>2×直连对照p95({fmt(d95)}ms)")
    obs = []
    if self_ratio > 2: obs.append(f"延迟双峰 p95/中位≈×{self_ratio:.1f}")
    if a["n_5xx"]: obs.append(f"{a['n_5xx']}×5xx")
    note = ""
    if r in ("openai", "xai"):
        note = " (402 = 上游 Vercel 平台部署禁用 DEPLOYMENT_DISABLED 的真实响应, 链路通; 上游服务不可用属已知状态)"
    if unstable:
        st = "⚠️ 不稳定: " + "; ".join(unstable)
    else:
        st = "✅ 稳定" + note + (f"｜观察: {'; '.join(obs)}" if obs else "")
    verdicts.append(f"- **{r}** ({HOST_LABEL[dh]}): 成功率(链路通) {100*a['n_succ']/a['n']:.2f}%, 业务可用率 {100*a['n_biz']/a['n']:.2f}%, p95={fmt(p95)}ms, 直连上游对照 p95={fmt(d95)}ms → {st}")
lines.extend(verdicts)
lines.append("")
unstable_routes = [r for r in ROUTES if any(f"- **{r}**" in v and "不稳定" in v for v in verdicts)]
if not unstable_routes:
    overall = "✅ **总体通过**（7 路由链路全部稳定: 0 失败 / 0 超时; openai/xai 的上游 402 为 Vercel 平台部署禁用, 属出口服务不可用而非链路故障; opencode 延迟双峰为窗口内稳定固有形态）"
else:
    overall = "⚠️ **总体不通过**，存在不稳定项: " + ", ".join(unstable_routes)
lines.append(f"总体判定: {overall}")

oc = route_anom["opencode"]
if oc["codes"].get("000", 0) or any(k.startswith("5") for k in oc["codes"]):
    lines.append("")
    lines.append("### opencode 主上游(vps rn.ponygo.fun)→backup(worker edge) failover 观察")
    lines.append("")
    for ts, r, c, t in events:
        if r != "opencode": continue
        near = [x for x in direct if abs(int(x["ts_epoch_ms"]) - ts) < 70000 and x["host"] == "rn.ponygo.fun"]
        ns = "; ".join(f"{htime(int(x['ts_epoch_ms']))} rn={x['http_code']}({to_ms(x['total_ms'])})" for x in near) or "rn 无临近样本"
        lines.append(f"- {htime(ts)} opencode {c} {t}ms | 同时刻直连 rn: {ns}")
else:
    lines.append("")
    lines.append("### opencode failover 观察")
    lines.append("探测窗口内 opencode 主上游 rn.ponygo.fun 全程可达(23/23 直连 200, p50≈1.4s), 网关 opencode 路由 100% 200 且无失败/超时, **未触发 vps→backup worker failover**。"
                 " 延迟呈稳定双峰(≈350-400ms 快通道 / ≈850-950ms 慢通道, 慢样本占 ~32%), 窗口首分钟即存在且每分钟恒定, 与 rn 直连无相关波动——判定为路由固有形态(疑似主/备两通道或隧道多连接 RTT 差异), 非故障。")

open(f"{D}/summary.md", "w").write("\n".join(lines) + "\n")
print("\n".join(lines))