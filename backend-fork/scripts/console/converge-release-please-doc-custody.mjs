#!/usr/bin/env node
/**
 * Rewrite the exact Release Please action tip so CHANGELOG custody converges.
 *
 * Release Please only edits `.release-please-manifest.json` + `CHANGELOG.md`.
 * `CHANGELOG.md` is first-party manifest evidence, so `check:doc-manifest` /
 * `check:doc-links` fail until the seed + index blob_sha rows are regenerated.
 * This script keeps a single bot tip (same subject/parent) whose parent..tip
 * diff is the release core paths plus the documentation custody pair — the
 * fail-closed set admitted by `release-please-bot-candidate.mjs`.
 *
 * Security / operability constraints:
 * - Executable code always comes from the protected checkout (this script's
 *   directory / Actions main checkout). The tip is opened only as a data
 *   worktree; its `scripts/**` are never executed.
 * - Custody bytes are exactly the protected generator's `--write` output.
 * - The protected action output, live PR metadata, exact parent, exact bot
 *   commit, and generated core bytes are bound before a write token is used.
 * - Rewrites require RELEASE_PLEASE_TOKEN (or CONSOLE_RELEASE_PUSH_TOKEN) only
 *   for transport. Non-transport child environments never inherit either
 *   push-token name; read/API authority remains the workflow GITHUB_TOKEN.
 *
 * Intended caller: `.github/workflows/release-please.yml` immediately after the
 * pinned action. There is no heuristic PR discovery and no identity-healing
 * path: a raced or poisoned tip fails before mutation.
 */
import { execFileSync } from 'node:child_process';
import { appendFileSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  RELEASE_PLEASE_BOT_EMAIL,
  RELEASE_PLEASE_BOT_NAME,
  RELEASE_PLEASE_COMMITTER_EMAIL,
  RELEASE_PLEASE_COMMITTER_NAME,
  RELEASE_PLEASE_CUSTODY_PATHS,
  RELEASE_PLEASE_HEAD_REF,
  RELEASE_PLEASE_PATHS,
  RELEASE_PLEASE_SUBJECT,
  assertReleasePleaseBotIdentity,
  assertReleasePleaseBotPathDiff,
  gitCommitIdentity,
  gitRawDiff,
  releasePleasePrCreatorClass,
  releasePleaseCustodyRewritePlan,
} from './release-please-bot-candidate.mjs';
import {
  assertReleasePleasePrEnvelope,
  deriveReleasePleasePrEnvelope,
} from './release-please-pr-envelope.mjs';

const PROTECTED_ROOT = process.cwd();
const PROTECTED_GENERATOR = join(
  dirname(fileURLToPath(import.meta.url)),
  'generate-documentation-manifest.mjs',
);
const ALLOWED = [...RELEASE_PLEASE_PATHS, ...RELEASE_PLEASE_CUSTODY_PATHS];
const SHA = /^[0-9a-f]{40}$/;
const RELEASE_PUSH_ASKPASS = `#!/bin/sh
set -eu
case "\${1-}" in
  Password*) printf '%s\\n' "\$CONSOLE_RELEASE_PUSH_TOKEN" ;;
  *) exit 1 ;;
esac
`;

function fail(message) {
  throw new Error(`release custody convergence: ${message}`);
}

function nonEmptyString(value, label) {
  if (typeof value !== 'string' || value.length === 0) fail(`${label} must be a non-empty string`);
  return value;
}

function exactSha(value, label) {
  if (!SHA.test(value ?? '')) fail(`${label} must be a lowercase 40-character SHA`);
  return value;
}

function positiveSafeInteger(value, label) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    fail(`${label} must be a positive safe integer`);
  }
  return value;
}

function assertStableReleasePr(pr, expected, phase) {
  if (pr === null || Array.isArray(pr) || typeof pr !== 'object') {
    fail(`${phase} live PR metadata must be an object`);
  }
  const checks = [
    [pr.number === expected.number, 'number'],
    [pr.state === 'open', 'state'],
    [pr.draft === false, 'draft state'],
    [pr.title === expected.title, 'title'],
    [pr.body === expected.body, 'body'],
    [pr?.user?.login === expected.creatorLogin, 'creator login'],
    [pr?.user?.id === expected.creatorId, 'creator id'],
    [pr?.head?.ref === expected.headRef, 'head ref'],
    [pr?.head?.repo?.full_name === expected.repository, 'head repository name'],
    [pr?.head?.repo?.id === expected.repositoryId, 'head repository id'],
    [pr?.base?.ref === 'dev', 'base ref'],
    [pr?.base?.sha === expected.parentSha, 'base SHA'],
    [pr?.base?.repo?.full_name === expected.repository, 'base repository name'],
    [pr?.base?.repo?.id === expected.repositoryId, 'base repository id'],
  ];
  for (const [valid, label] of checks) {
    if (!valid) fail(`${phase} live PR ${label} changed`);
  }
}

export function parseReleasePleaseActionPr(raw) {
  if (typeof raw !== 'string' || raw.length === 0) fail('release action PR output is missing');
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    fail('release action PR output must be valid JSON');
  }
  if (parsed === null || Array.isArray(parsed) || typeof parsed !== 'object') {
    fail('release action PR output must be an object');
  }
  if (!Number.isSafeInteger(parsed.number) || parsed.number <= 0) {
    fail('release action PR output number must be a positive integer');
  }
  for (const key of ['headBranchName', 'baseBranchName', 'title', 'body']) {
    nonEmptyString(parsed[key], `release action PR output ${key}`);
  }
  if (!Array.isArray(parsed.labels) || !parsed.labels.every((value) => typeof value === 'string')) {
    fail('release action PR output labels must be a string array');
  }
  if (!Array.isArray(parsed.files)) fail('release action PR output files must be an array');
  return Object.freeze({
    headBranchName: parsed.headBranchName,
    baseBranchName: parsed.baseBranchName,
    number: parsed.number,
    title: parsed.title,
    body: parsed.body,
    labels: Object.freeze([...parsed.labels]),
    files: Object.freeze([...parsed.files]),
  });
}

export function assertReleasePleasePrePushSnapshot({
  actionPr,
  initialPr,
  repository,
  expectedParentSha,
  oldTip,
} = {}) {
  if (!actionPr || typeof actionPr !== 'object') fail('release action PR output is unavailable');
  const repo = nonEmptyString(repository, 'protected repository');
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo)) {
    fail('protected repository must be an exact owner/name pair');
  }
  const parentSha = exactSha(expectedParentSha, 'triggering main SHA');
  const oldSha = exactSha(oldTip, 'pre-push lease tip SHA');
  if (initialPr === null || Array.isArray(initialPr) || typeof initialPr !== 'object') {
    fail('pre-push live PR metadata must be an object');
  }
  const prNumber = positiveSafeInteger(actionPr.number, 'release action PR number');
  const title = nonEmptyString(actionPr.title, 'release action PR title');
  const body = nonEmptyString(actionPr.body, 'release action PR body');
  const headRef = nonEmptyString(actionPr.headBranchName, 'release action PR head ref');
  if (!RELEASE_PLEASE_HEAD_REF.test(headRef)) {
    fail('release action PR head ref must match the release-please branch pattern');
  }
  if (actionPr.baseBranchName !== 'dev') fail('release action PR base must be dev');

  if (releasePleasePrCreatorClass(initialPr?.user) === null) {
    fail('pre-push PR creator must be the exact bot or pinned transport principal');
  }

  const expected = Object.freeze({
    number: prNumber,
    title,
    body,
    creatorLogin: initialPr.user.login,
    creatorId: positiveSafeInteger(initialPr?.user?.id, 'pre-push PR creator id'),
    headRef,
    repository: repo,
    repositoryId: positiveSafeInteger(initialPr?.head?.repo?.id, 'pre-push repository id'),
    parentSha,
  });
  if (initialPr?.base?.repo?.id !== expected.repositoryId) {
    fail('pre-push head and base repository ids must match');
  }
  assertStableReleasePr(initialPr, expected, 'pre-push');
  if (exactSha(initialPr?.head?.sha, 'pre-push live PR head SHA') !== oldSha) {
    fail('pre-push live PR head must equal the lease tip');
  }
  return Object.freeze({ expected, oldSha });
}

export function pollReleasePleasePostPushHead({
  actionPr,
  initialPr,
  repository,
  expectedParentSha,
  oldTip,
  newTip,
  readPullRequest,
  sleep,
  maxReads = 20,
  delayMs = 500,
} = {}) {
  const newSha = exactSha(newTip, 'post-push custody tip SHA');
  if (typeof readPullRequest !== 'function') fail('post-push PR reader must be a function');
  if (typeof sleep !== 'function') fail('post-push sleeper must be a function');
  if (!Number.isSafeInteger(maxReads) || maxReads < 1 || maxReads > 20) {
    fail('post-push maximum reads must be an integer from 1 through 20');
  }
  if (!Number.isSafeInteger(delayMs) || delayMs < 0 || delayMs > 500) {
    fail('post-push delay must be an integer from 0 through 500 milliseconds');
  }
  const { expected, oldSha } = assertReleasePleasePrePushSnapshot({
    actionPr,
    initialPr,
    repository,
    expectedParentSha,
    oldTip,
  });
  if (oldSha === newSha) fail('pre-push and post-push tips must differ');

  for (let read = 1; read <= maxReads; read += 1) {
    const current = readPullRequest();
    assertStableReleasePr(current, expected, 'post-push');
    const currentSha = exactSha(current?.head?.sha, 'post-push live PR head SHA');
    if (currentSha === newSha) return current;
    if (currentSha !== oldSha) {
      fail(`unexpected post-push PR head ${currentSha}; expected old lease tip or new custody tip`);
    }
    if (read === maxReads) {
      fail(`post-push PR remained at old lease tip after ${maxReads} reads`);
    }
    sleep(delayMs);
  }
  fail('post-push PR polling exhausted unexpectedly');
}

/**
 * Bind Release Please's protected action output to the exact two-file core tip.
 * A later PAT rewrite may add only deterministic custody bytes; it cannot bless
 * a raced human tip or content not represented by the pinned action output.
 */
export function assertReleasePleaseActionCoreBinding({
  actionPr,
  livePr,
  repository,
  actualHeadSha,
  expectedParentSha,
  actualParentSha,
  identity,
  pathChanges,
  baseManifest,
  headManifest,
  baseChangelog,
  headChangelog,
} = {}) {
  if (!actionPr || typeof actionPr !== 'object') fail('release action PR output is unavailable');
  if (!livePr || typeof livePr !== 'object') fail('live PR metadata is unavailable');
  const repo = nonEmptyString(repository, 'protected repository');
  const parentSha = exactSha(actualParentSha, 'release tip parent');
  if (parentSha !== exactSha(expectedParentSha, 'triggering main SHA')) {
    fail(`release tip parent ${parentSha} must equal triggering main SHA ${expectedParentSha}`);
  }
  const headSha = exactSha(actualHeadSha, 'action-bound release tip SHA');
  if (exactSha(livePr?.head?.sha, 'live PR head SHA') !== headSha) {
    fail('live PR head SHA must equal the action-bound release tip SHA');
  }

  if (livePr.number !== actionPr.number) fail('live PR number must equal release action PR output');
  if (livePr.state !== 'open') fail('live PR must remain open');
  if (livePr.title !== actionPr.title) fail('live PR title must equal release action PR output');
  if (livePr.body !== actionPr.body) fail('live PR body must equal release action PR output');
  if (releasePleasePrCreatorClass(livePr?.user) === null) {
    fail('live PR creator must be the exact bot or pinned transport principal');
  }
  if (livePr?.head?.repo?.full_name !== repo) fail('live PR head repository must equal the protected repository');
  if (livePr?.head?.ref !== actionPr.headBranchName) fail('live PR head ref must equal release action PR output');
  if (livePr?.base?.ref !== actionPr.baseBranchName || actionPr.baseBranchName !== 'dev') {
    fail('live PR base must equal the release action output and protected dev branch');
  }
  if (!RELEASE_PLEASE_HEAD_REF.test(actionPr.headBranchName)) {
    fail('release action PR head ref must match the release-please branch pattern');
  }

  assertReleasePleaseBotIdentity(identity);
  if (identity.subject !== actionPr.title) fail('release tip subject must equal release action PR title');

  const changedPaths = Array.isArray(pathChanges)
    ? pathChanges.map((change) => change?.path).sort()
    : [];
  if (changedPaths.length !== RELEASE_PLEASE_PATHS.length
    || JSON.stringify(changedPaths) !== JSON.stringify([...RELEASE_PLEASE_PATHS].sort())) {
    fail('release core must change exactly .release-please-manifest.json and CHANGELOG.md in order');
  }
  assertReleasePleaseBotPathDiff(pathChanges);

  const envelope = deriveReleasePleasePrEnvelope({
    baseManifest,
    headManifest,
    baseChangelog,
    headChangelog,
    subject: identity.subject,
    headRef: actionPr.headBranchName,
  });
  assertReleasePleasePrEnvelope(actionPr, envelope);

  return Object.freeze({
    prNumber: actionPr.number,
    headRef: actionPr.headBranchName,
    headSha,
    parentSha,
    version: envelope.version,
  });
}

export function sanitizedChildEnvironment(source = process.env) {
  const environment = { ...source };
  delete environment.CONSOLE_RELEASE_PUSH_TOKEN;
  delete environment.RELEASE_PLEASE_TOKEN;
  return environment;
}

export function releaseTransportCommitEnvironment({
  sourceCommitterEpoch,
  sourceEnvironment = process.env,
} = {}) {
  const rawEpoch = nonEmptyString(sourceCommitterEpoch, 'source tip committer epoch');
  if (!/^(0|[1-9][0-9]{0,9})$/.test(rawEpoch)) {
    fail('source tip committer epoch must be a canonical non-negative Git timestamp');
  }
  const epoch = BigInt(rawEpoch);
  if (epoch > 9_999_999_998n) {
    fail('source tip committer epoch cannot produce a bounded distinct timestamp');
  }
  return {
    ...sanitizedChildEnvironment(sourceEnvironment),
    GIT_AUTHOR_NAME: RELEASE_PLEASE_BOT_NAME,
    GIT_AUTHOR_EMAIL: RELEASE_PLEASE_BOT_EMAIL,
    GIT_COMMITTER_NAME: RELEASE_PLEASE_COMMITTER_NAME,
    GIT_COMMITTER_EMAIL: RELEASE_PLEASE_COMMITTER_EMAIL,
    // The action tip and transport rewrite can otherwise share every commit
    // field in the same second. A provably distinct committer timestamp makes
    // the PAT push advance the ref and therefore emit the required PR event.
    GIT_COMMITTER_DATE: `@${epoch + 1n} +0000`,
  };
}

export function createReleasePushAskpass(directory) {
  const root = nonEmptyString(directory, 'release push transport directory');
  const askpassPath = join(root, 'git-askpass.sh');
  writeFileSync(askpassPath, RELEASE_PUSH_ASKPASS, {
    encoding: 'utf8',
    flag: 'wx',
    mode: 0o700,
  });
  return askpassPath;
}

export function releasePushInvocation({
  repository,
  headRef,
  leaseTip,
  token,
  askpassPath,
  sourceEnvironment = process.env,
} = {}) {
  const repo = nonEmptyString(repository, 'release push repository');
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo)) {
    fail('release push repository must be an exact owner/name pair');
  }
  const ref = nonEmptyString(headRef, 'release push head ref');
  if (!RELEASE_PLEASE_HEAD_REF.test(ref)) fail('release push head ref is outside the release-please class');
  const expectedTip = exactSha(leaseTip, 'release push lease tip');
  const secret = nonEmptyString(token, 'release push token');
  if (/[\0\r\n]/.test(secret)) fail('release push token must not contain control bytes');
  const askpass = nonEmptyString(askpassPath, 'release push askpass path');
  return Object.freeze({
    args: Object.freeze([
      '-c', 'credential.helper=',
      'push',
      `--force-with-lease=refs/heads/${ref}:${expectedTip}`,
      `https://x-access-token@github.com/${repo}.git`,
      `HEAD:refs/heads/${ref}`,
    ]),
    env: Object.freeze({
      ...sanitizedChildEnvironment(sourceEnvironment),
      CONSOLE_RELEASE_PUSH_TOKEN: secret,
      GIT_ASKPASS: askpass,
      GIT_ASKPASS_REQUIRE: 'force',
      GIT_TERMINAL_PROMPT: '0',
      LANG: 'C',
      LC_ALL: 'C',
    }),
  });
}

export function redactReleasePushError(error, tokens = [
  process.env.CONSOLE_RELEASE_PUSH_TOKEN,
  process.env.RELEASE_PLEASE_TOKEN,
]) {
  let message = error instanceof Error ? error.stack || error.message : String(error);
  for (const token of tokens) {
    if (typeof token === 'string' && token.length > 0) message = message.replaceAll(token, '[REDACTED]');
  }
  return message;
}

function run(cmd, args, opts = {}) {
  return execFileSync(cmd, args, {
    cwd: opts.cwd ?? PROTECTED_ROOT,
    encoding: Object.hasOwn(opts, 'encoding') ? opts.encoding : 'utf8',
    stdio: opts.stdio ?? ['ignore', 'pipe', 'pipe'],
    env: opts.env ?? sanitizedChildEnvironment(),
  });
}

function ghJson(args) {
  const raw = run('gh', args);
  return raw.trim() ? JSON.parse(raw) : null;
}

function git(cwd, args) {
  return run('git', args, { cwd });
}

function gitBytes(cwd, args) {
  return run('git', args, { cwd, encoding: null });
}

function pushToken() {
  return process.env.CONSOLE_RELEASE_PUSH_TOKEN
    || process.env.RELEASE_PLEASE_TOKEN
    || '';
}

function assertPushTokenCanScheduleChecks(token) {
  const defaultActions = process.env.GITHUB_TOKEN || '';
  if (defaultActions && token === defaultActions) {
    throw new Error(
      'refusing to rewrite release tip with default GITHUB_TOKEN: GitHub suppresses '
      + 'workflow triggers on that push, so Required checks would not schedule. '
      + 'Set RELEASE_PLEASE_TOKEN (or CONSOLE_RELEASE_PUSH_TOKEN) for the converge push.',
    );
  }
}

function emitProofOutputs({ prNumber, headSha, parentSha }) {
  const output = process.env.GITHUB_OUTPUT;
  if (!output) return;
  appendFileSync(output, `pr_number=${prNumber}\nhead_sha=${headSha}\nparent_sha=${parentSha}\n`, 'utf8');
}

export function main() {
  const actionPr = parseReleasePleaseActionPr(process.env.RELEASE_PLEASE_PR ?? '');
  const repository = nonEmptyString(process.env.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY');
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) {
    fail('GITHUB_REPOSITORY must be an exact owner/name pair');
  }
  const expectedParentSha = exactSha(process.env.GITHUB_SHA, 'GITHUB_SHA');
  if (!RELEASE_PLEASE_HEAD_REF.test(actionPr.headBranchName)) {
    fail('release action PR head ref must match the release-please branch pattern');
  }
  if (actionPr.baseBranchName !== 'dev') fail('release action PR base must be dev');

  const pr = ghJson(['api', `repos/${repository}/pulls/${actionPr.number}`]);
  run('git', [
    'fetch', '--no-tags', 'origin',
    `+refs/heads/${actionPr.headBranchName}:refs/remotes/origin/${actionPr.headBranchName}`,
  ]);
  const tip = run('git', ['rev-parse', `refs/remotes/origin/${actionPr.headBranchName}`]).trim();

  const gitAt = (root, args) => run('git', ['-C', root, ...args]);
  const identity = gitCommitIdentity(PROTECTED_ROOT, tip, gitAt);
  const commitLine = gitAt(PROTECTED_ROOT, ['rev-list', '--parents', '-n', '1', tip]).trim().split(' ');
  if (commitLine.length !== 2 || commitLine[0] !== tip) {
    fail('release action tip must be a one-parent commit');
  }
  const parent = commitLine[1];
  const pathChanges = gitRawDiff(PROTECTED_ROOT, parent, tip, gitAt);
  const binding = assertReleasePleaseActionCoreBinding({
    actionPr,
    livePr: pr,
    repository,
    actualHeadSha: tip,
    expectedParentSha,
    actualParentSha: parent,
    identity,
    pathChanges,
    baseManifest: gitBytes(PROTECTED_ROOT, ['show', `${parent}:.release-please-manifest.json`]),
    headManifest: gitBytes(PROTECTED_ROOT, ['show', `${tip}:.release-please-manifest.json`]),
    baseChangelog: gitBytes(PROTECTED_ROOT, ['show', `${parent}:CHANGELOG.md`]),
    headChangelog: gitBytes(PROTECTED_ROOT, ['show', `${tip}:CHANGELOG.md`]),
  });
  assertReleasePleasePrePushSnapshot({
    actionPr,
    initialPr: pr,
    repository,
    expectedParentSha,
    oldTip: tip,
  });

  const work = mkdtempSync(join(tmpdir(), 'console-rp-custody-'));
  try {
    // Data-only tip worktree: never execute tip scripts; generator path is protected.
    run('git', ['worktree', 'add', '--detach', work, tip]);

    run('node', [PROTECTED_GENERATOR, '--write'], { cwd: work });

    const dirty = [
      ...git(work, ['diff', '--name-only', '-z', 'HEAD']).split('\0').filter(Boolean),
      ...git(work, ['ls-files', '--others', '--exclude-standard', '-z']).split('\0').filter(Boolean),
    ].sort();
    const dirtyCustodyPaths = dirty.filter((path) => RELEASE_PLEASE_CUSTODY_PATHS.includes(path));
    // Fail closed if the protected generator dirtied anything outside custody.
    for (const path of dirty) {
      if (!RELEASE_PLEASE_CUSTODY_PATHS.includes(path)) {
        throw new Error(`unexpected dirty path outside custody pair: ${path}`);
      }
    }

    // The action/core binding above already authenticated the exact bot tip.
    // The plan may now decide only whether deterministic custody bytes changed.
    const plan = releasePleaseCustodyRewritePlan({
      identity,
      pathChanges,
      dirtyCustodyPaths,
    });
    if (dirtyCustodyPaths.length !== 0
      && dirtyCustodyPaths.length !== RELEASE_PLEASE_CUSTODY_PATHS.length) {
      throw new Error('documentation custody paths must be regenerated together');
    }
    // Even if custody is already current, replace the default-token action tip
    // with a tree-equivalent PAT transport commit so PR workflows are scheduled.
    const reason = plan.action === 'noop' ? 'transport' : plan.reason;
    const token = pushToken();
    if (!token) throw new Error('RELEASE_PLEASE_TOKEN or CONSOLE_RELEASE_PUSH_TOKEN is required');
    assertPushTokenCanScheduleChecks(token);

    const subject = identity.subject;
    if (!RELEASE_PLEASE_SUBJECT.test(subject)) {
      throw new Error(`tip subject is not a release subject: ${subject}`);
    }
    const sourceCommitterEpoch = git(work, ['show', '-s', '--format=%ct', tip]).trim();

    git(work, ['reset', '--soft', parent]);
    git(work, ['add', '--', ...ALLOWED]);
    const staged = git(work, ['diff', '--cached', '--name-only', '-z'])
      .split('\0').filter(Boolean)
      .sort();
    const expected = [...RELEASE_PLEASE_PATHS, ...dirtyCustodyPaths].sort();
    if (JSON.stringify(staged) !== JSON.stringify(expected)) {
      throw new Error(`staged set ${JSON.stringify(staged)} != allowlisted ${JSON.stringify(expected)}`);
    }

    // Match release-please / GitHub web-flow identity exactly (author bot, committer GitHub).
    // gpgsign=false: bot tips are unsigned by design; identity headers are the gate.
    run('git', ['-c', 'commit.gpgsign=false', 'commit', '--no-verify', '-m', subject], {
      cwd: work,
      env: releaseTransportCommitEnvironment({ sourceCommitterEpoch }),
    });
    const newTip = git(work, ['rev-parse', 'HEAD']).trim();
    if (newTip === tip) {
      throw new Error('custody transport commit did not advance the release tip; refusing a no-op push');
    }
    const healed = gitCommitIdentity(work, newTip, (root, args) => git(root, args));
    assertReleasePleaseBotIdentity(healed);
    const rewrittenParents = git(work, ['rev-list', '--parents', '-n', '1', newTip]).trim().split(' ');
    if (rewrittenParents.length !== 2 || rewrittenParents[1] !== parent) {
      throw new Error('rewrite changed the action-bound parent; refusing to push');
    }
    try {
      git(work, ['diff', '--quiet', tip, newTip, '--', ...RELEASE_PLEASE_PATHS]);
    } catch {
      throw new Error('rewrite changed action-bound release core bytes; refusing to push');
    }
    assertReleasePleaseBotPathDiff(gitRawDiff(work, parent, newTip, (root, args) => git(root, args)));

    // Push with the scheduling-capable token (not the Actions default). A
    // private one-shot askpass keeps the credential out of argv and therefore
    // out of Node command-failure stacks and process listings.
    const transportDirectory = mkdtempSync(join(tmpdir(), 'console-rp-transport-'));
    try {
      const askpassPath = createReleasePushAskpass(transportDirectory);
      const invocation = releasePushInvocation({
        repository,
        headRef: actionPr.headBranchName,
        leaseTip: tip,
        token,
        askpassPath,
      });
      run('git', invocation.args, { cwd: work, env: invocation.env });
    } finally {
      rmSync(transportDirectory, { recursive: true, force: true });
    }
    pollReleasePleasePostPushHead({
      actionPr,
      initialPr: pr,
      repository,
      expectedParentSha,
      oldTip: tip,
      newTip,
      readPullRequest: () => ghJson([
        'api', `repos/${repository}/pulls/${actionPr.number}`,
      ]),
      sleep: (milliseconds) => {
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds);
      },
    });
    emitProofOutputs({ prNumber: binding.prNumber, headSha: newTip, parentSha: parent });
    console.log(
      `converge-release-please-doc-custody: PR #${pr.number} tip ${tip} -> ${newTip}`
      + ` (rewritten reason=${reason})`,
    );
    return {
      status: 'rewritten', pr: pr.number, tip, newTip, parent, reason,
    };
  } finally {
    try { run('git', ['worktree', 'remove', '--force', work]); } catch { /* best-effort */ }
    try { rmSync(work, { recursive: true, force: true }); } catch { /* best-effort */ }
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(redactReleasePushError(error));
    process.exit(1);
  }
}
