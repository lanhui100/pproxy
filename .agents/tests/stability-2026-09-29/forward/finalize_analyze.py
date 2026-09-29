#!/usr/bin/env python3
"""Track-A forward-chain stability analysis (self-contained).
Usage: python3 finalize_analyze.py <forward_dir> <logs_dir> <start_epoch_s>
Reads: probe_*.csv, ../logs/probe-failures.log, ../logs/journal-window.txt
Writes: summary.md, tunnel-pool-window.json, journal-errors.txt
"""
import csv, json, math, re, sys
from collections import defaultdict, Counter
from datetime import datetime, timezone, timedelta
from statistics import median

DIR, LOGS, START_S = sys.argv[1], sys.argv[2], int(sys.argv[3])
START_MS = START_S * 1000
CST = timezone(timedelta(hours=8))

HOSTS = {
    "cloudcode": "daily-cloudcode-pa.googleapis.com",
    "google204": "www.google.com",
    "github": "api.github.com",
    "opencode": "opencode.ai",
}
HOST_BY_NAME = {v: k for k, v in HOSTS.items()}

def hms(ms):
    return datetime.fromtimestamp(ms / 1000.0, tz=CST).strftime("%H:%M:%S")

def pct(vals, p):
    if not vals:
        return None
    s = sorted(vals)
    k = (len(s) - 1) * p / 100.0
    lo, hi = math.floor(k), math.ceil(k)
    if lo == hi:
        return s[lo]
    return s[lo] + (s[hi] - s[lo]) * (k - lo)

def load_csv(path):
    rows = []
    with open(path, encoding="utf-8") as f:
        for row in csv.DictReader(f):
            rows.append({
                "epoch_ms": int(row["epoch_ms"]),
                "host": row["host"],
                "http_code": int(row["http_code"]),
                "connect_ms": int(row["connect_ms"]),
                "appconnect_ms": int(row["appconnect_ms"]),
                "total_ms": int(row["total_ms"]),
                "size_bytes": int(row["size_bytes"]),
            })
    return rows

# ---------------- tunnel events from journal window ----------------
TS_UTC_RE = re.compile(r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6}Z)")
EST_RE = re.compile(r"tunnel established host=(\S+) establish_ms=(\d+) pooled=(true|false)")
ADV_RE = re.compile(r"tunnel establish \(allowlist advisory only\) host=(\S+) allowlisted=(true|false)")
ERROR_KW = ("timeout", "denied", "error", "retry", "fail", "refused", "closed", "reconnect")

def to_epoch_ms(iso):
    dt = datetime.strptime(iso, "%Y-%m-%dT%H:%M:%S.%fZ").replace(tzinfo=timezone.utc)
    return int(dt.timestamp() * 1000)

established, advisory, errors = [], [], []
with open(f"{LOGS}/journal-window.txt", encoding="utf-8", errors="replace") as f:
    for line in f:
        low = line.lower()
        mts = TS_UTC_RE.search(line)
        ts = to_epoch_ms(mts.group(1)) if mts else None
        if "tunnel established" in line:
            m = EST_RE.search(line)
            if m:
                established.append({"ts_ms": ts, "host": m.group(1),
                                    "establish_ms": int(m.group(2)),
                                    "pooled": m.group(3) == "true"})
        elif "allowlist advisory" in line:
            m = ADV_RE.search(line)
            if m:
                advisory.append({"ts_ms": ts, "host": m.group(1),
                                 "allowlisted": m.group(2) == "true"})
        if any(k in low for k in ERROR_KW):
            errors.append(line.rstrip("\n"))

ev_by_host = defaultdict(list)
for e in established:
    ev_by_host[e["host"]].append(e)
for h in ev_by_host:
    ev_by_host[h].sort(key=lambda e: e["ts_ms"])

def nearest_cold(events, ts_ms, window_ms=15000):
    """nearest event to ts_ms within window_ms; returns (pooled, dist_ms) or None"""
    best, best_d = None, window_ms + 1
    lo, hi = 0, len(events) - 1
    while lo <= hi:                       # binary search insertion point
        mid = (lo + hi) // 2
        if events[mid]["ts_ms"] < ts_ms:
            lo = mid + 1
        else:
            hi = mid - 1
    for i in (lo - 1, lo, lo + 1):        # check neighbors around the point
        if 0 <= i < len(events):
            d = abs(events[i]["ts_ms"] - ts_ms)
            if d < best_d:
                best, best_d = events[i], d
    if best is None or best_d > window_ms:
        return None
    return best["pooled"], best_d

# ---------------- failures log ----------------
FAIL_RE = re.compile(r"FAIL epoch_ms=(\d+) rc=(\d+) host=(\S+) code=(\d+) connect=(\S+) appconnect=(\S+) total=(\S+) size=(\S+)")
fails_by_host = defaultdict(list)   # per host: list of dicts
try:
    with open(f"{LOGS}/probe-failures.log", encoding="utf-8", errors="replace") as f:
        for line in f:
            m = FAIL_RE.search(line)
            if m:
                d = {"epoch_ms": int(m.group(1)), "rc": int(m.group(2)), "host": m.group(3),
                     "code": int(m.group(4)), "total": float(m.group(7))}
                key = HOST_BY_NAME.get(d["host"], d["host"])
                fails_by_host[key].append(d)
except FileNotFoundError:
    pass

# ---------------- per-host CSV stats ----------------
all_rows = {}
for hid, hname in HOSTS.items():
    all_rows[hname] = load_csv(f"{DIR}/probe_{hid}.csv")

summary = {}
verdict_lines = []
timeouts_all = []        # rc=28 (10s+ timeout events, -m 20 cap)

for hid, hname in HOSTS.items():
    rows = all_rows[hname]
    ok = [r for r in rows if 200 <= r["http_code"] <= 499]
    fails = [r for r in rows if r["http_code"] == 0]
    ok_totals = sorted(r["total_ms"] for r in ok)
    baseline = median(ok_totals) if ok_totals else None
    st = {
        "host": hname, "N": len(rows),
        "ok": len(ok), "fail": len(fails),
        "fail_rate": round(len(fails) / len(rows), 5) if rows else None,
        "baseline_median_ms": baseline,
        "p50": pct(ok_totals, 50), "p90": pct(ok_totals, 90),
        "p95": pct(ok_totals, 95), "p99": pct(ok_totals, 99),
        "max": max(ok_totals) if ok_totals else None,
    }
    st["p95_over_baseline"] = round(st["p95"] / baseline, 2) if baseline else None
    # 5-min buckets
    buckets = defaultdict(lambda: {"n": 0, "sum": 0.0, "fails": 0})
    for r in rows:
        b = (r["epoch_ms"] - START_MS) // 300000
        if r["http_code"] == 0:
            buckets[b]["fails"] += 1
        else:
            buckets[b]["n"] += 1
            buckets[b]["sum"] += r["total_ms"]
    st["buckets"] = {int(b): (round(v["sum"] / v["n"], 1) if v["n"] else None,
                              v["n"], v["fails"])
                     for b, v in sorted(buckets.items())}
    # slow + fail correlation with cold connects
    p99 = st["p99"]
    slow = [r for r in ok if r["total_ms"] > (p99 or 10**9)]
    events = ev_by_host.get(hname, [])
    corr = {"slow_total": len(slow), "slow_with_cold_15s": 0, "slow_cold_pairs": [],
            "fail_total": len(fails), "fail_with_cold_15s": 0, "fail_cold_pairs": []}
    for r in slow:
        nc = nearest_cold(events, r["epoch_ms"], 15000)
        if nc and not nc[0]:
            corr["slow_with_cold_15s"] += 1
            corr["slow_cold_pairs"].append((hms(r["epoch_ms"]), r["total_ms"], nc[1]))
    for r in fails:
        nc = nearest_cold(events, r["epoch_ms"], 15000)
        if nc and not nc[0]:
            corr["fail_with_cold_15s"] += 1
            corr["fail_cold_pairs"].append((hms(r["epoch_ms"]), r["total_ms"], nc[1]))
    st["correlation"] = corr
    # host timeouts
    htimeouts = [d for d in fails_by_host.get(hid, []) if d["rc"] == 28]
    timeouts_all += htimeouts
    st["timeout_rc28"] = len(htimeouts)
    # verdict per host
    flags = []
    if baseline and st["p95_over_baseline"] and st["p95_over_baseline"] > 2.0:
        flags.append(f"p95={st['p95']}ms > 2x 基线 {baseline}ms")
    if st["fail_rate"] is not None and st["fail_rate"] > 0.005:
        flags.append(f"失败率 {st['fail_rate']*100:.3f}% > 0.5%")
    if len(htimeouts) >= 2:
        flags.append(f"{len(htimeouts)} 次 10s+ 超时(rc=28) ≥ 2")
    st["host_verdict"] = "不稳定" if flags else "稳定"
    summary[hid] = st

overall_fail = sum(s["fail"] for s in summary.values())
overall_n = sum(s["N"] for s in summary.values())
overall_fail_rate = overall_fail / overall_n if overall_n else None
any_unstable = any(s["host_verdict"] == "不稳定" for s in summary.values())
if overall_fail_rate and overall_fail_rate > 0.005:
    verdict_lines.append(f"全局失败率 {overall_fail_rate*100:.3f}% > 0.5%")
if len(timeouts_all) >= 2:
    verdict_lines.append(f"窗口内 {len(timeouts_all)} 次 10s+ 超时(rc=28) ≥ 2")
# global p95 baseline check: pooled median of all success samples
all_ok = [r["total_ms"] for s in summary.values() for r in all_rows[s["host"]] if 200 <= r["http_code"] <= 499]
g_base, g_p95 = median(all_ok), pct(all_ok, 95)
if g_p95 and g_p95 > 2 * g_base:
    verdict_lines.append(f"全局 p95 {g_p95}ms > 2x 基线 {g_base}ms")
if verdict_lines or any_unstable:
    verdict = "不稳定"
else:
    verdict = "稳定"

# ---------------- tunnel pool window stats ----------------
all_est = [e["establish_ms"] for e in established]
pool = {
    "establishes_total": len(established),
    "pooled_true": sum(1 for e in established if e["pooled"]),
    "pooled_false": sum(1 for e in established if not e["pooled"]),
    "advisory_events": len(advisory),
    "advisory_allowlisted_true": sum(1 for a in advisory if a["allowlisted"]),
    "establish_ms_all": {"min": min(all_est) if all_est else None,
                         "p50": pct(all_est, 50), "p90": pct(all_est, 90),
                         "p95": pct(all_est, 95), "p99": pct(all_est, 99),
                         "max": max(all_est) if all_est else None},
    "by_host": {},
    "error_class_total": len(errors),
}
for h, evs in ev_by_host.items():
    est = [e["establish_ms"] for e in evs]
    pool["by_host"][h] = {
        "establishes": len(evs),
        "pooled_false": sum(1 for e in evs if not e["pooled"]),
        "establish_ms": {"min": min(est) if est else None, "p50": pct(est, 50),
                         "p90": pct(est, 90), "p95": pct(est, 95), "p99": pct(est, 99),
                         "max": max(est) if est else None},
    }
pool["error_keywords"] = dict(Counter(k for k in ERROR_KW for line in errors if k in line.lower()))
json.dump(pool, open(f"{DIR}/tunnel-pool-window.json", "w", encoding="utf-8"), indent=2, ensure_ascii=False)

# journal error samples
sampled = {}
for line in errors:
    key = next((k for k in ERROR_KW if k in line.lower()), "other")
    bucket = sampled.setdefault(key, [])
    if len(bucket) < 3:
        bucket.append(line)
with open(f"{DIR}/journal-errors.txt", "w", encoding="utf-8") as f:
    f.write(f"# journal error-class lines in window: {len(errors)}\n")
    for k, kc in sorted(pool["error_keywords"].items(), key=lambda x: -x[1]):
        f.write(f"## keyword='{k}' count={kc}\n")
        for s in sampled.get(k, []):
            f.write(f"SAMPLE: {s}\n")
        f.write("\n")

# ---------------- summary.md ----------------
L = []
A = L.append
A("# 轨道A：转发链路稳定性测试报告（pproxy CONNECT 隧道池）")
A("")
A("- **测试时间**：2026-09-29 16:45 – 17:10 CST（探测窗口 ≈25 分钟）")
A("- **链路**：devserver 本地数据面 `127.0.0.1:8899`（正向 CONNECT，无鉴权）→ 隧道池 `wss://rn.ponygo.fun/ws`、`wss://gate.ponygo.fun/ws`、`wss://vgate.ponygo.fun/ws` → 目标站")
A("- **方法**：4 目标 host 顺序循环探测，轮间 sleep 2s，`curl -x http://127.0.0.1:8899 -m 20`，CSV 记录 epoch_ms/connect/appconnect/total/size")
A("- **链路通定义**：http_code ∈ [200,499]；失败 = 000（含 -m 20 超时）")
A("")
A("## 1. 每 host 结果")
A("")
A("| host | N | 成功率 | p50(ms) | p90(ms) | p95(ms) | p99(ms) | max(ms) | p95/基线 | 超时(rc=28) | 判定 |")
A("|---|---|---|---|---|---|---|---|---|---|---|")
for hid, s in summary.items():
    A(f"| {s['host']} | {s['N']} | {100*(1-(s['fail_rate'] or 0)):.2f}% | {s['p50']} | {s['p90']} | {s['p95']} | {s['p99']} | {s['max']} | {s['p95_over_baseline']}x | {s['timeout_rc28']} | {s['host_verdict']} |")
A(f"| **合计** | {overall_n} | {100*(1-(overall_fail_rate or 0)):.2f}% | - | - | 全局p95 {g_p95}ms | - | - | 基线 {g_base}ms | {len(timeouts_all)} | - |")
A("")
A("### 失败明细（probe-failures.log）")
if any(fails_by_host.values()):
    for hid, s in summary.items():
        fl = fails_by_host.get(hid, [])
        if fl:
            A(f"- **{s['host']}**：{len(fl)} 次失败 → " + "; ".join(
                f"rc={d['rc']} @{hms(d['epoch_ms'])} total={d['total']:.1f}s" for d in fl[:8]))
else:
    A("- 无（0 次失败）")
A("")
A("## 2. 每 5 分钟桶平均 total_ms（成功样本，ms）")
A("")
A("| host | 0-5min | 5-10min | 10-15min | 15-20min | 20-25min |")
A("|---|---|---|---|---|---|")
for hid, s in summary.items():
    cells = []
    for b in range(5):
        v = s["buckets"].get(b)
        cells.append(f"{v[0] if v and v[0] else '-'}({v[2]}✗)" if v else "-")
    A(f"| {s['host']} | " + " | ".join(cells) + " |")
A("")
A("## 3. 隧道池窗口统计（journalctl -u pproxy.service，窗口内全部 tunnel 事件）")
A("")
A(f"- 窗口内 `tunnel established` 总数：**{pool['establishes_total']}**（pooled=true **{pool['pooled_true']}** / pooled=false **{pool['pooled_false']}**，冷建连占比 {100*pool['pooled_false']/max(pool['establishes_total'],1):.1f}%）")
A(f"- establish_ms（全部）：min={pool['establish_ms_all']['min']} p50={pool['establish_ms_all']['p50']} p90={pool['establish_ms_all']['p90']} p95={pool['establish_ms_all']['p95']} p99={pool['establish_ms_all']['p99']} max={pool['establish_ms_all']['max']}")
A(f"- `allowlist advisory only` 事件：{pool['advisory_events']}（其中 allowlisted=true {pool['advisory_allowlisted_true']}）")
A(f"- 错误类日志行（timeout/denied/error/retry/fail/refused/closed/reconnect）：**{pool['error_class_total']}** 条（明细见 journal-errors.txt）")
A("")
A("| host | establishes | pooled=false | establish_ms p50/p95/max |")
A("|---|---|---|---|")
for h, hs in pool["by_host"].items():
    A(f"| {h} | {hs['establishes']} | {hs['pooled_false']} | {hs['establish_ms']['p50']}/{hs['establish_ms']['p95']}/{hs['establish_ms']['max']} |")
A("")
A("## 4. 异常关联：慢样本/失败 vs pooled=false 冷建连（±15s 内最近事件）")
A("")
for hid, s in summary.items():
    c = s["correlation"]
    A(f"- **{s['host']}**：慢样本(total>p99) {c['slow_total']} 个，其中 {c['slow_with_cold_15s']} 个命中冷建连；失败 {c['fail_total']} 个，其中 {c['fail_with_cold_15s']} 个命中冷建连。"
      + (f" 示例慢样本 {c['slow_cold_pairs'][:4]}" if c["slow_cold_pairs"] else "")
      + (f" 失败冷建连 {c['fail_cold_pairs'][:4]}" if c["fail_cold_pairs"] else ""))
A("")
A("## 5. 判定")
A("")
A(f"- 触发条件：{('；'.join(verdict_lines)) if verdict_lines else '无（p95 未超 2x 基线、失败率未超 0.5%、10s+ 超时 <2 次）'}")
A(f"- **结论：转发链路 {verdict}**")
A("")
A("## 6. 原始数据")
A("- `probe_<host>.csv`（4 个，逐请求原始行）；`tunnel-pool-window.json`；`journal-errors.txt`；`../logs/journal-window.txt`；`../logs/probe-failures.log`")

with open(f"{DIR}/summary.md", "w", encoding="utf-8") as f:
    f.write("\n".join(L) + "\n")

print(json.dumps({"overall_n": overall_n, "overall_fail": overall_fail,
                  "overall_fail_rate": overall_fail_rate, "verdict": verdict,
                  "timeouts_rc28": len(timeouts_all),
                  "per_host": {k: {"ok": v["ok"], "fail": v["fail"], "p95": v["p95"],
                                   "fail_rate": v["fail_rate"], "verdict": v["host_verdict"]}
                               for k, v in summary.items()}}, indent=2))
print("summary.md written:", f"{DIR}/summary.md")