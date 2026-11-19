import asyncio, ssl, time, json, os, base64

N = int(os.environ.get('PROBE_N', '400'))
HOLD = float(os.environ.get('PROBE_HOLD', '18'))
SAMPLE = 2.0
HOST, PORT = '127.0.0.1', 8443
TARGET = 'www.cloudflare.com:443'
TOKEN = os.environ.get('PROBE_TOKEN', '')
if not TOKEN:
    raise SystemExit('PROBE_TOKEN required')
BASIC = base64.b64encode((TOKEN + ':' + TOKEN).encode()).decode()

ctx = ssl.create_default_context()
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE

PAGE_KB = os.sysconf('SC_PAGE_SIZE') // 1024


def meminfo():
    d = {}
    with open('/proc/meminfo') as f:
        for line in f:
            k, v = line.split(':', 1)
            d[k.strip()] = int(v.strip().split()[0])
    return d


def conntrack():
    try:
        return int(open('/proc/sys/net/netfilter/nf_conntrack_count').read().strip())
    except Exception:
        return -1


def rss_by_comm():
    out = {}
    for pid in os.listdir('/proc'):
        if not pid.isdigit():
            continue
        try:
            with open('/proc/' + pid + '/comm') as f:
                comm = f.read().strip()
            if comm == 'nginx' or comm.startswith('pproxy-gate'):
                with open('/proc/' + pid + '/statm') as f:
                    rss = int(f.read().split()[1]) * PAGE_KB
                group = 'nginx' if comm == 'nginx' else 'gate'
                out[group] = out.get(group, 0) + rss
        except Exception:
            pass
    return out


def self_rss():
    try:
        with open('/proc/self/statm') as f:
            return int(f.read().split()[1]) * PAGE_KB
    except Exception:
        return -1


def tcp_established():
    n = 0
    for fn in ('/proc/net/tcp', '/proc/net/tcp6'):
        try:
            with open(fn) as f:
                next(f)
                for line in f:
                    if line.split()[3] == '01':
                        n += 1
        except Exception:
            pass
    return n


def netdev_drops():
    rx = tx = 0
    with open('/proc/net/dev') as f:
        next(f)
        next(f)
        for line in f:
            p = line.split(':', 1)[1].split()
            rx += int(p[2]) + int(p[3])
            tx += int(p[10]) + int(p[11])
    return rx, tx


def load1():
    return float(open('/proc/loadavg').read().split()[0])


def sample(tag, t0):
    m = meminfo()
    rx, tx = netdev_drops()
    procs = rss_by_comm()
    return {
        'tag': tag, 't': round(time.time() - t0, 1),
        'memavail_kb': m.get('MemAvailable', -1),
        'memfree_kb': m.get('MemFree', -1),
        'swapfree_kb': m.get('SwapFree', -1),
        'swaptotal_kb': m.get('SwapTotal', -1),
        'conntrack': conntrack(),
        'rss_kb': procs,
        'rss_sum_kb': sum(procs.values()),
        'self_rss_kb': self_rss(),
        'estab': tcp_established(),
        'net_err_drop_rx': rx, 'net_err_drop_tx': tx,
        'load1': load1(),
    }


async def main():
    t0 = time.time()
    base = sample('baseline', t0)
    print(json.dumps(base), flush=True)
    series = [base]

    results = {'ok': 0, 'fail': 0}
    reasons = {}
    tunnels = []
    release = asyncio.Event()
    stop = asyncio.Event()
    guard = {'trip': False}

    async def sampler():
        while not stop.is_set():
            await asyncio.sleep(SAMPLE)
            if stop.is_set():
                break
            s = sample('during', t0)
            series.append(s)
            print(json.dumps(s), flush=True)
            if s['memavail_kb'] < 40000 or s['swapfree_kb'] < 40000:
                guard['trip'] = True
                print(json.dumps({'tag': 'guard-trip', 't': s['t']}), flush=True)
                release.set()
                stop.set()

    samp_task = asyncio.create_task(sampler())

    async def one():
        w = None
        ok = False
        key = 'unknown'
        try:
            r, w = await asyncio.wait_for(
                asyncio.open_connection(HOST, PORT, ssl=ctx, server_hostname='rn.ponygo.fun'),
                timeout=20)
            req = ('CONNECT ' + TARGET + ' HTTP/1.1\r\n'
                   'Host: ' + TARGET + '\r\n'
                   'Proxy-Authorization: Basic ' + BASIC + '\r\n\r\n')
            w.write(req.encode())
            await w.drain()
            head = await asyncio.wait_for(r.readuntil(b'\r\n\r\n'), timeout=10)
            ok = head.startswith(b'HTTP/1.1 200') or head.startswith(b'HTTP/1.0 200')
            key = head.split(b'\r\n', 1)[0].decode('ascii', 'replace')[:90]
            if ok:
                tunnels.append((r, w))
                w = None
        except Exception as e:
            key = type(e).__name__ + ': ' + str(e)[:90]
        finally:
            if w is not None:
                try:
                    w.close()
                except Exception:
                    pass
        results['ok' if ok else 'fail'] += 1
        reasons[key] = reasons.get(key, 0) + 1

    sem = asyncio.Semaphore(50)

    async def bounded():
        async with sem:
            await one()

    await asyncio.gather(*(bounded() for _ in range(N)))
    est = sample('established', t0)
    series.append(est)
    print(json.dumps(dict(est, ok=results['ok'], fail=results['fail'],
                          reasons=dict(sorted(reasons.items(), key=lambda kv: -kv[1])[:6])),
                     ensure_ascii=False), flush=True)

    if not guard['trip']:
        try:
            await asyncio.wait_for(release.wait(), timeout=HOLD)
        except asyncio.TimeoutError:
            pass
    stop.set()
    try:
        await samp_task
    except asyncio.CancelledError:
        pass

    for r, w in tunnels:
        try:
            w.close()
        except Exception:
            pass
    await asyncio.sleep(1)
    print(json.dumps(sample('released-1s', t0)), flush=True)
    await asyncio.sleep(8)
    print(json.dumps(sample('released-9s', t0)), flush=True)

    print(json.dumps({
        'tag': 'summary',
        'n': N,
        'ok': results['ok'], 'fail': results['fail'],
        'guard_trip': guard['trip'],
        'peak_conntrack': max(s['conntrack'] for s in series),
        'min_memavail_kb': min(s['memavail_kb'] for s in series),
        'min_swapfree_kb': min(s['swapfree_kb'] for s in series),
        'peak_server_rss_kb': max(s['rss_sum_kb'] for s in series),
        'rss_delta_kb': max(s['rss_sum_kb'] for s in series) - base['rss_sum_kb'],
        'swap_delta_kb': base['swapfree_kb'] - min(s['swapfree_kb'] for s in series),
        'memavail_delta_kb': base['memavail_kb'] - min(s['memavail_kb'] for s in series),
        'peak_estab': max(s['estab'] for s in series),
        'per_conn_rss_kb': round((max(s['rss_sum_kb'] for s in series) - base['rss_sum_kb']) / N, 1),
        'per_conn_swap_kb': round((base['swapfree_kb'] - min(s['swapfree_kb'] for s in series)) / N, 1),
        'per_conn_conntrack': round((max(s['conntrack'] for s in series) - base['conntrack']) / N, 2),
        'net_drop_rx_delta': max(s['net_err_drop_rx'] for s in series) - base['net_err_drop_rx'],
        'net_drop_tx_delta': max(s['net_err_drop_tx'] for s in series) - base['net_err_drop_tx'],
    }), flush=True)


asyncio.run(main())
