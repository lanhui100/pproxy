import logging, os, time
from urllib.parse import urlparse

import httpx
import uvicorn
from fastapi import FastAPI, Request, Response
from fastapi.responses import StreamingResponse

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("pproxy-edge")

PROXY_SECRET = os.environ.get("PROXY_SECRET", "<REDACTED_DEV_SECRET>")

HOP_BY_HOP = {
    "host", "connection", "keep-alive", "proxy-authenticate",
    "proxy-authorization", "te", "trailer", "transfer-encoding",
    "upgrade", "content-length", "x-proxy-secret",
    # geo / proxy leakage
    "x-forwarded-for", "x-forwarded-proto", "x-forwarded-host",
    "x-real-ip", "forwarded", "via", "true-client-ip",
    "cf-connecting-ip", "cf-ipcountry", "cf-ray", "cf-worker",
}

EXPOSE_SKIP = {"transfer-encoding", "connection", "content-length"}

app = FastAPI(title="pproxy-edge")
start_time = time.time()


@app.get("/health")
async def health():
    return {"status": "ok", "uptime": int(time.time() - start_time)}


@app.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"])
async def proxy(request: Request, path: str):
    if request.headers.get("x-proxy-secret") != PROXY_SECRET:
        return Response("Unauthorized", status_code=403)

    target = request.query_params.get("url")
    if not target:
        return Response("Missing ?url=", status_code=400)

    parsed = urlparse(target)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        return Response("Invalid URL", status_code=400)

    headers = {
        k: v for k, v in request.headers.items()
        if k.lower() not in HOP_BY_HOP
    }
    headers["host"] = parsed.netloc

    body = await request.body()
    client = httpx.AsyncClient(http2=True, follow_redirects=True, timeout=None)

    req = client.build_request(
        method=request.method,
        url=target,
        headers=headers,
        content=body,
    )
    resp = await client.send(req, stream=True)

    out_headers = {
        k: v for k, v in resp.headers.items()
        if k.lower() not in EXPOSE_SKIP
    }
    out_headers["x-proxy-edge"] = "hf-space"

    return StreamingResponse(
        resp.aiter_raw(),
        status_code=resp.status_code,
        headers=out_headers,
        background=httpx_background_close(client, resp),
    )


def httpx_background_close(client: httpx.AsyncClient, resp: httpx.Response):
    from starlette.background import BackgroundTask

    async def _close():
        await resp.aclose()
        await client.aclose()

    return BackgroundTask(_close)


if __name__ == "__main__":
    uvicorn.run(app, host="0.0.0.0", port=7860)
