import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { buildFanoutPlan, normalizePattern, patternsOverlap } from './plan-fanout.mjs';
import { installGitFixtureEnvironment } from '../lib/git-fixture-environment.mjs';

installGitFixtureEnvironment();

const SHA = 'a'.repeat(40);
const resources = { writer: 1, postgres: 0, browser: 0, ios: 0, graph: 1, cas: 1 };
function cap(id, roots, patch = {}) { return { id, label: id, priority: { score: .7, inputs: { correctness_and_risk_reduction: .8, verification_readiness: .9 } }, dependencies: [], owner: `owner-${id}`, worktree: `/tmp/${id}`, branch: `codex/${id}`, ownership: { frontend_roots: roots.filter((root) => root.startsWith('web/')), backend_roots: roots.filter((root) => root.startsWith('backend/')), api_schema_roots: [], migration_owner: 'not_applicable', integration_owner: 'console-consolidation' }, signature_story: { id: `STORY-${id}`, outcome: id }, evidence_path: `docs/evidence/${id}`, tests: { leaf_commands: ['git diff --check'], buck2_targets: roots.some((root) => root.startsWith('backend/')) ? ['//backend/example:test'] : [] }, resource_requirements: resources, state: { backend: roots.some((root) => root.startsWith('backend/')) ? 'writer_assigned_in_progress' : 'not_applicable', frontend: roots.some((root) => root.startsWith('web/')) ? 'writer_assigned_in_progress' : 'not_applicable', independent_review: 'missing', production_exposure: 'dark' }, ...patch }; }
function reg(capabilities, patch = {}) { return { schema_version: 'console-capability-registry-v1', source_revision: `git@${SHA}`, resource_budgets: { writer: 3, postgres: 1, browser: 1, ios: 1, graph: 3, cas: 3 }, shared_collision_roots: { owner: 'console-consolidation', generated_face_registry: 'tools/buck/generated_face_registry.json', paths: ['backend/openapi/openapi.yaml', 'backend/crates/platform/db/migrations/**', 'web/src/console/screens/registry.ts'] }, capabilities, ...patch }; }
const faces = { schema_version: 2, faces: [{ id: 'buck', output_patterns: ['clients/ts/src/schema.d.ts'] }] };
function plan(capabilities, options = {}) { return buildFanoutPlan(reg(capabilities), { anchorSha: SHA, maxWriters: 3, qualityBias: .6, generatedFaces: faces, ...options }); }

test('only literal paths plus terminal subtree suffix are canonical ownership syntax', () => {
  assert.equal(normalizePattern('backend/crates/a/**'), 'backend/crates/a/**');
  for (const bad of ['foo*/**', 'foobar*', '.', '/.', '..', 'a/../b', './a', 'a/**/b']) assert.throws(() => normalizePattern(bad));
  assert.equal(patternsOverlap('backend/crates/a/**', 'backend/crates/a/x'), true);
  assert.equal(patternsOverlap('backend/crates/a/**', 'backend/crates/ab/**'), false);
});

test('unsupported authority intersections fail closed', () => {
  assert.throws(() => plan([cap('A', ['backend/crates/a/**'])], { generatedFaces: { schema_version: 2, faces: [{ output_patterns: ['foo*'] }] } }));
  assert.throws(() => plan([cap('A', ['backend/crates/a/**'])], { generatedFaces: null }));
});

test('migration roots are excluded from leaves and serialised', () => {
  const value = plan([cap('A', ['backend/crates/a/**', 'backend/crates/platform/db/migrations/**'])]);
  assert.equal(value.selected.length, 0);
  assert.match(value.held[0].reasons.join(','), /protected_shared_root_intersection/);
});

test('per-lane resource declarations are mandatory while source admission creates no verification jobs', () => {
  const missing = cap('MISSING', ['backend/crates/missing/**'], { resource_requirements: undefined });
  assert.match(plan([missing]).held[0].reasons.join(','), /invalid_lane_resource_requirements/);
  const first = cap('A', ['backend/crates/a/**'], { resource_requirements: { ...resources, postgres: 1 } });
  const second = cap('B', ['backend/crates/b/**'], { resource_requirements: { ...resources, postgres: 1 } });
  const value = plan([first, second]);
  assert.equal(value.selected.length, 2);
  assert.deepEqual(value.verification_queue, []);
});

test('cold Rust capacity does not create expensive jobs from selected source writers', () => {
  const backendA = cap('RUST-A', ['backend/crates/rust-a/**']);
  const backendB = cap('RUST-B', ['backend/crates/rust-b/**']);
  const backendC = cap('RUST-C', ['backend/crates/rust-c/**']);
  const frontend = cap('WEB', ['web/console/**']);
  const value = buildFanoutPlan(reg([backendA, backendB, backendC, frontend], { resource_budgets: { writer: 4, postgres: 1, browser: 1, ios: 1, graph: 4, cas: 4 } }), { anchorSha: SHA, maxWriters: 4, qualityBias: .6, generatedFaces: faces });
  assert.deepEqual(value.selected.map((lane) => lane.lane_id).sort(), ['RUST-A', 'RUST-B', 'RUST-C', 'WEB']);
  assert.deepEqual(value.verification_queue, []);
});

test('same writer, worktree, or branch cannot be co-selected', () => {
  const a = cap('A', ['backend/crates/a/**']); const b = cap('B', ['backend/crates/b/**'], { owner: a.owner });
  const value = plan([a, b]);
  assert.equal(value.selected.length, 0);
  assert.equal(value.held.filter((entry) => entry.reasons.includes('duplicate_owner_within_epoch')).length, 2);
});

test('completed source cannot bypass invalid roots or exact review receipts', () => {
  const complete = cap('DONE', ['backend/crates/done/**'], { state: { backend: 'complete', frontend: 'not_applicable' } });
  const value = plan([complete]);
  assert.deepEqual(value.completed_source_capabilities, []);
  assert.match(value.held[0].reasons.join(','), /completed_source_missing_exact_leaf_review_receipts/);
  const forgedReceipt = { status: 'approved', epoch_base_sha: SHA, lane_id: 'DONE', implementer: complete.owner, reviewer: 'reviewer-b', leaf_commit: 'b'.repeat(40), review_commit: 'c'.repeat(40), leaf_result_sha256: 'd'.repeat(64) };
  const forged = plan([complete], { admissionReceipts: { DONE: forgedReceipt }, runtimeReviewEligibility: {} });
  assert.deepEqual(forged.completed_source_capabilities, []);
  assert.deepEqual(forged.verification_queue, []);
  const invalid = cap('BAD', ['backend/x*/**'], { state: { backend: 'complete', frontend: 'not_applicable' } });
  assert.throws(() => plan([invalid]));
});

test('completed source checks unowned private roots before its review shortcut', () => {
  const complete = cap('DONE', ['backend/crates/done/**'], { state: { backend: 'complete', frontend: 'not_applicable' } });
  complete.lane_assignments = { source: { owner: complete.owner, worktree: complete.worktree, branch: complete.branch, roots: ['docs/evidence/DONE/**'], resources, tests: complete.tests } };
  const value = plan([complete]);
  assert.match(value.held[0].reasons.join(','), /unassigned_private_ownership_roots/);
});

test('consolidation is blocked until exact independent review and valid consolidation identity', () => {
  const source = cap('A', ['backend/crates/a/**', 'backend/openapi/openapi.yaml']);
  source.lane_assignments = { source: { owner: source.owner, worktree: source.worktree, branch: source.branch, roots: ['backend/crates/a/**'], resources, tests: source.tests }, consolidation: { owner: 'console-consolidation', worktree: '/tmp/consolidation', branch: 'codex/consolidation', resources } };
  const value = plan([source]);
  assert.equal(value.consolidation_queue[0].ready_after_leaf_review, false);
  assert.match(value.consolidation_queue[0].review_prerequisites.join(','), /exact_leaf_review_receipts_required/);
});

test('quality-weighted maximal independent set is deterministic', () => {
  const economy = cap('ECONOMY', ['backend/crates/shared/**'], { priority: { score: .9, inputs: { correctness_and_risk_reduction: .2, verification_readiness: .2 } } });
  const quality = cap('QUALITY', ['backend/crates/shared/child/**'], { priority: { score: .65, inputs: { correctness_and_risk_reduction: 1, verification_readiness: 1 } } });
  const first = plan([economy, quality], { maxWriters: 1 }); const second = plan([quality, economy], { maxWriters: 1 });
  assert.deepEqual(first.selected.map((entry) => entry.lane_id), ['QUALITY']);
  assert.equal(JSON.stringify(first), JSON.stringify(second));
});

test('Buck isolation directories include a stable full-lane hash and reject caller-controlled traversal', async () => {
  const { buckIsolationDir } = await import('./plan-fanout.mjs');
  assert.equal(buckIsolationDir(SHA, 'A#backend'), buckIsolationDir(SHA, 'A#backend'));
  assert.notEqual(buckIsolationDir(SHA, 'A#backend'), buckIsolationDir(SHA, 'A#frontend'));
  assert.match(buckIsolationDir(SHA, 'A#backend'), /^\.buck2\/console-epochs\/[a-f0-9]{12}\/[a-z0-9-]+-[a-f0-9]{64}$/);
  const prefix = 'A'.repeat(90);
  assert.notEqual(buckIsolationDir(SHA, `${prefix}#one`), buckIsolationDir(SHA, `${prefix}#two`));
  assert.throws(() => buckIsolationDir(SHA, '../escape'));
});

test('review receipts fail closed unless an independent reviewer custody-binds the exact leaf result', async () => {
  const { validateReviewReceiptForAnchor, leafResultDigest } = await import('./plan-fanout.mjs');
  const LEAF = 'b'.repeat(40); const REVIEW = 'c'.repeat(40); const OTHER = 'd'.repeat(40);
  const lane = { laneId: 'A', owner: 'writer-a', privateRoots: ['src/a/**'], protectedRoots: ['shared/**'] };
  const authority = { reviewers: [{ id: 'reviewer-b', author_name: 'Reviewer', author_email: 'review@example.test', committer_name: 'Reviewer', committer_email: 'review@example.test', signing_fingerprint: 'ABCD' }] };
  const receipt = { status: 'approved', epoch_base_sha: SHA, lane_id: 'A', implementer: 'writer-a', reviewer: 'reviewer-b', leaf_commit: LEAF, leaf_result_sha256: leafResultDigest('base..leaf'), review_commit: REVIEW };
  const signature = '[GNUPG:] VALIDSIG ABCD1234ABCD1234ABCD1234ABCD1234ABCD1234 2026-07-24 0 4 0 1 10 00 ABCD1234ABCD1234ABCD1234ABCD1234ABCD1234\n';
  authority.reviewers[0].signing_fingerprint = 'ABCD1234ABCD1234ABCD1234ABCD1234ABCD1234';
  const receiptFromSeparatelyParsedContent = JSON.parse(JSON.stringify(receipt));
  const operations = { hasCommit: (sha) => [LEAF, REVIEW, OTHER].includes(sha), isAncestor: (ancestor, descendant) => ancestor === SHA && descendant === LEAF, parentOf: (sha) => sha === REVIEW ? LEAF : OTHER, parentCount: () => 1, changedPaths: (sha) => sha === LEAF ? ['src/a/file.rs'] : ['docs/evidence/console/fanout-receipts/559aead08264d5795d3909718cdd05abd49572e84fe55590eef31a88a08fdffd.json'], readJson: () => receiptFromSeparatelyParsedContent, leafDiff: () => 'base..leaf', commitIdentity: () => ({ author_name: 'Reviewer', author_email: 'review@example.test', committer_name: 'Reviewer', committer_email: 'review@example.test' }), verifySignature: () => signature };
  assert.deepEqual(validateReviewReceiptForAnchor(receipt, SHA, lane, authority, operations), receipt);
  const forgedDigest = { ...receipt, leaf_result_sha256: 'e'.repeat(64) };
  assert.throws(() => validateReviewReceiptForAnchor(forgedDigest, SHA, lane, authority, { ...operations, readJson: () => forgedDigest }), /digest/);
  assert.throws(() => validateReviewReceiptForAnchor({ ...receipt, implementer: '' }, SHA, lane, authority, operations), /not an exact trusted/);
  assert.throws(() => validateReviewReceiptForAnchor({ ...receipt, reviewer: 'writer-a' }, SHA, lane, authority, operations), /not an exact trusted/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, readJson: () => null }), /absent/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, changedPaths: () => ['code.js'] }), /mutates/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, parentOf: () => OTHER }), /direct child/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, parentCount: (sha) => sha === LEAF ? 2 : 1 }), /single-parent/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, parentCount: (sha) => sha === REVIEW ? 2 : 1 }), /single-parent/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, changedPaths: (sha) => sha === LEAF ? ['shared/generated.rs'] : operations.changedPaths(sha) }), /private roots/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, isAncestor: () => false }), /anchored/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, verifySignature: () => '[GNUPG:] VALIDSIG 0000000000000000000000000000000000000000\n' }), /signature/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, verifySignature: () => '[GNUPG:] GOODSIG ABCD1234ABCD1234ABCD1234ABCD1234ABCD1234 reviewer\n' }), /signature/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, verifySignature: () => '[GNUPG:] VALIDSIG malformed\n' }), /signature/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, verifySignature: () => `${signature}[GNUPG:] VALIDSIG malformed\n` }), /signature/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, verifySignature: () => `${signature}${signature}` }), /signature/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, verifySignature: () => { throw new Error('git unavailable'); } }), /unavailable/);
  assert.throws(() => validateReviewReceiptForAnchor(receipt, SHA, lane, authority, { ...operations, commitIdentity: () => ({ author_name: 'Reviewer', author_email: 'review@example.test', committer_name: 'Other', committer_email: 'review@example.test' }) }), /identity/);
});

test('review authority rejects duplicate IDs or fingerprints and incomplete exact identities', async () => {
  const { buildFanoutPlan } = await import('./plan-fanout.mjs');
  const reviewer = { id: 'reviewer-b', author_name: 'Reviewer', author_email: 'review@example.test', committer_name: 'Reviewer', committer_email: 'review@example.test', signing_fingerprint: 'ABCD1234ABCD1234ABCD1234ABCD1234ABCD1234' };
  assert.throws(() => buildFanoutPlan(reg([cap('A', ['backend/crates/a/**'])], { review_authority: { reviewers: [reviewer, { ...reviewer, id: 'reviewer-c' }] } }), { anchorSha: SHA, maxWriters: 3, qualityBias: .6, generatedFaces: faces }), /duplicate trusted reviewer signing fingerprint/);
  assert.throws(() => buildFanoutPlan(reg([cap('A', ['backend/crates/a/**'])], { review_authority: { reviewers: [reviewer, { ...reviewer, signing_fingerprint: 'BCDE1234ABCD1234ABCD1234ABCD1234ABCD1234' }] } }), { anchorSha: SHA, maxWriters: 3, qualityBias: .6, generatedFaces: faces }), /duplicate trusted reviewer id/);
  assert.throws(() => buildFanoutPlan(reg([cap('A', ['backend/crates/a/**'])], { review_authority: { reviewers: [{ ...reviewer, committer_email: 'committer@example.test ' }] } }), { anchorSha: SHA, maxWriters: 3, qualityBias: .6, generatedFaces: faces }), /invalid trusted reviewer identity/);
});

test('format-discriminated SSH authority requires the exact verified principal and SHA256 fingerprint', async () => {
  const { signatureMatchesAuthority } = await import('./plan-fanout.mjs');
  const authority = { format: 'ssh', principal: 'jason19931225@gmail.com', fingerprint: 'SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8' };
  const raw = 'Good "git" signature for jason19931225@gmail.com with ED25519 key SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8\n';
  assert.equal(signatureMatchesAuthority(raw, authority), true);
  assert.equal(signatureMatchesAuthority(raw, { ...authority, principal: 'other@example.test' }), false);
  assert.equal(signatureMatchesAuthority(raw, { ...authority, fingerprint: 'SHA256:0000000000000000000000000000000000000000000' }), false);
  assert.equal(signatureMatchesAuthority(`${raw}Good "git" signature for malformed\n`, authority), false);
});

test('SSH verification fails closed for absent or non-Git status output', async () => {
  const { signatureMatchesAuthority } = await import('./plan-fanout.mjs');
  const authority = { format: 'ssh', principal: 'jason19931225@gmail.com', fingerprint: 'SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8' };
  assert.equal(signatureMatchesAuthority('', authority), false);
  assert.equal(signatureMatchesAuthority('Good "file" signature for jason19931225@gmail.com with ED25519 key SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8\n', authority), false);
});

test('hermetic signed-candidate smoke ignores an unsigned synthetic HEAD and global config', async () => {
  const { verifyCommitWithCandidateSshPolicy } = await import('./ssh-signature-policy.mjs');
  const repo = mkdtempSync(path.join(tmpdir(), 'fanout-candidate-'));
  const git = (args, options = {}) => {
    const result = spawnSync('git', args, { cwd: repo, encoding: 'utf8', ...options });
    assert.equal(result.status, 0, result.stderr || result.error?.message);
    return result.stdout.trim();
  };
  const isolatedHome = mkdtempSync(path.join(tmpdir(), 'fanout-candidate-home-'));
  const originalHome = process.env.HOME;
  try {
    git(['init', '-b', 'main']); git(['config', 'user.name', 'Fixture Reviewer']); git(['config', 'user.email', 'fixture@example.test']);
    const signingKey = path.join(repo, 'signing_key');
    const generated = spawnSync('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-f', signingKey], { encoding: 'utf8' });
    assert.equal(generated.status, 0, generated.stderr || generated.error?.message);
    git(['config', 'gpg.format', 'ssh']); git(['config', 'user.signingkey', signingKey]);
    const publicKey = readFileSync(`${signingKey}.pub`, 'utf8').trim().split(/\s+/).slice(0, 2).join(' ');
    const fingerprint = spawnSync('ssh-keygen', ['-lf', `${signingKey}.pub`, '-E', 'sha256'], { encoding: 'utf8' });
    assert.equal(fingerprint.status, 0, fingerprint.stderr || fingerprint.error?.message);
    const authority = { format: 'ssh', principal: 'fixture@example.test', fingerprint: fingerprint.stdout.trim().split(/\s+/)[1] };
    mkdirSync(path.join(repo, '.github/trust'), { recursive: true }); writeFileSync(path.join(repo, '.github/trust/console.allowed_signers'), `${authority.principal} ${publicKey}\n`); writeFileSync(path.join(repo, 'candidate.txt'), 'candidate\n');
    git(['add', '.']); git(['commit', '-S', '-m', 'signed candidate']); const candidate = git(['rev-parse', 'HEAD']);
    git(['commit', '--allow-empty', '--no-gpg-sign', '-m', 'unsigned synthetic PR HEAD']); const syntheticHead = git(['rev-parse', 'HEAD']);
    process.env.HOME = isolatedHome;
    const rawHead = spawnSync('git', ['verify-commit', '--raw', syntheticHead], { cwd: repo, encoding: 'utf8', env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1' } });
    assert.notEqual(rawHead.status, 0);
    const status = verifyCommitWithCandidateSshPolicy(repo, candidate, candidate, authority);
    assert.match(status, new RegExp(`^Good "git" signature for ${authority.principal.replace(/[.@]/g, '\\$&')} with ED25519 key ${authority.fingerprint.replace(/[+/]/g, '\\$&')}$`, 'm'));
  } finally {
    process.env.HOME = originalHome;
    rmSync(repo, { recursive: true, force: true }); rmSync(isolatedHome, { recursive: true, force: true });
  }
});

test('real SSH-signed admission train excludes reviewed leaves and caps cold Buck jobs', () => {
  const repo = mkdtempSync(path.join(tmpdir(), 'fanout-reviewed-'));
  const git = (args, input) => {
    const result = spawnSync('git', args, { cwd: repo, encoding: 'utf8', input });
    assert.equal(result.status, 0, result.stderr || result.error?.message);
    return result.stdout.trim();
  };
  try {
    git(['init', '-b', 'main']); git(['config', 'user.name', 'Jason Lee']); git(['config', 'user.email', 'jason19931225@gmail.com']);
    const signingKey = path.join(repo, 'review_key');
    const generated = spawnSync('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-f', signingKey], { encoding: 'utf8' });
    assert.equal(generated.status, 0, generated.stderr || generated.error?.message);
    git(['config', 'gpg.format', 'ssh']); git(['config', 'user.signingkey', signingKey]);
    for (const directory of ['backend/crates/platform/db/migrations', 'backend/openapi', 'tools/buck', 'backend/crates/a', 'backend/crates/b', 'backend/crates/c', 'docs/program', 'docs/evidence/console/fanout-receipts', '.github/trust']) mkdirSync(path.join(repo, directory), { recursive: true });
    const publicKey = readFileSync(`${signingKey}.pub`, 'utf8').trim().split(/\s+/).slice(0, 2).join(' ');
    writeFileSync(path.join(repo, '.github/trust/console.allowed_signers'), `jason19931225@gmail.com ${publicKey}\n`);
    writeFileSync(path.join(repo, 'backend/crates/platform/db/migrations/.keep'), ''); writeFileSync(path.join(repo, 'backend/openapi/openapi.yaml'), 'openapi: 3.1.0\n'); writeFileSync(path.join(repo, 'tools/buck/generated_face_registry.json'), JSON.stringify(faces));
    for (const id of ['a', 'b', 'c']) writeFileSync(path.join(repo, `backend/crates/${id}/file.txt`), 'base\n');
    git(['add', '.']); git(['commit', '-m', 'base']); const base = git(['rev-parse', 'HEAD']);
    const liveWorktree = git(['rev-parse', '--show-toplevel']);
    const fingerprint = spawnSync('ssh-keygen', ['-lf', `${signingKey}.pub`, '-E', 'sha256'], { encoding: 'utf8' });
    assert.equal(fingerprint.status, 0, fingerprint.stderr || fingerprint.error?.message);
    const reviewer = { id: 'reviewer', author_name: 'Jason Lee', author_email: 'jason19931225@gmail.com', committer_name: 'Jason Lee', committer_email: 'jason19931225@gmail.com', signing: { format: 'ssh', principal: 'jason19931225@gmail.com', fingerprint: fingerprint.stdout.trim().split(/\s+/)[1] } };
    const capability = (id) => {
      const source = cap(id.toUpperCase(), [`backend/crates/${id}/**`]);
      return {
        ...source,
        ownership: { frontend_roots: [], backend_roots: [`backend/crates/${id}/**`], api_schema_roots: ['backend/openapi/openapi.yaml'], migration_owner: 'not_applicable', integration_owner: 'console-consolidation' },
        worktree: repo,
        branch: 'main',
        lane_assignments: {
          source: { owner: source.owner, worktree: liveWorktree, branch: 'main', roots: [`backend/crates/${id}/**`], resources, tests: source.tests },
          consolidation: { owner: 'console-consolidation', worktree: liveWorktree, branch: 'main', resources },
        },
        state: { backend: 'writer_assigned_in_progress', frontend: 'not_applicable' },
      };
    };
    const registry = reg(['a', 'b', 'c'].map(capability), { source_revision: `main@${base}`, resource_budgets: { writer: 6, postgres: 6, browser: 6, ios: 6, graph: 6, cas: 6 }, review_authority: { reviewers: [reviewer] } });
    writeFileSync(path.join(repo, 'docs/program/console-capability-registry.json'), JSON.stringify(registry)); git(['add', '.']); git(['commit', '-m', 'anchor']); const anchor = git(['rev-parse', 'HEAD']);
    const receiptRefs = []; const leafs = [];
    for (const id of ['a', 'b', 'c']) {
      const laneId = `${id.toUpperCase()}#source`;
      writeFileSync(path.join(repo, `backend/crates/${id}/file.txt`), `leaf-${id}\n`); git(['add', '.']); git(['commit', '-m', `leaf-${id}`]); const leaf = git(['rev-parse', 'HEAD']); leafs.push(leaf);
      const canonicalReceiptPath = `docs/evidence/console/fanout-receipts/${createHash('sha256').update(laneId).digest('hex')}.json`;
      const leafDigest = createHash('sha256').update(spawnSync('git', ['diff', '--no-ext-diff', '--no-renames', '--full-index', '--binary', anchor, leaf], { cwd: repo }).stdout).digest('hex');
      const receipt = { status: 'approved', epoch_base_sha: anchor, lane_id: laneId, implementer: `owner-${id.toUpperCase()}`, reviewer: 'reviewer', leaf_commit: leaf, leaf_result_sha256: leafDigest };
      writeFileSync(path.join(repo, canonicalReceiptPath), JSON.stringify(receipt)); git(['add', '.']); git(['commit', '-S', '-m', `review-${id}`]);
      receiptRefs.push({ lane_id: laneId, review_commit: git(['rev-parse', 'HEAD']), receipt_path: canonicalReceiptPath });
    }
    const admission = { schema_version: 'console-fanout-admission-v1', epoch_base_sha: anchor, receipts: receiptRefs };
    writeFileSync(path.join(repo, 'docs/evidence/console/fanout-admission.json'), JSON.stringify(admission)); git(['add', '.']); git(['commit', '-m', 'admission']); const admissionSha = git(['rev-parse', 'HEAD']);
    const runner = path.join(path.dirname(new URL(import.meta.url).pathname), 'plan-fanout.mjs');
    const isolatedHome = mkdtempSync(path.join(tmpdir(), 'fanout-home-'));
    const { CONSOLE_CANDIDATE_SHA: _candidate, CONSOLE_AUTHORITY_TIP_SHA: _authorityTip, CONSOLE_SYNTHETIC_MERGE_SHA: _syntheticMerge, ...hostileOuterEnvironment } = process.env;
    const result = spawnSync('node', [runner, '--candidate', anchor, '--admission', admissionSha], { cwd: repo, encoding: 'utf8', env: { ...hostileOuterEnvironment, HOME: isolatedHome } });
    rmSync(isolatedHome, { recursive: true, force: true });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /console-capability-registry-v2/);
  } finally { rmSync(repo, { recursive: true, force: true }); }
});

test('immutable JSON rejects duplicate object keys before canonicalization', async () => {
  const { parseImmutableJson } = await import('./plan-fanout.mjs');
  assert.equal(parseImmutableJson('{"lane_id":"A","review_commit":"x"}', 'receipt').canonical_sha256.length, 64);
  for (const value of ['{"lane_id":"A","lane_id":"B"}', '{"outer":{"id":"a","id":"b"}}', '{"items":[{"id":"a","id":"b"}]}']) assert.throws(() => parseImmutableJson(value, 'immutable fixture'), /duplicate JSON key/);
});

test('admission manifests are single-parent manifest-only commits with unique connected references', async () => {
  const { validateAdmissionManifest } = await import('./plan-fanout.mjs');
  const REVIEW_A = 'b'.repeat(40); const REVIEW_B = 'c'.repeat(40); const ADMISSION = 'd'.repeat(40);
  const manifest = { schema_version: 'console-fanout-admission-v1', epoch_base_sha: SHA, receipts: [
    { lane_id: 'A', review_commit: REVIEW_A, receipt_path: 'docs/evidence/console/fanout-receipts/559aead08264d5795d3909718cdd05abd49572e84fe55590eef31a88a08fdffd.json' },
    { lane_id: 'B', review_commit: REVIEW_B, receipt_path: 'docs/evidence/console/fanout-receipts/df7e70e5021544f4834bbee64a9e3789febc4be81470df629cad6ddb03320a5c.json' },
  ] };
  const operations = { parentCount: () => 1, changedPaths: () => ['docs/evidence/console/fanout-admission.json'], isAncestor: (ancestor, descendant) => descendant === ADMISSION && [REVIEW_A, REVIEW_B].includes(ancestor), readJson: (sha) => ({ lane_id: sha === REVIEW_A ? 'A' : 'B', review_commit: sha }) };
  assert.deepEqual(validateAdmissionManifest(manifest, SHA, ADMISSION, operations), manifest.receipts);
  assert.throws(() => validateAdmissionManifest(manifest, SHA, ADMISSION, { ...operations, parentCount: () => 2 }), /single-parent/);
  assert.throws(() => validateAdmissionManifest(manifest, SHA, ADMISSION, { ...operations, changedPaths: () => ['docs/evidence/console/fanout-admission.json', 'unrelated.txt'] }), /manifest-only/);
  assert.throws(() => validateAdmissionManifest({ ...manifest, receipts: [manifest.receipts[0], { ...manifest.receipts[0] }] }, SHA, ADMISSION, operations), /duplicate/);
  assert.throws(() => validateAdmissionManifest({ ...manifest, receipts: [manifest.receipts[0], { ...manifest.receipts[1], review_commit: REVIEW_A }] }, SHA, ADMISSION, operations), /duplicate/);
  assert.throws(() => validateAdmissionManifest(manifest, SHA, ADMISSION, { ...operations, isAncestor: () => false }), /ancestor/);
});

test('epoch admission closure rejects unreviewed, unrelated, and divergent-train commits', async () => {
  const { validateEpochAdmissionClosure } = await import('./plan-fanout.mjs');
  const LEAF_A = 'b'.repeat(40), REVIEW_A = 'c'.repeat(40), LEAF_B = 'd'.repeat(40), REVIEW_B = 'e'.repeat(40), ADMISSION = 'f'.repeat(40), OTHER = '1'.repeat(40);
  const receipts = [{ leaf_commit: LEAF_A, review_commit: REVIEW_A }, { leaf_commit: LEAF_B, review_commit: REVIEW_B }];
  const operations = { commitsBetween: () => [LEAF_A, REVIEW_A, LEAF_B, REVIEW_B, ADMISSION], parentOf: (sha) => ({ [LEAF_A]: SHA, [REVIEW_A]: LEAF_A, [LEAF_B]: REVIEW_A, [REVIEW_B]: LEAF_B, [ADMISSION]: REVIEW_B })[sha] };
  assert.doesNotThrow(() => validateEpochAdmissionClosure(SHA, ADMISSION, receipts, operations));
  assert.throws(() => validateEpochAdmissionClosure(SHA, ADMISSION, receipts, { ...operations, commitsBetween: () => [LEAF_A, OTHER, REVIEW_A, LEAF_B, REVIEW_B, ADMISSION] }), /unreviewed or unrelated/);
  assert.throws(() => validateEpochAdmissionClosure(SHA, ADMISSION, receipts, { ...operations, parentOf: (sha) => sha === LEAF_B ? SHA : operations.parentOf(sha) }), /serialized admission train/);
});

test('private lane roots that intersect protected authority are held instead of partially admitted', () => {
  const unsafe = cap('UNSAFE', ['backend/crates/unsafe/**']);
  unsafe.lane_assignments = { source: { owner: unsafe.owner, worktree: unsafe.worktree, branch: unsafe.branch, roots: ['backend/**', 'docs/evidence/UNSAFE/**'], resources, tests: unsafe.tests } };
  const value = plan([unsafe]);
  assert.equal(value.selected.length, 0);
  assert.match(value.held.find((entry) => entry.lane_id === 'UNSAFE#source').reasons.join(','), /protected_shared_root_intersection/);
});

test('lane assignment roots must be covered by exactly one private capability root', () => {
  const expanding = cap('EXPANDING', ['backend/crates/owned/**']);
  expanding.lane_assignments = { source: { owner: expanding.owner, worktree: expanding.worktree, branch: expanding.branch, roots: ['backend/crates/**'], resources, tests: expanding.tests } };
  const value = plan([expanding]);
  assert.equal(value.selected.length, 0);
  assert.match(value.held.find((entry) => entry.lane_id === 'EXPANDING#source').reasons.join(','), /lane_root_outside_capability_private_ownership/);
});

test('completed and legacy lanes remain held before completion shortcuts', () => {
  const complete = cap('COMPLETE', ['backend/crates/complete/**'], { state: { backend: 'complete', frontend: 'not_applicable' } });
  complete.lane_assignments = { source: { owner: complete.owner, worktree: complete.worktree, branch: complete.branch, roots: ['backend/crates/**'], resources, tests: complete.tests } };
  const invalid = plan([complete]);
  assert.match(invalid.held[0].reasons.join(','), /lane_root_outside_capability_private_ownership/);
  const legacy = buildFanoutPlan(reg([cap('LEGACY', ['backend/crates/legacy/**'])], { fanout_epoch: { current_epoch: 1, normalized_lane_ids: [] } }), { anchorSha: SHA, maxWriters: 3, qualityBias: .6, generatedFaces: faces });
  assert.match(legacy.held[0].reasons.join(','), /legacy_lane_not_normalized_for_epoch/);
});

test('runtime-ineligible lanes are held before ranking so lower safe lanes fill capacity', () => {
  const high = cap('HIGH', ['backend/crates/high/**'], { priority: { score: 1, inputs: { correctness_and_risk_reduction: 1, verification_readiness: 1 } } });
  const low = cap('LOW', ['backend/crates/low/**'], { priority: { score: .2, inputs: { correctness_and_risk_reduction: .2, verification_readiness: .2 } } });
  const value = plan([high, low], { maxWriters: 1, runtimeLaneEligibility: { HIGH: 'declared_worktree_dirty' } });
  assert.deepEqual(value.selected.map((lane) => lane.lane_id), ['LOW']);
  assert.match(value.held.find((entry) => entry.lane_id === 'HIGH').reasons.join(','), /declared_worktree_dirty/);
});

test('source revision syntax requires a full immutable baseline SHA', async () => {
  const { parseSourceRevision } = await import('./plan-fanout.mjs');
  assert.deepEqual(parseSourceRevision(`origin/main@${SHA}`), { ref: 'origin/main', sha: SHA });
  for (const bad of ['origin/main@abcdef', `origin/main@${'A'.repeat(40)}`, `origin/main@${SHA} trailing`, `@${SHA}`]) assert.throws(() => parseSourceRevision(bad));
});

test('source provenance admits only an existing immutable ancestor and non-conflicting ref', async () => {
  const { validateSourceRevisionForAnchor } = await import('./plan-fanout.mjs');
  const BASE = 'b'.repeat(40); const ANCHOR = 'c'.repeat(40); const OTHER = 'd'.repeat(40);
  const operations = { hasCommit: (sha) => [BASE, ANCHOR, OTHER].includes(sha), isAncestor: (a, b) => a === b || (a === BASE && b === ANCHOR), resolveRef: (ref) => ref === 'origin/main' ? ANCHOR : null };
  assert.deepEqual(validateSourceRevisionForAnchor(`origin/main@${BASE}`, ANCHOR, operations), { ref: 'origin/main', sha: BASE });
  assert.throws(() => validateSourceRevisionForAnchor(`origin/main@${'e'.repeat(40)}`, ANCHOR, operations), /does not exist/);
  assert.throws(() => validateSourceRevisionForAnchor(`origin/main@${OTHER}`, ANCHOR, operations), /not an ancestor/);
  assert.throws(() => validateSourceRevisionForAnchor(`origin/main@${'abcdef'}`, ANCHOR, operations));
  assert.throws(() => validateSourceRevisionForAnchor(`origin/main@${OTHER}`, OTHER, { ...operations, resolveRef: () => BASE }), /behind or conflicts/);
});


test('exact-SHA verification scheduling groups only compatible validated work and uses deterministic density', async () => {
  const { groupValidatedVerificationEntries } = await import('./plan-fanout.mjs');
  const leafA = 'b'.repeat(40); const leafB = 'c'.repeat(40); const leafC = 'd'.repeat(40);
  const entry = (lane_id, receipt_leaf_commit, quality_utility, resources, buck2_targets) => ({ capability_id: lane_id, lane_id, receipt_leaf_commit, quality_utility, resources, buck2_targets, leaf_commands: ['git diff --check'] });
  assert.throws(() => groupValidatedVerificationEntries([
    entry('BAD', 'B'.repeat(40), 1, { writer: 1, postgres: 0, browser: 0, ios: 0, graph: 0, cas: 0 }, []),
  ]), /exact leaf SHA/);
  const shared = groupValidatedVerificationEntries([
    entry('B', leafA, 0.8, { writer: 1, postgres: 1, browser: 0, ios: 0, graph: 1, cas: 0 }, ['//b:test']),
    entry('A', leafA, 0.9, { writer: 1, postgres: 0, browser: 0, ios: 0, graph: 1, cas: 1 }, ['//a:test', '//b:test']),
    entry('C', leafB, 0.9, { writer: 1, postgres: 0, browser: 0, ios: 0, graph: 1, cas: 1 }, ['//c:test']),
    entry('D', leafC, 0.9, { writer: 1, postgres: 0, browser: 0, ios: 0, graph: 1, cas: 1 }, ['//d:test']),
  ]);
  assert.equal(shared.length, 3, 'different exact SHAs must never share a verification job');
  const leafAGroup = shared.find((group) => group.verification_sha === leafA);
  assert.deepEqual(leafAGroup.lane_ids, ['A', 'B']);
  assert.deepEqual(leafAGroup.buck2_targets, ['//a:test', '//b:test']);
  assert.deepEqual(leafAGroup.resources, { writer: 2, postgres: 1, browser: 0, ios: 0, graph: 2, cas: 1 });
  assert.equal(leafAGroup.execution, 'canonical_shared_daemon_combined_targets');
  // Equal density/quality ties fall through to the exact SHA, then lane IDs.
  assert.deepEqual(shared.filter((group) => group.quality_utility === 0.9).map((group) => group.verification_sha), [leafB, leafC]);
});

test('receipt-controlled grouping ignores caller-provided consolidated-tip hints', async () => {
  const { groupValidatedVerificationEntries } = await import('./plan-fanout.mjs');
  const leafA = 'b'.repeat(40); const leafB = 'c'.repeat(40); const fakeConsolidatedTip = 'd'.repeat(40);
  const resources = { writer: 1, postgres: 0, browser: 0, ios: 0, graph: 0, cas: 0 };
  const groups = groupValidatedVerificationEntries([
    { capability_id: 'A', lane_id: 'A', receipt_leaf_commit: leafA, consolidated_tip_sha: fakeConsolidatedTip, quality_utility: 1, resources, buck2_targets: [], leaf_commands: [] },
    { capability_id: 'B', lane_id: 'B', receipt_leaf_commit: leafB, consolidated_tip_sha: fakeConsolidatedTip, quality_utility: 1, resources, buck2_targets: [], leaf_commands: [] },
  ]);
  assert.deepEqual(groups.map((group) => group.verification_sha).sort(), [leafA, leafB]);
  assert.ok(groups.every((group) => group.cache_affinity !== fakeConsolidatedTip));
});

test('v2 truth ledger cannot bypass validated admission', () => {
  const v2 = reg([cap('V2-HOLD', ['backend/crates/v2-hold/**'])], { schema_version: 'console-capability-registry-v2' });
  assert.throws(() => buildFanoutPlan(v2, { anchorSha: SHA, maxWriters: 1, qualityBias: .6, generatedFaces: faces }), /validated truth-ledger/);
});
