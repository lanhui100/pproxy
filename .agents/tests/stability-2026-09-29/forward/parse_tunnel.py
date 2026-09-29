#!/usr/bin/env python3
"""Parse the pproxy journal window (logs/journal-window.txt) into
forward/tunnel-pool-window.json and forward/journal-errors.txt.

Input line examples:
  Sep 29 16:44:35 devserver pproxy-server[694697]: 2026-09-29T08:44:35.369841Z  INFO pproxy_server::connect: tunnel established host=www.google.com establish_ms=283 pooled=true
  ... tunnel establish (allowlist advisory only) host=... allowlisted=true
"""
import json
import re
import sys
import math
from collections import Counter, defaultdict
from datetime import datetime, timezone

SRC = sys.argv[1] if len(sys.argv) > 1 else "../logs/journal-window.txt"
OUT_JSON = "tunnel-pool-window.json"
OUT_ERR = "journal-errors.txt"

EST_RE = re.compile(r"tunnel established host=(\S+) establish_ms=(\d+) pooled=(true|false)")
ADV_RE = re.compile(r"tunnel establish \(allowlist advisory only\) host=(\S+) allowlisted=(true|false)")
TS_UTC_RE = re.compile(r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6}Z)")

established = []      # dicts: ts_ms, host, establish_ms, pooled
advisory = []         # dicts: ts_ms, host, allowlisted
errors = []           # raw lines of error class

ERROR_KEYWORDS = ("timeout", "denied", "error", "retry", "fail", "refused", "closed", "reconnect")

def to_epoch_ms(iso_utc):
    dt = datetime.strptime(iso_utc, "%Y-%m-%dT%H:%M:%S.%fZ").replace(tzinfo=timezone.utc)
    return int(dt.timestamp() * 1000)

with open(SRC, encoding="utf-8", errors="replace") as f:
    for line in f:
        low = line.lower()
        mts = TS_UTC_RE.search(line)
        ts_ms = to_epoch_ms(mts.group(1)) if mts else None
        if "tunnel established" in line:
            m = EST_RE.search(line)
            if m:
                established.append({
                    "ts_ms": ts_ms, "host": m.group(1),
                    "establish_ms": int(m.group(2)), "pooled": m.group(3) == "true",
                })
        elif "allowlist advisory" in line:
            m = ADV_RE.search(line)
            if m:
                advisory.append({"ts_ms": ts_ms, "host": m.group(1),
                                 "allowlisted": m.group(3) == "true"})
        if any(k in low for k in ERROR_KEYWORDS):
            errors.append(line.rstrip("\n"))

def pct(vals, p):
    if not vals:
        return None
    s = sorted(vals)
    k = (len(s) - 1) * p / 100.0
    lo, hi = math.floor(k), math.ceil(k)
    if lo == hi:
        return s[lo]
    return s[lo] + (s[hi] - s[lo]) * (k - lo)

by_host = defaultdict(list)
for e in established:
    by_host[e["host"]].append(e)

host_stats = {}
for host, evs in by_host.items():
    est_ms = [e["establish_ms"] for e in evs]
    pooled_true = sum(1 for e in evs if e["pooled"])
    pooled_false = sum(1 for e in evs if not e["pooled"])
    host_stats[host] = {
        "establishes": len(evs),
        "pooled_true": pooled_true,
        "pooled_false": pooled_false,
        "pooled_false_frac": round(pooled_false / len(evs), 4) if evs else None,
        "establish_ms": {
            "min": min(est_ms) if est_ms else None,
            "p50": pct(est_ms, 50),
            "p90": pct(est_ms, 90),
            "p95": pct(est_ms, 95),
            "p99": pct(est_ms, 99),
            "max": max(est_ms) if est_ms else None,
        },
    }

all_est_ms = [e["establish_ms"] for e in established]
out = {
    "window_start_epoch_s": None,
    "window_end_epoch_s": None,
    "establishes_total": len(established),
    "pooled_true": sum(1 for e in established if e["pooled"]),
    "pooled_false": sum(1 for e in established if not e["pooled"]),
    "advisory_events": len(advisory),
    "advisory_allowlisted_true": sum(1 for a in advisory if a["allowlisted"]),
    "establish_ms_all": {
        "min": min(all_est_ms) if all_est_ms else None,
        "p50": pct(all_est_ms, 50),
        "p90": pct(all_est_ms, 90),
        "p95": pct(all_est_ms, 95),
        "p99": pct(all_est_ms, 99),
        "max": max(all_est_ms) if all_est_ms else None,
    },
    "by_host": host_stats,
    "error_class_total": len(errors),
    "error_keywords_seen": dict(Counter(k for k in ERROR_KEYWORDS
                                        for line in errors if k in line.lower())),
}
if established:
    out["window_start_epoch_s"] = established[0]["ts_ms"] / 1000.0
    out["window_end_epoch_s"] = established[-1]["ts_ms"] / 1000.0

with open(OUT_JSON, "w", encoding="utf-8") as f:
    json.dump(out, f, indent=2, ensure_ascii=False)

# error samples: up to 3 per keyword
sampled = {}
for line in errors:
    key = next((k for k in ERROR_KEYWORDS if k in line.lower()), "other")
    bucket = sampled.setdefault(key, [])
    if len(bucket) < 3:
        bucket.append(line)

with open(OUT_ERR, "w", encoding="utf-8") as f:
    f.write(f"# journal error-class lines in window: {len(errors)}\n")
    for k, kc in sorted(out["error_keywords_seen"].items(), key=lambda x: -x[1]):
        f.write(f"## keyword='{k}' count={kc}\n")
        for s in sampled.get(k, []):
            f.write(f"SAMPLE: {s}\n")
        f.write("\n")
    f.write(f"## keyword='other' count={out['error_class_total'] - sum(out['error_keywords_seen'].values())}\n")
    for s in sampled.get("other", []):
        f.write(f"SAMPLE: {s}\n")

print(json.dumps(out, indent=2, ensure_ascii=False))