#!/usr/bin/env python3
"""raw_probe.py — CONNECT 数据面协议级探针（L2-AT 对抗注入专用）。

定位：只做"协议级观测"，不做业务判定；判定阈值全部由调用方 bash 用例通过
`--budget` 传入，脚本以退出码 0/1 表达"是否在预算内取得契约要求的观察"。

子命令：
  halfopen   半死连接：发半截首部（或一个字节都不发）后静默，断言服务端在
             budget 秒内关闭连接（契约 C1：首字节/首部读取窗口各 30s）。
  oversize   超大畸形首部：单行 >16KiB 或总首部 >16KiB，断言被拒绝
             （4xx 或直接关闭），且不越界/不挂起（契约 C1：首部上限 16 KiB）。
  concurrent 确定性并发竞态：固定 N 个连接 + Barrier 同步起跑，断言全部在
             budget 内返回（成功或明确拒绝），无死锁。

确定性纪律：固定迭代次数 + 固定超时预算；无随机 sleep、无概率重试。
"""

import argparse
import base64
import socket
import sys
import threading
import time

DEFAULT_TARGET_HOST = "cp.cloudflare.com"
DEFAULT_TARGET_PORT = 443
KIB = 1024


def _connect(host: str, port: int, timeout: float) -> socket.socket:
    sock = socket.create_connection((host, port), timeout=timeout)
    sock.settimeout(timeout)
    return sock


def _recv_until_eof(sock: socket.socket, budget: float, cap: int = 8192):
    """读到 EOF 或预算耗尽。返回 (elapsed, closed, data)。closed=True 表示对端已关闭/重置。"""
    start = time.monotonic()
    data = b""
    while True:
        remaining = budget - (time.monotonic() - start)
        if remaining <= 0:
            return time.monotonic() - start, False, data
        sock.settimeout(min(0.5, remaining))
        try:
            chunk = sock.recv(4096)
        except socket.timeout:
            continue
        except (ConnectionResetError, ConnectionAbortedError, BrokenPipeError):
            return time.monotonic() - start, True, data
        except OSError:
            return time.monotonic() - start, True, data
        if not chunk:
            return time.monotonic() - start, True, data
        if len(data) < cap:
            data += chunk[: cap - len(data)]
    # unreachable


def _send_all(sock: socket.socket, payload: bytes) -> str:
    """尽力发送；返回空串表示全部送出，否则返回错误类别（对端提前关闭属预期拒绝路径）。"""
    try:
        sock.sendall(payload)
        return ""
    except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
        return "peer-closed-during-send"
    except OSError as exc:
        return f"send-error:{exc.__class__.__name__}"


def _basic(token: str) -> str:
    raw = f"{token}:{token}".encode()
    return "Basic " + base64.b64encode(raw).decode()


def cmd_halfopen(args) -> int:
    """半死连接：故意不发完首部，观察服务端是否在有界时间内关闭。"""
    if args.mode == "partial":
        payload = (
            f"CONNECT {args.target_host}:{args.target_port} HTTP/1.1\r\n"
            f"Host: {args.target_host}:{args.target_port}\r\n"
        ).encode()  # 故意缺结尾空行
    else:  # none：连上后一个字节都不发（首字节等待窗口）
        payload = b""

    try:
        sock = _connect(args.connect_host, args.connect_port, timeout=min(5.0, args.budget))
    except OSError as exc:
        print(f"connect_failed={exc.__class__.__name__} endpoint={args.connect_host}:{args.connect_port}")
        return 1

    sent = ""
    if payload:
        sent = _send_all(sock, payload)
    elapsed, closed, data = _recv_until_eof(sock, args.budget)
    try:
        sock.close()
    except OSError:
        pass

    first = data.split(b"\r\n", 1)[0].decode("latin-1") if data else "<none>"
    print(
        f"mode={args.mode} sent={len(payload)}B send_err={sent or '-'} "
        f"closed={int(closed)} elapsed={elapsed:.2f}s budget={args.budget:.0f}s first_line={first!r}"
    )
    return 0 if closed else 1


def cmd_oversize(args) -> int:
    """超大畸形首部：单行 >16KiB（line）或总首部 >16KiB（headers）。"""
    if args.mode == "line":
        line = "CONNECT " + ("a" * (24 * KIB)) + f":{args.target_port} HTTP/1.1"
        payload = (line + "\r\n\r\n").encode()
    else:
        head = f"CONNECT {args.target_host}:{args.target_port} HTTP/1.1\r\n"
        hdrs = "".join(f"X-Pad-{i:04d}: {'p' * 96}\r\n" for i in range(400))
        payload = (head + hdrs + "\r\n").encode()

    try:
        sock = _connect(args.connect_host, args.connect_port, timeout=min(5.0, args.budget))
    except OSError as exc:
        print(f"connect_failed={exc.__class__.__name__} endpoint={args.connect_host}:{args.connect_port}")
        return 1

    sent = _send_all(sock, payload)
    elapsed, closed, data = _recv_until_eof(sock, args.budget)
    try:
        sock.close()
    except OSError:
        pass

    first = data.split(b"\r\n", 1)[0].decode("latin-1").strip() if data else "<none>"
    status = None
    parts = first.split()
    if len(parts) >= 2 and parts[0].startswith("HTTP/"):
        try:
            status = int(parts[1])
        except ValueError:
            status = None

    rejected = closed or (status is not None and 400 <= status < 500)
    print(
        f"mode={args.mode} sent={len(payload)}B send_err={sent or '-'} closed={int(closed)} "
        f"status={status if status is not None else 'none'} elapsed={elapsed:.2f}s "
        f"budget={args.budget:.0f}s first_line={first!r} rejected={int(rejected)}"
    )
    return 0 if rejected else 1


def cmd_concurrent(args) -> int:
    """确定性并发竞态：Barrier 同步起跑，固定 N 连接 × R 轮。"""
    results = []
    lock = threading.Lock()

    total = args.n * args.rounds
    barrier = threading.Barrier(args.n)
    deadline = time.monotonic() + args.budget
    auth = _basic(args.token) if args.token else None

    def worker(index: int) -> None:
        code = "NO-CONNECT"
        hung = False
        # Barrier 同步：N 个线程尽量同时进入 CONNECT 首字节
        try:
            barrier.wait(timeout=args.budget)
        except threading.BrokenBarrierError:
            pass
        try:
            sock = _connect(args.connect_host, args.connect_port, timeout=min(5.0, args.budget))
        except OSError as exc:
            with lock:
                results.append((f"CONNECT-{exc.__class__.__name__}", index))
            return
        req = (
            f"CONNECT {args.target_host}:{args.target_port} HTTP/1.1\r\n"
            f"Host: {args.target_host}:{args.target_port}\r\n"
        )
        if auth:
            req += f"Proxy-Authorization: {auth}\r\n"
        req += "\r\n"
        try:
            sock.settimeout(min(5.0, args.budget))
            sock.sendall(req.encode())
            buf = b""
            sock.settimeout(min(args.budget, 15.0))
            while b"\r\n" not in buf:
                if time.monotonic() > deadline:
                    hung = True
                    break
                chunk = sock.recv(1024)
                if not chunk:
                    break
                buf += chunk
            if hung:
                code = "HUNG"
            elif not buf:
                code = "CLOSED-NO-STATUS"
            else:
                first = buf.split(b"\r\n", 1)[0].decode("latin-1", "replace")
                parts = first.split()
                code = parts[1] if len(parts) >= 2 and parts[0].startswith("HTTP/") else f"BAD-LINE:{first[:32]}"
        except socket.timeout:
            code = "HUNG"
            hung = True
        except OSError as exc:
            code = f"IO-{exc.__class__.__name__}"
        finally:
            try:
                sock.close()
            except OSError:
                pass
        with lock:
            results.append((code, index))

    threads = []
    t0 = time.monotonic()
    for _round in range(args.rounds):
        results.clear()
        barrier.reset()
        threads = [
            threading.Thread(target=worker, args=(i,), daemon=True) for i in range(args.n)
        ]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=max(1.0, args.budget - (time.monotonic() - t0)))
    elapsed = time.monotonic() - t0

    hist = {}
    hung = 0
    for code, _idx in results:
        hist[code] = hist.get(code, 0) + 1
        if code in ("HUNG", "NO-STATUS"):
            hung += 1
    # 最后一个 round 的统计最接近"同轮真正并发"；但历史上限取全部轮次
    allowed = {"200", "403", "407", "502"}
    unexpected = {c: n for c, n in hist.items() if c not in allowed}
    survivors = [t for t in threads if t.is_alive()]

    print(
        f"n={args.n} rounds={args.rounds} total={total} last_round_hist={hist} "
        f"elapsed={elapsed:.2f}s budget={args.budget:.0f}s hung={hung} "
        f"survivors={len(survivors)} unexpected={unexpected or '-'}"
    )
    ok = (
        not survivors
        and hung == 0
        and not unexpected
        and elapsed < args.budget
        and any(code == "200" for code, _ in results)
    )
    return 0 if ok else 1


def main() -> int:
    parser = argparse.ArgumentParser(description="pproxy CONNECT 数据面协议级探针")
    sub = parser.add_subparsers(dest="cmd", required=True)

    common = dict(
        connect_host="127.0.0.1",
        connect_port=3102,
        target_host=DEFAULT_TARGET_HOST,
        target_port=DEFAULT_TARGET_PORT,
        budget=35.0,
    )

    p = sub.add_parser("halfopen")
    p.add_argument("--connect-host", default=common["connect_host"])
    p.add_argument("--connect-port", type=int, default=common["connect_port"])
    p.add_argument("--target-host", default=common["target_host"])
    p.add_argument("--target-port", type=int, default=common["target_port"])
    p.add_argument("--mode", choices=["partial", "none"], default="partial")
    p.add_argument("--budget", type=float, default=common["budget"])
    p.set_defaults(fn=cmd_halfopen)

    p = sub.add_parser("oversize")
    p.add_argument("--connect-host", default=common["connect_host"])
    p.add_argument("--connect-port", type=int, default=common["connect_port"])
    p.add_argument("--target-host", default=common["target_host"])
    p.add_argument("--target-port", type=int, default=common["target_port"])
    p.add_argument("--mode", choices=["line", "headers"], default="line")
    p.add_argument("--budget", type=float, default=20.0)
    p.set_defaults(fn=cmd_oversize)

    p = sub.add_parser("concurrent")
    p.add_argument("--connect-host", default=common["connect_host"])
    p.add_argument("--connect-port", type=int, default=common["connect_port"])
    p.add_argument("--target-host", default=common["target_host"])
    p.add_argument("--target-port", type=int, default=common["target_port"])
    p.add_argument("--token", default="")
    p.add_argument("--n", type=int, default=20)
    p.add_argument("--rounds", type=int, default=3)
    p.add_argument("--budget", type=float, default=40.0)
    p.set_defaults(fn=cmd_concurrent)

    args = parser.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
