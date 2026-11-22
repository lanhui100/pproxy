#!/usr/bin/env node
/**
 * backlog v2 → v3 一次性迁移：单文件 JSONL 账本 → 批次日志（fold-on-read）。
 *
 * 两种模式：
 *   （默认）整库迁移：`backlog/tasks.jsonl` 存在、`backlog/MODEL` 不存在。
 *   `--adopt-v2` 接管：目标已是 v3，但工作树里又出现了一份 v2 `tasks.jsonl`
 *     （典型场景：在途分支基于 v2 记账，合并/变基到已迁移的 main 后残留该文件）。
 *     只把 **fold 里还不存在** 的任务行接管进一个新批次；已存在的 ID 跳过并告警，
 *     然后删除 `tasks.jsonl`/`.next-id`，不碰任何已有批次。
 *
 * 默认模式做的事（顺序即安全边界）：
 *   1) 校验前提：`backlog/tasks.jsonl` 存在、`backlog/MODEL` 不存在、`backlog/log/` 为空；
 *   2) 用一个基线批次承接现有活跃任务（每条一条 `record` 事件，保持原 ID / 字段）；
 *      活跃池里的 done 记录补一条 `resolve` 事件；
 *   3) 临时写 `backlog/MODEL`，用 v3 `fold` 对账（活跃数、done 数必须与迁移前一致），
 *      不一致 → 回滚（删 MODEL 与基线批次）并退出；
 *   4) 删除 `backlog/tasks.jsonl` 与 `backlog/.next-id`（v2 遗留归档 tasks-<YYYY>.jsonl 保留只读，
 *      fold 时并入历史）。
 *
 * 幂等：MODEL 已存在 → 默认模式 exit 2（提示改用 --adopt-v2 或确认无需迁移）。
 * `--dry-run` 只打印计划，不动磁盘。
 *
 * 用法：node scripts/backlog-migrate-v2-v3.mjs [--dry-run] [--surface <s>] [--release <v>] [--adopt-v2]
 * 根解析同 backlog.mjs（PONY_BACKLOG_ROOT 可覆盖，便于夹具演练）。
 */
import fs from 'node:fs';
import path from 'node:path';
import {
  BATCH_MODEL,
  MODEL_FILE,
  fold,
  ledgerPaths,
  newBatchId,
  readJsonl,
  readModel,
  resolveLedgerRoot,
  sanitizeSurface,
  validateTask
} from './backlog.mjs';

function fail(message) {
  console.error(`[backlog-migrate] FAIL ${message}`);
  process.exit(2);
}

function main(argv = process.argv.slice(2)) {
  const arg = (name, fallback) => {
    const i = argv.indexOf(`--${name}`);
    return i === -1 ? fallback : argv[i + 1];
  };
  const dryRun = argv.includes('--dry-run');
  const adopt = argv.includes('--adopt-v2');
  const surface = sanitizeSurface(arg('surface', 'legacy-migration'));
  const release = arg('release');
  const root = resolveLedgerRoot();
  const paths = ledgerPaths(root);

  const model = readModel(paths);
  if (adopt) {
    if (model !== 'v3') fail(`--adopt-v2 需要目标已是 v3（当前 ${model ?? '未启用'}）`);
    if (!fs.existsSync(paths.legacyTasks)) fail('--adopt-v2 未发现 backlog/tasks.jsonl（无行可接管）');
  } else {
    if (model === 'v3') {
      fail(`已是 v3（${MODEL_FILE} 存在），拒绝重复迁移；若工作树里残留 tasks.jsonl，请改用 --adopt-v2 接管`);
    }
    if (model !== 'v2') fail('未发现 backlog/tasks.jsonl（v2 账本不存在，无可迁移）');
    if (fs.existsSync(paths.logsDir) && fs.readdirSync(paths.logsDir).filter(n => n.endsWith('.jsonl')).length > 0) {
      fail('backlog/log/ 已有批次日志，拒绝把 v2 账本混入 v3（请先人工确认）');
    }
  }

  const rows = readJsonl(paths.legacyTasks);
  for (const row of rows) {
    const errors = validateTask(row);
    if (errors.length) fail(`v2 记录 ${row?.id ?? '?'} 非法：${errors.join('；')}（先修 v2 账本再迁移）`);
  }

  const before = adopt ? fold(paths) : null;
  const imported = adopt ? rows.filter(row => !before.rows.has(row.id)) : rows;
  const skipped = adopt ? rows.filter(row => before.rows.has(row.id)).map(row => row.id) : [];
  const active = imported.filter(row => row.status !== 'done');
  const doneInActive = imported.filter(row => row.status === 'done');
  const archivedCount = adopt
    ? 0
    : (fs.existsSync(paths.archiveDir) ? fs.readdirSync(paths.archiveDir).filter(n => /^tasks-\d{4}\.jsonl$/.test(n)) : [])
      .reduce((n, name) => n + readJsonl(path.join(paths.archiveDir, name)).length, 0);

  const batchId = newBatchId({ surface });
  const batchPath = path.join(paths.logsDir, `${batchId}.jsonl`);
  const now = new Date().toISOString();
  if (adopt && skipped.length) {
    console.log(`[backlog-migrate] 接管模式：跳过 v3 已存在的 ID ${skipped.length} 条（不覆盖既有状态）：${skipped.join(', ')}`);
  }
  console.log(`[backlog-migrate] 计划：v2 待迁移 ${imported.length} 条（活跃 ${active.length}、done ${doneInActive.length}）` +
    (adopt ? '' : `，遗留归档 ${archivedCount} 条`));
  console.log(`[backlog-migrate] 批次 → ${path.relative(root, batchPath)}（surface=${surface}${release ? `，release=${release}` : ''}）`);
  if (dryRun) {
    console.log('[backlog-migrate] [dry-run] 未改动磁盘（去掉 --dry-run 执行）');
    return 0;
  }
  if (!imported.length) {
    fs.rmSync(paths.legacyTasks, { force: true });
    fs.rmSync(paths.legacyNextId, { force: true });
    console.log('[backlog-migrate] 没有需要接管的新行；已删除残留 tasks.jsonl/.next-id');
    return 0;
  }

  const lines = [{
    kind: 'batch.start',
    model: BATCH_MODEL,
    batch_id: batchId,
    surface,
    ...(release ? { release } : {}),
    created_at: now
  }];
  // v2 账本按 ID 顺序累积（legacy 看板导入尤其如此），created_at 不保证单调；
  // v3 批次事件要求 `at` 非递减（lint 判「事件时间倒退」）。故按事件时间**稳定排序**重放：
  // 只改事件排列顺序，不改任何 task 字段（created_at/closed_at 原样保留）。
  const events = [];
  for (const row of imported) {
    const at = row.created_at ?? now;
    events.push({ kind: 'record', at, task: row });
    if (row.status === 'done') {
      const closedAt = row.closed_at ?? now;
      events.push({
        kind: 'resolve',
        // closed_at 早于 created_at 本身非法；夹到 record 时间，保证批次事件不倒退。
        at: Date.parse(closedAt) < Date.parse(at) ? at : closedAt,
        id: row.id,
        evidence: row.evidence ?? 'v2 活跃池 done 记录迁移'
      });
    }
  }
  lines.push(...events
    .map((event, index) => ({ event, index }))
    .sort((a, b) => (Date.parse(a.event.at) - Date.parse(b.event.at)) || (a.index - b.index))
    .map(({ event }) => event));
  fs.mkdirSync(paths.logsDir, { recursive: true });
  fs.writeFileSync(batchPath, lines.map(line => JSON.stringify(line)).join('\n') + '\n');

  // 默认模式：临时写 MODEL 以 v3 视角对账；失败即回滚。接管模式：MODEL 已在，直接对账。
  const wroteModel = !adopt;
  if (wroteModel) {
    fs.mkdirSync(path.dirname(paths.model), { recursive: true });
    fs.writeFileSync(paths.model, 'v3\n');
  }
  const state = fold(paths);
  const expectedActive = (before?.counts.active ?? 0) + active.length;
  const expectedDone = (before?.counts.done ?? 0) + archivedCount + doneInActive.length;
  const ok = state.problems.length === 0 &&
    state.counts.active === expectedActive &&
    state.counts.done === expectedDone;
  if (!ok) {
    if (wroteModel) fs.rmSync(paths.model, { force: true });
    fs.rmSync(batchPath, { force: true });
    const detail = state.problems.length
      ? state.problems.join('；')
      : `活跃 ${state.counts.active}/${expectedActive}，done ${state.counts.done}/${expectedDone}`;
    fail(`迁移对账不一致（已回滚）：${detail}`);
  }

  fs.rmSync(paths.legacyTasks, { force: true });
  fs.rmSync(paths.legacyNextId, { force: true });

  console.log(`[backlog-migrate] PASS：活跃 ${state.counts.active}、done ${state.counts.done}（含遗留归档 ${state.counts.legacyArchived}）`);
  console.log(`[backlog-migrate] 已删除 backlog/tasks.jsonl 与 backlog/.next-id；请把 ${wroteModel ? `${MODEL_FILE}、` : ''}${path.relative(root, batchPath)} 与删除一起提交`);
  console.log('[backlog-migrate] 下一步：node scripts/backlog.mjs lint && node scripts/backlog.mjs doctor && node scripts/backlog.mjs verify-archive');
  return 0;
}

process.exit(main());
