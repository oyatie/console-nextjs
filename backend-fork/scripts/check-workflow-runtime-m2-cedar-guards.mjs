#!/usr/bin/env node
// M2 Cedar-guard observe-and-record gate (AC3).
//
// Proves that the M2 workflow-runtime Cedar guards at node transitions and
// waiting-task completion are STRICTLY observe-and-record: pinned to
// DualEngineMode::LegacyOnly with the legacy engine as the SOLE enforcer, an
// inert Cedar that can NEVER deny (nor grant), and a shadow decision written via
// observe_cedar_pbac_decision as exactly ONE audit_events row inside the SAME
// with_audits transaction as the guarded state change.
//
// The load-bearing proofs are unconditional (the Cedar/PBAC boundary contract in
// cedar_pbac.rs, the same-txn audit contract in audit_tx.rs, and a backing Rust
// proof test). The guard-adapter wiring is additionally hardened once the sibling
// runtime adapter lands on the branch — the M2 crates land together. If any
// invariant regresses, this gate fails closed so a guard can never start
// enforcing (or hiding) the inert Cedar verdict at merge.
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const CEDAR_PBAC = "backend/crates/platform/authz/src/cedar_pbac.rs";
const AUDIT_TX = "backend/crates/platform/db/src/audit_tx.rs";
const PROOF_TEST =
  "backend/crates/platform/authz/tests/cedar_pbac_legacy_only_observe_and_record.rs";
const CRATES_DIR = "backend/crates";

const failures = [];
const passes = [];
const notes = [];

function abs(path) {
  return resolve(root, path);
}

function read(path) {
  const full = abs(path);
  if (!existsSync(full)) {
    failures.push(`missing required file: ${path}`);
    return "";
  }
  return readFileSync(full, "utf8");
}

function pass(label) {
  passes.push(label);
}

function assert(condition, ok, failure) {
  if (condition) {
    pass(ok);
  } else {
    failures.push(failure);
  }
}

function requireMatches(path, text, pattern, label) {
  assert(pattern.test(text), label, `${path}: must match ${pattern} (${label})`);
}

function requireIncludes(path, needle, label) {
  const text = read(path);
  assert(
    text.includes(needle),
    label,
    `${path}: must include ${JSON.stringify(needle)} (${label})`,
  );
}

// Extract a Rust fn body by brace-balancing from the first `{` after the header
// match. Returns "" if the header is absent. Structural, comment-tolerant.
function fnBody(source, headerPattern) {
  const m = source.match(headerPattern);
  if (!m) return "";
  let i = source.indexOf("{", m.index);
  if (i < 0) return "";
  let depth = 0;
  const start = i;
  for (; i < source.length; i += 1) {
    const c = source[i];
    if (c === "{") depth += 1;
    else if (c === "}") {
      depth -= 1;
      if (depth === 0) return source.slice(start, i + 1);
    }
  }
  return "";
}

// --- (1) LegacyOnly is the SOLE enforcer: the boundary delegates to legacy. ---
const cedar = read(CEDAR_PBAC);

requireMatches(
  CEDAR_PBAC,
  cedar,
  /DualEngineMode::LegacyOnly\s*=>\s*evaluate_legacy_contract\(\s*request\s*\)/,
  "under LegacyOnly the boundary returns evaluate_legacy_contract(request) — legacy is the sole enforcer",
);

// evaluate_legacy_contract takes ONLY the request; it structurally cannot
// consult a Cedar verdict, so an inert Cedar can never influence enforcement.
requireMatches(
  CEDAR_PBAC,
  cedar,
  /fn\s+evaluate_legacy_contract\(\s*request:\s*&AuthorizationRequest\s*\)\s*->\s*AuthorizationDecision/,
  "evaluate_legacy_contract takes no CedarEvaluation — legacy enforcement cannot read Cedar",
);

// LegacyOnly is NOT in the cedar_required set: no bundle/subject-freshness Cedar
// preconditions gate the LegacyOnly path, and Cedar is never evaluated.
const cedarRequired = fnBody(cedar, /const fn cedar_required\(mode: DualEngineMode\) -> bool/);
assert(
  cedarRequired.length > 0,
  "cedar_required(mode) is defined",
  `${CEDAR_PBAC}: cedar_required(mode) must be defined`,
);
assert(
  cedarRequired.includes("CedarShadowLegacyEnforce") &&
    cedarRequired.includes("CedarEnforceLegacyCompare") &&
    cedarRequired.includes("CedarOnly"),
  "cedar_required enumerates the three Cedar-bearing modes",
  `${CEDAR_PBAC}: cedar_required must name the Cedar-bearing modes`,
);
assert(
  !/\bLegacyOnly\b/.test(cedarRequired),
  "cedar_required excludes LegacyOnly — Cedar is never required (or evaluated) under the M2 mode",
  `${CEDAR_PBAC}: cedar_required must NOT include LegacyOnly (Cedar must stay inert under LegacyOnly)`,
);

// evaluate_legacy_contract stamps the enforced decision engine=Legacy,
// mode=LegacyOnly for both the allow and deny arms — legacy is the recorded
// enforcer, not Cedar.
const legacyContract = fnBody(
  cedar,
  /fn\s+evaluate_legacy_contract\(\s*request:\s*&AuthorizationRequest\s*\)\s*->\s*AuthorizationDecision/,
);
assert(
  /DecisionEngine::Legacy/.test(legacyContract) &&
    /DecisionReason::LegacyAllowed/.test(legacyContract) &&
    /DecisionReason::LegacyDenied/.test(legacyContract) &&
    /Some\(DualEngineMode::LegacyOnly\)/.test(legacyContract),
  "legacy decisions are stamped engine=Legacy, mode=LegacyOnly (legacy is the recorded enforcer)",
  `${CEDAR_PBAC}: evaluate_legacy_contract must stamp DecisionEngine::Legacy + DualEngineMode::LegacyOnly`,
);

// --- (2) Observation RECORDS, never mutates: fed the decision, returns 1 event. ---
requireMatches(
  CEDAR_PBAC,
  cedar,
  /#\[must_use\]\s*pub fn observe_cedar_pbac_decision\(/,
  "observe_cedar_pbac_decision is #[must_use] — the shadow observation cannot be silently dropped",
);

const observe = fnBody(cedar, /pub fn observe_cedar_pbac_decision\(/);
assert(observe.length > 0, "observe_cedar_pbac_decision is defined", `${CEDAR_PBAC}: missing observe_cedar_pbac_decision`);
assert(
  /decision:\s*AuthorizationDecision/.test(cedar.slice(cedar.indexOf("observe_cedar_pbac_decision"))),
  "observe_cedar_pbac_decision is FED the already-computed decision — observation cannot change enforcement",
  `${CEDAR_PBAC}: observe_cedar_pbac_decision must accept the enforced decision: AuthorizationDecision`,
);
assert(
  /->\s*AuthorizationAuditEvent/.test(cedar.slice(cedar.indexOf("observe_cedar_pbac_decision"))),
  "observe_cedar_pbac_decision returns exactly ONE AuthorizationAuditEvent per decision",
  `${CEDAR_PBAC}: observe_cedar_pbac_decision must return a single AuthorizationAuditEvent`,
);
assert(
  /AuthorizationAuditEvent\s*\{\s*decision,/.test(observe),
  "the shadow audit event carries the enforced decision verbatim (recorded, not recomputed)",
  `${CEDAR_PBAC}: observe_cedar_pbac_decision must place the enforced decision on the audit event`,
);
// The would-be Cedar verdict is preserved for the forensic trail without gaining
// enforcement weight.
assert(
  /evaluated_bundle_key:\s*cedar\.and_then/.test(observe) &&
    /evaluated_reason_detail:\s*cedar\.and_then/.test(observe),
  "the shadow Cedar verdict (bundle + reason) is recorded on the audit event with zero enforcement weight",
  `${CEDAR_PBAC}: observe_cedar_pbac_decision must record evaluated_bundle_key/evaluated_reason_detail from the inert Cedar result`,
);

// AuthorizationDecision is byte-comparable so parity ("byte-identical to legacy")
// is a real equality, not a fuzzy match.
requireMatches(
  CEDAR_PBAC,
  cedar,
  /#\[derive\([^\)]*PartialEq[^\)]*\)\]\s*(?:#\[[^\]]*\]\s*)*pub struct AuthorizationDecision\b/,
  "AuthorizationDecision derives PartialEq — LegacyOnly parity is an exact equality",
);

// --- (3) ONE audit_events row inside the SAME with_audits transaction. ---
const auditTx = read(AUDIT_TX);
requireMatches(
  AUDIT_TX,
  auditTx,
  /pub async fn with_audits<F, T, E>\(pool: &PgPool, org: OrgId, f: F\)/,
  "with_audits(pool, org, closure) is the same-transaction audited-write helper the guard uses",
);
const withAudits = fnBody(auditTx, /pub async fn with_audits<F, T, E>/);
assert(withAudits.length > 0, "with_audits body is present", `${AUDIT_TX}: missing with_audits body`);
assert(
  /set_current_org\(&mut tx, org\)/.test(withAudits),
  "with_audits arms app.current_org on the same tx BEFORE the closure (RLS-scoped console_rt write)",
  `${AUDIT_TX}: with_audits must arm app.current_org on the transaction`,
);
assert(
  /insert_audit_event_tx\(&mut tx, event\)/.test(withAudits) && /tx\.commit\(\)/.test(withAudits),
  "with_audits inserts the audit event(s) and commits in the SAME transaction (atomic with the state change)",
  `${AUDIT_TX}: with_audits must insert audit events and commit in the same transaction`,
);
assert(
  /tx\.rollback\(\)/.test(withAudits),
  "with_audits rolls back on error so neither the state change nor the shadow audit row lands",
  `${AUDIT_TX}: with_audits must roll back on the closure error path`,
);

// --- (4) The backing Rust proof test asserts the runtime invariants. ---
const proof = read(PROOF_TEST);
requireMatches(
  PROOF_TEST,
  proof,
  /DualEngineMode::LegacyOnly/,
  "the proof pins DualEngineMode::LegacyOnly",
);
requireMatches(
  PROOF_TEST,
  proof,
  /assert_eq!\(\s*enforced,\s*legacy/,
  "the proof asserts the enforced decision is byte-identical to the legacy decision (legacy sole enforcer)",
);
// Inert Cedar can never deny (nor grant): the proof sweeps every CedarEvaluation
// shape a guard could feed the boundary.
for (const variant of ["NotConfigured", "Allow", "Deny", "Error"]) {
  assert(
    new RegExp(`CedarEvaluation::${variant}`).test(proof),
    `the proof exercises the inert Cedar shape CedarEvaluation::${variant}`,
    `${PROOF_TEST}: must exercise CedarEvaluation::${variant} to prove inert Cedar can never flip enforcement`,
  );
}
requireMatches(
  PROOF_TEST,
  proof,
  /observe_cedar_pbac_decision/,
  "the proof records the shadow decision via observe_cedar_pbac_decision",
);
requireMatches(
  PROOF_TEST,
  proof,
  /assert_eq!\(\s*(?:a\.decision|audit\.decision),\s*enforced/,
  "the proof asserts observation records — never mutates — the enforced decision",
);
requireMatches(
  PROOF_TEST,
  proof,
  /with_audits/,
  "the proof documents that the single shadow event lands via with_audits (same-txn audit row)",
);

// --- (5) Forward-compatible: harden the runtime guard adapter once it lands. ---
function listRustFiles(relDir) {
  const dir = abs(relDir);
  if (!existsSync(dir)) return [];
  return readdirSync(dir, { recursive: true })
    .map((name) => join(relDir, String(name)))
    .filter((p) => p.endsWith(".rs"));
}

const guardFiles = listRustFiles(CRATES_DIR).filter((p) => {
  const src = read(p);
  return (
    // A guard adapter CALLS the observation fn; a crate root that merely
    // `pub use`-re-exports the name (no call site) is not an adapter and must
    // not be swept in here (false positive seen when #182 added the
    // re-export to backend/crates/platform/authz/src/lib.rs).
    /observe_cedar_pbac_decision\s*\(/.test(src) &&
    /node[_ ]?(transition|run)|waiting[_ ]?task|task[_ ]?completion|workflow[_ ]?runtime/i.test(src) &&
    !p.includes("/tests/") &&
    p !== CEDAR_PBAC
  );
});

if (guardFiles.length === 0) {
  notes.push(
    "workflow-runtime guard adapter not yet on the tree — contract-level observe-and-record proofs stand; " +
      "guard-adapter wiring is hardened automatically once the sibling runtime crate lands on feat/workflow-engine-m2.",
  );
} else {
  for (const guard of guardFiles) {
    const src = read(guard);
    assert(
      /DualEngineMode::LegacyOnly/.test(src),
      `${guard}: guard pins DualEngineMode::LegacyOnly`,
      `${guard}: a Cedar guard must pin DualEngineMode::LegacyOnly at M2`,
    );
    assert(
      /observe_cedar_pbac_decision/.test(src),
      `${guard}: guard records the shadow decision via observe_cedar_pbac_decision`,
      `${guard}: a Cedar guard must record the shadow decision via observe_cedar_pbac_decision`,
    );
    assert(
      /with_audits\b/.test(src),
      `${guard}: guard persists the shadow decision inside a with_audits transaction`,
      `${guard}: a Cedar guard must write the shadow audit row via with_audits (same txn as the state change)`,
    );
    // The guard must NOT branch enforcement on the inert Cedar verdict: it may
    // only observe it. Deny/return driven by a CedarEvaluation is a regression.
    assert(
      !/if[^;{]*cedar[^;{]*\{[\s\S]*?\breturn\b/i.test(src),
      `${guard}: enforcement never branches on the inert Cedar verdict`,
      `${guard}: a Cedar guard must not branch enforcement/return on the Cedar verdict under LegacyOnly`,
    );
  }
}

// --- (6) Wiring: this gate is a real command check in package.json + CI, and it ---
// --- does not weaken the sibling spine / strangler dark-landing gates.          ---
requireIncludes("package.json", '"check:workflow-runtime-spine"', "spine gate remains wired");
requireIncludes(
  "package.json",
  '"check:workflow-runtime-m2-strangler"',
  "M2 strangler dark-landing gate remains wired",
);
requireIncludes(
  "package.json",
  '"check:workflow-runtime-m2-cedar-guards": "node scripts/check-workflow-runtime-m2-cedar-guards.mjs"',
  "package script check:workflow-runtime-m2-cedar-guards is wired",
);
requireIncludes(
  ".github/workflows/ci.yml",
  "npm run check:workflow-runtime-m2-cedar-guards",
  "CI runs the M2 Cedar-guard observe-and-record gate",
);

if (failures.length) {
  console.error(`Workflow runtime M2 Cedar-guard observe-and-record gate FAILED (${failures.length} issues):`);
  for (const item of failures) console.error(`- ${item}`);
  process.exit(1);
}

console.log(`Workflow runtime M2 Cedar-guard observe-and-record gate passed (${passes.length} checks).`);
console.log(
  "- Guards are strictly observe-and-record under DualEngineMode::LegacyOnly: legacy is the sole enforcer, " +
    "inert Cedar can never deny/grant, and observe_cedar_pbac_decision records ONE shadow audit_events row inside " +
    "the SAME with_audits transaction as the guarded state change.",
);
for (const note of notes) console.log(`- NOTE: ${note}`);
for (const item of passes) console.log(`- ${item}`);
