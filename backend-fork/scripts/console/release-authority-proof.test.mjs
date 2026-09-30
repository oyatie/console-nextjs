import assert from 'node:assert/strict';
import test from 'node:test';
import {
  PINNED_RELEASE_REPOSITORY,
  PINNED_RELEASE_REPOSITORY_ID,
  RELEASE_PLEASE_WORKFLOW_ID,
  RELEASE_PLEASE_WORKFLOW_PATH,
  assertReleaseAuthorityProof,
  githubJsonRequest,
  pollReleaseAuthorityProof,
} from './verify-console-pr-authority-bootstrap.mjs';
import {
  RELEASE_PLEASE_BOT_ID,
  RELEASE_PLEASE_BOT_NAME,
  RELEASE_PLEASE_TRANSPORT_ID,
  RELEASE_PLEASE_TRANSPORT_NAME,
} from './release-please-bot-candidate.mjs';

const C = 'c'.repeat(40);
const T = 'a'.repeat(40);
const OTHER = 'b'.repeat(40);
const PR_NUMBER = 760;
const HEAD_REF = 'release-please--branches--dev--components--console';
const proofName = `release-authority-proof pr=${PR_NUMBER} head=${T}`;
const run = Object.freeze({
  id: 31906390000,
  workflow_id: RELEASE_PLEASE_WORKFLOW_ID,
  path: RELEASE_PLEASE_WORKFLOW_PATH,
  event: 'push',
  head_branch: 'dev',
  head_sha: C,
  run_number: 842,
  run_attempt: 2,
  status: 'completed',
  conclusion: 'success',
  repository: { id: PINNED_RELEASE_REPOSITORY_ID, full_name: PINNED_RELEASE_REPOSITORY },
});
const job = Object.freeze({
  id: 95065020000,
  run_id: run.id,
  run_attempt: run.run_attempt,
  workflow_name: 'Release Please',
  head_sha: C,
  name: proofName,
  status: 'completed',
  conclusion: 'success',
});
const exact = (overrides = {}) => ({
  runs: [run],
  jobs: [job],
  repository: PINNED_RELEASE_REPOSITORY,
  prNumber: PR_NUMBER,
  headSha: T,
  parentSha: C,
  ...overrides,
});
const runsResponse = (runs, totalCount = runs.length) => ({
  total_count: totalCount,
  workflow_runs: runs,
});

test('accepts exactly one successful native proof job from the pinned protected run', () => {
  const expected = {
    workflowId: RELEASE_PLEASE_WORKFLOW_ID,
    runId: run.id,
    runNumber: run.run_number,
    runAttempt: run.run_attempt,
    jobId: job.id,
    prNumber: PR_NUMBER,
    headSha: T,
    parentSha: C,
  };
  assert.deepEqual(assertReleaseAuthorityProof(exact()), expected);
});

test('rejects run provenance, state, attempt, or ambiguity drift', () => {
  const mutations = [
    [{ ...run, workflow_id: 1 }, /workflow id/],
    [{ ...run, path: '.github/workflows/ci.yml' }, /workflow path/],
    [{ ...run, event: 'workflow_dispatch' }, /allowed event/],
    [{ ...run, event: 'pull_request' }, /allowed event/],
    [{ ...run, head_branch: 'release-branch' }, /dev branch/],
    [{ ...run, head_sha: OTHER }, /parent SHA/],
    [{ ...run, repository: { ...run.repository, id: 1 } }, /repository id/],
    [{ ...run, repository: { ...run.repository, full_name: 'attacker/console' } }, /repository name/],
    [{ ...run, status: 'in_progress', conclusion: null }, /completed successfully/],
    [{ ...run, conclusion: 'failure' }, /completed successfully/],
    [{ ...run, run_number: 0 }, /run number/],
    [{ ...run, run_attempt: 0 }, /run attempt/],
  ];
  for (const [changed, pattern] of mutations) {
    assert.throws(() => assertReleaseAuthorityProof(exact({ runs: [changed] })), pattern);
  }
  assert.throws(() => assertReleaseAuthorityProof(exact({ runs: [run, { ...run, id: run.id + 1 }] })), /exactly one/);
});

test('rejects forged, failed, stale-attempt, wrong-head, or duplicate proof jobs', () => {
  const mutations = [
    [{ ...job, name: 'authenticate-console-authority' }, /exact proof job/],
    [{ ...job, run_id: run.id + 1 }, /run id/],
    [{ ...job, run_attempt: run.run_attempt - 1 }, /run attempt/],
    [{ ...job, head_sha: OTHER }, /parent SHA/],
    [{ ...job, status: 'in_progress', conclusion: null }, /completed successfully/],
    [{ ...job, conclusion: 'failure' }, /completed successfully/],
    [{ ...job, workflow_name: 'CI' }, /workflow name/],
  ];
  for (const [changed, pattern] of mutations) {
    assert.throws(() => assertReleaseAuthorityProof(exact({ jobs: [changed] })), pattern);
  }
  assert.throws(() => assertReleaseAuthorityProof(exact({ jobs: [job, { ...job, id: job.id + 1 }] })), /exact proof job/);
});

const botCreator = Object.freeze({ id: RELEASE_PLEASE_BOT_ID, login: RELEASE_PLEASE_BOT_NAME });
const transportCreator = Object.freeze({
  id: RELEASE_PLEASE_TRANSPORT_ID,
  login: RELEASE_PLEASE_TRANSPORT_NAME,
});
const livePr = (sha = T, creator = botCreator) => ({
  number: PR_NUMBER,
  state: 'open',
  user: { ...creator },
  head: { sha, ref: HEAD_REF, repo: { full_name: PINNED_RELEASE_REPOSITORY } },
  base: { ref: 'dev' },
});

test('polls boundedly for delayed native proof visibility and rechecks the live PR head', async () => {
  let runReads = 0;
  let sleeps = 0;
  let pullReads = 0;
  const request = async (endpoint) => {
    if (endpoint.includes(`/pulls/${PR_NUMBER}`)) {
      pullReads += 1;
      return livePr();
    }
    if (endpoint.includes('/actions/workflows/')) {
      runReads += 1;
      return runsResponse(runReads === 1 ? [] : [run]);
    }
    if (endpoint.includes(`/actions/runs/${run.id}/attempts/${run.run_attempt}/jobs`)) {
      return { jobs: [job] };
    }
    throw new Error(`unexpected endpoint: ${endpoint}`);
  };
  assert.deepEqual(await pollReleaseAuthorityProof({
    request,
    sleep: async () => { sleeps += 1; },
    repository: PINNED_RELEASE_REPOSITORY,
    prNumber: PR_NUMBER,
    headSha: T,
    parentSha: C,
    headRef: HEAD_REF,
    maxAttempts: 3,
  }), {
    workflowId: RELEASE_PLEASE_WORKFLOW_ID,
    runId: run.id,
    runNumber: run.run_number,
    runAttempt: run.run_attempt,
    jobId: job.id,
    prNumber: PR_NUMBER,
    headSha: T,
    parentSha: C,
  });
  assert.equal(runReads, 3);
  assert.equal(sleeps, 1);
  assert.equal(pullReads, 2);
});

test('accepts the exact pinned transport creator and rejects mixed creator identities', async () => {
  const poll = (creator) => pollReleaseAuthorityProof({
    request: async (endpoint) => {
      if (endpoint.includes(`/pulls/${PR_NUMBER}`)) return livePr(T, creator);
      if (endpoint.includes('/actions/workflows/')) return runsResponse([run]);
      if (endpoint.includes(`/actions/runs/${run.id}/attempts/${run.run_attempt}/jobs`)) {
        return { jobs: [job] };
      }
      throw new Error(`unexpected endpoint: ${endpoint}`);
    },
    sleep: async () => {},
    repository: PINNED_RELEASE_REPOSITORY,
    prNumber: PR_NUMBER,
    headSha: T,
    parentSha: C,
    headRef: HEAD_REF,
    maxAttempts: 1,
  });
  assert.equal((await poll(transportCreator)).headSha, T);
  for (const creator of [
    { id: RELEASE_PLEASE_BOT_ID, login: RELEASE_PLEASE_TRANSPORT_NAME },
    { id: RELEASE_PLEASE_TRANSPORT_ID, login: RELEASE_PLEASE_BOT_NAME },
    { id: 1, login: RELEASE_PLEASE_TRANSPORT_NAME },
    { id: RELEASE_PLEASE_TRANSPORT_ID, login: 'attacker' },
  ]) {
    await assert.rejects(() => poll(creator), /live PR head.*moved/);
  }
});

test('fails closed when the current run attempt advances after the proof jobs are read', async () => {
  let runReads = 0;
  let currentAttempt = run.run_attempt;
  await assert.rejects(() => pollReleaseAuthorityProof({
    request: async (endpoint) => {
      if (endpoint.includes(`/pulls/${PR_NUMBER}`)) return livePr();
      if (endpoint.includes('/actions/workflows/')) {
        runReads += 1;
        return runsResponse([{
          ...run,
          run_attempt: currentAttempt,
          status: currentAttempt === run.run_attempt ? 'completed' : 'in_progress',
          conclusion: currentAttempt === run.run_attempt ? 'success' : null,
        }]);
      }
      if (endpoint.includes(`/actions/runs/${run.id}/attempts/${run.run_attempt}/jobs`)) {
        currentAttempt += 1;
        return { jobs: [job] };
      }
      throw new Error(`unexpected endpoint: ${endpoint}`);
    },
    sleep: async () => {},
    repository: PINNED_RELEASE_REPOSITORY,
    prNumber: PR_NUMBER,
    headSha: T,
    parentSha: C,
    headRef: HEAD_REF,
    maxAttempts: 1,
  }), /changed during proof validation/);
  assert.equal(runReads, 2);
});

test('filters by exact parent SHA and paginates before selecting the globally newest run', async () => {
  const firstPage = Array.from({ length: 100 }, (_, index) => ({
    ...run,
    id: run.id - 1000 + index,
    run_number: index + 1,
    run_attempt: 1,
  }));
  const pages = [];
  const proof = await pollReleaseAuthorityProof({
    request: async (endpoint) => {
      if (endpoint.includes(`/pulls/${PR_NUMBER}`)) return livePr();
      if (endpoint.includes('/actions/workflows/')) {
        const url = new URL(endpoint, 'https://api.github.com');
        assert.equal(url.searchParams.get('branch'), 'dev');
        assert.equal(url.searchParams.get('event'), 'push');
        assert.equal(url.searchParams.get('head_sha'), C);
        const page = Number(url.searchParams.get('page') ?? '1');
        pages.push(page);
        return page === 1 ? runsResponse(firstPage, 101) : runsResponse([run], 101);
      }
      if (endpoint.includes(`/actions/runs/${run.id}/attempts/${run.run_attempt}/jobs`)) {
        return { jobs: [job] };
      }
      throw new Error(`unexpected endpoint: ${endpoint}`);
    },
    sleep: async () => {},
    repository: PINNED_RELEASE_REPOSITORY,
    prNumber: PR_NUMBER,
    headSha: T,
    parentSha: C,
    headRef: HEAD_REF,
    maxAttempts: 1,
  });
  assert.equal(proof.runId, run.id);
  assert.deepEqual(pages, [1, 2, 1, 2]);
});

test('a page-two newer pending or failed run overrides a page-one success', async () => {
  const olderSuccess = {
    ...run,
    id: run.id - 1,
    run_number: run.run_number - 1,
    run_attempt: 1,
  };
  const firstPage = [olderSuccess, ...Array.from({ length: 99 }, (_, index) => ({
    ...run,
    id: run.id - 1000 - index,
    run_number: index + 1,
    run_attempt: 1,
  }))];
  const invoke = (newest) => pollReleaseAuthorityProof({
    request: async (endpoint) => {
      if (endpoint.includes(`/pulls/${PR_NUMBER}`)) return livePr();
      if (endpoint.includes('/actions/workflows/')) {
        const page = Number(new URL(endpoint, 'https://api.github.com').searchParams.get('page') ?? '1');
        return page === 1 ? runsResponse(firstPage, 101) : runsResponse([newest], 101);
      }
      if (endpoint.includes(`/actions/runs/${olderSuccess.id}/attempts/${olderSuccess.run_attempt}/jobs`)) {
        return { jobs: [{ ...job, run_id: olderSuccess.id, run_attempt: olderSuccess.run_attempt }] };
      }
      throw new Error(`unexpected endpoint: ${endpoint}`);
    },
    sleep: async () => {},
    repository: PINNED_RELEASE_REPOSITORY,
    prNumber: PR_NUMBER,
    headSha: T,
    parentSha: C,
    headRef: HEAD_REF,
    maxAttempts: 1,
  });
  await assert.rejects(
    () => invoke({ ...run, status: 'in_progress', conclusion: null }),
    /timed out/,
  );
  await assert.rejects(
    () => invoke({ ...run, status: 'completed', conclusion: 'failure' }),
    /completed successfully/,
  );
});

test('fails closed when exact-parent pagination metadata is missing or changes between pages', async () => {
  const invoke = (workflowResponse) => pollReleaseAuthorityProof({
    request: async (endpoint) => {
      if (endpoint.includes(`/pulls/${PR_NUMBER}`)) return livePr();
      if (endpoint.includes('/actions/workflows/')) {
        const page = Number(new URL(endpoint, 'https://api.github.com').searchParams.get('page') ?? '1');
        return workflowResponse(page);
      }
      throw new Error(`unexpected endpoint: ${endpoint}`);
    },
    sleep: async () => {},
    repository: PINNED_RELEASE_REPOSITORY,
    prNumber: PR_NUMBER,
    headSha: T,
    parentSha: C,
    headRef: HEAD_REF,
    maxAttempts: 1,
  });
  await assert.rejects(
    () => invoke(() => ({ workflow_runs: [run] })),
    /total_count/,
  );
  await assert.rejects(
    () => invoke((page) => page === 1
      ? runsResponse(Array.from({ length: 100 }, (_, index) => ({
        ...run,
        id: run.id - 1000 + index,
        run_number: index + 1,
      })), 101)
      : runsResponse([run], 102)),
    /changed while paginating/,
  );
});

test('newest protected run wins so recovery supersedes an older failure', async () => {
  const olderFailure = {
    ...run,
    id: run.id - 1,
    run_number: run.run_number - 1,
    status: 'completed',
    conclusion: 'failure',
  };
  const proof = await pollReleaseAuthorityProof({
    request: async (endpoint) => {
      if (endpoint.includes(`/pulls/${PR_NUMBER}`)) return livePr();
      if (endpoint.includes('/actions/workflows/')) return runsResponse([olderFailure, run]);
      if (endpoint.includes(`/actions/runs/${run.id}/attempts/${run.run_attempt}/jobs`)) return { jobs: [job] };
      throw new Error(`unexpected endpoint: ${endpoint}`);
    },
    sleep: async () => {},
    repository: PINNED_RELEASE_REPOSITORY,
    prNumber: PR_NUMBER,
    headSha: T,
    parentSha: C,
    headRef: HEAD_REF,
    maxAttempts: 1,
  });
  assert.equal(proof.runId, run.id);
  assert.equal(proof.runNumber, run.run_number);
});

test('a newer pending or failed protected run overrides an older success', async () => {
  const olderSuccess = { ...run, id: run.id - 1, run_number: run.run_number - 1 };
  const invoke = (newest) => pollReleaseAuthorityProof({
    request: async (endpoint) => {
      if (endpoint.includes(`/pulls/${PR_NUMBER}`)) return livePr();
      if (endpoint.includes('/actions/workflows/')) return runsResponse([olderSuccess, newest]);
      if (endpoint.includes(`/actions/runs/${olderSuccess.id}/`)) return { jobs: [{ ...job, run_id: olderSuccess.id }] };
      throw new Error(`unexpected endpoint: ${endpoint}`);
    },
    sleep: async () => {},
    repository: PINNED_RELEASE_REPOSITORY,
    prNumber: PR_NUMBER,
    headSha: T,
    parentSha: C,
    headRef: HEAD_REF,
    maxAttempts: 1,
  });
  await assert.rejects(
    () => invoke({ ...run, status: 'in_progress', conclusion: null }),
    /timed out/,
  );
  await assert.rejects(
    () => invoke({ ...run, status: 'completed', conclusion: 'failure' }),
    /completed successfully/,
  );
});

test('fails closed when the PR moves during polling or proof never appears', async () => {
  let pullReads = 0;
  await assert.rejects(() => pollReleaseAuthorityProof({
    request: async (endpoint) => {
      if (endpoint.includes(`/pulls/${PR_NUMBER}`)) {
        pullReads += 1;
        return livePr(pullReads === 1 ? T : OTHER);
      }
      if (endpoint.includes('/actions/workflows/')) return runsResponse([run]);
      return { jobs: [job] };
    },
    sleep: async () => {},
    repository: PINNED_RELEASE_REPOSITORY,
    prNumber: PR_NUMBER,
    headSha: T,
    parentSha: C,
    headRef: HEAD_REF,
    maxAttempts: 1,
  }), /live PR head.*moved/);

  await assert.rejects(() => pollReleaseAuthorityProof({
    request: async (endpoint) => (
      endpoint.includes(`/pulls/${PR_NUMBER}`) ? livePr() : runsResponse([])
    ),
    sleep: async () => {},
    repository: PINNED_RELEASE_REPOSITORY,
    prNumber: PR_NUMBER,
    headSha: T,
    parentSha: C,
    headRef: HEAD_REF,
    maxAttempts: 2,
  }), /timed out/);
});

test('GitHub proof adapter pins the API origin, authentication, and JSON success contract', async () => {
  let observed;
  const result = await githubJsonRequest(`/repos/${PINNED_RELEASE_REPOSITORY}/pulls/${PR_NUMBER}`, {
    token: 'test-token',
    fetchImpl: async (url, options) => {
      observed = { url, options };
      return { ok: true, status: 200, json: async () => ({ number: PR_NUMBER }) };
    },
  });
  assert.deepEqual(result, { number: PR_NUMBER });
  assert.equal(observed.url, `https://api.github.com/repos/${PINNED_RELEASE_REPOSITORY}/pulls/${PR_NUMBER}`);
  assert.equal(observed.options.method, 'GET');
  assert.equal(observed.options.redirect, 'error');
  assert.equal(observed.options.headers.Authorization, 'Bearer test-token');

  await assert.rejects(
    () => githubJsonRequest('/repos/oyatie/console/pulls/760', { token: '', fetchImpl: async () => {} }),
    /GITHUB_TOKEN/,
  );
  await assert.rejects(
    () => githubJsonRequest('https://attacker.invalid/', { token: 'test-token', fetchImpl: async () => {} }),
    /relative GitHub API path/,
  );
  await assert.rejects(
    () => githubJsonRequest('/repos/oyatie/console/pulls/760', {
      token: 'test-token',
      fetchImpl: async () => ({ ok: false, status: 403 }),
    }),
    /HTTP 403/,
  );
  await assert.rejects(
    () => githubJsonRequest('/repos/oyatie/console/pulls/760', {
      token: 'test-token',
      fetchImpl: async () => ({ ok: true, status: 200, json: async () => { throw new Error('bad JSON'); } }),
    }),
    /not JSON/,
  );
});
