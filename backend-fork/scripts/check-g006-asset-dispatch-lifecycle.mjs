#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";
import { beginGate, emitProvenanceIfRequested, noteAssertion, noteRead } from "./lib/gate-inputs.mjs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
beginGate({
  gate: "check:g006-asset-dispatch-lifecycle",
  script: "scripts/check-g006-asset-dispatch-lifecycle.mjs",
  documentInputs: [
    "docs/specs/foundation-gates.md",
  ],
});

const matrixPath = "docs/benchmarks/g006-asset-dispatch-lifecycle-matrix.json";
const auditPath = "docs/benchmarks/enterprise-ui-route-audit.json";
const goalId = "G006-assets-equipment-inventory-dispatch";
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

const matrix = parseJson(matrixPath);
const routeAudit = parseJson(auditPath);
const packageJson = parseJson("package.json") ?? {};
const ci = read(".github/workflows/ci.yml");

assert(
  packageJson.scripts?.["check:g006-asset-dispatch-lifecycle"] === "node scripts/check-g006-asset-dispatch-lifecycle.mjs",
  "package script check:g006-asset-dispatch-lifecycle",
  "package.json must define check:g006-asset-dispatch-lifecycle",
);
assert(
  ci.includes("npm run check:g006-asset-dispatch-lifecycle"),
  "CI runs G006 asset dispatch lifecycle gate",
  ".github/workflows/ci.yml must run npm run check:g006-asset-dispatch-lifecycle",
);
requireFile(matrixPath, "G006 asset dispatch lifecycle matrix");
requireFile(auditPath, "enterprise UI route audit register");

if (matrix) {
  assert(matrix.schemaVersion === 1, "G006 matrix schema version 1", `${matrixPath}: schemaVersion must be 1`);
  assert(matrix.goalId === goalId, "G006 matrix goal id", `${matrixPath}: goalId must be ${goalId}`);
  assert(
    typeof matrix.nonClaimPolicy === "string" && matrix.nonClaimPolicy.includes("G009"),
    "G006 matrix records live-evidence non-claim policy",
    `${matrixPath}: nonClaimPolicy must reserve live rollout/screenshot claims for G009`,
  );
  assert(Array.isArray(matrix.routePaths) && matrix.routePaths.length >= 9, "G006 matrix routePaths", `${matrixPath}: routePaths must cover asset/dispatch routes`);
  assert(Array.isArray(matrix.dependencyRoutes) && matrix.dependencyRoutes.length >= 3, "G006 matrix dependency routePaths", `${matrixPath}: dependencyRoutes must cover work-order/finance/catalog dependencies`);
  requireArrayOfStrings(matrix.requiredBackendTests, matrixPath, "requiredBackendTests");
  assert(Array.isArray(matrix.backendContracts) && matrix.backendContracts.length >= 12, "G006 backend contract inventory", `${matrixPath}: backendContracts must include registry/dispatch/geodata/finance contracts`);
  assert(Array.isArray(matrix.safetyAssertions) && matrix.safetyAssertions.length >= 10, "G006 safety assertions", `${matrixPath}: safetyAssertions must capture owner/operator/transfer/search/map/dispatch/economics guardrails`);

  const requiredRouteGroups = new Set(["sites", "equipment-list", "equipment-detail", "equipment-manage", "legacy-import", "geodata", "dispatch-board", "dispatch-map", "maintenance-inspection"]);
  const matrixRoutes = new Map();
  for (const row of matrix.routePaths ?? []) {
    assert(typeof row.path === "string" && row.path.startsWith("/"), `G006 route ${row.path ?? "<missing>"}: path`, `${matrixPath}: route row missing path`);
    assert(row.mustContainOwnerGoal === "G006", `G006 route ${row.path}: owner marker`, `${matrixPath}: route ${row.path} mustContainOwnerGoal must be G006`);
    assert(typeof row.requiredStory === "string" && row.requiredStory.length >= 56, `G006 route ${row.path}: required story`, `${matrixPath}: route ${row.path} requiredStory is too weak`);
    matrixRoutes.set(row.path, row);
  }
  for (const group of requiredRouteGroups) {
    assert([...matrixRoutes.values()].some((row) => row.routeGroup === group), `G006 route group ${group}: covered`, `${matrixPath}: no route covers group ${group}`);
  }

  if (routeAudit?.routeCoverage) {
    const auditByPath = new Map(routeAudit.routeCoverage.map((row) => [row.canonicalPath, row]));
    const ownedG006Rows = routeAudit.routeCoverage.filter((row) => String(row.ownerLane ?? "").startsWith("G006"));
    assert(ownedG006Rows.length >= 9, "enterprise route audit has G006-owned rows", `${auditPath}: expected G006-owned route rows`);
    for (const auditRow of ownedG006Rows) {
      assert(matrixRoutes.has(auditRow.canonicalPath), `route audit ${auditRow.canonicalPath}: represented in G006 matrix`, `${matrixPath}: missing routePaths row for G006-owned route ${auditRow.canonicalPath}`);
    }
    const weakNeedles = ["unclassified", "demo", "placeholder", "coming soon", "black background", "text wall"];
    for (const [path, matrixRow] of matrixRoutes) {
      const auditRow = auditByPath.get(path);
      assert(Boolean(auditRow), `G006 matrix route ${path}: exists in route audit`, `${auditPath}: missing routeCoverage for ${path}`);
      if (!auditRow) continue;
      assert(String(auditRow.ownerLane ?? "").includes("G006"), `G006 matrix route ${path}: ownerLane contains G006`, `${auditPath}: ${path} ownerLane must include G006`);
      assert(String(auditRow.e2eSpec ?? "").includes("Required browser"), `G006 matrix route ${path}: browser story required`, `${auditPath}: ${path} e2eSpec must require browser story`);
      assert(String(auditRow.denialScopeTest ?? "").includes("Wrong org"), `G006 matrix route ${path}: wrong-org denial`, `${auditPath}: ${path} denialScopeTest must cover wrong-org mutation denial`);
      assert(String(auditRow.groupScopeStory ?? "").includes("KNL operator"), `G006 matrix route ${path}: KNL operator/affiliate story`, `${auditPath}: ${path} groupScopeStory must preserve KNL operator story`);
      for (const field of ["sourceObject", "lifecycleStates", "denialScopeTest", "groupScopeStory"]) {
        assert(typeof auditRow[field] === "string" && auditRow[field].length >= 36, `G006 matrix route ${path}: ${field}`, `${auditPath}: ${path} missing ${field}`);
      }
      const combined = `${auditRow.ownerLane} ${auditRow.e2eSpec} ${auditRow.denialScopeTest} ${auditRow.groupScopeStory} ${matrixRow.requiredStory}`.toLowerCase();
      for (const needle of weakNeedles) {
        assert(!combined.includes(needle), `G006 matrix route ${path}: no weak ${needle} marker`, `${auditPath}: ${path} still contains weak marker ${needle}`);
      }
      assert(typeof auditRow.screenshotTraceEvidence === "string" && auditRow.screenshotTraceEvidence.includes("Pending"), `G006 matrix route ${path}: screenshot/trace is explicitly non-closed`, `${auditPath}: ${path} screenshotTraceEvidence must remain explicit until G009 live closure evidence lands`);
    }

    for (const depRoute of matrix.dependencyRoutes ?? []) {
      const auditRow = auditByPath.get(depRoute.path);
      assert(Boolean(auditRow), `G006 dependency route ${depRoute.path}: exists in route audit`, `${auditPath}: missing dependency routeCoverage for ${depRoute.path}`);
      if (!auditRow) continue;
      const combined = `${auditRow.ownerLane} ${auditRow.sourceObject} ${auditRow.e2eSpec} ${auditRow.groupScopeStory}`;
      assert(combined.includes(depRoute.expectedDependency), `G006 dependency route ${depRoute.path}: ${depRoute.expectedDependency}`, `${auditPath}: ${depRoute.path} must mention dependency ${depRoute.expectedDependency}`);
      assert(typeof depRoute.requiredStory === "string" && depRoute.requiredStory.length >= 56, `G006 dependency route ${depRoute.path}: required story`, `${matrixPath}: dependency route ${depRoute.path} requiredStory is too weak`);
    }
  }

  for (const contract of matrix.backendContracts ?? []) {
    requireFile(contract.file, `backend contract ${contract.file}`);
    assert(typeof contract.contract === "string" && contract.contract.length >= 48, `backend contract ${contract.file}: rationale`, `${matrixPath}: ${contract.file} contract rationale is too weak`);
    requireArrayOfStrings(contract.requiredSnippets, matrixPath, `backend contract ${contract.file} required snippets`);
    for (const snippet of contract.requiredSnippets ?? []) {
      requireIncludes(contract.file, snippet, `backend contract ${contract.file}: ${snippet}`);
    }
  }


  for (const test of matrix.requiredBackendTests ?? []) requireFile(test, `G006 backend test ${test}`);

  requireIncludes("docs/specs/foundation-gates.md", "ownership transfers", "foundation gate mentions legal ownership transfer contract");
  requireIncludes("docs/specs/foundation-gates.md", "assets only to equipment/inventory schemas", "foundation gate mentions asset/inventory mapping contract");

}

if (failures.length) {
  console.error(`G006 asset dispatch lifecycle gate failed:\n${failures.map((failure) => `- ${failure}`).join("\n")}`);
  process.exit(1);
}

emitProvenanceIfRequested();
console.log(`G006 asset dispatch lifecycle gate passed (${passes.length} checks).`);
for (const item of passes) console.log(`- ${item}`);
