export const maxDuration = 300;

const SECRET = process.env.PROXY_SECRET || "<REDACTED_DEV_SECRET>";

const REQ_STRIP = new Set([
  "host", "connection", "keep-alive", "transfer-encoding", "upgrade",
  "content-length", "x-proxy-secret",
  "x-forwarded-for", "x-forwarded-proto", "x-forwarded-host",
  "x-real-ip", "forwarded", "via", "true-client-ip",
]);

const RESP_STRIP = new Set([
  "transfer-encoding", "connection", "content-length", "content-encoding",
]);

export default async function handler(req, res) {
  if (req.headers["x-proxy-secret"] !== SECRET) {
    return res.status(403).send("Unauthorized");
  }

  const target = req.query.url;
  if (!target) return res.status(400).send("Missing ?url=");

  let u;
  try {
    u = new URL(target);
  } catch {
    return res.status(400).send("Invalid URL");
  }
  if (!/^https?:$/.test(u.protocol)) return res.status(400).send("Invalid URL");

  const headers = {};
  for (const [k, v] of Object.entries(req.headers)) {
    if (!REQ_STRIP.has(k)) headers[k] = v;
  }
  headers["host"] = u.host;

  const chunks = [];
  for await (const c of req) chunks.push(c);
  const body = chunks.length ? Buffer.concat(chunks) : undefined;

  let upstream;
  try {
    upstream = await fetch(target, {
      method: req.method,
      headers,
      body: ["GET", "HEAD"].includes(req.method) ? undefined : body,
      redirect: "follow",
    });
  } catch (e) {
    return res.status(502).send("upstream error: " + e.message);
  }

  res.status(upstream.status);
  upstream.headers.forEach((v, k) => {
    if (!RESP_STRIP.has(k)) res.setHeader(k, v);
  });
  res.setHeader("x-proxy-edge", "vercel");

  const reader = upstream.body.getReader();
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      res.write(Buffer.from(value));
      await new Promise((r) => setImmediate(r));
    }
  } catch {}
  res.end();
}
