import csv, json, datetime, statistics

TZ = datetime.timezone(datetime.timedelta(hours=8))
def ts2s(ms):
    return datetime.datetime.fromtimestamp(ms/1000, TZ).strftime("%H:%M:%S")

def load_csv(p):
    rows = []
    try:
        for r in csv.reader(open(p)):
            if len(r) >= 5 and r[0].isdigit():
                rows.append(r)
    except FileNotFoundError:
        pass
    return rows

def dedupe(rows):
    seen = {}
    for r in rows:
        seen[int(r[0])] = r   # last write wins
    return [seen[k] for k in sorted(seen)]

def pct(vals, p):
    if not vals: return 0
    sv = sorted(vals)
    return round(sv[min(len(sv)-1, int(len(sv)*p))], 1)

def link_stats(rows, path_label, expected_codes):
    """rows: csv rows. Return dict of stats."""
    ok = []       # success latencies (ms)
    fail = []     # genuine failure rows
    for r in rows:
        code = r[2]
        lat = float(r[3]) * 1000
        err = r[4]
        if code in expected_codes:
            ok.append(lat)
        elif code == '-1' and err.startswith('HTTPError:HTTP Error 404'):
            # old-format http error with completed round trip = reachable
            ok.append(lat)
        else:
            fail.append(r)
    n = len(rows)
    return {
        "label": path_label,
        "n": n,
        "success": len(ok),
        "fail": len(fail),
        "success_rate_pct": round(len(ok)/n*100, 2) if n else None,
        "latency_ms_p50": pct(ok, 0.5) if ok else None,
        "latency_ms_p90": pct(ok, 0.9) if ok else None,
        "latency_ms_p95": pct(ok, 0.95) if ok else None,
        "latency_ms_p99": pct(ok, 0.99) if ok else None,
        "latency_ms_max": round(max(ok), 1) if ok else None,
        "fail_samples": [dict(ts=ts2s(int(r[0])), code=r[2], ms=round(float(r[3])*1000), err=r[4]) for r in fail[:10]],
    }

def main():
    proxy = dedupe(load_csv('/tmp/proxy_clean_merged.csv'))
    rev = dedupe(load_csv('/tmp/rev_clean_merged.csv'))
    inf = load_csv('/tmp/inference_full.csv')

    # persist merged raw data
    import csv as _csv
    with open('/tmp/pod_proxy_merged.csv', 'w', newline='') as f:
        _csv.writer(f).writerows(proxy)
    with open('/tmp/pod_reverse_merged.csv', 'w', newline='') as f:
        _csv.writer(f).writerows(rev)

    out = {
        "link1_antigravity": link_stats(proxy, "link1-antigravity-proxy", {"404"}),
        "link2_opencode_reverse": link_stats(rev, "link2-opencode-reverse", {"200"}),
    }
    # inference
    inf_rows = [r for r in inf if len(r) >= 10 and r[0].isdigit() and r[9] != "err"]
    inf_rows = [r for r in inf_rows if len(r) >= 10]
    inf_out = []
    for r in inf_rows:
        inf_out.append({
            "seq": r[1], "mode": r[2], "model": r[3], "status": int(r[4]),
            "ttft_ms": int(r[5]), "total_ms": int(r[6]), "chunks": int(r[7]),
            "max_gap_ms": int(r[8]), "err": r[9],
            "stall": int(r[8]) > 2000,
        })
    inf_out.sort(key=lambda x: int(x["seq"]))
    stream_ok = [x for x in inf_out if x["status"] == 200 and x["mode"] != "nonstream"]
    ttfts = [x["ttft_ms"] for x in stream_ok if x["ttft_ms"] > 0]
    gaps = [x["max_gap_ms"] for x in stream_ok]
    out["inference"] = {
        "n": len(inf_out),
        "n_stream_ok": len(stream_ok),
        "errors": [x for x in inf_out if x["status"] != 200],
        "ttft_ms_p50": pct(ttfts, 0.5), "ttft_ms_p90": pct(ttfts, 0.9),
        "ttft_ms_p95": pct(ttfts, 0.95), "ttft_ms_p99": pct(ttfts, 0.99),
        "ttft_ms_max": max(ttfts) if ttfts else None,
        "gap_ms_max": max(gaps) if gaps else None,
        "stall_count": sum(1 for x in inf_out if x.get("stall")),
        "stall_details": [x for x in inf_out if x.get("stall")],
        "rows": inf_out,
    }
    with open('/tmp/summary_data.json', 'w') as f:
        json.dump(out, f, indent=2, ensure_ascii=False)
    print(json.dumps(out["link1_antigravity"], ensure_ascii=False, indent=1))
    print(json.dumps(out["link2_opencode_reverse"], ensure_ascii=False, indent=1))
    print(json.dumps(out["inference"], ensure_ascii=False, indent=1)[:4000])

if __name__ == "__main__":
    main()