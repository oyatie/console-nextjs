#!/usr/bin/env node

import { createHash } from 'node:crypto';

import path from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import { createConsoleBuckTargetResolver, createConsoleCandidateSourceResolver, extractConsoleRouteFactsFromCandidate, isValidatedConsoleTruthLedger, validateConsoleTruthLedger } from './validate-console-truth-ledger.mjs';
import { verifyConsoleAuthorityTrain } from './verify-console-authority-train.mjs';
import { RELEASE_PLEASE_TRAIN_CLASS } from './release-please-bot-candidate.mjs';
import { sshSignatureMatchesAuthority, verifyCommitWithCandidateSshPolicy } from './ssh-signature-policy.mjs';

const FULL_SHA = /^[0-9a-f]{40}$/;
const QUALITY_BIAS_DEFAULT = 0.6;
const RESOURCE_KEYS = ['writer', 'postgres', 'browser', 'ios', 'graph', 'cas'];
const COLD_RUST_COMPILE_LANES = 2;
const COLD_RUST_COMPILE_JOBS = 6;
const validatedAttestationTokens = new WeakSet();

function compareText(a, b) { return a.localeCompare(b, 'en'); }
function arrays(value) { return Array.isArray(value) ? value : []; }
function finiteNumber(value, fallback = 0) { return Number.isFinite(value) ? value : fallback; }
function round(value) { return Number(value.toFixed(6)); }
function stableValue(value) {
  if (Array.isArray(value)) return value.map(stableValue);
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).sort(([a], [b]) => compareText(a, b)).map(([key, child]) => [key, stableValue(child)]));
  return value;
}
function stableJson(value) { return JSON.stringify(stableValue(value)); }
function digest(value) { return createHash('sha256').update(stableJson(value)).digest('hex'); }
function registryDigest(registry) { return digest({ ...registry, capabilities: [...registry.capabilities].sort((a, b) => compareText(a.id, b.id)) }); }

/** Only repository literals and a terminal subtree suffix are ownership syntax. */
export function normalizePattern(value) {
  if (typeof value !== 'string' || value.trim() !== value || value === '') throw new Error('ownership pattern must be a non-empty canonical string');
  if (value.includes('\\') || value.includes('\0') || value.startsWith('/') || value.endsWith('/') || /[?*[{]/.test(value.replace(/\/\*\*$/, ''))) throw new Error(`unsupported ownership wildcard or path alias: ${value}`);
  const subtree = value.endsWith('/**');
  const literal = subtree ? value.slice(0, -3) : value;
  if (literal === '' || literal === '.' || literal === '..' || literal.startsWith('./') || literal.startsWith('../') || literal.includes('/./') || literal.includes('/../') || literal.split('/').some((segment) => segment === '' || segment === '.' || segment === '..')) throw new Error(`ownership pattern must be a repository-relative literal: ${value}`);
  if (!/^[A-Za-z0-9._/-]+$/.test(literal)) throw new Error(`ownership pattern contains unsupported characters: ${value}`);
  return subtree ? `${literal}/**` : literal;
}
function isSubtree(pattern) { return pattern.endsWith('/**'); }
function patternRoot(pattern) { const normalized = normalizePattern(pattern); return isSubtree(normalized) ? normalized.slice(0, -3) : normalized; }
function boundaryPrefix(prefix, candidate) { return prefix === candidate || candidate.startsWith(`${prefix}/`); }
export function patternsOverlap(left, right) {
  const leftNormal = normalizePattern(left); const rightNormal = normalizePattern(right);
  const leftRoot = patternRoot(leftNormal); const rightRoot = patternRoot(rightNormal);
  if (!isSubtree(leftNormal) && !isSubtree(rightNormal)) return leftRoot === rightRoot;
  if (isSubtree(leftNormal) && isSubtree(rightNormal)) return boundaryPrefix(leftRoot, rightRoot) || boundaryPrefix(rightRoot, leftRoot);
  return isSubtree(leftNormal) ? boundaryPrefix(leftRoot, rightRoot) : boundaryPrefix(rightRoot, leftRoot);
}
function patternFullyCovers(parent, child) {
  const normalizedParent = normalizePattern(parent); const normalizedChild = normalizePattern(child);
  return normalizedParent === normalizedChild || (isSubtree(normalizedParent) && boundaryPrefix(patternRoot(normalizedParent), patternRoot(normalizedChild)));
}
function validatePatternList(values, label) { return arrays(values).map((value) => { try { return normalizePattern(value); } catch (error) { throw new Error(`${label}: ${error.message}`); } }); }
function stateText(capability, key) {
  // Own-property only — capability.state from JSON inherits Object.prototype.
  const state = capability.state;
  const value = state && typeof state === 'object' && Object.hasOwn(state, key) ? state[key] : undefined;
  return typeof value === 'string' ? value : '';
}
function backendSettled(value) { return value === 'not_applicable' || value.startsWith('existing_real_') || ['integrated_on_local_train', 'integrated_dark_on_pr488', 'complete'].includes(value); }
function frontendSettled(value) { return value === 'not_applicable' || ['integrated_on_local_train', 'integrated_dark_on_pr488', 'complete'].includes(value); }
function sourceComplete(capability) { return backendSettled(stateText(capability, 'backend')) && frontendSettled(stateText(capability, 'frontend')); }
function laneSourceComplete(capability, kind) { return kind.includes('frontend') ? frontendSettled(stateText(capability, 'frontend')) : kind.includes('backend') ? backendSettled(stateText(capability, 'backend')) : sourceComplete(capability); }
function capabilityRoots(capability) { const ownership = capability.ownership ?? {}; return [...arrays(ownership.frontend_roots), ...arrays(ownership.backend_roots), ...arrays(ownership.api_schema_roots)]; }
function validResources(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const normalized = {};
  for (const key of RESOURCE_KEYS) {
    if (!Number.isInteger(value[key]) || value[key] < 0) return null;
    normalized[key] = value[key];
  }
  return normalized.writer === 1 ? normalized : null;
}
function resourceBudgets(registry) {
  const raw = registry.resource_budgets;
  if (!raw || typeof raw !== 'object') throw new Error('missing resource_budgets authority');
  const result = {};
  for (const key of RESOURCE_KEYS) {
    if (!Number.isInteger(raw[key]) || raw[key] < 0) throw new Error(`invalid resource budget: ${key}`);
    result[key] = raw[key];
  }
  if (result.writer < 1) throw new Error('writer budget must be positive');
  return result;
}
function normalizedEpochLanes(registry) {
  const epoch = registry.fanout_epoch;
  if (epoch === undefined) return null;
  if (!epoch || typeof epoch !== 'object' || !Number.isInteger(epoch.current_epoch) || epoch.current_epoch < 0 || !Array.isArray(epoch.normalized_lane_ids) || epoch.normalized_lane_ids.some((laneId) => typeof laneId !== 'string') || new Set(epoch.normalized_lane_ids).size !== epoch.normalized_lane_ids.length) throw new Error('invalid fanout epoch authority');
  return new Set(epoch.normalized_lane_ids);
}
function sourceLaneDefinitions(capability) {
  const assignments = capability.lane_assignments ?? {};
  const split = Object.entries(assignments).filter(([name, entry]) => name !== 'consolidation' && entry && typeof entry === 'object' && arrays(entry.roots).length > 0).sort(([a], [b]) => compareText(a, b)).map(([kind, entry]) => ({
    laneId: `${capability.id}#${kind}`, kind, owner: entry.owner, worktree: entry.worktree, branch: entry.branch, roots: arrays(entry.roots),
    leafCommands: arrays(entry.tests?.leaf_commands).length ? arrays(entry.tests.leaf_commands) : arrays(capability.tests?.leaf_commands),
    buckTargets: arrays(entry.tests?.buck2_targets).length ? arrays(entry.tests.buck2_targets) : arrays(capability.tests?.buck2_targets),
    resources: entry.resources ?? capability.resource_requirements,
  }));
  if (split.length) return split;
  return [{ laneId: capability.id, kind: 'source', owner: capability.owner, worktree: capability.worktree, branch: capability.branch, roots: capabilityRoots(capability), leafCommands: arrays(capability.tests?.leaf_commands), buckTargets: arrays(capability.tests?.buck2_targets), resources: capability.resource_requirements }];
}
function classifyRoots(rawRoots, sharedPatterns, migrationRoots) {
  const roots = validatePatternList(rawRoots, 'ownership root');
  const privateRoots = roots.filter((root) => !sharedPatterns.some((shared) => patternFullyCovers(shared, root)) && !migrationRoots.some((migration) => patternFullyCovers(migration, root)));
  const shared = sharedPatterns.filter((shared) => roots.some((root) => patternsOverlap(root, shared)));
  const migrations = migrationRoots.filter((migration) => roots.some((root) => patternsOverlap(root, migration)));
  return { roots: [...new Set(roots)].sort(compareText), privateRoots: [...new Set(privateRoots)].sort(compareText), sharedRoots: shared, migrationRoots: migrations, excludedSharedRoots: [...new Set([...shared, ...migrations])].sort(compareText) };
}
function laneRootReasons(laneRoots, capabilityPrivateRoots, protectedRoots) {
  const reasons = [];
  for (const root of validatePatternList(laneRoots, 'lane ownership root')) {
    if (protectedRoots.some((protectedRoot) => patternsOverlap(root, protectedRoot))) reasons.push('protected_shared_root_intersection');
    const owners = capabilityPrivateRoots.filter((ownedRoot) => patternFullyCovers(ownedRoot, root));
    if (owners.length !== 1) reasons.push('lane_root_outside_capability_private_ownership');
  }
  return [...new Set(reasons)];
}
export function buckIsolationDir(anchorSha, laneId) {
  if (!FULL_SHA.test(anchorSha) || typeof laneId !== 'string' || !/^[A-Za-z0-9#._-]+$/.test(laneId)) throw new Error('invalid immutable lane identity for Buck isolation');
  const slug = laneId.replace(/[^A-Za-z0-9]+/g, '-').replace(/^-|-$/g, '').toLowerCase();
  // The slug is only for operator readability.  The full lane identity hash
  // prevents distinct long IDs with the same readable prefix from colliding.
  return `.buck2/console-epochs/${anchorSha.slice(0, 12)}/${slug.slice(0, 72)}-${createHash('sha256').update(laneId).digest('hex')}`;
}
function laneScore(capability, qualityBias) { const score = finiteNumber(capability.priority?.score); const c = finiteNumber(capability.priority?.inputs?.correctness_and_risk_reduction, score); const v = finiteNumber(capability.priority?.inputs?.verification_readiness, score); return (1 - qualityBias) * score + qualityBias * ((c + v) / 2); }
function conflictBetween(left, right) { for (const a of left.private_roots) for (const b of right.private_roots) if (patternsOverlap(a, b)) return [a, b]; return null; }
function nonemptyIdentity(value) { return typeof value === 'string' && /^[A-Za-z0-9._@/-]+$/.test(value); }
function exactIdentity(value) { return typeof value === 'string' && value !== '' && value.trim() === value && !value.includes('\0'); }
function canonicalFingerprint(value) { return typeof value === 'string' && /^[A-F0-9]{40,64}$/.test(value); }
function sshFingerprint(value) { return typeof value === 'string' && /^SHA256:[A-Za-z0-9+/]+={0,2}$/.test(value); }
function canonicalReceiptDigest(receipt) { return createHash('sha256').update(stableJson(receipt)).digest('hex'); }
function receiptContentDigest(receipt) { const { review_commit: _reviewCommit, ...content } = receipt; return canonicalReceiptDigest(content); }
function signingAuthority(reviewer) {
  if (reviewer?.signing === undefined) return reviewer?.signing_fingerprint ? { format: 'gpg', fingerprint: reviewer.signing_fingerprint } : null;
  const signing = reviewer.signing;
  if (!signing || typeof signing !== 'object' || Array.isArray(signing)) return null;
  if (signing.format === 'gpg' && canonicalFingerprint(signing.fingerprint)) return { format: 'gpg', fingerprint: signing.fingerprint };
  if (signing.format === 'ssh' && exactIdentity(signing.principal) && sshFingerprint(signing.fingerprint)) return { format: 'ssh', principal: signing.principal, fingerprint: signing.fingerprint };
  return null;
}
export function validateReviewAuthority(authority) {
  if (authority === undefined) return [];
  if (!authority || typeof authority !== 'object' || !Array.isArray(authority.reviewers)) throw new Error('invalid trusted reviewer authority');
  const ids = new Set(), fingerprints = new Set();
  for (const reviewer of authority.reviewers) {
    if (!reviewer || typeof reviewer !== 'object' || !nonemptyIdentity(reviewer.id)) throw new Error('invalid trusted reviewer id');
    if (ids.has(reviewer.id)) throw new Error('duplicate trusted reviewer id');
    const signing = signingAuthority(reviewer);
    if (!signing) throw new Error('invalid trusted reviewer signing authority');
    const signingKey = `${signing.format}:${signing.fingerprint}`;
    if (fingerprints.has(signingKey)) throw new Error('duplicate trusted reviewer signing fingerprint');
    if (!exactIdentity(reviewer.author_name) || !exactIdentity(reviewer.author_email) || !exactIdentity(reviewer.committer_name) || !exactIdentity(reviewer.committer_email)) throw new Error('invalid trusted reviewer identity');
    ids.add(reviewer.id); fingerprints.add(signingKey);
  }
  return authority.reviewers;
}
function receiptPath(laneId) { return `docs/evidence/console/fanout-receipts/${createHash('sha256').update(laneId).digest('hex')}.json`; }
function staticReviewReceipt(receipt, epochBaseSha, lane, trustedReviewer) {
  if (!receipt || typeof receipt !== 'object' || receipt.status !== 'approved' || receipt.epoch_base_sha !== epochBaseSha) return null;
  if (receipt.lane_id !== lane.laneId || receipt.implementer !== lane.owner || !nonemptyIdentity(receipt.implementer) || !trustedReviewer || receipt.reviewer !== trustedReviewer.id || receipt.reviewer === receipt.implementer) return null;
  if (!FULL_SHA.test(receipt.leaf_commit ?? '') || !FULL_SHA.test(receipt.review_commit ?? '') || !/^[0-9a-f]{64}$/.test(receipt.leaf_result_sha256 ?? '')) return null;
  return receipt;
}
function trustedReviewer(authority, id) { return validateReviewAuthority(authority).find((entry) => entry.id === id) ?? null; }
function runtimeReceipt(options, lane) {
  const attestations = options.validatedAttestations;
  if (!attestations || typeof attestations !== 'object' || !validatedAttestationTokens.has(attestations)) return null;
  const receipts = attestations.receipts;
  const receipt = receipts && typeof receipts === 'object' && Object.hasOwn(receipts, lane.laneId)
    ? receipts[lane.laneId]
    : undefined;
  return staticReviewReceipt(receipt, options.anchorSha, lane, trustedReviewer(options.registry.review_authority, receipt?.reviewer)) ? receipt : null;
}
export function leafResultDigest(diff) { return createHash('sha256').update(diff).digest('hex'); }
export function signatureMatchesFingerprint(rawStatus, fingerprint) {
  if (typeof rawStatus !== 'string' || !canonicalFingerprint(fingerprint)) return false;
  const signatureLines = rawStatus.split(/\r?\n/).filter((line) => line.startsWith('[GNUPG:] VALIDSIG'));
  if (signatureLines.length !== 1) return false;
  const match = signatureLines[0].match(/^\[GNUPG:\] VALIDSIG ([A-F0-9]{40,64})(?:\s|$)/);
  return match?.[1] === fingerprint;
}
export function signatureMatchesAuthority(rawStatus, authority) {
  if (authority?.format === 'gpg') return signatureMatchesFingerprint(rawStatus, authority.fingerprint);
  if (authority?.format !== 'ssh' || !exactIdentity(authority.principal) || !sshFingerprint(authority.fingerprint) || typeof rawStatus !== 'string') return false;
  return sshSignatureMatchesAuthority(rawStatus, authority);
}
export function validateReviewReceiptForAnchor(receipt, anchor, lane, authority, operations) {
  const trusted = trustedReviewer(authority, receipt?.reviewer);
  const valid = staticReviewReceipt(receipt, anchor, lane, trusted);
  if (!valid) throw new Error('review receipt is not an exact trusted leaf result receipt');
  if (!operations.hasCommit(valid.leaf_commit)) throw new Error('review receipt leaf commit does not exist');
  if (!operations.isAncestor(anchor, valid.leaf_commit)) throw new Error('review receipt leaf commit is not anchored to the epoch');
  if (operations.parentCount(valid.leaf_commit) !== 1 || operations.parentCount(valid.review_commit) !== 1) throw new Error('leaf and review commits must be single-parent');
  if (operations.parentOf(valid.review_commit) !== valid.leaf_commit) throw new Error('review receipt review commit must be the direct child of the leaf');
  const leafPaths = operations.changedPaths(valid.leaf_commit);
  if (!Array.isArray(lane.privateRoots) || !lane.privateRoots.length || leafPaths.some((changedPath) => lane.protectedRoots?.some((protectedRoot) => patternsOverlap(changedPath, protectedRoot)) || !lane.privateRoots.some((privateRoot) => patternFullyCovers(privateRoot, changedPath)))) throw new Error('leaf commit mutates outside validated lane private roots');
  if (operations.changedPaths(valid.review_commit).length !== 1 || operations.changedPaths(valid.review_commit)[0] !== receiptPath(lane.laneId)) throw new Error('review commit mutates outside its canonical receipt artifact');
  let immutableReceipt;
  try { immutableReceipt = operations.readJson(valid.review_commit, receiptPath(lane.laneId)); } catch { throw new Error('review receipt artifact is absent or malformed'); }
  if (!immutableReceipt || receiptContentDigest(immutableReceipt) !== receiptContentDigest(receipt)) throw new Error('review receipt artifact is absent or differs from the admitted receipt');
  if (leafResultDigest(operations.leafDiff(anchor, valid.leaf_commit)) !== valid.leaf_result_sha256) throw new Error('review receipt leaf result digest does not match immutable Git data');
  const identity = operations.commitIdentity(valid.review_commit);
  if (!identity || identity.author_name !== trusted.author_name || identity.author_email !== trusted.author_email || identity.committer_name !== trusted.committer_name || identity.committer_email !== trusted.committer_email) throw new Error('review commit identity is not trusted at epoch base');
  let signatureStatus;
  try { signatureStatus = operations.verifySignature(valid.review_commit); } catch { throw new Error('review commit signature verification is unavailable'); }
  if (!signatureMatchesAuthority(signatureStatus, signingAuthority(trusted))) throw new Error('review commit signature is not verified by trusted epoch-base authority');
  return valid;
}
function validateInputs(registry, options) {
  if (!registry || !['console-capability-registry-v1', 'console-capability-registry-v2'].includes(registry.schema_version)) throw new Error('unsupported console capability registry schema');
  if (!FULL_SHA.test(options.anchorSha ?? '')) throw new Error('anchor SHA must be a full lowercase 40-character Git SHA');
  if (!Number.isInteger(options.maxWriters) || options.maxWriters < 1) throw new Error('max writers must be a positive integer');
  if (!Number.isFinite(options.qualityBias) || options.qualityBias < 0 || options.qualityBias > 1) throw new Error('quality bias must be between 0 and 1');
  if (!Array.isArray(registry.capabilities)) throw new Error('registry capabilities must be an array');
  validateReviewAuthority(registry.review_authority);
}
function normalizeGeneratedFacePattern(value) {
  try { return normalizePattern(value); } catch (error) {
    // A generated-output glob such as backend/crates/**/BUCK cannot be
    // intersected precisely by the literal ownership algebra. Conservatively
    // widen only the literal prefix to a terminal subtree.
    if (typeof value === 'string' && value.match(/^[A-Za-z0-9._/-]+\/\*\*\/[A-Za-z0-9._-]+$/)) {
      return `${value.slice(0, value.indexOf('/**/'))}/**`;
    }
    throw error;
  }
}
function validateAuthority(registry, generatedFaces) {
  const paths = validatePatternList(registry.shared_collision_roots?.paths, 'shared authority root');
  const generatedPath = registry.shared_collision_roots?.generated_face_registry;
  if (typeof generatedPath !== 'string') throw new Error('missing generated-face authority path');
  normalizePattern(generatedPath);
  if (!generatedFaces || generatedFaces.schema_version !== 2 || !Array.isArray(generatedFaces.faces) || generatedFaces.faces.length === 0) throw new Error('missing or incompatible generated-face authority');
  const generated = generatedFaces.faces.flatMap((face) => arrays(face?.output_patterns).map(normalizeGeneratedFacePattern));
  if (generated.length === 0) throw new Error('generated-face authority has no output patterns');
  return { sharedPatterns: [...new Set([...paths, ...generated])].sort(compareText), migrationRoots: paths.filter((entry) => entry.includes('/migrations/**')) };
}

function addResources(entries) {
  return Object.fromEntries(RESOURCE_KEYS.map((key) => [key, entries.reduce((total, entry) => total + entry.resources[key], 0)]));
}

/**
 * Only independently validated receipts may create expensive verification work.
 * Entries share a canonical local Buck daemon only when they name the same exact
 * leaf SHA. A future consolidated-tip job needs separate runtime-validated
 * authority; receipt fields alone never alter this grouping key.
 */
function verifiedReviewEntry(capability, lane, receipt, qualityBias) {
  const resources = validResources(lane.resources);
  if (!resources || !receipt) return null;
  return {
    capability_id: capability.id,
    lane_id: lane.laneId,
    lane_kind: lane.kind,
    owner: lane.owner,
    resources,
    buck2_targets: [...new Set(lane.buckTargets)].sort(compareText),
    leaf_commands: [...new Set(lane.leafCommands)].sort(compareText),
    quality_utility: round(laneScore(capability, qualityBias)),
    receipt_leaf_commit: receipt.leaf_commit,
  };
}

export function groupValidatedVerificationEntries(entries) {
  const byLeafSha = new Map();
  for (const entry of entries) {
    const key = entry.receipt_leaf_commit;
    if (!FULL_SHA.test(key ?? '')) throw new Error('validated verification entry must name an exact leaf SHA');
    if (!byLeafSha.has(key)) byLeafSha.set(key, []);
    byLeafSha.get(key).push(entry);
  }
  return [...byLeafSha.entries()].map(([verificationSha, groupEntries]) => {
    const ordered = [...groupEntries].sort((left, right) => compareText(left.lane_id, right.lane_id));
    const resources = addResources(ordered);
    const qualityUtility = round(ordered.reduce((total, entry) => total + entry.quality_utility, 0));
    // The denominator is the aggregate resource claim plus one, so denser
    // verified work wins without hiding resource contention. Ties are resolved
    // below by total quality, exact SHA, then lane IDs.
    const verificationDensity = round(qualityUtility / (1 + RESOURCE_KEYS.reduce((total, key) => total + resources[key], 0)));
    return {
      capability_ids: [...new Set(ordered.map((entry) => entry.capability_id))].sort(compareText),
      lane_ids: ordered.map((entry) => entry.lane_id),
      verification_sha: verificationSha,
      cache_affinity: verificationSha,
      execution: 'canonical_shared_daemon_combined_targets',
      resources,
      buck2_targets: [...new Set(ordered.flatMap((entry) => entry.buck2_targets))].sort(compareText),
      leaf_commands: [...new Set(ordered.flatMap((entry) => entry.leaf_commands))].sort(compareText),
      quality_utility: qualityUtility,
      verification_density: verificationDensity,
    };
  }).sort((left, right) => right.verification_density - left.verification_density
    || right.quality_utility - left.quality_utility
    || compareText(left.verification_sha, right.verification_sha)
    || compareText(left.lane_ids.join(','), right.lane_ids.join(',')));
}

export function buildFanoutPlan(registry, options) {
  if (registry?.schema_version === 'console-capability-registry-v2' && !isValidatedConsoleTruthLedger(registry)) throw new Error('v2 fanout requires a validated truth-ledger attestation');
  validateInputs(registry, options);
  options = { ...options, registry };
  const budgets = resourceBudgets(registry);
  const normalizedLanes = normalizedEpochLanes(registry);
  const generatedFaces = options.generatedFaces;
  const { sharedPatterns, migrationRoots } = validateAuthority(registry, generatedFaces);
  if (!migrationRoots.includes('backend/crates/platform/db/migrations/**')) throw new Error('shared authority must declare backend/crates/platform/db/migrations/**');
  const ids = new Set();
  for (const capability of registry.capabilities) {
    if (typeof capability.id !== 'string' || capability.id === '' || ids.has(capability.id)) throw new Error(`invalid or duplicate capability id: ${capability.id}`);
    ids.add(capability.id);
    // Validate all roots before any completed-source shortcut.
    validatePatternList(capabilityRoots(capability), `capability ${capability.id}`);
    if (typeof capability.evidence_path !== 'string') throw new Error(`capability ${capability.id} missing evidence_path`);
    normalizePattern(capability.evidence_path);
  }

  const held = [], admitted = [], completed = [], completedLeafLanes = [], reviewReady = [], verifiedReviewLanes = [];
  const sharedByCapability = new Map(), laneIdsByCapability = new Map(), unassignedByCapability = new Map();
  for (const capability of [...registry.capabilities].sort((a, b) => compareText(a.id, b.id))) {
    const classes = classifyRoots(capabilityRoots(capability), sharedPatterns, migrationRoots);
    sharedByCapability.set(capability.id, [...classes.sharedRoots, ...classes.migrationRoots].sort(compareText));
    const lanes = sourceLaneDefinitions(capability);
    const protectedRoots = [...sharedPatterns, ...migrationRoots];
    const laneReasons = new Map(lanes.map((lane) => [lane.laneId, laneRootReasons(lane.roots, classes.privateRoots, protectedRoots)]));
    const assigned = lanes.flatMap((lane) => validatePatternList(lane.roots, `lane ${lane.laneId}`));
    const unassigned = classes.privateRoots.filter((root) => !assigned.some((entry) => patternFullyCovers(entry, root)));
    unassignedByCapability.set(capability.id, unassigned);
    if (sourceComplete(capability)) {
      if (unassigned.length) held.push({ capability_id: capability.id, lane_id: `${capability.id}#unowned-roots`, reasons: ['unassigned_private_ownership_roots'], unassigned_roots: unassigned });
      else if (lanes.some((lane) => laneReasons.get(lane.laneId).length)) held.push({ capability_id: capability.id, lane_id: `${capability.id}#invalid-lane-ownership`, reasons: [...new Set(lanes.flatMap((lane) => laneReasons.get(lane.laneId)))] });
      else if (normalizedLanes && lanes.some((lane) => !normalizedLanes.has(lane.laneId))) held.push({ capability_id: capability.id, lane_id: `${capability.id}#legacy-lane`, reasons: ['legacy_lane_not_normalized_for_epoch'] });
      else if (!lanes.every((lane) => runtimeReceipt(options, lane))) held.push({ capability_id: capability.id, lane_id: `${capability.id}#completed-review`, reasons: ['completed_source_missing_exact_leaf_review_receipts'] });
      else {
        completed.push(capability.id);
        for (const lane of lanes) {
          const entry = verifiedReviewEntry(capability, lane, runtimeReceipt(options, lane), options.qualityBias);
          if (entry) verifiedReviewLanes.push(entry);
          // A validated receipt, not a mutable static state label, is what
          // removes this leaf from future source/re-review selection.
          completedLeafLanes.push(lane.laneId);
        }
      }
      continue;
    }
    if (unassigned.length) held.push({ capability_id: capability.id, lane_id: `${capability.id}#unowned-roots`, reasons: ['unassigned_private_ownership_roots'], unassigned_roots: unassigned });
    const incomplete = [];
    for (const lane of lanes) {
      const trustedReceipt = runtimeReceipt(options, lane);
      if (trustedReceipt) {
        const receiptReasons = [...laneReasons.get(lane.laneId)];
        if (normalizedLanes && !normalizedLanes.has(lane.laneId)) receiptReasons.push('legacy_lane_not_normalized_for_epoch');
        const entry = verifiedReviewEntry(capability, lane, trustedReceipt, options.qualityBias);
        if (!entry) receiptReasons.push('invalid_lane_resource_requirements');
        if (receiptReasons.length) {
          held.push({ capability_id: capability.id, lane_id: lane.laneId, reasons: [...new Set(receiptReasons)] });
          continue;
        }
        verifiedReviewLanes.push(entry);
        // Validated admission is sufficient even while static product-state
        // fields lag the immutable review train.
        completedLeafLanes.push(lane.laneId);
        continue;
      }
      if (laneSourceComplete(capability, lane.kind)) { completedLeafLanes.push(lane.laneId); if (!trustedReceipt) reviewReady.push({ capability_id: capability.id, lane_id: lane.laneId, reason: 'source_lane_complete_exact_independent_review_required' }); continue; }
      incomplete.push(lane);
      const roots = classifyRoots(lane.roots, sharedPatterns, migrationRoots);
      const reasons = [];
      reasons.push(...laneReasons.get(lane.laneId));
      if (normalizedLanes && !normalizedLanes.has(lane.laneId)) reasons.push('legacy_lane_not_normalized_for_epoch');
      if (!lane.owner || lane.owner === 'unassigned') reasons.push('missing_assigned_owner');
      if (typeof lane.worktree !== 'string' || !path.isAbsolute(lane.worktree)) reasons.push('missing_isolated_worktree');
      if (typeof lane.branch !== 'string' || !lane.branch.trim()) reasons.push('missing_branch');
      if (!capability.signature_story?.id || !capability.signature_story?.outcome?.trim()) reasons.push('missing_signature_story');
      if (!lane.leafCommands.length) reasons.push('missing_leaf_gates');
      if (!roots.privateRoots.length) reasons.push('missing_private_ownership_roots');
      const resources = validResources(lane.resources);
      if (!resources) reasons.push('invalid_lane_resource_requirements');
      // Own-property only — eligibility maps are plain objects; optional-index false-resolves
      // Object.prototype names (constructor/toString/…) as truthy Functions (console-g14a).
      if (options.runtimeLaneEligibility && typeof options.runtimeLaneEligibility === 'object'
        && Object.hasOwn(options.runtimeLaneEligibility, lane.laneId)) {
        reasons.push(options.runtimeLaneEligibility[lane.laneId]);
      }
      if (lane.roots.some((root) => normalizePattern(root).startsWith('backend/')) && stateText(capability, 'backend') !== 'not_applicable' && !lane.buckTargets.length) reasons.push('missing_backend_buck_targets');
      if (reasons.length) { held.push({ capability_id: capability.id, lane_id: lane.laneId, reasons }); continue; }
      admitted.push({ capability_id: capability.id, lane_id: lane.laneId, lane_kind: lane.kind, owner: lane.owner, worktree: lane.worktree, branch: lane.branch, resources, buck_isolation_dir: buckIsolationDir(options.anchorSha, lane.laneId), signature_story_id: capability.signature_story.id, evidence_path: capability.evidence_path, private_roots: roots.privateRoots, protected_roots: protectedRoots, shared_roots: sharedByCapability.get(capability.id), excluded_shared_roots: roots.excludedSharedRoots, buck2_targets: [...new Set(lane.buckTargets)].sort(compareText), leaf_commands: [...new Set(lane.leafCommands)].sort(compareText), quality_utility: round(laneScore(capability, options.qualityBias)), dependencies: [...new Set(arrays(capability.dependencies))].sort(compareText) });
    }
    laneIdsByCapability.set(capability.id, incomplete.map((lane) => lane.laneId));
  }
  const duplicateFields = ['owner', 'worktree', 'branch'];
  for (const field of duplicateFields) {
    const index = new Map(); for (const lane of admitted) { if (!index.has(lane[field])) index.set(lane[field], []); index.get(lane[field]).push(lane.lane_id); }
    for (const [value, laneIds] of index) if (laneIds.length > 1) for (const laneId of laneIds) held.push({ capability_id: laneId.split('#')[0], lane_id: laneId, reasons: [`duplicate_${field}_within_epoch`], duplicate_value: value });
  }
  const invalidLaneIds = new Set(held.filter((entry) => entry.lane_id && !entry.lane_id.endsWith('#unowned-roots') && !entry.lane_id.endsWith('#completed-review')).map((entry) => entry.lane_id));
  const candidates = admitted.filter((lane) => !invalidLaneIds.has(lane.lane_id));
  const degree = new Map(candidates.map((lane) => [lane.lane_id, 0])); const privateConflicts = [];
  for (let i = 0; i < candidates.length; i += 1) for (let j = i + 1; j < candidates.length; j += 1) { const roots = conflictBetween(candidates[i], candidates[j]); if (roots) { privateConflicts.push({ lane_ids: [candidates[i].lane_id, candidates[j].lane_id], roots }); degree.set(candidates[i].lane_id, degree.get(candidates[i].lane_id) + 1); degree.set(candidates[j].lane_id, degree.get(candidates[j].lane_id) + 1); } }
  const ranked = candidates.map((lane) => ({ ...lane, selection_density: round(lane.quality_utility / (1 + degree.get(lane.lane_id))) })).sort((a, b) => b.selection_density - a.selection_density || b.quality_utility - a.quality_utility || compareText(a.lane_id, b.lane_id));
  const selected = [], collisionBlocked = [];
  for (const lane of ranked) {
    if (selected.length >= Math.min(options.maxWriters, budgets.writer)) { collisionBlocked.push({ capability_id: lane.capability_id, lane_id: lane.lane_id, reason: 'writer_budget_exhausted' }); continue; }
    const conflict = selected.find((chosen) => conflictBetween(lane, chosen)); if (conflict) { collisionBlocked.push({ capability_id: lane.capability_id, lane_id: lane.lane_id, reason: 'private_root_conflict', conflicts_with: conflict.lane_id }); continue; }
    selected.push(lane);
  }
  const verificationAllocated = Object.fromEntries(RESOURCE_KEYS.map((key) => [key, 0])); let coldRustCompileLanes = 0;
  const verificationQueue = groupValidatedVerificationEntries(verifiedReviewLanes).map((group) => {
    const exceeded = RESOURCE_KEYS.filter((key) => verificationAllocated[key] + group.resources[key] > budgets[key]);
    const coldBlocked = group.buck2_targets.length && coldRustCompileLanes >= COLD_RUST_COMPILE_LANES;
    if (exceeded.length || coldBlocked) return { ...group, scheduled: false, hold_reason: coldBlocked ? 'cold_rust_compile_capacity_exhausted' : 'verification_resource_capacity_exhausted', constrained_resources: exceeded };
    for (const key of RESOURCE_KEYS) verificationAllocated[key] += group.resources[key]; if (group.buck2_targets.length) coldRustCompileLanes += 1;
    return { ...group, scheduled: true };
  });
  const byId = new Map(registry.capabilities.map((capability) => [capability.id, capability]));
  const selectedCapabilities = [...new Set([...selected.map((lane) => lane.capability_id), ...verifiedReviewLanes.map((lane) => lane.capability_id)])].sort(compareText);
  const reviewedSourceCapability = (capability) => sourceLaneDefinitions(capability).every((lane) => runtimeReceipt(options, lane));
  const mergeDependencyHolds = selectedCapabilities.map((id) => {
    const capability = byId.get(id);
    return {
      capability_id: id,
      unresolved_dependencies: arrays(capability?.dependencies).filter((dependency) => {
        const dependencyCapability = byId.get(dependency);
        return !dependencyCapability || (!sourceComplete(dependencyCapability) && !reviewedSourceCapability(dependencyCapability));
      }),
    };
  }).filter((entry) => entry.unresolved_dependencies.length).sort((a,b) => compareText(a.capability_id,b.capability_id));
  const consolidationQueue = selectedCapabilities.map((id) => {
    const capability = byId.get(id); const consolidation = capability.lane_assignments?.consolidation; const selectedIds = new Set([...selected.filter((lane) => lane.capability_id === id).map((lane) => lane.lane_id), ...verifiedReviewLanes.filter((lane) => lane.capability_id === id).map((lane) => lane.lane_id)]); const awaiting = (laneIdsByCapability.get(id) ?? []).filter((laneId) => !selectedIds.has(laneId)).sort(compareText); const lanes = sourceLaneDefinitions(capability); const hasReviews = lanes.every((lane) => runtimeReceipt(options, lane)); const consolidationResources = validResources(consolidation?.resources ?? capability.consolidation_resources);
    const prerequisites = [];
    if (!hasReviews) prerequisites.push('exact_leaf_review_receipts_required');
    if (!consolidation?.owner || consolidation.owner !== registry.shared_collision_roots.owner) prerequisites.push('invalid_consolidation_owner');
    if (typeof consolidation?.worktree !== 'string' || !path.isAbsolute(consolidation.worktree)) prerequisites.push('invalid_consolidation_worktree');
    if (typeof consolidation?.branch !== 'string' || !consolidation.branch.trim()) prerequisites.push('invalid_consolidation_branch');
    if (!consolidationResources) prerequisites.push('invalid_consolidation_resource_requirements');
    else if (RESOURCE_KEYS.some((key) => consolidationResources[key] > budgets[key])) prerequisites.push('consolidation_resources_exceed_epoch_capacity');
    if (options.runtimeConsolidationEligibility && typeof options.runtimeConsolidationEligibility === 'object'
      && Object.hasOwn(options.runtimeConsolidationEligibility, capability.id)) {
      prerequisites.push(options.runtimeConsolidationEligibility[capability.id]);
    }
    return { capability_id: id, shared_roots: sharedByCapability.get(id) ?? [], ready_after_leaf_review: prerequisites.length === 0 && awaiting.length === 0 && !(unassignedByCapability.get(id) ?? []).length, review_prerequisites: prerequisites, awaiting_lane_ids: awaiting, awaiting_unassigned_roots: unassignedByCapability.get(id) ?? [] };
  }).filter((entry) => entry.shared_roots.length).sort((a,b) => compareText(a.capability_id,b.capability_id));
  const reviewQueue = [...selected.map((lane) => ({ capability_id: lane.capability_id, lane_id: lane.lane_id, reason: 'leaf_result_requires_exact_independent_review_receipt' })), ...reviewReady].sort((a,b) => compareText(a.capability_id,b.capability_id) || compareText(a.reason,b.reason));
  return { schema_version: 'console-fanout-epoch-v2', anchor_sha: options.anchorSha, authority: { registry_schema: registry.schema_version, registry_digest_sha256: registryDigest(registry), registry_source_revision: registry.source_revision ?? null, generated_face_registry_schema: generatedFaces.schema_version, generated_face_registry_digest_sha256: digest(generatedFaces) }, policy: { max_writer_lanes: Math.min(options.maxWriters, budgets.writer), resource_budgets: budgets, verification_allocated_resources: verificationAllocated, cold_rust_compile_lanes: COLD_RUST_COMPILE_LANES, cold_rust_compile_jobs: COLD_RUST_COMPILE_JOBS, buck_isolation_policy: 'canonical_local_daemon_for_compatible_exact_sha', quality_bias: options.qualityBias, shared_owner: registry.shared_collision_roots?.owner ?? null, dependency_behavior: 'source_parallel_merge_fail_closed', generated_face_behavior: 'single_writer_after_exact_review', selection_algorithm: 'deterministic_quality_weighted_maximal_independent_set' }, selected, verification_queue: verificationQueue, held, collision_blocked: collisionBlocked, completed_source_capabilities: completed, completed_leaf_lanes: completedLeafLanes.sort(compareText), private_conflicts: privateConflicts, merge_dependency_holds: mergeDependencyHolds, review_queue: reviewQueue, consolidation_queue: consolidationQueue };
}

const ADMISSION_PATH = 'docs/evidence/console/fanout-admission.json';
function usage() { return ['usage: node scripts/console/plan-fanout.mjs --candidate <40-char-sha> --authority-tip <40-char-sha> --synthetic-merge <40-char-sha> [--admission <40-char-sha>] [options]', '', 'options:', '  --registry <path>       capability registry authority path', '  --candidate <sha>       immutable product candidate C', '  --authority-tip <sha>   signed authority/control tip T', '  --synthetic-merge <sha> unsigned GitHub merge M (structural-only)', '  --admission <sha>       immutable receipt manifest commit (default: candidate)', '  --max-writers <count>   bounded source writers (default: 6)', `  --quality-bias <0..1>   quality weighting (default: ${QUALITY_BIAS_DEFAULT})`].join('\n'); }
function parseArgs(argv) { const result = { registryPath: 'docs/program/console-capability-registry.json', candidateSha: process.env.CONSOLE_CANDIDATE_SHA ?? null, admissionSha: null, authorityTipSha: process.env.CONSOLE_AUTHORITY_TIP_SHA ?? null, syntheticMergeSha: process.env.CONSOLE_SYNTHETIC_MERGE_SHA ?? null, maxWriters: 6, qualityBias: QUALITY_BIAS_DEFAULT }; for (let i = 0; i < argv.length; i += 1) { const arg = argv[i]; if (arg === '--help' || arg === '-h') { process.stdout.write(`${usage()}\n`); process.exit(0); } const value = argv[i + 1]; if (value === undefined) throw new Error(`missing value for ${arg}`); if (arg === '--registry') result.registryPath = value; else if (arg === '--candidate') result.candidateSha = value; else if (arg === '--authority-tip') result.authorityTipSha = value; else if (arg === '--synthetic-merge') result.syntheticMergeSha = value; else if (arg === '--admission') result.admissionSha = value; else if (arg === '--max-writers') result.maxWriters = Number(value); else if (arg === '--quality-bias') result.qualityBias = Number(value); else throw new Error(`unknown argument: ${arg}`); i += 1; } result.admissionSha ??= result.candidateSha; return result; }
function git(repoRoot, args) { return execFileSync('git', ['-C', repoRoot, ...args], { encoding: 'utf8' }).trim(); }
export { parseImmutableJson } from './immutable-json.mjs';
import { parseImmutableJson as parseImmutableJsonInternal } from './immutable-json.mjs';
function readAnchorJson(repoRoot, anchor, relativePath) { const raw = execFileSync('git', ['-C', repoRoot, 'show', `${anchor}:${relativePath}`], { encoding: 'utf8' }); return parseImmutableJsonInternal(raw, `${anchor}:${relativePath}`).value; }
export function parseSourceRevision(value) {
  if (typeof value !== 'string' || value.trim() !== value) throw new Error('source_revision must be canonical <ref>@<40sha>');
  const match = value.match(/^(.+?)@([0-9a-f]{40})$/);
  if (!match || !match[1] || /\s/.test(match[1])) throw new Error('source_revision must be canonical <ref>@<40sha>');
  return { ref: match[1], sha: match[2] };
}
function gitSucceeds(repoRoot, args) { try { git(repoRoot, args); return true; } catch { return false; } }
export function validateSourceRevisionForAnchor(value, anchor, operations) {
  const { ref, sha } = parseSourceRevision(value);
  if (!FULL_SHA.test(anchor ?? '')) throw new Error('anchor must be a full immutable SHA');
  if (!operations.hasCommit(sha)) throw new Error('source_revision SHA does not exist');
  if (!operations.isAncestor(sha, anchor)) throw new Error('source_revision SHA is not an ancestor of anchor');
  const refSha = operations.resolveRef(ref);
  if (refSha && !operations.isAncestor(sha, refSha)) throw new Error('source_revision ref is behind or conflicts with recorded SHA');
  return { ref, sha };
}
function assertSourceRevision(repoRoot, value, anchor) {
  return validateSourceRevisionForAnchor(value, anchor, {
    hasCommit: (sha) => gitSucceeds(repoRoot, ['cat-file', '-e', `${sha}^{commit}`]),
    isAncestor: (ancestor, descendant) => gitSucceeds(repoRoot, ['merge-base', '--is-ancestor', ancestor, descendant]),
    resolveRef: (ref) => gitSucceeds(repoRoot, ['rev-parse', '--verify', '--quiet', `${ref}^{commit}`]) ? git(repoRoot, ['rev-parse', '--verify', `${ref}^{commit}`]) : null,
  });
}
function worktreeEntries(repoRoot) { const output = git(repoRoot, ['worktree', 'list', '--porcelain']); const entries = []; let current = {}; for (const line of output.split('\n')) { if (!line) { if (current.worktree) entries.push(current); current = {}; } else { const [key, ...rest] = line.split(' '); current[key] = rest.join(' '); } } if (current.worktree) entries.push(current); return entries; }
function assertAnchorAuthority(repoRoot, anchor, registryPath, registry, generatedPath, generated) { if (git(repoRoot, ['rev-parse', '--verify', anchor]) !== anchor) throw new Error('anchor is not a resolvable immutable commit'); if (git(repoRoot, ['status', '--porcelain']) !== '') throw new Error('planner requires a clean worktree'); assertSourceRevision(repoRoot, registry.source_revision, anchor); const anchoredRegistry = readAnchorJson(repoRoot, anchor, registryPath); if (registryDigest(anchoredRegistry) !== registryDigest(registry)) throw new Error('registry differs from anchor blob'); const anchoredGenerated = readAnchorJson(repoRoot, anchor, generatedPath); if (digest(anchoredGenerated) !== digest(generated)) throw new Error('generated-face authority differs from anchor blob'); try { git(repoRoot, ['cat-file', '-e', `${anchor}:backend/crates/platform/db/migrations`]); } catch { throw new Error('migration authority root missing at anchor'); } }
function declaredWorktreeReason(repoRoot, byPath, declaration, anchor) {
  const entry = byPath.get(declaration.worktree);
  if (!entry || entry.branch !== `refs/heads/${declaration.branch}`) return 'declared_worktree_or_branch_not_live';
  if (git(declaration.worktree, ['status', '--porcelain']) !== '') return 'declared_worktree_dirty';
  const head = git(declaration.worktree, ['rev-parse', 'HEAD']);
  return head === anchor || gitSucceeds(repoRoot, ['merge-base', '--is-ancestor', anchor, head]) ? null : 'declared_worktree_head_not_anchor_descendant';
}
export function validateAdmissionManifest(manifest, epochBaseSha, admissionSha, operations) {
  if (manifest?.schema_version !== 'console-fanout-admission-v1' || manifest.epoch_base_sha !== epochBaseSha || !Array.isArray(manifest.receipts)) throw new Error('invalid immutable fanout admission manifest');
  if (operations.parentCount(admissionSha) !== 1) throw new Error('admission commit must be single-parent to avoid merge ambiguity');
  const changedPaths = operations.changedPaths(admissionSha);
  if (changedPaths.length !== 1 || changedPaths[0] !== ADMISSION_PATH) throw new Error('admission commit must be manifest-only');
  const lanes = new Set(), reviews = new Set(), paths = new Set();
  for (const entry of manifest.receipts) {
    if (!entry || typeof entry !== 'object' || typeof entry.lane_id !== 'string' || !FULL_SHA.test(entry.review_commit ?? '') || entry.receipt_path !== receiptPath(entry.lane_id)) throw new Error('invalid fanout admission receipt reference');
    if (lanes.has(entry.lane_id) || reviews.has(entry.review_commit) || paths.has(entry.receipt_path)) throw new Error('duplicate fanout admission receipt reference');
    if (!operations.isAncestor(entry.review_commit, admissionSha)) throw new Error('admission review commit is not an ancestor of admission');
    let receipt;
    try { receipt = operations.readJson(entry.review_commit, entry.receipt_path); } catch { throw new Error('admission receipt reference does not bind its review artifact'); }
    if (receipt?.lane_id !== entry.lane_id) throw new Error('admission receipt reference does not bind its review artifact');
    lanes.add(entry.lane_id); reviews.add(entry.review_commit); paths.add(entry.receipt_path);
  }
  return manifest.receipts;
}
export function validateEpochAdmissionClosure(epochBaseSha, admissionSha, receipts, operations) {
  const commits = operations.commitsBetween(epochBaseSha, admissionSha);
  if (!Array.isArray(commits) || commits.at(-1) !== admissionSha) throw new Error('admission closure is not an ordered epoch-to-admission train');
  const leafs = new Set(receipts.map((receipt) => receipt.leaf_commit));
  const reviews = new Set(receipts.map((receipt) => receipt.review_commit));
  let previous = epochBaseSha;
  for (const commit of commits) {
    if (commit === admissionSha) {
      if (operations.parentOf(commit) !== previous) throw new Error('admission commit does not close the serialized admission train');
      continue;
    }
    if (leafs.has(commit)) {
      if (operations.parentOf(commit) !== previous) throw new Error('leaf commit is not on the serialized admission train');
    } else if (reviews.has(commit)) {
      const receipt = receipts.find((entry) => entry.review_commit === commit);
      if (!receipt || operations.parentOf(commit) !== receipt.leaf_commit || receipt.leaf_commit !== previous) throw new Error('review commit is not the direct authorized receipt for the preceding leaf');
    } else {
      throw new Error('admission closure contains an unreviewed or unrelated commit');
    }
    previous = commit;
  }
}
function admissionReceipts(repoRoot, epochBaseSha, admissionSha) {
  if (admissionSha === epochBaseSha) return {};
  if (!gitSucceeds(repoRoot, ['merge-base', '--is-ancestor', epochBaseSha, admissionSha])) throw new Error('admission SHA must descend from epoch base SHA');
  const manifest = readAnchorJson(repoRoot, admissionSha, ADMISSION_PATH);
  validateAdmissionManifest(manifest, epochBaseSha, admissionSha, {
    parentCount: (sha) => git(repoRoot, ['rev-list', '--parents', '-n', '1', sha]).split(' ').length - 1,
    changedPaths: (sha) => git(repoRoot, ['diff-tree', '--no-commit-id', '--name-only', '-r', sha]).split('\n').filter(Boolean),
    isAncestor: (ancestor, descendant) => gitSucceeds(repoRoot, ['merge-base', '--is-ancestor', ancestor, descendant]),
    readJson: (sha, filePath) => readAnchorJson(repoRoot, sha, filePath),
  });
  validateEpochAdmissionClosure(epochBaseSha, admissionSha, manifest.receipts.map((entry) => ({ ...readAnchorJson(repoRoot, entry.review_commit, entry.receipt_path), review_commit: entry.review_commit })), {
    commitsBetween: (base, tip) => git(repoRoot, ['rev-list', '--reverse', `${base}..${tip}`]).split('\n').filter(Boolean),
    parentOf: (sha) => git(repoRoot, ['rev-parse', `${sha}^`]),
  });
  const result = {};
  for (const entry of manifest.receipts) result[entry.lane_id] = { ...readAnchorJson(repoRoot, entry.review_commit, entry.receipt_path), review_commit: entry.review_commit };
  return result;
}
function verifyGitCommitSignature(repoRoot, candidateSha, sha, authority) {
  if (authority?.format === 'ssh') return verifyCommitWithCandidateSshPolicy(repoRoot, candidateSha, sha, authority);
  const result = spawnSync('git', ['-C', repoRoot, 'verify-commit', '--raw', sha], { encoding: 'utf8' });
  if (result.error) throw new Error(`git verify-commit unavailable: ${result.error.message}`);
  if (result.status !== 0 || typeof result.stdout !== 'string' || typeof result.stderr !== 'string') throw new Error('git verify-commit rejected the review commit');
  return `${result.stdout}${result.stderr}`;
}
function runtimeEligibility(repoRoot, registry, anchor, receipts) {
  const entries = worktreeEntries(repoRoot); const byPath = new Map(entries.map((entry) => [entry.worktree, entry])); const validatedReceipts = {};
  const runtimeLaneEligibility = {}, runtimeConsolidationEligibility = {}, runtimeReviewEligibility = {};
  const generated = readAnchorJson(repoRoot, anchor, registry.shared_collision_roots.generated_face_registry);
  const { sharedPatterns, migrationRoots } = validateAuthority(registry, generated);
  for (const capability of registry.capabilities) {
    for (const lane of sourceLaneDefinitions(capability)) {
      const reason = declaredWorktreeReason(repoRoot, byPath, lane, anchor);
      if (reason) runtimeLaneEligibility[lane.laneId] = reason;
      const receipt = receipts[lane.laneId];
      if (receipt) {
        try {
          const roots = classifyRoots(lane.roots, sharedPatterns, migrationRoots);
          validateReviewReceiptForAnchor(receipt, anchor, { ...lane, privateRoots: roots.privateRoots, protectedRoots: [...sharedPatterns, ...migrationRoots] }, registry.review_authority, {
            hasCommit: (sha) => gitSucceeds(repoRoot, ['cat-file', '-e', `${sha}^{commit}`]),
            isAncestor: (ancestor, descendant) => gitSucceeds(repoRoot, ['merge-base', '--is-ancestor', ancestor, descendant]),
            parentOf: (sha) => git(repoRoot, ['rev-parse', `${sha}^`]),
            parentCount: (sha) => git(repoRoot, ['rev-list', '--parents', '-n', '1', sha]).split(' ').length - 1,
            changedPaths: (sha) => git(repoRoot, ['diff-tree', '--no-commit-id', '--name-only', '-r', sha]).split('\n').filter(Boolean),
            readJson: (sha, filePath) => readAnchorJson(repoRoot, sha, filePath),
            leafDiff: (base, leaf) => execFileSync('git', ['-C', repoRoot, 'diff', '--no-ext-diff', '--no-renames', '--full-index', '--binary', base, leaf]),
            commitIdentity: (sha) => { const [author_name, author_email, committer_name, committer_email] = git(repoRoot, ['show', '-s', '--format=%an%x00%ae%x00%cn%x00%ce', sha]).split('\0'); return { author_name, author_email, committer_name, committer_email }; },
            verifySignature: (sha) => verifyGitCommitSignature(repoRoot, anchor, sha, signingAuthority(trustedReviewer(registry.review_authority, receipt.reviewer))),
          });
          validatedReceipts[lane.laneId] = receipt;
        } catch {
          runtimeReviewEligibility[lane.laneId] = 'invalid_exact_leaf_review_receipt';
        }
      }
    }
    const consolidation = capability.lane_assignments?.consolidation;
    if (consolidation) {
      const reason = declaredWorktreeReason(repoRoot, byPath, consolidation, anchor);
      if (reason) runtimeConsolidationEligibility[capability.id] = reason;
    }
  }
  const validatedAttestations = { receipts: validatedReceipts };
  validatedAttestationTokens.add(validatedAttestations);
  return { runtimeLaneEligibility, runtimeConsolidationEligibility, runtimeReviewEligibility, validatedAttestations };
}
function main() { const args = parseArgs(process.argv.slice(2)); const repoRoot = process.cwd(); if (!FULL_SHA.test(args.candidateSha ?? '')) throw new Error('planner requires --candidate <40-char-sha> or CONSOLE_CANDIDATE_SHA'); const candidateRegistry = readAnchorJson(repoRoot, args.candidateSha, args.registryPath); if (candidateRegistry.schema_version !== 'console-capability-registry-v2') throw new Error('planner requires console-capability-registry-v2'); if (!FULL_SHA.test(args.authorityTipSha ?? '')) throw new Error('planner requires --authority-tip <40-char-sha> or CONSOLE_AUTHORITY_TIP_SHA'); if (!FULL_SHA.test(args.syntheticMergeSha ?? '')) throw new Error('planner requires --synthetic-merge <40-char-sha> or CONSOLE_SYNTHETIC_MERGE_SHA'); const train = verifyConsoleAuthorityTrain(repoRoot, args.candidateSha, args.authorityTipSha, args.syntheticMergeSha); if (train.trainClass === RELEASE_PLEASE_TRAIN_CLASS) { process.stdout.write(`${JSON.stringify({ verdict: 'RELEASE_PLEASE_BOT_CANDIDATE_ADMITTED', train_class: RELEASE_PLEASE_TRAIN_CLASS, candidate_sha: train.candidateSha, authority_tip_sha: train.authorityTipSha, synthetic_merge_sha: train.syntheticMergeSha }, null, 2)}\n`); return; } const registry = readAnchorJson(repoRoot, args.authorityTipSha, args.registryPath); const jurisdiction = readAnchorJson(repoRoot, args.authorityTipSha, 'docs/program/console-jurisdiction-register.json'); const candidateSource = createConsoleCandidateSourceResolver(repoRoot, args.candidateSha, args.authorityTipSha); validateConsoleTruthLedger(registry, jurisdiction, { expectedCandidateSha: args.candidateSha, resolveSha: (sha) => gitSucceeds(repoRoot, ['cat-file', '-e', `${sha}^{commit}`]), resolveRef: (ref, sha) => gitSucceeds(repoRoot, ['rev-parse', '--verify', `${ref}^{commit}`]) && git(repoRoot, ['rev-parse', '--verify', `${ref}^{commit}`]) === sha, resolveBuckTarget: createConsoleBuckTargetResolver(candidateSource), resolveSource: candidateSource.resolveSource, routeFacts: extractConsoleRouteFactsFromCandidate(candidateSource), repoRoot }); const generatedPath = registry.shared_collision_roots?.generated_face_registry; if (typeof generatedPath !== 'string') throw new Error('missing generated-face authority path'); const generated = readAnchorJson(repoRoot, args.candidateSha, generatedPath); if (git(repoRoot, ['status', '--porcelain']) !== '') throw new Error('planner requires a clean worktree'); assertSourceRevision(repoRoot, registry.source_revision, args.candidateSha); const receipts = admissionReceipts(repoRoot, args.candidateSha, args.admissionSha); const runtime = runtimeEligibility(repoRoot, registry, args.candidateSha, receipts); const plan = buildFanoutPlan(registry, { anchorSha: args.candidateSha, admissionSha: args.admissionSha, maxWriters: args.maxWriters, qualityBias: args.qualityBias, generatedFaces: generated, ...runtime }); process.stdout.write(`${JSON.stringify({ ...plan, epoch_base_sha: args.candidateSha, admission_sha: args.admissionSha }, null, 2)}\n`); }
const invokedPath = process.argv[1] ? pathToFileURL(path.resolve(process.argv[1])).href : null;
if (invokedPath === import.meta.url) { try { main(); } catch (error) { process.stderr.write(`console-fanout-plan: ${error instanceof Error ? error.message : String(error)}\n`); process.exitCode = 1; } }
