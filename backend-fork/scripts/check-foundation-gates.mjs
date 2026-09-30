#!/usr/bin/env node
import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { createTextGate } from "./lib/text-gate.mjs";
import { beginGate, emitProvenanceIfRequested, noteAssertion } from "./lib/gate-inputs.mjs";
import { evaluateSecurityWorkflowHardening } from "./check-workflow-hardening.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
beginGate({
  gate: "check:foundation-gates",
  script: "scripts/check-foundation-gates.mjs",
  documentInputs: [
    "docs/CI-GATES.md",
    "docs/benchmarks/enterprise-parity-matrix.md",
    "docs/specs/backlog-clearance-ledger.md",
    "docs/specs/foundation-gates.md",
    "docs/program/console-fanout-epoch-contract.md",
    "docs/program/console-buck2-scale-playbook.md",
    "docs/specs/review-fix-merge-governance.md",
  ],
});
const textGate = createTextGate({
  root,
  includeFailure: ({ path, needle, label }) => `${label}: ${path} must include ${JSON.stringify(needle)}`,
  notIncludeFailure: ({ path, needle, label }) => `${label}: ${path} must not include ${JSON.stringify(needle)}`,
});
const { checks: passes, read, requireIncludes, requireNotIncludes, requireAbsent } = textGate;

// The drift-inventory checks below collect (rather than throw) so every drift is
// reported at once; they surface through this failures[] gate at the end. The
// shared text-gate helpers throw on the first failure — both paths exit non-zero.
const failures = [];

function requireFile(path, label = path) {
  if (existsSync(resolve(root, path))) {
    passes.push(`${label}: present`);
    return;
  }
  throw new Error(`${label}: missing (${path})`);
}

function uniqueSorted(values) {
  return [...new Set(values)].sort((a, b) => a.localeCompare(b));
}

function markdownCodeListUnderHeading(path, heading) {
  const lines = read(path).split(/\r?\n/);
  const headingLine = `### ${heading}`;
  const start = lines.findIndex((line) => line.trim() === headingLine);
  if (start === -1) {
    failures.push(`${path}: missing heading ${headingLine}`);
    return [];
  }

  const entries = [];
  for (const line of lines.slice(start + 1)) {
    if (/^#{2,3}\s+/.test(line)) {
      break;
    }
    const match = line.match(/^\s*-\s+`([^`]+)`/);
    if (match) {
      entries.push(match[1]);
    }
  }

  if (entries.length === 0) {
    failures.push(`${path}: ${headingLine} must contain a markdown bullet list of backticked entries`);
  }
  return uniqueSorted(entries);
}

function compareInventory(label, documented, actual, docsPath, sourcePath) {
  const missing = actual.filter((entry) => !documented.includes(entry));
  const stale = documented.filter((entry) => !actual.includes(entry));

  if (missing.length > 0 || stale.length > 0) {
    const details = [];
    if (missing.length > 0) {
      details.push(`missing from ${docsPath}: ${missing.join(", ")}`);
    }
    if (stale.length > 0) {
      details.push(`documented but not found in ${sourcePath}: ${stale.join(", ")}`);
    }
    failures.push(`${label} drift: ${details.join("; ")}`);
  } else {
    passes.push(`${label} inventory matches ${sourcePath}`);
  }
}

function extractCiBackendGatePackages(ciText) {
  const cargo = [...ciText.matchAll(/\bcargo\s+run(?:\s+-q)?\s+-p\s+(console-gate-[a-z0-9-]+)/g)].map(
    ([, gatePackage]) => gatePackage,
  );
  // Wave C / DN-0006: Required CI may invoke the same binaries via Buck2.
  // Match both `tools/buck2` (repo-root cwd) and `../tools/buck2` (backend/ cwd).
  const buck = [
    ...ciText.matchAll(
      /\b(?:\.\.\/)?tools\/buck2\s+run\s+\/\/backend\/ci\/gates\/[a-z0-9-]+:(console-gate-[a-z0-9-]+)/g,
    ),
  ].map(([, gatePackage]) => gatePackage);
  return uniqueSorted([...cargo, ...buck]);
}

function extractCiNpmRunInvocations(ciText) {
  const invocations = [];
  for (const line of ciText.split(/\r?\n/)) {
    for (const match of line.matchAll(/\bnpm\s+run\s+([^\s&|;]+)([^&|;]*)/g)) {
      invocations.push({
        script: match[1].replace(/^['"]|['"]$/g, ""),
        options: match[2] ?? "",
      });
    }
  }
  return invocations;
}

function requireNoMissingPackageScripts(label, scripts, packageJson, packagePath, displayName = (script) => script) {
  const missing = scripts.filter((script) => !Object.hasOwn(packageJson.scripts ?? {}, script));
  if (missing.length > 0) {
    failures.push(`${label}: ${packagePath} is missing scripts used by CI: ${missing.map(displayName).join(", ")}`);
  } else {
    passes.push(`${label}: all CI-run package scripts exist in ${packagePath}`);
  }
}

function requireNoOwnPackageScript(packageJson, packagePath, script) {
  if (Object.hasOwn(packageJson.scripts ?? {}, script)) {
    failures.push(`${packagePath}: scripts must not define retired ${script}`);
  } else {
    passes.push(`${packagePath}: scripts omit retired ${script}`);
  }
}

function requireCiGateDocsDriftInventory() {
  const docsPath = "docs/CI-GATES.md";
  const ciPath = ".github/workflows/ci.yml";
  const rootPackagePath = "package.json";

  const docs = read(docsPath);
  const ci = read(ciPath);
  const rootPackage = JSON.parse(read(rootPackagePath));
  const npmInvocations = extractCiNpmRunInvocations(ci);

  requireNoOwnPackageScript(rootPackage, rootPackagePath, "check:openapi-app");

  const backendGatePackages = extractCiBackendGatePackages(ci);
  const rootScripts = uniqueSorted(npmInvocations.map(({ script }) => script));
  const liveDocs = docs.split("\n## Backend gates\n", 1)[0];
  const liveDocScripts = uniqueSorted(
    extractCiNpmRunInvocations(liveDocs)
      .map(({ script }) => script.replace(/[`),.]+$/g, ""))
      .filter((script) => !script.includes("$")),
  );
  // The repo has no npm workspaces left, so any `--workspace` invocation in CI
  // is by definition uncovered by the drift policy.
  const unknownWorkspaceInvocations = npmInvocations.filter(({ options }) =>
    /\s--workspace\s+/.test(options),
  );

  if (!docs.includes("check:foundation-gates") || !docs.includes(".github/workflows/ci.yml")) {
    failures.push(`${docsPath}: CI drift inventory must name check:foundation-gates and .github/workflows/ci.yml as the source of truth`);
  }
  if (unknownWorkspaceInvocations.length > 0) {
    failures.push(
      `${ciPath}: npm workspace scripts are not covered by docs/CI-GATES.md drift policy: ${unknownWorkspaceInvocations
        .map(({ script, options }) => `${script}${options.trim() ? ` ${options.trim()}` : ""}`)
        .join(", ")}`,
    );
  }

  requireNoMissingPackageScripts("root CI package scripts", rootScripts, rootPackage, rootPackagePath);
  requireNoMissingPackageScripts(
    "live CI gate documentation package scripts",
    liveDocScripts,
    rootPackage,
    rootPackagePath,
  );

  compareInventory(
    "docs/CI-GATES.md backend console-gate binaries run by CI",
    markdownCodeListUnderHeading(docsPath, "Backend console-gate binaries run by CI"),
    backendGatePackages,
    docsPath,
    ciPath,
  );
  compareInventory(
    "docs/CI-GATES.md root package scripts run by CI",
    markdownCodeListUnderHeading(docsPath, "Root package scripts run by CI"),
    rootScripts,
    docsPath,
    `${ciPath} + ${rootPackagePath}`,
  );
}

// Canonical backlog and foundation-gate docs.
requireFile("docs/specs/backlog-clearance-ledger.md", "G001 backlog ledger");
requireIncludes("docs/specs/backlog-clearance-ledger.md", "## Lane taxonomy", "G001 lane ownership matrix");
requireIncludes("docs/specs/backlog-clearance-ledger.md", "## Generated-client and contract rules", "G001 generated-client rules");
requireIncludes("docs/specs/backlog-clearance-ledger.md", "## Evidence and signoff columns required", "G001 evidence/signoff columns");
requireFile("docs/specs/foundation-gates.md", "G002 foundation-gates contract");
requireIncludes("docs/specs/foundation-gates.md", "FOUNDATION-GATE-READY: true", "foundation gate readiness marker");
requireIncludes("docs/specs/foundation-gates.md", "G002-wave-1-shared-contracts-and-hard-gat", "current G002 goal id recorded");
requireIncludes("docs/specs/foundation-gates.md", "Domain goals G003-G009 must not claim completion", "downstream domain-lane block");
requireIncludes("docs/specs/foundation-gates.md", "## Gate B — workflow/approval/action lifecycle baseline", "workflow/action lifecycle gate recorded");
requireIncludes("docs/specs/foundation-gates.md", "## Gate C — ontology/import/export/object-lineage baseline", "ontology/import/export gate recorded");
requireIncludes("docs/specs/foundation-gates.md", "## Gate E — UI shell/design/i18n/a11y/no-text-wall baseline", "UI no-text-wall gate recorded");

for (const staleGoal of ["G011", "G012", "G013", "G014", "G015", "G016", "G017", "G018", "G019", "G020", "G021", "G022", "G023", "G024", "G025", "G026", "G027", "G028", "G029", "G030", "W1A-W1H"]) {
  requireNotIncludes("docs/specs/foundation-gates.md", staleGoal, `foundation gate has no stale ${staleGoal} plan reference`);
}

// Policy/audit/passkey baseline. Explicit required gates plus any additional
// console-gate binary CI runs, so a newly wired gate cannot ship without its crate.
for (const gate of [
  "layer-boundary",
  "audit-coverage",
  "migration-safety",
  "tenant-isolation",
  "pii-no-logs",
  "rls-arming",
]) {
  requireFile(`backend/ci/gates/${gate}/Cargo.toml`, `backend ${gate} gate`);
}
for (const gatePackage of extractCiBackendGatePackages(read(".github/workflows/ci.yml"))) {
  const gate = gatePackage.replace(/^console-gate-/, "");
  requireFile(`backend/ci/gates/${gate}/Cargo.toml`, `backend ${gate} gate (CI-run)`);
}
requireIncludes("backend/openapi/openapi.yaml", "Sensitive actions require a fresh passkey step-up assertion", "object action passkey step-up contract");
requireIncludes("backend/openapi/openapi.yaml", "tenant RLS, feature authorization, and branch scope", "approval feed authz/RLS contract");
requireIncludes("backend/openapi/openapi.yaml", "Both required agreements must be accepted", "initial-login agreement acceptance contract");
requireIncludes("backend/openapi/openapi.yaml", "status update is a sensitive passkey step-up action", "account lifecycle passkey step-up contract");
requireIncludes("backend/openapi/openapi.yaml", "Append-only Policy Studio audit evidence", "policy audit evidence contract");

// CI/CD/security/release baseline.
requireIncludes("package.json", "\"check:foundation-gates\": \"node scripts/check-foundation-gates.mjs\"", "package script check:foundation-gates");
requireIncludes("package.json", "\"test:text-gate\": \"node --test scripts/lib/text-gate.test.mjs\"", "package script test:text-gate");
requireIncludes(".github/workflows/ci.yml", "npm run check:foundation-gates", "CI runs foundation gate contract");
requireIncludes(".github/workflows/ci.yml", "npm run test:text-gate", "CI runs shared text-gate tests");
if (/^    paths(?:-ignore)?:/m.test(read(".github/workflows/ci.yml"))) {
  throw new Error("CI required-context triggers: .github/workflows/ci.yml must not define paths or paths-ignore filters");
}
passes.push("CI required-context triggers: push and pull_request are unfiltered");
requireCiGateDocsDriftInventory();
requireNotIncludes("docs/CI-GATES.md", "test:contract", "live CI gate docs exclude retired generated-client round-trip");
requireNotIncludes("docs/CI-GATES.md", "check:openapi-app", "live CI gate docs exclude retired app-served OpenAPI gate");
requireNotIncludes("docs/CI-GATES.md", "CONTRACT_DATABASE_URL", "live CI gate docs exclude retired contract database handoff");
for (const ciNeedle of [
  "cargo fmt --all -- --check",
  "cargo clippy --all-targets -- -D warnings",
  "SQLX_OFFLINE=true cargo test",
  "cargo run -p console-gate-audit-coverage",
  "cargo run -p console-gate-pii-no-logs",
  "cargo run -p console-gate-rls-arming",
  "npm run check:platform-contract-drift",
]) {
  requireIncludes(".github/workflows/ci.yml", ciNeedle, `CI gate: ${ciNeedle}`);
}
for (const securityNeedle of [
  "trivy fs --scanners vuln,secret",
  "node --test scripts/generate-trivy-dev-codegen-exceptions.test.mjs",
  "node scripts/generate-trivy-dev-codegen-exceptions.mjs --check",
  "--ignorefile security/trivy-dev-codegen-exceptions.yaml",
  "trivy config --severity HIGH,CRITICAL --exit-code 1",
  "cargo-security-tools/bin/cargo-audit",
  "cargo-security-tools/bin/cargo-deny",
  "node --test scripts/check-workflow-hardening.test.mjs",
  "node --test scripts/check-node-audit-exceptions.test.mjs",
  "npm audit --omit=dev --audit-level=high --json",
  "check-node-audit-exceptions.mjs --mode production",
  "npm audit --audit-level=high --json",
  "check-node-audit-exceptions.mjs --mode dev-codegen",
]) {
  requireIncludes(".github/workflows/security.yml", securityNeedle, `security workflow: ${securityNeedle}`);
}
const securityWorkflowHardening = evaluateSecurityWorkflowHardening(
  read(".github/workflows/security.yml"),
);
passes.push(...securityWorkflowHardening.passes);
failures.push(...securityWorkflowHardening.failures);
requireFile("security/node-audit-exceptions.json", "Node audit exception registry");
requireFile("security/trivy-dev-codegen-exceptions.yaml", "Trivy dev/codegen exception registry");
requireFile("scripts/check-node-audit-exceptions.mjs", "Node audit exception gate");
requireFile("scripts/check-node-audit-exceptions.test.mjs", "Node audit exception gate regressions");
requireFile("scripts/generate-trivy-dev-codegen-exceptions.mjs", "Trivy exception projection gate");
requireFile("scripts/generate-trivy-dev-codegen-exceptions.test.mjs", "Trivy exception projection regressions");
requireIncludes("package.json", "\"test:node-audit-exceptions\"", "Node audit exception regression script");
requireIncludes("package.json", "\"check:trivy-dev-codegen-exceptions\"", "Trivy exception projection script");
requireIncludes("package.json", "\"test:trivy-dev-codegen-exceptions\"", "Trivy exception projection regression script");
for (const releaseNeedle of [
  "workflow_run:",
  "Classify exact immutable source release",
  "eligible: ${{ steps.admit.outputs.eligible }}",
  "needs.ci-admission.outputs.eligible == 'true'",
  "type=semver,pattern={{version}},value=${{ needs.ci-admission.outputs.release_tag }}",
  "type=raw,value=sha-${{ needs.ci-admission.outputs.release_sha }}",
  "org.opencontainers.image.version=${{ needs.ci-admission.outputs.release_version }}",
  "org.opencontainers.image.revision=${{ needs.ci-admission.outputs.release_sha }}",
  ".immutable == true",
  'readonly SECURITY_WORKFLOW_ID="296023731"',
  "Required / Security",
  "Install checksum-pinned Trivy without candidate checkout",
  "Trivy scan both arches (fail on HIGH/CRITICAL)",
  "target: linux/amd64",
  "target: linux/arm64",
  "docker buildx imagetools create",
  "cosign sign --yes",
  "attest-build-provenance",
  "Promote signed digests to production overlay",
]) {
  requireIncludes(".github/workflows/image-release.yml", releaseNeedle, `image release gate: ${releaseNeedle}`);
}
requireFile("scripts/check-image-release-workflow.test.mjs", "image release admission regressions");
requireIncludes(
  "package.json",
  "scripts/check-image-release-workflow.test.mjs",
  "image release regressions execute in production-hardening suite",
);
requireIncludes(".github/workflows/release-please.yml", "RELEASE_PLEASE_TOKEN", "release-please branch transport token documented");
requireIncludes("backend/rust-toolchain.toml", "channel = \"1.98.1\"", "Rust toolchain pinned to 1.98.1");

// Enterprise UX benchmark matrices stay repo-owned even though the browser
// shell they were written against now lives outside this repository.
requireIncludes("docs/benchmarks/enterprise-parity-matrix.md", "SAP Fiori", "enterprise UX benchmark matrix");
requireIncludes("docs/benchmarks/enterprise-parity-matrix.md", "Palantir", "ontology/operations benchmark matrix");

// Concurrent execution authority is repository-owned: CI must not rely on a
// developer-local profile, home-state metadata, or external orchestration runtime.
const foundationGateText = read("docs/specs/foundation-gates.md");
for (const [path, needle, label] of [
  ["docs/specs/foundation-gates.md", "docs/program/console-fanout-epoch-contract.md", "fan-out epoch contract recorded"],
  ["docs/specs/foundation-gates.md", "docs/program/console-buck2-scale-playbook.md", "Buck2 scale playbook recorded"],
  ["docs/program/console-fanout-epoch-contract.md", "Buck2 remains the only", "Buck2-only fan-out authority"],
  ["docs/program/console-fanout-epoch-contract.md", "exact-SHA", "exact-SHA fan-out evidence"],
  ["docs/program/console-buck2-scale-playbook.md", "bounded", "bounded fan-out evidence"],
  ["docs/program/console-buck2-scale-playbook.md", "multi-cell", "multi-cell ownership evidence"],
  ["docs/program/console-buck2-scale-playbook.md", "Candidate CI", "candidate CI evidence"],
  ["docs/program/console-buck2-scale-playbook.md", "full release matrices", "release matrix evidence"],
]) {
  requireIncludes(path, needle, label);
}
const liveAuthorityContracts = [
  "docs/specs/foundation-gates.md",
  "docs/specs/review-fix-merge-governance.md",
];
const retiredAuthorityPatterns = [
  [/\bnousresearch\s+hermes\b/i, "NousResearch Hermes authority"],
  [/\bhermes\s+(?:kanban|profile)\b/i, "Hermes Kanban/profile authority"],
  [/\bomx\b/i, "OMX authority"],
  [/\bomc\b/i, "OMC authority"],
  [/\bgjc\b/i, "GJC authority"],
  [/~\/\.codex(?:\/agents)?\b/i, "developer-home role authority"],
  [/\b(?:developer[- ]home|home[- ])(?:role|profile|agent)\s+authority\b/i, "developer-home role authority"],
];
function requireOnlyReactNativeHermesEngineLines(path) {
  const allowedLine = /^\s*(?:[-*]\s+)?`?React Native Hermes JS engine`?(?: technical dependency)?[.!]?\s*$/i;
  for (const [index, line] of read(path).split(/\r?\n/).entries()) {
    if (/\bhermes\b/i.test(line) && !allowedLine.test(line)) {
      throw new Error(`${path}:${index + 1} must not use Hermes outside the exact React Native Hermes JS engine technical line`);
    }
  }
  passes.push(`${path} permits only exact React Native Hermes JS engine technical lines`);
  noteAssertion(path);
}

for (const path of liveAuthorityContracts) {
  for (const [pattern, label] of retiredAuthorityPatterns) {
    requireAbsent(path, pattern, `${path} excludes retired ${label}`);
  }
  requireOnlyReactNativeHermesEngineLines(path);
}
if (foundationGateText.includes("Concurrent-delivery authority")) {
  passes.push("repository-owned concurrent execution authority recorded in foundation contract");
} else {
  failures.push("repository-owned concurrent execution authority missing from docs/specs/foundation-gates.md");
}

if (failures.length) {
  console.error("Foundation gate check failed:\n" + failures.map((failure) => `- ${failure}`).join("\n"));
  process.exit(1);
}

emitProvenanceIfRequested();
console.log(`Foundation gate check passed (${passes.length} checks).`);
for (const pass of passes) {
  console.log(`- ${pass}`);
}
