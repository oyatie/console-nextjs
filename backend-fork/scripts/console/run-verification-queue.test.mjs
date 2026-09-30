#!/usr/bin/env node

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  buildVerificationCommands,
  executeVerificationQueue,
  parseWorktreePorcelain,
  selectCanonicalWorktree,
  validateVerificationPlan,
  verifyPlanAgainstAuthority,
  verifyPlanBytes,
  readBuildReport,
} from './run-verification-queue.mjs';

const SHA = 'a'.repeat(40);
const OTHER_SHA = 'b'.repeat(40);

function queueEntry(overrides = {}) {
  return {
    scheduled: true,
    verification_sha: SHA,
    cache_affinity: SHA,
    execution: 'canonical_shared_daemon_combined_targets',
    buck2_targets: ['//backend/crates/example:unit', '//tools/buck:app-example-postgres'],
    leaf_commands: [],
    ...overrides,
  };
}

test('admission rejects malformed or non-executable verification cohorts', () => {
  assert.deepEqual(validateVerificationPlan({ schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry()] }).map((entry) => entry.verification_sha), [SHA]);
  for (const invalid of [
    { schema_version: 'wrong', verification_queue: [queueEntry()] },
    { schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry({ cache_affinity: OTHER_SHA })] },
    { schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry({ buck2_targets: [] })] },
    { schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry({ buck2_targets: ['/absolute:target'] })] },
    { schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry({ buck2_targets: ['//../outside:target'] })] },
    { schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry({ buck2_targets: ['cell//a/../../outside:target'] })] },
    { schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry({ buck2_targets: ['root//tools/buck:app-example-postgres'] })] },
    { schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry({ leaf_commands: ['git diff --check'] })] },
    { schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry(), queueEntry()] },
  ]) assert.throws(() => validateVerificationPlan(invalid));
  assert.deepEqual(validateVerificationPlan({ schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry({ scheduled: false })] }), []);
});

test('clean matching worktree selection is canonical and rejects absent or dirty candidates', () => {
  const parsed = parseWorktreePorcelain([
    'worktree /z/repo', `HEAD ${SHA}`, 'branch refs/heads/z', '',
    'worktree /a/repo', `HEAD ${SHA}`, 'branch refs/heads/a', '',
    'worktree /dirty/repo', `HEAD ${SHA}`, 'branch refs/heads/dirty', '',
  ].join('\n'));
  const selected = selectCanonicalWorktree(parsed, SHA, (candidate) => candidate.path !== '/dirty/repo');
  assert.equal(selected.selected.path, '/a/repo');
  assert.deepEqual(selected.candidates.map((candidate) => candidate.path), ['/a/repo', '/z/repo']);
  assert.throws(() => selectCanonicalWorktree(parsed, OTHER_SHA, () => true), /no clean exact-HEAD/);
});

test('singleton worktree listings still use canonical selection rather than bypassing cleanliness checks', async () => {
  await assert.rejects(() => executeVerificationQueue({ schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry()] }, {
    listWorktrees: () => [{ path: '/only', head: SHA }], inspectWorktree: () => ({ clean: false, head: SHA }),
  }), /no clean exact-HEAD/);
});

test('receipt staging creates a missing local parent before worktree admission', async () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'queue-missing-parent-'));
  const receiptRoot = path.join(root, 'missing', 'nested', 'reports');
  try {
    await assert.rejects(() => executeVerificationQueue({ schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry()] }, {
      receiptRoot,
      listWorktrees: () => [{ path: '/only', head: SHA }],
      inspectWorktree: () => ({ clean: false, head: SHA }),
    }), /no clean exact-HEAD/);
    assert.equal(existsSync(path.dirname(receiptRoot)), true);
    assert.equal(existsSync(receiptRoot), false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('active children receive SIGTERM and are awaited during interruption cleanup', async () => {
  const { terminateActiveChildren } = await import('./run-verification-queue.mjs');
  let killed = 0; let waited = 0;
  const child = { exitCode: null, kill: (signal) => { assert.equal(signal, 'SIGTERM'); killed += 1; }, once: (event, callback) => { assert.equal(event, 'close'); waited += 1; callback(); } };
  await terminateActiveChildren(new Set([child]));
  assert.equal(killed, 1); assert.equal(waited, 1);
});

test('a supplied JSON plan is held unless it exactly matches explicit immutable authority recomputation', () => {
  const authority = { candidate: SHA, authorityTip: OTHER_SHA, syntheticMerge: 'c'.repeat(40), admission: 'd'.repeat(40) };
  const plan = { schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry()] };
  assert.equal(verifyPlanAgainstAuthority(plan, authority, () => structuredClone(plan)).schema_version, plan.schema_version);
  assert.throws(() => verifyPlanAgainstAuthority(plan, authority, () => ({ ...plan, verification_queue: [] })), /differs/);
  assert.throws(() => verifyPlanAgainstAuthority(plan, { ...authority, candidate: 'bad' }, () => plan), /immutable SHA/);
  const canonical = `${JSON.stringify(plan, null, 2)}\n`;
  assert.equal(verifyPlanBytes(canonical, plan).schema_version, plan.schema_version);
  assert.throws(() => verifyPlanBytes(`${JSON.stringify({ verification_queue: plan.verification_queue, schema_version: plan.schema_version })}\n`, plan), /noncanonical/);
});

test('a signal sentinel prevents publication even when a runner reports success', async () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'queue-signal-'));
  try {
    await assert.rejects(() => executeVerificationQueue({ schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry()] }, {
      receiptRoot: path.join(root, 'reports'), listWorktrees: () => [{ path: '/repo', head: SHA }], inspectWorktree: () => ({ clean: true, head: SHA }), toolDigest: () => 'c'.repeat(64), platformFacts: () => ({}), monotonicNow: () => 1,
      queryMetadata: () => ({ 'root//backend/crates/example:unit': { labels: [] }, 'root//tools/buck:app-example-postgres': { labels: ['needs-postgres'] } }),
      run: (command) => { process.emit('SIGINT'); writeFileSync(command.reportPath, 'ok\n'); return { status: 0, signal: null }; },
    }), /interrupted/);
    assert.equal(existsSync(path.join(root, 'reports')), false);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('atomic report reads reject a symlink replacement before consuming its bytes', () => {
  let closed = false;
  assert.throws(() => readBuildReport('/receipts/report.json', '/receipts', {
    open: () => { const error = new Error('symlink'); error.code = 'ELOOP'; throw error; }, stat: () => { throw new Error('unreachable'); }, read: () => { throw new Error('unreachable'); }, close: () => { closed = true; },
  }), /symbolic link/);
  assert.equal(closed, false);
  const digest = readBuildReport('/receipts/report.json', '/receipts', {
    open: () => 7, stat: () => ({ isFile: () => true, size: 2 }), read: (_fd, buffer, offset) => { buffer.write('ok', offset); return 2; }, close: () => { closed = true; },
  });
  assert.equal(digest, createHash('sha256').update('ok').digest('hex')); assert.equal(closed, true);
});

test('a post-command promotion signal and a post-staging failure leave no published receipt root', async () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'queue-promotion-'));
  const common = { listWorktrees: () => [{ path: '/repo', head: SHA }], inspectWorktree: () => ({ clean: true, head: SHA }), toolDigest: () => 'c'.repeat(64), platformFacts: () => ({}), monotonicNow: () => 1, queryMetadata: () => ({ 'root//backend/crates/example:unit': { labels: [] }, 'root//tools/buck:app-example-postgres': { labels: ['needs-postgres'] } }), run: (command) => { writeFileSync(command.reportPath, 'ok\n'); return { status: 0, signal: null }; } };
  try {
    const signalRoot = path.join(root, 'signal');
    await assert.rejects(() => executeVerificationQueue({ schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry()] }, { ...common, receiptRoot: signalRoot, beforePromotion: () => process.emit('SIGTERM') }), /interrupted/);
    assert.equal(existsSync(signalRoot), false);
    const failRoot = path.join(root, 'failure');
    await assert.rejects(() => executeVerificationQueue({ schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry()] }, { ...common, receiptRoot: failRoot, resourceSnapshot: () => ({ fd: -1, rss_kb: 1 }) }), /telemetry/);
    assert.equal(existsSync(failRoot), false);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('promotion refuses and preserves a receipt root created by a concurrent publisher', async () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'queue-promotion-race-'));
  const receiptRoot = path.join(root, 'reports');
  let foreignInode;
  try {
    await assert.rejects(() => executeVerificationQueue({ schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry()] }, {
      receiptRoot,
      listWorktrees: () => [{ path: '/repo', head: SHA }],
      inspectWorktree: () => ({ clean: true, head: SHA }),
      toolDigest: () => 'c'.repeat(64),
      platformFacts: () => ({}),
      monotonicNow: () => 1,
      queryMetadata: () => ({ 'root//backend/crates/example:unit': { labels: [] }, 'root//tools/buck:app-example-postgres': { labels: ['needs-postgres'] } }),
      run: (command) => { writeFileSync(command.reportPath, 'ok\n'); return { status: 0, signal: null }; },
      beforePromotion: () => {
        mkdirSync(receiptRoot);
        foreignInode = statSync(receiptRoot).ino;
      },
    }), /receipt root appeared before promotion/);
    assert.equal(statSync(receiptRoot).ino, foreignInode);
    assert.deepEqual(readdirSync(receiptRoot), []);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('resource-aware scheduling serializes conflicting PostgreSQL cohorts instead of rejecting their total', async () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'queue-resource-'));
  try {
    const resources = { writer: 0, postgres: 1, browser: 0, ios: 0, graph: 0, cas: 0 };
    const other = queueEntry({ verification_sha: OTHER_SHA, cache_affinity: OTHER_SHA, resources });
    const worktrees = [{ path: '/repo-a', head: SHA }, { path: '/repo-b', head: OTHER_SHA }];
    let active = 0; let peak = 0;
    const receipts = await executeVerificationQueue({ schema_version: 'console-fanout-epoch-v2', policy: { cold_rust_compile_lanes: 2, resource_budgets: { writer: 2, postgres: 1, browser: 2, ios: 2, graph: 2, cas: 2 } }, verification_queue: [queueEntry({ resources }), other] }, {
      receiptRoot: path.join(root, 'reports'), maxCohorts: 2, listWorktrees: () => worktrees, inspectWorktree: (candidate) => ({ clean: true, head: worktrees.find((item) => item.path === candidate).head }), toolDigest: () => 'c'.repeat(64), platformFacts: () => ({}), monotonicNow: () => 1,
      queryMetadata: () => ({ 'root//backend/crates/example:unit': { labels: [] }, 'root//tools/buck:app-example-postgres': { labels: ['needs-postgres'] } }),
      run: async (command) => { active += 1; peak = Math.max(peak, active); await new Promise((resolve) => setTimeout(resolve, 5)); writeFileSync(command.reportPath, 'ok\n'); active -= 1; return { status: 0, signal: null }; },
    });
    assert.equal(receipts.length, 2); assert.equal(peak, 1);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('command construction uses exact metadata rather than a path heuristic and never serializes Buck compilation', async () => {
  const { partitionTargetsByMetadata } = await import('./run-verification-queue.mjs');
  const partition = partitionTargetsByMetadata(queueEntry().buck2_targets, {
    'root//backend/crates/example:unit': { labels: ['test.unit'] },
    'root//tools/buck:app-example-postgres': { labels: ['needs-postgres', 'resource.postgres'] },
  });
  const commands = buildVerificationCommands(queueEntry(), '/repo', '/reports', partition);
  assert.equal(commands.length, 2);
  assert.deepEqual(commands[0].argv.slice(0, 5), ['/repo/tools/buck2', '--build-report', commands[0].reportPath, 'test', '-j']);
  assert.equal(commands[0].argv[5], '6');
  assert.deepEqual(commands[0].targets, ['//backend/crates/example:unit']);
  assert.deepEqual(commands[1].argv, ['/repo/tools/buck/test_needs_postgres.sh', '--build-report', commands[1].reportPath, '//tools/buck:app-example-postgres']);
  assert.ok(commands.flatMap((command) => command.argv).every((arg) => arg !== '--num-threads=1' && arg !== '--isolation-dir'));
  assert.throws(() => partitionTargetsByMetadata(['//backend/crates/example:integration'], { 'root//backend/crates/example:integration': { labels: ['needs-postgres'] } }), /credential-safe/);
});

test('execution writes a digest-bound receipt only after every combined call succeeds', async () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'queue-receipt-'));
  try {
    const reportRoot = path.join(root, 'reports');
    const plan = { schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry()] };
    const selected = { path: '/canonical/repo', head: SHA };
    const calls = [];
    const result = await executeVerificationQueue(plan, {
      receiptRoot: reportRoot,
      listWorktrees: () => [selected],
      inspectWorktree: () => ({ clean: true, head: SHA }),
      toolDigest: () => 'c'.repeat(64),
      platformFacts: () => ({ os: 'test', arch: 'test' }),
      queryMetadata: () => ({ 'root//backend/crates/example:unit': { labels: [] }, 'root//tools/buck:app-example-postgres': { labels: ['needs-postgres'] } }),
      run: (command) => {
        calls.push(command);
        writeFileSync(command.reportPath, `report-${calls.length}\n`);
        return { status: 0, signal: null };
      },
      monotonicNow: (() => { let tick = 0; return () => ++tick; })(),
    });
    assert.equal(calls.length, 2);
    assert.equal(result.length, 1);
    const receipt = JSON.parse(readFileSync(result[0].receipt_path, 'utf8'));
    assert.equal(receipt.status, 'passed');
    assert.equal(receipt.verification_sha, SHA);
    assert.equal(receipt.calls.length, 2);
    assert.equal(receipt.calls[0].build_report_sha256, createHash('sha256').update('report-1\n').digest('hex'));
    assert.equal(receipt.isolation_absent, true);
    const queueReceipt = JSON.parse(readFileSync(path.join(reportRoot, 'queue-receipt.json'), 'utf8'));
    assert.equal(queueReceipt.status, 'passed');
    assert.equal(queueReceipt.orchestrator_peak_cohorts, 1);
    assert.ok(Number.isInteger(queueReceipt.orchestrator_peak_fd));
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('failed calls leave no successful or partial receipt behind', async () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'queue-failure-'));
  try {
    const reportRoot = path.join(root, 'reports');
    await assert.rejects(() => executeVerificationQueue({ schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry()] }, {
      receiptRoot: reportRoot,
      listWorktrees: () => [{ path: '/canonical/repo', head: SHA }],
      inspectWorktree: () => ({ clean: true, head: SHA }),
      toolDigest: () => 'c'.repeat(64),
      platformFacts: () => ({ os: 'test', arch: 'test' }),
      queryMetadata: () => ({ 'root//backend/crates/example:unit': { labels: [] }, 'root//tools/buck:app-example-postgres': { labels: ['needs-postgres'] } }),
      run: (command) => { writeFileSync(command.reportPath, 'failed\n'); return { status: 1, signal: null }; },
      monotonicNow: () => 1,
    }));
    assert.equal(requireNoReceipts(reportRoot), true);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('wrapper-affecting environment is a fail-closed admission boundary', async () => {
  const saved = Object.fromEntries(['CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK', 'CONSOLE_BUCK_NEEDS_POSTGRES_TEST_EXACT'].map((key) => [key, process.env[key]]));
  try {
    for (const key of Object.keys(saved)) {
      process.env[key] = 'forged';
      await assert.rejects(() => executeVerificationQueue({ schema_version: 'console-fanout-epoch-v2', verification_queue: [queueEntry()] }, { listWorktrees: () => [{ path: '/repo', head: SHA }], inspectWorktree: () => ({ clean: true, head: SHA }) }), /wrapper-affecting environment/);
      delete process.env[key];
    }
  } finally {
    for (const [key, value] of Object.entries(saved)) { if (value === undefined) delete process.env[key]; else process.env[key] = value; }
  }
});

test('a later cohort failure publishes no earlier cohort receipt', async () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'queue-global-atomic-'));
  try {
    const other = queueEntry({ verification_sha: OTHER_SHA, cache_affinity: OTHER_SHA });
    const worktrees = [{ path: '/repo-a', head: SHA }, { path: '/repo-b', head: OTHER_SHA }];
    let call = 0;
    await assert.rejects(() => executeVerificationQueue({ schema_version: 'console-fanout-epoch-v2', policy: { cold_rust_compile_lanes: 1 }, verification_queue: [queueEntry(), other] }, {
      receiptRoot: path.join(root, 'reports'), listWorktrees: () => worktrees,
      inspectWorktree: (candidate) => ({ clean: true, head: worktrees.find((worktree) => worktree.path === candidate).head }), toolDigest: () => 'c'.repeat(64), platformFacts: () => ({}), monotonicNow: () => 1,
      queryMetadata: () => ({ 'root//backend/crates/example:unit': { labels: [] }, 'root//tools/buck:app-example-postgres': { labels: ['needs-postgres'] } }),
      run: (command) => { call += 1; writeFileSync(command.reportPath, 'report\n'); return { status: call === 3 ? 1 : 0, signal: null }; },
    }));
    assert.equal(existsSync(path.join(root, 'reports')), false);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('scheduled exact-SHA cohorts run only up to declared cold-Rust capacity and expose peak telemetry', async () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'queue-capacity-'));
  try {
    const other = queueEntry({ verification_sha: OTHER_SHA, cache_affinity: OTHER_SHA });
    const worktrees = [{ path: '/repo-a', head: SHA }, { path: '/repo-b', head: OTHER_SHA }];
    let active = 0; let peak = 0;
    const receipts = await executeVerificationQueue({ schema_version: 'console-fanout-epoch-v2', policy: { cold_rust_compile_lanes: 2 }, verification_queue: [queueEntry(), other] }, {
      receiptRoot: path.join(root, 'reports'), maxCohorts: 2,
      listWorktrees: () => worktrees,
      inspectWorktree: (candidate) => ({ clean: true, head: worktrees.find((worktree) => worktree.path === candidate).head }),
      toolDigest: () => 'c'.repeat(64), platformFacts: () => ({ os: 'test' }), monotonicNow: () => 1,
      queryMetadata: () => ({ 'root//backend/crates/example:unit': { labels: [] }, 'root//tools/buck:app-example-postgres': { labels: ['needs-postgres'] } }),
      run: async (command) => { active += 1; peak = Math.max(peak, active); await new Promise((resolve) => setTimeout(resolve, 5)); writeFileSync(command.reportPath, 'ok\n'); active -= 1; return { status: 0, signal: null }; },
    });
    assert.equal(receipts.length, 2);
    assert.equal(receipts.max_cohorts, 2);
    assert.equal(receipts.peak_cohorts, 2);
    assert.ok(peak <= 2);
    await assert.rejects(() => executeVerificationQueue({ schema_version: 'console-fanout-epoch-v2', policy: { cold_rust_compile_lanes: 1 }, verification_queue: [queueEntry()] }, { receiptRoot: path.join(root, 'reject'), maxCohorts: 2 }), /capacity/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

function requireNoReceipts(root) {
  try { return !readFileSync(path.join(root, SHA, 'receipt.json'), 'utf8'); } catch (error) { return error.code === 'ENOENT'; }
}
