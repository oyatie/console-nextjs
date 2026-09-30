#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const migrationPath = "backend/crates/platform/db/migrations/0077_create_workflow_runtime_spine.sql";
const hardeningMigrationPath = "backend/crates/platform/db/migrations/0078_harden_workflow_runtime_integrity.sql";
const adrPath = "docs/decisions/ADR-0018-clean-room-rust-corporate-workflow-engine.md";
const failures = [];
const passes = [];

function abs(path) {
  return resolve(root, path);
}

function read(path) {
  const pathAbs = abs(path);
  if (!existsSync(pathAbs)) {
    failures.push(`${path}: missing`);
    return "";
  }
  return readFileSync(pathAbs, "utf8");
}

function normalizeSql(sql) {
  return sql.replace(/\s+/g, " ").toLowerCase();
}

function pass(label) {
  passes.push(label);
}

function assert(condition, ok, failure) {
  if (condition) pass(ok);
  else failures.push(failure);
}

function requireIncludes(path, needle, label) {
  const text = read(path);
  assert(text.includes(needle), label, `${path}: must include ${JSON.stringify(needle)} (${label})`);
}

function requireSqlIncludes(sql, needle, label) {
  assert(sql.includes(needle.toLowerCase()), label, `${migrationPath} or ${hardeningMigrationPath}: must include SQL fragment ${JSON.stringify(needle)} (${label})`);
}

requireIncludes(adrPath, "workflow_runs", "ADR records workflow runtime tables");
requireIncludes(adrPath, "workflow_outbox_events", "ADR records workflow outbox");

const migration = read(migrationPath);
const hardeningMigration = read(hardeningMigrationPath);
const runtimeSql = `${migration}
${hardeningMigration}`;
const sql = normalizeSql(runtimeSql);
const packageJson = JSON.parse(read("package.json") || "{}");
const ci = read(".github/workflows/ci.yml");

assert(
  packageJson.scripts?.["check:workflow-runtime-spine"] === "node scripts/check-workflow-runtime-spine.mjs",
  "package script check:workflow-runtime-spine",
  "package.json must define check:workflow-runtime-spine",
);
assert(
  ci.includes("npm run check:workflow-runtime-spine"),
  "CI runs workflow runtime spine gate",
  ".github/workflows/ci.yml must run npm run check:workflow-runtime-spine",
);

const tables = [
  "workflow_runs",
  "workflow_node_runs",
  "workflow_waiting_tasks",
  "workflow_outbox_events",
  "workflow_execution_locks",
];

for (const table of tables) {
  requireSqlIncludes(sql, `create table ${table}`, `${table}: created`);
  requireIncludes(migrationPath, `-- console-gate: audited-table ${table}`, `${table}: audited marker`);
  requireSqlIncludes(sql, `alter table ${table} enable row level security`, `${table}: RLS enabled`);
  requireSqlIncludes(sql, `alter table ${table} force row level security`, `${table}: RLS forced`);
  requireSqlIncludes(sql, `create policy org_isolation on ${table}`, `${table}: org isolation policy`);
  requireSqlIncludes(sql, `grant select, insert, update on ${table} to console_rt`, `${table}: table-qualified grants present`);
  requireSqlIncludes(sql, `trg_${table}_org_immutable`, `${table}: org immutable trigger`);
  requireSqlIncludes(sql, `trg_${table}_no_delete`, `${table}: no-delete trigger`);
}

for (const table of ["workflow_runs", "workflow_node_runs", "workflow_waiting_tasks", "workflow_outbox_events"]) {
  requireSqlIncludes(sql, `unique (id, org_id)`, `${table}: same-org compound key available`);
  requireSqlIncludes(sql, `references organizations(id)`, `${table}: scoped to organization`);
}

for (const table of ["workflow_node_runs", "workflow_waiting_tasks", "workflow_outbox_events"]) {
  requireSqlIncludes(sql, `foreign key (run_id, org_id) references workflow_runs(id, org_id)`, `${table}: run same-org FK`);
}

for (const required of [
  "definition_id",
  "definition_version",
  "idempotency_key",
  "correlation_id",
  "trace_id",
  "object_type",
  "object_id",
  "initiated_by",
  "started_at",
  "completed_at",
  "failed_at",
]) {
  requireSqlIncludes(sql, required, `workflow_runs field ${required}`);
}

for (const required of [
  "node_key",
  "node_type",
  "attempt",
  "input_payload",
  "output_payload",
  "error_payload",
]) {
  requireSqlIncludes(sql, required, `workflow_node_runs field ${required}`);
}

for (const required of [
  "waiting_key",
  "assignee_user_id",
  "assignee_role_key",
  "required_policy",
  "decision_payload",
  "completed_by",
  "completed_at",
  "passkey_assertion_id",
]) {
  requireSqlIncludes(sql, required, `workflow_waiting_tasks field ${required}`);
}

for (const required of [
  "channel",
  "destination_ref",
  "payload",
  "attempt_count",
  "next_attempt_at",
  "delivered_at",
  "dead_lettered_at",
]) {
  requireSqlIncludes(sql, required, `workflow_outbox_events field ${required}`);
}

for (const required of [
  "lock_key",
  "acquired_by",
  "expires_at",
]) {
  requireSqlIncludes(sql, required, `workflow_execution_locks field ${required}`);
}

for (const required of [
  "workflow_runtime_no_delete",
  "workflow_runtime_org_immutable",
  "trg_workflow_runs_no_delete",
  "trg_workflow_node_runs_no_delete",
  "trg_workflow_waiting_tasks_no_delete",
  "trg_workflow_outbox_events_no_delete",
]) {
  requireSqlIncludes(sql, required, `durability guard ${required}`);
}

requireSqlIncludes(sql, "foreign key (definition_id, org_id) references workflow_definitions(id, org_id)", "workflow runs bind to definition same-org FK");
requireSqlIncludes(sql, "foreign key (initiated_by, org_id) references users(id, org_id)", "workflow run initiator same-org FK");
requireSqlIncludes(sql, "foreign key (assignee_user_id, org_id) references users(id, org_id)", "waiting task assignee same-org FK");
requireSqlIncludes(sql, "foreign key (claimed_by, org_id) references users(id, org_id)", "waiting task claimer same-org FK");
requireSqlIncludes(sql, "foreign key (completed_by, org_id) references users(id, org_id)", "waiting task completer same-org FK");
requireSqlIncludes(sql, "unique (org_id, idempotency_key)", "workflow run idempotency per org");
requireSqlIncludes(sql, "unique (org_id, run_id, node_key, attempt)", "node attempt idempotency per run");
requireSqlIncludes(sql, "unique (org_id, run_id, id)", "node run identity is scoped to parent run");
requireSqlIncludes(sql, "foreign key (org_id, run_id, node_run_id) references workflow_node_runs(org_id, run_id, id)", "waiting/outbox node run must match parent run");
requireSqlIncludes(sql, "unique (org_id, idempotency_key)", "outbox idempotency per org");
requireSqlIncludes(sql, "workflow_outbox_events_delivered_requires_timestamp", "outbox delivered status requires timestamp");
requireSqlIncludes(sql, "workflow_outbox_events_dead_letter_requires_evidence", "outbox dead-letter status requires timestamp and error evidence");
requireSqlIncludes(sql, "unique (org_id, lock_key)", "execution lock uniqueness per org");
requireSqlIncludes(sql, "check ((object_type is null and object_id is null) or (object_type is not null and object_id is not null))", "object binding is explicit");
requireSqlIncludes(sql, "check ((assignee_user_id is not null)::int + (assignee_role_key is not null)::int + (required_policy is not null)::int >= 1)", "waiting task has an accountable assignee/policy");

if (failures.length) {
  console.error(`Workflow runtime spine gate failed:\n${failures.map((failure) => `- ${failure}`).join("\n")}`);
  process.exit(1);
}

console.log(`Workflow runtime spine gate passed (${passes.length} checks).`);
for (const item of passes) console.log(`- ${item}`);
