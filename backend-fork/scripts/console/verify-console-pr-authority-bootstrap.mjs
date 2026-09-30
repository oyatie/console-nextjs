#!/usr/bin/env node
/**
 * Protected-target PR router.
 *
 * Ordinary PRs are bound to the exact live base/head graph without executing
 * candidate bytes. Release-shaped PRs enter a separate fail-closed route that
 * additionally requires the pinned Release Please run/job proof. This module
 * does not authenticate ordinary commit authors and has no signing capability.
 */
import { execFileSync } from 'node:child_process';
import {
  RELEASE_PLEASE_BOT_ID,
  RELEASE_PLEASE_BOT_EMAIL,
  RELEASE_PLEASE_BOT_NAME,
  RELEASE_PLEASE_HEAD_REF,
  RELEASE_PLEASE_SUBJECT,
  RELEASE_PLEASE_TRANSPORT_ID,
  RELEASE_PLEASE_TRANSPORT_NAME,
  RELEASE_PLEASE_TRAIN_CLASS,
  classifyReleasePleaseBotTip,
  releasePleasePrCreatorClass,
  verifyReleasePleaseBotTrain,
} from './release-please-bot-candidate.mjs';

const SHA = /^[0-9a-f]{40}$/;
// Repository transferred to the `oyatie` organisation. The numeric id below is
// unchanged (1269693002), which is what proves this is the same repository
// rather than a look-alike: GitHub preserves the id across a transfer and
// cannot preserve it across a re-creation. The id pin stays load-bearing; only
// the human-readable name moved.
export const PINNED_RELEASE_REPOSITORY = 'oyatie/console';
export const PINNED_RELEASE_REPOSITORY_ID = 1269693002;
export { RELEASE_PLEASE_BOT_ID };
export const RELEASE_PLEASE_WORKFLOW_ID = 296023729;
export const RELEASE_PLEASE_WORKFLOW_PATH = '.github/workflows/release-please.yml';
const RELEASE_PLEASE_WORKFLOW_NAME = 'Release Please';
const RELEASE_PLEASE_WORKFLOW_EVENTS = Object.freeze(['push']);
const RAW_DIFF_ARGS = Object.freeze(['diff', '--raw', '-z', '--abbrev=40', '--no-renames', '--no-ext-diff']);
const SAFE_ENVIRONMENT_KEYS = Object.freeze(['PATH', 'SystemRoot', 'SYSTEMROOT', 'ComSpec', 'TEMP', 'TMP', 'TMPDIR', 'LANG', 'LC_ALL', 'TZ']);
const fail = (message) => { throw new Error(`console authority bootstrap: ${message}`); };
const exactSha = (value, label) => { if (!SHA.test(value ?? '')) fail(`${label} must be a lowercase 40-character SHA`); return value; };

function exactPositiveInteger(value, label) {
  if (!Number.isSafeInteger(value) || value <= 0) fail(`${label} must be a positive integer`);
  return value;
}

function releaseProofJobName(prNumber, headSha) {
  return `release-authority-proof pr=${prNumber} head=${headSha}`;
}

/** Validate GitHub's native run/job record; generic statuses and checks are not inputs. */
export function assertReleaseAuthorityProof({
  runs,
  jobs,
  repository,
  prNumber,
  headSha,
  parentSha,
} = {}) {
  if (repository !== PINNED_RELEASE_REPOSITORY) fail('release proof repository name is not pinned');
  const number = exactPositiveInteger(prNumber, 'release proof PR number');
  const T = exactSha(headSha, 'release proof head SHA');
  const C = exactSha(parentSha, 'release proof parent SHA');
  if (!Array.isArray(runs) || runs.length !== 1) fail('release proof requires exactly one protected workflow run');
  const [run] = runs;
  if (!run || typeof run !== 'object') fail('release proof workflow run is malformed');
  if (run.workflow_id !== RELEASE_PLEASE_WORKFLOW_ID) fail('release proof workflow id is not pinned');
  if (run.path !== RELEASE_PLEASE_WORKFLOW_PATH) fail('release proof workflow path is not pinned');
  if (!RELEASE_PLEASE_WORKFLOW_EVENTS.includes(run.event)) {
    fail('release proof must originate from an allowed event');
  }
  if (run.head_branch !== 'dev') fail('release proof run must use the dev branch');
  if (run.head_sha !== C) fail('release proof run head must equal the release tip parent SHA');
  if (run?.repository?.id !== PINNED_RELEASE_REPOSITORY_ID) fail('release proof repository id is not pinned');
  if (run?.repository?.full_name !== PINNED_RELEASE_REPOSITORY) fail('release proof repository name is not pinned');
  const runId = exactPositiveInteger(run.id, 'release proof run id');
  const runNumber = exactPositiveInteger(run.run_number, 'release proof run number');
  const runAttempt = exactPositiveInteger(run.run_attempt, 'release proof run attempt');
  if (run.status !== 'completed' || run.conclusion !== 'success') {
    fail('release proof workflow run must have completed successfully');
  }

  if (!Array.isArray(jobs)) fail('release proof jobs are unavailable');
  const name = releaseProofJobName(number, T);
  const matching = jobs.filter((entry) => entry?.name === name);
  if (matching.length !== 1) fail('release proof requires exactly one exact proof job');
  const [job] = matching;
  const jobId = exactPositiveInteger(job.id, 'release proof job id');
  if (job.run_id !== runId) fail('release proof job run id must equal the protected run');
  if (job.run_attempt !== runAttempt) fail('release proof job run attempt must equal the current run attempt');
  if (job.workflow_name !== RELEASE_PLEASE_WORKFLOW_NAME) fail('release proof workflow name is not pinned');
  if (job.head_sha !== C) fail('release proof job head must equal the release tip parent SHA');
  if (job.status !== 'completed' || job.conclusion !== 'success') {
    fail('release proof job must have completed successfully');
  }
  return Object.freeze({
    workflowId: RELEASE_PLEASE_WORKFLOW_ID,
    runId,
    runNumber,
    runAttempt,
    jobId,
    prNumber: number,
    headSha: T,
    parentSha: C,
  });
}

function assertLiveReleasePr(pr, { repository, prNumber, headSha, headRef }, phase) {
  if (
    pr?.number !== prNumber
    || pr?.state !== 'open'
    || releasePleasePrCreatorClass(pr?.user) === null
    || pr?.head?.sha !== headSha
    || pr?.head?.ref !== headRef
    || pr?.head?.repo?.full_name !== repository
    || pr?.base?.ref !== 'dev'
  ) {
    fail(`live PR head or protected release shape moved ${phase} proof polling`);
  }
}

async function listAllReleaseProofRuns({ request, repository, parentSha }) {
  const endpoint = `/repos/${repository}/actions/workflows/${RELEASE_PLEASE_WORKFLOW_ID}/runs`
    + `?branch=dev&event=push&head_sha=${parentSha}&per_page=100`;
  const runs = [];
  const seenRunIds = new Set();
  let totalCount = null;
  for (let page = 1; page <= 10; page += 1) {
    const response = await request(`${endpoint}&page=${page}`);
    if (!Number.isSafeInteger(response?.total_count) || response.total_count < 0) {
      fail('release proof run search total_count is unavailable');
    }
    if (response.total_count > 1000) {
      fail('release proof run search exceeds the GitHub 1000-result search bound');
    }
    if (totalCount === null) totalCount = response.total_count;
    if (response.total_count !== totalCount) {
      fail('release proof run search changed while paginating');
    }
    const pageRuns = response.workflow_runs;
    if (!Array.isArray(pageRuns) || pageRuns.length > 100) {
      fail('release proof run search page is malformed');
    }
    for (const entry of pageRuns) {
      const runId = exactPositiveInteger(entry?.id, 'release proof run id');
      if (seenRunIds.has(runId)) fail('release proof run search repeated a run across pages');
      seenRunIds.add(runId);
      runs.push(entry);
    }
    if (runs.length === totalCount) return runs;
    if (runs.length > totalCount || pageRuns.length === 0) {
      fail('release proof run search pagination is incomplete');
    }
  }
  fail('release proof run search did not complete within ten pages');
}

function selectNewestReleaseProofRuns(allRuns, parentSha) {
  const runs = allRuns.filter((entry) => (
    entry?.workflow_id === RELEASE_PLEASE_WORKFLOW_ID && entry?.head_sha === parentSha
  ));
  if (runs.length === 0) return [];
  const numbered = runs.map((entry) => ({
    entry,
    number: exactPositiveInteger(entry?.run_number, 'release proof run number'),
  }));
  const newestNumber = Math.max(...numbered.map(({ number }) => number));
  const selected = numbered
    .filter(({ number }) => number === newestNumber)
    .map(({ entry }) => entry);
  if (selected.length !== 1) {
    fail('release proof requires exactly one newest protected workflow run');
  }
  return selected;
}

function releaseProofRunSnapshot(run) {
  return JSON.stringify([
    run?.id,
    run?.workflow_id,
    run?.path,
    run?.event,
    run?.head_branch,
    run?.head_sha,
    run?.run_number,
    run?.run_attempt,
    run?.status,
    run?.conclusion,
    run?.repository?.id,
    run?.repository?.full_name,
  ]);
}

export async function pollReleaseAuthorityProof({
  request,
  sleep,
  repository,
  prNumber,
  headSha,
  parentSha,
  headRef,
  maxAttempts = 60,
} = {}) {
  if (typeof request !== 'function' || typeof sleep !== 'function') fail('release proof polling adapters are required');
  if (repository !== PINNED_RELEASE_REPOSITORY) fail('release proof repository name is not pinned');
  const number = exactPositiveInteger(prNumber, 'release proof PR number');
  const T = exactSha(headSha, 'release proof head SHA');
  const C = exactSha(parentSha, 'release proof parent SHA');
  if (typeof headRef !== 'string' || !RELEASE_PLEASE_HEAD_REF.test(headRef)) {
    fail('release proof PR head ref is outside the exact release-please class');
  }
  if (!Number.isSafeInteger(maxAttempts) || maxAttempts < 1 || maxAttempts > 120) {
    fail('release proof max attempts must be between 1 and 120');
  }
  const pullEndpoint = `/repos/${repository}/pulls/${number}`;
  assertLiveReleasePr(await request(pullEndpoint), {
    repository, prNumber: number, headSha: T, headRef,
  }, 'before');

  for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
    const allRuns = await listAllReleaseProofRuns({ request, repository, parentSha: C });
    const selectedRuns = selectNewestReleaseProofRuns(allRuns, C);
    if (selectedRuns.length === 1 && selectedRuns[0]?.status === 'completed') {
      if (selectedRuns[0].conclusion !== 'success') {
        assertReleaseAuthorityProof({ runs: selectedRuns, jobs: [], repository, prNumber: number, headSha: T, parentSha: C });
      }
      const runId = exactPositiveInteger(selectedRuns[0].id, 'release proof run id');
      const runAttempt = exactPositiveInteger(selectedRuns[0].run_attempt, 'release proof run attempt');
      const jobsEndpoint = `/repos/${repository}/actions/runs/${runId}/attempts/${runAttempt}/jobs?per_page=100`;
      const jobsResponse = await request(jobsEndpoint);
      const jobs = Array.isArray(jobsResponse?.jobs) ? jobsResponse.jobs : [];
      if (jobs.some((entry) => entry?.name === releaseProofJobName(number, T))) {
        const proof = assertReleaseAuthorityProof({
          runs: selectedRuns, jobs, repository, prNumber: number, headSha: T, parentSha: C,
        });
        const refreshedRuns = selectNewestReleaseProofRuns(
          await listAllReleaseProofRuns({ request, repository, parentSha: C }),
          C,
        );
        if (refreshedRuns.length !== 1
          || releaseProofRunSnapshot(refreshedRuns[0]) !== releaseProofRunSnapshot(selectedRuns[0])) {
          fail('release proof run changed during proof validation');
        }
        assertReleaseAuthorityProof({
          runs: refreshedRuns, jobs, repository, prNumber: number, headSha: T, parentSha: C,
        });
        assertLiveReleasePr(await request(pullEndpoint), {
          repository, prNumber: number, headSha: T, headRef,
        }, 'after');
        return proof;
      }
    }
    if (attempt + 1 < maxAttempts) await sleep();
  }
  fail(`release proof timed out after ${maxAttempts} bounded attempts`);
}

function mapReleasePleaseError(error) {
  const message = error instanceof Error ? error.message : String(error);
  fail(message.replace(/^release-please bot candidate: /, ''));
}

function assertReleaseProofCoordinates(proof, {
  prNumber,
  headSha,
  parentSha,
  prHeadRef,
  prHeadRepository,
  repository,
}) {
  if (repository !== PINNED_RELEASE_REPOSITORY || prHeadRepository !== repository) {
    fail('release PR head repository must equal the pinned protected repository');
  }
  if (typeof prHeadRef !== 'string' || !RELEASE_PLEASE_HEAD_REF.test(prHeadRef)) {
    fail('release PR head ref must match the exact release-please class');
  }
  const number = typeof prNumber === 'string' && /^\d+$/.test(prNumber)
    ? Number(prNumber)
    : prNumber;
  exactPositiveInteger(number, 'release authority proof PR number');
  if (
    !proof
    || typeof proof !== 'object'
    || proof.workflowId !== RELEASE_PLEASE_WORKFLOW_ID
    || !Number.isSafeInteger(proof.runId)
    || proof.runId <= 0
    || !Number.isSafeInteger(proof.runNumber)
    || proof.runNumber <= 0
    || !Number.isSafeInteger(proof.runAttempt)
    || proof.runAttempt <= 0
    || !Number.isSafeInteger(proof.jobId)
    || proof.jobId <= 0
    || proof.prNumber !== number
    || proof.headSha !== headSha
    || proof.parentSha !== parentSha
  ) {
    fail('release authority proof does not bind the exact PR, head, parent, and protected workflow');
  }
}

function sanitizedGitEnvironment(source = process.env) {
  const environment = {};
  for (const key of SAFE_ENVIRONMENT_KEYS) {
    if (Object.hasOwn(source, key)) environment[key] = source[key];
  }
  return {
    ...environment,
    HOME: '/dev/null',
    XDG_CONFIG_HOME: '/dev/null',
    GIT_CONFIG_NOSYSTEM: '1',
    GIT_CONFIG_GLOBAL: '/dev/null',
    GIT_CONFIG_SYSTEM: '/dev/null',
    GIT_TERMINAL_PROMPT: '0',
    GIT_ASKPASS: '/bin/false',
  };
}

/**
 * Three-state router. Any protected release signal reserves the release lane;
 * incomplete or contradictory signals cannot downgrade into ordinary admission.
 */
export function classifyProtectedPrRoute(ops, {
  baseSha,
  headSha,
  prAuthorId,
  prAuthorLogin,
  prHeadRef,
  prHeadRepository,
  repository,
}) {
  const B = exactSha(baseSha, 'protected base');
  const H = exactSha(headSha, 'PR head');
  if (repository !== PINNED_RELEASE_REPOSITORY) fail('repository name is not pinned');
  if (typeof prHeadRef !== 'string' || prHeadRef.length === 0) fail('PR head ref is unavailable');
  if (typeof prHeadRepository !== 'string' || prHeadRepository.length === 0) fail('PR head repository is unavailable');

  let identity;
  try { identity = ops.commitIdentity(H); } catch { fail('PR head commit identity is unavailable'); }
  if (!identity || typeof identity !== 'object') fail('PR head commit identity is unavailable');
  let changes;
  try { changes = ops.diff(B, H); } catch { fail('protected base..head diff is unavailable'); }
  if (!Array.isArray(changes)) fail('protected base..head diff is unavailable');

  const classifiedRelease = classifyReleasePleaseBotTip(ops, H);
  // The pinned transport principal is also the repository's ordinary human
  // contributor, so that identity alone cannot reserve the release lane. It is
  // accepted only after an independent release-shaped signal below. The bot
  // identity remains a release claim by itself because it has no ordinary lane.
  const creatorClaimsRelease = prAuthorId === RELEASE_PLEASE_BOT_ID
    || prAuthorLogin === RELEASE_PLEASE_BOT_NAME;
  const refClaimsRelease = prHeadRef.startsWith('release-please');
  const envelopeClaimsRelease = identity.authorName === RELEASE_PLEASE_BOT_NAME
    || identity.authorEmail === RELEASE_PLEASE_BOT_EMAIL
    || (typeof identity.subject === 'string' && RELEASE_PLEASE_SUBJECT.test(identity.subject));
  const manifestClaimsRelease = changes.some((change) => change?.path === '.release-please-manifest.json');
  const hasReleaseClaim = creatorClaimsRelease || refClaimsRelease
    || envelopeClaimsRelease || manifestClaimsRelease || classifiedRelease !== null;

  if (!hasReleaseClaim) return Object.freeze({ admissionClass: 'ordinary-pr' });
  const exactRelease = classifiedRelease !== null
    && classifiedRelease.candidateSha === B
    && releasePleasePrCreatorClass({ id: prAuthorId, login: prAuthorLogin }) !== null
    && RELEASE_PLEASE_HEAD_REF.test(prHeadRef)
    && prHeadRepository === repository;
  if (!exactRelease) return Object.freeze({ admissionClass: 'malformed-release-claim' });
  return Object.freeze({
    admissionClass: RELEASE_PLEASE_TRAIN_CLASS,
    candidateSha: classifiedRelease.candidateSha,
  });
}

/** Pure structural seam. It reads Git object facts but never candidate bytes. */
export function verifyBootstrapGraph(ops, coordinates) {
  const B = exactSha(coordinates?.baseSha, 'protected base');
  const H = exactSha(coordinates?.headSha, 'PR head');
  if (!ops.hasCommit(B)) fail('protected base object is unavailable');
  if (!ops.hasCommit(H)) fail('PR head object is unavailable');
  if (!ops.isAncestor(B, H)) fail('protected base must be an ancestor of the PR head');
  const route = classifyProtectedPrRoute(ops, coordinates);
  if (route.admissionClass === 'malformed-release-claim') {
    fail('malformed protected release claim cannot use ordinary admission');
  }
  if (route.admissionClass === 'ordinary-pr') {
    return Object.freeze({ baseSha: B, headSha: H, admissionClass: 'ordinary-pr' });
  }

  try {
    assertReleaseProofCoordinates(coordinates.releaseAuthorityProof, {
      prNumber: coordinates.prNumber,
      headSha: H,
      parentSha: B,
      prHeadRef: coordinates.prHeadRef,
      prHeadRepository: coordinates.prHeadRepository,
      repository: coordinates.repository,
    });
    const M = exactSha(coordinates.mergeSha, 'PR merge');
    const admitted = verifyReleasePleaseBotTrain(ops, {
      headSha: H,
      mergeSha: M,
      requirePrMeta: false,
    });
    const mergeParents = ops.parents(M);
    if (!Array.isArray(mergeParents) || mergeParents.length !== 2
      || mergeParents[0] !== B || mergeParents[1] !== H) {
      fail('release merge parents must equal the protected base and exact release head');
    }
    if (admitted.candidateSha !== B) fail('release tip parent must equal the protected base');
    return Object.freeze({
      baseSha: B,
      headSha: H,
      mergeSha: M,
      admissionClass: RELEASE_PLEASE_TRAIN_CLASS,
    });
  } catch (error) {
    mapReleasePleaseError(error);
  }
}

export function assertLivePullRequestSnapshot(pr, {
  repository,
  prNumber,
  baseSha,
  headSha,
}, phase) {
  if (repository !== PINNED_RELEASE_REPOSITORY) fail('live PR repository name is not pinned');
  const number = typeof prNumber === 'string' && /^\d+$/.test(prNumber)
    ? Number(prNumber)
    : prNumber;
  exactPositiveInteger(number, 'live PR number');
  const B = exactSha(baseSha, 'live PR expected base');
  const H = exactSha(headSha, 'live PR expected head');
  if (
    pr?.number !== number
    || pr?.state !== 'open'
    || pr?.draft !== false
    || !Number.isSafeInteger(pr?.user?.id)
    || typeof pr?.user?.login !== 'string'
    || pr.user.login.length === 0
    || pr?.base?.ref !== 'dev'
    || pr?.base?.sha !== B
    || pr?.base?.repo?.id !== PINNED_RELEASE_REPOSITORY_ID
    || pr?.base?.repo?.full_name !== repository
    || pr?.head?.sha !== H
    || typeof pr?.head?.ref !== 'string'
    || pr.head.ref.length === 0
    || typeof pr?.head?.repo?.full_name !== 'string'
    || pr.head.repo.full_name.length === 0
  ) {
    fail(`live PR snapshot does not match the exact protected coordinates ${phase}`);
  }
  return Object.freeze({
    prNumber: number,
    prAuthorId: pr.user.id,
    prAuthorLogin: pr.user.login,
    prHeadRef: pr.head.ref,
    prHeadRepository: pr.head.repo.full_name,
  });
}

function git(repo, args, options = {}) {
  const { env, ...rest } = options;
  return execFileSync('git', ['-C', repo, '-c', 'core.hooksPath=/dev/null', ...args], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
    env: sanitizedGitEnvironment(env),
    ...rest,
  });
}

function gitOk(repo, args) {
  try { git(repo, args); return true; } catch { return false; }
}

export function rawDiff(repo, from, to) {
  const fields = git(repo, [...RAW_DIFF_ARGS, from, to]).split('\0');
  const changes = [];
  for (let index = 0; index < fields.length - 1;) {
    const header = fields[index++];
    if (!header) continue;
    const match = header.match(/^:(\d{6}) (\d{6}) [0-9a-f]{40} [0-9a-f]{40} ([A-Z])$/);
    if (!match) fail('Git diff contains an unsupported entry');
    const [, oldMode, newMode, status] = match;
    changes.push({
      path: fields[index++],
      status,
      oldMode,
      newMode,
      oldType: oldMode === '000000' ? null : 'blob',
      newType: newMode === '000000' ? null : 'blob',
    });
  }
  return changes;
}

export function createProtectedGitOps(repo) {
  return {
    hasCommit: (sha) => gitOk(repo, ['cat-file', '-e', `${sha}^{commit}`]),
    parents: (sha) => git(repo, ['show', '-s', '--format=%P', sha]).trim().split(/\s+/).filter(Boolean),
    commitIdentity: (sha) => {
      const raw = git(repo, ['show', '-s', '--format=%an%x00%ae%x00%cn%x00%ce%x00%s', sha]);
      const [authorName, authorEmail, committerName, committerEmail, subject] = raw.replace(/\n$/, '').split('\0');
      return { authorName, authorEmail, committerName, committerEmail, subject };
    },
    diff: (from, to) => rawDiff(repo, from, to),
    tree: (sha) => git(repo, ['show', '-s', '--format=%T', sha]).trim(),
    sameTreeDiff: (left, right) => gitOk(repo, ['diff', '--quiet', '--no-ext-diff', left, right]),
    isAncestor: (ancestor, descendant) => gitOk(repo, ['merge-base', '--is-ancestor', ancestor, descendant]),
  };
}

function parseArgs(argv) {
  const result = {};
  const allowed = new Set(['--pr-number', '--head', '--base-sha', '--base', '--repository']);
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    const name = typeof key === 'string' ? key.slice(2) : '';
    if (!allowed.has(key) || value === undefined || Object.hasOwn(result, name)) {
      fail('usage: --pr-number N --head SHA --base-sha SHA --base dev --repository OWNER/REPO');
    }
    result[name] = value;
  }
  if (!/^\d+$/.test(result['pr-number'] ?? '')) fail('PR number is invalid');
  exactSha(result.head, 'PR head');
  exactSha(result['base-sha'], 'protected base');
  if (result.base !== 'dev') fail('PR base is outside the protected dev trust scope');
  if (result.repository !== PINNED_RELEASE_REPOSITORY) fail('repository name is not pinned');
  return result;
}

function exactPrNumber(value) {
  const parsed = String(value);
  if (!/^\d+$/.test(parsed) || Number(parsed) <= 0) fail('PR number is invalid');
  return parsed;
}

export function fetchExactProtectedBase(repo, expectedBase) {
  const B = exactSha(expectedBase, 'protected base');
  const ref = 'refs/console-bootstrap/protected-main';
  git(repo, ['fetch', '--no-tags', '--no-recurse-submodules', 'origin', `+refs/heads/dev:${ref}`]);
  if (git(repo, ['rev-parse', ref]).trim() !== B) fail('protected dev ref does not match the event base SHA');
  return B;
}

export function fetchExactPullHead(repo, number, expectedHead) {
  const parsed = exactPrNumber(number);
  const H = exactSha(expectedHead, 'PR head');
  const ref = `refs/console-bootstrap/${parsed}/head`;
  git(repo, ['fetch', '--no-tags', '--no-recurse-submodules', 'origin', `+refs/pull/${parsed}/head:${ref}`]);
  if (git(repo, ['rev-parse', ref]).trim() !== H) fail('GitHub pull head ref does not match event SHA');
  return H;
}

function fetchExactPullMerge(repo, number) {
  const parsed = exactPrNumber(number);
  const ref = `refs/console-bootstrap/${parsed}/merge`;
  git(repo, ['fetch', '--no-tags', '--no-recurse-submodules', 'origin', `+refs/pull/${parsed}/merge:${ref}`]);
  return exactSha(git(repo, ['rev-parse', ref]).trim(), 'GitHub pull merge ref');
}

export async function githubJsonRequest(endpoint, {
  token = process.env.GITHUB_TOKEN,
  fetchImpl = globalThis.fetch,
} = {}) {
  if (typeof token !== 'string' || token.length === 0) fail('protected GITHUB_TOKEN is required for GitHub reads');
  if (typeof endpoint !== 'string' || !endpoint.startsWith(`/repos/${PINNED_RELEASE_REPOSITORY}/`)) {
    fail('request must use a relative GitHub API path for the pinned repository');
  }
  if (typeof fetchImpl !== 'function') fail('GitHub API fetch adapter is unavailable');
  let response;
  try {
    response = await fetchImpl(`https://api.github.com${endpoint}`, {
      method: 'GET',
      redirect: 'error',
      signal: AbortSignal.timeout(15_000),
      headers: {
        Accept: 'application/vnd.github+json',
        Authorization: `Bearer ${token}`,
        'X-GitHub-Api-Version': '2022-11-28',
        'User-Agent': 'console-authority-bootstrap',
      },
    });
  } catch {
    fail('GitHub API request failed');
  }
  if (!response.ok) fail(`GitHub API read failed with HTTP ${response.status}`);
  try {
    return await response.json();
  } catch {
    fail('GitHub API response was not JSON');
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const repo = process.cwd();
  const prNumber = Number(args['pr-number']);
  const baseSha = args['base-sha'];
  const headSha = args.head;
  if (git(repo, ['rev-parse', 'HEAD']).trim() !== baseSha) {
    fail('protected checkout HEAD does not equal the event base SHA');
  }
  fetchExactProtectedBase(repo, baseSha);
  fetchExactPullHead(repo, prNumber, headSha);
  const pullEndpoint = `/repos/${args.repository}/pulls/${prNumber}`;
  const expected = { repository: args.repository, prNumber, baseSha, headSha };
  const before = assertLivePullRequestSnapshot(
    await githubJsonRequest(pullEndpoint),
    expected,
    'before validation',
  );
  const ops = createProtectedGitOps(repo);
  const coordinates = {
    ...expected,
    ...before,
  };
  const route = classifyProtectedPrRoute(ops, coordinates);
  if (route.admissionClass === 'malformed-release-claim') {
    fail('malformed protected release claim cannot use ordinary admission');
  }
  if (route.admissionClass === RELEASE_PLEASE_TRAIN_CLASS) {
    coordinates.releaseAuthorityProof = await pollReleaseAuthorityProof({
      request: githubJsonRequest,
      sleep: () => new Promise((resolve) => setTimeout(resolve, 3_000)),
      repository: args.repository,
      prNumber,
      headSha,
      parentSha: route.candidateSha,
      headRef: before.prHeadRef,
      maxAttempts: 60,
    });
    coordinates.mergeSha = fetchExactPullMerge(repo, prNumber);
  }
  const graph = verifyBootstrapGraph(ops, coordinates);

  fetchExactProtectedBase(repo, baseSha);
  fetchExactPullHead(repo, prNumber, headSha);
  const after = assertLivePullRequestSnapshot(
    await githubJsonRequest(pullEndpoint),
    expected,
    'after validation',
  );
  if (JSON.stringify(after) !== JSON.stringify(before)) {
    fail('live PR routing metadata moved during validation');
  }
  process.stdout.write(`${JSON.stringify({ verdict: 'PASS', ...graph }, null, 2)}\n`);
}

if (import.meta.url === new URL(`file://${process.argv[1]}`).href) await main();
