import urllib.request, urllib.error, time, json

GW = "http://ponyllm-pod-service.ponyllm.svc:8080/v1/chat/completions"
TOKEN = "sk-pony-7cc4cd2c0cb646a9a571067ce89eefa9"

def do_stream(model, content, max_tokens):
    body = json.dumps({
        "model": model,
        "messages": [{"role": "user", "content": content}],
        "stream": True,
        "max_tokens": max_tokens,
    }).encode()
    r = urllib.request.Request(GW, data=body, method="POST")
    r.add_header("Authorization", "Bearer " + TOKEN)
    r.add_header("Content-Type", "application/json")
    t0 = time.time()
    status = -1
    first_content_t = None
    chunks = 0
    prev_t = None
    max_gap = 0.0
    err = ""
    try:
        resp = urllib.request.urlopen(r, timeout=120)
        status = resp.status
        t_headers = time.time()
        while True:
            line = resp.readline()
            if not line:
                break
            if line.startswith(b"data:"):
                payload = line[5:].strip()
                if payload == b"[DONE]":
                    break
                now = time.time()
                if chunks == 0:
                    first_content_t = now
                else:
                    gap = now - prev_t
                    if gap > max_gap:
                        max_gap = gap
                prev_t = now
                chunks += 1
        total = time.time() - t0
        ttft = (first_content_t - t0) if first_content_t else (t_headers - t0)
        return status, round(ttft * 1000), round(total * 1000), chunks, round(max_gap * 1000), ""
    except Exception as e:
        total = time.time() - t0
        return status if status > 0 else -1, 0, round(total * 1000), chunks, round(max_gap * 1000), type(e).__name__ + ":" + str(e)[:100]

def do_nonstream(model, content, max_tokens):
    body = json.dumps({
        "model": model,
        "messages": [{"role": "user", "content": content}],
        "stream": False,
        "max_tokens": max_tokens,
    }).encode()
    r = urllib.request.Request(GW, data=body, method="POST")
    r.add_header("Authorization", "Bearer " + TOKEN)
    r.add_header("Content-Type", "application/json")
    t0 = time.time()
    try:
        with urllib.request.urlopen(r, timeout=120) as resp:
            data = resp.read()
            total = time.time() - t0
            ok = json.loads(data).get("choices") is not None
            return resp.status, round(total * 1000), 0, 0, 0, "" if ok else "no-choices"
    except Exception as e:
        total = time.time() - t0
        return -1, round(total * 1000), 0, 0, 0, type(e).__name__ + ":" + str(e)[:100]

f = open("/tmp/inference.csv", "w")
f.write("epoch_ms,seq,mode,model,status,ttft_ms,total_ms,chunks,max_gap_ms,err\n")

start = time.time()
seq = 0
# 12 short (max_tokens=8) spread over ~17min, every 85s; seq 5 and 9 replaced by long runs
for i in range(12):
    target = start + i * 85
    wait = target - time.time()
    if wait > 0:
        time.sleep(wait)
    if i in (5, 9):
        mode, model, mt = "long", "auto:economy", 200
        content = "Count from 1 to 40, one number per line, no other text."
    else:
        mode, model, mt = "short", ("gemini-3.8-flash-high" if i % 4 == 3 else "auto:economy"), 8
        content = "ping"
    ts = int(time.time() * 1000)
    status, ttft, total, chunks, max_gap, err = do_stream(model, content, mt)
    f.write(f"{ts},{seq},{mode},{model},{status},{ttft},{total},{chunks},{max_gap},{err}\n")
    f.flush()
    seq += 1
    print(f"seq{seq-1} {mode} {model} status={status} ttft={ttft}ms total={total}ms chunks={chunks} maxgap={max_gap}ms err={err}", flush=True)

# 1 non-streaming confirm routing
ts = int(time.time() * 1000)
status, total, _, _, _, err = do_nonstream("gemini-3.8-flash-high", "ping", 8)
f.write(f"{ts},{seq},nonstream,gemini-3.8-flash-high,{status},0,{total},0,0,{err}\n")
f.flush()
print(f"nonstream status={status} total={total}ms err={err}", flush=True)
f.close()
print("INFERENCE_DONE", flush=True)
