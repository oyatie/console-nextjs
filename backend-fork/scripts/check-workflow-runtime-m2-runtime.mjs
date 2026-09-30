#!/usr/bin/env node
// M2 flag-ON runtime E2E gate (AC4).
//
// Proves that with the per-tenant workflow_runtime_m2_strangler flag turned ON
// for a TEST tenant only, the new runtime drives ONE run→node finite-state
// machine through the reused ADR-0018 spine and idempotently stages exactly ONE
// payroll_draft_runs row landing status BLOCKED_LEGAL_GATE — with a replay adding
// ZERO rows. The behavioral proof is the backing #[sqlx::test] (run by the
// backend cargo-test job as the real console_rt role against a fresh migrated DB);
// this gate asserts that E2E genuinely encodes the AC's invariants and that it
// reuses the spine (no new runtime tables), so it cannot silently rot into a
// tautology or start creating runtime state. If any invariant regresses, the gate
// fails closed.
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const E2E =
  "backend/crates/platform/db/tests/m2_flag_on_runtime_drain.rs";
const migrationsDir = "backend/crates/platform/db/migrations";

const failures = [];
const passes = [];

function abs(path) {
  return resolve(root, path);
}

function read(path) {
  const full = abs(path);
  if (!existsSync(full)) {
    failures.push(`${path}: file is missing`);
    return "";
  }
  return readFileSync(full, "utf8");
}

function pass(label) {
  passes.push(label);
}

function assert(condition, ok, failure) {
  if (condition) pass(ok);
  else failures.push(failure);
}

function requireIncludes(source, path, needle, label) {
  assert(source.includes(needle), label, `${path}: must include ${JSON.stringify(needle)} (${label})`);
}

function requireMatches(source, path, pattern, label) {
  assert(pattern.test(source), label, `${path}: must match ${pattern} (${label})`);
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
// Never a superuser / BYPASSRLS path that would mask a broken RLS/flag path.
// Every `SET [LOCAL] ROLE <x>` in the E2E must target console_rt — never a
// superuser/owner role (postgres/console_app) which would bypass RLS and mask a
// broken RLS/flag path. Scan actual statements, not documentation prose.
const roleSwitches = [...e2e.matchAll(/set\s+(?:local\s+)?role\s+([a-z_]+)/gi)].map(
  (m) => m[1].toLowerCase(),
);
assert(
  roleSwitches.length > 0 && roleSwitches.every((r) => r === "console_rt"),
  "every role switch in the E2E targets console_rt (never a superuser/owner/BYPASSRLS role)",
  `${E2E}: every SET ROLE must target console_rt (found: ${[...new Set(roleSwitches)].join(", ") || "none"})`,
);

// --- (2) Flag-ON for the TEST tenant only, resolved through the real resolver. ---
requireMatches(
  e2e,
  E2E,
  /INSERT INTO org_runtime_flags[\s\S]*enabled[\s\S]*VALUES[\s\S]*TRUE/,
  "the E2E enrolls the TEST tenant (org_runtime_flags enabled = TRUE) — flag ON",
);
requireIncludes(
  e2e,
  E2E,
  "org_runtime_flag_enabled",
  "the E2E resolves the flag through the real org_runtime_flag_enabled() resolver",
);
requireMatches(
  e2e,
  E2E,
  /strangler_enabled\(&pool, TEST_TENANT\)\.await,/,
  "the E2E asserts the enrolled TEST tenant resolves the strangler flag ON",
);
requireMatches(
  e2e,
  E2E,
  /!strangler_enabled\(&pool, OTHER_TENANT\)\.await,/,
  "the E2E asserts an un-enrolled tenant stays OFF (per-tenant strangler)",
);

// --- (3) One run→node FSM on the reused ADR-0018 spine. -------------------------
for (const table of ["workflow_runs", "workflow_node_runs", "workflow_outbox_events"]) {
  requireIncludes(e2e, E2E, table, `the E2E drives the spine table ${table}`);
}
// The completion tail's run FSM walks STARTING→RUNNING→SUCCEEDED: the payroll JOB
// node is terminal (production emit_payroll sets run_target=SUCCEEDED), so the run
// lands SUCCEEDED rather than parking WAITING (the strangler treats the approval
// gate as an already-satisfied precondition).
for (const state of ["'STARTING'", "'RUNNING'", "'SUCCEEDED'"]) {
  requireIncludes(e2e, E2E, state, `the E2E walks the run FSM through ${state}`);
}
for (const state of ["'PENDING'", "'SUCCEEDED'"]) {
  requireIncludes(e2e, E2E, state, `the E2E walks the node FSM through ${state}`);
}
requireIncludes(e2e, E2E, "'JOB'", "the payroll node emits a JOB outbox event (internal.jobs connector)");
requireIncludes(e2e, E2E, "internal.jobs", "the JOB outbox event targets the internal.jobs connector");
requireMatches(
  e2e,
  E2E,
  /the FSM must create exactly ONE workflow_runs row/,
  "the E2E asserts exactly ONE workflow_runs row is created",
);

// --- (4) Idempotent drain → exactly ONE BLOCKED_LEGAL_GATE draft. ---------------
requireIncludes(e2e, E2E, "FOR UPDATE SKIP LOCKED", "the drainer consumes the outbox with FOR UPDATE SKIP LOCKED");
requireIncludes(
  e2e,
  E2E,
  "ON CONFLICT (org_id, period_start, period_end, source_label) DO NOTHING",
  "the drainer stages the draft idempotently on the reused payroll natural key",
);
requireIncludes(
  e2e,
  E2E,
  "ON CONFLICT (org_id, idempotency_key) DO NOTHING",
  "re-emitting the outbox event is idempotent on the spine UNIQUE(org_id, idempotency_key)",
);
requireIncludes(
  e2e,
  E2E,
  "workflow_runtime_m2:run:",
  "the draft natural key is the deterministic per-run source_label workflow_runtime_m2:run:{run_id}",
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
  /the first drain must stage exactly ONE payroll draft/,
  "the E2E asserts the first drain stages exactly ONE payroll draft",
);
requireMatches(
  e2e,
  E2E,
  /replaying the drain must create ZERO additional payroll drafts/,
  "the E2E asserts a drain replay creates ZERO additional drafts",
);
requireMatches(
  e2e,
  E2E,
  /re-emitting the same JOB outbox event must insert ZERO rows/,
  "the E2E asserts re-emitting the outbox event inserts ZERO rows",
);
requireMatches(
  e2e,
  E2E,
  /re-inserting the draft under the same natural key must insert ZERO rows/,
  "the E2E asserts re-inserting the draft inserts ZERO rows",
);
requireMatches(
  e2e,
  E2E,
  /still exactly ONE payroll_draft_runs row after replay/,
  "the E2E asserts exactly ONE payroll_draft_runs row survives the replay",
);

// --- (5) Tenant scoping: the un-enrolled tenant sees NONE of the runtime state. -
requireMatches(
  e2e,
  E2E,
  /the un-enrolled tenant must see ZERO workflow_runs rows/,
  "the E2E asserts the runtime state is strictly tenant-scoped (RLS)",
);

// --- (6) Spine reuse: NO new runtime tables introduced by this AC. --------------
assert(
  !/create\s+table/i.test(e2e),
  "the E2E creates NO tables — it reuses the spine (0077/0078), payroll (0074), strangler (0095), studio (0069)",
  `${E2E}: must not CREATE TABLE — the M2 runtime reuses existing tables (check:workflow-runtime-spine)`,
);
// The reused spine/payroll/strangler/studio migrations must all be PRESENT (the
// M2 runtime binds to them). This asserts REUSE without pinning the repository's
// globally-highest migration to 0095 — future unrelated migrations (0096+) are
// fine; the "no new runtime tables" invariant is enforced by the no-CREATE-TABLE
// check above plus the check:workflow-runtime-spine gate.
const migrationFiles = existsSync(abs(migrationsDir))
  ? readdirSync(abs(migrationsDir)).filter((f) => f.endsWith(".sql")).sort()
  : [];
assert(migrationFiles.length > 0, "migration set is discoverable", `${migrationsDir}: no migrations found`);
for (const required of [
  "0069_create_workflow_studio.sql",
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

// --- (7) Wiring: real command check in package.json + CI; sibling gates intact. -
const pkg = read("package.json");
requireIncludes(pkg, "package.json", '"check:workflow-runtime-spine"', "spine gate remains wired");
requireIncludes(pkg, "package.json", '"check:workflow-runtime-m2-strangler"', "M2 strangler dark-landing gate remains wired");
requireIncludes(pkg, "package.json", '"check:workflow-runtime-m2-cedar-guards"', "M2 Cedar-guard gate remains wired");
requireIncludes(
  pkg,
  "package.json",
  '"check:workflow-runtime-m2-runtime": "node scripts/check-workflow-runtime-m2-runtime.mjs"',
  "package script check:workflow-runtime-m2-runtime is wired",
);
const ci = read(".github/workflows/ci.yml");
requireIncludes(
  ci,
  ".github/workflows/ci.yml",
  "npm run check:workflow-runtime-m2-runtime",
  "CI runs the M2 flag-ON runtime E2E gate",
);

if (failures.length) {
  console.error("Workflow runtime M2 flag-ON runtime gate FAILED:");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(`Workflow runtime M2 flag-ON runtime gate passed (${passes.length} checks).`);
console.log(
  "- Flag-ON (test tenant only) E2E drives one run→node FSM through the reused ADR-0018 spine and " +
    "idempotently stages exactly ONE payroll_draft_runs row landing BLOCKED_LEGAL_GATE " +
    "(source_label workflow_runtime_m2:run:{run_id}); the drain replay, outbox re-emit, and draft " +
    "re-insert each add ZERO rows. Proven as the real console_rt role with app.current_org armed; no new runtime tables.",
);
for (const item of passes) console.log(`- ${item}`);
