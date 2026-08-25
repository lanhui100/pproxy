#!/usr/bin/env node
// contract-smoke.mjs — M5 spec §7.1.2：对 dev 服务器实拉只读端点，响应过与
// 前端相同的 zod schema（契约双向夹住的"真值"侧）。本地运行，CI 不可达 tailnet。
//
// 用法：node scripts/contract-smoke.mjs <admin-base-url> <admin-token>
//   例：node scripts/contract-smoke.mjs http://<TAILNET_IP>:8900 pony_admin_xxx
//       （<admin-base-url> 缺省时读环境变量 TAILNET_ADMIN_BASE；tailnet 标识不入库）
// 凭据纪律：token 仅经 argv/环境变量传入，不落盘不进日志。
import { z } from 'zod'

const base = (process.argv[2] ?? process.env.TAILNET_ADMIN_BASE ?? '').replace(/\/$/, '')
const token = process.argv[3] ?? process.env.PONY_ADMIN_TOKEN ?? ''
if (!base || !token) {
  console.error('usage: contract-smoke.mjs <base-url> <token>')
  process.exit(2)
}

// 与 desktop/src/api/schemas.ts 保持同步的最小形状集（只读端点）
const Schemas = {
  '/api/health': z.object({
    status: z.string(),
    routes: z.record(z.string(), z.object({ enabled: z.boolean(), upstream: z.string() })),
    tokens_active: z.number(),
    db: z.string(),
  }),
  '/api/tokens': z.object({
    tokens: z.array(z.object({
      id: z.number(), name: z.string(), created_at: z.number(),
      expires_at: z.number().nullable(), revoked_at: z.number().nullable(),
      last_used_at: z.number().nullable(), status: z.string(),
    })),
  }),
  '/api/routes': z.object({
    routes: z.array(z.object({
      name: z.string(), target_host: z.string(), upstream: z.string().nullable(),
      override_upstream: z.string().nullable(), enabled: z.boolean(),
      created_at: z.number(), effective_upstream: z.string(),
    })),
  }),
  '/api/quota': z.object({
    snapshots: z.array(z.object({
      ts: z.number(), upstream: z.string(), metric: z.string(),
      used: z.number(), quota: z.number(), pct: z.number(),
    })),
    sources: z.array(z.object({
      name: z.string(), state: z.string(), last_ok: z.number().nullable(),
    })),
  }),
  '/api/alerts?limit=500': z.object({
    alerts: z.array(z.object({
      id: z.number(), ts: z.number(), level: z.string(),
      message: z.string(), read_at: z.number().nullable(),
    })),
  }),
  '/api/monitor/config': z.object({
    threshold_pct: z.number(),
    poll_interval_sec: z.number(),
  }),
}

let failed = 0
for (const [path, schema] of Object.entries(Schemas)) {
  const resp = await fetch(base + path, { headers: { Authorization: `Bearer ${token}` } })
  const body = await resp.json()
  if (!resp.ok) {
    console.log(`FAIL ${path} -> HTTP ${resp.status}`)
    failed++
    continue
  }
  const parsed = schema.safeParse(body)
  if (!parsed.success) {
    console.log(`FAIL ${path} -> shape mismatch: ${parsed.error.issues[0]?.path} ${parsed.error.issues[0]?.message}`)
    failed++
  } else {
    console.log(`PASS ${path}`)
  }
}
process.exit(failed ? 1 : 0)
