#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";
import { beginGate, emitProvenanceIfRequested, noteAssertion, noteRead } from "./lib/gate-inputs.mjs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
beginGate({
  gate: "check:g007-collaboration-mobile-lifecycle",
  script: "scripts/check-g007-collaboration-mobile-lifecycle.mjs",
  documentInputs: ["docs/specs/foundation-gates.md"],
});

const matrixPath = "docs/benchmarks/g007-collaboration-mobile-lifecycle-matrix.json";
const auditPath = "docs/benchmarks/enterprise-ui-route-audit.json";
const goalId = "G007-collaboration-mail-calendar-poll-mob";
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
  assert(
    Array.isArray(value) && value.length > 0 && value.every((entry) => typeof entry === "string" && entry.length > 0),
    label,
    `${path}: ${label} must be a non-empty string array`,
  );
}

function requireContractSet(matrix, key, minCount, label) {
  // Own-property only — matrix JSON inherits Object.prototype; a key colliding with
  // a prototype name must not resolve to a Function / Object method.
  const contracts = matrix && typeof matrix === "object" && Object.hasOwn(matrix, key)
    ? matrix[key]
    : undefined;
  assert(Array.isArray(contracts) && contracts.length >= minCount, label, `${matrixPath}: ${key} must have at least ${minCount} rows`);
  for (const contract of contracts ?? []) {
    requireFile(contract.file, `${key} ${contract.file}`);
    assert(typeof contract.contract === "string" && contract.contract.length >= 48, `${key} ${contract.file}: rationale`, `${matrixPath}: ${contract.file} contract rationale is too weak`);
    requireArrayOfStrings(contract.requiredSnippets, matrixPath, `${key} ${contract.file} required snippets`);
    for (const snippet of contract.requiredSnippets ?? []) {
      requireIncludes(contract.file, snippet, `${key} ${contract.file}: ${snippet}`);
    }
  }
}

const matrix = parseJson(matrixPath);
const routeAudit = parseJson(auditPath);
const packageJson = parseJson("package.json") ?? {};
const ci = read(".github/workflows/ci.yml");

assert(
  packageJson.scripts?.["check:g007-collaboration-mobile-lifecycle"] === "node scripts/check-g007-collaboration-mobile-lifecycle.mjs",
  "package script check:g007-collaboration-mobile-lifecycle",
  "package.json must define check:g007-collaboration-mobile-lifecycle",
);
assert(
  ci.includes("npm run check:g007-collaboration-mobile-lifecycle"),
  "CI runs G007 collaboration mobile lifecycle gate",
  ".github/workflows/ci.yml must run npm run check:g007-collaboration-mobile-lifecycle",
);
requireFile(matrixPath, "G007 collaboration mobile lifecycle matrix");
requireFile(auditPath, "enterprise UI route audit register");

if (matrix) {
  assert(matrix.schemaVersion === 1, "G007 matrix schema version 1", `${matrixPath}: schemaVersion must be 1`);
  assert(matrix.goalId === goalId, "G007 matrix goal id", `${matrixPath}: goalId must be ${goalId}`);
  assert(
    typeof matrix.nonClaimPolicy === "string" && matrix.nonClaimPolicy.includes("G009"),
    "G007 matrix records live-evidence non-claim policy",
    `${matrixPath}: nonClaimPolicy must reserve live rollout/screenshot claims for G009`,
  );
  assert(Array.isArray(matrix.routePaths) && matrix.routePaths.length >= 3, "G007 matrix routePaths", `${matrixPath}: routePaths must cover collaboration routes`);
  assert(Array.isArray(matrix.dependencyRoutes) && matrix.dependencyRoutes.length >= 2, "G007 matrix dependency routePaths", `${matrixPath}: dependencyRoutes must cover Work Hub/Approvals dependencies`);
  requireArrayOfStrings(matrix.requiredBackendTests, matrixPath, "requiredBackendTests");
  assert(Array.isArray(matrix.safetyAssertions) && matrix.safetyAssertions.length >= 12, "G007 safety assertions", `${matrixPath}: safetyAssertions must capture collaboration/mail/mobile guardrails`);

  const requiredRouteGroups = new Set(["collaboration-hub", "messenger", "mailbox"]);
  const matrixRoutes = new Map();
  for (const row of matrix.routePaths ?? []) {
    assert(typeof row.path === "string" && row.path.startsWith("/"), `G007 route ${row.path ?? "<missing>"}: path`, `${matrixPath}: route row missing path`);
    assert(row.mustContainOwnerGoal === "G007", `G007 route ${row.path}: owner marker`, `${matrixPath}: route ${row.path} mustContainOwnerGoal must be G007`);
    assert(typeof row.requiredStory === "string" && row.requiredStory.length >= 72, `G007 route ${row.path}: required story`, `${matrixPath}: route ${row.path} requiredStory is too weak`);
    matrixRoutes.set(row.path, row);
  }
  for (const group of requiredRouteGroups) {
    assert([...matrixRoutes.values()].some((row) => row.routeGroup === group), `G007 route group ${group}: covered`, `${matrixPath}: no route covers group ${group}`);
  }

  if (routeAudit?.routeCoverage) {
    const auditByPath = new Map(routeAudit.routeCoverage.map((row) => [row.canonicalPath, row]));
    const ownedRows = routeAudit.routeCoverage.filter((row) => String(row.ownerLane ?? "").startsWith("G007"));
    assert(ownedRows.length >= 3, "enterprise route audit has G007-owned rows", `${auditPath}: expected G007-owned route rows`);
    for (const auditRow of ownedRows) {
      assert(matrixRoutes.has(auditRow.canonicalPath), `route audit ${auditRow.canonicalPath}: represented in G007 matrix`, `${matrixPath}: missing routePaths row for G007-owned route ${auditRow.canonicalPath}`);
    }
    const weakNeedles = ["unclassified", "demo", "placeholder", "coming soon", "black background", "text wall"];
    for (const [path, matrixRow] of matrixRoutes) {
      const auditRow = auditByPath.get(path);
      assert(Boolean(auditRow), `G007 matrix route ${path}: exists in route audit`, `${auditPath}: missing routeCoverage for ${path}`);
      if (!auditRow) continue;
      assert(String(auditRow.ownerLane ?? "").includes("G007"), `G007 matrix route ${path}: ownerLane contains G007`, `${auditPath}: ${path} ownerLane must include G007`);
      assert(String(auditRow.e2eSpec ?? "").includes("Required browser/mobile"), `G007 matrix route ${path}: browser/mobile story required`, `${auditPath}: ${path} e2eSpec must require browser/mobile story`);
      assert(String(auditRow.denialScopeTest ?? "").includes("Private thread"), `G007 matrix route ${path}: private/permission denial`, `${auditPath}: ${path} denialScopeTest must cover private/mail/poll denial`);
      assert(String(auditRow.groupScopeStory ?? "").includes("Group and subsidiary"), `G007 matrix route ${path}: group/subsidiary story`, `${auditPath}: ${path} groupScopeStory must preserve group/subsidiary collaboration scope`);
      for (const field of ["sourceObject", "lifecycleStates", "denialScopeTest", "groupScopeStory"]) {
        assert(typeof auditRow[field] === "string" && auditRow[field].length >= 36, `G007 matrix route ${path}: ${field}`, `${auditPath}: ${path} missing ${field}`);
      }
      const combined = `${auditRow.ownerLane} ${auditRow.e2eSpec} ${auditRow.denialScopeTest} ${auditRow.groupScopeStory} ${matrixRow.requiredStory}`.toLowerCase();
      for (const needle of weakNeedles) {
        assert(!combined.includes(needle), `G007 matrix route ${path}: no weak ${needle} marker`, `${auditPath}: ${path} still contains weak marker ${needle}`);
      }
      assert(typeof auditRow.screenshotTraceEvidence === "string" && auditRow.screenshotTraceEvidence.includes("Pending"), `G007 matrix route ${path}: screenshot/trace is explicitly non-closed`, `${auditPath}: ${path} screenshotTraceEvidence must remain explicit until G009 live closure evidence lands`);
    }

    for (const depRoute of matrix.dependencyRoutes ?? []) {
      const auditRow = auditByPath.get(depRoute.path);
      assert(Boolean(auditRow), `G007 dependency route ${depRoute.path}: exists in route audit`, `${auditPath}: missing dependency routeCoverage for ${depRoute.path}`);
      if (!auditRow) continue;
      const combined = `${auditRow.ownerLane} ${auditRow.sourceObject} ${auditRow.e2eSpec} ${auditRow.groupScopeStory}`;
      assert(combined.includes(depRoute.expectedDependency), `G007 dependency route ${depRoute.path}: ${depRoute.expectedDependency}`, `${auditPath}: ${depRoute.path} must mention dependency ${depRoute.expectedDependency}`);
      assert(typeof depRoute.requiredStory === "string" && depRoute.requiredStory.length >= 72, `G007 dependency route ${depRoute.path}: required story`, `${matrixPath}: dependency route ${depRoute.path} requiredStory is too weak`);
    }
  }

  requireContractSet(matrix, "backendContracts", 12, "G007 backend contract inventory");

  for (const test of matrix.requiredBackendTests ?? []) requireFile(test, `G007 backend test ${test}`);

  requireIncludes("docs/specs/foundation-gates.md", "notification rules", "foundation gate mentions notification contract");
  requireIncludes("docs/specs/foundation-gates.md", "passkey step-up", "foundation gate mentions passkey step-up contract");

}

if (failures.length) {
  console.error(`G007 collaboration mobile lifecycle gate failed:\n${failures.map((failure) => `- ${failure}`).join("\n")}`);
  process.exit(1);
}

emitProvenanceIfRequested();
console.log(`G007 collaboration mobile lifecycle gate passed (${passes.length} checks).`);
for (const item of passes) console.log(`- ${item}`);
