#!/usr/bin/env node
/**
 * backlog —— 批次日志任务账本 CLI（v3 / write-per-batch, fold-on-read）
 *
 * 为什么是 v3（2026-11-22 起）：v2 把全部任务写进同一个 `backlog/tasks.jsonl`，并用
 * `backlog/.next-id` 做全局计数器。只要两个并发会话/PR 都记账，就必然在同一文件的行尾与
 * 同一计数器上撞车（实证：#40 与 #41 的账本冲突）。v3 把「一次会话按发布面选出的任务批次」
 * 做成账本的一等写入单元：
 *
 *   写：`backlog/log/<批次>.jsonl`（只追加；不同会话 = 不同文件 ⇒ 合并零冲突）
 *   读：把所有批次 + 归档批次按时间折叠（fold）出现行视图，不落盘
 *
 * 布局（git-tracked 除 .locks/.state 外）：
 *   backlog/MODEL                              模型标记（内容 v3）
 *   backlog/log/<batchId>.jsonl                活跃批次日志（本会话只写自己的文件）
 *   backlog/archive/batches/<year>/<batchId>.jsonl  已归档批次（只读，按批封存）
 *   backlog/archive/batches/<year>/manifest.sha256  归档清单（sha256 + 相对路径，追加）
 *   backlog/archive/tasks-<YYYY>.jsonl         v2 遗留归档（只读，fold 时并入）
 *   backlog/.locks/<ID>.lock                   任务租约（本地运行时，不入仓）
 *   backlog/.state/batch                       当前工作区活动批次 id（本地运行时，不入仓）
 *
 * 事件（每行一个 JSON）：
 *   batch.start  {batch_id, surface, release?, created_at, session_id?}
 *   record       {at, task:<完整任务行>}
 *   dedup        {at, id, context?}            同 fingerprint 去重命中
 *   start        {at, id, owner, paths[], session_id?}
 *   release      {at, id, owner?}
 *   annotate     {at, id, description?, priority?}
 *   resolve      {at, id, evidence}
 *
 * 根解析：默认取**当前 worktree 顶层**（不是 git common dir）——每个分支/工作树写自己的
 * 批次日志并随 PR 提交；归档从各自检出树只读加载。`PONY_BACKLOG_ROOT` 可覆盖（测试/夹具）。
 *
 * 兼容：v2 仓不迁移也能继续用旧工具；本工具遇到 `tasks.jsonl` 且无 `MODEL` 时明确指向
 * `scripts/backlog-migrate-v2-v3.mjs`，绝不静默猜模型。
 */
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const MODEL = 'v3';
export const MODEL_FILE = 'MODEL';
export const BATCH_MODEL = 'backlog-v3';
export const ID_RE = /^B\d{3,}$/;
export const BATCH_ID_RE = /^\d{8}T\d{6}Z-[a-z0-9][a-z0-9-]{0,40}-[a-z0-9]{4}$/;
export const CATEGORIES = ['Defect', 'Task', 'Debt'];
export const PRIORITIES = ['P0', 'P1', 'P2', 'P3'];
export const STATUSES = ['todo', 'in_progress', 'done'];
const EVENT_KINDS = new Set(['batch.start', 'record', 'dedup', 'start', 'release', 'annotate', 'resolve']);
const TOP_KEYS = new Set([
  'id', 'category', 'priority', 'status', 'title', 'description',
  'created_at', 'updated_at', 'owner', 'touched_paths', 'evidence',
  'closed_at', 'archived_at', 'source_ref', 'metadata',
  'leased_at', 'heartbeat_at'
]);
const META_KEYS = new Set(['fingerprint', 'occurred_count', 'triage_context']);
const MAX_CONTEXT_BYTES = 8 * 1024;
const MAX_DESCRIPTION_CHARS = 2000;
export const DEFAULT_LEASE_TTL_MS = 2 * 60 * 60 * 1000;

const SURFACE_RE = /^[a-z0-9][a-z0-9-]*$/;

/** 解析账本根：默认当前 worktree 顶层；`PONY_BACKLOG_ROOT` 覆盖（夹具/测试）。 */
export function resolveLedgerRoot(env = process.env, cwd = process.cwd()) {
  if (env.PONY_BACKLOG_ROOT) return path.resolve(env.PONY_BACKLOG_ROOT);
  const top = execFileSync('git', ['rev-parse', '--show-toplevel'], { cwd, encoding: 'utf8' }).trim();
  return path.resolve(top);
}

export function ledgerPaths(root) {
  const backlogDir = path.join(root, 'backlog');
  return {
    root,
    backlogDir,
    model: path.join(backlogDir, MODEL_FILE),
    logsDir: path.join(backlogDir, 'log'),
    archiveDir: path.join(backlogDir, 'archive'),
    archivedBatchesDir: path.join(backlogDir, 'archive', 'batches'),
    locksDir: path.join(backlogDir, '.locks'),
    stateDir: path.join(backlogDir, '.state'),
    stateBatch: path.join(backlogDir, '.state', 'batch'),
    // v2 遗留（只读）
    legacyTasks: path.join(backlogDir, 'tasks.jsonl'),
    legacyNextId: path.join(backlogDir, '.next-id')
  };
}

function atomicWrite(file, content) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const tmp = `${file}.tmp-${process.pid}`;
  const fd = fs.openSync(tmp, 'w');
  try {
    fs.writeSync(fd, content);
    fs.fsyncSync(fd);
  } finally {
    fs.closeSync(fd);
  }
  fs.renameSync(tmp, file);
}

/** 追加一行到批次日志（append-only；每行独立 JSON，不重写历史）。 */
function appendLine(file, row) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.appendFileSync(file, JSON.stringify(row) + '\n');
}

export function readJsonl(file) {
  if (!fs.existsSync(file)) return [];
  const rows = [];
  for (const [index, line] of fs.readFileSync(file, 'utf8').split('\n').entries()) {
    if (!line.trim()) continue;
    try {
      rows.push(JSON.parse(line));
    } catch (error) {
      throw new Error(`${file}:${index + 1} 非法 JSON：${error.message}`);
    }
  }
  return rows;
}

export function writeJsonl(file, rows) {
  atomicWrite(file, rows.map(row => JSON.stringify(row)).join('\n') + (rows.length ? '\n' : ''));
}

/** 模型判定：`v3` / `v2`（有 tasks.jsonl 无 MODEL）/ `null`（未启用账本）。 */
export function readModel(paths) {
  if (fs.existsSync(paths.model)) {
    const value = fs.readFileSync(paths.model, 'utf8').trim();
    return value || MODEL;
  }
  if (fs.existsSync(paths.legacyTasks)) return 'v2';
  return null;
}

function requireModel(paths, { write = false } = {}) {
  const model = readModel(paths);
  if (model === MODEL) return;
  if (model === 'v2') {
    throw new Error(
      '检测到 v2 单文件账本（backlog/tasks.jsonl）。请先运行 `node scripts/backlog-migrate-v2-v3.mjs` 完成迁移' +
      (write ? '（写操作已拒绝）' : '')
    );
  }
  throw new Error('账本未启用（缺 backlog/MODEL）；新仓请先执行迁移脚本或按文档创建 v3 布局');
}

// ───────────────────────────── 批次 ─────────────────────────────

export function sanitizeSurface(raw, fallback = 'general') {
  const value = String(raw ?? '').trim().toLowerCase();
  if (!value) return fallback;
  const cleaned = value.replace(/[^a-z0-9-]/g, '-').replace(/-+/g, '-').replace(/^-|-$/g, '');
  if (!cleaned) return fallback;
  return cleaned.slice(0, 40);
}

function batchTimestamp(now = Date.now()) {
  return new Date(now).toISOString().replace(/[-:]/g, '').replace(/\.\d{3}Z$/, 'Z');
}

function randomSuffix() {
  return crypto.randomBytes(2).toString('hex').slice(0, 4);
}

export function newBatchId({ surface = 'general', now = Date.now() } = {}) {
  const s = sanitizeSurface(surface);
  return `${batchTimestamp(now)}-${s}-${randomSuffix()}`;
}

export function batchFile(paths, batchId) {
  return path.join(paths.logsDir, `${batchId}.jsonl`);
}

function archivedBatchFile(paths, year, batchId) {
  return path.join(paths.archivedBatchesDir, String(year), `${batchId}.jsonl`);
}

export function readCurrentBatchId(paths) {
  if (!fs.existsSync(paths.stateBatch)) return null;
  const value = fs.readFileSync(paths.stateBatch, 'utf8').trim();
  return value || null;
}

function setCurrentBatchId(paths, batchId) {
  atomicWrite(paths.stateBatch, `${batchId}\n`);
}

/** 打开（或按需创建）本工作区的活动批次；写命令共用同一批次文件。 */
export function ensureBatch(paths, { surface, release, sessionId } = {}) {
  const current = readCurrentBatchId(paths);
  if (current) {
    const file = batchFile(paths, current);
    if (fs.existsSync(file)) return { batchId: current, file, created: false };
  }
  const batchId = newBatchId({ surface });
  const file = batchFile(paths, batchId);
  appendLine(file, {
    kind: 'batch.start',
    model: BATCH_MODEL,
    batch_id: batchId,
    surface: sanitizeSurface(surface),
    ...(release ? { release } : {}),
    ...(sessionId ? { session_id: sessionId } : {}),
    created_at: new Date().toISOString()
  });
  setCurrentBatchId(paths, batchId);
  return { batchId, file, created: true };
}

function currentSessionId() {
  return process.env.DSH_SESSION_ID ?? process.env.PONY_BACKLOG_SESSION ?? null;
}

/** 向活动批次追加一条事件（自动创建批次）。 */
export function appendEvent(paths, event, options = {}) {
  const { batchId, file } = ensureBatch(paths, {
    surface: options.surface,
    release: options.release,
    sessionId: currentSessionId()
  });
  appendLine(file, event);
  return { batchId, file };
}

// ───────────────────────────── 读模型：fold ─────────────────────────────

function listFiles(dir, pattern) {
  if (!fs.existsSync(dir)) return [];
  return fs.readdirSync(dir)
    .filter(name => pattern.test(name))
    .sort()
    .map(name => path.join(dir, name));
}

/** 解析一个批次文件：`{ batchId, meta, events, problems, file, archived }`。 */
function parseBatch(file, { archived = false } = {}) {
  const problems = [];
  const rows = readJsonl(file);
  const stem = path.basename(file, '.jsonl');
  const first = rows[0];
  if (!first || first.kind !== 'batch.start') {
    problems.push(`批次 ${stem}: 首行必须是 batch.start`);
    return { batchId: stem, meta: null, events: [], problems, file, archived };
  }
  if (first.batch_id !== stem) problems.push(`批次 ${stem}: batch.start.batch_id=${first.batch_id} 与文件名不一致`);
  if (first.model !== BATCH_MODEL) problems.push(`批次 ${stem}: batch.start.model 必须是 ${BATCH_MODEL}`);
  if (!SURFACE_RE.test(String(first.surface ?? ''))) problems.push(`批次 ${stem}: surface 非法（${first.surface}）`);
  if (typeof first.created_at !== 'string' || Number.isNaN(Date.parse(first.created_at))) {
    problems.push(`批次 ${stem}: batch.start.created_at 必须是 ISO 时间`);
  }
  let lastAt = 0;
  const events = rows.slice(1);
  for (const [index, event] of events.entries()) {
    if (!event || typeof event !== 'object' || Array.isArray(event)) {
      problems.push(`批次 ${stem}:${index + 2} 事件必须是对象`);
      continue;
    }
    if (!EVENT_KINDS.has(event.kind)) problems.push(`批次 ${stem}:${index + 2} 未知事件 \`${event.kind}\``);
    if (typeof event.at === 'string') {
      const at = Date.parse(event.at);
      if (Number.isNaN(at)) problems.push(`批次 ${stem}:${index + 2} at 非法时间`);
      else if (at < lastAt) problems.push(`批次 ${stem}:${index + 2} 事件时间倒退`);
      else lastAt = at;
    } else {
      problems.push(`批次 ${stem}:${index + 2} 事件缺 at（ISO 时间）`);
    }
  }
  return { batchId: stem, meta: first, events, problems, file, archived };
}

function collectBatches(paths) {
  const active = listFiles(paths.logsDir, /\.jsonl$/).map(file => parseBatch(file));
  const archived = [];
  if (fs.existsSync(paths.archivedBatchesDir)) {
    for (const year of fs.readdirSync(paths.archivedBatchesDir).sort()) {
      const dir = path.join(paths.archivedBatchesDir, year);
      if (!fs.statSync(dir).isDirectory()) continue;
      for (const file of listFiles(dir, /\.jsonl$/)) archived.push(parseBatch(file, { archived: true }));
    }
  }
  return [...active, ...archived];
}

function legacyArchiveRows(paths) {
  return listFiles(paths.archiveDir, /^tasks-\d{4}\.jsonl$/).flatMap(file => readJsonl(file));
}

function applyEvent(state, event, batch) {
  const problems = [];
  const at = event.at;
  switch (event.kind) {
    case 'record': {
      const task = event.task;
      if (!task || typeof task !== 'object') {
        problems.push(`批次 ${batch.batchId} record 缺 task`);
        break;
      }
      if (state.rows.has(task.id)) {
        problems.push(`重复 record：${task.id}（已出现于批次 ${state.rows.get(task.id)._batch}）`);
      } else {
        state.rows.set(task.id, {
          ...JSON.parse(JSON.stringify(task)),
          _batch: batch.batchId,
          _surface: batch.meta.surface,
          _recorded_at: at
        });
      }
      break;
    }
    case 'dedup': {
      const row = state.rows.get(event.id);
      if (!row) { problems.push(`dedup 指向不存在的任务：${event.id}`); break; }
      row.metadata = row.metadata ?? { fingerprint: 'unknown', occurred_count: 1 };
      row.metadata.occurred_count = (row.metadata.occurred_count ?? 1) + 1;
      row.updated_at = at;
      if (event.context) row.metadata.triage_context = event.context;
      break;
    }
    case 'start': {
      const row = state.rows.get(event.id);
      if (!row) { problems.push(`start 指向不存在的任务：${event.id}`); break; }
      if (row.status === 'done') { problems.push(`已闭环任务不得再次 start：${event.id}`); break; }
      row.status = 'in_progress';
      row.owner = event.owner;
      row.touched_paths = Array.isArray(event.paths) ? event.paths : [];
      row.leased_at = row.leased_at ?? at;
      row.heartbeat_at = at;
      row.updated_at = at;
      break;
    }
    case 'release': {
      const row = state.rows.get(event.id);
      if (!row) { problems.push(`release 指向不存在的任务：${event.id}`); break; }
      if (row.status === 'done') { problems.push(`已闭环任务不得 release：${event.id}`); break; }
      row.status = 'todo';
      row.updated_at = at;
      break;
    }
    case 'annotate': {
      const row = state.rows.get(event.id);
      if (!row) { problems.push(`annotate 指向不存在的任务：${event.id}`); break; }
      if (event.description !== undefined) row.description = event.description;
      if (event.priority !== undefined) row.priority = event.priority;
      row.updated_at = at;
      break;
    }
    case 'resolve': {
      const row = state.rows.get(event.id);
      if (!row) { problems.push(`resolve 指向不存在的任务：${event.id}`); break; }
      if (row.status === 'done') { problems.push(`重复 resolve：${event.id}`); break; }
      row.status = 'done';
      row.evidence = event.evidence;
      row.closed_at = at;
      row.archived_at = at;
      row.updated_at = at;
      break;
    }
    default:
      break;
  }
  return problems;
}

/**
 * 折叠出当前视图（模型无关入口）：v3 → 批次日志；v2 → 单文件只读视图。
 *
 * 返回 `{ rows: Map(id → row), active: [...], done: [...], batches: [...], problems, notes, counts, model }`。
 * v3 排序口径：批次 `created_at` 升序，同刻按 batchId 字典序；批内按行序。
 */
export function fold(paths) {
  const model = readModel(paths);
  if (model === 'v2') return foldV2(paths);
  return foldV3(paths);
}

/** v2 单文件账本 → 只读视图（迁移前/迁移对账用；不做任何写入）。 */
export function foldV2(paths) {
  const problems = [];
  const active = readJsonl(paths.legacyTasks).map(row => ({ ...row, _batch: 'v2-tasks', _surface: 'legacy-v2' }));
  const done = legacyArchiveRows(paths).map(row => ({ ...row, _batch: 'v2-archive', _surface: 'legacy-v2' }));
  const rows = new Map();
  for (const row of [...active, ...done]) {
    if (!row || typeof row.id !== 'string') continue;
    if (rows.has(row.id)) problems.push(`ID 重复：${row.id}（同时出现于活跃池/归档）`);
    rows.set(row.id, row);
  }
  for (const row of active) {
    if (row.status === 'done') problems.push(`${row.id} 活跃池不得残留 done（应走 resolve 归档）`);
  }
  return {
    rows,
    active,
    done,
    batches: [],
    problems,
    notes: [],
    model: 'v2',
    counts: { active: active.length, done: done.length, batches: 0, archivedBatches: 0, legacyArchived: done.length }
  };
}

/** v3 批次日志 → 折叠视图。 */
export function foldV3(paths) {
  const problems = [];
  const notes = [];
  const batches = collectBatches(paths);
  for (const batch of batches) problems.push(...batch.problems);
  batches.sort((a, b) => {
    const at = String(a.meta?.created_at ?? '').localeCompare(String(b.meta?.created_at ?? ''));
    return at !== 0 ? at : a.batchId.localeCompare(b.batchId);
  });

  const rows = new Map();
  // v2 遗留归档：只读历史，先入表（done 终态）。
  for (const row of legacyArchiveRows(paths)) {
    if (!row || typeof row.id !== 'string') continue;
    rows.set(row.id, { ...row, _batch: 'legacy-archive', _surface: 'legacy' });
  }
  const state = { rows };
  for (const batch of batches) {
    if (!batch.meta) continue;
    for (const event of batch.events) {
      problems.push(...applyEvent(state, event, batch));
    }
  }
  const active = [...rows.values()].filter(row => row.status !== 'done');
  const done = [...rows.values()].filter(row => row.status === 'done');
  for (const row of active) {
    for (const e of validateTask(row)) problems.push(`${row.id} ${e}`);
  }
  for (const batch of batches) {
    const touched = new Set();
    for (const event of batch.events) {
      if (event.id) touched.add(event.id);
      if (event.kind === 'record' && event.task?.id) touched.add(event.task.id);
    }
    batch.touched = [...touched];
    batch.all_done = touched.size > 0 && [...touched].every(id => rows.get(id)?.status === 'done');
  }
  return {
    rows,
    active,
    done,
    batches,
    problems,
    notes,
    model: 'v3',
    counts: {
      active: active.length,
      done: done.length,
      batches: batches.filter(b => !b.archived).length,
      archivedBatches: batches.filter(b => b.archived).length,
      legacyArchived: legacyArchiveRows(paths).length
    }
  };
}

// ───────────────────────────── 记录校验 ─────────────────────────────

/** 单条任务校验：返回错误数组（空数组 = 合法）。字段口径与 v2 完全一致。 */
export function validateTask(row, { archived = false } = {}) {
  const errors = [];
  if (!row || typeof row !== 'object' || Array.isArray(row)) return ['必须是对象'];
  for (const key of Object.keys(row)) {
    if (key.startsWith('_')) continue; // fold 附加标记
    if (!TOP_KEYS.has(key)) errors.push(`未知字段 \`${key}\``);
  }
  if (typeof row.id !== 'string' || !ID_RE.test(row.id)) errors.push(`id 必须匹配 ${ID_RE}`);
  if (!CATEGORIES.includes(row.category)) errors.push(`category 必须是 ${CATEGORIES.join('|')}`);
  if (!PRIORITIES.includes(row.priority)) errors.push(`priority 必须是 ${PRIORITIES.join('|')}`);
  if (!STATUSES.includes(row.status)) errors.push(`status 必须是 ${STATUSES.join('|')}`);
  if (typeof row.title !== 'string' || !row.title.trim()) errors.push('title 必填');
  if (typeof row.created_at !== 'string' || Number.isNaN(Date.parse(row.created_at))) {
    errors.push('created_at 必填且为 ISO 时间');
  }
  if (!row.metadata || typeof row.metadata !== 'object') {
    errors.push('metadata 必填（含 fingerprint）');
  } else {
    for (const key of Object.keys(row.metadata)) {
      if (!META_KEYS.has(key)) errors.push(`metadata 未知字段 \`${key}\``);
    }
    if (typeof row.metadata.fingerprint !== 'string' || !row.metadata.fingerprint.trim()) {
      errors.push('metadata.fingerprint 必填');
    }
    const ctx = row.metadata.triage_context;
    if (ctx !== undefined) {
      const bytes = Buffer.byteLength(JSON.stringify(ctx), 'utf8');
      if (bytes > MAX_CONTEXT_BYTES) errors.push(`triage_context 超限（${bytes} > ${MAX_CONTEXT_BYTES} 字节）`);
    }
  }
  if (row.description !== undefined &&
      (typeof row.description !== 'string' || row.description.length > MAX_DESCRIPTION_CHARS)) {
    errors.push(`description 必须是 ≤${MAX_DESCRIPTION_CHARS} 字符的字符串`);
  }
  if (row.status === 'in_progress') {
    if (typeof row.owner !== 'string' || !row.owner.trim()) errors.push('in_progress 必填 owner');
    if (!Array.isArray(row.touched_paths) || row.touched_paths.length === 0) {
      errors.push('in_progress 必填 touched_paths（≥1 个仓内相对路径）');
    }
  }
  if (row.status === 'done') {
    if (typeof row.evidence !== 'string' || !row.evidence.trim()) errors.push('done 必填 evidence');
    if (typeof row.closed_at !== 'string' || Number.isNaN(Date.parse(row.closed_at))) {
      errors.push('done 必填 closed_at（ISO 时间）');
    }
  }
  if (archived && row.status !== 'done') errors.push('归档记录必须是 done');
  return errors;
}

// ───────────────────────────── 租约（与 v2 同语义） ─────────────────────────────

function lockFile(paths, id) {
  return path.join(paths.locksDir, `${id}.lock`);
}

export function readLease(paths, id) {
  const file = lockFile(paths, id);
  if (!fs.existsSync(file)) return null;
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
}

export function isLeaseTrackingActive(paths) {
  return fs.existsSync(paths.locksDir);
}

export function acquireLease(paths, id, { owner, sessionId, ttlMs = DEFAULT_LEASE_TTL_MS, now = Date.now() } = {}) {
  if (!owner) throw new Error('租约必须提供 owner（会话标识）');
  const existing = readLease(paths, id);
  if (existing) {
    if (existing.owner !== owner) {
      throw new Error(`任务 ${id} 已被 ${existing.owner} 持有（${existing.acquired_at}），不得并发编辑；如确认其已失效可先 reap`);
    }
    return heartbeatLease(paths, id, { owner, now });
  }
  fs.mkdirSync(paths.locksDir, { recursive: true });
  const payload = {
    task_id: id,
    owner,
    session_id: sessionId ?? owner,
    pid: process.pid,
    acquired_at: new Date(now).toISOString(),
    heartbeat_at: new Date(now).toISOString(),
    ttl_ms: ttlMs
  };
  try {
    const fd = fs.openSync(lockFile(paths, id), 'wx');
    try {
      fs.writeSync(fd, JSON.stringify(payload));
      fs.fsyncSync(fd);
    } finally {
      fs.closeSync(fd);
    }
  } catch (error) {
    if (error.code === 'EEXIST') throw new Error(`任务 ${id} 的租约被并发抢占，请重试`);
    throw error;
  }
  return payload;
}

export function heartbeatLease(paths, id, { owner, now = Date.now() } = {}) {
  const cur = readLease(paths, id);
  if (!cur) throw new Error(`任务 ${id} 没有活跃租约`);
  if (owner && cur.owner !== owner) throw new Error(`任务 ${id} 的租约属于 ${cur.owner}`);
  const next = { ...cur, heartbeat_at: new Date(now).toISOString() };
  atomicWrite(lockFile(paths, id), JSON.stringify(next));
  return next;
}

export function releaseLease(paths, id, { owner } = {}) {
  const cur = readLease(paths, id);
  if (!cur) return { released: false };
  if (owner && cur.owner !== owner) throw new Error(`任务 ${id} 的租约属于 ${cur.owner}，不得释放`);
  fs.rmSync(lockFile(paths, id), { force: true });
  return { released: true, previous: cur };
}

export function reapStaleLeases(paths, { maxAgeMs = DEFAULT_LEASE_TTL_MS, now = Date.now(), dryRun = false, onlyId } = {}) {
  if (!fs.existsSync(paths.locksDir)) return [];
  const reaped = [];
  for (const name of fs.readdirSync(paths.locksDir)) {
    if (!name.endsWith('.lock')) continue;
    if (onlyId && name !== `${onlyId}.lock`) continue;
    const file = path.join(paths.locksDir, name);
    const lease = readLease(paths, name.replace(/\.lock$/, ''));
    const beat = lease?.heartbeat_at ? Date.parse(lease.heartbeat_at) : fs.statSync(file).mtimeMs;
    if (Number.isFinite(beat) && now - beat > maxAgeMs) {
      if (!dryRun) fs.rmSync(file, { force: true });
      reaped.push(lease?.task_id ?? name.replace(/\.lock$/, ''));
    }
  }
  return reaped;
}

/** 记录状态与本地租约的一致性；口径与 v2 一致（干净克隆只提示，不罚）。 */
export function leaseConsistency(paths, rows) {
  const problems = [];
  const notes = [];
  const tracking = isLeaseTrackingActive(paths);
  for (const row of rows) {
    const lease = readLease(paths, row.id);
    if (row.status !== 'in_progress') {
      if (lease && tracking) {
        problems.push(`${row.id} 存在租约锁但记录状态为 ${row.status}（疑似手改状态或 start 中断；确认后 reap --id ${row.id} --minutes 0 只清该锁）`);
      }
      continue;
    }
    if (lease) {
      if (row.owner && lease.owner !== row.owner) {
        problems.push(`${row.id} 租约与记录 owner 冲突：记录=${row.owner} 锁=${lease.owner}`);
      }
      continue;
    }
    if (tracking) {
      problems.push(`${row.id} 记录为 in_progress 但缺少租约锁文件（疑似手动改状态）`);
    } else {
      notes.push(`${row.id} in_progress 无本地租约目录（干净克隆/CI 常态：租约不入仓，跳过锁文件校验）`);
    }
  }
  return { problems, notes };
}

function orphanLeaseIds(paths, activeRows) {
  if (!fs.existsSync(paths.locksDir)) return [];
  const live = new Set(activeRows.map(row => row.id));
  return fs.readdirSync(paths.locksDir)
    .filter(name => name.endsWith('.lock'))
    .map(name => name.replace(/\.lock$/, ''))
    .filter(id => !live.has(id));
}

// ───────────────────────────── 写命令 ─────────────────────────────

function allocateId(state) {
  let candidate = Date.now();
  while (state.rows.has(`B${candidate}`)) candidate += 1;
  return `B${candidate}`;
}

function requireActiveRow(state, id) {
  const row = state.rows.get(id);
  if (!row) throw new Error(`未找到 ${id}（fold 视图里没有这条任务）`);
  return row;
}

export function recordTask(paths, { category, priority = 'P1', title, description, fingerprint, context, surface, release, sessionId }) {
  requireModel(paths, { write: true });
  if (!CATEGORIES.includes(category)) throw new Error(`category 必须是 ${CATEGORIES.join('|')}`);
  if (!PRIORITIES.includes(priority)) throw new Error(`priority 必须是 ${PRIORITIES.join('|')}`);
  if (!title || !title.trim()) throw new Error('title 必填');
  if (!fingerprint || !fingerprint.trim()) throw new Error('fingerprint 必填（用于去重）');
  const state = fold(paths);
  const now = new Date().toISOString();
  const existing = state.active.find(row =>
    row.metadata?.fingerprint === fingerprint && (row.status === 'todo' || row.status === 'in_progress'));
  if (existing) {
    const { batchId } = appendEvent(paths, { kind: 'dedup', at: now, id: existing.id, ...(context ? { context } : {}) }, { surface });
    return { type: 'dedup', row: existing, batchId };
  }
  const row = {
    id: allocateId(state),
    category,
    priority,
    status: 'todo',
    title: title.trim(),
    created_at: now,
    metadata: { fingerprint, occurred_count: 1, ...(context ? { triage_context: context } : {}) },
    ...(description ? { description } : {}),
    ...(sessionId ? { source_ref: sessionId } : {})
  };
  const errors = validateTask(row);
  if (errors.length) throw new Error(`记录非法：${errors.join('；')}`);
  const { batchId } = appendEvent(paths, { kind: 'record', at: now, task: row }, { surface, release });
  return { type: 'created', row, batchId };
}

export function startTask(paths, { id, owner, sessionId, touchedPaths, surface }) {
  requireModel(paths, { write: true });
  const state = fold(paths);
  const row = requireActiveRow(state, id);
  if (row.status === 'done') throw new Error(`${id} 已闭环（done 是终态，回归请开新任务并引用旧 ID）`);
  if (!owner) throw new Error('进入 in_progress 必须提供 owner');
  if (!touchedPaths?.length) throw new Error('进入 in_progress 必须提供 touched_paths');
  const lease = acquireLease(paths, id, { owner, sessionId: sessionId ?? owner });
  const now = new Date().toISOString();
  const { batchId } = appendEvent(paths, {
    kind: 'start',
    at: now,
    id,
    owner,
    paths: [...new Set(touchedPaths)],
    ...(sessionId ? { session_id: sessionId } : {})
  }, { surface });
  return { id, owner, batchId, lease };
}

export function releaseTask(paths, { id, owner, surface }) {
  requireModel(paths, { write: true });
  const state = fold(paths);
  const row = requireActiveRow(state, id);
  if (row.status === 'done') throw new Error(`${id} 已闭环，无需释放`);
  const lease = releaseLease(paths, id, { owner });
  const now = new Date().toISOString();
  const { batchId } = appendEvent(paths, { kind: 'release', at: now, id, ...(owner ? { owner } : {}) }, { surface });
  return { id, batchId, lease };
}

export function annotateTask(paths, { id, description, priority, surface }) {
  requireModel(paths, { write: true });
  const state = fold(paths);
  requireActiveRow(state, id);
  if (description === undefined && priority === undefined) throw new Error('annotate 至少提供 --description 或 --priority');
  if (priority !== undefined && !PRIORITIES.includes(priority)) throw new Error(`priority 必须是 ${PRIORITIES.join('|')}`);
  const now = new Date().toISOString();
  const { batchId } = appendEvent(paths, {
    kind: 'annotate',
    at: now,
    id,
    ...(description !== undefined ? { description } : {}),
    ...(priority !== undefined ? { priority } : {})
  }, { surface });
  return { id, batchId };
}

export function resolveTask(paths, { id, evidence, surface }) {
  requireModel(paths, { write: true });
  if (!evidence || !evidence.trim()) throw new Error('resolve 必须提供 evidence');
  const state = fold(paths);
  const row = requireActiveRow(state, id);
  if (row.status === 'done') throw new Error(`重复 resolve：${id} 已是 done`);
  const now = new Date().toISOString();
  const { batchId } = appendEvent(paths, { kind: 'resolve', at: now, id, evidence: evidence.trim() }, { surface });
  const lease = releaseLease(paths, id);
  return { id, batchId, lease };
}

// ───────────────────────────── 校验与对账 ─────────────────────────────

/** 校验入口（模型无关）：v3 → 批次+租约+归档清单；v2 → 单文件只读校验（迁移期兼容）。 */
export function lintAll(paths) {
  const model = readModel(paths);
  if (model === 'v2') return lintV2(paths);
  if (model !== MODEL) {
    return { ok: false, problems: ['账本未启用（缺 backlog/MODEL 与 tasks.jsonl）'], notes: [], counts: {} };
  }
  return lintV3(paths);
}

/** v2 单文件只读校验：字段不变量 + 活跃池 done 残留 + ID 重复 + 高水位 + 租约一致性。 */
export function lintV2(paths) {
  const problems = [];
  const state = foldV2(paths);
  problems.push(...state.problems);
  for (const row of state.active) {
    for (const e of validateTask(row)) problems.push(`tasks.jsonl (${row?.id ?? '?'}) ${e}`);
  }
  for (const row of state.done) {
    for (const e of validateTask(row, { archived: true })) problems.push(`archive/${row?.id ?? '?'} ${e}`);
  }
  const leaseProblems = leaseConsistency(paths, state.active);
  problems.push(...leaseProblems.problems);
  const highWater = fs.existsSync(paths.legacyNextId)
    ? Number.parseInt(fs.readFileSync(paths.legacyNextId, 'utf8').trim(), 10)
    : 0;
  if (highWater > 0) {
    for (const row of [...state.active, ...state.done]) {
      const n = Number.parseInt(String(row?.id ?? '').replace(/^B/, ''), 10);
      if (Number.isFinite(n) && n >= highWater) {
        problems.push(`ID ${row.id} 越过高水位（下一个候选 B${String(highWater).padStart(3, '0')}），疑似手工改号`);
      }
    }
  }
  return {
    ok: problems.length === 0,
    problems,
    notes: leaseProblems.notes,
    counts: state.counts
  };
}

/** v3 批次日志校验：批次结构 + fold 不变量 + 遗留归档 + 租约一致性 + 批次 id 唯一。 */
export function lintV3(paths) {
  const problems = [];
  const state = foldV3(paths);
  problems.push(...state.problems);
  for (const row of state.active) {
    if (row.status === 'done') problems.push(`${row.id} 活跃视图残留 done`);
  }
  for (const row of legacyArchiveRows(paths)) {
    for (const e of validateTask(row, { archived: true })) problems.push(`遗留归档 ${row?.id ?? '?'} ${e}`);
  }
  const leaseProblems = leaseConsistency(paths, state.active);
  problems.push(...leaseProblems.problems);
  const stems = new Map();
  for (const batch of state.batches) {
    const seen = stems.get(batch.batchId) ?? 0;
    if (seen > 0) problems.push(`批次 id 重复：${batch.batchId}（同时出现在活跃与归档目录）`);
    stems.set(batch.batchId, seen + 1);
  }
  return {
    ok: problems.length === 0,
    problems,
    notes: leaseProblems.notes,
    counts: state.counts
  };
}

export function verifyArchive(paths) {
  const problems = [];
  const checked = [];
  if (!fs.existsSync(paths.archivedBatchesDir)) return { ok: true, problems, checked };
  const listed = new Set();
  for (const year of fs.readdirSync(paths.archivedBatchesDir).sort()) {
    const dir = path.join(paths.archivedBatchesDir, year);
    if (!fs.statSync(dir).isDirectory()) continue;
    const manifestFile = path.join(dir, 'manifest.sha256');
    if (!fs.existsSync(manifestFile)) {
      problems.push(`归档目录缺清单：${path.relative(paths.root, manifestFile)}`);
      continue;
    }
    for (const line of fs.readFileSync(manifestFile, 'utf8').split('\n')) {
      if (!line.trim()) continue;
      const match = /^([0-9a-f]{64})\s{2}(.+)$/.exec(line);
      if (!match) { problems.push(`清单行非法：${path.relative(paths.root, manifestFile)} → ${line}`); continue; }
      const [, sha, rel] = match;
      listed.add(rel);
      const abs = path.resolve(paths.root, rel);
      if (!fs.existsSync(abs)) { problems.push(`清单登记但文件缺失：${rel}`); continue; }
      const actual = crypto.createHash('sha256').update(fs.readFileSync(abs)).digest('hex');
      if (actual !== sha) problems.push(`归档文件被改动（sha256 不符）：${rel}`);
      checked.push(rel);
    }
    for (const file of listFiles(dir, /\.jsonl$/)) {
      const rel = path.relative(paths.root, file).split(path.sep).join('/');
      if (!listed.has(rel)) problems.push(`归档文件未入清单：${rel}`);
    }
  }
  return { ok: problems.length === 0, problems, checked };
}

export function doctor(paths) {
  const report = { ok: true, issues: [], counts: {}, archive: null, notes: [] };
  const model = readModel(paths);
  report.counts.model = model;
  const state = fold(paths);
  report.counts = { ...report.counts, ...state.counts };
  if (model !== MODEL && model !== 'v2') report.issues.push(`账本未启用（模型=${model ?? 'null'}）`);
  report.issues.push(...state.problems);
  const ids = new Map();
  for (const row of state.rows.values()) ids.set(row.id, (ids.get(row.id) ?? 0) + 1);
  for (const [id, n] of ids) if (n > 1) report.issues.push(`ID 重复 ${id}×${n}`);
  for (const id of orphanLeaseIds(paths, state.active)) {
    report.issues.push(`残留租约 ${id}.lock：活跃视图无此记录（node scripts/backlog.mjs reap --id ${id} --minutes 0 清理）`);
  }
  report.archive = verifyArchive(paths);
  report.issues.push(...report.archive.problems);
  const lint = lintAll(paths);
  report.issues.push(...lint.problems);
  report.notes = lint.notes ?? [];
  report.ok = report.issues.length === 0;
  return report;
}

// ───────────────────────────── 归档 ─────────────────────────────

/**
 * 按批次归档：把「该批次引用到的任务全部 done」的活跃批次搬进归档目录并封存。
 *
 * 时机（ADR 约定）：批次随某个版本发布之后，由发版 housekeeping 或每周兜底任务执行；
 * 未闭环批次一律不归档（否则要在归档后开新批次补正）。归档只动**已全部闭环**的批次，
 * 因此在途分支的活跃批次文件与归档目录永不重叠 ⇒ 仍然零冲突。
 */
export function archiveBatches(paths, { release, before, dryRun = false } = {}) {
  requireModel(paths, { write: true });
  const state = fold(paths);
  const candidates = [];
  for (const batch of state.batches) {
    if (batch.archived || !batch.meta) continue;
    if (!batch.all_done) continue;
    if (release && batch.meta.release !== release) continue;
    if (before) {
      const at = Date.parse(batch.meta.created_at);
      if (!Number.isFinite(at) || at >= Date.parse(before)) continue;
    }
    candidates.push(batch);
  }
  const moved = [];
  for (const batch of candidates) {
    const year = String(batch.meta.created_at).slice(0, 4);
    const target = archivedBatchFile(paths, year, batch.batchId);
    const rel = path.relative(paths.root, target).split(path.sep).join('/');
    if (dryRun) { moved.push(rel); continue; }
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.renameSync(batch.file, target);
    const sha = crypto.createHash('sha256').update(fs.readFileSync(target)).digest('hex');
    const manifest = path.join(path.dirname(target), 'manifest.sha256');
    fs.appendFileSync(manifest, `${sha}  ${rel}\n`);
    if (readCurrentBatchId(paths) === batch.batchId) fs.rmSync(paths.stateBatch, { force: true });
    moved.push(rel);
  }
  return { ok: true, problems: [], moved, candidates: candidates.map(b => b.batchId), dryRun };
}

// ───────────────────────────── 视图 ─────────────────────────────

function formatRow(row) {
  const owner = row.owner ? ` @${row.owner}` : '';
  const surface = row._surface && row._surface !== 'legacy' ? ` ·${row._surface}` : '';
  return `${row.id} [${row.priority}/${row.status}] ${row.category}${owner}${surface} — ${row.title}`;
}

export function listRows(paths, { surface } = {}) {
  const model = readModel(paths);
  if (model !== MODEL && model !== 'v2') throw new Error('账本未启用（缺 backlog/MODEL）');
  const state = fold(paths);
  let rows = [...state.active];
  if (surface) rows = rows.filter(row => row._surface === sanitizeSurface(surface));
  const order = { in_progress: 0, todo: 1, done: 2 };
  rows.sort((a, b) => (order[a.status] - order[b.status]) ||
    PRIORITIES.indexOf(a.priority) - PRIORITIES.indexOf(b.priority) ||
    String(a.id).localeCompare(String(b.id)));
  return rows;
}

// ───────────────────────────── CLI ─────────────────────────────

export function main(argv = process.argv.slice(2)) {
  const root = resolveLedgerRoot();
  const paths = ledgerPaths(root);
  const [command, ...rest] = argv;
  const arg = (name, fallback) => {
    const i = rest.indexOf(`--${name}`);
    return i === -1 ? fallback : rest[i + 1];
  };
  const surface = arg('surface', process.env.PONY_BACKLOG_SURFACE);
  switch (command) {
    case 'record': {
      const res = recordTask(paths, {
        category: arg('category', arg('type', 'Task')),
        priority: arg('priority', 'P1'),
        title: arg('title', rest.filter(x => !x.startsWith('--'))[0]),
        description: arg('description'),
        fingerprint: arg('fingerprint', 'manual'),
        surface,
        release: arg('release'),
        sessionId: arg('session', currentSessionId() ?? undefined)
      });
      console.log(res.type === 'dedup'
        ? `[backlog] 去重命中：${res.row.id}（频次 ${res.row.metadata.occurred_count}，批次 ${res.batchId}）`
        : `[backlog] 已登记 ${res.row.id}：${res.row.title}（批次 ${res.batchId}）`);
      console.log(`[backlog] 记得把 backlog/log/${res.batchId}.jsonl 与本 PR 一起提交`);
      return 0;
    }
    case 'start': {
      const id = rest[0];
      const owner = arg('owner', currentSessionId() ?? process.env.USER ?? 'me');
      const res = startTask(paths, {
        id,
        owner,
        sessionId: arg('session', owner),
        touchedPaths: (arg('paths') ?? '').split(',').map(s => s.trim()).filter(Boolean),
        surface
      });
      console.log(`[backlog] ${res.id} → in_progress @${res.owner}（租约已获取，批次 ${res.batchId}）`);
      return 0;
    }
    case 'release': {
      const res = releaseTask(paths, { id: rest[0], owner: arg('owner'), surface });
      console.log(`[backlog] ${res.id} → todo（租约已释放，批次 ${res.batchId}）`);
      return 0;
    }
    case 'heartbeat': {
      const lease = heartbeatLease(paths, rest[0], { owner: arg('owner') });
      console.log(`[backlog] ${lease.task_id} 心跳 ${lease.heartbeat_at}`);
      return 0;
    }
    case 'reap': {
      const usage = '用法: backlog reap [--id <B###>] [--minutes N] [--dry-run]（未知参数一律拒绝，不会回落为全局清理）';
      let onlyId;
      let minutes = '120';
      let dryRun = false;
      for (let i = 0; i < rest.length; i += 1) {
        const token = rest[i];
        if (token === '--dry-run') { dryRun = true; continue; }
        if (token === '--id') {
          const value = rest[i + 1];
          if (!value || !ID_RE.test(value)) { console.error(usage); return 2; }
          onlyId = value; i += 1; continue;
        }
        if (token === '--minutes') {
          const value = rest[i + 1];
          if (!value || !/^\d+$/.test(value)) { console.error(usage); return 2; }
          minutes = value; i += 1; continue;
        }
        console.error(usage);
        return 2;
      }
      const reaped = reapStaleLeases(paths, { maxAgeMs: Number(minutes) * 60_000, dryRun, onlyId });
      const scope = onlyId ? `（仅 ${onlyId}）` : '（全部租约）';
      if (reaped.length) {
        console.log(dryRun
          ? `[backlog] [dry-run] 将回收超龄租约${scope}（未执行）：${reaped.join(', ')}`
          : `[backlog] 回收超龄租约${scope}：${reaped.join(', ')}`);
      } else {
        console.log(dryRun ? `[backlog] [dry-run] 无超龄租约${scope}` : `[backlog] 无超龄租约${scope}`);
      }
      return 0;
    }
    case 'annotate': {
      const res = annotateTask(paths, {
        id: rest[0],
        description: arg('description'),
        priority: arg('priority'),
        surface
      });
      console.log(`[backlog] ${res.id} 已更新注释（批次 ${res.batchId}）`);
      return 0;
    }
    case 'resolve': {
      const evidence = arg('evidence', rest.slice(1).filter(x => !x.startsWith('--')).join(' '));
      const res = resolveTask(paths, { id: rest[0], evidence, surface });
      console.log(`[backlog] ${res.id} 已闭环（批次 ${res.batchId}）`);
      if (res.lease.released) console.log(`[backlog] 租约已释放（原持有者 @${res.lease.previous.owner}）`);
      return 0;
    }
    case 'list': {
      const rows = listRows(paths, { surface });
      console.log(`=== backlog (${rows.length} 条活跃${surface ? `，surface=${sanitizeSurface(surface)}` : ''}) ===`);
      for (const row of rows) console.log(formatRow(row));
      return 0;
    }
    case 'fold': {
      const state = fold(paths);
      console.log(JSON.stringify({
        model: readModel(paths),
        counts: state.counts,
        active: state.active.map(row => ({ id: row.id, status: row.status, surface: row._surface, batch: row._batch })),
        batches: state.batches.map(batch => ({
          batchId: batch.batchId,
          surface: batch.meta?.surface,
          release: batch.meta?.release,
          created_at: batch.meta?.created_at,
          archived: batch.archived,
          touched: batch.touched,
          all_done: batch.all_done
        })),
        problems: state.problems
      }, null, 2));
      return state.problems.length ? 1 : 0;
    }
    case 'lint': {
      const res = lintAll(paths);
      for (const n of res.notes) console.error(`[backlog-lint] NOTE ${n}`);
      if (!res.ok) {
        for (const p of res.problems) console.error(`[backlog-lint] ${p}`);
        console.error(`[backlog-lint] FAIL（${res.problems.length} 项）`);
        return 1;
      }
      console.log(`[backlog-lint] PASS（活跃 ${res.counts.active} / 已闭环 ${res.counts.done} / 批次 ${res.counts.batches} 活跃 + ${res.counts.archivedBatches} 归档）`);
      return 0;
    }
    case 'doctor': {
      const report = doctor(paths);
      console.log(JSON.stringify({ root, ...report }, null, 2));
      return report.ok ? 0 : 1;
    }
    case 'archive': {
      const res = archiveBatches(paths, {
        release: arg('release'),
        before: arg('before'),
        dryRun: rest.includes('--dry-run')
      });
      if (!res.moved.length) {
        console.log('[backlog] 没有可归档批次（需要批次内任务全部 done）');
        return 0;
      }
      for (const rel of res.moved) console.log(`[backlog] ${res.dryRun ? '[dry-run] 将归档' : '已归档'} ${rel}`);
      console.log(`[backlog] 共 ${res.moved.length} 个批次${res.dryRun ? '（dry-run，未改动磁盘）' : '，清单已追加'}`);
      return 0;
    }
    case 'verify-archive': {
      const res = verifyArchive(paths);
      if (!res.ok) {
        for (const p of res.problems) console.error(`[backlog-archive] ${p}`);
        console.error(`[backlog-archive] FAIL（${res.problems.length} 项）`);
        return 1;
      }
      console.log(`[backlog-archive] PASS（校验 ${res.checked.length} 个归档批次）`);
      return 0;
    }
    case 'batch': {
      const [sub] = rest;
      if (sub === 'show') {
        const id = readCurrentBatchId(paths);
        if (!id) { console.log('[backlog] 当前工作区没有活动批次（首次 record/start 时自动创建）'); return 0; }
        console.log(id);
        return 0;
      }
      if (sub === 'new') {
        fs.rmSync(paths.stateBatch, { force: true });
        const { batchId, file } = ensureBatch(paths, {
          surface,
          release: arg('release'),
          sessionId: currentSessionId() ?? undefined
        });
        console.log(`[backlog] 已创建批次 ${batchId} → ${path.relative(root, file)}`);
        return 0;
      }
      console.log('用法: backlog batch {new [--surface <s>] [--release <v>]|show}');
      return 1;
    }
    default:
      console.log('用法: backlog {record|list [--surface <s>]|start|heartbeat|release|resolve|annotate|reap|fold|lint|doctor|archive [--release <v>] [--before <ISO>] [--dry-run]|verify-archive|batch {new|show}}');
      return 1;
  }
}

/**
 * 是否作为 CLI 直接执行（`node scripts/backlog.mjs <cmd>`）而非被 import。
 *
 * 旧写法 `path.resolve(new URL(import.meta.url).pathname)` 在 Windows 上把
 * `/C:/repo/scripts/backlog.mjs` 这类带前导斜杠的 URL pathname 交给 path.resolve，
 * 得到与 `process.argv[1]` 永不相等的结果（CLI 静默不执行）。现两侧都先 realpath 再比。
 */
function isDirectInvocation() {
  const entry = process.argv[1];
  if (!entry) return false;
  try {
    const entryUrl = pathToFileURL(fs.realpathSync(entry)).href;
    const selfUrl = pathToFileURL(fs.realpathSync(fileURLToPath(import.meta.url))).href;
    return process.platform === 'win32'
      ? entryUrl.toLowerCase() === selfUrl.toLowerCase()
      : entryUrl === selfUrl;
  } catch {
    return false;
  }
}

const isDirect = isDirectInvocation();
if (isDirect) {
  try {
    process.exit(main());
  } catch (error) {
    console.error(`[backlog] ${error.message}`);
    process.exit(1);
  }
}
