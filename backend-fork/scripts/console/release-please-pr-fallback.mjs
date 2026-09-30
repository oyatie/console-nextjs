#!/usr/bin/env node
/**
 * Exact-state fallback for repositories where the protected GITHUB_TOKEN may
 * generate a Release Please commit/ref but enterprise policy blocks its PR POST.
 *
 * The scheduling-capable PAT is admitted only to authenticate its pinned owner
 * and create one PR. All candidate reads, labeling, and readback use the
 * protected workflow token. This script never approves, reviews, merges, tags,
 * publishes, deletes, or retries a mutation.
 */
import { appendFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import {
  RELEASE_PLEASE_BOT_EMAIL,
  RELEASE_PLEASE_BOT_ID,
  RELEASE_PLEASE_BOT_NAME,
  RELEASE_PLEASE_COMMITTER_EMAIL,
  RELEASE_PLEASE_COMMITTER_NAME,
  RELEASE_PLEASE_HEAD_REF,
  RELEASE_PLEASE_PATHS,
  RELEASE_PLEASE_TRANSPORT_ID,
  RELEASE_PLEASE_TRANSPORT_NAME,
  RELEASE_PLEASE_TRANSPORT_TYPE,
  assertReleasePleaseBotIdentity,
  assertReleasePleaseBotPathDiff,
} from './release-please-bot-candidate.mjs';
import {
  RELEASE_PLEASE_PENDING_LABEL,
  deriveReleasePleasePrEnvelope,
} from './release-please-pr-envelope.mjs';

export const FALLBACK_REPOSITORY = 'oyatie/console';
export const FALLBACK_REPOSITORY_ID = 1269693002;
export const FALLBACK_WORKFLOW_ID = 296023729;
export const FALLBACK_WORKFLOW_PATH = '.github/workflows/release-please.yml';
export const FALLBACK_WORKFLOW_NAME = 'Release Please';
export const FALLBACK_HEAD_REF = 'release-please--branches--dev--components--console';
export const FALLBACK_WEB_FLOW_ID = 19864447;
export const FALLBACK_WEB_FLOW_LOGIN = 'web-flow';
export const FALLBACK_WEB_FLOW_TYPE = 'User';

const API_ORIGIN = 'https://api.github.com';
const SHA = /^[0-9a-f]{40}$/;
const MAX_API_BYTES = 8 * 1024 * 1024;
const SNAPSHOT_VERSION = 1;

function fail(message) {
  throw new Error(`release-please PR fallback: ${message}`);
}

function nonEmpty(value, label) {
  if (typeof value !== 'string' || value.length === 0) fail(`${label} must be a non-empty string`);
  return value;
}

function exactSha(value, label) {
  if (!SHA.test(value ?? '')) fail(`${label} must be a lowercase 40-character SHA`);
  return value;
}

function positiveInteger(value, label) {
  const parsed = typeof value === 'string' && /^[1-9][0-9]*$/.test(value) ? Number(value) : value;
  if (!Number.isSafeInteger(parsed) || parsed <= 0) fail(`${label} must be a positive safe integer`);
  return parsed;
}

function exactEnvironment(source = process.env) {
  const sha = exactSha(source.GITHUB_SHA, 'GITHUB_SHA');
  const repository = nonEmpty(source.GITHUB_REPOSITORY, 'GITHUB_REPOSITORY');
  if (repository !== FALLBACK_REPOSITORY) fail('GITHUB_REPOSITORY is not the pinned repository');
  if (String(source.GITHUB_REPOSITORY_ID ?? '') !== String(FALLBACK_REPOSITORY_ID)) {
    fail('GITHUB_REPOSITORY_ID is not the pinned repository id');
  }
  if (source.GITHUB_EVENT_NAME !== 'push' || source.GITHUB_REF !== 'refs/heads/dev') {
    fail('fallback is restricted to a protected dev push');
  }
  if (source.GITHUB_WORKFLOW !== FALLBACK_WORKFLOW_NAME) fail('GITHUB_WORKFLOW is not pinned');
  const workflowRef = `${FALLBACK_REPOSITORY}/${FALLBACK_WORKFLOW_PATH}@refs/heads/dev`;
  if (source.GITHUB_WORKFLOW_REF !== workflowRef || source.GITHUB_WORKFLOW_SHA !== sha) {
    fail('workflow ref/SHA is not bound to the triggering protected dev commit');
  }
  return Object.freeze({
    repository,
    repositoryId: FALLBACK_REPOSITORY_ID,
    sha,
    runId: positiveInteger(source.GITHUB_RUN_ID, 'GITHUB_RUN_ID'),
    runNumber: positiveInteger(source.GITHUB_RUN_NUMBER, 'GITHUB_RUN_NUMBER'),
    runAttempt: positiveInteger(source.GITHUB_RUN_ATTEMPT, 'GITHUB_RUN_ATTEMPT'),
  });
}

function exactToken(value, label) {
  const token = nonEmpty(value, label);
  if (/[\0\r\n]/.test(token)) fail(`${label} contains control bytes`);
  return token;
}

function apiEndpoint(path, parameters = {}) {
  if (typeof path !== 'string' || !path.startsWith('/')) fail('GitHub API path must be absolute');
  const url = new URL(path, API_ORIGIN);
  if (url.origin !== API_ORIGIN || url.username || url.password) fail('GitHub API origin is not pinned');
  for (const [key, value] of Object.entries(parameters)) url.searchParams.set(key, String(value));
  return `${url.pathname}${url.search}`;
}

export async function githubApiRequest({
  token,
  method = 'GET',
  endpoint,
  body,
  expectedStatus = 200,
  fetchImpl = globalThis.fetch,
} = {}) {
  const secret = exactToken(token, 'GitHub API token');
  if (typeof fetchImpl !== 'function') fail('GitHub API fetch adapter is unavailable');
  if (!['GET', 'POST'].includes(method)) fail('GitHub API method is not allowlisted');
  const path = apiEndpoint(endpoint);
  const url = `${API_ORIGIN}${path}`;
  const payload = body === undefined ? undefined : JSON.stringify(body);
  if (payload !== undefined && Buffer.byteLength(payload, 'utf8') > 64 * 1024) {
    fail('GitHub API request body exceeds 65536 bytes');
  }
  let response;
  try {
    response = await fetchImpl(url, {
      method,
      redirect: 'error',
      signal: AbortSignal.timeout(15_000),
      headers: {
        Accept: 'application/vnd.github+json',
        Authorization: `Bearer ${secret}`,
        'User-Agent': 'oyatie-console-release-fallback',
        'X-GitHub-Api-Version': '2022-11-28',
        ...(payload === undefined ? {} : { 'Content-Type': 'application/json' }),
      },
      ...(payload === undefined ? {} : { body: payload }),
    });
  } catch {
    fail(`GitHub API ${method} ${path} transport failed without retry`);
  }
  if (response?.status !== expectedStatus) {
    fail(`GitHub API ${method} ${path} returned ${response?.status ?? 'no status'}; expected ${expectedStatus}`);
  }
  let text;
  try { text = await response.text(); } catch { fail(`GitHub API ${method} ${path} body read failed`); }
  if (Buffer.byteLength(text, 'utf8') > MAX_API_BYTES) fail('GitHub API response exceeds the bounded JSON size');
  try { return JSON.parse(text); } catch { fail(`GitHub API ${method} ${path} returned invalid JSON`); }
}

async function requestJson(request, args) {
  if (typeof request !== 'function') fail('GitHub API request adapter is unavailable');
  return request(args);
}

function assertRun(run, coordinates) {
  if (
    run?.id !== coordinates.runId
    || run?.workflow_id !== FALLBACK_WORKFLOW_ID
    || run?.path !== FALLBACK_WORKFLOW_PATH
    || run?.event !== 'push'
    || run?.head_branch !== 'dev'
    || run?.head_sha !== coordinates.sha
    || run?.run_number !== coordinates.runNumber
    || run?.run_attempt !== coordinates.runAttempt
    || !['queued', 'in_progress'].includes(run?.status)
    || run?.conclusion !== null
    || run?.repository?.id !== FALLBACK_REPOSITORY_ID
    || run?.repository?.full_name !== FALLBACK_REPOSITORY
  ) {
    fail('current protected workflow run coordinates moved or are not active');
  }
}

async function assertSoleActiveRun({ request, token, coordinates }) {
  const current = await requestJson(request, {
    token,
    method: 'GET',
    endpoint: `/repos/${FALLBACK_REPOSITORY}/actions/runs/${coordinates.runId}`,
    expectedStatus: 200,
  });
  assertRun(current, coordinates);
  const active = [];
  for (const status of ['in_progress', 'queued']) {
    const response = await requestJson(request, {
      token,
      method: 'GET',
      endpoint: apiEndpoint(
        `/repos/${FALLBACK_REPOSITORY}/actions/workflows/${FALLBACK_WORKFLOW_ID}/runs`,
        { branch: 'dev', event: 'push', status, per_page: 100 },
      ),
      expectedStatus: 200,
    });
    if (!Number.isSafeInteger(response?.total_count) || response.total_count < 0
      || response.total_count > 100 || !Array.isArray(response?.workflow_runs)
      || response.workflow_runs.length !== response.total_count) {
      fail('active Release Please run inventory is incomplete');
    }
    active.push(...response.workflow_runs);
  }
  const ids = active.map((entry) => entry?.id);
  if (new Set(ids).size !== ids.length || ids.length !== 1 || ids[0] !== coordinates.runId) {
    fail('another active or newer Release Please run competes with the fallback');
  }
  assertRun(active[0], coordinates);
}

async function readRef({ request, token, ref }) {
  const response = await requestJson(request, {
    token,
    method: 'GET',
    endpoint: `/repos/${FALLBACK_REPOSITORY}/git/ref/heads/${encodeURIComponent(ref)}`,
    expectedStatus: 200,
  });
  if (response?.ref !== `refs/heads/${ref}` || response?.object?.type !== 'commit') {
    fail(`ref ${ref} is not an exact commit ref`);
  }
  return exactSha(response.object.sha, `${ref} tip`);
}

async function readOptionalRef({ request, token, ref }) {
  const response = await requestJson(request, {
    token,
    method: 'GET',
    endpoint: `/repos/${FALLBACK_REPOSITORY}/git/matching-refs/heads/${encodeURIComponent(ref)}`,
    expectedStatus: 200,
  });
  if (!Array.isArray(response)) fail(`matching ref inventory for ${ref} is unavailable`);
  if (response.length === 0) return null;
  if (response.length !== 1
    || response[0]?.ref !== `refs/heads/${ref}`
    || response[0]?.object?.type !== 'commit') {
    fail(`matching ref inventory for ${ref} is not exact`);
  }
  return exactSha(response[0].object.sha, `${ref} tip`);
}

async function listReleasePulls({ request, token }) {
  const pulls = await requestJson(request, {
    token,
    method: 'GET',
    endpoint: apiEndpoint(`/repos/${FALLBACK_REPOSITORY}/pulls`, {
      state: 'open',
      base: 'dev',
      head: `oyatie:${FALLBACK_HEAD_REF}`,
      per_page: 100,
    }),
    expectedStatus: 200,
  });
  if (!Array.isArray(pulls) || pulls.length >= 100) fail('open release PR inventory is incomplete');
  if (pulls.length > 1) fail('multiple open release PRs compete for the exact ref');
  return pulls;
}

function canonicalSnapshot(snapshot) {
  if (!snapshot || typeof snapshot !== 'object' || Array.isArray(snapshot)) fail('snapshot is malformed');
  const coordinates = {
    repository: snapshot.repository,
    repositoryId: snapshot.repositoryId,
    sha: exactSha(snapshot.sha, 'snapshot dev SHA'),
    runId: positiveInteger(snapshot.runId, 'snapshot run id'),
    runNumber: positiveInteger(snapshot.runNumber, 'snapshot run number'),
    runAttempt: positiveInteger(snapshot.runAttempt, 'snapshot run attempt'),
  };
  if (snapshot.version !== SNAPSHOT_VERSION || coordinates.repository !== FALLBACK_REPOSITORY
    || coordinates.repositoryId !== FALLBACK_REPOSITORY_ID) {
    fail('snapshot repository/version is not pinned');
  }
  const releaseTip = snapshot.releaseTip === null
    ? null
    : exactSha(snapshot.releaseTip, 'snapshot release ref tip');
  if (!Array.isArray(snapshot.openPullNumbers)
    || !snapshot.openPullNumbers.every((number) => Number.isSafeInteger(number) && number > 0)
    || new Set(snapshot.openPullNumbers).size !== snapshot.openPullNumbers.length
    || snapshot.openPullNumbers.length > 1) {
    fail('snapshot open release PR inventory is malformed');
  }
  return Object.freeze({
    version: SNAPSHOT_VERSION,
    ...coordinates,
    releaseTip,
    openPullNumbers: Object.freeze([...snapshot.openPullNumbers]),
  });
}

export function encodeReleasePleaseSnapshot(snapshot) {
  return Buffer.from(JSON.stringify(canonicalSnapshot(snapshot)), 'utf8').toString('base64url');
}

export function decodeReleasePleaseSnapshot(encoded) {
  if (typeof encoded !== 'string' || encoded.length === 0 || encoded.length > 4096
    || !/^[A-Za-z0-9_-]+$/.test(encoded)) {
    fail('encoded snapshot is missing or noncanonical');
  }
  let parsed;
  try { parsed = JSON.parse(Buffer.from(encoded, 'base64url').toString('utf8')); } catch {
    fail('encoded snapshot is invalid JSON');
  }
  const canonical = canonicalSnapshot(parsed);
  if (encodeReleasePleaseSnapshot(canonical) !== encoded) fail('encoded snapshot is not canonical');
  return canonical;
}

export async function snapshotReleasePleaseState({
  environment = process.env,
  request = githubApiRequest,
} = {}) {
  const coordinates = exactEnvironment(environment);
  const token = exactToken(environment.GITHUB_TOKEN, 'GITHUB_TOKEN');
  await assertSoleActiveRun({ request, token, coordinates });
  const mainTip = await readRef({ request, token, ref: 'dev' });
  if (mainTip !== coordinates.sha) fail('live dev moved before Release Please action execution');
  // GitHub deletes an unprotected release branch when its PR is merged. An
  // absent exact ref is therefore a valid pre-action state; the post-action
  // fallback still requires one newly created exact commit ref.
  const releaseTip = await readOptionalRef({ request, token, ref: FALLBACK_HEAD_REF });
  const pulls = await listReleasePulls({ request, token });
  return canonicalSnapshot({
    version: SNAPSHOT_VERSION,
    ...coordinates,
    releaseTip,
    openPullNumbers: pulls.map((pull) => positiveInteger(pull?.number, 'open release PR number')),
  });
}

function exactTreeEntry(tree, path, expectedTreeSha) {
  if (tree?.sha !== expectedTreeSha || tree?.truncated !== false || !Array.isArray(tree?.tree)) {
    fail('Git root tree response is incomplete');
  }
  const matches = tree.tree.filter((entry) => entry?.path === path);
  if (matches.length !== 1 || matches[0]?.mode !== '100644' || matches[0]?.type !== 'blob') {
    fail(`${path} must be one regular mode-100644 blob`);
  }
  return Object.freeze({
    path,
    mode: matches[0].mode,
    type: matches[0].type,
    sha: exactSha(matches[0].sha, `${path} blob SHA`),
  });
}

function exactBlob(blob, entry, label) {
  if (blob?.sha !== entry.sha || blob?.encoding !== 'base64'
    || !Number.isSafeInteger(blob?.size) || blob.size < 1 || blob.size > MAX_API_BYTES
    || typeof blob?.content !== 'string') {
    fail(`${label} Git blob response is malformed`);
  }
  const encoded = blob.content.replaceAll('\n', '');
  if (!/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(encoded)) {
    fail(`${label} Git blob is not canonical base64`);
  }
  const bytes = Buffer.from(encoded, 'base64');
  if (bytes.length !== blob.size || bytes.toString('base64') !== encoded) {
    fail(`${label} Git blob size/base64 does not round trip`);
  }
  return bytes;
}

function assertValidHostedCommit(commit, sha, label) {
  if (commit?.sha !== sha || commit?.commit?.verification?.verified !== true
    || commit?.commit?.verification?.reason !== 'valid' || !SHA.test(commit?.commit?.tree?.sha ?? '')) {
    fail(`${label} hosted commit/signature is not exact and valid`);
  }
}

async function readReleaseCandidate({ request, token, baseSha, tipSha }) {
  const [baseCommit, headCommit] = await Promise.all([
    requestJson(request, {
      token, method: 'GET', endpoint: `/repos/${FALLBACK_REPOSITORY}/commits/${baseSha}`, expectedStatus: 200,
    }),
    requestJson(request, {
      token, method: 'GET', endpoint: `/repos/${FALLBACK_REPOSITORY}/commits/${tipSha}`, expectedStatus: 200,
    }),
  ]);
  assertValidHostedCommit(baseCommit, baseSha, 'triggering main');
  assertValidHostedCommit(headCommit, tipSha, 'generated release tip');
  if (!Array.isArray(headCommit?.parents) || headCommit.parents.length !== 1
    || headCommit.parents[0]?.sha !== baseSha) {
    fail('generated release tip must have exactly the triggering main parent');
  }
  if (headCommit?.author?.id !== RELEASE_PLEASE_BOT_ID
    || headCommit?.author?.login !== RELEASE_PLEASE_BOT_NAME
    || headCommit?.author?.type !== 'Bot'
    || headCommit?.committer?.id !== FALLBACK_WEB_FLOW_ID
    || headCommit?.committer?.login !== FALLBACK_WEB_FLOW_LOGIN
    || headCommit?.committer?.type !== FALLBACK_WEB_FLOW_TYPE) {
    fail('generated release tip GitHub actor identity is not pinned');
  }
  const subject = headCommit?.commit?.message;
  const identity = {
    authorName: headCommit?.commit?.author?.name,
    authorEmail: headCommit?.commit?.author?.email,
    committerName: headCommit?.commit?.committer?.name,
    committerEmail: headCommit?.commit?.committer?.email,
    subject,
  };
  assertReleasePleaseBotIdentity(identity);
  if (headCommit.commit.message !== subject || subject.includes('\n')) {
    fail('generated release tip must contain only the canonical subject');
  }
  if (!Array.isArray(headCommit.files) || headCommit.files.length !== RELEASE_PLEASE_PATHS.length) {
    fail('generated release tip must expose exactly two changed paths');
  }
  const fileNames = headCommit.files.map((file) => file?.filename).sort();
  if (JSON.stringify(fileNames) !== JSON.stringify([...RELEASE_PLEASE_PATHS].sort())
    || headCommit.files.some((file) => file?.status !== 'modified' || file?.previous_filename !== undefined)) {
    fail('generated release tip changed paths/statuses outside the release core');
  }

  const baseTreeSha = baseCommit.commit.tree.sha;
  const headTreeSha = headCommit.commit.tree.sha;
  const [baseTree, headTree] = await Promise.all([
    requestJson(request, {
      token, method: 'GET', endpoint: `/repos/${FALLBACK_REPOSITORY}/git/trees/${baseTreeSha}`, expectedStatus: 200,
    }),
    requestJson(request, {
      token, method: 'GET', endpoint: `/repos/${FALLBACK_REPOSITORY}/git/trees/${headTreeSha}`, expectedStatus: 200,
    }),
  ]);
  const baseEntries = new Map();
  const headEntries = new Map();
  for (const path of RELEASE_PLEASE_PATHS) {
    baseEntries.set(path, exactTreeEntry(baseTree, path, baseTreeSha));
    headEntries.set(path, exactTreeEntry(headTree, path, headTreeSha));
  }
  const pathChanges = RELEASE_PLEASE_PATHS.map((path) => ({
    path,
    status: 'M',
    oldMode: baseEntries.get(path).mode,
    newMode: headEntries.get(path).mode,
    oldType: baseEntries.get(path).type,
    newType: headEntries.get(path).type,
  }));
  assertReleasePleaseBotPathDiff(pathChanges);

  const blobs = new Map();
  await Promise.all([...baseEntries, ...headEntries].map(async ([path, entry], index) => {
    const blob = await requestJson(request, {
      token, method: 'GET', endpoint: `/repos/${FALLBACK_REPOSITORY}/git/blobs/${entry.sha}`, expectedStatus: 200,
    });
    blobs.set(`${index < RELEASE_PLEASE_PATHS.length ? 'base' : 'head'}:${path}`, exactBlob(
      blob,
      entry,
      `${index < RELEASE_PLEASE_PATHS.length ? 'base' : 'head'} ${path}`,
    ));
  }));
  const envelope = deriveReleasePleasePrEnvelope({
    baseManifest: blobs.get('base:.release-please-manifest.json'),
    headManifest: blobs.get('head:.release-please-manifest.json'),
    baseChangelog: blobs.get('base:CHANGELOG.md'),
    headChangelog: blobs.get('head:CHANGELOG.md'),
    subject,
    headRef: FALLBACK_HEAD_REF,
  });
  return Object.freeze({ envelope, identity, pathChanges });
}

function exactLabels(pr, expectedLabels) {
  if (!Array.isArray(pr?.labels)) fail('PR labels are unavailable');
  const names = pr.labels.map((label) => label?.name).sort();
  if (JSON.stringify(names) !== JSON.stringify([...expectedLabels].sort())) {
    fail('PR labels differ from the exact expected set');
  }
}

function assertCreatedPullRequest(pr, { number, baseSha, headSha, envelope, labels }) {
  const expectedNumber = number === undefined
    ? positiveInteger(pr?.number, 'created PR number')
    : positiveInteger(number, 'expected PR number');
  if (
    pr?.number !== expectedNumber
    || pr?.state !== 'open'
    || pr?.draft !== false
    || pr?.title !== envelope.title
    || pr?.body !== envelope.body
    || pr?.user?.id !== RELEASE_PLEASE_TRANSPORT_ID
    || pr?.user?.login !== RELEASE_PLEASE_TRANSPORT_NAME
    || pr?.user?.type !== RELEASE_PLEASE_TRANSPORT_TYPE
    || pr?.head?.ref !== FALLBACK_HEAD_REF
    || pr?.head?.sha !== headSha
    || pr?.head?.repo?.id !== FALLBACK_REPOSITORY_ID
    || pr?.head?.repo?.full_name !== FALLBACK_REPOSITORY
    || pr?.base?.ref !== 'dev'
    || pr?.base?.sha !== baseSha
    || pr?.base?.repo?.id !== FALLBACK_REPOSITORY_ID
    || pr?.base?.repo?.full_name !== FALLBACK_REPOSITORY
  ) {
    fail('created release PR response/readback does not match exact coordinates');
  }
  exactLabels(pr, labels);
  return expectedNumber;
}

function assertActionFailureEnvelope(environment) {
  if (environment.RELEASE_PLEASE_ACTION_OUTCOME !== 'failure') {
    fail('fallback requires the Release Please action outcome to be failure');
  }
  for (const key of [
    'RELEASE_PLEASE_ACTION_PR',
    'RELEASE_PLEASE_ACTION_PRS',
    'RELEASE_PLEASE_ACTION_RELEASE_CREATED',
    'RELEASE_PLEASE_ACTION_RELEASES_CREATED',
  ]) {
    const value = environment[key] ?? '';
    if (value !== '' && value !== 'false' && value !== '[]') {
      fail(`fallback refuses non-empty action output ${key}`);
    }
  }
}

export async function createReleasePleaseFallbackPr({
  environment = process.env,
  request = githubApiRequest,
} = {}) {
  const coordinates = exactEnvironment(environment);
  assertActionFailureEnvelope(environment);
  const snapshot = decodeReleasePleaseSnapshot(environment.RELEASE_PLEASE_SNAPSHOT ?? '');
  if (JSON.stringify({
    repository: snapshot.repository,
    repositoryId: snapshot.repositoryId,
    sha: snapshot.sha,
    runId: snapshot.runId,
    runNumber: snapshot.runNumber,
    runAttempt: snapshot.runAttempt,
  }) !== JSON.stringify(coordinates)) {
    fail('snapshot coordinates do not equal the current workflow coordinates');
  }
  if (snapshot.openPullNumbers.length !== 0) {
    fail('fallback cannot create a PR when one existed before the action');
  }
  const workflowToken = exactToken(environment.GITHUB_TOKEN, 'GITHUB_TOKEN');
  const transportToken = exactToken(environment.CONSOLE_RELEASE_PUSH_TOKEN, 'CONSOLE_RELEASE_PUSH_TOKEN');
  if (workflowToken === transportToken) fail('transport token must differ from GITHUB_TOKEN');

  await assertSoleActiveRun({ request, token: workflowToken, coordinates });
  const mainTip = await readRef({ request, token: workflowToken, ref: 'dev' });
  if (mainTip !== coordinates.sha) fail('live dev moved after the action');
  const newTip = await readRef({ request, token: workflowToken, ref: FALLBACK_HEAD_REF });
  if (newTip === snapshot.releaseTip) fail('Release Please ref did not advance after the action failure');
  if ((await listReleasePulls({ request, token: workflowToken })).length !== 0) {
    fail('a competing release PR appeared after the action');
  }
  const { envelope } = await readReleaseCandidate({
    request,
    token: workflowToken,
    baseSha: coordinates.sha,
    tipSha: newTip,
  });

  const principal = await requestJson(request, {
    token: transportToken,
    method: 'GET',
    endpoint: '/user',
    expectedStatus: 200,
  });
  if (principal?.id !== RELEASE_PLEASE_TRANSPORT_ID
    || principal?.login !== RELEASE_PLEASE_TRANSPORT_NAME
    || principal?.type !== RELEASE_PLEASE_TRANSPORT_TYPE) {
    fail('transport token principal is not the pinned release PR creator');
  }

  // Sole fallback mutation with the PAT. Deliberately no retry or cleanup write.
  const created = await requestJson(request, {
    token: transportToken,
    method: 'POST',
    endpoint: `/repos/${FALLBACK_REPOSITORY}/pulls`,
    expectedStatus: 201,
    body: {
      title: envelope.title,
      head: FALLBACK_HEAD_REF,
      base: 'dev',
      body: envelope.body,
      draft: false,
    },
  });
  const prNumber = assertCreatedPullRequest(created, {
    baseSha: coordinates.sha,
    headSha: newTip,
    envelope,
    labels: [],
  });

  const labelResponse = await requestJson(request, {
    token: workflowToken,
    method: 'POST',
    endpoint: `/repos/${FALLBACK_REPOSITORY}/issues/${prNumber}/labels`,
    expectedStatus: 200,
    body: { labels: [RELEASE_PLEASE_PENDING_LABEL] },
  });
  if (!Array.isArray(labelResponse)
    || JSON.stringify(labelResponse.map((label) => label?.name).sort())
      !== JSON.stringify([RELEASE_PLEASE_PENDING_LABEL])) {
    fail('protected-token pending-label response is not exact');
  }

  const reread = await requestJson(request, {
    token: workflowToken,
    method: 'GET',
    endpoint: `/repos/${FALLBACK_REPOSITORY}/pulls/${prNumber}`,
    expectedStatus: 200,
  });
  assertCreatedPullRequest(reread, {
    number: prNumber,
    baseSha: coordinates.sha,
    headSha: newTip,
    envelope,
    labels: [RELEASE_PLEASE_PENDING_LABEL],
  });
  const pulls = await listReleasePulls({ request, token: workflowToken });
  if (pulls.length !== 1 || pulls[0]?.number !== prNumber) {
    fail('post-create open release PR inventory is not exact');
  }
  assertCreatedPullRequest(pulls[0], {
    number: prNumber,
    baseSha: coordinates.sha,
    headSha: newTip,
    envelope,
    labels: [RELEASE_PLEASE_PENDING_LABEL],
  });
  if ((await readRef({ request, token: workflowToken, ref: 'dev' })) !== coordinates.sha
    || (await readRef({ request, token: workflowToken, ref: FALLBACK_HEAD_REF })) !== newTip) {
    fail('dev or release ref moved during fallback readback');
  }
  await assertSoleActiveRun({ request, token: workflowToken, coordinates });

  return Object.freeze({
    pr: JSON.stringify({
      headBranchName: envelope.headBranchName,
      baseBranchName: envelope.baseBranchName,
      number: prNumber,
      title: envelope.title,
      body: envelope.body,
      labels: [...envelope.labels],
      files: [],
    }),
    prNumber,
    headSha: newTip,
    parentSha: coordinates.sha,
  });
}

function emptyOutput(value) {
  return value === undefined || value === '' || value === 'false' || value === '[]';
}

export function selectReleasePleaseResult({
  snapshotOutcome,
  releaseOutcome,
  releasePr,
  releaseCreated,
  releasesCreated,
  fallbackOutcome,
  fallbackPr,
} = {}) {
  if (snapshotOutcome !== 'success') fail('pre-action snapshot did not succeed');
  const nativePr = releasePr ?? '';
  const createdRelease = releaseCreated === 'true';
  if (releaseOutcome === 'success') {
    if (fallbackOutcome !== 'skipped' || !emptyOutput(fallbackPr)) {
      fail('fallback must stay skipped after native action success');
    }
    if (nativePr !== '') {
      if (createdRelease || !emptyOutput(releasesCreated)) {
        fail('native action cannot emit PR and release results together');
      }
      return Object.freeze({ mode: 'pr', pr: nativePr });
    }
    return Object.freeze({ mode: createdRelease ? 'release' : 'noop', pr: '' });
  }
  if (releaseOutcome === 'failure') {
    if (!emptyOutput(nativePr) || createdRelease || !emptyOutput(releasesCreated)
      || fallbackOutcome !== 'success' || typeof fallbackPr !== 'string' || fallbackPr.length === 0) {
      fail('native action failure lacks one exact fallback PR result');
    }
    return Object.freeze({ mode: 'fallback-pr', pr: fallbackPr });
  }
  fail('Release Please action outcome is neither success nor handled failure');
}

export function finalizeReleasePleaseResult({
  selectionOutcome,
  mode,
  selectedPr,
  convergeOutcome,
  prNumber,
  headSha,
  parentSha,
  expectedParentSha,
} = {}) {
  if (selectionOutcome !== 'success') fail('result selector did not succeed');
  if (mode === 'pr' || mode === 'fallback-pr') {
    if (typeof selectedPr !== 'string' || selectedPr.length === 0 || convergeOutcome !== 'success') {
      fail('selected release PR did not complete custody convergence');
    }
    const number = positiveInteger(prNumber, 'final PR number');
    const head = exactSha(headSha, 'final release head SHA');
    const parent = exactSha(parentSha, 'final release parent SHA');
    if (parent !== exactSha(expectedParentSha, 'expected final parent SHA') || head === parent) {
      fail('final release proof coordinates do not bind the triggering main SHA');
    }
    return Object.freeze({ prNumber: number, headSha: head, parentSha: parent });
  }
  if (!['release', 'noop'].includes(mode) || !emptyOutput(selectedPr)
    || convergeOutcome !== 'skipped' || !emptyOutput(prNumber)
    || !emptyOutput(headSha) || !emptyOutput(parentSha)) {
    fail('non-PR action result has inconsistent convergence outputs');
  }
  return Object.freeze({ prNumber: '', headSha: '', parentSha: '' });
}

function writeOutputs(values) {
  const output = nonEmpty(process.env.GITHUB_OUTPUT, 'GITHUB_OUTPUT');
  const lines = Object.entries(values).map(([key, value]) => {
    const rendered = String(value);
    if (rendered.includes('\n') || rendered.includes('\r')) fail(`output ${key} is not single-line`);
    return `${key}=${rendered}\n`;
  }).join('');
  appendFileSync(output, lines, 'utf8');
}

function redact(error) {
  let message = error instanceof Error ? error.stack || error.message : String(error);
  for (const token of [process.env.GITHUB_TOKEN, process.env.CONSOLE_RELEASE_PUSH_TOKEN]) {
    if (typeof token === 'string' && token.length > 0) message = message.replaceAll(token, '[REDACTED]');
  }
  return message;
}

async function cli() {
  const mode = process.argv[2];
  if (mode === 'snapshot') {
    writeOutputs({ snapshot: encodeReleasePleaseSnapshot(await snapshotReleasePleaseState()) });
    return;
  }
  if (mode === 'fallback') {
    const result = await createReleasePleaseFallbackPr();
    writeOutputs({ pr: result.pr, pr_number: result.prNumber, head_sha: result.headSha, parent_sha: result.parentSha });
    return;
  }
  if (mode === 'select') {
    const result = selectReleasePleaseResult({
      snapshotOutcome: process.env.RELEASE_PLEASE_SNAPSHOT_OUTCOME,
      releaseOutcome: process.env.RELEASE_PLEASE_ACTION_OUTCOME,
      releasePr: process.env.RELEASE_PLEASE_ACTION_PR,
      releaseCreated: process.env.RELEASE_PLEASE_ACTION_RELEASE_CREATED,
      releasesCreated: process.env.RELEASE_PLEASE_ACTION_RELEASES_CREATED,
      fallbackOutcome: process.env.RELEASE_PLEASE_FALLBACK_OUTCOME,
      fallbackPr: process.env.RELEASE_PLEASE_FALLBACK_PR,
    });
    writeOutputs(result);
    return;
  }
  if (mode === 'finalize') {
    const result = finalizeReleasePleaseResult({
      selectionOutcome: process.env.RELEASE_PLEASE_SELECTION_OUTCOME,
      mode: process.env.RELEASE_PLEASE_SELECTION_MODE,
      selectedPr: process.env.RELEASE_PLEASE_SELECTED_PR,
      convergeOutcome: process.env.RELEASE_PLEASE_CONVERGE_OUTCOME,
      prNumber: process.env.RELEASE_PLEASE_CONVERGE_PR_NUMBER,
      headSha: process.env.RELEASE_PLEASE_CONVERGE_HEAD_SHA,
      parentSha: process.env.RELEASE_PLEASE_CONVERGE_PARENT_SHA,
      expectedParentSha: process.env.GITHUB_SHA,
    });
    writeOutputs({
      pr_number: result.prNumber,
      head_sha: result.headSha,
      parent_sha: result.parentSha,
    });
    return;
  }
  fail('usage: release-please-pr-fallback.mjs snapshot|fallback|select|finalize');
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    await cli();
  } catch (error) {
    console.error(redact(error));
    process.exitCode = 1;
  }
}
