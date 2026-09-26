# Pony Proxy (pproxy)

> [English](README.en.md) | [中文](README.md)

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Self-hosted developer proxy for overseas access + intelligent API gateway:
- **Priority 1 (forward proxy)**: HTTP/HTTPS CONNECT tunneling over a WebSocket standby connection pool — one-command environment proxy (`pproxy on / off / status / env`), Windows desktop whitelist proxy, and mobile/all-platform HTTP node access with millisecond-level latency.
- **Priority 2 (reverse API gateway)**: LLM-first multi-upstream intelligent routing (`/{token}/{route}/*`), on-demand proxying, multi-node egress (CF Worker / Vercel AWS IPs), with usage accounting and token auth.
- **High availability & commercialization (cluster failover & multi-tenancy)**: Local HA Forwarder shielding local workloads with zero outage; zero-touch cluster mesh; Ed25519 asymmetric self-contained multi-tenant quota tokens (offline issuance & instant client quota display).

## Architecture overview

```
[Dev terminal / browser / mobile client]
   │
   ├─ 1. Forward proxy traffic (CONNECT / HTTP Proxy)
   │     └─ pony-engine (:8899 / :18900) ──[TunnelPool standby WS tunnels]──> gate.example.com (CF gate-worker)
   │           └─ Basic Auth / Token auth + Allowlist filtering ──> target overseas sites (TCP 443)
   │
   └─ 2. Reverse API gateway traffic (/{token}/{route}/*)
         └─ pony-server (dev server, 127.0.0.1:8899)
               ├─ anthropic/google/github/x/facebook → CF Worker  (edge.example.com)
               └─ openai/opencode                   → Vercel Functions (vedge.example.com, AWS egress)
```

- **Dual-mode architecture**: forward CONNECT tunnel (generic overseas access + all-platform proxy nodes) + reverse HTTP gateway (`/{token}/{route}/*path` zero-config client SDK).
- **Standby connection pool (TunnelPool)**: pre-built WebSocket sessions cut forward cold-connect latency to ~1 RTT.
- **Security & control**: mandatory Basic Auth / Token auth + Gatekeeper anti-brute-force rate limiting; routes/tokens/usage stored in SQLite (`~/.pony/state.db`).

## Quick start

### 1. Local development environment proxy (forward egress)

```bash
# enable persistent environment proxy (sets http_proxy / https_proxy / no_proxy)
pproxy on

# check proxy status + connectivity speed test
pproxy status

# temporarily suspend / resume
eval "$(pproxy env suspend)"
eval "$(pproxy env resume)"

# disable
pproxy off
```

### 2. Mobile & all-platform client access (as an egress node)

* **Start LAN sharing service**:
  ```bash
  pproxy serve --lan   # binds 0.0.0.0 and prints the LAN IP/port for your phone
  ```
* **Clash Meta one-scan import (easiest)**:
  ```bash
  pproxy clash         # generates config and prints a QR code; scan with Clash
  ```
* **System Wi-Fi proxy**: set HTTP proxy `http://<PC-LAN-IP>:8899` in Wi-Fi settings and enter credentials (Basic Auth or Token).
* **VPN clients (Clash Meta / Shadowrocket / Surge)**: add an HTTP proxy node pointing to `pproxy` for **global VPN** or **rule-based smart routing** via the TUN virtual NIC.

### 3. API reverse gateway usage

```bash
# create a data-plane token (plaintext shown only once)
curl -X POST http://127.0.0.1:8900/api/tokens \
  -H "Authorization: Bearer <admin_token>" -H "Content-Type: application/json" \
  -d '{"name":"my-laptop"}'

# usage example (Anthropic)
export ANTHROPIC_BASE_URL=http://127.0.0.1:8899/<token>/anthropic
export ANTHROPIC_API_KEY=<your-key>

# health check (admin plane)
curl http://127.0.0.1:8900/api/health -H "Authorization: Bearer <admin_token>"
```

- admin_token: printed once on first start, or injected via the `PPROXY_ADMIN_TOKEN` env var.
- Full protocol: [docs/ops/API.md](docs/ops/API.md).

### 4. Zero-Touch Cluster Mesh (distributed failover & one-command mesh)

Nodes automatically establish mesh networking via One-Time Join Tokens, enabling zero-config egress configuration sync and decentralized health monitoring:

```bash
# Generate a join token on the seed node (default 10m validity; seed addr -s and minutes -m configurable)
pproxy cluster token-create -s 100.95.193.103:8899 -m 30

# Join an existing cluster (auto-syncs egress endpoints and public key; --auto-start pulls up background daemon)
pproxy cluster join -t <token> --auto-start

# Inspect peer statuses and real-world latency in milliseconds
pproxy cluster status
```

- **Zero-Touch Bootstrap**: The token payload embeds egress tunnel URLs, credentials, and verification keys — new nodes self-heal and configure automatically without manual editing.
- **Live Mesh Dashboard**: `cluster status` concurrently probes peer reachability in real time, accurately displaying online/offline status and round-trip latency.

### 5. Multi-Tenant Ed25519 Token & Quota System

Designed for commercial proxy operations and team multi-tenancy with decentralized, asymmetric offline token issuance:

```bash
# Generate asymmetric keypair (private key stored strictly on management host at ~/.pony/cluster_signing_key.hex)
pproxy user keygen

# Issue a self-contained tenant token offline (supports 50G/100M, client displays quota and concurrency limits)
pproxy user add alice -q 50G -d 30 -c 3

# Revoke a token or user (added to blacklist with real-time hot-reloading across gateway nodes)
pproxy user revoke usr_alice
# Or revoke a specific token ID
pproxy user revoke tok-9f8e7d6c
```

- **Asymmetric Key Isolation**: Private signing keys never leave the administrative workstation, preventing token forging even if an edge node is compromised; edge nodes verify signatures locally via the public key.
- **Self-Contained & Instant Display**: Tokens embed `sub`, `quota_bytes`, `exp`, and `max_conns`. Desktop and CLI clients display user quotas directly without hitting a central database.
- **Real-Time Revocation**: `pproxy user revoke` writes to disk and hot-pushes to the live gateway, instantly blocking subsequent connections and profiles with 401/403.

### 6. Local HA Forwarder (transparent high-availability proxy stub)

```bash
# When running pproxy serve with remote peers configured, Local HA Forwarder is spawned automatically
pproxy serve
```

- **Zero-Outage Shield for Local Workloads**: Exclusively guards `127.0.0.1:8899`, ensuring local services (e.g. LLM gateway ponyllm, IDEs, terminal proxies) never encounter connection drops.
- **Zero-Latency Seamless Failover**: A background zero-wait health prober monitors the local engine; when the local engine crashes, restarts, or undergoes rolling upgrades, traffic instantly drifts to remote cluster peer nodes (e.g. RackNerd / remote nodes) with zero added delay.
- **HMAC Ticket Verification**: Cluster-wide failover traffic is authenticated via time-bound `X-Pony-Cluster-Ticket` headers signed with cluster HMAC keys and hardened with loopback bypass checks.

## Current routes (reverse API gateway)

Routes are managed hot in SQLite (`/api/routes` takes effect immediately). The initial 7 routes were migrated from config.json:

| Route | Target | Upstream |
|-------|--------|----------|
| /anthropic | api.anthropic.com | CF Worker |
| /google | www.google.com | CF Worker |
| /github | github.com | CF Worker |
| /x | api.twitter.com | CF Worker |
| /facebook | www.facebook.com | CF Worker |
| /openai | api.openai.com | Vercel |
| /opencode | opencode.ai | Vercel |

## Open-source deployment checklist: placeholders & credentials

> This repository is in **open-source-neutral form**: all private domains have been replaced with `*.example.com` placeholders, and real credentials are never committed (`.secrets.env` / `config.json` / `.pproxy.env` / `*.env.local` / `.vercel/` are all gitignored).
> Before cloning/self-hosting, fill in the tables below. **Unreplaced placeholders break deploys; missing credentials make the edges fail closed (403/401).**
> Also mirrored in [docs/ops/DEPLOY.md](docs/ops/DEPLOY.md).

### 1. Files that must have real domains restored before deploy

| File | Placeholder location | If not changed |
|---|---|---|
| `deploy/cf-worker/wrangler.toml` | `routes` → `pattern` | `wrangler deploy` fails |
| `deploy/cf-gate-worker/wrangler.toml` | comments & bound domain | wrong gate tunnel bridge domain |
| `deploy/cloudflared/config.yml` | `ingress.hostname` | wrong CF Tunnel entry domain |
| `desktop/src-tauri/tauri.conf.json` | updater `endpoints` | desktop auto-update broken |
| `scripts/install.sh` | CDN `get.example.com`, `GITHUB_RELEASE_BASE` | one-click installer 404 |
| `scripts/publish-desktop-dist.sh` / `sync-desktop-release.sh` | `dl` / `access` dist URLs | desktop release publish/sync fails |
| `scripts/m4_test.sh` | `PUBLIC_URL` | smoke test hits wrong endpoint |

### 2. Required credentials (env/secrets injection, never commit)

| Credential | Inject into | Notes |
|---|---|---|
| `PROXY_SECRET` | CF Worker `wrangler secret put PROXY_SECRET`; Vercel env `PROXY_SECRET` | shared upstream secret for edge/vedge; missing → 500 fail-closed |
| `GATE_TUNNEL_TOKEN` → `TUNNEL_TOKEN_HASH` | CF Gate Worker `wrangler versions secret put TUNNEL_TOKEN_HASH` + `wrangler versions deploy <version-id>` (wrangler 4); Vercel env `TUNNEL_TOKEN_HASH` (redeploy required after change) | tunnel Bearer auth; `TUNNEL_TOKEN_HASH = sha256(GATE_TUNNEL_TOKEN)` (`printf '%s'`), **must be identical on both live gates (CF+Vercel)** (VPS is a legacy un-deployed fallback, excluded); rotation runbook in `docs/ops/DEPLOY.md` |
| `VERCEL_TOKEN` / `VERCEL_ORG_ID` / `VERCEL_PROJECT_ID_EDGE` / `VERCEL_PROJECT_ID_GATE` | GitHub Actions secrets | for gitOps deploys via `.github/workflows/deploy-vercel.yml` |

### 3. Env vars overriding defaults (no code changes needed)

| Variable | Purpose |
|---|---|
| `PPROXY_EDGE_URL` / `PPROXY_VERCEL_URL` | `pproxy serve` upstream egress endpoints (placeholder defaults) |
| `PPROXY_DOWNLOAD_BASE` | `install.sh` binary download source (default GitHub Releases) |
| `PPROXY_DIST_BASE` | desktop static distribution base URL (`publish-desktop-dist.sh` writes it into latest.json as the download source; placeholder default) |
| `PONY_DIST_URL` | CLI self-update (`pproxy upgrade`) dist source |
| `PPROXY_DESKTOP_DIST_DIR` | `/dsk/` static distribution dir (default `/opt/pony-desktop-releases`) |
| `PPROXY_CONFIG` | server config file path (default `/etc/pproxy/config.json`) |
| `PPROXY_TUNNEL_GATE_URL` / `PPROXY_TUNNEL_TOKEN` / `PPROXY_TUNNEL_ALLOWLIST` | tunnel endpoint/token/allowlist (server-side `.pproxy.env`) |
| `PPROXY_LISTEN_ADMIN` | admin-plane listen address (tailnet rebind via systemd drop-in) |
| `PPROXY_SERVICE_USER` | user asserted by `m4_test.sh` (default `pproxy`) |

### 4. Desktop client configuration

The desktop (Tauri) needs no source changes: paste the **access code** (a `pony-gate://` connection code or a bare token; the code carries the endpoint) in Settings → Scheme A to enable the tunnel. Endpoints are persisted client-side; the in-code fallback defaults are placeholders only.

### 5. Credential hygiene (open-source red lines)

- Real values only in non-committed locations: `.secrets.env` (chmod 600), `config.json`, `.pproxy.env`, `*.env.local`, `.vercel/`.
- Rotation runbook for `GATE_TUNNEL_TOKEN` lives in `docs/ops/DEPLOY.md` (dual-gate parity + version trace + evidence before distributing codes), or the tunnel returns 401.
- This repo's git history has been credential-scrubbed (filter-repo); **never introduce any real token/secret literal in future commits**.

## Documentation index

| Doc | Content |
|------|--------|
| [docs/product/PRD.md](docs/product/PRD.md) | Product requirements & positioning (forward proxy first + reverse API gateway) |
| [docs/product/TECH_DESIGN.md](docs/product/TECH_DESIGN.md) | Target technical design |
| [docs/product/ROADMAP.md](docs/product/ROADMAP.md) | Milestones M1–M6 |
| [docs/architecture/CURRENT.md](docs/architecture/CURRENT.md) | Current system architecture (dual-mode topology & tunnels) |
| [docs/architecture/decisions/](docs/architecture/decisions/) | Architecture Decision Records (ADRs) |
| [docs/ops/DEPLOY.md](docs/ops/DEPLOY.md) | Deploy, update, rollback |
| [docs/ops/API.md](docs/ops/API.md) | Data-plane / admin-plane protocols |
| [docs/ops/TROUBLESHOOTING.md](docs/ops/TROUBLESHOOTING.md) | Troubleshooting manual |

## Code structure

```
crates/transport/ # cross-platform transport protocol (WS tunnel / standby pool / retry / ping-pong keepalive)
crates/engine/    # embedded gateway core engine (CONNECT tunnel / Basic&Token auth / anti-brute-force gatekeeper)
crates/core/      # store (SQLite) / token / route / usage / EdgeClient (upstream forwarding protocol)
crates/server/    # standalone daemon (gateway dispatch + admin API)
crates/cli/       # CLI toolkit (env proxy on/off/env/status, service mgmt, route & usage mgmt)
desktop/          # Windows desktop client (Tauri 2 + tray + whitelist proxy engine)
deploy/cf-worker/      # CF Worker (edge.example.com)
deploy/cf-gate-worker/ # CF Gate Worker (gate.example.com, WS↔TCP tunnel bridge)
deploy/vercel/         # Vercel Functions (vedge.example.com)
config.json       # runtime config (upstreams, secrets; routes migrated to SQLite; never commit)
systemd/          # pproxy.service
.secrets.env      # credentials (chmod 600, never commit)
```
