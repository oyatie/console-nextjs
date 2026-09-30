/**
 * Fail-closed admission class for release-please bot candidates.
 *
 * Ordinary PR admission no longer uses a C/T signing train. Release-shaped PRs
 * remain a distinct protected class because their automation provenance cannot
 * be inferred from commit headers alone. This module classifies ONLY tips that
 * satisfy every clause below — never a broad bot/ref exception:
 *
 * 1. Exact github-actions[bot] author + GitHub noreply committer
 * 2. Subject `chore(<scope>): release X.Y.Z`
 * 3. Parent..tip MUST change `.release-please-manifest.json` + `CHANGELOG.md`,
 *    and MAY also change the documentation custody pair
 *    (`docs/documentation-manifest.seed.json` + `docs/documentation-index.json`)
 *    so CHANGELOG blob_sha can converge. All are regular mode-100644
 *    modifications (no adds/deletes/renames). Custody paths are all-or-nothing.
 * 4. On the pull_request_target bootstrap path: the same-repository head/ref
 *    shape is combined with a native proof job from the pinned protected
 *    Release Please run at the tip's parent. Event sender and commit headers are
 *    not sufficient authority (a PAT transport push is human-owned).
 */

import { AUTHORITY_DIFF_ARGS } from './authority-ledger-path.mjs';

export const RELEASE_PLEASE_BOT_NAME = 'github-actions[bot]';
export const RELEASE_PLEASE_BOT_ID = 41898282;
export const RELEASE_PLEASE_BOT_EMAIL = '41898282+github-actions[bot]@users.noreply.github.com';
export const RELEASE_PLEASE_COMMITTER_NAME = 'GitHub';
export const RELEASE_PLEASE_COMMITTER_EMAIL = 'noreply@github.com';
export const RELEASE_PLEASE_TRANSPORT_NAME = 'jason931225';
export const RELEASE_PLEASE_TRANSPORT_ID = 56489493;
export const RELEASE_PLEASE_TRANSPORT_TYPE = 'User';
export const RELEASE_PLEASE_PR_AUTHORS = Object.freeze(['github-actions[bot]']);
export const RELEASE_PLEASE_HEAD_REF = /^release-please--branches--dev--components--[A-Za-z0-9._-]+$/;
export const RELEASE_PLEASE_SUBJECT = /^chore\([^)]+\): release \d+\.\d+\.\d+$/;
export const RELEASE_PLEASE_PATHS = Object.freeze(['.release-please-manifest.json', 'CHANGELOG.md']);
export const RELEASE_PLEASE_CUSTODY_PATHS = Object.freeze([
  'docs/documentation-manifest.seed.json',
  'docs/documentation-index.json',
]);
export const RELEASE_PLEASE_TRAIN_CLASS = 'release-please-bot';

const SHA = /^[0-9a-f]{40}$/;

function fail(message) {
  throw new Error(`release-please bot candidate: ${message}`);
}

/** Classify creator metadata; protected workflow proof remains mandatory. */
export function releasePleasePrCreatorClass(creator = {}) {
  if (!creator || typeof creator !== 'object' || Array.isArray(creator)) return null;
  const { id, login } = creator;
  if (id === RELEASE_PLEASE_BOT_ID && login === RELEASE_PLEASE_BOT_NAME) return 'bot';
  if (id === RELEASE_PLEASE_TRANSPORT_ID && login === RELEASE_PLEASE_TRANSPORT_NAME) {
    return 'transport';
  }
  return null;
}

// Compatibility validator for callers that explicitly request event metadata.
// The protected-target admission does not treat this actor check as proof; it
// requires the pinned native workflow proof in its own protected module.
export function assertTrustedReleasePleasePrMeta({
  prAuthorLogin,
  prHeadRef,
  eventSenderLogin,
  prHeadRepository,
  repository,
} = {}) {
  if (!RELEASE_PLEASE_PR_AUTHORS.includes(prAuthorLogin)) {
    fail('PR author must be github-actions[bot] (event payload, not commit header)');
  }
  if (!RELEASE_PLEASE_PR_AUTHORS.includes(eventSenderLogin)) {
    fail('event sender must be github-actions[bot] for the current release tip');
  }
  if (typeof repository !== 'string' || repository.length === 0
    || typeof prHeadRepository !== 'string' || prHeadRepository !== repository) {
    fail('PR head repository must equal the protected repository');
  }
  if (typeof prHeadRef !== 'string' || !RELEASE_PLEASE_HEAD_REF.test(prHeadRef)) {
    fail('PR head ref must match release-please--branches--dev--components--*');
  }
}

export function assertReleasePleaseBotIdentity(identity) {
  if (!identity || typeof identity !== 'object') fail('commit identity is unavailable');
  const { authorName, authorEmail, committerName, committerEmail, subject } = identity;
  if (authorName !== RELEASE_PLEASE_BOT_NAME || authorEmail !== RELEASE_PLEASE_BOT_EMAIL) {
    fail('tip author must be exactly github-actions[bot]');
  }
  if (committerName !== RELEASE_PLEASE_COMMITTER_NAME || committerEmail !== RELEASE_PLEASE_COMMITTER_EMAIL) {
    fail('tip committer must be exactly GitHub <noreply@github.com>');
  }
  if (typeof subject !== 'string' || !RELEASE_PLEASE_SUBJECT.test(subject)) {
    fail('tip subject must match chore(<scope>): release X.Y.Z');
  }
}

/**
 * Decide whether converge must rewrite the tip.
 * - noop: exact bot identity and custody already clean
 * - rewrite/custody: identity OK but custody pair dirty
 * Poisoned identity, path sprawl, or a bad subject always throw before mutation.
 */
export function releasePleaseCustodyRewritePlan({ identity, pathChanges, dirtyCustodyPaths } = {}) {
  if (!identity || typeof identity !== 'object') fail('commit identity is unavailable');
  assertReleasePleaseBotIdentity(identity);
  assertReleasePleaseBotPathDiff(pathChanges);
  if (!Array.isArray(dirtyCustodyPaths)) fail('dirty custody path list is unavailable');
  for (const path of dirtyCustodyPaths) {
    if (!RELEASE_PLEASE_CUSTODY_PATHS.includes(path)) {
      fail(`unexpected dirty path outside custody pair: ${path}`);
    }
  }
  if (dirtyCustodyPaths.length === 0) {
    return Object.freeze({ action: 'noop' });
  }
  return Object.freeze({ action: 'rewrite', reason: 'custody' });
}

export function assertReleasePleaseBotPathDiff(changes) {
  if (!Array.isArray(changes)) fail('parent..tip diff is unavailable');
  const required = new Set(RELEASE_PLEASE_PATHS);
  const custody = new Set(RELEASE_PLEASE_CUSTODY_PATHS);
  const allowed = new Set([...required, ...custody]);
  const seen = new Set();
  for (const change of changes) {
    if (!change || typeof change.path !== 'string') fail('parent..tip diff entry is malformed');
    if (!allowed.has(change.path) || seen.has(change.path)) {
      fail(
        'parent..tip may change only .release-please-manifest.json, CHANGELOG.md, and the documentation custody pair',
      );
    }
    if (change.status !== 'M' || change.oldMode !== '100644' || change.newMode !== '100644') {
      fail('release paths must be regular mode-100644 modifications');
    }
    if (change.oldType !== undefined && change.oldType !== 'blob') fail('release paths must remain blobs');
    if (change.newType !== undefined && change.newType !== 'blob') fail('release paths must remain blobs');
    seen.add(change.path);
  }
  for (const path of required) {
    if (!seen.has(path)) {
      fail('parent..tip must change both .release-please-manifest.json and CHANGELOG.md');
    }
  }
  const custodySeen = [...custody].filter((path) => seen.has(path));
  if (custodySeen.length !== 0 && custodySeen.length !== custody.size) {
    fail('documentation custody paths must be regenerated together');
  }
}

/** Pure seam: ops supplies identity + parent..tip diff. */
export function classifyReleasePleaseBotTip(ops, tipSha) {
  if (!SHA.test(tipSha ?? '')) return null;
  if (!ops.hasCommit(tipSha)) return null;
  const parents = ops.parents(tipSha);
  if (!Array.isArray(parents) || parents.length !== 1 || !SHA.test(parents[0])) return null;
  const parentSha = parents[0];
  let identity;
  try { identity = ops.commitIdentity(tipSha); } catch { return null; }
  try { assertReleasePleaseBotIdentity(identity); } catch { return null; }
  let changes;
  try { changes = ops.diff(parentSha, tipSha); } catch { return null; }
  try { assertReleasePleaseBotPathDiff(changes); } catch { return null; }
  return Object.freeze({
    trainClass: RELEASE_PLEASE_TRAIN_CLASS,
    candidateSha: parentSha,
    authorityTipSha: tipSha,
    identity: Object.freeze({ ...identity }),
  });
}

/**
 * Historical regression seam for a release-please tip that already landed as a
 * one-parent main squash S. It recognizes S as a tree-bound candidate
 * when a previously classifiable tip T0 still resolves with:
 *   parents(S) === [T0.candidateSha] && tree(S) === tree(T0)
 *
 * Fail-closed: a tip off main, wrong identity, path sprawl, missing T0, or
 * tree/parent drift returns null. This is not live ordinary-PR admission.
 */
export function classifyReleasePleaseSquashBinding(ops, squashSha) {
  if (!SHA.test(squashSha ?? '')) return null;
  if (!ops.hasCommit(squashSha)) return null;
  const parents = ops.parents(squashSha);
  if (!Array.isArray(parents) || parents.length !== 1 || !SHA.test(parents[0])) return null;
  const preMergeBaseSha = parents[0];

  // Protected history only — a PR-local object must not look like a landed squash.
  if (typeof ops.mainTip !== 'function' || typeof ops.isAncestor !== 'function') return null;
  let mainTip;
  try { mainTip = ops.mainTip(); } catch { return null; }
  if (!SHA.test(mainTip ?? '') || !ops.isAncestor(squashSha, mainTip)) return null;

  let tipSha = null;
  if (typeof ops.releasePleasePreMergeTip === 'function') {
    try { tipSha = ops.releasePleasePreMergeTip(squashSha); } catch { return null; }
  }
  if (!tipSha) {
    let identity;
    try { identity = ops.commitIdentity(squashSha); } catch { return null; }
    const prMatch = typeof identity?.subject === 'string'
      ? identity.subject.match(/^chore\([^)]+\): release \d+\.\d+\.\d+ \(#(\d+)\)$/)
      : null;
    if (!prMatch || typeof ops.pullHead !== 'function') return null;
    try { tipSha = ops.pullHead(prMatch[1]); } catch { return null; }
  }
  if (!SHA.test(tipSha ?? '') || tipSha === squashSha || !ops.hasCommit(tipSha)) return null;

  const classified = classifyReleasePleaseBotTip(ops, tipSha);
  if (!classified) return null;
  if (classified.candidateSha !== preMergeBaseSha) return null;
  if (ops.tree(squashSha) !== ops.tree(tipSha) || !ops.sameTreeDiff(squashSha, tipSha)) return null;

  return Object.freeze({
    trainClass: RELEASE_PLEASE_TRAIN_CLASS,
    squashSha,
    preMergeBaseSha,
    releasePleaseTipSha: tipSha,
    admittedCandidateSha: squashSha,
    candidateSha: classified.candidateSha,
    authorityTipSha: tipSha,
    identity: classified.identity,
  });
}

/**
 * Admit a release-please bot tip as the authority tip of a structural M.
 * When `requirePrMeta` is true, legacy event author/ref checks are mandatory.
 * Protected-target admission uses `requirePrMeta: false` only after validating
 * its stronger pinned native workflow proof.
 */
export function verifyReleasePleaseBotTrain(ops, {
  headSha,
  mergeSha,
  prAuthorLogin,
  prHeadRef,
  eventSenderLogin,
  prHeadRepository,
  repository,
  requirePrMeta = false,
}) {
  if (!SHA.test(headSha ?? '')) fail('PR head must be a lowercase 40-character SHA');
  if (!SHA.test(mergeSha ?? '')) fail('PR merge must be a lowercase 40-character SHA');
  const classified = classifyReleasePleaseBotTip(ops, headSha);
  if (!classified) fail('tip is not a release-please bot docs-only candidate');
  if (requirePrMeta) {
    assertTrustedReleasePleasePrMeta({
      prAuthorLogin,
      prHeadRef,
      eventSenderLogin,
      prHeadRepository,
      repository,
    });
  }
  if (!ops.hasCommit(mergeSha)) fail('PR merge object is unavailable');
  const mergeParents = ops.parents(mergeSha);
  if (!Array.isArray(mergeParents) || mergeParents.length !== 2 || mergeParents[1] !== headSha) {
    fail('M must be a two-parent merge whose second parent is the release tip');
  }
  if (ops.tree(mergeSha) !== ops.tree(headSha) || !ops.sameTreeDiff(mergeSha, headSha)) {
    fail('M tree/diff must equal the release tip exactly');
  }
  return Object.freeze({
    trainClass: RELEASE_PLEASE_TRAIN_CLASS,
    candidateSha: classified.candidateSha,
    integrationTipSha: headSha,
    authorityTipSha: headSha,
    mergeSha,
  });
}

/** Git-backed helpers for the authority-train / CI path (real repository). */
export function gitCommitIdentity(repoRoot, sha, git) {
  const raw = git(repoRoot, ['show', '-s', '--format=%an%x00%ae%x00%cn%x00%ce%x00%s', sha]);
  const [authorName, authorEmail, committerName, committerEmail, subject] = raw.replace(/\n$/, '').split('\0');
  return { authorName, authorEmail, committerName, committerEmail, subject };
}

export function gitRawDiff(repoRoot, from, to, git) {
  const fields = git(repoRoot, [...AUTHORITY_DIFF_ARGS, from, to]).split('\0');
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

export function gitOpsForReleasePlease(repoRoot, git, gitSucceeds) {
  return {
    hasCommit: (sha) => gitSucceeds(repoRoot, ['cat-file', '-e', `${sha}^{commit}`]),
    parents: (sha) => git(repoRoot, ['show', '-s', '--format=%P', sha]).trim().split(/\s+/).filter(Boolean),
    commitIdentity: (sha) => gitCommitIdentity(repoRoot, sha, git),
    diff: (from, to) => gitRawDiff(repoRoot, from, to, git),
    tree: (sha) => git(repoRoot, ['show', '-s', '--format=%T', sha]).trim(),
    sameTreeDiff: (left, right) => {
      try { git(repoRoot, ['diff', '--quiet', '--no-ext-diff', left, right]); return true; } catch { return false; }
    },
    mainTip: () => {
      // Prefer remotes: shared hubs often leave refs/heads/dev stale across worktrees.
      for (const ref of ['refs/remotes/origin/dev', 'origin/dev', 'refs/heads/dev']) {
        try {
          const tip = git(repoRoot, ['rev-parse', ref]).trim();
          if (SHA.test(tip)) return tip;
        } catch { /* try next */ }
      }
      fail('protected dev tip is unresolvable');
    },
    isAncestor: (ancestor, descendant) => gitSucceeds(repoRoot, ['merge-base', '--is-ancestor', ancestor, descendant]),
    pullHead: (number) => {
      const parsed = String(number);
      if (!/^\d+$/.test(parsed)) fail('PR number is invalid');
      const ref = `refs/console-release-squash-binding/${parsed}/head`;
      git(repoRoot, ['fetch', '--no-tags', '--no-recurse-submodules', 'origin', `+refs/pull/${parsed}/head:${ref}`]);
      const tip = git(repoRoot, ['rev-parse', ref]).trim();
      if (!SHA.test(tip)) fail('release-please pre-merge tip is unresolvable');
      return tip;
    },
  };
}
