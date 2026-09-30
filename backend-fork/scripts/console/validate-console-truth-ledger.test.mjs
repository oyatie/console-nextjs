import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createHash } from 'node:crypto';
import test from 'node:test';

import { createConsoleBuckTargetResolver, createConsoleCandidateSourceResolver, promotionAuthorityDigests, validateConsoleTruthLedger } from './validate-console-truth-ledger.mjs';
import { verifyCommitWithCandidateSshPolicy } from './ssh-signature-policy.mjs';
import { ABSENT_CONSOLE_ROUTE_FACTS, extractConsoleRouteFactsFromTexts } from './route-inventory.mjs';
import { installGitFixtureEnvironment } from '../lib/git-fixture-environment.mjs';

installGitFixtureEnvironment();

const registry = JSON.parse(readFileSync(new URL('../../docs/program/console-capability-registry.json', import.meta.url)));
const jurisdiction = JSON.parse(readFileSync(new URL('../../docs/program/console-jurisdiction-register.json', import.meta.url)));
const repoRoot = fileURLToPath(new URL('../../', import.meta.url));
// The registers no longer carry the candidate SHA — it is supplied by the caller, which in CI is
// git's own derivation of the C/T/M train. Any full lowercase SHA distinct from the two provenance
// anchors exercises the same code path the real one does, so this fixture is deliberately NOT a
// commit that exists: a real SHA here would have to be rebound on every rebase, which is the cost
// this train removes.
const CANDIDATE_SHA = 'cafe1a7e'.repeat(5);
const ROUTE_CLAIM_FIELDS = ['source_mounted', 'production_exposed', 'registry_body_present', 'nav_declared'];

function withoutRouteClaims(value) {
  const cleared = structuredClone(value);
  for (const capability of cleared.capabilities) {
    capability.route_presentation.route_keys = [];
    for (const field of ROUTE_CLAIM_FIELDS) capability.route_presentation[field] = false;
  }
  if (cleared.source_inventory) cleared.source_inventory.unmodeled_keys = [];
  return cleared;
}

test('current candidate truth ledger is structurally complete but remains candidate-bound HOLD where evidence is absent', () => {
  assert.doesNotThrow(() => validateConsoleTruthLedger(registry, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }));
  assert.equal(registry.schema_version, 'console-capability-registry-v2');
  assert.ok(registry.capabilities.some((capability) => capability.id === 'CAP-ASSET-MASTER-ACTION'));
  assert.ok(registry.capabilities.every((capability) => capability.benchmark?.verdict === 'HOLD'));
  assert.ok(registry.capabilities.every((capability) => capability.benchmark?.native_outcomes?.length >= 3));
  // The payload still has to be there — that is the guarantee the deleted SHA equality was also
  // carrying — and it has to carry the fields that make it evidence, which is the REST of what
  // that equality was carrying: `?.candidate_sha !== sha` refuses `{}` too, since `undefined !== sha`.
  for (const payload of [...registry.capabilities.map((c) => c.candidate_evidence), ...jurisdiction.controls.map((c) => c.candidate_evidence)]) {
    assert.ok(payload && typeof payload === 'object' && !Array.isArray(payload));
    assert.ok(typeof payload.status === 'string' && payload.status !== '');
    assert.ok(typeof payload.reason === 'string' && payload.reason.trim() !== '');
  }
  // This suite accepts fixtures from both ends of an authority train, while ordinary CI runs the
  // regression from the candidate checkout. It asserts the property that holds at both: whether
  // a stored copy is present or absent, the validator's answer is the same, because it no longer
  // reads one. The absence in the shipped documents is a diff to read, not a runtime fact this
  // file can pin at C; that a
  // present copy is INERT is asserted below, in 'a re-added stored copy buys nothing'.
  const stripped = structuredClone(registry), strippedJurisdiction = structuredClone(jurisdiction);
  delete stripped.candidate; delete stripped.provenance.exact_current_candidate_sha;
  for (const capability of stripped.capabilities) delete capability.candidate_evidence.candidate_sha;
  delete strippedJurisdiction.candidate; delete strippedJurisdiction.provenance.exact_current_candidate_sha;
  for (const control of strippedJurisdiction.controls) delete control.candidate_evidence.candidate_sha;
  assert.doesNotThrow(() => validateConsoleTruthLedger(stripped, strippedJurisdiction, { expectedCandidateSha: CANDIDATE_SHA }));
  assert.ok(jurisdiction.controls.every((control) => control.release_disposition === 'HOLD'));
});

test('an unavailable governing lifecycle cannot become an absolute workstation dependency', () => {
  const absolute = structuredClone(jurisdiction);
  absolute.governing_lifecycle.path = '/Users/example/private-lifecycle.md';
  assert.throws(
    () => validateConsoleTruthLedger(registry, absolute, { expectedCandidateSha: CANDIDATE_SHA }),
    /pathless HOLD_UNAVAILABLE provenance record/,
  );

  const unheld = structuredClone(jurisdiction);
  unheld.governing_lifecycle.status = 'CURRENT';
  assert.throws(
    () => validateConsoleTruthLedger(registry, unheld, { expectedCandidateSha: CANDIDATE_SHA }),
    /pathless HOLD_UNAVAILABLE provenance record/,
  );

  const unexplained = structuredClone(jurisdiction);
  unexplained.governing_lifecycle.reason = '';
  assert.throws(
    () => validateConsoleTruthLedger(registry, unexplained, { expectedCandidateSha: CANDIDATE_SHA }),
    /unavailable reason must be a non-empty string/,
  );
});

test('pre-wipe paths, branches, lanes, and progress labels cannot become current continuation authority', () => {
  const unheldReset = structuredClone(registry);
  unheldReset.continuation_reset.lane_dispatch = 'enabled';
  assert.throws(
    () => validateConsoleTruthLedger(unheldReset, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }),
    /continuation reset must keep every pre-wipe worktree, branch, and lane on HOLD/,
  );

  const worktree = structuredClone(registry);
  worktree.capabilities[0].worktree = '/Users/example/old-worktree';
  assert.throws(
    () => validateConsoleTruthLedger(worktree, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }),
    /current continuation assignment must be null while reset is HOLD/,
  );

  const branch = structuredClone(registry);
  branch.capabilities[0].branch = 'deleted/old-branch';
  assert.throws(
    () => validateConsoleTruthLedger(branch, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }),
    /current continuation assignment must be null while reset is HOLD/,
  );

  const lanes = structuredClone(registry);
  lanes.capabilities[0].lane_assignments = { writer: { owner: 'stale' } };
  assert.throws(
    () => validateConsoleTruthLedger(lanes, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }),
    /current continuation assignment must be null while reset is HOLD/,
  );

  const progress = structuredClone(registry);
  progress.capabilities[0].state.runtime = 'live';
  assert.throws(
    () => validateConsoleTruthLedger(progress, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }),
    /current continuation state must contain only HOLD values/,
  );

  const deletedHistory = structuredClone(registry);
  delete deletedHistory.capabilities[0].historical_reset_state;
  assert.throws(
    () => validateConsoleTruthLedger(deletedHistory, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }),
    /missing historical continuation field historical_reset_state/,
  );

  const rewrittenHistory = structuredClone(registry);
  rewrittenHistory.capabilities[0].historical_reset_state.runtime = 'rewritten';
  assert.throws(
    () => validateConsoleTruthLedger(rewrittenHistory, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }),
    /historical continuation fields differ from the pinned reset snapshot/,
  );

  assert.equal(
    registry.capabilities.find((capability) => capability.id === 'CAP-CONSOLE-SHELL').historical_reset_state.frontend,
    'frontend_surface_deleted_by_2026_07_28_clean_slate_pivot',
  );
  assert.equal(
    registry.capabilities.find((capability) => capability.id === 'CAP-ONTOLOGY-ENGINE').historical_reset_state.backend,
    '27_types_seeded_and_frozen_no_additive_upgrade_function_exists',
  );
});

test('provenance is reproducible from advertised tags in a candidate-only clone', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'console-provenance-'));
  const source = path.join(root, 'source');
  const origin = path.join(root, 'origin.git');
  const checkout = path.join(root, 'checkout');
  const run = (cwd, args) => execFileSync('git', ['-C', cwd, ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  try {
    execFileSync('git', ['init', '-b', 'archive-build', source], { stdio: 'ignore' });
    run(source, ['config', 'user.name', 'Provenance Test']);
    run(source, ['config', 'user.email', 'provenance@example.invalid']);
    writeFileSync(path.join(source, 'freeze.txt'), 'historical implementation freeze\n');
    run(source, ['add', 'freeze.txt']);
    run(source, ['commit', '-m', 'historical freeze']);
    const freezeSha = run(source, ['rev-parse', 'HEAD']);
    const freezeRef = 'refs/tags/archive/pre-pivot-implementation-freeze-test';
    run(source, ['tag', freezeRef.slice('refs/tags/'.length), freezeSha]);

    run(source, ['checkout', '--orphan', 'main']);
    rmSync(path.join(source, 'freeze.txt'));
    writeFileSync(path.join(source, 'base.txt'), 'authority base\n');
    run(source, ['add', '-A']);
    run(source, ['commit', '-m', 'authority base']);
    const baseSha = run(source, ['rev-parse', 'HEAD']);
    const baseRef = 'refs/tags/archive/authority-base-test';
    run(source, ['tag', baseRef.slice('refs/tags/'.length), baseSha]);
    writeFileSync(path.join(source, 'candidate.txt'), 'candidate\n');
    run(source, ['add', 'candidate.txt']);
    run(source, ['commit', '-m', 'candidate']);
    const candidateSha = run(source, ['rev-parse', 'HEAD']);

    execFileSync('git', ['clone', '--bare', '--no-local', source, origin], { stdio: 'ignore' });
    execFileSync('git', ['clone', '--no-local', '--no-tags', '--single-branch', '--branch', 'main', origin, checkout], { stdio: 'ignore' });
    const isolatedRegistry = structuredClone(registry);
    const isolatedJurisdiction = structuredClone(jurisdiction);
    for (const document of [isolatedRegistry, isolatedJurisdiction]) {
      Object.assign(document.provenance, {
        authority_base_sha: baseSha,
        authority_base_ref: baseRef,
        historical_implementation_freeze_sha: freezeSha,
        historical_implementation_freeze_ref: freezeRef,
      });
    }
    const resolveSha = (value) => {
      try { run(checkout, ['cat-file', '-e', `${value}^{commit}`]); return true; } catch { return false; }
    };
    const resolveRef = (ref, expectedSha) => {
      try { return run(checkout, ['rev-parse', '--verify', `${ref}^{commit}`]) === expectedSha; } catch { return false; }
    };

    assert.throws(
      () => validateConsoleTruthLedger(isolatedRegistry, isolatedJurisdiction, { expectedCandidateSha: candidateSha, resolveSha, resolveRef }),
      /authority_base_ref does not resolve/,
      'a locally present authority commit is insufficient without its advertised custody ref',
    );
    run(checkout, ['fetch', '--no-tags', 'origin', `${baseRef}:${baseRef}`]);
    assert.throws(
      () => validateConsoleTruthLedger(isolatedRegistry, isolatedJurisdiction, { expectedCandidateSha: candidateSha, resolveSha, resolveRef }),
      /historical_implementation_freeze_sha SHA is unresolvable/,
      'candidate history must not make an unrelated historical object appear reachable',
    );
    run(checkout, ['fetch', '--no-tags', 'origin', `${freezeRef}:${freezeRef}`]);
    assert.doesNotThrow(() => validateConsoleTruthLedger(isolatedRegistry, isolatedJurisdiction, { expectedCandidateSha: candidateSha, resolveSha, resolveRef }));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('validator fails closed for duplicate IDs, missing evidence, missing module benchmark, and unsafe exposure', () => {
  const duplicate = structuredClone(registry);
  duplicate.capabilities.push(structuredClone(duplicate.capabilities[0]));
  assert.throws(() => validateConsoleTruthLedger(duplicate, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }), /duplicate capability id/);

  // The equality against a stored SHA is gone; the EXISTENCE guarantee it was also carrying is
  // explicit and still refuses a row that ships no candidate-bound evidence payload at all.
  const noEvidence = structuredClone(registry);
  delete noEvidence.capabilities[0].candidate_evidence;
  assert.throws(() => validateConsoleTruthLedger(noEvidence, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }), /candidate evidence must be an object/);

  const noBenchmark = structuredClone(registry);
  delete noBenchmark.capabilities[0].benchmark;
  assert.throws(() => validateConsoleTruthLedger(noBenchmark, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }), /per-module benchmark/);

  const noOutcomes = structuredClone(registry);
  noOutcomes.capabilities[0].benchmark.native_outcomes = [];
  assert.throws(() => validateConsoleTruthLedger(noOutcomes, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }), /3-7 measurable native outcomes/);

  const unsafeExposure = structuredClone(registry);
  unsafeExposure.capabilities[0].route_presentation = { route_keys: [], source_mounted: false, production_exposed: true, registry_body_present: false, nav_declared: true, evidence_receipt_status: 'HOLD', source: 'test' };
  assert.throws(() => validateConsoleTruthLedger(unsafeExposure, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }), /exposed route must be mounted/);
});

test('validator rejects generic outcomes, duplicate controls, legal promotion, and control evidence that is absent', () => {
  const generic = structuredClone(registry);
  generic.capabilities[1].benchmark.native_outcomes = structuredClone(generic.capabilities[0].benchmark.native_outcomes);
  assert.throws(() => validateConsoleTruthLedger(generic, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }), /duplicate benchmark outcome/);
  const duplicateControl = structuredClone(jurisdiction);
  duplicateControl.controls.push(structuredClone(duplicateControl.controls[0]));
  assert.throws(() => validateConsoleTruthLedger(registry, duplicateControl, { expectedCandidateSha: CANDIDATE_SHA }), /duplicate control id/);
  const promoted = structuredClone(jurisdiction);
  promoted.controls[0].release_disposition = 'MEET';
  assert.throws(() => validateConsoleTruthLedger(registry, promoted, { expectedCandidateSha: CANDIDATE_SHA }), /must remain HOLD/);
  // `control.candidate_evidence?.candidate_sha !== candidate.sha` used to imply the payload was
  // there. The implication is now written out, so deleting the payload still fails closed.
  const noControlEvidence = structuredClone(jurisdiction);
  delete noControlEvidence.controls[0].candidate_evidence;
  assert.throws(() => validateConsoleTruthLedger(registry, noControlEvidence, { expectedCandidateSha: CANDIDATE_SHA }), /control candidate evidence must be an object/);
  // The caller's SHA is the ONLY source now, so it is required rather than optional.
  assert.throws(() => validateConsoleTruthLedger(registry, jurisdiction, {}), /candidate sha must be a full lowercase Git SHA/);
});

test('raw ledger JSON rejects duplicate keys before JSON.parse can hide them', async () => {
  const { parseImmutableJson } = await import('./immutable-json.mjs');
  for (const raw of ['{"schema_version":"a","schema_version":"b"}', '{"controls":[{"id":"x","id":"y"}]}']) assert.throws(() => parseImmutableJson(raw, 'truth ledger'), /duplicate JSON key/);
});

test('source route facts reject a ledger claim that disagrees with mounted/exposed source', () => {
  // Fixture rather than candidate source: the 2026-07-28 pivot deleted the
  // frontend, so reading the candidate would exercise nothing. The rule under
  // test is the per-key comparison, which must reject a claim that contradicts
  // whatever route source exists.
  const routeFacts = extractConsoleRouteFactsFromTexts(
    'export const MOUNTED_SCREEN_KEYS = ["overview"] as const;\nexport const EXPOSED_SCREEN_KEYS = [] as const;\nconst nav = [{ screen: "overview" }];\n',
    'export const SCREEN_REGISTRY = {\n  overview: () => null,\n};\n',
  );
  const truthful = withoutRouteClaims(registry);
  Object.assign(truthful.capabilities[0].route_presentation, { route_keys: ['overview'], source_mounted: true, registry_body_present: true, nav_declared: true });
  assert.doesNotThrow(() => validateConsoleTruthLedger(truthful, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA, routeFacts }));
  const bad = structuredClone(truthful);
  bad.capabilities[0].route_presentation.source_mounted = false;
  assert.throws(() => validateConsoleTruthLedger(bad, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA, routeFacts }), /route source fact mismatch/);
  const unowned = structuredClone(truthful);
  unowned.capabilities[0].route_presentation.route_keys = [];
  unowned.capabilities[0].route_presentation.source_mounted = false;
  unowned.capabilities[0].route_presentation.registry_body_present = false;
  unowned.capabilities[0].route_presentation.nav_declared = false;
  assert.throws(() => validateConsoleTruthLedger(unowned, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA, routeFacts }), /complete bijection/);
});

test('an absent route source refutes every positive route claim instead of iterating zero keys', () => {
  // With no route source the per-key loop and the bijection both see zero keys,
  // so they corroborate nothing. The boolean claims must be judged on their own.
  const truthful = withoutRouteClaims(registry);
  assert.doesNotThrow(() => validateConsoleTruthLedger(truthful, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA, routeFacts: ABSENT_CONSOLE_ROUTE_FACTS }));
  for (const field of ROUTE_CLAIM_FIELDS) {
    const claimed = withoutRouteClaims(registry);
    claimed.capabilities[0].route_presentation[field] = true;
    if (field === 'production_exposed') { claimed.capabilities[0].route_presentation.source_mounted = true; claimed.capabilities[0].truth.exposure = 'EXPOSED'; claimed.capabilities[0].truth.verification = 'VERIFIED'; }
    assert.throws(
      () => validateConsoleTruthLedger(claimed, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA, routeFacts: ABSENT_CONSOLE_ROUTE_FACTS }),
      /no console route source to corroborate it/,
      `${field} claim survived an absent route source`,
    );
  }
  // A fact object that forgets to declare its provenance is treated as absent,
  // never as corroboration.
  const claimed = withoutRouteClaims(registry);
  claimed.capabilities[0].route_presentation.nav_declared = true;
  assert.throws(() => validateConsoleTruthLedger(claimed, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA, routeFacts: { facts: {} } }), /no console route source to corroborate it/);
});


test('forged comparator source/date and attacker review cannot pass', () => {
  const forged = structuredClone(registry); forged.capabilities[0].benchmark.comparator_sources[0].observation_as_of = '2026-99-99';
  assert.throws(() => validateConsoleTruthLedger(forged, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA, resolveSource: () => true }), /ISO/);
  const source = structuredClone(registry);
  assert.throws(() => validateConsoleTruthLedger(source, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA, resolveSource: () => false }), /tracked regular file/);
  const review = structuredClone(registry); review.capabilities[0].benchmark.independent_outcome_review = { status: 'MEET', candidate_sha: CANDIDATE_SHA, receipt_path: 'forged.json', reviewer_id: review.capabilities[0].owner };
  assert.throws(() => validateConsoleTruthLedger(review, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }), /receipt schema/);
});

test('required Buck target fails closed when resolver rejects it', () => {
  const bad = structuredClone(registry); const equipment = bad.capabilities.find((capability) => capability.id === 'CAP-EQUIPMENT-3R-PILOT');
  assert.throws(() => validateConsoleTruthLedger(bad, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA, resolveBuckTarget: () => false }), /invalid\/nonexistent Buck target/);
});

test('Buck target existence is read from the candidate BUCK blob, never evaluated by Buck2', () => {
  const files = {
    '.buckconfig': '[cells]\n  root = .\n  prelude = prelude\n  toolchains = toolchains\n\n[project]\n  ignore = .git\n',
    'backend/crates/x/BUCK': 'rust_test(\n    name = "x-unit",\n    srcs = ["lib.rs"],\n)\n#    name = "ghost",\n    labels = ["name = \\"phantom\\""],\n',
    'toolchains/BUCK': 'toolchain(\n    name = "rust",\n)\n',
  };
  const resolve = createConsoleBuckTargetResolver({
    readText: (file) => { if (!(file in files)) throw new Error(`candidate source is missing: ${file}`); return files[file]; },
    resolveSource: (file) => (file.includes('..') ? false : file in files && { tracked_regular: true }),
  });
  assert.equal(resolve('//backend/crates/x:x-unit'), true);
  assert.equal(resolve('//backend/crates/x:ghost'), false, 'a commented-out declaration is not a target');
  assert.equal(resolve('//backend/crates/x:phantom'), false, 'a declaration lookalike inside a string is not a target');
  assert.equal(resolve('//backend/crates/x:x-uni'), false, 'a prefix of a real name is not a target');
  assert.equal(resolve('//backend/crates/gone:x-unit'), false, 'a package with no BUCK file fails closed');
  assert.equal(resolve('//toolchains:rust'), false, 'a nested cell is not a root-cell package');
  assert.equal(resolve('//../../etc:passwd'), false, 'a traversing label fails closed');
});

test('the real registry resolves against the real tree, and a nonexistent Buck target goes RED', () => {
  // Exercises the shipped resolver, not a stub: the same one main() hands to the
  // validator, so registry/tree drift fails here and not only in CI.
  const resolveBuckTarget = createConsoleBuckTargetResolver({
    readText: (file) => readFileSync(path.join(repoRoot, file), 'utf8'),
    resolveSource: (file) => (file.includes('..') || file.startsWith('/') ? false : existsSync(path.join(repoRoot, file)) && { tracked_regular: true }),
  });
  const declared = registry.capabilities.flatMap((capability) => capability.delivery_unit?.buck2_targets ?? []);
  assert.ok(declared.length > 0, 'the registry must declare at least one Buck target for this to assert anything');
  for (const target of declared) assert.equal(resolveBuckTarget(target), true, `declared Buck target does not exist in the tree: ${target}`);
  assert.doesNotThrow(() => validateConsoleTruthLedger(registry, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA, resolveBuckTarget }));

  for (const mutate of [
    (capability) => { capability.delivery_unit.buck2_targets[0] = '//backend/crates/equipment/domain:no-such-target'; },
    (capability) => { capability.delivery_unit.buck2_targets[0] = '//backend/crates/nonexistent/pkg:console-equipment-domain-unit'; },
  ]) {
    const bad = structuredClone(registry);
    const capability = bad.capabilities.find((entry) => entry.id === 'CAP-EQUIPMENT-3R-PILOT');
    mutate(capability);
    capability.tests.buck2_targets = structuredClone(capability.delivery_unit.buck2_targets);
    assert.throws(() => validateConsoleTruthLedger(bad, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA, resolveBuckTarget }), /invalid\/nonexistent Buck target/);
  }
});

test('delivery-unit Buck authority cannot diverge from declared verification targets', () => {
  const bad = structuredClone(registry);
  const equipment = bad.capabilities.find((capability) => capability.id === 'CAP-EQUIPMENT-3R-PILOT');
  equipment.delivery_unit.buck2_targets = [
    '//backend/crates/equipment/domain:console-equipment-domain-unit',
    '//backend/crates/equipment/rest:console-equipment-rest',
    '//backend/app:console-app-itest-equipment_3r_api',
  ];
  assert.throws(
    () => validateConsoleTruthLedger(bad, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }),
    /delivery Buck targets must match declared verification targets/,
  );
});

test('attestation rejects TOCTOU mutation after candidate-bound validation', async () => {
  const { isValidatedConsoleTruthLedger } = await import('./validate-console-truth-ledger.mjs');
  // Route facts are deliberately omitted: this attests digest staleness, not
  // route binding, and the candidate holds no route source to bind against.
  // Route-fact binding is covered by the two route tests above.
  const value = structuredClone(registry);
  validateConsoleTruthLedger(value, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA });
  assert.equal(isValidatedConsoleTruthLedger(value), true);
  value.capabilities[0].truth.exposure = 'EXPOSED';
  assert.equal(isValidatedConsoleTruthLedger(value), false);
});

test('moving candidate branch names are irrelevant and jurisdiction target bypasses reject', () => {
  const branch = structuredClone(registry);
  branch.candidate = { branch: 'attacker/moving-pr-branch', sha: 'a'.repeat(40) };
  assert.doesNotThrow(() => validateConsoleTruthLedger(branch, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA, resolveBranch: () => 'a'.repeat(40) }));
  const empty = structuredClone(jurisdiction); empty.target_jurisdiction_set = [];
  assert.throws(() => validateConsoleTruthLedger(registry, empty, { expectedCandidateSha: CANDIDATE_SHA }), /jurisdiction target/);
});

test('non-HOLD promotion requires canonical trusted immutable receipt fields', () => {
  const promoted = structuredClone(registry); const cap = promoted.capabilities[0];
  cap.benchmark.verdict = 'MEET'; cap.candidate_evidence.status = 'VERIFIED';
  cap.benchmark.independent_outcome_review = { status: 'MEET', reviewer_id: 'attacker', candidate_sha: CANDIDATE_SHA, capability_id: cap.id, outcome_ids: [cap.benchmark.native_outcomes[0].id], evidence_digest: 'a'.repeat(64), review_commit: 'b'.repeat(40), receipt_path: 'docs/evidence/fake.json' };
  assert.throws(() => validateConsoleTruthLedger(promoted, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA, resolveReceipt: () => true }), /receipt schema/);
  const trusted = structuredClone(registry); const c = trusted.capabilities[0]; trusted.candidate_evidence = undefined; c.benchmark.verdict='MEET'; c.candidate_evidence.status='VERIFIED'; c.benchmark.independent_outcome_review={status:'MEET',reviewer_id:'jasonlee-ssh-reviewer',candidate_sha:CANDIDATE_SHA,capability_id:c.id,outcome_ids:[c.benchmark.native_outcomes[0].id],evidence_digest:'a'.repeat(64),review_commit:'b'.repeat(40),receipt_sha256:'c'.repeat(64),receipt_canonical_sha256:'d'.repeat(64),registry_canonical_sha256:'e'.repeat(64),jurisdiction_canonical_sha256:'f'.repeat(64),receipt_path:`docs/evidence/console/reviews/${c.id}/${CANDIDATE_SHA}.json`};
  assert.throws(() => validateConsoleTruthLedger(trusted, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA,  }), /canonical repository root/);
});

test('a passing verdict cannot be self-asserted by declaring no independent review', () => {
  // The receipt machinery above is rigorous ONLY on the `status !== 'HOLD'` branch. Leaving the
  // review at HOLD while claiming a passing verdict skipped every one of those controls,
  // including the prohibition on reviewing your own capability — a prohibition that is not a
  // control while "no reviewer" remains an accepted answer. Both words a capability owner would
  // have to write here are written by that same owner.
  const selfAsserted = structuredClone(registry); const cap = selfAsserted.capabilities[0];
  cap.benchmark.verdict = 'MEET';
  cap.candidate_evidence.status = 'VERIFIED';
  assert.equal(cap.benchmark.independent_outcome_review.status, 'HOLD');
  assert.throws(
    () => validateConsoleTruthLedger(selfAsserted, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }),
    /non-HOLD benchmark requires a non-HOLD independent outcome review/,
  );

  // The gate must bind the VERDICT, not merely the evidence word beside it: flipping evidence
  // back to HOLD has to keep failing, or the two assertions would be satisfiable one at a time.
  const evidenceHeld = structuredClone(registry); const held = evidenceHeld.capabilities[0];
  held.benchmark.verdict = 'MEET';
  assert.throws(
    () => validateConsoleTruthLedger(evidenceHeld, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }),
    /non-HOLD benchmark requires verified candidate evidence/,
  );

  // And it must stay inert for the shipped documents, where everything is HOLD.
  assert.doesNotThrow(() => validateConsoleTruthLedger(structuredClone(registry), jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }));
});

test('Korea trace bijection rejects missing trace', () => {
  const bad=structuredClone(jurisdiction); bad.controls[0].capability_traceability.pop();
  assert.throws(() => validateConsoleTruthLedger(registry,bad,{expectedCandidateSha:CANDIDATE_SHA}),/bidirectional|bijection/);
  // A binding naming a control the register does not carry. Untested until now: deleting the
  // `controls.has(...)` guard turned this into an unhandled TypeError rather than a red test,
  // which is how a check that nothing exercises looks from the outside.
  const dangling=structuredClone(registry); dangling.capabilities[0].jurisdiction_bindings[0].control_id='CTRL-KR-DOES-NOT-EXIST';
  assert.throws(() => validateConsoleTruthLedger(dangling,jurisdiction,{expectedCandidateSha:CANDIDATE_SHA}),/has missing jurisdiction control CTRL-KR-DOES-NOT-EXIST/);
});


test('Korea exactness rejects duplicate bindings and duplicate targets', () => {
  const duplicateTarget = structuredClone(jurisdiction); duplicateTarget.target_jurisdiction_set = ['KR', 'KR'];
  assert.throws(() => validateConsoleTruthLedger(registry, duplicateTarget, { expectedCandidateSha: CANDIDATE_SHA }), /jurisdiction target/);
  const duplicateBinding = structuredClone(registry); const cap = duplicateBinding.capabilities[0]; cap.jurisdiction_bindings.push(structuredClone(cap.jurisdiction_bindings[0]));
  assert.throws(() => validateConsoleTruthLedger(duplicateBinding, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }), /duplicate jurisdiction binding/);
});


test('a real SSH-signed canonical Git receipt is the only non-HOLD admission path', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'console-receipt-'));
  const run = (args) => execFileSync('git', ['-C', root, ...args], { encoding: 'utf8' }).trim();
  try {
    run(['init']); run(['config', 'user.name', 'Jason Lee']); run(['config', 'user.email', 'jason19931225@gmail.com']);
    const signingKey = path.join(root, 'review_key'); execFileSync('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-f', signingKey]);
    run(['config', 'gpg.format', 'ssh']); run(['config', 'user.signingkey', signingKey]);
    const allowed = path.join(root, 'allowed_signers'); writeFileSync(allowed, 'jason19931225@gmail.com ' + readFileSync(`${signingKey}.pub`, 'utf8'));
    run(['config', 'gpg.ssh.allowedSignersFile', allowed]);
    const fingerprint = execFileSync('ssh-keygen', ['-lf', `${signingKey}.pub`, '-E', 'sha256'], { encoding: 'utf8' }).trim().split(/\s+/)[1];
    const candidateSigningAuthority = { format: 'ssh', principal: 'jason19931225@gmail.com', fingerprint };
    const publicKey = readFileSync(`${signingKey}.pub`, 'utf8').trim().split(/\s+/).slice(0, 2).join(' ');
    mkdirSync(path.join(root, 'scripts/console'), { recursive: true }); mkdirSync(path.join(root, '.github/trust'), { recursive: true }); mkdirSync(path.join(root, 'docs/program'), { recursive: true }); writeFileSync(path.join(root, 'scripts/console/control.mjs'), 'export const control = true;\n'); writeFileSync(path.join(root, 'candidate.txt'), 'candidate\n'); writeFileSync(path.join(root, '.github/trust/console.allowed_signers'), `jason19931225@gmail.com ${publicKey}\n`); for (const file of ['console-capability-registry.json', 'console-jurisdiction-register.json', 'console-program-ledger.md']) writeFileSync(path.join(root, 'docs/program', file), 'candidate\n'); run(['add', '.']); run(['commit', '-S', '-m', 'candidate']);
    const candidateSha = run(['rev-parse', 'HEAD']);
    for (const file of ['console-capability-registry.json', 'console-jurisdiction-register.json', 'console-program-ledger.md']) writeFileSync(path.join(root, 'docs/program', file), 'authority\n'); run(['add', '.']); run(['commit', '-S', '-m', 'authority control']);
    const authorityTip = run(['rev-parse', 'HEAD']);
    run(['config', '--unset', 'gpg.ssh.allowedSignersFile']);
    assert.equal(createConsoleCandidateSourceResolver(root, candidateSha, authorityTip, { candidateSigningAuthority }).readText('candidate.txt'), 'candidate\n');
    // A union merge resolution that keeps both entries but leaves the marker line behind is
    // otherwise invisible: the tip is correctly signed, is the candidate's only child, and
    // modifies exactly the three authority documents. Nine such lines reached main. Each of
    // the three asymmetric markers must fail on its own — a resolution that stripped two of
    // three is exactly what happened, so testing only `<<<<<<<` would have passed on the
    // real defect.
    for (const marker of ['<<<<<<< ours', '||||||| 18a21d7cd', '>>>>>>> theirs']) {
      run(['checkout', '-B', `marker-${marker.slice(0, 3)}`, candidateSha]);
      for (const file of ['console-capability-registry.json', 'console-jurisdiction-register.json', 'console-program-ledger.md']) writeFileSync(path.join(root, 'docs/program', file), 'authority\n');
      writeFileSync(path.join(root, 'docs/program/console-program-ledger.md'), `authority\n${marker}\nboth entries kept\n`);
      run(['add', '.']); run(['commit', '-S', '-m', 'authority control with unresolved marker']);
      assert.throws(
        () => createConsoleCandidateSourceResolver(root, candidateSha, run(['rev-parse', 'HEAD']), { candidateSigningAuthority }),
        /unresolved merge marker/,
        `a tip whose ledger line starts with ${marker.split(' ')[0]} must be refused`,
      );
    }
    // `=======` is a Markdown setext heading rule as well as a conflict marker. Failing on it
    // would fail the ledger on ordinary prose, so it is not a marker here — and that
    // exemption has to stay proven, not just commented.
    run(['checkout', '-B', 'setext-authority', candidateSha]);
    for (const file of ['console-capability-registry.json', 'console-jurisdiction-register.json', 'console-program-ledger.md']) writeFileSync(path.join(root, 'docs/program', file), 'authority\n');
    writeFileSync(path.join(root, 'docs/program/console-program-ledger.md'), 'A heading\n=======\n');
    run(['add', '.']); run(['commit', '-S', '-m', 'authority control with setext heading']);
    assert.doesNotThrow(() => createConsoleCandidateSourceResolver(root, candidateSha, run(['rev-parse', 'HEAD']), { candidateSigningAuthority }));
    run(['checkout', authorityTip]);
    const hostileHome = mkdtempSync(path.join(tmpdir(), 'console-hostile-home-'));
    const hostileVerifier = path.join(hostileHome, 'forged-ssh-keygen'); const hostileMarker = path.join(hostileHome, 'invoked');
    writeFileSync(hostileVerifier, `#!/bin/sh\ntouch '${hostileMarker}'\nprintf '%s\\n' 'Good "git" signature for ${candidateSigningAuthority.principal} with ED25519 key ${fingerprint}'\n`); chmodSync(hostileVerifier, 0o700);
    writeFileSync(path.join(hostileHome, '.gitconfig'), `[gpg "ssh"]\n\tprogram = ${hostileVerifier}\n`);
    const originalHome = process.env.HOME; process.env.HOME = hostileHome;
    try { assert.doesNotThrow(() => verifyCommitWithCandidateSshPolicy(root, candidateSha, authorityTip, candidateSigningAuthority)); assert.equal(existsSync(hostileMarker), false); } finally { process.env.HOME = originalHome; rmSync(hostileHome, { recursive: true, force: true }); }
    run(['checkout', '-b', 'renamed-authority', authorityTip]);
    run(['mv', 'scripts/console/control.mjs', 'scripts/console/renamed-control.mjs']); run(['commit', '--no-gpg-sign', '-m', 'rename authority control']);
    assert.throws(() => createConsoleCandidateSourceResolver(root, candidateSha, run(['rev-parse', 'HEAD']), { candidateSigningAuthority }), /integration tip commit signature|direct single-parent child/);
    run(['checkout', '-B', 'symlink-authority', authorityTip]);
    rmSync(path.join(root, 'scripts/console/control.mjs')); symlinkSync('target.mjs', path.join(root, 'scripts/console/control.mjs')); run(['add', '-A']); run(['commit', '--no-gpg-sign', '-m', 'symlink authority control']);
    assert.throws(() => createConsoleCandidateSourceResolver(root, candidateSha, run(['rev-parse', 'HEAD']), { candidateSigningAuthority }), /integration tip commit signature|direct single-parent child/);
    run(['checkout', '-B', 'product-drift', authorityTip]);
    writeFileSync(path.join(root, 'product.txt'), 'forbidden\n'); run(['add', '.']); run(['commit', '--no-gpg-sign', '-m', 'product drift']);
    const productTip = run(['rev-parse', 'HEAD']);
    assert.throws(() => createConsoleCandidateSourceResolver(root, candidateSha, productTip, { candidateSigningAuthority }), /integration tip commit signature|direct single-parent child/);
    assert.throws(() => createConsoleCandidateSourceResolver(root, productTip, productTip, { candidateSigningAuthority }), /candidate commit signature/);
    const orphanTip = run(['commit-tree', run(['write-tree']), '-m', 'unrelated authority']);
    assert.throws(() => createConsoleCandidateSourceResolver(root, candidateSha, orphanTip, { candidateSigningAuthority }), /integration tip commit signature|direct single-parent child/);
    const promoted = structuredClone(registry); const fixtureJurisdiction = structuredClone(jurisdiction); const cap = promoted.capabilities[0];
    // Nothing here rebinds a SHA into the documents any more: the candidate is a parameter, and
    // the fixture only has to move the two provenance anchors off it so they stay distinct.
    promoted.provenance.authority_base_sha = 'a'.repeat(40); promoted.provenance.historical_implementation_freeze_sha = 'b'.repeat(40);
    fixtureJurisdiction.provenance.historical_implementation_freeze_sha = promoted.provenance.historical_implementation_freeze_sha;
    cap.candidate_evidence.status = 'VERIFIED'; cap.benchmark.verdict = 'MEET';
    const receiptPath = `docs/evidence/console/reviews/${cap.id}/${candidateSha}.json`; mkdirSync(path.dirname(path.join(root, receiptPath)), { recursive: true });
    Object.assign(promoted.review_authority.reviewers[0], { author_name: 'Jason Lee', author_email: 'jason19931225@gmail.com', committer_name: 'Jason Lee', committer_email: 'jason19931225@gmail.com', signing: { format: 'ssh', principal: 'jason19931225@gmail.com', fingerprint } });
    const payload = { candidate_sha: candidateSha, capability_id: cap.id, outcome_ids: [cap.benchmark.native_outcomes[0].id], evidence_digest: 'a'.repeat(64), verdict: 'MEET', reviewer_id: 'jasonlee-ssh-reviewer' };
    const authorityDigests = promotionAuthorityDigests(promoted, fixtureJurisdiction);
    payload.registry_canonical_sha256 = authorityDigests.registry;
    payload.jurisdiction_canonical_sha256 = authorityDigests.jurisdiction;
    const raw = `${JSON.stringify(payload)}\n`; writeFileSync(path.join(root, receiptPath), raw); run(['add', receiptPath]); run(['commit', '-S', '-m', 'review receipt']);
    const reviewCommit = run(['rev-parse', 'HEAD']);
    const [authorName, authorEmail, committerName, committerEmail] = run(['show', '-s', '--format=%an%x00%ae%x00%cn%x00%ce', reviewCommit]).split('\0');
    assert.deepEqual({ authorName, authorEmail, committerName, committerEmail }, { authorName: 'Jason Lee', authorEmail: 'jason19931225@gmail.com', committerName: 'Jason Lee', committerEmail: 'jason19931225@gmail.com' });
    const canonicalDigest = createHash('sha256').update(JSON.stringify(Object.fromEntries(Object.entries(payload).sort(([a,], [b,]) => a.localeCompare(b, 'en'))))).digest('hex');
    cap.benchmark.independent_outcome_review = { status: 'MEET', reviewer_id: payload.reviewer_id, candidate_sha: candidateSha, capability_id: cap.id, outcome_ids: payload.outcome_ids, evidence_digest: payload.evidence_digest, review_commit: reviewCommit, receipt_path: receiptPath, receipt_sha256: createHash('sha256').update(raw).digest('hex'), receipt_canonical_sha256: canonicalDigest, registry_canonical_sha256: payload.registry_canonical_sha256, jurisdiction_canonical_sha256: payload.jurisdiction_canonical_sha256 };
    assert.doesNotThrow(() => validateConsoleTruthLedger(promoted, fixtureJurisdiction, { expectedCandidateSha: candidateSha, repoRoot: root }));
    const forged = structuredClone(promoted); forged.capabilities[0].benchmark.independent_outcome_review.receipt_path = 'package.json';
    assert.throws(() => validateConsoleTruthLedger(forged, fixtureJurisdiction, { expectedCandidateSha: candidateSha, repoRoot: root }), /not canonical/);
    const wrongDigest = structuredClone(promoted); wrongDigest.capabilities[0].benchmark.independent_outcome_review.receipt_sha256 = '0'.repeat(64);
    assert.throws(() => validateConsoleTruthLedger(wrongDigest, fixtureJurisdiction, { expectedCandidateSha: candidateSha, repoRoot: root }), /digest/);
    const wrongAuthorityDigest = structuredClone(promoted); wrongAuthorityDigest.capabilities[0].benchmark.independent_outcome_review.registry_canonical_sha256 = '0'.repeat(64);
    assert.throws(() => validateConsoleTruthLedger(wrongAuthorityDigest, fixtureJurisdiction, { expectedCandidateSha: candidateSha, repoRoot: root }), /candidate and authority digests/);
    // REPLAY PROTECTION, deliberately untouched by the strip: this `candidate_sha` originates in
    // a separately signed receipt, so comparing it to the candidate is a real cross-document
    // check rather than a value compared against a copy of itself.
    // The receipt above is otherwise valid and accepted, so `candidate_sha` is the only failing
    // disjunct here.
    const replayed = structuredClone(promoted); replayed.capabilities[0].benchmark.independent_outcome_review.candidate_sha = 'f'.repeat(40);
    assert.throws(() => validateConsoleTruthLedger(replayed, fixtureJurisdiction, { expectedCandidateSha: candidateSha, repoRoot: root }), /non-HOLD review receipt schema is invalid/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('candidate resolver requires the signed authority tip to be the direct three-document child of C', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'console-authority-tip-'));
  const run = (args) => execFileSync('git', ['-C', root, ...args], { encoding: 'utf8' }).trim();
  try {
    run(['init']); run(['config', 'user.name', 'Jason Lee']); run(['config', 'user.email', 'jason19931225@gmail.com']);
    const signingKey = path.join(root, 'key'); execFileSync('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-f', signingKey]);
    run(['config', 'gpg.format', 'ssh']); run(['config', 'user.signingkey', signingKey]);
    const publicKey = readFileSync(`${signingKey}.pub`, 'utf8').trim().split(/\s+/).slice(0, 2).join(' ');
    const fingerprint = execFileSync('ssh-keygen', ['-lf', `${signingKey}.pub`, '-E', 'sha256'], { encoding: 'utf8' }).trim().split(/\s+/)[1];
    const authority = { format: 'ssh', principal: 'jason19931225@gmail.com', fingerprint };
    mkdirSync(path.join(root, '.github/trust'), { recursive: true });
    mkdirSync(path.join(root, 'docs/program'), { recursive: true });
    writeFileSync(path.join(root, '.github/trust/console.allowed_signers'), `jason19931225@gmail.com ${publicKey}\n`);
    for (const file of ['console-capability-registry.json', 'console-jurisdiction-register.json', 'console-program-ledger.md']) writeFileSync(path.join(root, 'docs/program', file), 'C\n');
    writeFileSync(path.join(root, 'candidate.txt'), 'C\n'); run(['add', '.']); run(['commit', '-S', '-m', 'C']);
    const candidate = run(['rev-parse', 'HEAD']);
    for (const file of ['console-capability-registry.json', 'console-jurisdiction-register.json', 'console-program-ledger.md']) writeFileSync(path.join(root, 'docs/program', file), 'T\n');
    run(['add', '.']); run(['commit', '-S', '-m', 'T']); const tip = run(['rev-parse', 'HEAD']);
    assert.doesNotThrow(() => createConsoleCandidateSourceResolver(root, candidate, tip, { candidateSigningAuthority: authority }));
    run(['commit', '--allow-empty', '--no-gpg-sign', '-m', 'unsigned extra']);
    assert.throws(() => createConsoleCandidateSourceResolver(root, candidate, run(['rev-parse', 'HEAD']), { candidateSigningAuthority: authority }), /integration tip commit signature|direct single-parent child/);
    run(['checkout', '-B', 'indirect', tip]); writeFileSync(path.join(root, 'docs/program/console-program-ledger.md'), 'extra\n'); run(['add', '.']); run(['commit', '-S', '-m', 'indirect']);
    assert.throws(() => createConsoleCandidateSourceResolver(root, candidate, run(['rev-parse', 'HEAD']), { candidateSigningAuthority: authority }), /direct single-parent child/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('the candidate SHA comes only from the caller, and a re-added stored copy buys nothing', () => {
  // Any caller SHA validates against the shipped registers, whatever they happen to declare: the
  // SHA is a parameter now, derived by CI from git parentage, and nothing in either document is
  // read to check it. Two different SHAs both passing is not a hole — it is the point, and the
  // registers are what stopped being rewritten.
  for (const candidateSha of [CANDIDATE_SHA, 'a1'.repeat(20)]) {
    assert.doesNotThrow(() => validateConsoleTruthLedger(registry, jurisdiction, { expectedCandidateSha: candidateSha }));
  }
  // ACCEPTED LOSS, asserted rather than hidden: the validator no longer reads a stored copy, so
  // putting one back — right or wrong — changes nothing. Anyone re-adding the field to "restore"
  // a check must change code as well, which is where the argument belongs. The bound is that
  // every row is HOLD, that Buck targets and route facts are read from C, and that a promoted
  // capability binds `registry_canonical_sha256` in a separately signed receipt.
  const restored = structuredClone(registry); restored.candidate = { sha: 'b'.repeat(40) };
  for (const capability of restored.capabilities) capability.candidate_evidence.candidate_sha = 'b'.repeat(40);
  assert.doesNotThrow(() => validateConsoleTruthLedger(restored, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }));

  // What did NOT move: the payloads must exist, in both documents.
  const noEvidence = structuredClone(registry); delete noEvidence.capabilities[0].candidate_evidence;
  assert.throws(() => validateConsoleTruthLedger(noEvidence, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }), /candidate evidence must be an object/);
  const noControlEvidence = structuredClone(jurisdiction); delete noControlEvidence.controls[0].candidate_evidence;
  assert.throws(() => validateConsoleTruthLedger(registry, noControlEvidence, { expectedCandidateSha: CANDIDATE_SHA }), /control candidate evidence must be an object/);

  // …and EXISTENCE ALONE IS NOT WHAT WAS DELETED. `control.candidate_evidence?.candidate_sha !==
  // candidate.sha` refused an empty payload too, because `undefined !== sha`, so replacing it
  // with `object(...)` on its own would have been a weakening dressed up as a rename. The control
  // payload carries the same explicit `status` and `reason` checks the capability rows carry.
  const emptyControlEvidence = structuredClone(jurisdiction); emptyControlEvidence.controls[0].candidate_evidence = {};
  assert.throws(() => validateConsoleTruthLedger(registry, emptyControlEvidence, { expectedCandidateSha: CANDIDATE_SHA }), /control evidence status is invalid/);
  const noControlReason = structuredClone(jurisdiction); delete noControlReason.controls[0].candidate_evidence.reason;
  assert.throws(() => validateConsoleTruthLedger(registry, noControlReason, { expectedCandidateSha: CANDIDATE_SHA }), /control evidence reason must be a non-empty string/);
  const badControlStatus = structuredClone(jurisdiction); badControlStatus.controls[0].candidate_evidence.status = 'SHIPPED';
  assert.throws(() => validateConsoleTruthLedger(registry, badControlStatus, { expectedCandidateSha: CANDIDATE_SHA }), /control evidence status is invalid/);
  const emptyCapabilityEvidence = structuredClone(registry); emptyCapabilityEvidence.capabilities[0].candidate_evidence = {};
  assert.throws(() => validateConsoleTruthLedger(emptyCapabilityEvidence, jurisdiction, { expectedCandidateSha: CANDIDATE_SHA }), /candidate evidence status is invalid/);
});

test('the authority-only diff accepts one document and added ledger entries, and refuses added files elsewhere', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'console-authority-diff-'));
  const run = (args) => execFileSync('git', ['-C', root, ...args], { encoding: 'utf8' }).trim();
  const AUTHORITY_FILES = ['console-capability-registry.json', 'console-jurisdiction-register.json', 'console-program-ledger.md'];
  try {
    run(['init']); run(['config', 'user.name', 'Jason Lee']); run(['config', 'user.email', 'jason19931225@gmail.com']);
    const signingKey = path.join(root, 'key'); execFileSync('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-f', signingKey]);
    run(['config', 'gpg.format', 'ssh']); run(['config', 'user.signingkey', signingKey]);
    const publicKey = readFileSync(`${signingKey}.pub`, 'utf8').trim().split(/\s+/).slice(0, 2).join(' ');
    const fingerprint = execFileSync('ssh-keygen', ['-lf', `${signingKey}.pub`, '-E', 'sha256'], { encoding: 'utf8' }).trim().split(/\s+/)[1];
    const candidateSigningAuthority = { format: 'ssh', principal: 'jason19931225@gmail.com', fingerprint };
    mkdirSync(path.join(root, '.github/trust'), { recursive: true });
    mkdirSync(path.join(root, 'docs/program/ledger'), { recursive: true });
    writeFileSync(path.join(root, '.github/trust/console.allowed_signers'), `jason19931225@gmail.com ${publicKey}\n`);
    for (const file of AUTHORITY_FILES) writeFileSync(path.join(root, 'docs/program', file), 'C\n');
    writeFileSync(path.join(root, 'docs/program/ledger/0001-existing.md'), 'existing entry\n');
    writeFileSync(path.join(root, 'product.txt'), 'C\n');
    run(['add', '--', '.github', 'docs', 'product.txt']); run(['commit', '-S', '-m', 'C']);
    const candidateSha = run(['rev-parse', 'HEAD']);
    const tip = (relativePath, body) => {
      run(['checkout', '-q', '--detach', candidateSha]);
      if (relativePath) {
        const absolute = path.join(root, relativePath);
        mkdirSync(path.dirname(absolute), { recursive: true });
        writeFileSync(absolute, body);
        run(['add', '--', relativePath]);
      }
      run(['commit', '-S', '--allow-empty', '-m', 'T']);
      return run(['rev-parse', 'HEAD']);
    };
    const resolver = (relativePath, body) => createConsoleCandidateSourceResolver(root, candidateSha, tip(relativePath, body), { candidateSigningAuthority });
    // One document is enough; the allow-list, not the count, is the control.
    for (const file of AUTHORITY_FILES) assert.doesNotThrow(() => resolver(`docs/program/${file}`, 'T\n'), file);
    // Added ledger entries are accepted under the prefix, and modified ones stay accepted.
    assert.doesNotThrow(() => resolver('docs/program/ledger/2026-08-01-pr-1.md', 'entry\n'));
    assert.doesNotThrow(() => resolver('docs/program/ledger/0001-existing.md', 'edited\n'));
    // A new entry whose content matches a file already in the tree. This reader used to pass
    // `--find-renames --find-copies-harder`, which reports any source ≥50% similar as status
    // `C` with two paths, while the two gates that decide the merge pass `--no-renames` and see
    // `A` with one. Identical bytes are used here only to make the similarity score
    // deterministic; the divergence starts at 50%. Identical flags in all three readers now.
    assert.doesNotThrow(() => resolver('docs/program/ledger/0002-near-copy.md', 'existing entry\n'));
    // The prefix is a FLAT directory of `.md` entries: no subdirectory, no other extension.
    for (const file of ['docs/program/ledger/nested/2026-08-01-pr-1.md', 'docs/program/ledger/entry.mjs', 'docs/program/ledger/entry.txt']) {
      assert.throws(() => resolver(file, 'entry\n'), /product path after candidate|unsupported diff status/, file);
    }
    // Adding anywhere else is refused — including a near-miss sibling of the directory name.
    for (const file of ['docs/program/ledgerbook.md', 'docs/program/console-program-ledger-2.md', 'README.md']) {
      assert.throws(() => resolver(file, 'new\n'), /product path after candidate|unsupported diff status/, file);
    }
    // A non-authority MODIFICATION is unchanged, and an empty tip asserts nothing.
    assert.throws(() => resolver('product.txt', 'T\n'), /product path after candidate/);
    assert.throws(() => resolver(null), /at least one authority document/);
    // The unresolved-merge scan follows the ledger into its directory. Nine such lines reached
    // main through the one file everybody edited; moving to one file per entry must not move
    // the entries out from under the check.
    assert.throws(() => resolver('docs/program/ledger/2026-08-01-merged.md', 'both entries kept\n||||||| 18a21d7cd\nand the marker left behind\n'), /unresolved merge marker/);
    // …including an entry that arrived on an EARLIER train and is untouched by this one.
    const withMarker = tip('docs/program/ledger/2026-08-01-merged.md', '<<<<<<< ours\nkept\n');
    run(['checkout', '-q', '-B', 'later-train', withMarker]);
    writeFileSync(path.join(root, 'docs/program/console-program-ledger.md'), 'T\n');
    run(['add', '--', 'docs/program/console-program-ledger.md']); run(['commit', '-S', '-m', 'later T']);
    assert.throws(() => createConsoleCandidateSourceResolver(root, withMarker, run(['rev-parse', 'HEAD']), { candidateSigningAuthority }), /unresolved merge marker/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('Korea jurisdiction rows require exactly one canonical JUR-KR-001 row', () => {
  const empty = structuredClone(jurisdiction); empty.jurisdictions = [];
  assert.throws(() => validateConsoleTruthLedger(registry, empty, { expectedCandidateSha: CANDIDATE_SHA }), /jurisdiction target/);
  const duplicate = structuredClone(jurisdiction); duplicate.jurisdictions.push(structuredClone(duplicate.jurisdictions[0]));
  assert.throws(() => validateConsoleTruthLedger(registry, duplicate, { expectedCandidateSha: CANDIDATE_SHA }), /jurisdiction target/);
});
