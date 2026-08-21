#!/usr/bin/env python3
"""m1_echo_stub.py — M1 集成测试本地回显 stub（T8 §2）。

监听 127.0.0.1:18901，对任何请求返回 200 JSON：
  {"url": "<x-stub-url 头或路径>", "headers": {<全部请求头小写键值对>}}

用于 x-pony-token 泄露断言（步骤 5.5）与离线转发链路验证。
EdgeClient 直连本 stub（经临时 config 的 upstreams.localstub），不经 CF Worker。
"""

import json
from http.server import BaseHTTPRequestHandler, HTTPServer


class EchoHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _echo(self):
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)
        headers = {k.lower(): v for k, v in self.headers.items()}
        body = json.dumps({"url": self.path, "headers": headers}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = do_HEAD = _echo

    def log_message(self, fmt, *args):
        pass  # 静默：测试日志只看 server.log


if __name__ == "__main__":
    HTTPServer(("127.0.0.1", 18901), EchoHandler).serve_forever()
