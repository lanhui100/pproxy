#!/usr/bin/env node
/**
 * backlog —— 单一 JSONL 任务账本 CLI（v3）
 *
 * 真源：<repo-root>/backlog/tasks.jsonl（一行一任务，git-tracked）
 * 归档：<repo-root>/backlog/archive/tasks-<YYYY>.jsonl（滚动切片，幂等追加）
 * 高水位：<repo-root>/backlog/.next-id（单调不减，永不回退）
 *
 * 作用域：账本按「仓库」单例。解析根时使用 git common dir，使同一仓库的
 * 所有 worktree（含 detached / 临时 worktree）读写同一份账本，杜绝任务在不同
 * 工作树之间混淆。测试可用 PONY_BACKLOG_ROOT 覆盖。
 *
 * 设计约束（来自三路对抗审查结论）：
 * - 校验只看记录不变量，不解析 git log（避免自锁与 fail-open）；
 * - done 只需 evidence + closed_at，不写入自指的 closing_commit；
 * - ID 由持久高水位分配：归档/删除都不会让编号复用；
 * - 归档追加幂等：同 id 已存在则跳过，重放安全；
 * - 只做整文件原子替换（tmp + rename），不做跨文件"原子剪切"承诺。
 */
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const ID_RE = /^B\d{3,}$/;
export const CATEGORIES = ['Defect', 'Task', 'Debt'];
export const PRIORITIES = ['P0', 'P1', 'P2', 'P3'];
export const STATUSES = ['todo', 'in_progress', 'done'];
const TOP_KEYS = new Set([
  'id', 'category', 'priority', 'status', 'title', 'description',
  'created_at', 'updated_at', 'owner', 'touched_paths', 'evidence',
  'closed_at', 'archived_at', 'source_ref', 'metadata',
  'leased_at', 'heartbeat_at'
]);
const META_KEYS = new Set(['fingerprint', 'occurred_count', 'triage_context']);
const MAX_CONTEXT_BYTES = 8 * 1024;
const MAX_DESCRIPTION_CHARS = 2000;

/** 解析仓库根：优先环境变量，其次 git common dir 的父目录（跨 worktree 单例）。 */
export function resolveLedgerRoot(env = process.env, cwd = process.cwd()) {
  if (env.PONY_BACKLOG_ROOT) return path.resolve(env.PONY_BACKLOG_ROOT);
  const commonDir = execFileSync('git', ['rev-parse', '--git-common-dir'], {
    cwd, encoding: 'utf8'
  }).trim();
  const abs = path.isAbsolute(commonDir) ? commonDir : path.resolve(cwd, commonDir);
  return path.dirname(abs);
}

export function ledgerPaths(root) {
  return {
    root,
    tasks: path.join(root, 'backlog', 'tasks.jsonl'),
    archiveDir: path.join(root, 'backlog', 'archive'),
    nextId: path.join(root, 'backlog', '.next-id'),
    locksDir: path.join(root, 'backlog', '.locks')
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

/** `.next-id` 语义：**下一个候选编号**（不是最后已用编号），保证删除归档也不会回退复用。 */
export function readHighWater(paths) {
  if (!fs.existsSync(paths.nextId)) return 0;
  const raw = fs.readFileSync(paths.nextId, 'utf8').trim();
  const n = Number.parseInt(raw, 10);
  return Number.isFinite(n) && n > 0 ? n : 0;
}

/** 单调推进高水位：把 `.next-id` 提升到 `usedId + 1`，永不回退。 */
export function writeHighWater(paths, usedId) {
  const used = Number.parseInt(String(usedId).replace(/^B/, ''), 10);
  if (!Number.isFinite(used)) return;
  const prev = readHighWater(paths);
  const next = used + 1;
  if (next > prev) atomicWrite(paths.nextId, `${next}\n`);
}

export function archiveFileFor(paths, year = new Date().getUTCFullYear()) {
  return path.join(paths.archiveDir, `tasks-${year}.jsonl`);
}

export function readArchives(paths) {
  if (!fs.existsSync(paths.archiveDir)) return [];
  return fs.readdirSync(paths.archiveDir)
    .filter(name => /^tasks-\d{4}\.jsonl$/.test(name))
    .sort()
    .flatMap(name => readJsonl(path.join(paths.archiveDir, name)));
}

/** 单条记录校验：返回错误数组（空数组 = 合法）。 */
export function validateRecord(row, { archived = false } = {}) {
  const errors = [];
  const at = row && typeof row === 'object' ? 'record' : 'record';
  if (!row || typeof row !== 'object' || Array.isArray(row)) return [`${at}: 必须是对象`];
  for (const key of Object.keys(row)) {
    if (!TOP_KEYS.has(key)) errors.push(`未知字段 \`${key}\``);
  }
  if (typeof row.id !== 'string' || !ID_RE.test(row.id)) errors.push(`id 必须匹配 ${ID_RE}`);
  if (!CATEGORIES.includes(row.category)) errors.push(`category 必须是 ${CATEGORIES.join('|')}`);
  if (!PRIORITIES.includes(row.priority)) errors.push(`priority 必须是 ${PRIORITIES.join('|')}`);
  if (!STATUSES.includes(row.status)) errors.push(`status 必须是 ${STATUSES.join('|')}`);
  if (typeof row.title !== 'string' || !row.title.trim()) errors.push('title 必填');
  if (typeof row.created_at !== 'string' || Number.isNaN(Date.parse(row.created_at))) {
    errors.push('created_at 必填且为 ISO 时间（顶层字段）');
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

export function lintAll(paths) {
  const problems = [];
  const active = readJsonl(paths.tasks);
  const archived = readArchives(paths);
  for (const [i, row] of active.entries()) {
    for (const e of validateRecord(row)) problems.push(`tasks.jsonl:${i + 1} (${row?.id ?? '?'}) ${e}`);
    if (row.status === 'done') {
      problems.push(`tasks.jsonl:${i + 1} (${row.id}) 活跃池不得残留 done（应走 resolve 归档）`);
    }
  }
  for (const row of archived) {
    for (const e of validateRecord(row, { archived: true })) problems.push(`archive/${row?.id ?? '?'} ${e}`);
  }
  const seen = new Map();
  for (const row of [...active, ...archived]) {
    if (!row || typeof row.id !== 'string') continue;
    if (seen.has(row.id)) {
      problems.push(`ID 重复：${row.id}（同时出现于活跃池/归档，需人工裁决）`);
    } else {
      seen.set(row.id, true);
    }
  }
  const leaseProblems = leaseConsistency(paths, active);
  problems.push(...leaseProblems.problems);
  const nextId = readHighWater(paths);
  if (nextId > 0) {
    for (const row of [...active, ...archived]) {
      const n = Number.parseInt(String(row?.id ?? '').replace(/^B/, ''), 10);
      if (Number.isFinite(n) && n >= nextId) {
        problems.push(`ID ${row.id} 越过高水位（下一个候选 B${String(nextId).padStart(3, '0')}），疑似手工改号`);
      }
    }
  }
  return {
    ok: problems.length === 0,
    problems,
    notes: leaseProblems.notes,
    counts: { active: active.length, archived: archived.length }
  };
}

/** 分配下一个 ID：持久高水位与扫描值取大，保证单调不减。 */
function allocateId(paths) {
  const rows = [...readJsonl(paths.tasks), ...readArchives(paths)];
  const maxSeen = rows.reduce((max, row) => {
    const n = Number.parseInt(String(row?.id ?? '').replace(/^B/, ''), 10);
    return Number.isFinite(n) ? Math.max(max, n) : max;
  }, 0);
  const candidate = Math.max(readHighWater(paths), maxSeen + 1, 56);
  atomicWrite(paths.nextId, `${candidate + 1}\n`);
  return `B${String(candidate).padStart(3, '0')}`;
}

export function record(paths, { category, priority = 'P1', title, description, fingerprint, context, sessionId }) {
  if (!CATEGORIES.includes(category)) throw new Error(`category 必须是 ${CATEGORIES.join('|')}`);
  if (!PRIORITIES.includes(priority)) throw new Error(`priority 必须是 ${PRIORITIES.join('|')}`);
  if (!title || !title.trim()) throw new Error('title 必填');
  if (!fingerprint || !fingerprint.trim()) throw new Error('fingerprint 必填（用于去重）');
  const rows = readJsonl(paths.tasks);
  const now = new Date().toISOString();
  const existing = rows.find(row =>
    row.metadata?.fingerprint === fingerprint && (row.status === 'todo' || row.status === 'in_progress'));
  if (existing) {
    existing.metadata.occurred_count = (existing.metadata.occurred_count ?? 1) + 1;
    existing.updated_at = now;
    if (context) existing.metadata.triage_context = context;
    writeJsonl(paths.tasks, rows);
    return { type: 'dedup', row: existing };
  }
  const row = {
    id: allocateId(paths),
    category,
    priority,
    status: 'todo',
    title: title.trim(),
    created_at: now,
    metadata: { fingerprint, occurred_count: 1, ...(context ? { triage_context: context } : {}) },
    ...(description ? { description } : {}),
    ...(sessionId ? { source_ref: sessionId } : {})
  };
  const errors = validateRecord(row);
  if (errors.length) throw new Error(`记录非法：${errors.join('；')}`);
  rows.push(row);
  writeJsonl(paths.tasks, rows);
  return { type: 'created', row };
}


/** 任务级租约：同一任务同时只允许一个会话持有；不同任务可并行。 */
export const DEFAULT_LEASE_TTL_MS = 2 * 60 * 60 * 1000;

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

/**
 * 本工作区是否在跟踪租约。
 *
 * `backlog/.locks/` 是**本地运行时目录**（不入仓，见 .gitignore）：租约内容含
 * pid / session_id / 心跳时间戳，入仓只会带来每次 start 的 diff 噪音与并发冲突，
 * 因此干净克隆、CI、他人工作区里必然不存在。
 *
 * 只有它存在时，「记录为 in_progress 却缺锁文件」才可能是本地手改状态；
 * 不存在时该信号在干净环境恒为真，会让 governance lint 在任何克隆上恒红（B069）。
 */
export function isLeaseTrackingActive(paths) {
  return fs.existsSync(paths.locksDir);
}

/**
 * 记录状态与本地租约的一致性校验。
 *
 * 返回 `{ problems, notes }`：`problems` 阻断门禁，`notes` 只提示不阻断。
 * owner 冲突是硬错（两侧数据都在手，可比对）；锁文件缺失在无租约跟踪的工作区
 * 只能是提示——否则干净克隆恒红。
 */
export function leaseConsistency(paths, rows) {
  const problems = [];
  const notes = [];
  const tracking = isLeaseTrackingActive(paths);
  for (const row of rows) {
    const lease = readLease(paths, row.id);
    if (row.status !== 'in_progress') {
      // 反向缺口（D1/D2）：锁还在、记录却不是 in_progress——手改状态降级，或 start 取锁后 update 失败。
      // 只在本工作区确有租约目录时判硬错；干净克隆 .locks/ 不存在，lease 恒为 null。
      if (lease && tracking) {
        problems.push(`tasks.jsonl (${row.id}) 存在租约锁但记录状态为 ${row.status}（疑似手改状态或 start 中断；确认后 reap --id ${row.id} --minutes 0 只清该锁，或手动删除 .locks/${row.id}.lock）`);
      }
      continue;
    }
    if (lease) {
      if (row.owner && lease.owner !== row.owner) {
        problems.push(`tasks.jsonl (${row.id}) 租约与记录 owner 冲突：记录=${row.owner} 锁=${lease.owner}`);
      }
      continue;
    }
    if (!row.heartbeat_at) continue;
    const beat = Date.parse(row.heartbeat_at);
    if (!Number.isFinite(beat) || Date.now() - beat >= DEFAULT_LEASE_TTL_MS) continue;
    if (tracking) {
      problems.push(`tasks.jsonl (${row.id}) 记录为 in_progress 但缺少租约锁文件（疑似手动改状态）`);
    } else {
      notes.push(`tasks.jsonl (${row.id}) in_progress 无本地租约目录（干净克隆/CI 常态：租约不入仓，跳过锁文件校验）`);
    }
  }
  return { problems, notes };
}

/**
 * 获取任务租约（O_EXCL 原子创建）。
 * 已有他人租约且未过期 → 抛错；同 owner 重入 → 刷新心跳。
 */
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

/** 回收超龄租约（会话崩溃/无心跳）：返回被回收的任务 id 列表。`onlyId` 限定单个任务（并发 lane 安全）。 */
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

const TRANSITIONS = new Map([
  ['todo', new Set(['in_progress'])],
  ['in_progress', new Set(['in_progress', 'todo'])]
]);

export function update(paths, id, { status, owner, touchedPaths }) {
  const rows = readJsonl(paths.tasks);
  const row = rows.find(r => r.id === id);
  if (!row) throw new Error(`未找到 ${id}`);
  if (status === 'done') throw new Error('done 必须走 resolve（需 evidence 并归档）');
  if (status && !TRANSITIONS.get(row.status)?.has(status)) {
    throw new Error(`非法跃迁 ${row.status} → ${status}`);
  }
  if (status) row.status = status;
  if (status === 'in_progress') {
    if (!owner) throw new Error('进入 in_progress 必须提供 owner');
    if (!touchedPaths?.length) throw new Error('进入 in_progress 必须提供 touched_paths');
    row.owner = owner;
    row.touched_paths = [...new Set(touchedPaths)];
    row.leased_at = row.leased_at ?? new Date().toISOString();
    row.heartbeat_at = new Date().toISOString();
  }
  row.updated_at = new Date().toISOString();
  const errors = validateRecord(row);
  if (errors.length) throw new Error(`记录非法：${errors.join('；')}`);
  writeJsonl(paths.tasks, rows);
  return row;
}

export function resolve(paths, id, evidence) {
  if (!evidence || !evidence.trim()) throw new Error('resolve 必须提供 evidence');
  const rows = readJsonl(paths.tasks);
  const index = rows.findIndex(r => r.id === id);
  if (index === -1) throw new Error(`未找到 ${id}`);
  const row = { ...rows[index], status: 'done', evidence: evidence.trim(), closed_at: new Date().toISOString() };
  row.updated_at = row.closed_at;
  row.archived_at = row.closed_at;
  const errors = validateRecord(row);
  if (errors.length) throw new Error(`记录非法：${errors.join('；')}`);
  // 先写归档（幂等）再删活跃：重放安全，断电最坏是「双份」，doctor 可发现。
  const archiveFile = archiveFileFor(paths, Number(row.closed_at.slice(0, 4)));
  const archived = readJsonl(archiveFile);
  if (!archived.some(item => item.id === id)) {
    archived.push(row);
    writeJsonl(archiveFile, archived);
  }
  rows.splice(index, 1);
  writeJsonl(paths.tasks, rows);
  // 闭环同样推进高水位：`.next-id` 是「下一个候选编号」，若闭环最高编号的任务后不回写，
  // `.next-id` 会落后于「活跃∪归档」的最大编号，`lint` 随即判「ID 越过高水位（疑似手工改号）」
  // ——governance CI 因此在每次「闭环最高编号任务且未再开新任务」后恒红（实测 B066 案例）。
  writeHighWater(paths, id);
  // 闭环是终态，租约随之失效（B081）：resolve 只归档不清锁，会让 `.locks/` 里的
  // `<ID>.lock` 一直留到 TTL 被 reap 回收——期间该 ID 对其它会话是「被占用」的假信号，
  // 且 `isLeaseTrackingActive` 因目录常驻而永久走 lint 严格支。
  // 释放排在归档落盘之后：反过来（先清锁再归档）一旦中途失败，会留下「仍在活跃池却无人持锁」
  // 的伪手改态；当前顺序最坏只是多留一个可 reap 的锁，方向安全。
  const lease = releaseLease(paths, id);
  return { row, archiveFile, lease };
}

/**
 * 残留锁判定：`.locks/<ID>.lock` 存在、但活跃池已无该 id（B081）。
 *
 * 正常路径下 `resolve` 归档时会即时释放租约，能走到这里的只剩「会话在归档后崩溃」或
 * 「本单修复前的历史遗留」。残留锁要等 TTL（默认 2h）才被 reap，期间任何会话复用该 id
 * 都会撞锁，而空目录 git 不可见 ⇒ 只有 doctor 能提前发现。
 */
function orphanLeaseIds(paths, activeRows) {
  if (!fs.existsSync(paths.locksDir)) return [];
  const live = new Set(activeRows.map(row => row.id));
  return fs.readdirSync(paths.locksDir)
    .filter(name => name.endsWith('.lock'))
    .map(name => name.replace(/\.lock$/, ''))
    .filter(id => !live.has(id));
}

export function doctor(paths) {
  const report = { ok: true, issues: [], counts: {} };
  const active = readJsonl(paths.tasks);
  const archived = readArchives(paths);
  report.counts = { active: active.length, archived: archived.length, highWater: readHighWater(paths) };
  const ids = new Map();
  for (const row of [...active, ...archived]) {
    ids.set(row.id, (ids.get(row.id) ?? 0) + 1);
  }
  for (const [id, n] of ids) if (n > 1) report.issues.push(`ID 重复 ${id}×${n}`);
  for (const row of active) if (row.status === 'done') report.issues.push(`${row.id} 活跃池残留 done`);
  const maxSeen = [...ids.keys()].reduce((max, id) => {
    const n = Number.parseInt(id.replace(/^B/, ''), 10);
    return Number.isFinite(n) ? Math.max(max, n) : max;
  }, 0);
  const nextId = readHighWater(paths);
  if (nextId === 0 && maxSeen > 0) report.issues.push('高水位文件缺失（.next-id）');
  if (nextId > 0 && nextId <= maxSeen) report.issues.push(`高水位 B${String(nextId).padStart(3, '0')}（下一候选）未越过最大编号 B${String(maxSeen).padStart(3, '0')}`);
  report.leases = active
    .map(row => ({ id: row.id, lease: readLease(paths, row.id) }))
    .filter(item => item.lease);
  for (const id of orphanLeaseIds(paths, active)) {
    report.issues.push(`残留租约 ${id}.lock：活跃池无此记录（node scripts/backlog.mjs reap --id ${id} --minutes 0 清理，或手动删除 .locks/${id}.lock）`);
  }
  const lint = lintAll(paths);
  report.issues.push(...lint.problems);
  report.notes = lint.notes;
  report.ok = report.issues.length === 0;
  return report;
}

/** 一次性迁移：把旧 triage 账本的 7 条记录重编号为高水位段并规范化字段。 */
export function normalizeLegacy(legacyRows, { startId = 56 } = {}) {
  return legacyRows.map((row, index) => {
    const created = row.created_at ?? row.metadata?.created_at ?? new Date().toISOString();
    const base = {
      id: `B${String(startId + index).padStart(3, '0')}`,
      category: (row.metadata?.triage_context ?? row.context) ? 'Defect' : (row.category ?? 'Task'),
      priority: row.priority ?? 'P1',
      status: ['done', 'resolved', 'closed'].includes(row.status)
        ? 'done'
        : (['in_progress', 'doing'].includes(row.status) ? 'in_progress' : 'todo'),
      title: row.title,
      created_at: created,
      metadata: {
        fingerprint: row.metadata?.fingerprint ?? row.fingerprint ?? 'unknown',
        occurred_count: row.metadata?.occurred_count ?? row.occurred_count ?? 1,
        ...(row.metadata?.triage_context ?? row.context
          ? { triage_context: row.metadata?.triage_context ?? row.context }
          : {})
      },
      source_ref: row.session_id ?? row.source_ref ?? 'legacy-triage'
    };
    if (base.status === 'done') {
      base.evidence = row.evidence ?? 'legacy triage: 迁移前已闭环（见 .agents/issue-triage/archive）';
      base.closed_at = row.updated_at ?? created;
      base.archived_at = base.closed_at;
    }
    if (base.status === 'in_progress') {
      base.owner = 'legacy-triage';
      base.touched_paths = ['crates/pony-agent-core'];
    }
    return base;
  });
}

export function loadLedger(root) {
  const paths = ledgerPaths(root);
  return { paths, active: readJsonl(paths.tasks) };
}

function formatRow(row) {
  const owner = row.owner ? ` @${row.owner}` : '';
  return `${row.id} [${row.priority}/${row.status}] ${row.category}${owner} — ${row.title}`;
}

export function list(paths) {
  const rows = readJsonl(paths.tasks);
  const order = { in_progress: 0, todo: 1, done: 2 };
  rows.sort((a, b) => (order[a.status] - order[b.status]) ||
    PRIORITIES.indexOf(a.priority) - PRIORITIES.indexOf(b.priority) ||
    a.id.localeCompare(b.id));
  return rows;
}

export function main(argv = process.argv.slice(2)) {
  const root = resolveLedgerRoot();
  const paths = ledgerPaths(root);
  const [command, ...rest] = argv;
  const arg = (name, fallback) => {
    const i = rest.indexOf(`--${name}`);
    return i === -1 ? fallback : rest[i + 1];
  };
  switch (command) {
    case 'record': {
      const res = record(paths, {
        category: arg('category', arg('type', 'Task')),
        priority: arg('priority', 'P1'),
        title: arg('title', rest.filter(x => !x.startsWith('--'))[0]),
        description: arg('description'),
        fingerprint: arg('fingerprint', 'manual'),
        sessionId: arg('session')
      });
      console.log(res.type === 'dedup'
        ? `[backlog] 去重命中：${res.row.id}（频次 ${res.row.metadata.occurred_count}）`
        : `[backlog] 已登记 ${res.row.id}：${res.row.title}`);
      return 0;
    }
    case 'list': {
      const rows = list(paths);
      console.log(`=== backlog (${rows.length} 条活跃) ===`);
      for (const row of rows) console.log(formatRow(row));
      return 0;
    }
    case 'start': {
      const id = rest[0];
      const owner = arg('owner', process.env.DSH_SESSION_ID ?? process.env.USER ?? 'me');
      acquireLease(paths, id, { owner, sessionId: arg('session', owner) });
      const row = update(paths, id, {
        status: 'in_progress',
        owner,
        touchedPaths: (arg('paths') ?? '').split(',').map(s => s.trim()).filter(Boolean)
      });
      console.log(`[backlog] ${row.id} → in_progress @${row.owner}（租约已获取）`);
      return 0;
    }
    case 'release': {
      const id = rest[0];
      releaseLease(paths, id, { owner: arg('owner') });
      const row = update(paths, id, { status: 'todo' });
      console.log(`[backlog] ${row.id} → todo（租约已释放）`);
      return 0;
    }
    case 'heartbeat': {
      const lease = heartbeatLease(paths, rest[0], { owner: arg('owner') });
      console.log(`[backlog] ${lease.task_id} 心跳 ${lease.heartbeat_at}`);
      return 0;
    }
    case 'reap': {
      // 破坏性清理：参数白名单解析。未知 token（--id=B056 / --ide / -id / 位置参数）一律 exit 2，
      // 绝不静默回落为「清全部」。
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
      const reaped = reapStaleLeases(paths, {
        maxAgeMs: Number(minutes) * 60_000,
        dryRun,
        onlyId
      });
      const scope = onlyId ? `（仅 ${onlyId}）` : '（全部租约）';
      if (reaped.length) {
        console.log(dryRun
          ? `[backlog] [dry-run] 将回收超龄租约${scope}（未执行，未删除任何锁）：${reaped.join(', ')}`
          : `[backlog] 回收超龄租约${scope}：${reaped.join(', ')}`);
      } else {
        console.log(dryRun ? `[backlog] [dry-run] 无超龄租约${scope}` : `[backlog] 无超龄租约${scope}`);
      }
      return 0;
    }
    case 'annotate': {
      const id = rest[0];
      const rows = readJsonl(paths.tasks);
      const row = rows.find(r => r.id === id);
      if (!row) throw new Error(`未找到 ${id}`);
      const description = arg('description');
      if (description !== undefined) row.description = description;
      const priority = arg('priority');
      if (priority !== undefined) row.priority = priority;
      row.updated_at = new Date().toISOString();
      const errors = validateRecord(row);
      if (errors.length) throw new Error(`记录非法：${errors.join('；')}`);
      writeJsonl(paths.tasks, rows);
      console.log(`[backlog] ${row.id} 已更新注释`);
      return 0;
    }
    case 'resolve': {
      const evidence = arg('evidence', rest.slice(1).filter(x => !x.startsWith('--')).join(' '));
      const { row, archiveFile, lease } = resolve(paths, rest[0], evidence);
      console.log(`[backlog] ${row.id} 已闭环并归档 → ${path.relative(root, archiveFile)}`);
      if (lease.released) console.log(`[backlog] 租约已释放（原持有者 @${lease.previous.owner}）`);
      return 0;
    }
    case 'lint': {
      const res = lintAll(paths);
      for (const n of res.notes) console.error(`[backlog-lint] NOTE ${n}`);
      if (!res.ok) {
        for (const p of res.problems) console.error(`[backlog-lint] ${p}`);
        console.error(`[backlog-lint] FAIL（${res.problems.length} 项）`);
        return 1;
      }
      console.log(`[backlog-lint] PASS（活跃 ${res.counts.active} / 归档 ${res.counts.archived}）`);
      return 0;
    }
    case 'doctor': {
      const report = doctor(paths);
      console.log(JSON.stringify({ root, ...report }, null, 2));
      return report.ok ? 0 : 1;
    }
    case 'import-triage': {
      const legacyFile = path.resolve(arg('from', '.agents/backlog/tasks.jsonl'));
      const legacyRows = readJsonl(legacyFile);
      const normalized = normalizeLegacy(legacyRows, { startId: Number(arg('start', '56')) });
      const doneRows = normalized.filter(r => r.status === 'done');
      const activeRows = normalized.filter(r => r.status !== 'done');
      const existingArchive = readArchives(paths);
      const appended = doneRows.filter(r => !existingArchive.some(a => a.id === r.id));
      if (appended.length) {
        const perYear = new Map();
        for (const row of appended) {
          const year = row.closed_at.slice(0, 4);
          perYear.set(year, [...(perYear.get(year) ?? []), row]);
        }
        for (const [year, rows] of perYear) {
          const file = path.join(paths.archiveDir, `tasks-${year}.jsonl`);
          writeJsonl(file, [...readJsonl(file), ...rows]);
        }
      }
      const merged = [...readJsonl(paths.tasks), ...activeRows];
      writeJsonl(paths.tasks, merged);
      for (const row of normalized) writeHighWater(paths, row.id);
      console.log(`[backlog] 迁移完成：活跃 +${activeRows.length}，归档 +${appended.length}（${normalized.map(r => r.id).join(',')}）`);
      return 0;
    }
    default:
      console.log('用法: backlog {record|list|start|heartbeat|release|resolve|annotate|lint|doctor|reap [--id <ID>] [--minutes N] [--dry-run]|import-triage}');
  return 1;
  }
}

/**
 * 是否作为 CLI 直接执行（`node scripts/backlog.mjs <cmd>`）而非被 import。
 *
 * 旧写法 `path.resolve(new URL(import.meta.url).pathname)` 在 Windows 上把
 * `/C:/repo/scripts/backlog.mjs` 这类**带前导斜杠的 URL pathname** 交给 `path.resolve`，
 * 得到一个与 `process.argv[1]`（`C:\repo\scripts\backlog.mjs`）永不相等的结果：
 * isDirect 恒 false ⇒ CLI 静默不执行（空 stdout、exit 0、账本不落盘），
 * 这正是 tests/backlog-cli.spec.ts 在 Windows CI 上两例恒红的根因。
 *
 * 现口径：两侧都先解析为真实路径再转 file URL 逐字比较；Windows 文件系统大小写
 * 不敏感，故仅在 win32 下做小写归一（POSIX 保留大小写敏感性）。
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
