import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import yaml from 'js-yaml';
import {
  assertReleasePleaseActionCoreBinding,
  assertReleasePleasePrePushSnapshot,
  createReleasePushAskpass,
  parseReleasePleaseActionPr,
  pollReleasePleasePostPushHead,
  redactReleasePushError,
  releasePushInvocation,
  releaseTransportCommitEnvironment,
  sanitizedChildEnvironment,
} from './converge-release-please-doc-custody.mjs';
import {
  RELEASE_PLEASE_BOT_EMAIL,
  RELEASE_PLEASE_BOT_NAME,
  RELEASE_PLEASE_COMMITTER_EMAIL,
  RELEASE_PLEASE_COMMITTER_NAME,
  RELEASE_PLEASE_PATHS,
  RELEASE_PLEASE_TRANSPORT_ID,
  RELEASE_PLEASE_TRANSPORT_NAME,
} from './release-please-bot-candidate.mjs';
import { installGitFixtureEnvironment } from '../lib/git-fixture-environment.mjs';

installGitFixtureEnvironment();

const C = 'c'.repeat(40);
const T = 'a'.repeat(40);
const N = 'b'.repeat(40);
const REPOSITORY = 'oyatie/console';
const REPOSITORY_ID = 1269693002;
const BOT_ID = 41898282;
const HEAD_REF = 'release-please--branches--dev--components--console';
const NOTES = [
  '## [0.3.7](https://github.com/oyatie/console/compare/v0.3.6...v0.3.7) (2026-08-16)',
  '',
  '',
  '### Bug Fixes',
  '',
  '* **ci:** separate candidate validation from release authority ([abc1234](https://github.com/oyatie/console/commit/abc1234))',
].join('\n');
const PR_BODY = [
  ':robot: I have created a release *beep* *boop*',
  '---',
  '',
  '',
  NOTES,
  '',
  '---',
  'This PR was generated with [Release Please](https://github.com/googleapis/release-please). See [documentation](https://github.com/googleapis/release-please#release-please).',
].join('\n');
const BASE_CHANGELOG = [
  '# Changelog',
  '',
  '## [0.3.6](https://github.com/oyatie/console/compare/v0.3.5...v0.3.6) (2026-08-11)',
  '',
  '',
  '### Bug Fixes',
  '',
  '* previous release',
  '',
].join('\n');
const HEAD_CHANGELOG = `# Changelog\n\n${NOTES}\n\n${BASE_CHANGELOG.slice('# Changelog\n\n'.length)}`;
const actionPr = Object.freeze({
  headBranchName: HEAD_REF,
  baseBranchName: 'dev',
  number: 760,
  title: 'chore(dev): release 0.3.7',
  body: PR_BODY,
  labels: ['autorelease: pending'],
  files: [],
});
const livePr = Object.freeze({
  number: 760,
  state: 'open',
  draft: false,
  title: actionPr.title,
  body: actionPr.body,
  user: { login: 'github-actions[bot]', id: BOT_ID },
  head: {
    ref: HEAD_REF,
    sha: T,
    repo: { full_name: REPOSITORY, id: REPOSITORY_ID },
  },
  base: {
    ref: 'dev',
    sha: C,
    repo: { full_name: REPOSITORY, id: REPOSITORY_ID },
  },
});
const identity = Object.freeze({
  authorName: RELEASE_PLEASE_BOT_NAME,
  authorEmail: RELEASE_PLEASE_BOT_EMAIL,
  committerName: RELEASE_PLEASE_COMMITTER_NAME,
  committerEmail: RELEASE_PLEASE_COMMITTER_EMAIL,
  subject: actionPr.title,
});
const pathChanges = RELEASE_PLEASE_PATHS.map((path) => ({
  path,
  status: 'M',
  oldMode: '100644',
  newMode: '100644',
  oldType: 'blob',
  newType: 'blob',
}));
const fixture = (overrides = {}) => ({
  actionPr,
  livePr,
  repository: REPOSITORY,
  actualHeadSha: T,
  expectedParentSha: C,
  actualParentSha: C,
  identity,
  pathChanges,
  baseManifest: Buffer.from('{\n  ".": "0.3.6"\n}\n'),
  headManifest: Buffer.from('{\n  ".": "0.3.7"\n}\n'),
  baseChangelog: Buffer.from(BASE_CHANGELOG),
  headChangelog: Buffer.from(HEAD_CHANGELOG),
  ...overrides,
});
const postPushPr = (headSha) => ({
  ...structuredClone(livePr),
  head: { ...structuredClone(livePr.head), sha: headSha },
});
const pollFixture = (overrides = {}) => ({
  actionPr,
  initialPr: postPushPr(T),
  repository: REPOSITORY,
  expectedParentSha: C,
  oldTip: T,
  newTip: N,
  maxReads: 3,
  delayMs: 0,
  readPullRequest: () => postPushPr(N),
  sleep: () => {},
  ...overrides,
});
const protectedReleaseIssuerClosure = Object.freeze([
  ['./converge-release-please-doc-custody.mjs', 'a3d8d44840bd3397732a9a7cfd76f49c80029adfb4f7660d09ccdf2d3d22de70'],
  ['./release-please-pr-fallback.mjs', 'f0a8fb9c2e4698b07d38092efb29de2faa7a954143d12f8befa029b5c0edc0c6'],
  ['./release-please-pr-envelope.mjs', 'df8e389bd22f46e6747fd6f93027190e5c710be8cb1998ed7c8c5dd30b627096'],
  ['./generate-documentation-manifest.mjs', 'df87d926caa67c08ccecc6eba7fca58c9f4bc1190c1e08fd3857290985379481'],
  ['./validate-documentation-archive.mjs', '8462e2650bc8a5d2802d77701d490fcae4d0b33eaa34f2a2f6fb8130e193c2e8'],
  ['./release-please-bot-candidate.mjs', '91aa1f7c4f0c882c4de560e3d5af0a37351b233d48cb974b897b7c1ebdaa3b6e'],
  ['./authority-ledger-path.mjs', '756e838e3979508d3be0b7d9974a0e719de9f1a08effbe60c272c2cad25b498e'],
  ['../check-release-metadata.mjs', '534b49d8426a1a3ae86e88ca026cf034162dd1d6289de3f9c4103e447b998b4a'],
]);
const sha256 = (value) => createHash('sha256').update(value).digest('hex');

test('post-push proof polling accepts only the exact new tip with bounded old-tip retries', () => {
  for (const [sequence, expectedReads, expectedSleeps] of [
    [[N], 1, 0],
    [[T, N], 2, 1],
    [[T, T, N], 3, 2],
  ]) {
    let reads = 0;
    let sleeps = 0;
    const accepted = pollReleasePleasePostPushHead(pollFixture({
      readPullRequest: () => postPushPr(sequence[reads++]),
      sleep: (milliseconds) => {
        assert.equal(milliseconds, 0);
        sleeps += 1;
      },
    }));
    assert.equal(accepted.head.sha, N);
    assert.equal(reads, expectedReads);
    assert.equal(sleeps, expectedSleeps);
  }
});

test('pre-push validation rejects stable metadata drift before transport work', () => {
  const independentlyBoundFields = new Set([
    'number',
    'state',
    'draft state',
    'title',
    'body',
    'creator login',
    'head ref',
    'head repository name',
    'base ref',
    'base SHA',
    'base repository name',
  ]);
  for (const [label, mutate] of postPushMetadataMutations) {
    if (!independentlyBoundFields.has(label)) continue;
    const initialPr = postPushPr(T);
    mutate(initialPr);
    const expected = label === 'creator login'
      ? /pre-push PR creator/
      : new RegExp(`pre-push live PR ${label} changed`, 'i');
    assert.throws(
      () => assertReleasePleasePrePushSnapshot({
        actionPr,
        initialPr,
        repository: REPOSITORY,
        expectedParentSha: C,
        oldTip: T,
      }),
      expected,
      label,
    );
  }
});

test('post-push proof polling times out on a persistently stale old tip', () => {
  let reads = 0;
  let sleeps = 0;
  assert.throws(() => pollReleasePleasePostPushHead(pollFixture({
    readPullRequest: () => { reads += 1; return postPushPr(T); },
    sleep: () => { sleeps += 1; },
  })), /old lease tip.*3 reads|timed out/);
  assert.equal(reads, 3);
  assert.equal(sleeps, 2);
});

test('post-push proof polling defaults to twenty reads and 500 millisecond delays', () => {
  let reads = 0;
  const delays = [];
  assert.throws(() => pollReleasePleasePostPushHead(pollFixture({
    maxReads: undefined,
    delayMs: undefined,
    readPullRequest: () => { reads += 1; return postPushPr(T); },
    sleep: (milliseconds) => { delays.push(milliseconds); },
  })), /old lease tip.*20 reads/);
  assert.equal(reads, 20);
  assert.deepEqual(delays, Array(19).fill(500));
});

test('post-push proof polling fails immediately on any non-old, non-new head', () => {
  for (const sha of [undefined, '', 'ABC', 'd'.repeat(40)]) {
    let reads = 0;
    let sleeps = 0;
    assert.throws(() => pollReleasePleasePostPushHead(pollFixture({
      readPullRequest: () => {
        reads += 1;
        const candidate = postPushPr(T);
        candidate.head.sha = sha;
        return candidate;
      },
      sleep: () => { sleeps += 1; },
    })), /head SHA|unexpected post-push PR head/);
    assert.equal(reads, 1);
    assert.equal(sleeps, 0);
  }
});

test('post-push proof polling never retries API, response, or sleep failures', () => {
  let reads = 0;
  let sleeps = 0;
  const apiFailure = new Error('GitHub API unavailable');
  assert.throws(() => pollReleasePleasePostPushHead(pollFixture({
    readPullRequest: () => { reads += 1; throw apiFailure; },
    sleep: () => { sleeps += 1; },
  })), (error) => error === apiFailure);
  assert.equal(reads, 1);
  assert.equal(sleeps, 0);

  for (const response of [null, [], 'not an object']) {
    reads = 0;
    sleeps = 0;
    assert.throws(() => pollReleasePleasePostPushHead(pollFixture({
      readPullRequest: () => { reads += 1; return response; },
      sleep: () => { sleeps += 1; },
    })), /metadata must be an object/);
    assert.equal(reads, 1);
    assert.equal(sleeps, 0);
  }

  reads = 0;
  const sleepFailure = new Error('sleep interrupted');
  assert.throws(() => pollReleasePleasePostPushHead(pollFixture({
    readPullRequest: () => { reads += 1; return postPushPr(T); },
    sleep: () => { throw sleepFailure; },
  })), (error) => error === sleepFailure);
  assert.equal(reads, 1);
});

const postPushMetadataMutations = [
  ['number', (pr) => { pr.number = 761; }],
  ['state', (pr) => { pr.state = 'closed'; }],
  ['draft state', (pr) => { pr.draft = true; }],
  ['title', (pr) => { pr.title = `${pr.title} forged`; }],
  ['body', (pr) => { pr.body = `${pr.body}\nforged`; }],
  ['creator login', (pr) => { pr.user.login = 'attacker'; }],
  ['creator id', (pr) => { pr.user.id += 1; }],
  ['head ref', (pr) => { pr.head.ref = `${pr.head.ref}-attacker`; }],
  ['head repository name', (pr) => { pr.head.repo.full_name = 'attacker/console'; }],
  ['head repository id', (pr) => { pr.head.repo.id += 1; }],
  ['base ref', (pr) => { pr.base.ref = 'attacker'; }],
  ['base SHA', (pr) => { pr.base.sha = 'e'.repeat(40); }],
  ['base repository name', (pr) => { pr.base.repo.full_name = 'attacker/console'; }],
  ['base repository id', (pr) => { pr.base.repo.id += 1; }],
];

test('post-push proof polling validates all stable metadata before old/new SHA handling', () => {
  for (const headSha of [T, N]) {
    for (const [label, mutate] of postPushMetadataMutations) {
      let reads = 0;
      let sleeps = 0;
      const candidate = postPushPr(headSha);
      mutate(candidate);
      assert.throws(
        () => pollReleasePleasePostPushHead(pollFixture({
          readPullRequest: () => { reads += 1; return candidate; },
          sleep: () => { sleeps += 1; },
        })),
        new RegExp(`post-push live PR ${label} changed`, 'i'),
        `${label} at ${headSha}`,
      );
      assert.equal(reads, 1, label);
      assert.equal(sleeps, 0, label);
    }
  }
});

const invalidInitialMutations = [
  (pr) => { pr.head.sha = N; },
  (pr) => { delete pr.user.id; },
  (pr) => { pr.user.id = 0; },
  (pr) => { delete pr.head.repo.id; },
  (pr) => { pr.head.repo.id = 0; },
  (pr) => { delete pr.base.repo.id; },
  (pr) => { pr.base.repo.id += 1; },
];

test('post-push proof polling validates all inputs before its first read', () => {
  for (const mutate of invalidInitialMutations) {
    let reads = 0;
    const initialPr = postPushPr(T);
    mutate(initialPr);
    assert.throws(() => pollReleasePleasePostPushHead(pollFixture({
      initialPr,
      readPullRequest: () => { reads += 1; return postPushPr(N); },
    })));
    assert.equal(reads, 0);
  }

  for (const overrides of [
    { oldTip: N },
    { newTip: T },
    { oldTip: 'A'.repeat(40) },
    { newTip: 'short' },
    { actionPr: { ...actionPr, number: 0 } },
    { actionPr: { ...actionPr, baseBranchName: 'attacker' } },
    { repository: 'not-an-owner-name' },
    { readPullRequest: null },
    { sleep: null },
    { maxReads: 0 },
    { maxReads: 21 },
    { maxReads: 1.5 },
    { delayMs: -1 },
    { delayMs: 501 },
    { delayMs: 0.5 },
  ]) {
    let reads = 0;
    assert.throws(() => pollReleasePleasePostPushHead(pollFixture({
      ...overrides,
      ...(Object.hasOwn(overrides, 'readPullRequest') ? {} : {
        readPullRequest: () => { reads += 1; return postPushPr(N); },
      }),
    })));
    assert.equal(reads, 0);
  }
});

test('binds an exact release action output to the generated core tip', () => {
  assert.deepEqual(parseReleasePleaseActionPr(JSON.stringify(actionPr)), actionPr);
  assert.deepEqual(assertReleasePleaseActionCoreBinding(fixture()), {
    prNumber: 760,
    headRef: HEAD_REF,
    headSha: T,
    parentSha: C,
    version: '0.3.7',
  });
});

test('binds recurring native updates to the exact pinned transport-created PR', () => {
  const transportPr = structuredClone(livePr);
  transportPr.user = { id: RELEASE_PLEASE_TRANSPORT_ID, login: RELEASE_PLEASE_TRANSPORT_NAME };
  assert.equal(assertReleasePleaseActionCoreBinding(fixture({ livePr: transportPr })).prNumber, 760);
  const initialPr = structuredClone(transportPr);
  const accepted = pollReleasePleasePostPushHead(pollFixture({
    initialPr,
    readPullRequest: () => ({
      ...structuredClone(transportPr),
      head: { ...structuredClone(transportPr.head), sha: N },
    }),
  }));
  assert.equal(accepted.head.sha, N);
  for (const user of [
    { id: RELEASE_PLEASE_TRANSPORT_ID + 1, login: RELEASE_PLEASE_TRANSPORT_NAME },
    { id: RELEASE_PLEASE_TRANSPORT_ID, login: RELEASE_PLEASE_BOT_NAME },
  ]) {
    assert.throws(
      () => assertReleasePleaseActionCoreBinding(fixture({ livePr: { ...transportPr, user } })),
      /creator/,
    );
  }
});

test('rejects a raced live PR or a tip detached from the triggering main SHA', () => {
  assert.throws(
    () => assertReleasePleaseActionCoreBinding(fixture({
      livePr: { ...livePr, head: { ...livePr.head, sha: 'b'.repeat(40) } },
    })),
    /live PR head.*action-bound tip|head SHA/,
  );
  assert.throws(
    () => assertReleasePleaseActionCoreBinding(fixture({ actualParentSha: 'd'.repeat(40) })),
    /triggering main SHA|parent/,
  );
});

test('rejects PR metadata, repository, action body, or bot identity drift', () => {
  for (const [overrides, pattern] of [
    [{ livePr: { ...livePr, body: `${PR_BODY}\nforged` } }, /body/],
    [{ livePr: { ...livePr, title: 'chore(dev): release 9.9.9' } }, /title/],
    [{ livePr: { ...livePr, user: { login: 'oyatie' } } }, /creator/],
    [{ livePr: { ...livePr, head: { ...livePr.head, repo: { full_name: 'attacker/console' } } } }, /repository/],
    [{ actionPr: { ...actionPr, headBranchName: 'evil' } }, /head ref/],
    [{ identity: { ...identity, committerName: 'Jason Lee' } }, /tip committer/],
  ]) {
    assert.throws(() => assertReleasePleaseActionCoreBinding(fixture(overrides)), pattern);
  }
});

test('rejects release core bytes not deterministically represented by the action output', () => {
  for (const [overrides, pattern] of [
    [{ headManifest: Buffer.from('{\n  ".": "0.3.8"\n}\n') }, /0\.3\.8|title|body/],
    [{ headChangelog: Buffer.from(HEAD_CHANGELOG.replace('separate candidate', 'forged candidate')) }, /CHANGELOG|release notes|PR body/],
    [{ headChangelog: Buffer.from(`${HEAD_CHANGELOG}\nforged`) }, /CHANGELOG|parent/],
    [{ pathChanges: [...pathChanges, { path: 'README.md', status: 'M', oldMode: '100644', newMode: '100644', oldType: 'blob', newType: 'blob' }] }, /exactly|release core/],
    [{ pathChanges: pathChanges.map((entry, index) => index === 0 ? { ...entry, newMode: '100755' } : entry) }, /mode-100644/],
  ]) {
    assert.throws(() => assertReleasePleaseActionCoreBinding(fixture(overrides)), pattern);
  }
});

test('rejects missing, malformed, or ambiguous release action output', () => {
  for (const raw of ['', 'null', '[]', '{}', '{']) {
    assert.throws(() => parseReleasePleaseActionPr(raw), /release action PR output/);
  }
});

test('strips scheduling-capable push tokens from every non-transport child environment', () => {
  assert.deepEqual(sanitizedChildEnvironment({
    PATH: '/usr/bin',
    GH_TOKEN: 'protected-read-token',
    GITHUB_TOKEN: 'protected-read-token',
    CONSOLE_RELEASE_PUSH_TOKEN: 'push-token',
    RELEASE_PLEASE_TOKEN: 'legacy-push-token',
  }), {
    PATH: '/usr/bin',
    GH_TOKEN: 'protected-read-token',
    GITHUB_TOKEN: 'protected-read-token',
  });
});

test('a fixed-clock custody transport commit is guaranteed to differ from the action tip', () => {
  const repository = mkdtempSync(join(tmpdir(), 'console-rp-distinct-tip-test-'));
  const git = (args, env = process.env) => execFileSync(
    'git',
    ['-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgsign=false', ...args],
    { cwd: repository, encoding: 'utf8', env, stdio: ['ignore', 'pipe', 'pipe'] },
  );
  const fixedIdentity = {
    ...process.env,
    GIT_AUTHOR_NAME: RELEASE_PLEASE_BOT_NAME,
    GIT_AUTHOR_EMAIL: RELEASE_PLEASE_BOT_EMAIL,
    GIT_COMMITTER_NAME: RELEASE_PLEASE_COMMITTER_NAME,
    GIT_COMMITTER_EMAIL: RELEASE_PLEASE_COMMITTER_EMAIL,
  };
  try {
    git(['init', '-q']);
    writeFileSync(join(repository, 'release.txt'), 'base\n');
    git(['add', 'release.txt']);
    git(['commit', '-q', '-m', 'base'], {
      ...fixedIdentity,
      GIT_AUTHOR_DATE: '@1699999999 +0000',
      GIT_COMMITTER_DATE: '@1699999999 +0000',
    });
    writeFileSync(join(repository, 'release.txt'), 'release\n');
    git(['add', 'release.txt']);
    git(['commit', '-q', '-m', actionPr.title], {
      ...fixedIdentity,
      GIT_AUTHOR_DATE: '@1700000000 +0000',
      GIT_COMMITTER_DATE: '@1700000000 +0000',
    });
    const tip = git(['rev-parse', 'HEAD']).trim();
    const parent = git(['rev-parse', 'HEAD^']).trim();
    const sourceCommitterEpoch = git(['show', '-s', '--format=%ct', tip]).trim();
    git(['reset', '--soft', parent]);
    const transportEnvironment = releaseTransportCommitEnvironment({
      sourceCommitterEpoch,
      sourceEnvironment: {
        ...fixedIdentity,
        GIT_AUTHOR_DATE: '@1700000000 +0000',
      },
    });
    git(['commit', '-q', '-m', actionPr.title], transportEnvironment);
    const newTip = git(['rev-parse', 'HEAD']).trim();
    assert.notEqual(newTip, tip);
    assert.equal(git(['show', '-s', '--format=%ct', newTip]).trim(), '1700000001');
    assert.equal(transportEnvironment.GIT_COMMITTER_DATE, '@1700000001 +0000');
    assert.equal(transportEnvironment.CONSOLE_RELEASE_PUSH_TOKEN, undefined);
    assert.equal(transportEnvironment.RELEASE_PLEASE_TOKEN, undefined);
  } finally {
    rmSync(repository, { recursive: true, force: true });
  }
  for (const sourceCommitterEpoch of ['', '-1', '01', 'not-a-time', '9999999999']) {
    assert.throws(
      () => releaseTransportCommitEnvironment({ sourceCommitterEpoch }),
      /committer epoch|bounded distinct timestamp/,
    );
  }
});

test('keeps the push token out of argv, process errors, and non-password askpass responses', () => {
  const token = `ghp_${'S'.repeat(40)}`;
  const transportDirectory = mkdtempSync(join(tmpdir(), 'console-rp-transport-test-'));
  try {
    const askpassPath = createReleasePushAskpass(transportDirectory);
    assert.equal(statSync(askpassPath).mode & 0o777, 0o700);
    assert.equal(execFileSync(askpassPath, ['Password for https://x-access-token@github.com:'], {
      encoding: 'utf8',
      env: { CONSOLE_RELEASE_PUSH_TOKEN: token },
    }), `${token}\n`);
    const wrongPrompt = spawnSync(askpassPath, ['Username for https://github.com:'], {
      encoding: 'utf8',
      env: { CONSOLE_RELEASE_PUSH_TOKEN: token },
    });
    assert.notEqual(wrongPrompt.status, 0);
    assert.doesNotMatch(`${wrongPrompt.stdout}${wrongPrompt.stderr}`, new RegExp(token));

    const invocation = releasePushInvocation({
      repository: REPOSITORY,
      headRef: HEAD_REF,
      leaseTip: T,
      token,
      askpassPath,
      sourceEnvironment: {
        PATH: process.env.PATH,
        RELEASE_PLEASE_TOKEN: 'legacy-secret',
      },
    });
    assert.doesNotMatch(JSON.stringify(invocation.args), new RegExp(token));
    assert.match(JSON.stringify(invocation.args), /https:\/\/x-access-token@github\.com\/oyatie\/console\.git/);
    assert.equal(invocation.env.CONSOLE_RELEASE_PUSH_TOKEN, token);
    assert.equal(invocation.env.RELEASE_PLEASE_TOKEN, undefined);
    assert.equal(invocation.env.GIT_ASKPASS, askpassPath);
    assert.equal(invocation.env.GIT_TERMINAL_PROMPT, '0');

    let failed;
    try {
      execFileSync('/usr/bin/false', invocation.args, { env: invocation.env });
    } catch (error) {
      failed = error;
    }
    assert.ok(failed instanceof Error);
    assert.doesNotMatch(String(failed.stack), new RegExp(token));
    assert.doesNotMatch(redactReleasePushError(new Error(`transport failed: ${token}`), [token]), new RegExp(token));
  } finally {
    rmSync(transportDirectory, { recursive: true, force: true });
  }
});

test('protected producer validates before transport and accepts the exact post-push head before proof', () => {
  const source = readFileSync(
    new URL('./converge-release-please-doc-custody.mjs', import.meta.url),
    'utf8',
  );
  const main = source.slice(source.indexOf('export function main()'));
  assert.equal([...main.matchAll(/assertReleasePleasePrePushSnapshot\(\{/g)].length, 1);
  assert.equal([...main.matchAll(/pollReleasePleasePostPushHead\(\{/g)].length, 1);
  assert.equal([...main.matchAll(/emitProofOutputs\(\{/g)].length, 1);
  const prePushValidation = main.indexOf('assertReleasePleasePrePushSnapshot({');
  const worktree = main.indexOf("run('git', ['worktree', 'add', '--detach', work, tip]);");
  const push = main.indexOf("run('git', invocation.args, { cwd: work, env: invocation.env });");
  const poll = main.indexOf('pollReleasePleasePostPushHead({', push);
  const output = main.indexOf('emitProofOutputs({ prNumber: binding.prNumber', poll);
  assert.ok(prePushValidation >= 0 && worktree > prePushValidation);
  assert.ok(push > worktree && poll > push && output > poll);
  assert.doesNotMatch(main.slice(push, output), /const finalPr = ghJson/);
});

const expectedReleaseWorkflow = Object.freeze({
  name: 'Release Please',
  on: {
    push: { branches: ['dev'] },
  },
  permissions: {},
  concurrency: { group: 'release-please', 'cancel-in-progress': false },
  jobs: {
    'release-please': {
      'runs-on': 'ubuntu-latest',
      'timeout-minutes': 10,
      permissions: { actions: 'read', contents: 'write', 'pull-requests': 'write' },
      outputs: {
        pr_number: '${{ steps.finalize.outputs.pr_number }}',
        head_sha: '${{ steps.finalize.outputs.head_sha }}',
        parent_sha: '${{ steps.finalize.outputs.parent_sha }}',
      },
      steps: [
        {
          uses: 'actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0',
          with: { 'fetch-depth': 0, 'persist-credentials': false },
        },
        {
          uses: 'actions/setup-node@48b55a011bda9f5d6aeb4c2d9c7362e8dae4041e',
          with: { 'node-version': '24.16.0' },
        },
        {
          name: 'Snapshot protected release state',
          id: 'snapshot',
          env: { GITHUB_TOKEN: '${{ secrets.GITHUB_TOKEN }}' },
          run: 'node scripts/console/release-please-pr-fallback.mjs snapshot',
        },
        {
          uses: 'googleapis/release-please-action@8b8fd2cc23b2e18957157a9d923d75aa0c6f6ad5',
          id: 'release',
          'continue-on-error': true,
          with: {
            token: '${{ secrets.GITHUB_TOKEN }}',
            'config-file': 'release-please-config.json',
            'manifest-file': '.release-please-manifest.json',
          },
        },
        {
          name: 'Create exact release PR after permission-only action failure',
          id: 'fallback',
          if: "${{ steps.release.outcome == 'failure' && steps.release.outputs.pr == '' }}",
          env: {
            RELEASE_PLEASE_SNAPSHOT: '${{ steps.snapshot.outputs.snapshot }}',
            RELEASE_PLEASE_ACTION_OUTCOME: '${{ steps.release.outcome }}',
            RELEASE_PLEASE_ACTION_PR: '${{ steps.release.outputs.pr }}',
            RELEASE_PLEASE_ACTION_PRS: '${{ steps.release.outputs.prs }}',
            RELEASE_PLEASE_ACTION_RELEASE_CREATED: '${{ steps.release.outputs.release_created }}',
            RELEASE_PLEASE_ACTION_RELEASES_CREATED: '${{ steps.release.outputs.releases_created }}',
            CONSOLE_RELEASE_PUSH_TOKEN: '${{ secrets.RELEASE_PLEASE_TOKEN }}',
            GITHUB_TOKEN: '${{ secrets.GITHUB_TOKEN }}',
          },
          run: 'node scripts/console/release-please-pr-fallback.mjs fallback',
        },
        {
          name: 'Select one exact Release Please result',
          id: 'select',
          if: '${{ always() }}',
          env: {
            RELEASE_PLEASE_SNAPSHOT_OUTCOME: '${{ steps.snapshot.outcome }}',
            RELEASE_PLEASE_ACTION_OUTCOME: '${{ steps.release.outcome }}',
            RELEASE_PLEASE_ACTION_PR: '${{ steps.release.outputs.pr }}',
            RELEASE_PLEASE_ACTION_RELEASE_CREATED: '${{ steps.release.outputs.release_created }}',
            RELEASE_PLEASE_ACTION_RELEASES_CREATED: '${{ steps.release.outputs.releases_created }}',
            RELEASE_PLEASE_FALLBACK_OUTCOME: '${{ steps.fallback.outcome }}',
            RELEASE_PLEASE_FALLBACK_PR: '${{ steps.fallback.outputs.pr }}',
          },
          run: 'node scripts/console/release-please-pr-fallback.mjs select',
        },
        {
          name: 'Converge documentation custody on release-please tip',
          id: 'converge',
          if: "${{ steps.select.outcome == 'success' && steps.select.outputs.pr != '' }}",
          env: {
            RELEASE_PLEASE_PR: '${{ steps.select.outputs.pr }}',
            CONSOLE_RELEASE_PUSH_TOKEN: '${{ secrets.RELEASE_PLEASE_TOKEN }}',
            GH_TOKEN: '${{ secrets.GITHUB_TOKEN }}',
            GITHUB_TOKEN: '${{ secrets.GITHUB_TOKEN }}',
          },
          run: 'node scripts/console/converge-release-please-doc-custody.mjs',
        },
        {
          name: 'Adjudicate the complete protected release result',
          id: 'finalize',
          if: '${{ always() }}',
          env: {
            RELEASE_PLEASE_SELECTION_OUTCOME: '${{ steps.select.outcome }}',
            RELEASE_PLEASE_SELECTION_MODE: '${{ steps.select.outputs.mode }}',
            RELEASE_PLEASE_SELECTED_PR: '${{ steps.select.outputs.pr }}',
            RELEASE_PLEASE_CONVERGE_OUTCOME: '${{ steps.converge.outcome }}',
            RELEASE_PLEASE_CONVERGE_PR_NUMBER: '${{ steps.converge.outputs.pr_number }}',
            RELEASE_PLEASE_CONVERGE_HEAD_SHA: '${{ steps.converge.outputs.head_sha }}',
            RELEASE_PLEASE_CONVERGE_PARENT_SHA: '${{ steps.converge.outputs.parent_sha }}',
          },
          run: 'node scripts/console/release-please-pr-fallback.mjs finalize',
        },
      ],
    },
    'release-authority-proof': {
      needs: 'release-please',
      name: 'release-authority-proof pr=${{ needs.release-please.outputs.pr_number }} head=${{ needs.release-please.outputs.head_sha }}',
      if: "${{ needs.release-please.outputs.pr_number != '' && needs.release-please.outputs.head_sha != '' && needs.release-please.outputs.parent_sha != '' }}",
      'runs-on': 'ubuntu-latest',
      'timeout-minutes': 2,
      permissions: {},
      steps: [
        {
          name: 'Assert protected release proof coordinates',
          env: {
            PR_NUMBER: '${{ needs.release-please.outputs.pr_number }}',
            HEAD_SHA: '${{ needs.release-please.outputs.head_sha }}',
            PARENT_SHA: '${{ needs.release-please.outputs.parent_sha }}',
          },
          run: [
            'set -eu',
            'test "$PARENT_SHA" = "$GITHUB_SHA"',
            'test "$HEAD_SHA" != "$PARENT_SHA"',
            'test "${#HEAD_SHA}" -eq 40',
            'test "$PR_NUMBER" -gt 0',
            'case "$HEAD_SHA" in',
            '  *[!0-9a-f]*) exit 1 ;;',
            'esac',
            '',
          ].join('\n'),
        },
      ],
    },
  },
});

test('protected release workflow keeps the PAT transport-only and emits one native proof job', () => {
  const source = readFileSync(
    new URL('../../.github/workflows/release-please.yml', import.meta.url),
    'utf8',
  );
  const model = yaml.load(source);
  assert.deepEqual(model, expectedReleaseWorkflow);
  assert.equal(model.jobs['release-please']['timeout-minutes'], 10);
  const releaseConfig = JSON.parse(readFileSync(
    new URL('../../release-please-config.json', import.meta.url),
    'utf8',
  ));
  assert.equal(releaseConfig['always-update'], true);
  assert.deepEqual(model.permissions, {});

  const producer = model.jobs['release-please'];
  assert.deepEqual(producer.permissions, { actions: 'read', contents: 'write', 'pull-requests': 'write' });
  assert.deepEqual(producer.outputs, {
    pr_number: '${{ steps.finalize.outputs.pr_number }}',
    head_sha: '${{ steps.finalize.outputs.head_sha }}',
    parent_sha: '${{ steps.finalize.outputs.parent_sha }}',
  });
  const release = producer.steps.find((step) => step.id === 'release');
  assert.equal(release.with.token, '${{ secrets.GITHUB_TOKEN }}');
  assert.equal(release['continue-on-error'], true);
  assert.equal(Object.hasOwn(release.with, 'bootstrap-sha'), false);
  assert.doesNotMatch(JSON.stringify(release), /RELEASE_PLEASE_TOKEN/);
  assert.deepEqual(
    producer.steps.filter((step) => Object.hasOwn(step, 'continue-on-error')).map((step) => step.id),
    ['release'],
  );

  const checkout = producer.steps.find((step) => String(step.uses ?? '').startsWith('actions/checkout@'));
  assert.equal(Object.hasOwn(checkout, 'if'), false);
  assert.equal(checkout.with['persist-credentials'], false);
  assert.equal(Object.hasOwn(checkout.with, 'token'), false);
  const setup = producer.steps.find((step) => String(step.uses ?? '').startsWith('actions/setup-node@'));
  assert.equal(Object.hasOwn(setup, 'if'), false);

  const snapshot = producer.steps.find((step) => step.id === 'snapshot');
  assert.deepEqual(snapshot.env, { GITHUB_TOKEN: '${{ secrets.GITHUB_TOKEN }}' });
  const fallback = producer.steps.find((step) => step.id === 'fallback');
  assert.equal(fallback.if, "${{ steps.release.outcome == 'failure' && steps.release.outputs.pr == '' }}");
  assert.equal(fallback.env.CONSOLE_RELEASE_PUSH_TOKEN, '${{ secrets.RELEASE_PLEASE_TOKEN }}');
  const select = producer.steps.find((step) => step.id === 'select');
  assert.equal(select.if, '${{ always() }}');

  const converge = producer.steps.find((step) => step.id === 'converge');
  assert.equal(converge.if, "${{ steps.select.outcome == 'success' && steps.select.outputs.pr != '' }}");
  assert.deepEqual(converge.env, {
    RELEASE_PLEASE_PR: '${{ steps.select.outputs.pr }}',
    CONSOLE_RELEASE_PUSH_TOKEN: '${{ secrets.RELEASE_PLEASE_TOKEN }}',
    GH_TOKEN: '${{ secrets.GITHUB_TOKEN }}',
    GITHUB_TOKEN: '${{ secrets.GITHUB_TOKEN }}',
  });
  const finalize = producer.steps.find((step) => step.id === 'finalize');
  assert.equal(finalize.if, '${{ always() }}');
  for (const step of producer.steps) {
    if (!['fallback', 'converge'].includes(step.id)) {
      assert.doesNotMatch(JSON.stringify(step), /RELEASE_PLEASE_TOKEN/);
    }
  }

  const proof = model.jobs['release-authority-proof'];
  assert.equal(proof.needs, 'release-please');
  assert.equal(
    proof.name,
    'release-authority-proof pr=${{ needs.release-please.outputs.pr_number }} head=${{ needs.release-please.outputs.head_sha }}',
  );
  assert.equal(
    proof.if,
    "${{ needs.release-please.outputs.pr_number != '' && needs.release-please.outputs.head_sha != '' && needs.release-please.outputs.parent_sha != '' }}",
  );
  assert.deepEqual(proof.permissions, {});
  assert.equal(proof.steps.length, 1);
  assert.deepEqual(proof.steps[0].env, {
    PR_NUMBER: '${{ needs.release-please.outputs.pr_number }}',
    HEAD_SHA: '${{ needs.release-please.outputs.head_sha }}',
    PARENT_SHA: '${{ needs.release-please.outputs.parent_sha }}',
  });
  assert.match(proof.steps[0].run, /test "\$PARENT_SHA" = "\$GITHUB_SHA"/);
  assert.doesNotMatch(source, /statuses:\s*write|checks:\s*write|upload-artifact|createCommitStatus|external_id/);

  for (const mutation of [
    source.replace(
      '      - name: Converge documentation custody on release-please tip',
      '      - run: echo "$CONSOLE_RELEASE_PUSH_TOKEN"\n      - name: Converge documentation custody on release-please tip',
    ),
    source.replace(
      '          token: ${{ secrets.GITHUB_TOKEN }}',
      '          token: ${{ secrets.RELEASE_PLEASE_TOKEN }}',
    ),
    `${source}\n  attacker-job:\n    runs-on: ubuntu-latest\n    steps: []\n`,
  ]) {
    assert.notEqual(mutation, source);
    assert.notDeepEqual(yaml.load(mutation), expectedReleaseWorkflow);
  }
});

test('protected release authority issuer executable closure is review-pinned', () => {
  for (const [relativePath, expectedDigest] of protectedReleaseIssuerClosure) {
    const source = readFileSync(new URL(relativePath, import.meta.url));
    assert.equal(sha256(source), expectedDigest, relativePath);
    assert.notEqual(
      sha256(Buffer.concat([source, Buffer.from('\n// unreviewed issuer mutation\n')])),
      expectedDigest,
      `${relativePath} must reject any source mutation`,
    );
  }
});
