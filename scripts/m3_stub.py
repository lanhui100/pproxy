#!/usr/bin/env python3
"""m3_stub.py — M3 集成测试双 stub（全离线口径，spec §9.1）。

单进程双 HTTP 服务：
- API stub（STUB_PORT，默认 18894）：
    POST /graphql     → CF GraphQL 形状响应（当日 requests=STUB_INVOCATIONS，默认 85000 → pct=85%）
                        校验 variables.accountTag 必须等于 STUB_ACCOUNT_TAG（默认 test，
                        R1：dummy 凭据必须真实到达 stub，否则返回 errors 非空）
    GET  /v1/usage    → Vercel Hobby 门控形状 {"error":{"code":"plan_upgrade_required"}}
- Webhook 接收器（HOOK_PORT，默认 18893）：POST /hook 把 body 原样追加写 SINK_FILE
"""
import datetime
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

STUB_PORT = int(os.environ.get("STUB_PORT", "18894"))
HOOK_PORT = int(os.environ.get("HOOK_PORT", "18893"))
SINK_FILE = os.environ.get("SINK_FILE", "")
STUB_INVOCATIONS = int(os.environ.get("STUB_INVOCATIONS", "85000"))
STUB_ACCOUNT_TAG = os.environ.get("STUB_ACCOUNT_TAG", "test")


class ApiHandler(BaseHTTPRequestHandler):
    def log_message(self, *args):  # 静默，测试日志只留 server 输出
        pass

    def _send(self, code, body: bytes):
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0") or "0")
        body = self.rfile.read(length) if length else b""
        if not self.path.startswith("/graphql"):
            self._send(404, b'{"error":"not_found"}')
            return
        # GraphQL 变量注入面校验：accountTag 必须经 variables 字段到达
        try:
            payload = json.loads(body or b"{}")
        except Exception:
            payload = {}
        tag = payload.get("variables", {}).get("accountTag", "")
        today = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")
        if tag != STUB_ACCOUNT_TAG:
            resp = {"errors": [{"message": f"unexpected accountTag {tag!r}"}], "data": None}
        else:
            resp = {
                "errors": [],
                "data": {"viewer": {"accounts": [{
                    "workersInvocationsAdaptive": [
                        {"sum": {"requests": STUB_INVOCATIONS},
                         "dimensions": {"date": today}},
                    ],
                }]}},
            }
        self._send(200, json.dumps(resp).encode())

    def do_GET(self):
        if self.path.startswith("/v1/usage"):
            self._send(200, json.dumps(
                {"error": {"code": "plan_upgrade_required"}}).encode())
        else:
            self._send(404, b'{"error":"not_found"}')


class HookHandler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0") or "0")
        body = self.rfile.read(length) if length else b""
        with open(SINK_FILE, "ab") as f:  # 追加写：多轮告警可计数
            f.write(b"\n" + body)
        resp = b'{"ok":true}'
        self.send_response(200)
        self.send_header("Content-Length", str(len(resp)))
        self.end_headers()
        self.wfile.write(resp)


def main():
    api = ThreadingHTTPServer(("127.0.0.1", STUB_PORT), ApiHandler)
    hook = ThreadingHTTPServer(("127.0.0.1", HOOK_PORT), HookHandler)
    import threading
    threading.Thread(target=hook.serve_forever, daemon=True).start()
    api.serve_forever()


if __name__ == "__main__":
    main()
