export default {
  async fetch(request, env, ctx) {
    if (request.method === "OPTIONS") {
      return new Response(null, {
        headers: {
          "Access-Control-Allow-Origin": env.ALLOWED_ORIGINS,
          "Access-Control-Allow-Methods": "GET,POST,PUT,DELETE,PATCH,CONNECT,OPTIONS",
          "Access-Control-Allow-Headers": "*",
          "Access-Control-Max-Age": "86400"
        }
      });
    }

    const auth = request.headers.get("X-Proxy-Secret");
    if (auth !== env.PROXY_SECRET) {
      return new Response("Unauthorized", { status: 403 });
    }

    const url = new URL(request.url);
    const target = url.searchParams.get("url");
    if (!target) return new Response("Missing ?url=", { status: 400 });

    let targetUrl;
    try { targetUrl = new URL(target); } catch { return new Response("Invalid URL", { status: 400 }); }

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

    if (!["GET","HEAD"].includes(request.method)) {
      init.body = await request.arrayBuffer();
    }

    const resp = await fetch(targetUrl.toString(), init);
    const respHeaders = new Headers(resp.headers);
    respHeaders.set("Access-Control-Allow-Origin", env.ALLOWED_ORIGINS);
    respHeaders.set("X-Proxy-Edge", "cf-worker");

    return new Response(resp.body, {
      status: resp.status,
      headers: respHeaders
    });
  }
};
