#!/usr/bin/env node

import { createHash } from 'node:crypto';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { closeSync, constants as fsConstants, existsSync, fstatSync, lstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, readSync, readdirSync, realpathSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const FULL_SHA = /^[0-9a-f]{40}$/;
const BUCK_LABEL = /^(?:[A-Za-z0-9_.-]+)?\/\/[A-Za-z0-9_./-]+:[A-Za-z0-9_.-]+$/;
const POSTGRES_WRAPPER = /^\/\/tools\/buck:[A-Za-z0-9_.-]+$/;
const EXECUTION = 'canonical_shared_daemon_combined_targets';
const SCHEMA = 'console-fanout-epoch-v2';
const FORBIDDEN_WRAPPER_ENV = ['BUCK_ISOLATION_DIR', 'CONSOLE_BUCK_NEEDS_POSTGRES_ISOLATION_DIR', 'CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK', 'CONSOLE_BUCK_NEEDS_POSTGRES_TEST_EXACT'];
const RESOURCE_KEYS = ['writer', 'postgres', 'browser', 'ios', 'graph', 'cas'];

function stable(value) {
  if (Array.isArray(value)) return value.map(stable);
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).sort(([a], [b]) => a.localeCompare(b, 'en')).map(([key, child]) => [key, stable(child)]));
  return value;
}
function hashBytes(bytes) { return createHash('sha256').update(bytes).digest('hex'); }
function hashJson(value) { return hashBytes(JSON.stringify(stable(value))); }
function fail(message) { throw new Error(`verification queue HOLD: ${message}`); }
function requireDirectory(root, candidate) {
  const resolvedRoot = path.resolve(root);
  const resolvedCandidate = path.resolve(candidate);
  if (resolvedCandidate !== resolvedRoot && !resolvedCandidate.startsWith(`${resolvedRoot}${path.sep}`)) fail('receipt/report path escapes configured local receipt root');
  return resolvedCandidate;
}
function canonicalizeReceiptRoot(candidate) {
  const resolved = path.resolve(candidate);
  if (existsSync(resolved)) {
    if (lstatSync(resolved).isSymbolicLink()) fail('receipt root may not be a symbolic link');
    return resolved;
  }
  const parent = path.dirname(resolved);
  if (!existsSync(parent)) return canonicalizeReceiptRoot(parent) + path.sep + path.basename(resolved);
  // Canonicalize system aliases such as /tmp before appending an uncreated
  // operator leaf; no attacker-controlled existing leaf is ever followed.
  return path.join(realpathSync(parent), path.basename(resolved));
}
function prepareReceiptParent(receiptRoot) {
  const parent = path.dirname(receiptRoot);
  mkdirSync(parent, { recursive: true, mode: 0o700 });
  const parentStat = lstatSync(parent);
  if (!parentStat.isDirectory() || parentStat.isSymbolicLink()) fail('receipt staging parent is not a real directory');
  if (canonicalizeReceiptRoot(receiptRoot) !== receiptRoot) fail('receipt staging parent changed while being prepared');
  return parent;
}
function isPlainObject(value) { return Boolean(value) && typeof value === 'object' && !Array.isArray(value); }
function isCanonicalBuckLabel(target) {
  if (typeof target !== 'string' || target.startsWith('root//') || !BUCK_LABEL.test(target)) return false;
  const [prefix, suffix] = target.split('//'); const packagePath = suffix.slice(0, suffix.indexOf(':'));
  return prefix !== '.' && prefix !== '..' && packagePath.split('/').every((segment) => segment !== '' && segment !== '.' && segment !== '..');
}

/** Validate only the already-admitted scheduler contract; this does not re-plan work. */
export function validateVerificationPlan(plan) {
  if (!isPlainObject(plan) || plan.schema_version !== SCHEMA || !Array.isArray(plan.verification_queue)) fail('unsupported or malformed fanout plan');
  // Held cohorts remain planner evidence, not executable input. They must not
  // turn an otherwise safe scheduled cohort into a run, or into a silent pass.
  const scheduled = plan.verification_queue.filter((entry) => entry?.scheduled === true);
  const seen = new Set();
  for (const entry of scheduled) {
    if (!isPlainObject(entry) || entry.execution !== EXECUTION) fail('scheduled entry lacks canonical shared-daemon execution');
    if (!FULL_SHA.test(entry.verification_sha ?? '') || entry.cache_affinity !== entry.verification_sha) fail('scheduled entry lacks matching full verification SHA/cache affinity');
    if (seen.has(entry.verification_sha)) fail('duplicate exact-SHA verification cohort');
    seen.add(entry.verification_sha);
    if (!Array.isArray(entry.buck2_targets) || entry.buck2_targets.length === 0) fail('scheduled entry has no Buck targets');
    const targetSet = new Set();
    for (const target of entry.buck2_targets) {
      if (!isCanonicalBuckLabel(target) || targetSet.has(target)) fail('scheduled entry has malformed, root-qualified, traversal, or duplicate Buck target');
      targetSet.add(target);
    }
    if (!Array.isArray(entry.leaf_commands) || entry.leaf_commands.length !== 0) fail('scheduled entry has executable leaf commands outside this Buck-only executor');
  }
  return scheduled.map((entry) => ({ ...entry, buck2_targets: [...entry.buck2_targets].sort((a, b) => a.localeCompare(b, 'en')) }));
}

export function parseWorktreePorcelain(output) {
  const records = [];
  let current = null;
  for (const line of String(output).split(/\r?\n/)) {
    if (line === '') { if (current?.path) records.push(current); current = null; continue; }
    const separator = line.indexOf(' ');
    const key = separator < 0 ? line : line.slice(0, separator);
    const value = separator < 0 ? '' : line.slice(separator + 1);
    if (key === 'worktree') { if (current?.path) records.push(current); current = { path: value }; }
    else if (current && key === 'HEAD') current.head = value;
    else if (current && key === 'branch') current.branch = value;
  }
  if (current?.path) records.push(current);
  return records;
}

/** Returns only clean worktrees already at the immutable SHA, lexically ordered. */
export function selectCanonicalWorktree(worktrees, sha, isClean) {
  const candidates = worktrees.filter((entry) => entry?.head === sha && isClean(entry)).sort((left, right) => left.path.localeCompare(right.path, 'en'));
  if (!candidates.length) fail('no clean exact-HEAD worktree exists for verification SHA');
  return { selected: candidates[0], candidates };
}

export function buildVerificationCommands(entry, worktree, reportDirectory, partition) {
  const normalTargets = partition.normal;
  const postgresTargets = partition.postgres;
  const reports = [];
  if (normalTargets.length) {
    const reportPath = requireDirectory(reportDirectory, path.join(reportDirectory, 'normal.build-report.json'));
    reports.push({ kind: 'normal', targets: normalTargets, reportPath, argv: [path.join(worktree, 'tools/buck2'), '--build-report', reportPath, 'test', '-j', '6', ...normalTargets] });
  }
  if (postgresTargets.length) {
    const reportPath = requireDirectory(reportDirectory, path.join(reportDirectory, 'postgres.build-report.json'));
    // The wrapper already exports RUST_TEST_THREADS=1.  Deliberately do not add
    // Buck --num-threads=1: that would serialize analysis/compilation as well.
    reports.push({ kind: 'postgres', targets: postgresTargets, reportPath, argv: [path.join(worktree, 'tools/buck/test_needs_postgres.sh'), '--build-report', reportPath, ...postgresTargets] });
  }
  return reports;
}

function defaultInspectWorktree(worktree) {
  const head = execFileSync('git', ['-C', worktree, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  const dirty = execFileSync('git', ['-C', worktree, 'status', '--porcelain=v1', '--untracked-files=all'], { encoding: 'utf8' });
  return { head, clean: dirty === '' };
}
function defaultListWorktrees(repo) {
  const porcelain = execFileSync('git', ['-C', repo, 'worktree', 'list', '--porcelain'], { encoding: 'utf8' });
  return parseWorktreePorcelain(porcelain);
}
function defaultRun(command, worktree, activeChildren) {
  const [file, ...args] = command.argv;
  const environment = { ...process.env };
  for (const key of FORBIDDEN_WRAPPER_ENV) delete environment[key];
  return new Promise((resolve, reject) => {
    const child = spawn(file, args, { cwd: worktree, stdio: 'inherit', env: environment });
    activeChildren?.add(child);
    child.once('error', reject);
    child.once('close', (status, signal) => { activeChildren?.delete(child); resolve({ status, signal }); });
  });
}
function defaultToolDigest(worktree) { return hashBytes(readFileSync(path.join(worktree, 'tools/buck2'))); }
function defaultPlatformFacts() { return { platform: process.platform, arch: process.arch, release: os.release() }; }
function defaultMonotonicNow() { return Number(process.hrtime.bigint()); }
function defaultResourceSnapshot() { return { fd: readdirSync('/dev/fd').length, rss_kb: process.resourceUsage().maxRSS }; }
function defaultQueryMetadata(targets, worktree) {
  const query = `set(${targets.join(' ')})`;
  const output = execFileSync(path.join(worktree, 'tools/buck2'), ['uquery', '--json', '--output-attribute', '^labels$', query], { cwd: worktree, encoding: 'utf8' });
  return JSON.parse(output);
}
function normalizedQueryLabel(label) { return label.startsWith('root//') ? label.slice(4) : label; }
export function partitionTargetsByMetadata(targets, metadata) {
  if (!isPlainObject(metadata)) fail('Buck target metadata is unavailable or malformed');
  const expected = new Set(targets);
  const observed = new Map();
  for (const [rawLabel, attributes] of Object.entries(metadata)) {
    const label = normalizedQueryLabel(rawLabel);
    if (!expected.has(label) || observed.has(label) || !Array.isArray(attributes?.labels) || attributes.labels.some((item) => typeof item !== 'string')) fail('Buck target metadata does not exactly describe admitted targets');
    observed.set(label, attributes.labels);
  }
  if (observed.size !== expected.size) fail('Buck target metadata omitted an admitted target');
  const normal = [], postgres = [];
  for (const target of targets) {
    const labels = observed.get(target);
    if (labels.includes('needs-postgres')) {
      if (!POSTGRES_WRAPPER.test(target)) fail('needs-postgres target is not a credential-safe PostgreSQL wrapper');
      postgres.push(target);
    } else normal.push(target);
  }
  return { normal, postgres };
}

/** The supplied JSON is a cacheable transport artifact, never executable authority. */
export function verifyPlanAgainstAuthority(suppliedPlan, authority, runPlanner) {
  for (const [name, sha] of Object.entries(authority)) if (!FULL_SHA.test(sha ?? '')) fail(`${name} must be a full immutable SHA`);
  const recomputed = runPlanner(authority);
  if (!isPlainObject(recomputed) || hashJson(recomputed) !== hashJson(suppliedPlan)) fail('supplied fanout plan differs from the exact recomputed authority plan');
  return recomputed;
}
export function verifyPlanBytes(suppliedBytes, recomputedPlan) {
  const canonical = `${JSON.stringify(recomputedPlan, null, 2)}\n`;
  if (typeof suppliedBytes !== 'string' || suppliedBytes !== canonical) fail('supplied fanout plan bytes are noncanonical or differ from recomputation');
  return recomputedPlan;
}

function resolveWorktree(sha, options) {
  const listed = options.listWorktrees();
  if (!Array.isArray(listed)) fail('worktree discovery did not return a full porcelain array');
  const inspect = options.inspectWorktree;
  return selectCanonicalWorktree(listed, sha, (candidate) => {
    try { const state = inspect(candidate.path); return state.clean === true && state.head === sha; } catch { return false; }
  });
}
export async function terminateActiveChildren(children) {
  const waiting = [];
  for (const child of children) {
    if (child.exitCode == null && child.signalCode == null) {
      child.kill('SIGTERM');
      waiting.push(new Promise((resolve) => child.once('close', resolve)));
    }
  }
  await Promise.allSettled(waiting);
}
function recheckWorktree(worktree, sha, inspect) {
  const state = inspect(worktree);
  if (state.head !== sha || state.clean !== true) fail('selected worktree changed, is dirty, or no longer matches verification SHA');
}
export function readBuildReport(reportPath, receiptRoot, io = {}) {
  requireDirectory(receiptRoot, reportPath);
  const open = io.open ?? openSync; const stat = io.stat ?? fstatSync; const read = io.read ?? readSync; const close = io.close ?? closeSync;
  let descriptor;
  try {
    descriptor = open(reportPath, fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW);
    const metadata = stat(descriptor);
    if (!metadata.isFile() || metadata.size < 1) fail('Buck call produced no safe build report');
    const bytes = Buffer.alloc(metadata.size); let offset = 0;
    while (offset < bytes.length) { const count = read(descriptor, bytes, offset, bytes.length - offset, offset); if (count <= 0) fail('Buck call build report changed while being read'); offset += count; }
    return hashBytes(bytes);
  } catch (error) {
    if (String(error?.code) === 'ELOOP') fail('Buck call build report may not be a symbolic link');
    throw error;
  } finally { if (descriptor !== undefined) close(descriptor); }
}
function assertSafeReceiptTree(root) {
  const stat = lstatSync(root);
  if (!stat.isDirectory() || stat.isSymbolicLink()) fail('receipt staging tree is not a real directory');
  for (const name of readdirSync(root)) {
    const child = path.join(root, name); const childStat = lstatSync(child);
    if (childStat.isSymbolicLink()) fail('receipt staging tree contains a symbolic link');
    if (childStat.isDirectory()) assertSafeReceiptTree(child);
  }
}
function validateResourceVectors(plan, entries) {
  const budgets = plan.policy?.resource_budgets;
  if (budgets === undefined) return;
  if (!isPlainObject(budgets)) fail('resource budget authority is malformed');
  for (const key of RESOURCE_KEYS) if (!Number.isInteger(budgets[key]) || budgets[key] < 0) fail('resource budget authority is malformed');
  for (const entry of entries) {
    if (!isPlainObject(entry.resources)) fail('scheduled cohort lacks full resource vector');
    for (const key of RESOURCE_KEYS) {
      if (!Number.isInteger(entry.resources[key]) || entry.resources[key] < 0) fail('scheduled cohort has malformed resource vector');
      if (entry.resources[key] > budgets[key]) fail(`scheduled cohort exceeds ${key} budget`);
    }
  }
  return budgets;
}

/** Execute admitted cohorts within the planner's declared bounded cold-Rust capacity. */
export async function executeVerificationQueue(plan, supplied = {}) {
  if (FORBIDDEN_WRAPPER_ENV.some((key) => process.env[key] !== undefined)) fail('wrapper-affecting environment is forbidden for compatible exact-SHA cohorts');
  const entries = validateVerificationPlan(plan);
  const resourceBudgets = validateResourceVectors(plan, entries);
  const options = {
    receiptRoot: supplied.receiptRoot ?? path.join(process.cwd(), '.tmp/console-verification'),
    listWorktrees: supplied.listWorktrees ?? (() => defaultListWorktrees(process.cwd())),
    inspectWorktree: supplied.inspectWorktree ?? defaultInspectWorktree,
    run: supplied.run ?? defaultRun,
    toolDigest: supplied.toolDigest ?? defaultToolDigest,
    platformFacts: supplied.platformFacts ?? defaultPlatformFacts,
    monotonicNow: supplied.monotonicNow ?? defaultMonotonicNow,
    queryMetadata: supplied.queryMetadata ?? defaultQueryMetadata,
    resourceSnapshot: supplied.resourceSnapshot ?? defaultResourceSnapshot,
    beforePromotion: supplied.beforePromotion ?? (() => {}),
  };
  const receiptRoot = canonicalizeReceiptRoot(options.receiptRoot);
  const declaredCapacity = plan.policy?.cold_rust_compile_lanes;
  const maxCohorts = supplied.maxCohorts ?? declaredCapacity ?? 1;
  if (!Number.isInteger(maxCohorts) || maxCohorts < 1 || (declaredCapacity !== undefined && (!Number.isInteger(declaredCapacity) || maxCohorts > declaredCapacity))) fail('max cohorts exceeds declared cold-Rust capacity');
  if (existsSync(receiptRoot)) fail('local receipt root already exists; refusing to mix or overwrite evidence');
  const stagingRoot = mkdtempSync(path.join(prepareReceiptParent(receiptRoot), `.console-verification-staging-${process.pid}-`));
  if (lstatSync(stagingRoot).isSymbolicLink()) { rmSync(stagingRoot, { recursive: true, force: true }); fail('local receipt staging root is a symbolic link'); }
  let promoted = false;
  const activeChildren = new Set(); let interrupted = null;
  const interrupt = (signal) => { interrupted ??= signal; void terminateActiveChildren(activeChildren); };
  const onSigint = () => interrupt('SIGINT'); const onSigterm = () => interrupt('SIGTERM');
  process.once('SIGINT', onSigint); process.once('SIGTERM', onSigterm);
  try {
  let peakFd = 0; let peakRssKb = 0;
  const sampleResources = () => {
    const sample = options.resourceSnapshot();
    if (!Number.isInteger(sample?.fd) || sample.fd < 0 || !Number.isInteger(sample?.rss_kb) || sample.rss_kb < 0) fail('resource telemetry is malformed');
    peakFd = Math.max(peakFd, sample.fd); peakRssKb = Math.max(peakRssKb, sample.rss_kb); return sample;
  };
  const runEntry = async (entry) => {
    const selection = resolveWorktree(entry.verification_sha, options);
    const worktree = selection.selected.path;
    recheckWorktree(worktree, entry.verification_sha, options.inspectWorktree);
    const partition = partitionTargetsByMetadata(entry.buck2_targets, options.queryMetadata(entry.buck2_targets, worktree));
    const identity = hashJson({ verification_sha: entry.verification_sha, targets: entry.buck2_targets, buck2_sha256: options.toolDigest(worktree), execution_platform: options.platformFacts() });
    const staging = requireDirectory(stagingRoot, path.join(stagingRoot, entry.verification_sha, identity));
    mkdirSync(staging, { recursive: true, mode: 0o700 });
    try {
      const commands = buildVerificationCommands(entry, worktree, staging, partition);
      const calls = [];
      for (const command of commands) {
        recheckWorktree(worktree, entry.verification_sha, options.inspectWorktree);
        const started = options.monotonicNow();
        const resourceStart = sampleResources();
        const execution = await options.run(command, worktree, activeChildren);
        const ended = options.monotonicNow();
        const resourceEnd = sampleResources();
        if (interrupted || execution.status !== 0 || execution.signal) fail(`${command.kind} Buck call failed or was interrupted`);
        calls.push({ kind: command.kind, targets: command.targets, argv: command.argv, started_monotonic_ns: started, ended_monotonic_ns: ended, exit_status: execution.status, resource_start: resourceStart, resource_end: resourceEnd, build_report_sha256: readBuildReport(command.reportPath, stagingRoot) });
      }
      const receipt = stable({ schema_version: 'console-verification-receipt-v1', status: 'passed', verification_sha: entry.verification_sha, cache_affinity: entry.cache_affinity, execution: entry.execution, verification_identity_sha256: identity, selected_worktree: worktree, candidate_worktrees: selection.candidates.map((candidate) => candidate.path), target_partition: partition, buck2_sha256: options.toolDigest(worktree), execution_platform: options.platformFacts(), isolation_absent: true, calls });
      writeFileSync(path.join(staging, 'receipt.json'), `${JSON.stringify(receipt)}\n`, { mode: 0o600, flag: 'wx' });
      return { verification_sha: entry.verification_sha, receipt_relative_path: path.relative(stagingRoot, path.join(staging, 'receipt.json')), worktree, tool_digest: options.toolDigest(worktree) };
    } catch (error) {
      throw error;
    }
  };
  const pending = [...entries]; const results = []; let activeCohorts = 0; let peakCohorts = 0;
  const available = () => Object.fromEntries(RESOURCE_KEYS.map((key) => [key, resourceBudgets?.[key] ?? Number.MAX_SAFE_INTEGER]));
  const outcomes = [];
  while (pending.length) {
    const capacity = available(); const batch = [];
    for (let index = 0; index < pending.length && batch.length < maxCohorts;) {
      const entry = pending[index]; const resources = entry.resources ?? {};
      if (RESOURCE_KEYS.every((key) => (resources[key] ?? 0) <= capacity[key])) {
        pending.splice(index, 1); batch.push(entry); for (const key of RESOURCE_KEYS) capacity[key] -= resources[key] ?? 0;
      } else index += 1;
    }
    if (!batch.length) { rmSync(stagingRoot, { recursive: true, force: true }); fail('no pending cohort fits the declared resource budget'); }
    activeCohorts = batch.length; peakCohorts = Math.max(peakCohorts, activeCohorts);
    outcomes.push(...await Promise.allSettled(batch.map((entry) => runEntry(entry)))); activeCohorts = 0;
    if (outcomes.some((outcome) => outcome.status === 'rejected')) break;
  }
  const rejected = outcomes.find((outcome) => outcome.status === 'rejected');
  if (rejected) throw rejected.reason;
  results.push(...outcomes.map((outcome) => outcome.value));
  for (const receipt of results) {
    recheckWorktree(receipt.worktree, receipt.verification_sha, options.inspectWorktree);
    if (options.toolDigest(receipt.worktree) !== receipt.tool_digest) fail('selected Buck tool changed after cohort execution');
  }
  if (interrupted) fail('verification queue was interrupted before receipt promotion');
  writeFileSync(path.join(stagingRoot, 'queue-receipt.json'), `${JSON.stringify(stable({ schema_version: 'console-verification-queue-receipt-v1', status: 'passed', cohorts: results.map((entry) => entry.verification_sha).sort(), max_cohorts: maxCohorts, orchestrator_peak_cohorts: peakCohorts, orchestrator_peak_fd: peakFd, orchestrator_peak_rss_kb: peakRssKb }))}\n`, { mode: 0o600, flag: 'wx' });
  options.beforePromotion();
  assertSafeReceiptTree(stagingRoot);
  if (interrupted) fail('verification queue was interrupted before receipt promotion');
  if (existsSync(receiptRoot)) fail('local receipt root appeared before promotion; refusing to overwrite or mix evidence');
  renameSync(stagingRoot, receiptRoot);
  promoted = true;
  const ordered = results.sort((left, right) => left.verification_sha.localeCompare(right.verification_sha, 'en'));
  for (const receipt of ordered) { receipt.receipt_path = path.join(receiptRoot, receipt.receipt_relative_path); delete receipt.receipt_relative_path; delete receipt.worktree; delete receipt.tool_digest; }
  Object.defineProperty(ordered, 'peak_cohorts', { value: peakCohorts, enumerable: false });
  Object.defineProperty(ordered, 'max_cohorts', { value: maxCohorts, enumerable: false });
  Object.defineProperty(ordered, 'orchestrator_peak_fd', { value: peakFd, enumerable: false });
  Object.defineProperty(ordered, 'orchestrator_peak_rss_kb', { value: peakRssKb, enumerable: false });
  return ordered;
  } finally {
    process.removeListener('SIGINT', onSigint); process.removeListener('SIGTERM', onSigterm);
    await terminateActiveChildren(activeChildren);
    if (!promoted) rmSync(stagingRoot, { recursive: true, force: true });
  }
}

function parseArgs(argv) {
  const result = { planPath: null, receiptRoot: null, candidate: null, authorityTip: null, syntheticMerge: null, admission: null, maxCohorts: null };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === '--help' || flag === '-h') return { help: true };
    const value = argv[index + 1];
    if (!value) fail(`missing value for ${flag}`);
    if (flag === '--plan') result.planPath = value;
    else if (flag === '--receipt-root') result.receiptRoot = value;
    else if (flag === '--candidate') result.candidate = value;
    else if (flag === '--authority-tip') result.authorityTip = value;
    else if (flag === '--synthetic-merge') result.syntheticMerge = value;
    else if (flag === '--admission') result.admission = value;
    else if (flag === '--max-cohorts') result.maxCohorts = Number(value);
    else fail(`unknown argument ${flag}`);
    index += 1;
  }
  if (result.help) return result;
  result.admission ??= result.candidate;
  if (!result.planPath || !result.candidate || !result.authorityTip || !result.syntheticMerge) fail('--plan, --candidate, --authority-tip, and --synthetic-merge are required');
  return result;
}
function recomputePlan(authority) {
  const planner = path.join(path.dirname(new URL(import.meta.url).pathname), 'plan-fanout.mjs');
  const args = [planner, '--candidate', authority.candidate, '--authority-tip', authority.authorityTip, '--synthetic-merge', authority.syntheticMerge, '--admission', authority.admission];
  const result = spawnSync(process.execPath, args, { cwd: process.cwd(), encoding: 'utf8' });
  if (result.error || result.status !== 0) fail(`canonical plan recomputation failed: ${result.stderr || result.error?.message || 'unknown error'}`);
  try { return JSON.parse(result.stdout); } catch { fail('canonical plan recomputation returned malformed JSON'); }
}
async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) { process.stdout.write('usage: node scripts/console/run-verification-queue.mjs --plan <fanout-plan.json> --candidate <sha> --authority-tip <sha> --synthetic-merge <sha> [--admission <sha>] [--receipt-root <ignored-local-dir>] [--max-cohorts <1..declared>]\n'); return; }
  const planBytes = readFileSync(args.planPath, 'utf8'); const plan = JSON.parse(planBytes);
  const authority = { candidate: args.candidate, authorityTip: args.authorityTip, syntheticMerge: args.syntheticMerge, admission: args.admission };
  const canonicalPlan = recomputePlan(authority); verifyPlanAgainstAuthority(plan, authority, () => canonicalPlan); verifyPlanBytes(planBytes, canonicalPlan);
  const receipts = await executeVerificationQueue(canonicalPlan, { receiptRoot: args.receiptRoot, maxCohorts: args.maxCohorts ?? undefined });
  process.stdout.write(`${JSON.stringify({ status: 'passed', max_cohorts: receipts.max_cohorts, peak_cohorts: receipts.peak_cohorts, orchestrator_peak_fd: receipts.orchestrator_peak_fd, orchestrator_peak_rss_kb: receipts.orchestrator_peak_rss_kb, receipts })}\n`);
}
if (import.meta.url === new URL(process.argv[1], 'file:').href) {
  main().catch((error) => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}
