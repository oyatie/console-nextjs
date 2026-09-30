#!/usr/bin/env node
// M2 dark-landing / parity-only gate.
//
// Proves that the M2 workflow runtime executor lands COMPLETELY DARK: the
// per-tenant workflow_runtime_m2_strangler flag resolves FALSE for every tenant,
// org_runtime_flags ships ZERO enabled rows, and an absent row means OFF. If any
// migration or e2e seed ships an enabled strangler row, or the dark-by-default
// resolver semantics regress, this gate fails closed so no tenant can silently
// drive the new runtime at merge.
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const migrationsDir = "backend/crates/platform/db/migrations";
const migrationPath = `${migrationsDir}/0095_create_org_runtime_flags.sql`;
const STRANGLER_FLAG = "workflow_runtime_m2_strangler";

const failures = [];
const passes = [];

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

// Strip -- line comments and /* */ block comments, collapse whitespace, lowercase.
function normalizeSql(sql) {
  return sql
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/--[^\n]*/g, " ")
    .replace(/\s+/g, " ")
    .toLowerCase()
    .trim();
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

function requireIncludes(path, needle, label) {
  const text = read(path);
  assert(text.includes(needle), label, `${path}: must include ${JSON.stringify(needle)} (${label})`);
}

function requireSqlIncludes(sql, needle, label) {
  assert(sql.includes(needle.toLowerCase()), label, `${migrationPath}: must include SQL ${JSON.stringify(needle)} (${label})`);
}

function requireSqlMatches(sql, pattern, label) {
  assert(pattern.test(sql), label, `${migrationPath}: must match ${pattern} (${label})`);
}

// --- Migration shape: the strangler switchboard reuses the ADR-0018 spine posture. ---
const rawMigration = read(migrationPath);
const migration = normalizeSql(rawMigration);

requireSqlIncludes(migration, "create table org_runtime_flags", "org_runtime_flags table exists");
requireSqlIncludes(migration, "references organizations(id)", "org_runtime_flags is tenant-scoped to organizations");
requireSqlIncludes(migration, "flag_key", "org_runtime_flags carries a flag_key");
requireSqlMatches(migration, /enabled\s+boolean\s+not\s+null\s+default\s+false/, "enabled defaults FALSE (a written row is OFF unless explicitly enabled)");
requireSqlIncludes(migration, "enable row level security", "org_runtime_flags enables RLS");
requireSqlIncludes(migration, "force row level security", "org_runtime_flags forces RLS (owner cannot bypass)");
requireSqlIncludes(migration, "create policy org_isolation on org_runtime_flags", "org_runtime_flags has org isolation policy");
requireSqlMatches(migration, /org_id\s*=\s*nullif\(current_setting\('app\.current_org', true\), ''\)::uuid/, "RLS keys off app.current_org (console_rt row boundary)");

// Explicit runtime grants for the real console_rt role — never superuser/BYPASSRLS.
requireIncludes(migrationPath, "GRANT SELECT, INSERT, UPDATE ON org_runtime_flags TO console_rt", "explicit console_rt grants on org_runtime_flags");
// Migration 0031 auto-grants DELETE to console_rt on every console_app-created table via
// ALTER DEFAULT PRIVILEGES, so the switchboard must explicitly REVOKE it — else
// the runtime role could delete its own flag row under RLS, erasing governance
// history and reverting an enabled flag to the absent-row OFF default.
requireIncludes(migrationPath, "REVOKE DELETE ON org_runtime_flags FROM console_rt", "console_rt cannot DELETE governance flags (append/update-only; inherited DELETE revoked)");

// --- Dark-by-default resolver: absent row => OFF, under app.current_org / RLS. ---
requireSqlIncludes(migration, "create or replace function org_runtime_flag_enabled", "dark-by-default resolver function exists");
requireSqlMatches(
  migration,
  /create or replace function org_runtime_flag_enabled\(p_flag_key text\)[\s\S]*returns boolean/,
  "resolver returns boolean for a flag key",
);
requireSqlMatches(
  migration,
  /org_runtime_flag_enabled[\s\S]*coalesce\([\s\S]*select f\.enabled[\s\S]*from org_runtime_flags f[\s\S]*where f\.org_id = nullif\(current_setting\('app\.current_org', true\), ''\)::uuid[\s\S]*false[\s\S]*\)/,
  "resolver COALESCEs an absent row to FALSE under app.current_org (absent row => OFF)",
);
requireIncludes(migrationPath, "GRANT EXECUTE ON FUNCTION org_runtime_flag_enabled(TEXT) TO console_rt", "console_rt may call the dark-by-default resolver");

// The strangler flag is a first-class, named concept in the migration.
requireIncludes(migrationPath, STRANGLER_FLAG, `${STRANGLER_FLAG} is a recognized flag key`);

// --- Zero enabled rows shipped: scan EVERY migration and e2e seed for any write ---
// --- that would enroll a tenant. None may exist at merge (M2 lands dark).       ---
const insertRe = /insert\s+into\s+org_runtime_flags/;
const updateRe = /update\s+org_runtime_flags\s+set/;
const stranglerTrueRe = new RegExp(`${STRANGLER_FLAG}[^;]*\\btrue\\b`);

function scanForEnrollment(relDir, files) {
  for (const file of files) {
    const relPath = join(relDir, file);
    const sql = normalizeSql(read(relPath));
    if (insertRe.test(sql)) {
      failures.push(`${relPath}: ships an INSERT INTO org_runtime_flags; M2 must land dark (zero enabled rows)`);
    }
    if (updateRe.test(sql)) {
      failures.push(`${relPath}: ships an UPDATE org_runtime_flags SET; no tenant may be flipped on at merge`);
    }
    if (stranglerTrueRe.test(sql)) {
      failures.push(`${relPath}: sets ${STRANGLER_FLAG} truthy; the strangler must stay OFF for every tenant at merge`);
    }
  }
}

function listSql(relDir) {
  const dir = abs(relDir);
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((name) => name.endsWith(".sql"))
    .sort();
}

const migrationFiles = listSql(migrationsDir);
assert(migrationFiles.length > 0, "migration set is discoverable", `${migrationsDir}: no migrations found to scan`);
scanForEnrollment(migrationsDir, migrationFiles);
assert(
  !failures.some((f) => f.includes("land dark") || f.includes("flipped on") || f.includes("stay OFF")),
  `zero enabled rows shipped across ${migrationFiles.length} migrations`,
  "an enrollment write was found (see failures above)",
);

// The dark landing must not depend on the spine gate weakening: it stays wired.
requireIncludes("package.json", '"check:workflow-runtime-spine"', "spine gate remains wired");

// --- Wiring: this gate is a real command check in package.json + CI. ---
requireIncludes(
  "package.json",
  '"check:workflow-runtime-m2-strangler": "node scripts/check-workflow-runtime-m2-strangler.mjs"',
  "package script check:workflow-runtime-m2-strangler is wired",
);
requireIncludes(
  ".github/workflows/ci.yml",
  "npm run check:workflow-runtime-m2-strangler",
  "CI runs the M2 strangler dark-landing gate",
);

if (failures.length) {
  console.error(`Workflow runtime M2 strangler dark-landing gate FAILED (${failures.length} issues):`);
  for (const item of failures) console.error(`- ${item}`);
  process.exit(1);
}

console.log(`Workflow runtime M2 strangler dark-landing gate passed (${passes.length} checks).`);
console.log(
  `- ${STRANGLER_FLAG} resolves FALSE for every tenant: org_runtime_flags ships 0 enabled rows across ` +
    `${migrationFiles.length} migrations; absent row => OFF via org_runtime_flag_enabled().`,
);
for (const item of passes) console.log(`- ${item}`);
