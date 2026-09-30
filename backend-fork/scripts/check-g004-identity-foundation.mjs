#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";
import { beginGate, emitProvenanceIfRequested, noteAssertion, noteRead } from "./lib/gate-inputs.mjs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
beginGate({
  gate: "check:g004-identity-foundation",
  script: "scripts/check-g004-identity-foundation.mjs",
  documentInputs: [],
});

const matrixPath = "docs/benchmarks/g004-identity-foundation-matrix.json";
const auditPath = "docs/benchmarks/enterprise-ui-route-audit.json";
const goalId = "G004-identity-group-org-people-policy-fou";
const failures = [];
const passes = [];

function pathOf(path) {
  return resolve(root, path);
}

function read(path) {
  const abs = pathOf(path);
  noteRead(path);
  if (!existsSync(abs)) {
    failures.push(`${path}: missing`);
    return "";
  }
  return readFileSync(abs, "utf8");
}

function parseJson(path) {
  const text = read(path);
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch (error) {
    failures.push(`${path}: invalid JSON: ${error.message}`);
    return null;
  }
}

function pass(label) {
  passes.push(label);
}

function assert(condition, ok, failure) {
  if (condition) pass(ok);
  else failures.push(failure);
}

function requireFile(path, label = path) {
  assert(existsSync(pathOf(path)), `${label}: present`, `${label}: missing (${path})`);
}

function requireIncludes(path, needle, label) {
  const text = read(path);
  assert(text.includes(needle), label, `${label}: ${path} must include ${JSON.stringify(needle)}`);
  if (text.includes(needle)) noteAssertion(path);
}

function requireNotIncludes(path, needle, label) {
  const text = read(path);
  assert(!text.includes(needle), label, `${label}: ${path} must not include ${JSON.stringify(needle)}`);
  if (!text.includes(needle)) noteAssertion(path);
}

function requireArrayOfStrings(value, path, label) {
  assert(Array.isArray(value) && value.length > 0 && value.every((entry) => typeof entry === "string" && entry.length > 0), label, `${path}: ${label} must be a non-empty string array`);
}

export function hasPasskeyContract(routePaths) {
  return routePaths.some(
    (row) =>
      typeof row.requiredStory === "string" &&
      row.requiredStory.toLowerCase().includes("passkey"),
  );
}

export function hasPolicyContract(routePaths) {
  return routePaths.some(
    (row) =>
      row.path === "/settings/policy" &&
      row.routeGroup === "policy" &&
      typeof row.requiredStory === "string" &&
      row.requiredStory.toLowerCase().includes("policy"),
  );
}

const matrix = parseJson(matrixPath);
const routeAudit = parseJson(auditPath);
const packageJson = parseJson("package.json") ?? {};
const ci = read(".github/workflows/ci.yml");

assert(packageJson.scripts?.["check:g004-identity-foundation"] === "node scripts/check-g004-identity-foundation.mjs", "package script check:g004-identity-foundation", "package.json must define check:g004-identity-foundation");
assert(ci.includes("npm run check:g004-identity-foundation"), "CI runs G004 identity foundation gate", ".github/workflows/ci.yml must run npm run check:g004-identity-foundation");
requireFile(matrixPath, "G004 identity foundation matrix");
requireFile(auditPath, "enterprise UI route audit register");

if (matrix) {
  assert(matrix.schemaVersion === 1, "G004 matrix schema version 1", `${matrixPath}: schemaVersion must be 1`);
  assert(matrix.goalId === goalId, "G004 matrix goal id", `${matrixPath}: goalId must be ${goalId}`);
  assert(typeof matrix.nonClaimPolicy === "string" && matrix.nonClaimPolicy.includes("G009"), "G004 matrix records live-evidence non-claim policy", `${matrixPath}: nonClaimPolicy must reserve live rollout/screenshot claims for G009`);
  assert(Array.isArray(matrix.routePaths) && matrix.routePaths.length > 0, "G004 matrix routePaths", `${matrixPath}: routePaths must be non-empty`);
  requireArrayOfStrings(matrix.requiredBackendTests, matrixPath, "requiredBackendTests");
  assert(Array.isArray(matrix.backendContracts) && matrix.backendContracts.length >= 6, "G004 backend contract inventory", `${matrixPath}: backendContracts must include RLS/group/policy/passkey/employee contracts`);
  assert(Array.isArray(matrix.safetyAssertions) && matrix.safetyAssertions.length >= 8, "G004 safety assertions", `${matrixPath}: safetyAssertions must capture identity/group/policy/passkey/lifecycle guardrails`);

  const requiredRouteGroups = new Set(["auth", "platform", "settings", "identity-admin", "group-org", "people", "policy"]);
  const matrixRoutes = new Map();
  for (const row of matrix.routePaths ?? []) {
    assert(typeof row.path === "string" && row.path.startsWith("/"), `G004 route ${row.path ?? "<missing>"}: path`, `${matrixPath}: route row missing path`);
    assert(row.mustContainOwnerGoal === "G004", `G004 route ${row.path}: owner marker`, `${matrixPath}: route ${row.path} mustContainOwnerGoal must be G004`);
    assert(typeof row.requiredStory === "string" && row.requiredStory.length >= 24, `G004 route ${row.path}: required story`, `${matrixPath}: route ${row.path} requiredStory is too weak`);
    matrixRoutes.set(row.path, row);
  }
  assert(
    hasPasskeyContract([...matrixRoutes.values()]),
    "G004 matrix records a passkey contract",
    `${matrixPath}: no route requiredStory records the passkey contract`,
  );
  assert(
    hasPolicyContract([...matrixRoutes.values()]),
    "G004 matrix records a policy contract",
    `${matrixPath}: no policy route has a substantive requiredStory`,
  );
  for (const group of requiredRouteGroups) {
    assert([...matrixRoutes.values()].some((row) => row.routeGroup === group), `G004 route group ${group}: covered`, `${matrixPath}: no route covers group ${group}`);
  }

  if (routeAudit?.routeCoverage) {
    const auditByPath = new Map(routeAudit.routeCoverage.map((row) => [row.canonicalPath, row]));
    const auditG004Rows = routeAudit.routeCoverage.filter((row) => String(row.ownerLane ?? "").includes("G004"));
    assert(auditG004Rows.length >= 12, "enterprise route audit has G004-owned rows", `${auditPath}: expected G004-owned route rows`);
    for (const auditRow of auditG004Rows) {
      assert(matrixRoutes.has(auditRow.canonicalPath), `route audit ${auditRow.canonicalPath}: represented in G004 matrix`, `${matrixPath}: missing routePaths row for G004-owned route ${auditRow.canonicalPath}`);
    }
    const weakNeedles = ["fallback", "unclassified", "demo", "stub", "placeholder", "coming soon"];
    for (const [path, matrixRow] of matrixRoutes) {
      const auditRow = auditByPath.get(path);
      assert(Boolean(auditRow), `G004 matrix route ${path}: exists in route audit`, `${auditPath}: missing routeCoverage for ${path}`);
      if (!auditRow) continue;
      assert(String(auditRow.ownerLane ?? "").includes("G004"), `G004 matrix route ${path}: ownerLane contains G004`, `${auditPath}: ${path} ownerLane must include G004`);
      assert(String(auditRow.e2eSpec ?? "").includes("Required browser"), `G004 matrix route ${path}: browser story required`, `${auditPath}: ${path} e2eSpec must require browser story`);
      for (const field of ["sourceObject", "lifecycleStates", "denialScopeTest", "groupScopeStory"]) {
        assert(typeof auditRow[field] === "string" && auditRow[field].length >= 20, `G004 matrix route ${path}: ${field}`, `${auditPath}: ${path} missing ${field}`);
      }
      const combined = `${auditRow.ownerLane} ${auditRow.e2eSpec} ${auditRow.denialScopeTest} ${auditRow.groupScopeStory} ${matrixRow.requiredStory}`.toLowerCase();
      for (const needle of weakNeedles) {
        assert(!combined.includes(needle), `G004 matrix route ${path}: no weak ${needle} marker`, `${auditPath}: ${path} still contains weak marker ${needle}`);
      }
      assert(typeof auditRow.screenshotTraceEvidence === "string" && auditRow.screenshotTraceEvidence.includes("Pending"), `G004 matrix route ${path}: screenshot/trace is explicitly non-closed`, `${auditPath}: ${path} screenshotTraceEvidence must remain explicit until G009 live closure evidence lands`);
    }
  }

  for (const contract of matrix.backendContracts ?? []) {
    requireFile(contract.file, `backend contract ${contract.file}`);
    assert(typeof contract.contract === "string" && contract.contract.length >= 32, `backend contract ${contract.file}: rationale`, `${matrixPath}: ${contract.file} contract rationale is too weak`);
    requireArrayOfStrings(contract.requiredSnippets, matrixPath, `backend contract ${contract.file} required snippets`);
    for (const snippet of contract.requiredSnippets ?? []) {
      requireIncludes(contract.file, snippet, `backend contract ${contract.file}: ${snippet}`);
    }
  }


  for (const test of matrix.requiredBackendTests ?? []) requireFile(test, `G004 backend test ${test}`);

  // T1-CONV: backlog ledger goal-id prose assertion removed; matrix.goalId is the executable source (see docs/program/ledger/2026-08-05-gate-input-provenance.md).
  requireNotIncludes("scripts/check-people-hr-maturity.mjs", "G027-people-hr-lifecycle-org-scope-ui-mat", "people HR gate has no stale G027 owner id");
}

if (failures.length) {
  console.error(`G004 identity foundation gate failed:\n${failures.map((failure) => `- ${failure}`).join("\n")}`);
  process.exit(1);
}

emitProvenanceIfRequested();
console.log(`G004 identity foundation gate passed (${passes.length} checks).`);
for (const item of passes) console.log(`- ${item}`);
