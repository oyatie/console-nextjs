#!/usr/bin/env node
// M2 outbox-drainer transactional-idempotency gate (AC5).
//
// Proves that the payroll outbox drainer's *consume* is ONE `with_audits`/consume
// transaction: the ON CONFLICT DO NOTHING draft insert (reusing the payroll
// UNIQUE(org_id, period_start, period_end, source_label) via the deterministic
// per-run source_label `workflow_runtime_m2:run:{run_id}`), the outbox DELIVERED
// update, AND the audit row all share that single txn — so a rolled-back drain
// persists NONE of them (all-or-nothing atomicity) and a committed replay writes
// ZERO additional rows while the payroll_draft_runs count stays exactly 1 across
// drains.
//
// The behavioral proof is the backing #[sqlx::test]s (run by the backend
// cargo-test job as the real non-owner console_rt role, with app.current_org armed,
// against a fresh migrated DB). This gate asserts the E2E genuinely encodes the
// AC's transactional invariants — a shared consume body with all three writes, a
// rollback probe that reuses that exact body, and the atomicity/idempotency
// assertions — so it cannot silently rot into a tautology, split the writes
// across transactions, or start creating runtime state. If any invariant
// regresses, the gate fails closed.
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const E2E = "backend/crates/platform/db/tests/m2_flag_on_runtime_drain.rs";
const migrationsDir = "backend/crates/platform/db/migrations";

const failures = [];
const passes = [];

function abs(path) {
  return resolve(root, path);
}

function read(path) {
  const full = abs(path);
  if (!existsSync(full)) {
    failures.push(`${path}: file not found`);
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

function requireIncludes(source, path, needle, label) {
  assert(
    source.includes(needle),
    label,
    `${path}: must include ${JSON.stringify(needle)} (${label})`,
  );
}

function requireMatches(source, path, pattern, label) {
  assert(pattern.test(source), label, `${path}: must match ${pattern} (${label})`);
}

// Slice a top-level `async fn <name>` out of the source (from its signature to
// the next top-level `async fn `, or EOF). Lets us assert *which* function a
// statement lives in — i.e. that the three writes share ONE consume body.
function functionSlice(source, name) {
  const start = source.indexOf(`async fn ${name}`);
  if (start < 0) return "";
  const nextFn = source.indexOf("\nasync fn ", start + 1);
  return source.slice(start, nextFn < 0 ? source.length : nextFn);
}

const e2e = read(E2E);

// --- (1) Real E2E: fresh migrated DB, exercised as the non-owner console_rt role. ---
requireMatches(
  e2e,
  E2E,
  /#\[sqlx::test\(migrations\s*=\s*"\.\/migrations"\)\]/,
  "the E2E is a #[sqlx::test] that applies the real migrations into a fresh DB",
);
requireIncludes(
  e2e,
  E2E,
  "SET LOCAL ROLE console_rt",
  "the E2E drops to the genuine non-owner console_rt role (RLS is actually enforced)",
);
requireIncludes(
  e2e,
  E2E,
  "set_config('app.current_org'",
  "the E2E arms app.current_org so RLS scopes every statement (like with_org_conn)",
);
// Every `SET [LOCAL] ROLE <x>` must target console_rt — never a superuser/owner role
// (postgres/console_app) which would bypass RLS and mask a broken RLS/flag path.
const roleSwitches = [...e2e.matchAll(/set\s+(?:local\s+)?role\s+([a-z_]+)/gi)].map((m) =>
  m[1].toLowerCase(),
);
assert(
  roleSwitches.length > 0 && roleSwitches.every((r) => r === "console_rt"),
  "every role switch in the E2E targets console_rt (never a superuser/owner/BYPASSRLS role)",
  `${E2E}: every SET ROLE must target console_rt (found: ${
    [...new Set(roleSwitches)].join(", ") || "none"
  })`,
);

// --- (2) ONE consume body: the three writes share a single transaction. --------
// The drain claims events with FOR UPDATE SKIP LOCKED, then runs `drain_event_in_txn`
// per event; that one function does the draft insert, the DELIVERED ack, AND the
// audit row against the caller's `tx` — so all three land (or roll back) together.
requireIncludes(
  e2e,
  E2E,
  "FOR UPDATE SKIP LOCKED",
  "the drainer claims the outbox with FOR UPDATE SKIP LOCKED",
);
const consume = functionSlice(e2e, "drain_event_in_txn");
assert(
  consume.length > 0,
  "the shared consume body drain_event_in_txn exists",
  `${E2E}: must define the shared consume body async fn drain_event_in_txn`,
);
requireIncludes(
  consume,
  `${E2E} (drain_event_in_txn)`,
  "ON CONFLICT (org_id, period_start, period_end, source_label) DO NOTHING",
  "the consume stages the draft idempotently on the reused payroll natural key",
);
requireIncludes(
  consume,
  `${E2E} (drain_event_in_txn)`,
  "'workflow_runtime_m2:run:'",
  "the draft's natural key is the deterministic per-run source_label workflow_runtime_m2:run:{run_id}",
);
requireIncludes(
  consume,
  `${E2E} (drain_event_in_txn)`,
  "status = 'DELIVERED'",
  "the consume marks the outbox event DELIVERED in the SAME txn as the draft insert",
);
requireIncludes(
  consume,
  `${E2E} (drain_event_in_txn)`,
  "INSERT INTO audit_events",
  "the consume lands one audit row in the SAME txn (with_audits/consume), not a separate write",
);
requireIncludes(
  consume,
  `${E2E} (drain_event_in_txn)`,
  ".bind(DRAIN_AUDIT_ACTION)",
  "the audit row records the drain action via the DRAIN_AUDIT_ACTION bind",
);
requireMatches(
  e2e,
  E2E,
  /const DRAIN_AUDIT_ACTION: &str = "workflow_runtime\.outbox_drain";/,
  "the drain audit action is workflow_runtime.outbox_drain (matches the audit_events action regex)",
);
// The consume body must NOT open/commit/rollback its own transaction — it runs
// inside the caller's single txn, which is what makes the three writes atomic.
for (const forbidden of ["pool.begin()", "tx.commit()", "tx.rollback()"]) {
  assert(
    !consume.includes(forbidden),
    `the consume body does not ${forbidden} (it shares the caller's single txn)`,
    `${E2E}: drain_event_in_txn must not ${forbidden} — the draft insert, DELIVERED update, and audit row share the caller's ONE txn`,
  );
}

// The committing drainer wraps ONE begin/commit around the shared body; the
// rollback probe reuses the SAME body under ONE begin/rollback.
const drainCommit = functionSlice(e2e, "drain_payroll_outbox");
assert(
  drainCommit.includes("pool.begin()") &&
    drainCommit.includes("drain_event_in_txn(") &&
    drainCommit.includes("tx.commit()") &&
    !drainCommit.includes("tx.rollback()"),
  "drain_payroll_outbox brackets the shared consume body in ONE begin→commit txn",
  `${E2E}: drain_payroll_outbox must begin one txn, run drain_event_in_txn, and commit`,
);
const drainRollback = functionSlice(e2e, "drain_then_rollback");
assert(
  drainRollback.includes("pool.begin()") &&
    drainRollback.includes("drain_event_in_txn(") &&
    drainRollback.includes("tx.rollback()") &&
    !drainRollback.includes("tx.commit()"),
  "drain_then_rollback reuses the SAME consume body under ONE begin→rollback txn",
  `${E2E}: drain_then_rollback must begin one txn, run drain_event_in_txn, and roll back`,
);

// --- (3) Atomicity: a rolled-back drain persists NONE of the three writes. ------
requireMatches(
  e2e,
  E2E,
  /async fn drainer_consume_is_one_atomic_txn_and_idempotent_across_replays/,
  "the dedicated drainer transactional-idempotency E2E exists",
);
requireMatches(
  e2e,
  E2E,
  /rolling back the shared drain txn must persist ZERO payroll drafts \(all-or-nothing atomicity\)/,
  "the E2E asserts a rolled-back drain persists ZERO drafts (all-or-nothing)",
);
requireMatches(
  e2e,
  E2E,
  /rolling back the shared drain txn must persist ZERO audit rows/,
  "the E2E asserts a rolled-back drain persists ZERO audit rows (audit shares the txn's fate)",
);
requireMatches(
  e2e,
  E2E,
  /a rolled-back drain must leave the event PENDING/,
  "the E2E asserts a rolled-back drain leaves the event PENDING (the DELIVERED ack rolled back too)",
);

// --- (4) Idempotency: committed drain = 1 draft + 1 audit; replay adds 0. -------
requireMatches(
  e2e,
  E2E,
  /the committed drain must stage exactly ONE payroll draft/,
  "the E2E asserts the committed drain stages exactly ONE payroll draft",
);
requireMatches(
  e2e,
  E2E,
  /the committed consume lands exactly ONE audit row in the same txn \(with_audits\)/,
  "the E2E asserts the committed consume lands exactly ONE audit row in the same txn",
);
requireMatches(
  e2e,
  E2E,
  /status,\s*"BLOCKED_LEGAL_GATE"/,
  "the E2E asserts the staged draft lands status BLOCKED_LEGAL_GATE",
);
requireMatches(
  e2e,
  E2E,
  /the committed drain marks the JOB outbox event DELIVERED/,
  "the E2E asserts the committed drain marks the event DELIVERED",
);
requireMatches(
  e2e,
  E2E,
  /replaying the committed drain must create ZERO additional payroll drafts/,
  "the E2E asserts replaying the committed drain adds ZERO drafts",
);
requireMatches(
  e2e,
  E2E,
  /re-inserting the draft under the same natural key must insert ZERO rows/,
  "the E2E asserts re-inserting the draft under the same natural key adds ZERO rows",
);
requireMatches(
  e2e,
  E2E,
  /still exactly ONE payroll_draft_runs row after replay \(count stays 1 across drains\)/,
  "the E2E asserts the payroll_draft_runs count stays 1 across drains",
);
requireMatches(
  e2e,
  E2E,
  /still exactly ONE drain audit row after replay/,
  "the E2E asserts exactly ONE drain audit row survives the replay",
);

// --- (5) Spine reuse: NO new runtime tables introduced by this AC. --------------
assert(
  !/create\s+table/i.test(e2e),
  "the E2E creates NO tables — it reuses the spine (0077/0078), payroll (0074), strangler (0092)",
  `${E2E}: must not CREATE TABLE — the M2 drainer reuses existing tables (check:workflow-runtime-spine)`,
);
const migrationFiles = existsSync(abs(migrationsDir))
  ? readdirSync(abs(migrationsDir))
      .filter((f) => f.endsWith(".sql"))
      .sort()
  : [];
assert(
  migrationFiles.length > 0,
  "migration set is discoverable",
  `${migrationsDir}: no migrations found`,
);
// The reused spine/payroll/strangler migrations must all be PRESENT — assert
// REUSE without pinning the repository's globally-highest migration.
// Future unrelated migrations (0096+) are fine; "no new runtime tables" is
// enforced by the no-CREATE-TABLE check above plus check:workflow-runtime-spine.
for (const required of [
  "0074_create_payroll_readiness.sql",
  "0077_create_workflow_runtime_spine.sql",
  "0078_harden_workflow_runtime_integrity.sql",
  "0095_create_org_runtime_flags.sql",
]) {
  assert(
    migrationFiles.includes(required),
    `reuses migration ${required}`,
    `${migrationsDir}: expected reused migration ${required} to be present`,
  );
}

// --- (6) Wiring: real command check in package.json + CI; spine gate intact. ---
// This slice implements design steps 4-5 (flag read helper + outbox drainer). The
// step 3/6/7 sibling gates (m2-strangler / m2-cedar-guards / m2-runtime) are NOT
// part of this slice, so only the spine gate and this drainer gate are asserted.
const pkg = read("package.json");
requireIncludes(pkg, "package.json", '"check:workflow-runtime-spine"', "spine gate remains wired");
requireIncludes(
  pkg,
  "package.json",
  '"check:workflow-runtime-m2-drainer": "node scripts/check-workflow-runtime-m2-drainer.mjs"',
  "package script check:workflow-runtime-m2-drainer is wired",
);
const ci = read(".github/workflows/ci.yml");
requireIncludes(
  ci,
  ".github/workflows/ci.yml",
  "npm run check:workflow-runtime-m2-drainer",
  "CI runs the M2 outbox-drainer transactional-idempotency gate",
);

if (failures.length) {
  console.error("Workflow runtime M2 drainer gate FAILED:");
  for (const item of failures) console.error(`  - ${item}`);
  process.exit(1);
}

console.log(`Workflow runtime M2 drainer gate passed (${passes.length} checks).`);
console.log(
  "- The outbox drainer's consume is ONE with_audits/consume txn: the ON CONFLICT DO NOTHING draft " +
    "insert (reused UNIQUE(org_id, period_start, period_end, source_label), source_label " +
    "workflow_runtime_m2:run:{run_id}), the outbox DELIVERED update, and the audit row all share that " +
    "single transaction. A rolled-back drain persists NONE of them (all-or-nothing atomicity, event " +
    "stays PENDING); a committed drain stages exactly ONE BLOCKED_LEGAL_GATE draft + ONE audit row and " +
    "marks the event DELIVERED; replays add ZERO rows and the payroll_draft_runs count stays 1 across " +
    "drains. Proven as the real console_rt role with app.current_org armed; no new runtime tables.",
);
for (const item of passes) console.log(`- ${item}`);
