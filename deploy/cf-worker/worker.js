// Cloudflare Worker 出口网关（去指纹伪装 + 恒定时间防时序攻击）

function timingSafeEqual(a, b) {
  if (typeof a !== "string" || typeof b !== "string") return false;
  const encoder = new TextEncoder();
  const aBuf = encoder.encode(a);
  const bBuf = encoder.encode(b);
  if (aBuf.byteLength !== bBuf.byteLength) return false;
  return crypto.subtle.timingSafeEqual(aBuf, bBuf);
}

const NOT_FOUND_HTML = `<!DOCTYPE html><html><head><title>404 Not Found</title></head><body><center><h1>404 Not Found</h1></center><hr><center>nginx</center></body></html>`;

export default {
  async fetch(request, env, ctx) {
    if (request.method === "OPTIONS") {
      return new Response(null, {
        headers: {
          "Access-Control-Allow-Origin": env.ALLOWED_ORIGINS || "*",
          "Access-Control-Allow-Methods": "GET,POST,PUT,DELETE,PATCH,CONNECT,OPTIONS",
          "Access-Control-Allow-Headers": "*",
          "Access-Control-Max-Age": "86400"
        }
      });
    }

    const auth = request.headers.get("X-Proxy-Secret") || "";
    const expected = env.PROXY_SECRET || "";

    // 恒定时间比对；若未授权伪装为标准 404 HTML，杜绝 Shodan 扫描识别
    if (!expected || !timingSafeEqual(auth, expected)) {
      return new Response(NOT_FOUND_HTML, {
        status: 404,
        headers: { "Content-Type": "text/html; charset=UTF-8" }
      });
    }

    const url = new URL(request.url);
    const target = url.searchParams.get("url");
    if (!target) {
      return new Response(NOT_FOUND_HTML, {
        status: 404,
        headers: { "Content-Type": "text/html; charset=UTF-8" }
      });
    }

    let targetUrl;
    try {
      targetUrl = new URL(target);
    } catch {
      return new Response("Invalid Target URL", { status: 400 });
    }

    const headers = new Headers();
    const stripped = new Set([
      "host",
      "cf-connecting-ip",
      "cf-ray",
      "cf-ipcountry",
      "cf-worker",
      "cf-ew-via",
      "x-proxy-secret",
      "x-forwarded-for",
      "x-forwarded-proto",
      "x-real-ip",
      "forwarded",
      "via",
      "true-client-ip",
    ]);

    request.headers.forEach((v, k) => {
      if (!stripped.has(k.toLowerCase())) {
        headers.set(k, v);
      }
    });
    headers.set("Host", targetUrl.host);

    const init = {
      method: request.method,
      headers,
      redirect: "follow"
    };

    if (!["GET", "HEAD"].includes(request.method)) {
      init.body = await request.arrayBuffer();
    }

    const resp = await fetch(targetUrl.toString(), init);
    const respHeaders = new Headers(resp.headers);
    if (env.ALLOWED_ORIGINS) {
      respHeaders.set("Access-Control-Allow-Origin", env.ALLOWED_ORIGINS);
    }

    return new Response(resp.body, {
      status: resp.status,
      headers: respHeaders
    });
  }
};
