import urllib.request, urllib.error, time, base64, json, sys

PROXY_B64 = base64.b64encode(b"user:0d1fa1ac39d22b5062c56fa25a33062921a8d4eb4492fc5f").decode()

def req(url, method="GET", body=None, headers=None, timeout=20):
    r = urllib.request.Request(url, data=body, method=method)
    for k, v in (headers or {}).items():
        r.add_header(k, v)
    t0 = time.time()
    try:
        with urllib.request.urlopen(r, timeout=timeout) as resp:
            return round(time.time()-t0, 3), resp.status, ""
    except urllib.error.HTTPError as e:
        # HTTP status received (incl. expected 404/403) => link reachable
        return round(time.time()-t0, 3), e.code, "HTTPError:" + str(e.code)
    except Exception as e:
        return round(time.time()-t0, 3), -1, type(e).__name__ + ":" + str(e)[:100]

def proxy_https(url):
    ph = urllib.request.ProxyHandler({"https": "http://user:0d1fa1ac39d22b5062c56fa25a33062921a8d4eb4492fc5f@pproxy-host.ponyllm.svc:8899"})
    opener = urllib.request.build_opener(ph)
    t0 = time.time()
    try:
        with opener.open(url, timeout=20) as resp:
            return round(time.time()-t0, 3), resp.status, ""
    except urllib.error.HTTPError as e:
        return round(time.time()-t0, 3), e.code, "HTTPError:" + str(e.code)
    except Exception as e:
        return round(time.time()-t0, 3), -1, type(e).__name__ + ":" + str(e)[:100]

end = time.time() + 1500  # 25 分钟窗口
f1 = open("/tmp/pod_proxy.csv", "w")
f2 = open("/tmp/pod_reverse.csv", "w")
n = 0
last2 = 0.0
while time.time() < end:
    ts = int(time.time() * 1000)
    dt, code, err = proxy_https("https://daily-cloudcode-pa.googleapis.com/")
    f1.write(f"{ts},antigravity,{code},{dt},{err}\n")
    f1.flush()
    if time.time() - last2 >= 8.0:
        last2 = time.time()
        dt2, code2, err2 = req("http://100.95.193.103:8899/pony_31abcbd448a003be0ea27524d60973d8/opencode/zen/v1/models", headers={"User-Agent": "curl/8.0"})
        f2.write(f"{ts},opencode-rev,{code2},{dt2},{err2}\n")
        f2.flush()
    n += 1
    time.sleep(3)
f1.close()
f2.close()
print("DONE", n, flush=True)
