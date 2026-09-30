import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  FALLBACK_HEAD_REF,
  FALLBACK_REPOSITORY,
  FALLBACK_REPOSITORY_ID,
  FALLBACK_WEB_FLOW_ID,
  FALLBACK_WEB_FLOW_LOGIN,
  FALLBACK_WEB_FLOW_TYPE,
  FALLBACK_WORKFLOW_ID,
  FALLBACK_WORKFLOW_NAME,
  FALLBACK_WORKFLOW_PATH,
  createReleasePleaseFallbackPr,
  decodeReleasePleaseSnapshot,
  encodeReleasePleaseSnapshot,
  finalizeReleasePleaseResult,
  githubApiRequest,
  selectReleasePleaseResult,
  snapshotReleasePleaseState,
} from './release-please-pr-fallback.mjs';
import {
  RELEASE_PLEASE_BOT_EMAIL,
  RELEASE_PLEASE_BOT_ID,
  RELEASE_PLEASE_BOT_NAME,
  RELEASE_PLEASE_COMMITTER_EMAIL,
  RELEASE_PLEASE_COMMITTER_NAME,
  RELEASE_PLEASE_TRANSPORT_ID,
  RELEASE_PLEASE_TRANSPORT_NAME,
  RELEASE_PLEASE_TRANSPORT_TYPE,
} from './release-please-bot-candidate.mjs';
import { RELEASE_PLEASE_PENDING_LABEL } from './release-please-pr-envelope.mjs';

const BASE = 'a'.repeat(40);
const OLD_TIP = 'b'.repeat(40);
const TIP = 'c'.repeat(40);
const BASE_TREE = 'd'.repeat(40);
const HEAD_TREE = 'e'.repeat(40);
const BASE_MANIFEST_BLOB = '1'.repeat(40);
const HEAD_MANIFEST_BLOB = '2'.repeat(40);
const BASE_CHANGELOG_BLOB = '3'.repeat(40);
const HEAD_CHANGELOG_BLOB = '4'.repeat(40);
const WORKFLOW_TOKEN = 'workflow-token';
const TRANSPORT_TOKEN = 'transport-token';
const RUN_ID = 40000000001;
const RUN_NUMBER = 901;
const RUN_ATTEMPT = 1;
const PR_NUMBER = 865;
const FALLBACK_CLI = fileURLToPath(new URL('./release-please-pr-fallback.mjs', import.meta.url));
const NOTES = [
  '## [0.3.9](https://github.com/oyatie/console/compare/v0.3.8...v0.3.9) (2026-08-24)',
  '',
  '',
  '### Bug Fixes',
  '',
  '* **release:** exact permission fallback',
].join('\n');
const PRIOR = [
  '## [0.3.8](https://github.com/oyatie/console/compare/v0.3.7...v0.3.8) (2026-08-19)',
  '',
  '* prior release',
  '',
].join('\n');
const BASE_CHANGELOG = Buffer.from(`# Changelog\n\n${PRIOR}`);
const HEAD_CHANGELOG = Buffer.from(`# Changelog\n\n${NOTES}\n\n${PRIOR}`);
const BASE_MANIFEST = Buffer.from('{\n  ".": "0.3.8"\n}\n');
const HEAD_MANIFEST = Buffer.from('{\n  ".": "0.3.9"\n}\n');

const environment = (overrides = {}) => ({
  GITHUB_REPOSITORY: FALLBACK_REPOSITORY,
  GITHUB_REPOSITORY_ID: String(FALLBACK_REPOSITORY_ID),
  GITHUB_EVENT_NAME: 'push',
  GITHUB_REF: 'refs/heads/dev',
  GITHUB_SHA: BASE,
  GITHUB_WORKFLOW: FALLBACK_WORKFLOW_NAME,
  GITHUB_WORKFLOW_REF: `${FALLBACK_REPOSITORY}/${FALLBACK_WORKFLOW_PATH}@refs/heads/dev`,
  GITHUB_WORKFLOW_SHA: BASE,
  GITHUB_RUN_ID: String(RUN_ID),
  GITHUB_RUN_NUMBER: String(RUN_NUMBER),
  GITHUB_RUN_ATTEMPT: String(RUN_ATTEMPT),
  GITHUB_TOKEN: WORKFLOW_TOKEN,
  CONSOLE_RELEASE_PUSH_TOKEN: TRANSPORT_TOKEN,
  RELEASE_PLEASE_ACTION_OUTCOME: 'failure',
  RELEASE_PLEASE_ACTION_PR: '',
  RELEASE_PLEASE_ACTION_PRS: '',
  RELEASE_PLEASE_ACTION_RELEASE_CREATED: '',
  RELEASE_PLEASE_ACTION_RELEASES_CREATED: '',
  ...overrides,
});

function runFinalizeCli(overrides = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'console-release-proof-output-'));
  const output = join(directory, 'github-output');
  try {
    const result = spawnSync(process.execPath, [FALLBACK_CLI, 'finalize'], {
      encoding: 'utf8',
      env: {
        GITHUB_OUTPUT: output,
        GITHUB_SHA: BASE,
        RELEASE_PLEASE_SELECTION_OUTCOME: 'success',
        RELEASE_PLEASE_SELECTION_MODE: 'fallback-pr',
        RELEASE_PLEASE_SELECTED_PR: `{"number":${PR_NUMBER}}`,
        RELEASE_PLEASE_CONVERGE_OUTCOME: 'success',
        RELEASE_PLEASE_CONVERGE_PR_NUMBER: String(PR_NUMBER),
        RELEASE_PLEASE_CONVERGE_HEAD_SHA: TIP,
        RELEASE_PLEASE_CONVERGE_PARENT_SHA: BASE,
        ...overrides,
      },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, '');
    assert.equal(result.stderr, '');
    return readFileSync(output, 'utf8');
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

const activeRun = (overrides = {}) => ({
  id: RUN_ID,
  workflow_id: FALLBACK_WORKFLOW_ID,
  path: FALLBACK_WORKFLOW_PATH,
  event: 'push',
  head_branch: 'dev',
  head_sha: BASE,
  run_number: RUN_NUMBER,
  run_attempt: RUN_ATTEMPT,
  status: 'in_progress',
  conclusion: null,
  repository: { id: FALLBACK_REPOSITORY_ID, full_name: FALLBACK_REPOSITORY },
  ...overrides,
});

const tree = (sha, manifestSha, changelogSha, overrides = {}) => ({
  sha,
  truncated: false,
  tree: [
    { path: '.release-please-manifest.json', mode: '100644', type: 'blob', sha: manifestSha },
    { path: 'CHANGELOG.md', mode: '100644', type: 'blob', sha: changelogSha },
    { path: 'README.md', mode: '100644', type: 'blob', sha: '5'.repeat(40) },
  ],
  ...overrides,
});

const blob = (sha, bytes) => ({
  sha,
  size: bytes.length,
  encoding: 'base64',
  content: bytes.toString('base64'),
});

function makeApi() {
  const state = {
    mainTip: BASE,
    releaseTip: OLD_TIP,
    releaseRefExists: true,
    matchingReleaseRefs: null,
    created: false,
    labeled: false,
    patPosts: 0,
    calls: [],
    extraActive: [],
    preexistingPulls: [],
    principal: {
      id: RELEASE_PLEASE_TRANSPORT_ID,
      login: RELEASE_PLEASE_TRANSPORT_NAME,
      type: RELEASE_PLEASE_TRANSPORT_TYPE,
    },
    baseCommit: {
      sha: BASE,
      commit: { tree: { sha: BASE_TREE }, verification: { verified: true, reason: 'valid' } },
    },
    headCommit: {
      sha: TIP,
      parents: [{ sha: BASE }],
      author: { id: RELEASE_PLEASE_BOT_ID, login: RELEASE_PLEASE_BOT_NAME, type: 'Bot' },
      committer: { id: FALLBACK_WEB_FLOW_ID, login: FALLBACK_WEB_FLOW_LOGIN, type: FALLBACK_WEB_FLOW_TYPE },
      commit: {
        message: 'chore(dev): release 0.3.9',
        tree: { sha: HEAD_TREE },
        author: { name: RELEASE_PLEASE_BOT_NAME, email: RELEASE_PLEASE_BOT_EMAIL },
        committer: { name: RELEASE_PLEASE_COMMITTER_NAME, email: RELEASE_PLEASE_COMMITTER_EMAIL },
        verification: { verified: true, reason: 'valid' },
      },
      files: [
        { filename: '.release-please-manifest.json', status: 'modified' },
        { filename: 'CHANGELOG.md', status: 'modified' },
      ],
    },
    baseTree: tree(BASE_TREE, BASE_MANIFEST_BLOB, BASE_CHANGELOG_BLOB),
    headTree: tree(HEAD_TREE, HEAD_MANIFEST_BLOB, HEAD_CHANGELOG_BLOB),
    blobs: new Map([
      [BASE_MANIFEST_BLOB, blob(BASE_MANIFEST_BLOB, BASE_MANIFEST)],
      [HEAD_MANIFEST_BLOB, blob(HEAD_MANIFEST_BLOB, HEAD_MANIFEST)],
      [BASE_CHANGELOG_BLOB, blob(BASE_CHANGELOG_BLOB, BASE_CHANGELOG)],
      [HEAD_CHANGELOG_BLOB, blob(HEAD_CHANGELOG_BLOB, HEAD_CHANGELOG)],
    ]),
    labelResponse: [{ name: RELEASE_PLEASE_PENDING_LABEL }],
    postError: null,
  };

  const pull = () => ({
    number: PR_NUMBER,
    state: 'open',
    draft: false,
    title: 'chore(dev): release 0.3.9',
    body: `:robot: I have created a release *beep* *boop*\n---\n\n\n${NOTES}\n\n---\nThis PR was generated with [Release Please](https://github.com/googleapis/release-please). See [documentation](https://github.com/googleapis/release-please#release-please).`,
    user: {
      id: RELEASE_PLEASE_TRANSPORT_ID,
      login: RELEASE_PLEASE_TRANSPORT_NAME,
      type: RELEASE_PLEASE_TRANSPORT_TYPE,
    },
    labels: state.labeled ? [{ name: RELEASE_PLEASE_PENDING_LABEL }] : [],
    head: {
      ref: FALLBACK_HEAD_REF,
      sha: state.releaseTip,
      repo: { id: FALLBACK_REPOSITORY_ID, full_name: FALLBACK_REPOSITORY },
    },
    base: {
      ref: 'dev',
      sha: state.mainTip,
      repo: { id: FALLBACK_REPOSITORY_ID, full_name: FALLBACK_REPOSITORY },
    },
  });

  const request = async (args) => {
    state.calls.push(structuredClone(args));
    const { endpoint, method = 'GET', token } = args;
    if (endpoint === `/repos/${FALLBACK_REPOSITORY}/actions/runs/${RUN_ID}`) return activeRun();
    if (endpoint.startsWith(`/repos/${FALLBACK_REPOSITORY}/actions/workflows/${FALLBACK_WORKFLOW_ID}/runs?`)) {
      const status = new URL(`https://api.github.com${endpoint}`).searchParams.get('status');
      const runs = status === 'in_progress' ? [activeRun(), ...state.extraActive] : [];
      return { total_count: runs.length, workflow_runs: runs };
    }
    if (endpoint === `/repos/${FALLBACK_REPOSITORY}/git/ref/heads/dev`) {
      return { ref: 'refs/heads/dev', object: { type: 'commit', sha: state.mainTip } };
    }
    if (endpoint === `/repos/${FALLBACK_REPOSITORY}/git/matching-refs/heads/${encodeURIComponent(FALLBACK_HEAD_REF)}`) {
      if (state.matchingReleaseRefs !== null) return structuredClone(state.matchingReleaseRefs);
      if (!state.releaseRefExists) return [];
      return [{
        ref: `refs/heads/${FALLBACK_HEAD_REF}`,
        object: { type: 'commit', sha: state.releaseTip },
      }];
    }
    if (endpoint === `/repos/${FALLBACK_REPOSITORY}/git/ref/heads/${encodeURIComponent(FALLBACK_HEAD_REF)}`) {
      assert.equal(state.releaseRefExists, true);
      return { ref: `refs/heads/${FALLBACK_HEAD_REF}`, object: { type: 'commit', sha: state.releaseTip } };
    }
    if (endpoint.startsWith(`/repos/${FALLBACK_REPOSITORY}/pulls?`)) {
      if (state.created) return [pull()];
      return state.preexistingPulls;
    }
    if (endpoint === `/repos/${FALLBACK_REPOSITORY}/commits/${BASE}`) return structuredClone(state.baseCommit);
    if (endpoint === `/repos/${FALLBACK_REPOSITORY}/commits/${TIP}`) return structuredClone(state.headCommit);
    if (endpoint === `/repos/${FALLBACK_REPOSITORY}/git/trees/${BASE_TREE}`) return structuredClone(state.baseTree);
    if (endpoint === `/repos/${FALLBACK_REPOSITORY}/git/trees/${HEAD_TREE}`) return structuredClone(state.headTree);
    if (endpoint.startsWith(`/repos/${FALLBACK_REPOSITORY}/git/blobs/`)) {
      return structuredClone(state.blobs.get(endpoint.split('/').at(-1)));
    }
    if (endpoint === '/user') {
      assert.equal(token, TRANSPORT_TOKEN);
      return structuredClone(state.principal);
    }
    if (endpoint === `/repos/${FALLBACK_REPOSITORY}/pulls` && method === 'POST') {
      assert.equal(token, TRANSPORT_TOKEN);
      state.patPosts += 1;
      if (state.postError) throw state.postError;
      state.created = true;
      return pull();
    }
    if (endpoint === `/repos/${FALLBACK_REPOSITORY}/issues/${PR_NUMBER}/labels` && method === 'POST') {
      assert.equal(token, WORKFLOW_TOKEN);
      state.labeled = true;
      return structuredClone(state.labelResponse);
    }
    if (endpoint === `/repos/${FALLBACK_REPOSITORY}/pulls/${PR_NUMBER}`) return pull();
    throw new Error(`unexpected request ${method} ${endpoint}`);
  };
  return { state, request };
}

async function preparedFallback() {
  const api = makeApi();
  const snapshot = await snapshotReleasePleaseState({ environment: environment(), request: api.request });
  api.state.releaseTip = TIP;
  const env = environment({ RELEASE_PLEASE_SNAPSHOT: encodeReleasePleaseSnapshot(snapshot) });
  return { api, snapshot, env };
}

test('snapshots exact protected coordinates, old ref, active run, and empty PR state', async () => {
  const api = makeApi();
  const snapshot = await snapshotReleasePleaseState({ environment: environment(), request: api.request });
  assert.deepEqual(snapshot, {
    version: 1,
    repository: FALLBACK_REPOSITORY,
    repositoryId: FALLBACK_REPOSITORY_ID,
    sha: BASE,
    runId: RUN_ID,
    runNumber: RUN_NUMBER,
    runAttempt: RUN_ATTEMPT,
    releaseTip: OLD_TIP,
    openPullNumbers: [],
  });
  assert.deepEqual(decodeReleasePleaseSnapshot(encodeReleasePleaseSnapshot(snapshot)), snapshot);
  assert.throws(() => decodeReleasePleaseSnapshot(`${encodeReleasePleaseSnapshot(snapshot)}=`));
});

test('snapshots a deleted release ref as null and admits one newly created exact ref', async () => {
  const api = makeApi();
  api.state.releaseRefExists = false;
  const snapshot = await snapshotReleasePleaseState({ environment: environment(), request: api.request });
  assert.equal(snapshot.releaseTip, null);
  api.state.releaseRefExists = true;
  api.state.releaseTip = TIP;
  const result = await createReleasePleaseFallbackPr({
    environment: environment({ RELEASE_PLEASE_SNAPSHOT: encodeReleasePleaseSnapshot(snapshot) }),
    request: api.request,
  });
  assert.equal(result.headSha, TIP);
  assert.equal(api.state.patPosts, 1);
});

test('snapshot rejects prefix or ambiguous matches for the exact release ref', async () => {
  for (const matchingReleaseRefs of [
    [{ ref: `refs/heads/${FALLBACK_HEAD_REF}-attacker`, object: { type: 'commit', sha: OLD_TIP } }],
    [
      { ref: `refs/heads/${FALLBACK_HEAD_REF}`, object: { type: 'commit', sha: OLD_TIP } },
      { ref: `refs/heads/${FALLBACK_HEAD_REF}-attacker`, object: { type: 'commit', sha: TIP } },
    ],
  ]) {
    const api = makeApi();
    api.state.matchingReleaseRefs = matchingReleaseRefs;
    await assert.rejects(
      () => snapshotReleasePleaseState({ environment: environment(), request: api.request }),
      /matching ref inventory/,
    );
  }
});

test('rejects wrong repository, workflow, run, attempt, event, main, or competing active run at snapshot', async () => {
  for (const changed of [
    { GITHUB_REPOSITORY: 'attacker/console' },
    { GITHUB_REPOSITORY_ID: '1' },
    { GITHUB_EVENT_NAME: 'workflow_dispatch' },
    { GITHUB_REF: 'refs/heads/main' },
    { GITHUB_WORKFLOW: 'CI' },
    { GITHUB_WORKFLOW_SHA: TIP },
    { GITHUB_RUN_ATTEMPT: '0' },
  ]) {
    const api = makeApi();
    await assert.rejects(() => snapshotReleasePleaseState({ environment: environment(changed), request: api.request }));
  }
  const api = makeApi();
  api.state.extraActive.push(activeRun({ id: RUN_ID + 1, run_number: RUN_NUMBER + 1 }));
  await assert.rejects(
    () => snapshotReleasePleaseState({ environment: environment(), request: api.request }),
    /competes/,
  );
});

test('creates one exact PR with the pinned PAT, labels/readbacks with GITHUB_TOKEN, and never duplicates POST', async () => {
  const { api, env } = await preparedFallback();
  const result = await createReleasePleaseFallbackPr({ environment: env, request: api.request });
  assert.equal(result.prNumber, PR_NUMBER);
  assert.equal(result.headSha, TIP);
  assert.equal(result.parentSha, BASE);
  const pr = JSON.parse(result.pr);
  assert.equal(pr.number, PR_NUMBER);
  assert.deepEqual(pr.labels, [RELEASE_PLEASE_PENDING_LABEL]);
  assert.equal(api.state.patPosts, 1);
  const patCalls = api.state.calls.filter((call) => call.token === TRANSPORT_TOKEN);
  assert.deepEqual(patCalls.map(({ method, endpoint }) => [method, endpoint]), [
    ['GET', '/user'],
    ['POST', `/repos/${FALLBACK_REPOSITORY}/pulls`],
  ]);
  assert.equal(JSON.stringify(patCalls.map(({ endpoint, body }) => ({ endpoint, body }))).includes(TRANSPORT_TOKEN), false);
});

test('fallback requires exact action failure, no action output, no prior/competing PR, and an advanced ref', async () => {
  for (const changed of [
    { RELEASE_PLEASE_ACTION_OUTCOME: 'success' },
    { RELEASE_PLEASE_ACTION_PR: '{}' },
    { RELEASE_PLEASE_ACTION_RELEASE_CREATED: 'true' },
  ]) {
    const { api, env } = await preparedFallback();
    await assert.rejects(() => createReleasePleaseFallbackPr({
      environment: { ...env, ...changed }, request: api.request,
    }));
    assert.equal(api.state.patPosts, 0);
  }
  {
    const api = makeApi();
    api.state.preexistingPulls = [{ number: 700 }];
    const snapshot = await snapshotReleasePleaseState({ environment: environment(), request: api.request });
    api.state.releaseTip = TIP;
    await assert.rejects(() => createReleasePleaseFallbackPr({
      environment: environment({ RELEASE_PLEASE_SNAPSHOT: encodeReleasePleaseSnapshot(snapshot) }),
      request: api.request,
    }), /existed/);
  }
  {
    const { api, snapshot, env } = await preparedFallback();
    api.state.releaseTip = snapshot.releaseTip;
    await assert.rejects(() => createReleasePleaseFallbackPr({ environment: env, request: api.request }), /did not advance/);
  }
  {
    const { api, env } = await preparedFallback();
    api.state.preexistingPulls = [{ number: 701 }];
    await assert.rejects(() => createReleasePleaseFallbackPr({ environment: env, request: api.request }), /competing/);
  }
});

test('rejects main/ref/run races before or after the sole PAT mutation', async () => {
  {
    const { api, env } = await preparedFallback();
    api.state.mainTip = TIP;
    await assert.rejects(() => createReleasePleaseFallbackPr({ environment: env, request: api.request }), /dev moved/);
    assert.equal(api.state.patPosts, 0);
  }
  {
    const { api, env } = await preparedFallback();
    api.state.extraActive.push(activeRun({ id: RUN_ID + 2, run_number: RUN_NUMBER + 1 }));
    await assert.rejects(() => createReleasePleaseFallbackPr({ environment: env, request: api.request }), /competes/);
    assert.equal(api.state.patPosts, 0);
  }
});

test('rejects wrong parent, signature, bot/GitHub identity, path, mode, or type before PAT use', async () => {
  const mutators = [
    (state) => { state.headCommit.parents = [{ sha: OLD_TIP }]; },
    (state) => { state.headCommit.commit.verification.verified = false; },
    (state) => { state.headCommit.author.id += 1; },
    (state) => { state.headCommit.committer.type = 'Bot'; },
    (state) => { state.headCommit.commit.committer.email = 'attacker@example.invalid'; },
    (state) => { state.headCommit.files.push({ filename: 'README.md', status: 'modified' }); },
    (state) => { state.headTree.tree[0].mode = '120000'; },
    (state) => { state.headTree.tree[1].type = 'tree'; },
  ];
  for (const mutate of mutators) {
    const { api, env } = await preparedFallback();
    mutate(api.state);
    await assert.rejects(() => createReleasePleaseFallbackPr({ environment: env, request: api.request }));
    assert.equal(api.state.patPosts, 0);
  }
});

test('rejects semantic version, changelog body, blob encoding, and tree truncation drift', async () => {
  const mutators = [
    (state) => {
      state.blobs.set(HEAD_MANIFEST_BLOB, blob(HEAD_MANIFEST_BLOB, BASE_MANIFEST));
    },
    (state) => {
      state.blobs.set(HEAD_CHANGELOG_BLOB, blob(HEAD_CHANGELOG_BLOB, Buffer.from('# Changelog\n\ntruncated')));
    },
    (state) => { state.blobs.get(HEAD_CHANGELOG_BLOB).content = '%%%'; },
    (state) => { state.headTree.truncated = true; },
  ];
  for (const mutate of mutators) {
    const { api, env } = await preparedFallback();
    mutate(api.state);
    await assert.rejects(() => createReleasePleaseFallbackPr({ environment: env, request: api.request }));
    assert.equal(api.state.patPosts, 0);
  }
});

test('wrong PAT principal and ambiguous POST failure stop without a retry', async () => {
  {
    const { api, env } = await preparedFallback();
    api.state.principal.id += 1;
    await assert.rejects(() => createReleasePleaseFallbackPr({ environment: env, request: api.request }), /principal/);
    assert.equal(api.state.patPosts, 0);
  }
  {
    const { api, env } = await preparedFallback();
    api.state.postError = new Error('timeout with unknown server state');
    await assert.rejects(() => createReleasePleaseFallbackPr({ environment: env, request: api.request }));
    assert.equal(api.state.patPosts, 1);
  }
});

test('HTTP adapter pins origin, redirect policy, token header, and never retries or leaks response bodies', async () => {
  const calls = [];
  const data = await githubApiRequest({
    token: TRANSPORT_TOKEN,
    method: 'POST',
    endpoint: `/repos/${FALLBACK_REPOSITORY}/pulls`,
    expectedStatus: 201,
    body: { title: 'safe' },
    fetchImpl: async (url, options) => {
      calls.push({ url, options });
      return { status: 201, text: async () => '{"number":865}' };
    },
  });
  assert.deepEqual(data, { number: 865 });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, `https://api.github.com/repos/${FALLBACK_REPOSITORY}/pulls`);
  assert.equal(calls[0].options.redirect, 'error');
  assert.equal(calls[0].options.headers.Authorization, `Bearer ${TRANSPORT_TOKEN}`);
  assert.equal(calls[0].url.includes(TRANSPORT_TOKEN), false);
  assert.equal(calls[0].options.body.includes(TRANSPORT_TOKEN), false);

  for (const fetchImpl of [
    async () => ({ status: 500, text: async () => `secret ${TRANSPORT_TOKEN}` }),
    async () => ({ status: 201, text: async () => 'not-json' }),
    async () => { throw new Error(`transport ${TRANSPORT_TOKEN}`); },
  ]) {
    let count = 0;
    await assert.rejects(
      () => githubApiRequest({
        token: TRANSPORT_TOKEN,
        method: 'POST',
        endpoint: `/repos/${FALLBACK_REPOSITORY}/pulls`,
        expectedStatus: 201,
        body: { title: 'safe' },
        fetchImpl: async (...args) => { count += 1; return fetchImpl(...args); },
      }),
      (error) => !String(error).includes(TRANSPORT_TOKEN),
    );
    assert.equal(count, 1);
  }
});

test('selector accepts native PR, recurring PR, release, no-op, or exact fallback and rejects unhandled failure', () => {
  const base = {
    snapshotOutcome: 'success',
    releaseOutcome: 'success',
    releasePr: '{"number":865}',
    releaseCreated: '',
    releasesCreated: '',
    fallbackOutcome: 'skipped',
    fallbackPr: '',
  };
  assert.deepEqual(selectReleasePleaseResult(base), { mode: 'pr', pr: base.releasePr });
  assert.deepEqual(selectReleasePleaseResult({ ...base, releasePr: '', releaseCreated: 'true' }), {
    mode: 'release', pr: '',
  });
  assert.deepEqual(selectReleasePleaseResult({ ...base, releasePr: '' }), { mode: 'noop', pr: '' });
  assert.deepEqual(selectReleasePleaseResult({
    ...base,
    releaseOutcome: 'failure',
    releasePr: '',
    fallbackOutcome: 'success',
    fallbackPr: '{"number":866}',
  }), { mode: 'fallback-pr', pr: '{"number":866}' });
  for (const mutation of [
    { ...base, snapshotOutcome: 'failure' },
    { ...base, releaseOutcome: 'failure', releasePr: '', fallbackOutcome: 'skipped' },
    { ...base, releaseOutcome: 'cancelled' },
    { ...base, releaseCreated: 'true' },
    { ...base, fallbackOutcome: 'success', fallbackPr: '{}' },
  ]) assert.throws(() => selectReleasePleaseResult(mutation));
});

test('final adjudicator requires convergence for every PR and exact parent-bound proof outputs', () => {
  const prResult = {
    selectionOutcome: 'success',
    mode: 'fallback-pr',
    selectedPr: '{"number":865}',
    convergeOutcome: 'success',
    prNumber: String(PR_NUMBER),
    headSha: TIP,
    parentSha: BASE,
    expectedParentSha: BASE,
  };
  assert.deepEqual(finalizeReleasePleaseResult(prResult), {
    prNumber: PR_NUMBER,
    headSha: TIP,
    parentSha: BASE,
  });
  assert.deepEqual(finalizeReleasePleaseResult({
    ...prResult,
    mode: 'noop',
    selectedPr: '',
    convergeOutcome: 'skipped',
    prNumber: '',
    headSha: '',
    parentSha: '',
  }), { prNumber: '', headSha: '', parentSha: '' });
  for (const mutation of [
    { ...prResult, selectionOutcome: 'failure' },
    { ...prResult, convergeOutcome: 'failure' },
    { ...prResult, parentSha: OLD_TIP },
    { ...prResult, headSha: BASE },
    { ...prResult, prNumber: '' },
  ]) assert.throws(() => finalizeReleasePleaseResult(mutation));
});

test('finalize CLI emits the exact snake_case workflow proof-output protocol for PR and no-op results', () => {
  const prOutput = runFinalizeCli();
  assert.equal(prOutput, [
    `pr_number=${PR_NUMBER}`,
    `head_sha=${TIP}`,
    `parent_sha=${BASE}`,
    '',
  ].join('\n'));

  const noOpOutput = runFinalizeCli({
    RELEASE_PLEASE_SELECTION_MODE: 'noop',
    RELEASE_PLEASE_SELECTED_PR: '',
    RELEASE_PLEASE_CONVERGE_OUTCOME: 'skipped',
    RELEASE_PLEASE_CONVERGE_PR_NUMBER: '',
    RELEASE_PLEASE_CONVERGE_HEAD_SHA: '',
    RELEASE_PLEASE_CONVERGE_PARENT_SHA: '',
  });
  assert.equal(noOpOutput, 'pr_number=\nhead_sha=\nparent_sha=\n');

  for (const output of [prOutput, noOpOutput]) {
    assert.doesNotMatch(output, /^(?:prNumber|headSha|parentSha)=/m);
  }
});
