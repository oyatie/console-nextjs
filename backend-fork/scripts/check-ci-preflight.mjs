#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { appendFileSync, lstatSync, readFileSync, realpathSync } from "node:fs";
import { dirname, posix, relative, resolve, sep } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { fileURLToPath, pathToFileURL } from "node:url";
import yaml from "js-yaml";

import { gitFixtureEnvironment } from "./lib/git-fixture-environment.mjs";

const dotSlashBootstrap = "tools/buck/install_dotslash.sh";
const reindeerToolchainLock = "third-party/rust/reindeer/upstream.lock";
const reindeerToolchainSource = `source ${reindeerToolchainLock}`;
const reindeerToolchainInstall = 'rustup toolchain install "$REINDEER_TOOLCHAIN" --profile minimal';
const strictShellMode = "set -euo pipefail";
const reindeerToolchainOverride = /^(?:export\s+)?REINDEER_TOOLCHAIN\s*=/;
const ciPreflightTestCommand = "node --test scripts/check-ci-preflight.test.mjs";
const buckImpactPlannerTestCommand = "python3 -m unittest -v tools.buck.impact.test_plan";
const reasoningLensManifestCommand = "node scripts/check-reasoning-lens-manifest.mjs";
const reasoningLensManifestName = "Reasoning lens manifest drift";
const reasoningLensRegressionCommand = "node --test scripts/check-reasoning-lens-manifest.test.mjs";
const reasoningLensRegressionName = "Reasoning lens manifest regression";
const consoleRouteInventoryTestCommand = "node --test scripts/console/route-inventory.test.mjs";
const consoleAuthorityTrainTestCommand = "node --test scripts/console/verify-console-authority-train.test.mjs";
const consoleTruthLedgerTestCommand = "node --test scripts/console/validate-console-truth-ledger.test.mjs";
const consoleFanoutPlannerTestCommand = "node --test scripts/console/plan-fanout.test.mjs";
const releaseMetadataRegressionCommand = "node --test scripts/check-release-metadata.test.mjs";
const releaseMetadataGateRunSha256 = "9ba171262e917ba80b83342f58164d470a7d28060e5b3322fd597d9316c40c98";
// This one gates the highest-privilege script in the repository — the `pull_request_target`
// bootstrap verifier — and executed NOWHERE until it was wired: `package.json` declared
// `test:console-authority-bootstrap` and no workflow ever invoked it. Breaking the verifier
// turned all its tests red locally while CI stayed green.
const consoleBootstrapTestCommand = "node --test scripts/console/release-please-pr-envelope.test.mjs scripts/console/release-please-pr-fallback.test.mjs scripts/console/verify-console-pr-authority-bootstrap.test.mjs scripts/console/release-please-bot-candidate.test.mjs scripts/console/release-authority-proof.test.mjs scripts/console/converge-release-please-doc-custody.test.mjs scripts/console/verify-console-merge-group-authority.test.mjs";
/** Sibling jobs: run expensive body only when preflight classified a non-docs change. */
const runHeavyCondition = "${{ needs.preflight.outputs.run_heavy == 'true' }}";
/** Sibling jobs: emit explicit skip-proof success for a thin path class. */
const skipProofCondition = "${{ needs.preflight.outputs.run_heavy != 'true' }}";
/** Live Postgres facets: oyatie pg-gate analog, independent of run_heavy. */
const runLivePostgresCondition = "${{ needs.preflight.outputs.run_live_postgres == 'true' }}";
const skipLivePostgresCondition = "${{ needs.preflight.outputs.run_live_postgres != 'true' }}";
/** Heavy proofs that are postsubmit-only (push / workflow_dispatch). */
const postsubmitHeavyJobIf =
  "${{ needs.preflight.result == 'success' && needs.preflight.outputs.run_heavy == 'true' && (github.event_name == 'push' || github.event_name == 'workflow_dispatch') }}";
const postsubmitProtectedJobs = Object.freeze([
  "backend",
  "company-conformance",
  "domain-unit",
  "generated-face-authority",
  "migration-expand-contract",
]);
/** Preflight-local: Rust/Buck/cargo steps for heavy path classes only. */
const preflightRunHeavyCondition = "${{ steps.path_class.outputs.run_heavy == 'true' }}";
/** repo-gates / leaves that already gate on !cancelled(): keep cancel semantics AND run_heavy. */
const runHeavyUnlessCancelledCondition =
  "${{ !cancelled() && needs.preflight.outputs.run_heavy == 'true' }}";
const runHeavyAlwaysCondition =
  "${{ always() && needs.preflight.outputs.run_heavy == 'true' }}";
const postgresAggregateNonEvaluationCondition =
  "${{ needs.preflight.result != 'success' }}";
const postgresAggregateSkipCondition =
  "${{ needs.preflight.result == 'success' && needs.preflight.outputs.run_live_postgres != 'true' }}";
const postgresAggregateHeavyCondition =
  "${{ needs.preflight.result == 'success' && needs.preflight.outputs.run_live_postgres == 'true' }}";

// Fail-slow one-sweep CI (D4): independent steps run even after a sibling step
// fails; dependent steps skip (not red) when their dependency failed. The step
// ids below are the dependency roots named in ci.yml.
const preflightCheckoutDependentCondition =
  "${{ !cancelled() && steps.checkout.outcome == 'success' }}";
const preflightSetupNodeDependentCondition =
  "${{ !cancelled() && steps.setup-node.outcome == 'success' }}";
const preflightNpmCiDependentCondition =
  "${{ !cancelled() && steps.npm-ci.outcome == 'success' }}";
const preflightReleaseMetadataCondition =
  "${{ !cancelled() && steps.npm-ci.outcome == 'success' && steps.path_class.outputs.path_class == 'release-metadata-only' }}";
const releaseMetadataEnvironment = Object.freeze({
  RELEASE_METADATA_BASE_SHA: "${{ github.event_name == 'pull_request' && github.event.pull_request.base.sha || github.event.merge_group.base_sha || github.event.before }}",
  RELEASE_METADATA_HEAD_SHA: "${{ github.event_name == 'pull_request' && github.event.pull_request.head.sha || github.event.merge_group.head_sha || github.sha }}",
});
const preflightCheckoutHeavyCondition =
  "${{ !cancelled() && steps.checkout.outcome == 'success' && steps.path_class.outputs.run_heavy == 'true' }}";
const preflightBuckHeavyCondition =
  "${{ !cancelled() && steps.dotslash.outcome == 'success' && steps.path_class.outputs.run_heavy == 'true' }}";
const preflightRustHeavyCondition =
  "${{ !cancelled() && steps.rust-toolchain.outcome == 'success' && steps.path_class.outputs.run_heavy == 'true' }}";
const preflightNpmCiHeavyCondition =
  "${{ !cancelled() && steps.npm-ci.outcome == 'success' && steps.path_class.outputs.run_heavy == 'true' }}";
const collectFailuresCondition = "${{ !cancelled() }}";
/** Backend fail-slow: independent (no prior-step dependency) and topology-dependent guards.
 *
 * `backend` is a three-leg matrix (cargo | buck-app | buck-dev-auth). Steps that
 * belong to ONE leg carry `matrix.leg == '<leg>' &&` ahead of the guard; steps
 * that run on EVERY leg (setup, the topology reconcile, collect-failures) keep
 * the leg-free form. The helpers exist so a leg name appears in exactly one
 * place per step and cannot drift from `BACKEND_MATRIX_LEGS` below. */
const BACKEND_MATRIX_LEGS = Object.freeze(["cargo", "buck-app", "buck-dev-auth"]);
const backendIndependentCondition =
  "${{ !cancelled() && needs.preflight.outputs.run_heavy == 'true' }}";
const backendTopologyDependentCondition =
  "${{ !cancelled() && steps.topology.outcome == 'success' && needs.preflight.outputs.run_heavy == 'true' }}";
function backendLegCondition(leg) {
  if (!BACKEND_MATRIX_LEGS.includes(leg)) throw new Error(`unknown backend leg ${leg}`);
  return `\${{ !cancelled() && matrix.leg == '${leg}' && needs.preflight.outputs.run_heavy == 'true' }}`;
}
function backendLegTopologyCondition(leg) {
  if (!BACKEND_MATRIX_LEGS.includes(leg)) throw new Error(`unknown backend leg ${leg}`);
  return `\${{ !cancelled() && matrix.leg == '${leg}' && steps.topology.outcome == 'success' && needs.preflight.outputs.run_heavy == 'true' }}`;
}

export const PATH_CLASS_RULES_VERSION = "6";
const docsOnlyRootFiles = new Set([
  "README.md",
  "CHANGELOG.md",
  "SPEC.md",
  "DESIGN.md",
  "HANDOFF.md",
  "AGENTS.md",
  "CLAUDE.md",
  "Agents.md",
  "Claude.md",
]);
const releaseMetadataRequiredPaths = Object.freeze([
  ".release-please-manifest.json",
  "CHANGELOG.md",
]);
const releaseMetadataCustodyPaths = Object.freeze([
  "docs/documentation-manifest.seed.json",
  "docs/documentation-index.json",
]);
const releaseMetadataAllowedPaths = new Set([
  ...releaseMetadataRequiredPaths,
  ...releaseMetadataCustodyPaths,
]);

/**
 * Fail-closed path-class classifier (S-CI2 / console-7rc mechanism B).
 * Only exact docs-only and release-metadata-only classes set runHeavy=false;
 * every other class keeps the full matrix. Live Postgres is a second axis
 * (oyatie pg-gate): adapter/SQL/migrations/.sqlx/platform-db/named gates and
 * postgres harness paths, or fail-closed unknown classes. Typical backend
 * Rust leaves stay skip-proofed so Required / CI can finish under 5m.
 */
export function isLivePostgresPath(path) {
  if (typeof path !== "string" || path.length === 0) return false;
  if (path.includes("adapter-postgres")) return true;
  if (path.endsWith(".sql") || /(^|\/)migrations\//.test(path)) return true;
  if (/(^|\/)\.sqlx(\/|$)/.test(path)) return true;
  if (path.includes("postgres_bridge")) return true;
  if (path.startsWith("backend/crates/platform/db/")) return true;
  if (
    path.startsWith("backend/ci/gates/writer-ownership/")
    || path.startsWith("backend/ci/gates/rls-arming/")
    || path.startsWith("backend/ci/gates/tenant-isolation/")
    || path.startsWith("backend/ci/gates/migration-safety/")
  ) {
    return true;
  }
  const lower = path.toLowerCase();
  if (
    (path.startsWith("tools/ci/")
      || path.startsWith("tools/buck/")
      || path.startsWith("ci/harness/")
      || path.startsWith("ops/")
      || path.startsWith("tools/lanes/"))
    && lower.includes("postgres")
  ) {
    return true;
  }
  return false;
}

function withLivePostgres(result, paths = []) {
  const runLivePostgres = result.runHeavy === true && (
    result.pathClass === "unknown"
    || (Array.isArray(paths) && paths.some((path) => isLivePostgresPath(path)))
  );
  return { ...result, runLivePostgres };
}

export function classifyChangedPaths(paths) {
  if (!Array.isArray(paths) || paths.length === 0) {
    return withLivePostgres({
      pathClass: "unknown",
      docsOnly: false,
      runHeavy: true,
      reason: "empty-or-unreadable",
    });
  }
  const normalizedPaths = [];
  for (const raw of paths) {
    if (typeof raw !== "string" || raw.length === 0
      || raw.includes("\0") || raw.includes("\\") || /(^|\/)\.\.(\/|$)/.test(raw)) {
      return withLivePostgres({
        pathClass: "unknown",
        docsOnly: false,
        runHeavy: true,
        reason: "hostile-path",
      });
    }
    const p = raw;
    normalizedPaths.push(p);
  }
  const uniquePaths = new Set(normalizedPaths);
  if (uniquePaths.size !== normalizedPaths.length) {
    return withLivePostgres({
      pathClass: "unknown",
      docsOnly: false,
      runHeavy: true,
      reason: "duplicate-path",
    });
  }
  const releaseMetadataRequired = releaseMetadataRequiredPaths
    .every((path) => uniquePaths.has(path));
  const releaseMetadataCustodyCount = releaseMetadataCustodyPaths
    .filter((path) => uniquePaths.has(path)).length;
  if (releaseMetadataRequired
    && normalizedPaths.every((path) => releaseMetadataAllowedPaths.has(path))
    && (releaseMetadataCustodyCount === 0
      || releaseMetadataCustodyCount === releaseMetadataCustodyPaths.length)) {
    return withLivePostgres({
      pathClass: "release-metadata-only",
      docsOnly: false,
      runHeavy: false,
      reason: "release-metadata-allowlist",
    }, normalizedPaths);
  }

  const classes = new Set();
  for (const p of normalizedPaths) {
    if (p === ".github" || p.startsWith(".github/")
      || p === "security" || p.startsWith("security/")
      || p === "backend/rust-toolchain.toml" || p === "backend/deny.toml"
      || p === "renovate.json5" || p === "release-please-config.json") {
      return withLivePostgres({
        pathClass: "unknown",
        docsOnly: false,
        runHeavy: true,
        reason: "always-full",
      }, normalizedPaths);
    }
    if (p.startsWith("backend/") || p.startsWith("third-party/rust/")) {
      classes.add("backend");
    } else if (p.startsWith("deploy/") || p.startsWith("ops/")) {
      classes.add("deploy");
    } else if (
      p.startsWith("scripts/")
      || p.startsWith("tools/")
      || p === "package.json"
      || p === "package-lock.json"
      || p === "BUCK"
    ) {
      classes.add("scripts-tools");
    } else if (p.startsWith("docs/") || docsOnlyRootFiles.has(p)) {
      classes.add("docs-only");
    } else {
      return withLivePostgres({
        pathClass: "unknown",
        docsOnly: false,
        runHeavy: true,
        reason: "unmapped-path",
      }, normalizedPaths);
    }
  }
  if (classes.size === 1 && classes.has("docs-only")) {
    return withLivePostgres({
      pathClass: "docs-only",
      docsOnly: true,
      runHeavy: false,
      reason: "docs-allowlist",
    }, normalizedPaths);
  }
  if (classes.size === 1) {
    return withLivePostgres({
      pathClass: [...classes][0],
      docsOnly: false,
      runHeavy: true,
      reason: "single-class",
    }, normalizedPaths);
  }
  if (classes.size > 1) {
    return withLivePostgres({
      pathClass: "mixed",
      docsOnly: false,
      runHeavy: true,
      reason: "mixed",
    }, normalizedPaths);
  }
  return withLivePostgres({
    pathClass: "unknown",
    docsOnly: false,
    runHeavy: true,
    reason: "empty-classes",
  }, normalizedPaths);
}

export function listChangedPathsForPathClass(env = process.env, runGit = spawnSync) {
  const event = env.PATH_CLASS_EVENT_NAME || "";
  let range = null;
  if (event === "pull_request") {
    const base = env.PATH_CLASS_PR_BASE_SHA;
    const head = env.PATH_CLASS_PR_HEAD_SHA || env.PATH_CLASS_SHA;
    if (!base || !head) return { ok: false, reason: "missing-pr-shas", paths: [] };
    range = `${base}...${head}`;
  } else if (event === "merge_group") {
    // A merge group is a candidate merge commit, not a PR. GitHub supplies the
    // queued base and head, so the diff is the same shape as a PR's. Without
    // this branch the classifier falls to `unsupported-event`, which is
    // fail-closed (runHeavy=true) and therefore SAFE but runs the full matrix
    // for every queue entry -- which caps queue throughput at the length of the
    // slowest shard.
    const base = env.PATH_CLASS_MERGE_GROUP_BASE_SHA;
    const head = env.PATH_CLASS_MERGE_GROUP_HEAD_SHA || env.PATH_CLASS_SHA;
    if (!base || !head) return { ok: false, reason: "missing-merge-group-shas", paths: [] };
    range = `${base}...${head}`;
  } else if (event === "push") {
    const before = env.PATH_CLASS_PUSH_BEFORE_SHA;
    const sha = env.PATH_CLASS_SHA;
    if (!before || !sha) return { ok: false, reason: "missing-push-shas", paths: [] };
    if (/^0{40}$/.test(before)) return { ok: false, reason: "new-branch", paths: [] };
    range = `${before}...${sha}`;
  } else {
    return { ok: false, reason: "unsupported-event", paths: [] };
  }
  // Include deletions (D). ACMR alone drops product deletes beside docs edits and
  // false-greens docs_only + Path-class skip proofs (console-7rc critic E6).
  // --no-renames: default rename detection emits only the destination path, so a
  // product→docs rename would inventory as docs-only (console-q58y residual).
  const result = runGit(
    "git",
    ["diff", "--name-only", "-z", "--no-renames", "--no-ext-diff", range, "--"],
  );
  if (result.status !== 0) {
    return { ok: false, reason: "git-diff-failed", paths: [] };
  }
  return parseNulDelimitedChangedPaths(result.stdout);
}

export function parseNulDelimitedChangedPaths(output) {
  if (!Buffer.isBuffer(output)) {
    return { ok: false, reason: "malformed-git-diff-output", paths: [] };
  }
  if (output.length === 0) return { ok: true, reason: "ok", paths: [] };
  if (output[output.length - 1] !== 0) {
    return { ok: false, reason: "malformed-git-diff-output", paths: [] };
  }
  const paths = [];
  let start = 0;
  for (let end = output.indexOf(0, start); end !== -1; end = output.indexOf(0, start)) {
    const raw = output.subarray(start, end);
    if (raw.length === 0) {
      return { ok: false, reason: "malformed-git-diff-output", paths: [] };
    }
    const path = raw.toString("utf8");
    if (!Buffer.from(path, "utf8").equals(raw)) {
      return { ok: false, reason: "non-utf8-path", paths: [] };
    }
    paths.push(path);
    start = end + 1;
    if (start === output.length) break;
  }
  if (start !== output.length) {
    return { ok: false, reason: "malformed-git-diff-output", paths: [] };
  }
  return { ok: true, reason: "ok", paths };
}

function applyEventLivePostgres(classified, event) {
  let runLivePostgres = classified.runLivePostgres === true;
  if (!classified.runHeavy) {
    runLivePostgres = false;
  } else if (event === "push" || event === "workflow_dispatch") {
    runLivePostgres = true;
  }
  return { ...classified, runLivePostgres };
}

export function resolvePathClassFromEnv(env = process.env) {
  const listed = listChangedPathsForPathClass(env);
  const event = env.PATH_CLASS_EVENT_NAME || "";
  if (!listed.ok) {
    return applyEventLivePostgres({
      pathClass: "unknown",
      docsOnly: false,
      runHeavy: true,
      runLivePostgres: true,
      reason: listed.reason,
      paths: listed.paths,
      rulesVersion: PATH_CLASS_RULES_VERSION,
    }, event);
  }
  const classified = classifyChangedPaths(listed.paths);
  return applyEventLivePostgres({
    ...classified,
    paths: listed.paths,
    rulesVersion: PATH_CLASS_RULES_VERSION,
  }, event);
}

export function emitPathClassGithubOutput(env = process.env, outputPath = env.GITHUB_OUTPUT) {
  const resolved = resolvePathClassFromEnv(env);
  const lines = [
    `path_class=${resolved.pathClass}`,
    `docs_only=${resolved.docsOnly ? "true" : "false"}`,
    `run_heavy=${resolved.runHeavy ? "true" : "false"}`,
    `run_live_postgres=${resolved.runLivePostgres ? "true" : "false"}`,
    `path_class_reason=${resolved.reason}`,
    `path_class_rules_version=${resolved.rulesVersion}`,
  ];
  if (outputPath) {
    appendFileSync(outputPath, `${lines.join("\n")}\n`);
  }
  return resolved;
}

const pathClassSkipProofScript = [
  "set -euo pipefail",
  "printf 'path-class skip proof: %s not required for class=%s\\n' \"${GITHUB_JOB}\" \"${{ needs.preflight.outputs.path_class }}\"",
];
const pathClassEmitScript = [
  "set -euo pipefail",
  "node scripts/check-ci-preflight.mjs --emit-path-class",
];
const postgresPreflightNonEvaluationScript = [
  "set -euo pipefail",
  "printf 'PostgreSQL reachability not evaluated because preflight result=%s\\n' \"${{ needs.preflight.result }}\"",
];

const buckPostgresEnvironmentTestCommand = "tools/buck/run_test_with_postgres_env.test.sh";
const buckPostgresHarnessTestCommand = "tools/buck/test_needs_postgres.test.sh";
// The domain-unit step is now a multi-line run. What must be asserted is not the exact
// text but that every audit-relevant package added on 2026-07-31 is still named — dropping
// one silently returns its tests to executing nowhere, which is the condition
// check:executed-tests exists to measure and this assertion exists to prevent.
const domainUnitPackages = [
  "console-support-domain",
  "console-payroll-domain",
  "console-payroll-adapter-postgres",
  "console-attendance-application",
  "console-compliance-domain",
  "console-governance-domain",
  "console-platform-audit-chain",
  "console-platform-authz",
  "console-policy-application",
  "console-policy-domain",
  "console-action-inbox-application",
  "console-intelligence-application",
  "console-attendance-domain",
  "console-benefit-application",
  "console-benefit-domain",
  "console-comms-application",
  "console-comms-domain",
  "console-dispatch-application",
  "console-dispatch-domain",
  "console-docs-application",
  "console-docs-domain",
  "console-equipment-domain",
  "console-erp-domain",
  "console-evaluation-application",
  "console-evaluation-domain",
  "console-finance-gl-domain",
  "console-inspection-domain",
  "console-messenger-domain",
  "console-registry-domain",
  "console-workorder-domain",
  "console-identity-domain",
  "console-inbox-application",
  "console-inbox-domain",
  "console-inventory-domain",
  "console-leave-domain",
  "console-logistics-domain",
  "console-messenger-application",
  "console-notices-domain",
  "console-notifications-domain",
  "console-ontology-application",
  "console-ontology-domain",
  "console-orgchange-domain",
  "console-recruiting-application",
  "console-recruiting-domain",
  "console-reporting-application",
  "console-reporting-domain",
  "console-sales-domain",
  "console-support-application",
  "console-todos-domain",
  "console-workflow-domain",
  "console-workorder-application",
  "console-analytics-quant-service",
  "console-attendance-adapter-postgres",
  "console-attendance-rest",
  "console-benefit-rest",
  "console-comms-adapter-imap",
  "console-comms-adapter-mox",
  "console-comms-adapter-smtp",
  "console-comms-credential-cipher",
  "console-comms-mailbox",
  "console-comms-rest",
  "console-compliance-rest",
  "console-consulting-rest",
  "console-dispatch-rest",
  "console-docs-adapter-postgres",
  "console-docs-rest",
  "console-evaluation-adapter-postgres",
  "console-facilities-rest",
  "console-financial-rest",
  "console-gate-dev-auth-absence",
  "console-gate-iac-tier",
  "console-gate-layer-boundary",
  "console-gate-tenant-isolation",
  "console-gate-vendor-lockin",
  "console-identity-rest",
  "console-inventory-adapter-postgres",
  "console-inventory-rest",
  "console-leave-adapter-postgres",
  "console-leave-rest",
  "console-ontology-adapter-postgres",
  "console-ontology-rest",
  "console-payroll-rest",
  "console-payroll-ui",
  "console-platform-email",
  "console-platform-jobs",
  "console-platform-push",
  "console-platform-realtime",
  "console-platform-request-context",
  "console-production-rest",
  "console-reporting-adapter-postgres",
  "console-support-adapter-postgres",
  "console-kernel-core",
  "console-registry-rest",
  "console-reporting-rest",
  "console-workflow-runtime-adapter-postgres",
  "console-workorder-rest",
  "console-financial-domain",
  "console-identity-application",
  "console-integrity",
  "console-platform-auth",
  "console-platform-authz-rest",
  "console-workflow-runtime",
  // The POD contract-agreement tests: pure #[test], no database. They embed
  // backend/openapi/openapi.yaml with include_str! and assert the published
  // evidenceReference schema agrees with validate_evidence_reference, so an
  // enforced rule cannot drift from the published one without this job going red.
  "console-logistics-rest",
  // The canonical-object roster the writer-ownership gate resolves table names
  // through. Pure #[test], no database; its lib binary executed nowhere until
  // the gate was wired.
  "console-ontology-canonical-domain",
  // avb CanonicalPortError port_error_kind pins (company + employment). Pure
  // #[test], no database; lib binaries darkened until wired into domain-unit.
  "console-ontology-canonical-adapter-postgres",
  "console-orgchange-adapter-postgres",
  // The writer-ownership gate's own lib. Cargo and not the Buck2 mutation-suite
  // step: its integration suite reads the real checkout, so the whole crate
  // stays in one job rather than being split across two build systems.
  "console-gate-writer-ownership",
  // o498 BranchScope rewrite pins (ensure_branch_rewrite_in_scope). Pure #[test],
  // no database; lib binary darkened until wired into domain-unit --lib.
  "console-identity-adapter-postgres",
]
// Integration invocations have one authority: the protected workflow. The old array here
// was a hand-transcribed second copy and twice drifted behind ci.yml. Read the exact ordered
// cargo/git surface from the checked-out workflow instead. The step proof digest remains the
// tamper-evident order/token seal; the executed-tests baseline below is the independent
// source-level oracle that prevents a newly derived invocation from naming a phantom binary.
const domainUnitWorkflowSource = readFileSync(
  new URL("../.github/workflows/ci.yml", import.meta.url),
  "utf8",
);
const domainUnitWorkflowCommands = domainUnitWorkflowInvocationTokens(domainUnitWorkflowSource);
const {
  invocations: domainUnitIntegrationInvocations,
  failures: domainUnitInventoryFailures,
} = parseDomainUnitIntegrationInvocations(domainUnitWorkflowCommands);
const domainUnitTestFiles = domainUnitIntegrationInvocations.flatMap(([, tests]) => tests);
const domainCargoPrefix = [
  "cargo",
  "test",
  "--locked",
  "--manifest-path",
  "backend/Cargo.toml",
];
const domainUnitOpenApiGenCommands = [
  [
    "cargo",
    "run",
    "--locked",
    "--manifest-path",
    "backend/Cargo.toml",
    "-p",
    "console-contracts",
    "--bin",
    "console-openapi-gen",
  ],
  ["git", "diff", "--exit-code", "--", "backend/openapi/openapi.yaml"],
];
const domainUnitExpectedCommands = [
  [...domainCargoPrefix, "--lib", ...domainUnitPackages.flatMap((pkg) => ["-p", pkg])],
  // Workspace-wide by contract: a `-p` list here executed 0 doctests for years
  // while 23 real ones -- including 10 `compile_fail` authorization claims --
  // never ran. See the matching note in .github/workflows/ci.yml.
  [...domainCargoPrefix, "--doc", "--workspace"],
  ...domainUnitIntegrationInvocations.flatMap(([pkg, tests]) => {
    const cargoTest = [
      ...domainCargoPrefix,
      "-p",
      pkg,
      ...tests.flatMap((test) => ["--test", test]),
    ];
    // b4z: after the fragment oracle, lock the regen+diff gate so openapi.yaml
    // cannot be hand-edited without regenerating from Fragments.
    if (pkg === "console-todos-rest" && tests.includes("openapi_fragment")) {
      return [cargoTest, ...domainUnitOpenApiGenCommands];
    }
    return [cargoTest];
  }),
];
const domainUnitExecutedTestsBaseline = JSON.parse(readFileSync(
  new URL("../docs/program/executed-tests-baseline.json", import.meta.url),
  "utf8",
));
// S1: facet harness invocations (must appear in workflow). Aggregator no longer runs cargo.
const postgresReachabilityFacetCommands = Object.freeze({
  "postgres-reachability-app":
    "tools/ci/cargo_needs_postgres.sh --workflow-only --shard-id app --num-threads=1 --runner nextest",
  "postgres-reachability-platform":
    "tools/ci/cargo_needs_postgres.sh --workflow-only --shard-id platform --num-threads=1 --runner nextest",
  "postgres-reachability-ontology":
    "tools/ci/cargo_needs_postgres.sh --workflow-only --shard-id ontology --num-threads=1 --runner nextest",
  "postgres-reachability-domain-a":
    "tools/ci/cargo_needs_postgres.sh --workflow-only --shard-id domain-a --num-threads=1 --runner nextest",
  // domain-b is the nextest pilot. The runner is named in the pinned command so
  // a silent revert to cargo -- which would erase the measured speedup while
  // still passing -- shows up here as a contract break.
  "postgres-reachability-domain-b":
    "tools/ci/cargo_needs_postgres.sh --workflow-only --shard-id domain-b --num-threads=1 --runner nextest",
});
// Extra locked SETUP run steps for a facet job, keyed by job name. Every facet
// otherwise carries exactly two run steps (skip proof, then the harness), and
// `requireOnlyLockedRuns` enforces that. domain-b is the cargo-nextest pilot and
// installs the pinned runner first; the install is locked here so it cannot be
// swapped for an unpinned fetch, and so no OTHER facet can quietly gain a step.
// The installer's pinned artifact, asserted against the script itself. The ci.yml
// step digest above only proves the job CALLS the installer; these two constants
// are what make the fetch reproducible, and nothing else in the tree would notice
// if they were replaced with `curl | sh` against a moving "latest" URL.
const nextestInstallerFile = readFileSync(
  new URL("../tools/ci/install_nextest.sh", import.meta.url),
  "utf8",
);
const NEXTEST_PINNED_URL =
  "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.138/cargo-nextest-0.9.138-x86_64-unknown-linux-gnu.tar.gz";
const NEXTEST_PINNED_SHA256 =
  "3793bf0c27607b196f502c39b2108f571de89fcda7586ae6beefa11ee177b216";
const nextestInstallSetupCommands = [
  'tools/ci/install_nextest.sh "${RUNNER_TEMP}/nextest-bin"',
  'echo "${RUNNER_TEMP}/nextest-bin" >> "${GITHUB_PATH}"',
].join("\n");
const postgresReachabilityFacetSetupCommands = Object.freeze({
  "postgres-reachability-app": nextestInstallSetupCommands,
  "postgres-reachability-platform": nextestInstallSetupCommands,
  "postgres-reachability-ontology": nextestInstallSetupCommands,
  "postgres-reachability-domain-a": nextestInstallSetupCommands,
  "postgres-reachability-domain-b": [
    'tools/ci/install_nextest.sh "${RUNNER_TEMP}/nextest-bin"',
    'echo "${RUNNER_TEMP}/nextest-bin" >> "${GITHUB_PATH}"',
  ].join("\n"),
});
const postgresDomainReachabilityAggregatorCommands = [
  'test "${{ needs.postgres-reachability-app.result }}" = success &&',
  'test "${{ needs.postgres-reachability-platform.result }}" = success &&',
  'test "${{ needs.postgres-reachability-ontology.result }}" = success &&',
  'test "${{ needs.postgres-reachability-domain-a.result }}" = success &&',
  'test "${{ needs.postgres-reachability-domain-b.result }}" = success',
];
const companyConformanceCommands = [
  "tools/buck/test_needs_postgres.sh --num-threads=1 \\",
  "//tools/buck:company-conformance-postgres",
];
const postgresWrapperContracts = [
  ["platform-db-feature-catalog-coverage", "//backend/crates/platform/db:console-platform-db-itest-feature_catalog_covers_every_feature"],

  // The remaining 167, 2026-07-31.
  ["app-action-inbox-api-pg", "//backend/app:console-app-itest-action_inbox_api"],
  ["app-attendance-persona-api-pg", "//backend/app:console-app-itest-attendance_persona_api"],
  ["app-audit-api-pg", "//backend/app:console-app-itest-audit_api"],
  ["app-auth-rest-pg", "//backend/app:console-app-itest-auth_rest"],
  ["app-board-ack-api-pg", "//backend/app:console-app-itest-board_ack_api"],
  ["app-cedar-freshness-mint-pg", "//backend/app:console-app-itest-cedar_freshness_mint"],
  ["app-cedar-parity-shadow-pg", "//backend/app:console-app-itest-cedar_parity_shadow"],
  ["app-cedar-shadow-role-manage-pg", "//backend/app:console-app-itest-cedar_shadow_role_manage"],
  ["app-compliance-api-pg", "//backend/app:console-app-itest-compliance_api"],
  ["app-console-kill-switch-pg", "//backend/app:console-app-itest-console_kill_switch"],
  ["app-console-route-telemetry-pg", "//backend/app:console-app-itest-console_route_telemetry"],
  ["app-dev-auth-persona-guard-pg", "//backend/app:console-app-itest-dev_auth_persona_guard"],
  ["app-dispatch-pipeline-api-pg", "//backend/app:console-app-itest-dispatch_pipeline_api"],
  ["app-equipment-3r-api-pg", "//backend/app:console-app-itest-equipment_3r_api"],
  ["app-evaluation-cycle-api-pg", "//backend/app:console-app-itest-evaluation_cycle_api"],
  ["app-field-visit-api-pg", "//backend/app:console-app-itest-field_visit_api"],
  ["app-finance-gl-voucher-sod-pg", "//backend/app:console-app-itest-finance_gl_voucher_sod"],
  ["app-health-readiness-pg", "//backend/app:console-app-itest-health_readiness"],
  ["app-hr-attendance-manager-scope-pg", "//backend/app:console-app-itest-hr_attendance_manager_scope"],
  ["app-hr-attendance-self-read-pg", "//backend/app:console-app-itest-hr_attendance_self_read"],
  ["app-hr-ingest-checklist-gate-pg", "//backend/app:console-app-itest-hr_ingest_checklist_gate"],
  ["app-hr-people-create-api-pg", "//backend/app:console-app-itest-hr_people_create_api"],
  ["app-m2-real-engine-drive-pg", "//backend/app:console-app-itest-m2_real_engine_drive"],
  ["app-maintenance-chain-api-pg", "//backend/app:console-app-itest-maintenance_chain_api"],
  ["app-mobile-api-pg", "//backend/app:console-app-itest-mobile_api"],
  ["app-notif-routing-api-pg", "//backend/app:console-app-itest-notif_routing_api"],
  ["app-notifications-api-pg", "//backend/app:console-app-itest-notifications_api"],
  ["app-object-graph-api-pg", "//backend/app:console-app-itest-object_graph_api"],
  ["app-object-links-api-pg", "//backend/app:console-app-itest-object_links_api"],
  ["app-object-ontology-api-pg", "//backend/app:console-app-itest-object_ontology_api"],
  ["app-object-resolve-api-pg", "//backend/app:console-app-itest-object_resolve_api"],
  ["app-office-versions-pg", "//backend/app:console-app-itest-office_versions"],
  ["app-org-change-api-pg", "//backend/app:console-app-itest-org_change_api"],
  ["app-persona-ess-http-pg", "//backend/app:console-app-itest-persona_ess_http"],
  ["app-platform-onboarding-e2e-pg", "//backend/app:console-app-itest-platform_onboarding_e2e"],
  ["app-purchase-request-collection-api-pg", "//backend/app:console-app-itest-purchase_request_collection_api"],
  ["app-realtime-ws-pg", "//backend/app:console-app-itest-realtime_ws"],
  ["app-recruiting-pipeline-api-pg", "//backend/app:console-app-itest-recruiting_pipeline_api"],
  ["app-registry-api-pg", "//backend/app:console-app-itest-registry_api"],
  ["app-router-layers-pg", "//backend/app:console-app-itest-router_layers"],
  ["app-search-api-pg", "//backend/app:console-app-itest-search_api"],
  ["app-submittable-definitions-api-pg", "//backend/app:console-app-itest-submittable_definitions_api"],
  ["app-tenant-context-e2e-pg", "//backend/app:console-app-itest-tenant_context_e2e"],
  ["app-workbench-native-api-pg", "//backend/app:console-app-itest-workbench_native_api"],
  ["app-workflow-automation-triggers-pg", "//backend/app:console-app-itest-workflow_automation_triggers"],
  ["app-workflow-dynamics-branch-pg", "//backend/app:console-app-itest-workflow_dynamics_branch"],
  ["app-workflow-four-eyes-publish-pg", "//backend/app:console-app-itest-workflow_four_eyes_publish"],
  ["app-workflow-object-context-api-pg", "//backend/app:console-app-itest-workflow_object_context_api"],
  ["app-workflow-object-kind-dynamics-pg", "//backend/app:console-app-itest-workflow_object_kind_dynamics"],
  ["app-workflow-run-read-surface-pg", "//backend/app:console-app-itest-workflow_run_read_surface"],
  ["app-workflow-runtime-finalize-api-pg", "//backend/app:console-app-itest-workflow_runtime_finalize_api"],
  ["app-workflow-runtime-instance-api-pg", "//backend/app:console-app-itest-workflow_runtime_instance_api"],
  ["app-workorder-api-pg", "//backend/app:console-app-itest-workorder_api"],
  ["rls-arming-lib-pg", "//backend/ci/gates/rls-arming:console-gate-rls-arming-unit"],
  ["attendance-adapter-postgres-self-service-pg", "//backend/crates/attendance/adapter-postgres:console-attendance-adapter-postgres-itest-self_service"],
  ["benefit-adapter-postgres-catalog-rls-surfaces-as-runtime-role-pg", "//backend/crates/benefit/adapter-postgres:console-benefit-adapter-postgres-itest-catalog_rls_surfaces_as_runtime_role"],
  ["comms-adapter-postgres-mail-account-rls-surfaces-as-runtime-role-pg", "//backend/crates/comms/adapter-postgres:console-comms-adapter-postgres-itest-mail_account_rls_surfaces_as_runtime_role"],
  ["comms-adapter-postgres-mail-sync-rls-surfaces-as-runtime-role-pg", "//backend/crates/comms/adapter-postgres:console-comms-adapter-postgres-itest-mail_sync_rls_surfaces_as_runtime_role"],
  ["comms-adapter-postgres-send-rate-limit-rls-surfaces-as-runtime-role-pg", "//backend/crates/comms/adapter-postgres:console-comms-adapter-postgres-itest-send_rate_limit_rls_surfaces_as_runtime_role"],
  ["comms-rest-mox-webhook-pg", "//backend/crates/comms/rest:console-comms-rest-itest-mox_webhook"],
  ["comms-rest-readiness-pg", "//backend/crates/comms/rest:console-comms-rest-itest-readiness"],
  ["consulting-rest-audit-atomicity-pg", "//backend/crates/consulting/rest:console-consulting-rest-itest-audit_atomicity"],
  ["dispatch-worker-timer-delivery-pg", "//backend/crates/dispatch/worker:console-dispatch-worker-itest-timer_delivery"],
  ["docs-rest-evidence-rest-rls-surfaces-as-runtime-role-pg", "//backend/crates/docs/rest:console-docs-rest-itest-evidence_rest_rls_surfaces_as_runtime_role"],
  ["finance-gl-adapter-postgres-voucher-rls-and-fsm-as-runtime-role-pg", "//backend/crates/finance-gl/adapter-postgres:console-finance-gl-adapter-postgres-itest-voucher_rls_and_fsm_as_runtime_role"],
  ["financial-adapter-postgres-lifecycle-rls-surfaces-as-runtime-role-pg", "//backend/crates/financial/adapter-postgres:console-financial-adapter-postgres-itest-lifecycle_rls_surfaces_as_runtime_role"],
  ["financial-adapter-postgres-period-lock-blocks-ledger-as-runtime-role-pg", "//backend/crates/financial/adapter-postgres:console-financial-adapter-postgres-itest-period_lock_blocks_ledger_as_runtime_role"],
  ["financial-adapter-postgres-use-cases-pg", "//backend/crates/financial/adapter-postgres:console-financial-adapter-postgres-itest-use_cases"],
  ["financial-rest-purchase-request-list-pg", "//backend/crates/financial/rest:console-financial-rest-itest-purchase_request_list"],
  ["governance-adapter-postgres-approvals-create-as-runtime-role-pg", "//backend/crates/governance/adapter-postgres:console-governance-adapter-postgres-itest-approvals_create_as_runtime_role"],
  ["governance-adapter-postgres-four-eyes-bind-consume-pg", "//backend/crates/governance/adapter-postgres:console-governance-adapter-postgres-itest-four_eyes_bind_consume"],
  ["governance-adapter-postgres-governance-rls-as-runtime-role-pg", "//backend/crates/governance/adapter-postgres:console-governance-adapter-postgres-itest-governance_rls_as_runtime_role"],
  ["identity-adapter-postgres-deactivate-revokes-credentials-pg", "//backend/crates/identity/adapter-postgres:console-identity-adapter-postgres-itest-deactivate_revokes_credentials"],
  ["identity-adapter-postgres-me-workspace-layouts-rls-pg", "//backend/crates/identity/adapter-postgres:console-identity-adapter-postgres-itest-me_workspace_layouts_rls"],
  ["identity-adapter-postgres-region-branch-crud-rls-surfaces-as-runtime-role-pg", "//backend/crates/identity/adapter-postgres:console-identity-adapter-postgres-itest-region_branch_crud_rls_surfaces_as_runtime_role"],
  ["identity-adapter-postgres-subject-authz-versions-freshness-rls-pg", "//backend/crates/identity/adapter-postgres:console-identity-adapter-postgres-itest-subject_authz_versions_freshness_rls"],
  ["inbox-adapter-postgres-inbox-docs-rls-surfaces-as-runtime-role-pg", "//backend/crates/inbox/adapter-postgres:console-inbox-adapter-postgres-itest-inbox_docs_rls_surfaces_as_runtime_role"],
  ["inbox-rest-api-pg", "//backend/crates/inbox/rest:console-inbox-rest-itest-api"],
  ["inspection-adapter-postgres-lifecycle-pg", "//backend/crates/inspection/adapter-postgres:console-inspection-adapter-postgres-itest-lifecycle"],
  ["inspection-adapter-postgres-schedule-window-rls-surfaces-as-runtime-role-pg", "//backend/crates/inspection/adapter-postgres:console-inspection-adapter-postgres-itest-schedule_window_rls_surfaces_as_runtime_role"],
  ["inventory-adapter-postgres-consume-idempotency-concurrency-pg", "//backend/crates/inventory/adapter-postgres:console-inventory-adapter-postgres-itest-consume_idempotency_concurrency"],
  ["leave-adapter-postgres-leave-migration-expand-contract-pg", "//backend/crates/leave/adapter-postgres:console-leave-adapter-postgres-itest-leave_migration_expand_contract"],
  ["leave-adapter-postgres-leave-rls-surfaces-as-runtime-role-pg", "//backend/crates/leave/adapter-postgres:console-leave-adapter-postgres-itest-leave_rls_surfaces_as_runtime_role"],
  ["leave-rest-leave-http-personas-pg", "//backend/crates/leave/rest:console-leave-rest-itest-leave_http_personas"],
  ["messenger-adapter-postgres-parity-tables-rls-as-runtime-role-pg", "//backend/crates/messenger/adapter-postgres:console-messenger-adapter-postgres-itest-parity_tables_rls_as_runtime_role"],
  ["messenger-adapter-postgres-use-cases-pg", "//backend/crates/messenger/adapter-postgres:console-messenger-adapter-postgres-itest-use_cases"],
  ["messenger-rest-api-pg", "//backend/crates/messenger/rest:console-messenger-rest-itest-api"],
  ["notices-adapter-postgres-notices-rls-surfaces-as-runtime-role-pg", "//backend/crates/notices/adapter-postgres:console-notices-adapter-postgres-itest-notices_rls_surfaces_as_runtime_role"],
  ["notices-rest-api-pg", "//backend/crates/notices/rest:console-notices-rest-itest-api"],
  ["notifications-adapter-postgres-notifications-rls-surfaces-as-runtime-role-pg", "//backend/crates/notifications/adapter-postgres:console-notifications-adapter-postgres-itest-notifications_rls_surfaces_as_runtime_role"],
  ["notifications-rest-api-pg", "//backend/crates/notifications/rest:console-notifications-rest-itest-api"],
  ["ontology-adapter-postgres-builtin-catalog-additive-upgrade-as-runtime-role-pg", "//backend/crates/ontology/adapter-postgres:console-ontology-adapter-postgres-itest-builtin_catalog_additive_upgrade_as_runtime_role"],
  ["ontology-adapter-postgres-c-chain-as-runtime-role-pg", "//backend/crates/ontology/adapter-postgres:console-ontology-adapter-postgres-itest-c_chain_as_runtime_role"],
  ["ontology-adapter-postgres-config-object-types-as-runtime-role-pg", "//backend/crates/ontology/adapter-postgres:console-ontology-adapter-postgres-itest-config_object_types_as_runtime_role"],
  ["ontology-adapter-postgres-instances-residual-filter-as-runtime-role-pg", "//backend/crates/ontology/adapter-postgres:console-ontology-adapter-postgres-itest-instances_residual_filter_as_runtime_role"],
  ["ontology-adapter-postgres-instances-rls-surfaces-as-runtime-role-pg", "//backend/crates/ontology/adapter-postgres:console-ontology-adapter-postgres-itest-instances_rls_surfaces_as_runtime_role"],
  ["ontology-adapter-postgres-key-revision-migration-upgrade-pg", "//backend/crates/ontology/adapter-postgres:console-ontology-adapter-postgres-itest-key_revision_migration_upgrade"],
  ["ontology-adapter-postgres-key-write-cas-as-runtime-role-pg", "//backend/crates/ontology/adapter-postgres:console-ontology-adapter-postgres-itest-key_write_cas_as_runtime_role"],
  ["ontology-adapter-postgres-niche-config-object-types-as-runtime-role-pg", "//backend/crates/ontology/adapter-postgres:console-ontology-adapter-postgres-itest-niche_config_object_types_as_runtime_role"],
  ["ontology-adapter-postgres-projected-instances-read-as-runtime-role-pg", "//backend/crates/ontology/adapter-postgres:console-ontology-adapter-postgres-itest-projected_instances_read_as_runtime_role"],
  ["ontology-adapter-postgres-property-derivation-as-runtime-role-pg", "//backend/crates/ontology/adapter-postgres:console-ontology-adapter-postgres-itest-property_derivation_as_runtime_role"],
  ["ontology-adapter-postgres-property-link-sync-as-runtime-role-pg", "//backend/crates/ontology/adapter-postgres:console-ontology-adapter-postgres-itest-property_link_sync_as_runtime_role"],
  ["ontology-adapter-postgres-registry-rls-surfaces-as-runtime-role-pg", "//backend/crates/ontology/adapter-postgres:console-ontology-adapter-postgres-itest-registry_rls_surfaces_as_runtime_role"],
  ["payroll-adapter-postgres-payroll-lifecycle-rls-as-runtime-role-pg", "//backend/crates/payroll/adapter-postgres:console-payroll-adapter-postgres-itest-payroll_lifecycle_rls_as_runtime_role"],
  ["payroll-rest-api-pg", "//backend/crates/payroll/rest:console-payroll-rest-itest-api"],
  ["payroll-rest-payslip-draft-api-pg", "//backend/crates/payroll/rest:console-payroll-rest-itest-payslip_draft_api"],
  ["payroll-rest-run-lifecycle-api-pg", "//backend/crates/payroll/rest:console-payroll-rest-itest-run_lifecycle_api"],
  ["platform-auth-rest-dev-auth-absence-pg", "//backend/crates/platform/auth-rest:console-platform-auth-rest-itest-dev_auth_absence"],
  ["platform-auth-refresh-tokens-pg", "//backend/crates/platform/auth:console-platform-auth-itest-refresh_tokens"],
  ["platform-auth-webauthn-ceremony-pg", "//backend/crates/platform/auth:console-platform-auth-itest-webauthn_ceremony"],
  ["platform-auth-webauthn-ceremony-replay-pg", "//backend/crates/platform/auth:console-platform-auth-itest-webauthn_ceremony_replay"],
  ["platform-authz-rest-cedar-authoring-rls-as-runtime-role-pg", "//backend/crates/platform/authz-rest:console-platform-authz-rest-itest-cedar_authoring_rls_as_runtime_role"],
  ["platform-authz-rest-decision-feed-as-runtime-role-pg", "//backend/crates/platform/authz-rest:console-platform-authz-rest-itest-decision_feed_as_runtime_role"],
  ["platform-authz-policy-pg", "//backend/crates/platform/authz:console-platform-authz-itest-policy"],
  ["platform-db-attendance-console-migration-contract-pg", "//backend/crates/platform/db:console-platform-db-itest-attendance_console_migration_contract"],
  ["platform-db-code-issuance-pg", "//backend/crates/platform/db:console-platform-db-itest-code_issuance"],
  ["platform-db-group-of-one-expand-contract-pg", "//backend/crates/platform/db:console-platform-db-itest-group_of_one_expand_contract"],
  ["platform-db-group-of-one-invariant-pg", "//backend/crates/platform/db:console-platform-db-itest-group_of_one_invariant"],
  ["platform-db-group-resolvers-pg", "//backend/crates/platform/db:console-platform-db-itest-group_resolvers"],
  ["platform-db-lifecycle-maker-checker-pg", "//backend/crates/platform/db:console-platform-db-itest-lifecycle_maker_checker"],
  ["platform-db-m2-flag-on-runtime-drain-pg", "//backend/crates/platform/db:console-platform-db-itest-m2_flag_on_runtime_drain"],
  ["platform-db-period-locks-and-lifecycle-pg", "//backend/crates/platform/db:console-platform-db-itest-period_locks_and_lifecycle"],
  ["platform-db-personal-data-classification-pg", "//backend/crates/platform/db:console-platform-db-itest-personal_data_classification"],
  ["platform-group-lib-pg", "//backend/crates/platform/group:console-platform-group-unit"],
  ["platform-jobs-apalis-adapter-pg", "//backend/crates/platform/jobs:console-platform-jobs-itest-apalis_adapter"],
  ["platform-jobs-apalis-schema-contract-pg", "//backend/crates/platform/jobs:console-platform-jobs-itest-apalis_schema_contract"],
  ["platform-platform-rest-onboard-seeds-config-objects-pg", "//backend/crates/platform/platform-rest:console-platform-rest-itest-onboard_seeds_config_objects"],
  ["platform-platform-rest-ops-dashboard-pg", "//backend/crates/platform/platform-rest:console-platform-rest-itest-ops_dashboard"],
  ["platform-platform-rest-platform-groups-pg", "//backend/crates/platform/platform-rest:console-platform-rest-itest-platform_groups"],
  ["platform-platform-rest-view-as-pg", "//backend/crates/platform/platform-rest:console-platform-rest-itest-view_as"],
  ["platform-provisioning-bootstrap-passkey-pg", "//backend/crates/platform/provisioning:console-platform-provisioning-itest-bootstrap_passkey"],
  ["platform-provisioning-bootstrap-passkey-replay-pg", "//backend/crates/platform/provisioning:console-platform-provisioning-itest-bootstrap_passkey_replay"],
  ["platform-provisioning-onboard-mints-group-of-one-pg", "//backend/crates/platform/provisioning:console-platform-provisioning-itest-onboard_mints_group_of_one"],
  ["platform-provisioning-roster-import-pg", "//backend/crates/platform/provisioning:console-platform-provisioning-itest-roster_import"],
  ["platform-provisioning-self-enroll-handoff-as-runtime-role-pg", "//backend/crates/platform/provisioning:console-platform-provisioning-itest-self_enroll_handoff_as_runtime_role"],
  ["platform-realtime-postgres-bridge-pg", "//backend/crates/platform/realtime:console-platform-realtime-itest-postgres_bridge"],
  ["platform-storage-lib-pg", "//backend/crates/platform/storage:console-platform-storage-unit"],
  ["platform-storage-evidence-processing-rls-surfaces-as-runtime-role-pg", "//backend/crates/platform/storage:console-platform-storage-itest-evidence_processing_rls_surfaces_as_runtime_role"],
  ["policy-adapter-postgres-draft-storage-pg", "//backend/crates/policy/adapter-postgres:console-policy-adapter-postgres-itest-draft_storage"],
  ["registry-adapter-postgres-create-rls-surfaces-as-runtime-role-pg", "//backend/crates/registry/adapter-postgres:console-registry-adapter-postgres-itest-create_rls_surfaces_as_runtime_role"],
  ["registry-adapter-postgres-equipment-list-rls-as-runtime-role-pg", "//backend/crates/registry/adapter-postgres:console-registry-adapter-postgres-itest-equipment_list_rls_as_runtime_role"],
  ["registry-adapter-postgres-equipment-lookup-normalization-rls-as-runtime-role-pg", "//backend/crates/registry/adapter-postgres:console-registry-adapter-postgres-itest-equipment_lookup_normalization_rls_as_runtime_role"],
  ["registry-adapter-postgres-equipment-versioning-as-runtime-role-pg", "//backend/crates/registry/adapter-postgres:console-registry-adapter-postgres-itest-equipment_versioning_as_runtime_role"],
  ["registry-adapter-postgres-master-list-import-pg", "//backend/crates/registry/adapter-postgres:console-registry-adapter-postgres-itest-master_list_import"],
  ["registry-adapter-postgres-master-list-import-rls-as-runtime-role-pg", "//backend/crates/registry/adapter-postgres:console-registry-adapter-postgres-itest-master_list_import_rls_as_runtime_role"],
  ["registry-adapter-postgres-site-address-postal-roundtrip-rls-as-runtime-role-pg", "//backend/crates/registry/adapter-postgres:console-registry-adapter-postgres-itest-site_address_postal_roundtrip_rls_as_runtime_role"],
  ["registry-rest-equipment-admin-pg", "//backend/crates/registry/rest:console-registry-rest-itest-equipment_admin"],
  ["reporting-adapter-postgres-excel-exports-pg", "//backend/crates/reporting/adapter-postgres:console-reporting-adapter-postgres-itest-excel_exports"],
  ["reporting-adapter-postgres-kpi-golden-dataset-pg", "//backend/crates/reporting/adapter-postgres:console-reporting-adapter-postgres-itest-kpi_golden_dataset"],
  ["reporting-adapter-postgres-ops-summary-pg", "//backend/crates/reporting/adapter-postgres:console-reporting-adapter-postgres-itest-ops_summary"],
  ["reporting-adapter-postgres-work-diary-rls-surfaces-as-runtime-role-pg", "//backend/crates/reporting/adapter-postgres:console-reporting-adapter-postgres-itest-work_diary_rls_surfaces_as_runtime_role"],
  ["sales-adapter-postgres-inquiry-rls-surfaces-as-runtime-role-pg", "//backend/crates/sales/adapter-postgres:console-sales-adapter-postgres-itest-inquiry_rls_surfaces_as_runtime_role"],
  ["sales-adapter-postgres-sales-store-pg", "//backend/crates/sales/adapter-postgres:console-sales-adapter-postgres-itest-sales_store"],
  ["sales-rest-public-inquiry-pg", "//backend/crates/sales/rest:console-sales-rest-itest-public_inquiry"],
  ["support-adapter-postgres-assignee-name-join-rls-surfaces-as-runtime-role-pg", "//backend/crates/support/adapter-postgres:console-support-adapter-postgres-itest-assignee_name_join_rls_surfaces_as_runtime_role"],
  ["support-adapter-postgres-create-internal-ticket-rls-surfaces-as-runtime-role-pg", "//backend/crates/support/adapter-postgres:console-support-adapter-postgres-itest-create_internal_ticket_rls_surfaces_as_runtime_role"],
  ["support-adapter-postgres-support-tickets-pg", "//backend/crates/support/adapter-postgres:console-support-adapter-postgres-itest-support_tickets"],
  ["support-rest-lib-pg", "//backend/crates/support/rest:console-support-rest-unit"],
  ["support-rest-authz-pg", "//backend/crates/support/rest:console-support-rest-itest-authz"],
  ["support-rest-intake-pg", "//backend/crates/support/rest:console-support-rest-itest-intake"],
  ["todos-adapter-postgres-todos-rls-surfaces-as-runtime-role-pg", "//backend/crates/todos/adapter-postgres:console-todos-adapter-postgres-itest-todos_rls_surfaces_as_runtime_role"],
  ["workflow-adapter-postgres-notification-bridge-pg", "//backend/crates/workflow/adapter-postgres:console-workflow-runtime-adapter-postgres-itest-notification_bridge"],
  ["workflow-adapter-postgres-payroll-drain-period-lock-pg", "//backend/crates/workflow/adapter-postgres:console-workflow-runtime-adapter-postgres-itest-payroll_drain_period_lock"],
  ["workorder-adapter-postgres-m2-flag-off-parity-pg", "//backend/crates/workorder/adapter-postgres:console-workorder-adapter-postgres-itest-m2_flag_off_parity"],
  ["workorder-adapter-postgres-rls-read-surfaces-as-runtime-role-pg", "//backend/crates/workorder/adapter-postgres:console-workorder-adapter-postgres-itest-rls_read_surfaces_as_runtime_role"],
  ["workorder-adapter-postgres-use-cases-pg", "//backend/crates/workorder/adapter-postgres:console-workorder-adapter-postgres-itest-use_cases"],
  ["workorder-rest-mobile-device-registration-pg", "//backend/crates/workorder/rest:console-workorder-rest-itest-mobile_device_registration"],
  ["workorder-rest-mobile-evidence-pg", "//backend/crates/workorder/rest:console-workorder-rest-itest-mobile_evidence"],
  ["workorder-rest-mobile-sync-pg", "//backend/crates/workorder/rest:console-workorder-rest-itest-mobile_sync"],

  // Tenant-isolation and PII tranche, 2026-07-31.
  ["platform-db-rls-isolation", "//backend/crates/platform/db:console-platform-db-itest-rls_isolation"],
  ["platform-db-rls-rollout-isolation", "//backend/crates/platform/db:console-platform-db-itest-rls_rollout_isolation"],
  ["platform-audit-chain-rls", "//backend/crates/platform/audit-chain:console-platform-audit-chain-itest-audit_chain_rls"],
  ["platform-provisioning-rls-auth-chain", "//backend/crates/platform/provisioning:console-platform-provisioning-itest-rls_auth_chain_as_runtime_role"],
  ["platform-rest-remove-tenant", "//backend/crates/platform/platform-rest:console-platform-rest-itest-remove_tenant"],
  ["compliance-location-consent-status-rls", "//backend/crates/compliance/adapter-postgres:console-compliance-adapter-postgres-itest-location_consent_status_rls_as_runtime_role"],
  ["compliance-location-store", "//backend/crates/compliance/adapter-postgres:console-compliance-adapter-postgres-itest-location_store"],
  ["payroll-rls-surfaces", "//backend/crates/payroll/adapter-postgres:console-payroll-adapter-postgres-itest-payroll_rls_surfaces_as_runtime_role"],

  ["dispatch-p1-postgres", "//backend/crates/dispatch/adapter-postgres:console-dispatch-adapter-postgres-itest-p1_dispatch"],
  ["attendance-cancel-substitution-postgres", "//backend/crates/attendance/adapter-postgres:console-attendance-adapter-postgres-itest-cancel_substitution"],
  ["attendance-concurrency-postgres", "//backend/crates/attendance/adapter-postgres:console-attendance-adapter-postgres-itest-concurrency"],
  ["app-inline-postgres", "//backend/app:console-app-itest-inline-postgres"],
  ["app-dev-auth-persona-guard-postgres", "//backend/app:console-app-itest-dev_auth_persona_guard_feature"],
  ["auth-rest-dev-auth-inline-postgres", "//backend/crates/platform/auth-rest:console-platform-auth-rest-itest-dev-auth-postgres"],
  ["auth-rest-dev-auth-session-postgres", "//backend/crates/platform/auth-rest:console-platform-auth-rest-itest-dev_auth_session"],
  ["auth-rest-dev-auth-group-admin-postgres", "//backend/crates/platform/auth-rest:console-platform-auth-rest-itest-group_admin_tenant_context"],
  ["provisioning-dev-principal-upsert-race-postgres", "//backend/crates/platform/provisioning:console-platform-provisioning-itest-dev_principal_upsert_race"],
  ["app-evaluation-cycle-api-postgres", "//backend/app:console-app-itest-evaluation_cycle_api"],
  ["ontology-builtin-catalog-additive-upgrade-postgres", "//backend/crates/ontology/adapter-postgres:console-ontology-adapter-postgres-itest-builtin_catalog_additive_upgrade_as_runtime_role"],
  ["app-org-change-api-postgres", "//backend/app:console-app-itest-org_change_api"],
  ["app-purchase-request-collection-api-postgres", "//backend/app:console-app-itest-purchase_request_collection_api"],
  ["app-workflow-object-context-api-postgres", "//backend/app:console-app-itest-workflow_object_context_api"],
  ["equipment-3r-http-postgres", "//backend/crates/equipment/rest:console-equipment-rest-itest-equipment_3r_http"],
  ["app-equipment-3r-api-postgres", "//backend/app:console-app-itest-equipment_3r_api"],
  ["ontology-object-type-lifecycle-postgres", "//backend/crates/ontology/rest:console-ontology-rest-itest-object_type_lifecycle_over_http"],
  ["ontology-object-type-cas-postgres", "//backend/crates/ontology/rest:console-ontology-rest-itest-object_type_cas_as_runtime_role"],
  ["ontology-publish-auto-create-action-postgres", "//backend/crates/ontology/rest:console-ontology-rest-itest-publish_auto_create_action_as_runtime_role"],
  ["ontology-object-policy-attach-postgres", "//backend/crates/ontology/rest:console-ontology-rest-itest-object_policy_attach_as_runtime_role"],
  ["ontology-action-execute-postgres", "//backend/crates/ontology/rest:console-ontology-rest-itest-action_execute_as_runtime_role"],
  ["ontology-gaps-postgres", "//backend/crates/ontology/rest:console-ontology-rest-itest-ont_gaps_as_runtime_role"],
  ["ontology-projected-dispatch-postgres", "//backend/crates/ontology/rest:console-ontology-rest-itest-projected_dispatch_as_runtime_role"],
  ["platform-erasure-ledger-postgres", "//backend/crates/platform/erasure-ledger:console-platform-erasure-ledger-itest-erasure_ledger_as_runtime_role"],
];
const postgresWrapperLoader = "run_test_with_postgres_env.sh";
const postgresWrapperLabels = '["test.integration", "resource.postgres", "needs-postgres"]';
const postgresWrapperBuildFile = readFileSync(new URL("../tools/buck/BUCK", import.meta.url), "utf8");
const freeRunnerDiskActionFile = readFileSync(
  new URL("../.github/actions/free-runner-disk/action.yml", import.meta.url),
  "utf8",
);
// GENERATED by tools/buck/gen_first_party.py from the crate's tests/ directory,
// which is what makes the check below a TOTALITY check rather than a second
// hand-kept list: adding a test file adds a target here without anyone deciding to.
const ontologyRestCrateBuildFile = readFileSync(
  new URL("../backend/crates/ontology/rest/BUCK", import.meta.url),
  "utf8",
);
// Always-run preflight commands (every thin path still executes these).
const requiredAlwaysPreflightCommands = [
  releaseMetadataRegressionCommand,
  "npm run check:foundation-gates",
  ciPreflightTestCommand,
  buckImpactPlannerTestCommand,
  consoleRouteInventoryTestCommand,
  "npm run check:ci-preflight",
  "npm run check:package-lock",
  // Locked on arrival. repo-gates taught this repository that a step wired into
  // ci.yml is not thereby protected — deleting `run: npm run check:adrs` from it
  // returned zero preflight failures. Credential literals stay always-on.
  "npm run check:test-credentials",
];
// Heavy preflight commands — gated on steps.path_class.outputs.run_heavy (mechanism B).
const requiredHeavyPreflightCommands = [
  "tools/buck/preflight.sh",
  buckPostgresEnvironmentTestCommand,
  buckPostgresHarnessTestCommand,
  "cargo metadata --manifest-path backend/Cargo.toml --locked --format-version=1 >/dev/null",
  "npm run check:executed-tests",
];
const requiredReleaseMetadataPreflightCommands = [
  "node scripts/check-release-metadata.mjs",
  "node --test scripts/check-doc-links.test.mjs",
  "npm run check:doc-manifest",
  "npm run check:doc-links",
];
const requiredPreflightCommands = [
  ...requiredAlwaysPreflightCommands,
  ...requiredHeavyPreflightCommands,
  ...requiredReleaseMetadataPreflightCommands,
];
const protectedJobs = [
  "backend",
  "repo-gates",
  "api-contract",
  "kubernetes-manifests",
  "generated-face-authority",
  "domain-unit",
  // Facets need preflight and have no job-level if (skip-proof when
  // run_live_postgres is false). Aggregator is fail-closed with if: always()
  // (like required-ci) and is not in this list. backend/domain-unit/
  // company-conformance/generated-face-authority/migration-expand-contract
  // carry a locked postsubmit if. rust-fmt is merge-blocking but parallel
  // with preflight, so it is locked separately (must not need preflight).
  "postgres-reachability-app",
  "postgres-reachability-platform",
  "postgres-reachability-ontology",
  "postgres-reachability-domain-a",
  "postgres-reachability-domain-b",
  "company-conformance",
  "migration-expand-contract",
];

function runContract(kind, name, run, options = {}) {
  return {
    kind,
    name,
    run,
    if: options.if ?? null,
    workingDirectory: options.workingDirectory ?? null,
    shell: options.shell ?? null,
  };
}

function runDigestContract(kind, name, runSha256, options = {}) {
  return {
    kind,
    name,
    runSha256,
    if: options.if ?? null,
    workingDirectory: options.workingDirectory ?? null,
    shell: options.shell ?? null,
  };
}

const setupRun = (name, run, options) => runContract("setup", name, run, options);
const proofRun = (name, run, options) => runContract("proof", name, run, options);
const cleanupRun = (name, run, options) => runContract("cleanup", name, run, options);
const setupDigest = (name, digest, options) => runDigestContract("setup", name, digest, options);
const proofDigest = (name, digest, options) => runDigestContract("proof", name, digest, options);

// This is the complete ordered run-step surface for every current and planned
// required job. Setup prepares a proof but is not itself acceptance evidence;
// proof supplies that evidence, and cleanup is the fail-safe teardown. Multiline
// programs are locked by a digest of js-yaml's exact parsed `run` string so shell
// text cannot be retained behind an early successful exit or changed under a
// familiar name.
const requiredJobRunContracts = Object.freeze({
  "preflight": [
    setupRun("Install workspace dependencies", "npm ci", { if: preflightSetupNodeDependentCondition }),
    setupDigest("Classify path class", "d963a8aa99e66c44a4ed8e3ef25725d206a11973545758d6c67a055b1f48cbbd", { if: preflightNpmCiDependentCondition, shell: "bash" }),
    proofRun("Release metadata semantic regression", releaseMetadataRegressionCommand, { if: preflightNpmCiDependentCondition }),
    proofDigest("Release metadata semantic gate", releaseMetadataGateRunSha256, { if: preflightReleaseMetadataCondition, shell: "bash" }),
    proofRun("Release metadata documentation link tests", "node --test scripts/check-doc-links.test.mjs", { if: preflightReleaseMetadataCondition }),
    proofRun("Release metadata documentation manifest gate", "npm run check:doc-manifest", { if: preflightReleaseMetadataCondition }),
    proofRun("Release metadata documentation local-link gate", "npm run check:doc-links", { if: preflightReleaseMetadataCondition }),
    setupRun("Install pinned DotSlash runtime", "tools/buck/install_dotslash.sh", { if: preflightCheckoutHeavyCondition }),
    proofRun("Cheap Buck2 generated-face admission", "tools/buck/preflight.sh", { if: preflightBuckHeavyCondition }),
    proofRun("Foundation gate contract", "npm run check:foundation-gates", { if: preflightNpmCiDependentCondition }),
    proofRun(reasoningLensRegressionName, reasoningLensRegressionCommand, { if: preflightNpmCiDependentCondition }),
    proofRun(reasoningLensManifestName, reasoningLensManifestCommand, { if: preflightNpmCiDependentCondition }),
    proofRun("CI preflight contract tests", "node --test scripts/check-ci-preflight.test.mjs", { if: preflightNpmCiDependentCondition }),
    proofRun("Buck impact planner regression", buckImpactPlannerTestCommand, { if: preflightNpmCiDependentCondition }),
    proofRun("Console route inventory regression", "node --test scripts/console/route-inventory.test.mjs", { if: preflightNpmCiDependentCondition }),
    proofRun("Console authority-train regression", "node --test scripts/console/verify-console-authority-train.test.mjs", { if: preflightNpmCiDependentCondition }),
    proofRun("Console lane-receipt validator regression", "npm run test:lane-receipt", { if: preflightNpmCiDependentCondition }),
    proofRun("Console PR authority bootstrap regression", consoleBootstrapTestCommand, { if: preflightNpmCiDependentCondition }),
    proofRun("Executed-tests baseline set regression", "npm run test:executed-tests-baseline", { if: preflightNpmCiDependentCondition }),
    proofRun("Local CI mirror contract", "node --test scripts/verify.test.mjs", { if: preflightNpmCiDependentCondition }),
    proofRun("Console truth-ledger validator exact-M regression", "node --test scripts/console/validate-console-truth-ledger.test.mjs", { if: preflightNpmCiDependentCondition }),
    proofRun("Console fanout planner exact-M regression", "node --test scripts/console/plan-fanout.test.mjs", { if: preflightNpmCiDependentCondition }),
    proofRun("Buck PostgreSQL environment wrapper regression", "tools/buck/run_test_with_postgres_env.test.sh", { if: preflightBuckHeavyCondition }),
    proofRun("Buck disposable PostgreSQL harness regression", "tools/buck/test_needs_postgres.test.sh", { if: preflightBuckHeavyCondition }),
    proofRun("CI preflight contract", "npm run check:ci-preflight", { if: preflightNpmCiDependentCondition }),
    proofRun("Canonical npm lockfile", "npm run check:package-lock", { if: preflightNpmCiDependentCondition }),
    proofRun("Cargo.lock consistency", "cargo metadata --manifest-path backend/Cargo.toml --locked --format-version=1 >/dev/null", { if: preflightRustHeavyCondition }),
    proofRun("Executed-tests ratchet — a test binary must have a path from a workflow step", "npm run check:executed-tests", { if: preflightNpmCiHeavyCondition }),
    proofRun("JavaScript test reachability ratchet", "npm run check:js-test-reachability", { if: preflightNpmCiDependentCondition }),
    proofRun("JavaScript test reachability unit tests", "npm run test:js-test-reachability", { if: preflightNpmCiDependentCondition }),
    proofRun("Lane fan-out harness preflight", "node scripts/console/workflows/lane-fanout.test.mjs", { if: preflightNpmCiDependentCondition }),
    proofRun("Workflow test-runner credential literals", "npm run check:test-credentials", { if: preflightNpmCiDependentCondition }),
    proofRun("Collect failures", "node scripts/ci-collect-failures.mjs", { if: collectFailuresCondition }),
  ],
  "domain-unit": [
    proofDigest("Path-class skip proof", "1fdf99dda32af815824808d703216d2c0cf04a0adc146dd29f24746e549c44e0", { if: skipProofCondition, shell: "bash" }),
    proofDigest("Domain crate unit tests", "a8bb301ecb82153acc05c7ee219f85cce5baf95425ec9f8c220fc9912ad35de7", { if: runHeavyCondition }),
  ],
  "backend": [
    proofDigest("Path-class skip proof", "1fdf99dda32af815824808d703216d2c0cf04a0adc146dd29f24746e549c44e0", { if: skipProofCondition, shell: "bash" }),
    setupRun("Install pinned DotSlash runtime", "../tools/buck/install_dotslash.sh", { if: backendIndependentCondition }),
    proofRun("rustfmt check", "cargo fmt --all -- --check", { if: backendLegCondition("cargo") }),
    proofRun("clippy -D warnings", "SQLX_OFFLINE=true cargo clippy --all-targets -- -D warnings", { if: backendLegCondition("cargo") }),
    proofRun("Layer-boundary gate", "../tools/buck2 run //backend/ci/gates/layer-boundary:console-gate-layer-boundary", { if: backendLegCondition("cargo") }),
    proofRun("Audit-coverage gate", "cargo run -p console-gate-audit-coverage", { if: backendLegCondition("cargo") }),
    proofRun("Migration-safety gate", "cargo run -p console-gate-migration-safety", { if: backendLegCondition("cargo") }),
    proofRun("Tenant-isolation gate", "cargo run -p console-gate-tenant-isolation", { if: backendLegCondition("cargo") }),
    proofRun("PII-no-logs gate", "cargo run -p console-gate-pii-no-logs", { if: backendLegCondition("cargo") }),
    proofRun("RLS-arming gate", "cargo run -p console-gate-rls-arming", { if: backendLegCondition("cargo") }),
    proofRun("Dev-auth-absence gate", "cargo run -p console-gate-dev-auth-absence", { if: backendLegCondition("cargo") }),
    proofRun("IaC tier-discipline gate", "cargo run -p console-gate-iac-tier", { if: backendLegCondition("cargo") }),
    proofRun("Fabricated-branch gate", "cargo run -p console-gate-fabricated-branch", { if: backendLegCondition("cargo") }),
    proofRun("Personal-data-classification gate", "cargo run -p console-gate-personal-data-classification", { if: backendLegCondition("cargo") }),
    proofRun("Writer-ownership gate", "cargo run -p console-gate-writer-ownership", { if: backendLegCondition("cargo") }),
    proofDigest("Buck2 CI-gate mutation suites — every gate proven to still reject", "f6614509bd73220754a83d449b8bf422e616309ba48965f730f0d3dcff9d2cf4", { if: backendLegCondition("cargo"), workingDirectory: "." }),
    proofRun("PR 473 migration operational contract tests", "python3 scripts/check-pr473-migration-operational.test.py -v", { if: backendLegCondition("cargo"), workingDirectory: "." }),
    setupDigest("Reconcile portable PostgreSQL role topology", "5da0f2d8c399657dbc0a9d358c81d71399af1ea6c659074a365653db21fcaded", { if: backendIndependentCondition }),
    proofDigest("Boot smoke — migrate + serve + /readyz", "d51d75f8cd49be1557c5b5c1f5f641345bc82f842d2384e9608e9872b0714d79", { if: backendLegTopologyCondition("cargo") }),
    proofDigest("Buck2 dev-auth feature PostgreSQL suites", "f059b50b432f8cafc4e58b14272fe76f5dd3d21842b8683f08c0a5f1f7a84001", { if: backendLegTopologyCondition("buck-dev-auth"), workingDirectory: "." }),
    proofRun("Buck2 platform-authz unit suite", "env -u DATABASE_URL tools/buck2 test //backend/crates/platform/authz:console-platform-authz-unit", { if: backendLegCondition("buck-app"), workingDirectory: "." }),
    proofRun("Cargo console-app unit suite", "env -u DATABASE_URL cargo test --locked --manifest-path backend/Cargo.toml -p console-app --lib", { if: backendLegCondition("buck-app"), workingDirectory: "." }),
    proofRun("Cargo console-app OpenAPI drift suite", "env -u DATABASE_URL cargo test --locked --manifest-path backend/Cargo.toml -p console-app --test openapi_drift", { if: backendLegCondition("buck-app"), workingDirectory: "." }),
    proofRun("Cargo console-app inline PostgreSQL suite", "export DATABASE_URL=\"${CONSOLE_BUCK_ADMIN_DATABASE_URL:?missing test bootstrap URL}\"\ncargo test --locked --manifest-path backend/Cargo.toml -p console-app --lib --features test-postgres -- --test-threads=1\n", { if: backendLegTopologyCondition("buck-app"), workingDirectory: "." }),
    proofRun("Cargo console-app dev-auth PostgreSQL suite", "export DATABASE_URL=\"${CONSOLE_BUCK_ADMIN_DATABASE_URL:?missing test bootstrap URL}\"\ncargo test --locked --manifest-path backend/Cargo.toml -p console-app --test dev_auth_persona_guard_feature --features dev-auth -- --test-threads=1\n", { if: backendLegTopologyCondition("buck-app"), workingDirectory: "." }),
    proofRun("Collect failures", "node scripts/ci-collect-failures.mjs", { if: collectFailuresCondition, workingDirectory: "." }),
  ],
  // Split out of `backend` 2026-08-18: this single gate was 445s of a 1176s job.
  // Contracts are duplicated from `backend` rather than shared because the mirror
  // pins per job; keeping them explicit is what makes a weakened step here fail.
  "migration-expand-contract": [
    proofDigest("Path-class skip proof", "1fdf99dda32af815824808d703216d2c0cf04a0adc146dd29f24746e549c44e0", { if: skipProofCondition, shell: "bash" }),
    setupRun("Install pinned DotSlash runtime", "../tools/buck/install_dotslash.sh", { if: backendIndependentCondition }),
    setupDigest("Reconcile portable PostgreSQL role topology", "5da0f2d8c399657dbc0a9d358c81d71399af1ea6c659074a365653db21fcaded", { if: backendIndependentCondition }),
    proofRun("Expand/contract migration rehearsal (0165, 0166)", "npm run check:pr473-migration-operational", { if: backendTopologyDependentCondition, workingDirectory: "." }),
    proofRun("Collect failures", "node scripts/ci-collect-failures.mjs", { if: collectFailuresCondition, workingDirectory: "." }),
  ],
  "kubernetes-manifests": [
    proofDigest("Path-class skip proof", "1fdf99dda32af815824808d703216d2c0cf04a0adc146dd29f24746e549c44e0", { if: skipProofCondition, shell: "bash" }),
    setupDigest("Install kubectl (for kustomize renderer)", "ed237728d562e10247b2ae17f435525b3b71d94efe5c0c63afa3c73cd16e096b", { if: runHeavyCondition }),
    setupDigest("Install kustomize (NetworkPolicy static render proof)", "4cc1bf875027906f3b8a9878c0f13ce9b1438490390d858088c5ca279e4b9b3c", { if: runHeavyCondition }),
    proofRun("Governed command-database DARK wiring regression", "node --test scripts/check-command-database-wiring.test.mjs", { if: runHeavyCondition }),
    proofRun("Render manifests and NetworkPolicy enforcement preflight", "npm run check:k8s", { if: runHeavyUnlessCancelledCondition }),
    proofRun("Production hardening contract", "npm run check:production-hardening", { if: runHeavyUnlessCancelledCondition }),
    setupRun("Install production-hardening test dependencies", "npm ci --ignore-scripts", { if: runHeavyUnlessCancelledCondition }),
    proofRun("Production hardening regression tests", "npm run test:production-hardening", { if: runHeavyUnlessCancelledCondition }),
  ],
  "repo-gates": [
    setupRun("Install workspace dependencies", "npm ci"),
    proofRun("ADR governance tests", "npm run test:adrs", { if: "${{ !cancelled() }}" }),
    proofRun("ADR governance gate", "npm run check:adrs", { if: "${{ !cancelled() }}" }),
    proofRun("Documentation link tests", "node --test scripts/check-doc-links.test.mjs", { if: "${{ !cancelled() }}" }),
    proofRun("Documentation manifest gate", "npm run check:doc-manifest", { if: "${{ !cancelled() }}" }),
    proofRun("Documentation local-link gate", "npm run check:doc-links", { if: "${{ !cancelled() }}" }),
    proofRun("Doc citations — every code citation must resolve", "npm run check:doc-citations", { if: "${{ !cancelled() }}" }),
    proofRun("Foundation gate contract", "npm run check:foundation-gates", { if: "${{ !cancelled() }}" }),
    proofRun("Canonical npm lockfile", "npm run check:package-lock", { if: "${{ !cancelled() }}" }),
    proofRun("Shared text gate unit tests", "npm run test:text-gate", { if: "${{ !cancelled() }}" }),
    proofRun("Gate-input provenance instrument", "npm run check:gate-input-provenance", { if: "${{ !cancelled() }}" }),
    proofRun("Gate-input provenance unit tests", "npm run test:gate-input-provenance", { if: "${{ !cancelled() }}" }),
    proofRun("G004 identity group org people policy foundation gate", "npm run check:g004-identity-foundation", { if: runHeavyUnlessCancelledCondition }),
    proofRun("G005 workflow approval Work Hub lifecycle gate", "npm run check:g005-workflow-lifecycle", { if: runHeavyUnlessCancelledCondition }),
    proofRun("Workflow runtime spine gate", "npm run check:workflow-runtime-spine", { if: runHeavyUnlessCancelledCondition }),
    proofRun("Workflow runtime M2 strangler dark-landing gate", "npm run check:workflow-runtime-m2-strangler", { if: runHeavyUnlessCancelledCondition }),
    proofRun("Workflow runtime M2 Cedar-guard observe-and-record gate", "npm run check:workflow-runtime-m2-cedar-guards", { if: runHeavyUnlessCancelledCondition }),
    proofRun("Workflow runtime M2 flag-ON runtime gate", "npm run check:workflow-runtime-m2-runtime", { if: runHeavyUnlessCancelledCondition }),
    proofRun("Workflow runtime M2 outbox-drainer transactional-idempotency gate", "npm run check:workflow-runtime-m2-drainer", { if: runHeavyUnlessCancelledCondition }),
    proofRun("G006 asset equipment dispatch lifecycle gate", "npm run check:g006-asset-dispatch-lifecycle", { if: runHeavyUnlessCancelledCondition }),
    proofRun("G007 collaboration mail calendar poll mobile lifecycle gate", "npm run check:g007-collaboration-mobile-lifecycle", { if: runHeavyUnlessCancelledCondition }),
    proofRun("G008 import HR payroll readiness gate", "npm run check:g008-payroll-readiness", { if: runHeavyUnlessCancelledCondition }),
    proofRun("People HR lifecycle maturity gate", "npm run check:people-hr-maturity", { if: runHeavyUnlessCancelledCondition }),
    proofRun("Payroll release-gate contract", "npm run check:payroll-release-gate", { if: runHeavyUnlessCancelledCondition }),
    proofRun("Undeclared imports — every bare specifier must be declared", "npm run check:undeclared-imports", { if: runHeavyUnlessCancelledCondition }),
    proofRun("Request-body contract — spec fields must exist on the handler", "npm run check:request-body-contract", { if: runHeavyUnlessCancelledCondition }),
  ],
  "api-contract": [
    proofDigest("Path-class skip proof", "1fdf99dda32af815824808d703216d2c0cf04a0adc146dd29f24746e549c44e0", { if: skipProofCondition, shell: "bash" }),
    setupRun("Install Node tooling", "npm ci", { if: runHeavyCondition }),
    proofRun("Platform contract drift gate", "npm run check:platform-contract-drift", { if: runHeavyUnlessCancelledCondition }),
    proofRun("Employee import replay contract", "npm run test:employee-import-contract", { if: runHeavyUnlessCancelledCondition }),
    proofRun("Ontology write precondition contract", "npm run test:ontology-write-precondition", { if: runHeavyUnlessCancelledCondition }),
  ],
  "generated-face-authority": [
    proofDigest("Path-class skip proof", "1fdf99dda32af815824808d703216d2c0cf04a0adc146dd29f24746e549c44e0", { if: skipProofCondition, shell: "bash" }),
    setupRun("Install pinned DotSlash runtime", "tools/buck/install_dotslash.sh", { if: runHeavyCondition }),
    setupDigest("Install lock-pinned Reindeer Rust toolchain", "e138ce62e419d3461df6e45108f0cb5a032e966486527b918d504c6ae604ed4e", { if: runHeavyCondition, shell: "bash" }),
    setupRun("Install workspace dependencies", "npm ci", { if: runHeavyCondition }),
    proofRun("Full generated-face closure", "tools/buck/preflight.sh --full-generated-faces", { if: runHeavyCondition }),
  ],
  "company-conformance": [
    proofDigest("Path-class skip proof", "1fdf99dda32af815824808d703216d2c0cf04a0adc146dd29f24746e549c44e0", { if: skipProofCondition, shell: "bash" }),
    setupRun("Install pinned DotSlash runtime", "tools/buck/install_dotslash.sh", { if: runHeavyCondition }),
    proofDigest("Company conformance against disposable PostgreSQL", "f2e478d7571d3dd31977783d4a13deeffd8bb09e045cdb8e3d205528ea6fe3c7", { if: runHeavyCondition }),
  ],
  "postgres-reachability-app": [
    proofDigest("Path-class skip proof", "1fdf99dda32af815824808d703216d2c0cf04a0adc146dd29f24746e549c44e0", { if: skipLivePostgresCondition, shell: "bash" }),
    setupDigest("Install pinned cargo-nextest", "1b9eee0f6292b56cca32c14842d9e03cff331ffcb13efc791e6efc486d5c58bb", { if: runLivePostgresCondition }),
    proofDigest("Run disposable PostgreSQL integration targets", "0644348a698f55fe8ae9c9e657f2594af26b9d218cdfb285a5f370bf110eaad9", { if: runLivePostgresCondition }),
  ],
  "postgres-reachability-platform": [
    proofDigest("Path-class skip proof", "1fdf99dda32af815824808d703216d2c0cf04a0adc146dd29f24746e549c44e0", { if: skipLivePostgresCondition, shell: "bash" }),
    setupDigest("Install pinned cargo-nextest", "1b9eee0f6292b56cca32c14842d9e03cff331ffcb13efc791e6efc486d5c58bb", { if: runLivePostgresCondition }),
    proofDigest("Run disposable PostgreSQL integration targets", "bd7fc913d56d851c24d2ff0ef7324c0821470eededbec21bc6270e4a2ead672a", { if: runLivePostgresCondition }),
  ],
  "postgres-reachability-ontology": [
    proofDigest("Path-class skip proof", "1fdf99dda32af815824808d703216d2c0cf04a0adc146dd29f24746e549c44e0", { if: skipLivePostgresCondition, shell: "bash" }),
    setupDigest("Install pinned cargo-nextest", "1b9eee0f6292b56cca32c14842d9e03cff331ffcb13efc791e6efc486d5c58bb", { if: runLivePostgresCondition }),
    proofDigest("Run disposable PostgreSQL integration targets", "b3ef40b98392e7c736237e81608e8bf70e0bff9a53742abd5baa6f3a910258e1", { if: runLivePostgresCondition }),
  ],
  "postgres-reachability-domain-a": [
    proofDigest("Path-class skip proof", "1fdf99dda32af815824808d703216d2c0cf04a0adc146dd29f24746e549c44e0", { if: skipLivePostgresCondition, shell: "bash" }),
    setupDigest("Install pinned cargo-nextest", "1b9eee0f6292b56cca32c14842d9e03cff331ffcb13efc791e6efc486d5c58bb", { if: runLivePostgresCondition }),
    proofDigest("Run disposable PostgreSQL integration targets", "076a06223fbb66290f9c13c3e07e58c8c736f54e276a0c031ec7ca4647fe6899", { if: runLivePostgresCondition }),
  ],
  "postgres-reachability-domain-b": [
    proofDigest("Path-class skip proof", "1fdf99dda32af815824808d703216d2c0cf04a0adc146dd29f24746e549c44e0", { if: skipLivePostgresCondition, shell: "bash" }),
    // SETUP, not proof: installing the runner prepares the evidence, it is not
    // the evidence. This digest locks the STEP TEXT only -- it says the job calls
    // the installer, nothing about what the installer does. The URL and sha256
    // inside the script are pinned separately by `requireNextestInstallerPin`;
    // without that, this digest would look like supply-chain protection while
    // protecting nothing.
    setupDigest("Install pinned cargo-nextest", "1b9eee0f6292b56cca32c14842d9e03cff331ffcb13efc791e6efc486d5c58bb", { if: runLivePostgresCondition }),
    proofDigest("Run disposable PostgreSQL integration targets", "4ea121588589036833c7f24f19f7cc2cfdd8e146cb4b5749f31d17227f5bf861", { if: runLivePostgresCondition }),
  ],
  "rust-fmt": [
    proofRun("rustfmt check", "cargo fmt --all -- --check"),
  ],
  "postgres-domain-reachability": [
    proofDigest("Preflight failure non-evaluation", "bd3b7317d5a581574bf8ec45e2ad2d44a24ea93159b9322157619d671e6250df", { if: postgresAggregateNonEvaluationCondition, shell: "bash" }),
    proofDigest("Path-class skip proof", "1fdf99dda32af815824808d703216d2c0cf04a0adc146dd29f24746e549c44e0", { if: postgresAggregateSkipCondition, shell: "bash" }),
    proofDigest("Require all PostgreSQL reachability facets", "7f9e079d4f3b5f15d81f9fffdb00ab11b8ca881e1d5842fbe448c5490af1152a", { if: postgresAggregateHeavyCondition }),
  ],
});

function actionStep(index, name, uses, withInputs, options = {}) {
  // Allow `actionStep(i, name, uses, { if, id })` when the action has no `with:`.
  if (
    options
    && Object.keys(options).length === 0
    && withInputs
    && typeof withInputs === "object"
    && !Array.isArray(withInputs)
    && Object.keys(withInputs).every((key) => key === "if" || key === "id")
  ) {
    options = withInputs;
    withInputs = undefined;
  }
  const step = withInputs === undefined
    ? { name, uses }
    : { name, uses, with: withInputs };
  if (options.if != null) step.if = options.if;
  if (options.id != null) step.id = options.id;
  return { index, step };
}

// Action setup is executable too. Pin the full parsed step object, including
// action identity and inputs, and its position among run steps. This prevents a
// skipped checkout/toolchain/cache action from inheriting whatever happens to
// be installed on a hosted runner and presenting that accident as proof.
const requiredJobActionContracts = Object.freeze({
  "preflight": [
    actionStep(0, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", {"persist-credentials":false,"fetch-depth":0}, { id: "checkout" }),
    actionStep(1, "Set up Node.js", "actions/setup-node@48b55a011bda9f5d6aeb4c2d9c7362e8dae4041e", {"node-version":"24.16.0","cache":"npm"}, { if: preflightCheckoutDependentCondition, id: "setup-node" }),
    actionStep(10, "Install Rust toolchain for Cargo.lock consistency", "dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8", {"toolchain":"1.98.1"}, { if: preflightCheckoutHeavyCondition, id: "rust-toolchain" }),
  ],
  "domain-unit": [
    actionStep(1, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", {"persist-credentials":false}, { if: runHeavyCondition }),
    actionStep(2, "Install Rust toolchain (pinned via rust-toolchain.toml)", "dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8", {"toolchain":"1.98.1"}, { if: runHeavyCondition }),
    actionStep(3, "Cache Rust dependencies + build artifacts", "Swatinem/rust-cache@c19371144df3bb44fab255c43d04cbc2ab54d1c4", {"workspaces":"backend","shared-key":"backend-cargo","cache-all-crates":"true","save-if":false}, { if: runHeavyCondition }),
  ],
  "backend": [
    actionStep(0, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", {"persist-credentials":false}, { id: "checkout" }),
    actionStep(3, "Install Rust toolchain (pinned via rust-toolchain.toml)", "dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8", {"toolchain":"1.98.1","components":"rustfmt, clippy"}, { if: backendIndependentCondition, id: "rust" }),
    actionStep(4, "Cache Rust dependencies + build artifacts", "Swatinem/rust-cache@c19371144df3bb44fab255c43d04cbc2ab54d1c4", {"workspaces":"backend","shared-key":"backend-cargo-${{ matrix.os }}","cache-all-crates":"true","save-if":"${{ github.ref == 'refs/heads/dev' && matrix.leg == 'cargo' }}"}, { if: backendIndependentCondition, id: "rust-cache" }),
  ],
  "migration-expand-contract": [
    actionStep(0, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", {"persist-credentials":false}, { id: "checkout" }),
    actionStep(3, "Install Rust toolchain (pinned via rust-toolchain.toml)", "dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8", {"toolchain":"1.98.1","components":"rustfmt, clippy"}, { if: backendIndependentCondition, id: "rust" }),
    actionStep(4, "Cache Rust dependencies + build artifacts", "Swatinem/rust-cache@c19371144df3bb44fab255c43d04cbc2ab54d1c4", {"workspaces":"backend","shared-key":"backend-cargo","cache-all-crates":"true","save-if":false}, { if: backendIndependentCondition, id: "rust-cache" }),
  ],
  "kubernetes-manifests": [
    actionStep(1, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", {"fetch-depth":0}, { if: runHeavyCondition }),
  ],
  "repo-gates": [
    actionStep(0, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0"),
    actionStep(1, "Set up Node.js", "actions/setup-node@48b55a011bda9f5d6aeb4c2d9c7362e8dae4041e", {"node-version":"24.16.0","cache":"npm"}),
  ],
  "api-contract": [
    actionStep(1, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", { if: runHeavyCondition }),
    actionStep(2, "Set up Node.js", "actions/setup-node@48b55a011bda9f5d6aeb4c2d9c7362e8dae4041e", {"node-version":"24","cache":"npm"}, { if: runHeavyCondition }),
  ],
  "generated-face-authority": [
    actionStep(1, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", {"persist-credentials":false}, { if: runHeavyCondition }),
    actionStep(3, "Set up Node.js", "actions/setup-node@48b55a011bda9f5d6aeb4c2d9c7362e8dae4041e", {"node-version":"24.16.0","cache":"npm"}, { if: runHeavyCondition }),
    actionStep(4, "Set up Java", "actions/setup-java@1bcf9fb12cf4aa7d266a90ae39939e61372fe520", {"distribution":"temurin","java-version":"21"}, { if: runHeavyCondition }),
    actionStep(5, "Install Rust toolchain (pinned via rust-toolchain.toml)", "dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8", {"toolchain":"1.98.1"}, { if: runHeavyCondition }),
  ],
  "company-conformance": [
    actionStep(1, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", {"persist-credentials":false}, { if: runHeavyCondition }),
    actionStep(3, "Install Rust toolchain (pinned via rust-toolchain.toml)", "dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8", {"toolchain":"1.98.1"}, { if: runHeavyCondition }),
  ],
  "postgres-reachability-app": [
    actionStep(1, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", {"persist-credentials":false}, { if: runLivePostgresCondition }),
    actionStep(2, "Install Rust toolchain (pinned via rust-toolchain.toml)", "dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8", {"toolchain":"1.98.1"}, { if: runLivePostgresCondition }),
    actionStep(3, "Cache Rust dependencies + build artifacts", "Swatinem/rust-cache@c19371144df3bb44fab255c43d04cbc2ab54d1c4", {"workspaces":"backend","shared-key":"backend-cargo","cache-all-crates":"true","save-if":false}, { if: runLivePostgresCondition }),
  ],
  "postgres-reachability-platform": [
    actionStep(1, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", {"persist-credentials":false}, { if: runLivePostgresCondition }),
    actionStep(2, "Install Rust toolchain (pinned via rust-toolchain.toml)", "dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8", {"toolchain":"1.98.1"}, { if: runLivePostgresCondition }),
    actionStep(3, "Cache Rust dependencies + build artifacts", "Swatinem/rust-cache@c19371144df3bb44fab255c43d04cbc2ab54d1c4", {"workspaces":"backend","shared-key":"backend-cargo","cache-all-crates":"true","save-if":false}, { if: runLivePostgresCondition }),
  ],
  "postgres-reachability-ontology": [
    actionStep(1, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", {"persist-credentials":false}, { if: runLivePostgresCondition }),
    actionStep(2, "Install Rust toolchain (pinned via rust-toolchain.toml)", "dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8", {"toolchain":"1.98.1"}, { if: runLivePostgresCondition }),
    actionStep(3, "Cache Rust dependencies + build artifacts", "Swatinem/rust-cache@c19371144df3bb44fab255c43d04cbc2ab54d1c4", {"workspaces":"backend","shared-key":"backend-cargo","cache-all-crates":"true","save-if":false}, { if: runLivePostgresCondition }),
  ],
  "postgres-reachability-domain-a": [
    actionStep(1, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", {"persist-credentials":false}, { if: runLivePostgresCondition }),
    actionStep(2, "Install Rust toolchain (pinned via rust-toolchain.toml)", "dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8", {"toolchain":"1.98.1"}, { if: runLivePostgresCondition }),
    actionStep(3, "Cache Rust dependencies + build artifacts", "Swatinem/rust-cache@c19371144df3bb44fab255c43d04cbc2ab54d1c4", {"workspaces":"backend","shared-key":"backend-cargo","cache-all-crates":"true","save-if":false}, { if: runLivePostgresCondition }),
  ],
  "postgres-reachability-domain-b": [
    actionStep(1, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", {"persist-credentials":false}, { if: runLivePostgresCondition }),
    actionStep(2, "Install Rust toolchain (pinned via rust-toolchain.toml)", "dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8", {"toolchain":"1.98.1"}, { if: runLivePostgresCondition }),
    actionStep(3, "Cache Rust dependencies + build artifacts", "Swatinem/rust-cache@c19371144df3bb44fab255c43d04cbc2ab54d1c4", {"workspaces":"backend","shared-key":"backend-cargo","cache-all-crates":"true","save-if":false}, { if: runLivePostgresCondition }),
  ],
  "rust-fmt": [
    actionStep(0, "Checkout", "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0", {"persist-credentials":false}),
    actionStep(1, "Install Rust toolchain (pinned via rust-toolchain.toml)", "dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8", {"toolchain":"1.98.1","components":"rustfmt"}),
  ],
  "postgres-domain-reachability": [
  ],
});

// Digest the complete parsed job envelope except `steps`: runner selection,
// needs, timeout, services, environment, defaults, container, strategy and
// permissions are all executable inputs and must change deliberately together.
const requiredJobMetadataSha256 = Object.freeze({
  "preflight": "11fa19135d0ea52d3146d17e341979233e3258d921a05e12e9db417dca2ac7ed",
  "domain-unit": "3d9b04badac173aad3429543cc3424bebc53e79069ad0c97bc2822e57aec379b",
  "backend": "537587fa01cac85002954990d91b38da5b8c77dce0db9852ab3fe0b25ac93655",
  "migration-expand-contract": "c85263d9b8dbf3b26b4cf4eb8707fc712b8bdf28df5480795aec59cb2462ad2e",
  "kubernetes-manifests": "1b215a62dac6d9a3decea6d6912792de3d033986833356b403fb157a15cb8b96",
  "repo-gates": "da8a07f3a19a6f46a5901e6a6d8eac2f7f1c11f52818b7dea25caf362335ee92",
  "api-contract": "101b70d29b1776058160ea23296e707a4f682f5987a9873371cb57180a737d41",
  "generated-face-authority": "23919f88a894f1667c8c82c3584ff7132c85ce31208131384387b407c524cdba",
  "company-conformance": "7adb052ce30381c0c041a53236fd9700b5f2efa732649a7cd9c8f2adcecf9067",
  "rust-fmt": "83e6e2eb742d70daecef162aad28ad8513d8ab156e71485db4f682b0a97a3657",
  "postgres-reachability-app": "34e297e0370053666f34ad3e6e9cd5d8f1bdd357ee05b8800746b0ead20a74af",
  "postgres-reachability-platform": "ade19aead076742c4cf8b8d2b0c4100758c36fce6bfd37cf489630e6f2592539",
  "postgres-reachability-ontology": "d6cba001f0a47c9f1da17ac150c9775cf277b9047d753f243603bc2096dd3a88",
  "postgres-reachability-domain-a": "7e1459918a3bca5822d5ed43274fbffb807a21316cbe665406913b855aa67ec7",
  "postgres-reachability-domain-b": "7b9513acbd48fef08433a26f2dedbee67b6ad017fc6041737fbf9212503ba754",
  "postgres-domain-reachability": "a550cd1d598d606236777ed184ee873c60a3a0e8844401c3ac14a5dc4bf8f074",
});

const workflowExecutionEnvelopeSha256 = "ca24e7bdbd6b02f79d1dea2fc2787835156b669a2ae4643d60290207f72864cf";
const freeRunnerDiskActionSha256 = "1c1a2307321f732c3dcd67e3af2f33a771ce5b81ea814445390b65946b52fc8f";
const exactCiJobIds = Object.freeze([
  "api-contract",
  "backend",
  "company-conformance",
  "domain-unit",
  "generated-face-authority",
  "kubernetes-manifests",
  "migration-expand-contract",
  "postgres-domain-reachability",
  "postgres-reachability-app",
  "postgres-reachability-domain-a",
  "postgres-reachability-domain-b",
  "postgres-reachability-ontology",
  "postgres-reachability-platform",
  "preflight",
  "repo-gates",
  "required-ci",
  "rust-fmt",
]);

const requiredCiAggregator = Object.freeze({
  name: "Required / CI",
  needs: [
    "preflight",
    "rust-fmt",
    "postgres-domain-reachability",
    "repo-gates",
    "api-contract",
    "kubernetes-manifests",
  ],
  if: "${{ always() }}",
  "runs-on": "ubuntu-latest",
  "timeout-minutes": 5,
  steps: [{
    name: "Require every CI proof to succeed",
    run: [
      'test "${{ needs.preflight.result }}" = success &&',
      '  test "${{ needs.rust-fmt.result }}" = success &&',
      '  test "${{ needs.postgres-domain-reachability.result }}" = success &&',
      '  test "${{ needs.repo-gates.result }}" = success &&',
      '  test "${{ needs.api-contract.result }}" = success &&',
      '  test "${{ needs.kubernetes-manifests.result }}" = success\n',
    ].join("\n"),
  }],
});

// Environment and job defaults are executable inputs: BASH_ENV, NODE_OPTIONS,
// PATH and RUSTC_WRAPPER can replace the program a visually exact `run:` line
// actually reaches. Keep the small amount of intentional metadata explicit and
// reject every other job/step injection on the jobs this preflight protects.
const protectedJobExecutionMetadata = {
  preflight: {
    stepEnv: [{
      name: "Classify path class",
      env: {
        PATH_CLASS_EVENT_NAME: "${{ github.event_name }}",
        PATH_CLASS_PR_BASE_SHA: "${{ github.event.pull_request.base.sha }}",
        PATH_CLASS_PR_HEAD_SHA: "${{ github.event.pull_request.head.sha }}",
        PATH_CLASS_PUSH_BEFORE_SHA: "${{ github.event.before }}",
        PATH_CLASS_MERGE_GROUP_BASE_SHA: "${{ github.event.merge_group.base_sha }}",
        PATH_CLASS_MERGE_GROUP_HEAD_SHA: "${{ github.event.merge_group.head_sha }}",
        PATH_CLASS_SHA: "${{ github.sha }}",
      },
    }, {
      name: "Release metadata semantic gate",
      env: releaseMetadataEnvironment,
    }, {
      name: "Collect failures",
      env: { CI_STEPS: "${{ toJSON(steps) }}" },
    }],
  },
  "domain-unit": { env: { CARGO_PROFILE_DEV_DEBUG: "0", CARGO_PROFILE_TEST_DEBUG: "0" } },
  "postgres-reachability-app": { env: { CARGO_PROFILE_DEV_DEBUG: "0", CARGO_PROFILE_TEST_DEBUG: "0" } },
  "postgres-reachability-platform": { env: { CARGO_PROFILE_DEV_DEBUG: "0", CARGO_PROFILE_TEST_DEBUG: "0" } },
  "postgres-reachability-ontology": { env: { CARGO_PROFILE_DEV_DEBUG: "0", CARGO_PROFILE_TEST_DEBUG: "0" } },
  "postgres-reachability-domain-a": { env: { CARGO_PROFILE_DEV_DEBUG: "0", CARGO_PROFILE_TEST_DEBUG: "0" } },
  "postgres-reachability-domain-b": { env: { CARGO_PROFILE_DEV_DEBUG: "0", CARGO_PROFILE_TEST_DEBUG: "0" } },
  "postgres-domain-reachability": {},
  "company-conformance": {},
  "generated-face-authority": {},
  backend: {
    // Path-class skip proof on this job inherits defaults.run.working-directory=backend.
    // On docs-only tips (run_heavy=false) that step runs BEFORE checkout, so GHA fails with
    // "No such file or directory .../backend" unless the skip step sets working-directory: ".".
    // This lane forces run_heavy via a scripts/ touch until that ci.yml override lands.
    env: {
      DATABASE_URL: "postgres://postgres:postgres@localhost:5432/console_ci",
      SQLX_OFFLINE: "true",
      CARGO_INCREMENTAL: "0",
      CARGO_PROFILE_DEV_DEBUG: "0",
      CARGO_PROFILE_TEST_DEBUG: "0",
    },
    defaults: { run: { "working-directory": "backend" } },
    stepEnv: [{
      name: "Collect failures",
      env: { CI_STEPS: "${{ toJSON(steps) }}" },
    }],
  },
  "migration-expand-contract": {
    defaults: { run: { "working-directory": "backend" } },
    stepEnv: [{
      name: "Collect failures",
      env: { CI_STEPS: "${{ toJSON(steps) }}" },
    }],
    env: {
      DATABASE_URL: "postgres://postgres:postgres@localhost:5432/console_ci",
      SQLX_OFFLINE: "true",
      CARGO_INCREMENTAL: "0",
      CARGO_PROFILE_DEV_DEBUG: "0",
      CARGO_PROFILE_TEST_DEBUG: "0",
    },
  },
  "repo-gates": {},
  "api-contract": {},
  "rust-fmt": {
    defaults: { run: { "working-directory": "backend" } },
  },
  "kubernetes-manifests": {
    stepEnv: [{
      name: "Install kubectl (for kustomize renderer)",
      env: { KUBECTL_VERSION: "v1.36.2" },
    }, {
      name: "Install kustomize (NetworkPolicy static render proof)",
      env: {
        KUSTOMIZE_VERSION: "v5.8.1",
        KUSTOMIZE_SHA256: "029a7f0f4e1932c52a0476cf02a0fd855c0bb85694b82c338fc648dcb53a819d",
      },
    }, {
      name: "Render manifests and NetworkPolicy enforcement preflight",
      env: { CONSOLE_NETWORKPOLICY_PREFLIGHT: "warn" },
    }],
  },
};

function jobBlock(workflow, job) {
  const jobs = workflow.slice(workflow.indexOf("jobs:\n") + "jobs:\n".length);
  const match = jobs.match(new RegExp(`^  ${job}:\\n([\\s\\S]*?)(?=^  [A-Za-z0-9_-]+:|(?![\\s\\S]))`, "m"));
  return match?.[1] ?? null;
}

function needsPreflight(block) {
  const value = block.match(/^    needs:\s*(.+)$/m)?.[1]?.trim();
  if (!value) return false;
  if (value.startsWith("[") && value.endsWith("]")) {
    return value.slice(1, -1).split(",").map((job) => job.trim()).includes("preflight");
  }
  return value === "preflight";
}

function stepBlocks(block) {
  const steps = block.match(/^    steps:\n([\s\S]*)$/m)?.[1] ?? "";
  return steps.split(/^      - /m).slice(1);
}

function runScalar(step) {
  return step.match(/^        run: ([^\n]+)$/m)?.[1]?.trim();
}

function isUnconditional(step) {
  return !/^        (?:if|continue-on-error):/m.test(step);
}

function hasJobDefaultShell(block) {
  const defaults = block.match(/^    defaults:\n((?:      [^\n]*(?:\n|$))*)/m)?.[1] ?? "";
  return /^        shell:/m.test(defaults);
}

function hasUnsafeStepShell(block) {
  return [...block.matchAll(/^        shell: ([^\n]+)$/gm)]
    .some(([, shell]) => shell.trim() !== "bash");
}

function requireProtectedExecutionMetadata(workflowModel, failures) {
  if (Object.hasOwn(workflowModel, "env") || Object.hasOwn(workflowModel, "defaults")) {
    failures.push("CI workflow must not define workflow-level env or defaults");
  }

  for (const [jobName, expected] of Object.entries(protectedJobExecutionMetadata)) {
    // Own-property only — YAML job maps inherit Object.prototype.
    const job = workflowModel.jobs && typeof workflowModel.jobs === "object"
      && Object.hasOwn(workflowModel.jobs, jobName)
      ? workflowModel.jobs[jobName]
      : undefined;
    if (!job || typeof job !== "object") continue;

    const expectedEnv = Object.hasOwn(expected, "env") ? expected.env : undefined;
    const expectedDefaults = Object.hasOwn(expected, "defaults") ? expected.defaults : undefined;
    if (!isDeepStrictEqual(job.env, expectedEnv)
      || !isDeepStrictEqual(job.defaults, expectedDefaults)) {
      failures.push(`${jobName} must preserve its exact job env/defaults execution metadata`);
    }

    const actualStepEnv = (Array.isArray(job.steps) ? job.steps : [])
      .filter((step) => step && typeof step === "object" && Object.hasOwn(step, "env"))
      .map((step) => ({ name: step.name, env: step.env }));
    if (!isDeepStrictEqual(actualStepEnv, expected.stepEnv ?? [])) {
      failures.push(`${jobName} must preserve its exact step environment allowlist`);
    }

    const unsafeShell = (Array.isArray(job.steps) ? job.steps : [])
      .some((step) => step
        && typeof step === "object"
        && Object.hasOwn(step, "shell")
        && step.shell !== "bash");
    if (unsafeShell) {
      failures.push(`${jobName} may use only the default shell or canonical shell: bash`);
    }
  }
}

function multilineRunCommands(step) {
  const run = step.match(/^        run: \|\n((?:          [^\n]*(?:\n|$))*)/m)?.[1] ?? "";
  return run
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

function runScript(step) {
  const scalar = runScalar(step);
  if (scalar && scalar !== "|") return scalar;
  const multiline = step.match(/^        run: \|\n((?:          [^\n]*(?:\n|$))*)/m)?.[1];
  return multiline?.replace(/^          /gm, "") ?? "";
}

function stripShellComment(line) {
  let quote = null;
  let escaped = false;
  for (let index = 0; index < line.length; index += 1) {
    const character = line[index];
    if (escaped) {
      escaped = false;
    } else if (character === "\\") {
      escaped = true;
    } else if (quote) {
      if (character === quote) quote = null;
    } else if (character === "'" || character === '"') {
      quote = character;
    } else if (character === "#" && (index === 0 || /\s/.test(line[index - 1]))) {
      return line.slice(0, index);
    }
  }
  return line;
}

function shellCommandTokens(script) {
  const commands = [];
  let command = "";
  for (const physicalLine of script.split(/\r?\n/)) {
    const line = stripShellComment(physicalLine.trim());
    if (!line) continue;
    const continued = /\\\s*$/.test(line);
    command += (command ? " " : "") + (continued ? line.replace(/\\\s*$/, "") : line);
    if (!continued) {
      commands.push(command);
      command = "";
    }
  }
  if (command) commands.push(command + "\\");

  return commands.map((surface) => {
    const tokens = [];
    let token = "";
    let quote = null;
    let escaped = false;
    const flush = () => {
      if (token) tokens.push(token);
      token = "";
    };
    for (let index = 0; index < surface.length; index += 1) {
      const character = surface[index];
      if (escaped) {
        token += character;
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (quote) {
        if (character === quote) quote = null;
        else token += character;
      } else if (character === "'" || character === '"') {
        quote = character;
      } else if (/\s/.test(character)) {
        flush();
      } else if (";|&".includes(character)) {
        flush();
        tokens.push(character);
      } else {
        token += character;
      }
    }
    const malformed = quote !== null || escaped;
    if (escaped) token += "\\";
    flush();
    return { tokens, malformed };
  });
}

function stripShellAssignments(tokens) {
  let index = 0;
  while (index < tokens.length && /^[A-Za-z_][A-Za-z0-9_]*=/.test(tokens[index])) index += 1;
  return tokens.slice(index);
}

function domainUnitWorkflowInvocationTokens(workflow) {
  const block = jobBlock(workflow, "domain-unit");
  if (!block) return [];
  const domainSteps = stepBlocks(block)
    .filter((step) => stepName(step) === "Domain crate unit tests");
  if (domainSteps.length !== 1) return [];
  return shellCommandTokens(runScript(domainSteps[0]))
    .map((command) => ({
      malformed: command.malformed,
      tokens: stripShellAssignments(command.tokens),
    }))
    .filter(({ tokens }) => tokens[0] === "cargo" || tokens[0] === "git")
    .map(({ malformed, tokens }) => malformed
      ? ["<malformed-domain-unit-command>", ...tokens]
      : tokens);
}

function parseDomainUnitIntegrationInvocations(commands) {
  const invocations = [];
  const failures = [];
  const cargoTestPrefix = [
    "cargo",
    "test",
    "--locked",
    "--manifest-path",
    "backend/Cargo.toml",
  ];
  for (const tokens of commands) {
    if (tokens[0] !== "cargo" || tokens[1] !== "test" || !tokens.includes("--test")) continue;
    const prefixMatches = cargoTestPrefix.every((token, index) => tokens[index] === token);
    const suffix = tokens.slice(cargoTestPrefix.length);
    if (!prefixMatches || suffix[0] !== "-p" || !/^[A-Za-z0-9_-]+$/.test(suffix[1] ?? "")) {
      failures.push("domain-unit integration invocations must use the locked one-package Cargo test shape");
      continue;
    }
    const packageName = suffix[1];
    const tests = [];
    let malformed = false;
    for (let index = 2; index < suffix.length; index += 2) {
      if (suffix[index] !== "--test"
        || !/^[A-Za-z0-9_-]+$/.test(suffix[index + 1] ?? "")) {
        malformed = true;
        break;
      }
      tests.push(suffix[index + 1]);
    }
    if (malformed || tests.length === 0 || new Set(tests).size !== tests.length) {
      failures.push(`domain-unit integration invocation for ${packageName} must name unique --test binaries only`);
      continue;
    }
    invocations.push([packageName, tests]);
  }
  return { invocations, failures };
}

const cargoPackageNameCache = new Map();
let regularGitIndexSourcesCache;
const gitEnvironment = gitFixtureEnvironment();

function regularGitIndexSources() {
  if (regularGitIndexSourcesCache !== undefined) return regularGitIndexSourcesCache;
  const tree = spawnSync("git", ["--no-replace-objects", "-C", rootDir(), "write-tree"], {
    cwd: rootDir(),
    encoding: "utf8",
    env: gitEnvironment,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const treeOid = tree.status === 0 ? tree.stdout.trim() : "";
  if (!/^[0-9a-f]{40,64}$/.test(treeOid)) {
    regularGitIndexSourcesCache = null;
    return regularGitIndexSourcesCache;
  }
  const listing = spawnSync("git", ["--no-replace-objects", "-C", rootDir(), "ls-tree", "-r", "-z", "--full-tree", treeOid], {
    cwd: rootDir(),
    encoding: "utf8",
    env: gitEnvironment,
    maxBuffer: 8 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (listing.status !== 0 || typeof listing.stdout !== "string") {
    regularGitIndexSourcesCache = null;
    return regularGitIndexSourcesCache;
  }
  const sources = new Set();
  for (const entry of listing.stdout.split("\0")) {
    if (!entry) continue;
    const separator = entry.indexOf("\t");
    if (separator === -1) continue;
    const metadata = entry.slice(0, separator);
    if (!/^(?:100644|100755) blob [0-9a-f]{40,64}$/.test(metadata)) continue;
    sources.add(entry.slice(separator + 1));
  }
  regularGitIndexSourcesCache = sources;
  return regularGitIndexSourcesCache;
}

function isCanonicalRegularRepositorySource(source) {
  if (typeof source !== "string"
    || posix.isAbsolute(source)
    || source === "."
    || source === ".."
    || source.startsWith("../")
    || source !== posix.normalize(source)
    || !regularGitIndexSources()?.has(source)) {
    return false;
  }
  try {
    const repositoryRoot = realpathSync(rootDir());
    const candidate = resolve(repositoryRoot, ...source.split("/"));
    if (!lstatSync(candidate).isFile()) return false;
    const canonicalSource = relative(repositoryRoot, realpathSync(candidate))
      .split(sep)
      .join("/");
    return canonicalSource === source;
  } catch {
    return false;
  }
}

function cargoPackageNameForTestSource(source) {
  if (!isCanonicalRegularRepositorySource(source)) return null;
  const packageRoot = source.match(/^(backend\/.+)\/tests\/[^/]+\.rs$/)?.[1];
  if (!packageRoot) return null;
  if (cargoPackageNameCache.has(packageRoot)) return cargoPackageNameCache.get(packageRoot);
  let packageName = null;
  try {
    const manifest = readFileSync(resolve(rootDir(), packageRoot, "Cargo.toml"), "utf8");
    const header = /^\[package\]\s*$/m.exec(manifest);
    if (header) {
      const rest = manifest.slice(header.index + header[0].length);
      const nextSection = /^\[/m.exec(rest);
      const packageBody = nextSection ? rest.slice(0, nextSection.index) : rest;
      packageName = packageBody.match(/^name\s*=\s*"([^"]+)"\s*$/m)?.[1] ?? null;
    }
  } catch {
    // Missing or unreadable ownership is a mismatch below, never a reason to skip the check.
  }
  cargoPackageNameCache.set(packageRoot, packageName);
  return packageName;
}

function requireDomainUnitExecutedTestsBaseline(
  invocations,
  baselineDocument,
  failures,
) {
  const baseline = baselineDocument?.test_attribute_baseline;
  if (!baseline || typeof baseline !== "object" || Array.isArray(baseline)) {
    failures.push("docs/program/executed-tests-baseline.json must expose test_attribute_baseline for domain-unit");
    return;
  }
  for (const [packageName, tests] of invocations) {
    for (const test of tests) {
      const sources = Object.entries(baseline)
        .filter(([source, count]) => source.endsWith(`/tests/${test}.rs`)
          && Number.isInteger(count)
          && count > 0
          && cargoPackageNameForTestSource(source) === packageName)
        .map(([source]) => source);
      if (sources.length !== 1) {
        failures.push(`domain-unit integration binary ${packageName} --test ${test} must resolve exactly once through docs/program/executed-tests-baseline.json and its owning Cargo.toml`);
      }
    }
  }
}

const maxExecutablePrefixDepth = 12;
const maxExecutableTokens = 512;
const assignmentToken = /^[A-Za-z_][A-Za-z0-9_]*=/;

function mergeCargoAnalysis(left, right) {
  for (const packageName of right.packages) left.packages.add(packageName);
  left.malformed ||= right.malformed;
  return left;
}

function emptyCargoAnalysis(malformed = false) {
  return { packages: new Set(), malformed };
}

function packageArguments(tokens, index) {
  const packages = new Set();
  for (; index < tokens.length; index += 1) {
    const argument = tokens[index];
    let packageName = null;
    if (argument === "-p" || argument === "--package") {
      packageName = tokens[index + 1];
      index += 1;
    } else if (argument.startsWith("-p=")) {
      packageName = argument.slice(3);
    } else if (argument.startsWith("--package=")) {
      packageName = argument.slice("--package=".length);
    }
    if (!packageName) continue;
    packages.add(packageName);
  }
  return { packages, malformed: false };
}

function cargoTestAnalysis(tokens, depth = 0) {
  if (depth > maxExecutablePrefixDepth || tokens.length > maxExecutableTokens) {
    return emptyCargoAnalysis(true);
  }
  let index = 0;
  while (assignmentToken.test(tokens[index] ?? "")) index += 1;
  if (index === tokens.length) return emptyCargoAnalysis();

  if (tokens[index] === "command") {
    index += 1;
    while (tokens[index]?.startsWith("-")) {
      const option = tokens[index];
      if (option === "--") {
        index += 1;
        break;
      }
      if (option === "-v" || option === "-V") return emptyCargoAnalysis();
      if (option === "-p") {
        index += 1;
        continue;
      }
      return emptyCargoAnalysis(true);
    }
    return index < tokens.length
      ? cargoTestAnalysis(tokens.slice(index), depth + 1)
      : emptyCargoAnalysis(true);
  }

  if (tokens[index] === "env") {
    index += 1;
    while (tokens[index]) {
      const option = tokens[index];
      if (option === "--") {
        index += 1;
        break;
      }
      if (option === "-S" || option === "--split-string") {
        const payload = tokens[index + 1];
        if (!payload || index + 2 !== tokens.length) return emptyCargoAnalysis(true);
        const payloads = shellCommandTokens(payload);
        if (payloads.length === 0) return emptyCargoAnalysis(true);
        return payloads.reduce(
          (analysis, surface) => mergeCargoAnalysis(
            analysis,
            surface.malformed
              ? emptyCargoAnalysis(true)
              : cargoTestAnalysis(surface.tokens, depth + 1),
          ),
          emptyCargoAnalysis(),
        );
      }
      if (["-u", "--unset", "-C", "--chdir"].includes(option)) {
        if (!tokens[index + 1]) return emptyCargoAnalysis(true);
        index += 2;
      } else if (["-i", "--ignore-environment", "-v", "--debug"].includes(option)) {
        index += 1;
      } else if (assignmentToken.test(option)) {
        index += 1;
      } else if (option.startsWith("-")) {
        return emptyCargoAnalysis(true);
      } else {
        break;
      }
    }
    return index < tokens.length
      ? cargoTestAnalysis(tokens.slice(index), depth + 1)
      : emptyCargoAnalysis(true);
  }

  if (tokens[index] !== "cargo") return emptyCargoAnalysis();
  index += 1;
  if (tokens[index] === "test") return packageArguments(tokens, index + 1);
  if (tokens[index] === "nextest" && tokens[index + 1] === "run") {
    return packageArguments(tokens, index + 2);
  }
  return emptyCargoAnalysis();
}

function cargoTestAnalysisInStep(step) {
  const analysis = emptyCargoAnalysis();
  for (const surface of shellCommandTokens(runScript(step))) {
    if (surface.malformed) {
      mergeCargoAnalysis(analysis, emptyCargoAnalysis(true));
      continue;
    }
    let segment = [];
    for (const token of [...surface.tokens, ";"]) {
      if (token === ";" || token === "|" || token === "&") {
        mergeCargoAnalysis(analysis, cargoTestAnalysis(segment));
        segment = [];
      } else {
        segment.push(token);
      }
    }
  }
  return analysis;
}

function requireUnconditionalRun(steps, command, job, failures) {
  const matchingSteps = steps.filter((step) => runScalar(step) === command);
  if (matchingSteps.length === 0) {
    failures.push(`${job} must run ${command}`);
  } else if (matchingSteps.some((step) => !isUnconditional(step))) {
    failures.push(`${job} must run ${command} unconditionally without if or continue-on-error`);
  }
}

function requireRunWithCondition(steps, command, job, expectedIf, failures) {
  const matchingSteps = steps.filter((step) => runScalar(step) === command);
  if (matchingSteps.length === 0) {
    failures.push(`${job} must run ${command}`);
  } else if (matchingSteps.some((step) => !hasOnlyExpectedCondition(step, expectedIf))) {
    failures.push(
      expectedIf == null
        ? `${job} must run ${command} unconditionally without if or continue-on-error`
        : `${job} must run ${command} only when ${expectedIf}`,
    );
  }
}

function requireReasoningLensManifest(steps, failures) {
  // The per-record lens evidence block was retired: it could only verify the
  // JSON's shape, not that any reasoning happened. What remains is the manifest
  // drift check, which is decidable -- two identifier lists either match or
  // they do not.
  for (const [name, command, label] of [
    [reasoningLensRegressionName, reasoningLensRegressionCommand, "regression suite"],
    [reasoningLensManifestName, reasoningLensManifestCommand, "drift check"],
  ]) {
    const byName = steps.filter((step) => stepName(step) === name);
    const byCommand = steps.filter((step) => runScalar(step) === command);
    if (
      byName.length !== 1
      || byCommand.length !== 1
      || byName[0] !== byCommand[0]
      || !hasOnlyExpectedCondition(byName[0] ?? "", preflightNpmCiDependentCondition)
    ) {
      failures.push(`preflight must run the reasoning-lens ${label} once and only after npm ci succeeds`);
    }
  }
}

function requireConsoleRegressionCoverage(workflow, steps, failures) {
  const serializedSteps = JSON.stringify(steps);
  if (steps.some((step) => [
    "Derive exact console C/T/M train",
    "Console truth-ledger exact-M admission",
    "Console fanout planner exact-M admission",
  ].includes(stepName(step)))
    || /CONSOLE_(?:CANDIDATE|AUTHORITY_TIP|SYNTHETIC_MERGE)_SHA/.test(serializedSteps)
    || steps.some((step) => runScript(step).includes("npm run check:console-truth-ledger"))
    || steps.some((step) => /(?:^|\s)node\s+scripts\/console\/plan-fanout\.mjs(?:\s|$)/.test(runScript(step)))) {
    failures.push("preflight must not run live C/T/M admission in general CI");
  }
  for (const command of [consoleAuthorityTrainTestCommand, consoleBootstrapTestCommand, consoleTruthLedgerTestCommand, consoleFanoutPlannerTestCommand]) {
    const matching = steps.filter((step) => runScalar(step) === command);
    if (matching.length !== 1 || !hasOnlyExpectedCondition(matching[0], preflightNpmCiDependentCondition)) failures.push(`preflight must run ${command} unconditionally on pull requests and main`);
  }
  if (workflow.includes("CONSOLE_INTEGRATION_TIP_SHA")) failures.push("preflight must not reference legacy CONSOLE_INTEGRATION_TIP_SHA");
}

function requireReleaseMetadataProofs(steps, failures) {
  const regression = steps
    .map((step, index) => ({ step, index }))
    .filter(({ step }) => stepName(step) === "Release metadata semantic regression");
  if (regression.length !== 1
    || runScalar(regression[0].step) !== releaseMetadataRegressionCommand
    || !hasOnlyExpectedCondition(regression[0].step, preflightNpmCiDependentCondition)) {
    failures.push("preflight must run the release metadata semantic regression after npm ci");
    return;
  }
  const contracts = [
    ["Release metadata semantic gate", null],
    ["Release metadata documentation link tests", "node --test scripts/check-doc-links.test.mjs"],
    ["Release metadata documentation manifest gate", "npm run check:doc-manifest"],
    ["Release metadata documentation local-link gate", "npm run check:doc-links"],
  ];
  let previousIndex = regression[0].index;
  for (const [name, command] of contracts) {
    const matching = steps
      .map((step, index) => ({ step, index }))
      .filter(({ step }) => stepName(step) === name);
    if (matching.length !== 1
      || (command === null
        ? sha256(runScript(matching[0]?.step ?? "")) !== releaseMetadataGateRunSha256
        : runScalar(matching[0]?.step ?? "") !== command)
      || !hasOnlyExpectedCondition(matching[0]?.step ?? "", preflightReleaseMetadataCondition)
      || matching[0].index <= previousIndex) {
      failures.push("preflight must preserve release-metadata-only semantic, custody, and link proofs");
      return;
    }
    previousIndex = matching[0].index;
  }
}


function rootDir() {
  return resolve(dirname(fileURLToPath(import.meta.url)), "..");
}

function cargoPostgresWorkflowMapNames() {
  try {
    const map = JSON.parse(readFileSync(resolve(rootDir(), "tools/ci/postgres-cargo-map.json"), "utf8"));
    return new Set((map.entries ?? []).filter((e) => e.in_workflow_postgres_job).map((e) => e.name));
  } catch {
    return new Set();
  }
}

function requireCargoPostgresMapCoversWorkflow(failures) {
  const mapPath = "tools/ci/postgres-cargo-map.json";
  let map;
  try {
    map = JSON.parse(readFileSync(resolve(rootDir(), mapPath), "utf8"));
  } catch (error) {
    failures.push(`${mapPath} must exist and parse as JSON (${error.message})`);
    return;
  }
  const workflowEntries = (map.entries ?? []).filter((e) => e.in_workflow_postgres_job);
  if (workflowEntries.length < 180) {
    failures.push(`${mapPath} must list >=180 in_workflow_postgres_job entries (found ${workflowEntries.length})`);
  }
  for (const entry of workflowEntries) {
    if (!Array.isArray(entry.cargo_argv) || entry.cargo_argv[0] !== "cargo" || entry.cargo_argv[1] !== "test") {
      failures.push(`${mapPath} entry ${entry.name ?? "?"} must have cargo test argv`);
    }
    if (!entry.package) failures.push(`${mapPath} entry ${entry.name ?? "?"} missing package`);
  }
  const workflow = readFileSync(resolve(rootDir(), ".github/workflows/ci.yml"), "utf8");
  for (const [job, cmd] of Object.entries(postgresReachabilityFacetCommands)) {
    if (!workflow.includes(cmd)) {
      failures.push(`${job} must invoke ${cmd}`);
    }
  }
  if (!workflow.includes("Dispatch, attendance and ontology — disposable PostgreSQL reachability")) {
    failures.push("load-bearing PostgreSQL reachability display name must remain in ci.yml");
  }
}

function requirePostgresWrapperContracts(buildFile, failures) {
  for (const [name, binary] of postgresWrapperContracts) {
    const block = buildFile.match(new RegExp(`sh_test\\(\\n    name = "${name}",[\\s\\S]*?\\n\\)`, "m"))?.[0];
    if (!block) {
      failures.push(`tools/buck/BUCK must define PostgreSQL wrapper ${name}`);
      continue;
    }
    const expectedArgs = `args = ["$(location ${binary})"],`;
    const expectedDeps = `deps = ["${binary}"],`;
    const hasExactAttribute = (attribute) => (
      [...block.matchAll(new RegExp(`^    ${attribute.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`, "gm"))].length === 1
    );
    const hasExactlyOneField = (field) => (
      [...block.matchAll(new RegExp(`^    ${field} = .+$`, "gm"))].length === 1
    );
    if (!hasExactAttribute(`test = "${postgresWrapperLoader}",`)
      || !hasExactAttribute(expectedArgs)
      || !hasExactAttribute(expectedDeps)
      || !hasExactAttribute(`labels = ${postgresWrapperLabels},`)
      || !["test", "args", "deps", "labels"].every(hasExactlyOneField)) {
      failures.push(`tools/buck/BUCK must bind PostgreSQL wrapper ${name} to the loader and exact Rust binary`);
    }
  }
}

/**
 * TOTALITY over the crate that decides row visibility: every integration test it
 * has must be wrapped in exactly one PostgreSQL `sh_test` and that wrapper must
 * be named in ci.yml.
 *
 * The lists above are per-target contracts — they check that the entries someone
 * REMEMBERED to add are wired correctly, and say nothing about the ones nobody
 * added. That is the four-coupled-places defect this repo has shipped seven
 * times, most recently to `action_execute`, `ont_gaps` and `projected_dispatch`,
 * all three of which had a `rust_test` target and appeared in no wrapper, no
 * ci.yml step and neither locked list. Two of them were broken for the whole
 * lane that introduced the break, and CI could not see it.
 *
 * Deliberately scoped to this ONE crate and driven off its GENERATED BUCK file
 * rather than a repo-wide sweep. The repo-wide version is not blocked by a lack
 * of signal — `TEST_RESOURCE_REQUIREMENTS` in tools/buck/gen_first_party.py
 * already declares `postgres` per test file, and gen_first_party FAILS on any
 * test file that has no declaration. It is blocked by scale: of the 188
 * postgres-declared integration tests in this repo, 163 have no `sh_test`
 * wrapper (measured 2026-07-29). Turning that into a gate today makes CI
 * permanently red, and a gate nobody can satisfy gets deleted. Widening this is
 * a per-crate decision with the same shape as this one, not a cleverer regex.
 */
function requireOntologyRestItestReachability(buildFile, workflow, failures) {
  const itests = [
    ...ontologyRestCrateBuildFile.matchAll(/^    name = "(console-ontology-rest-itest-[a-z0-9_]+)",$/gm),
  ].map(([, name]) => name);
  if (itests.length === 0) {
    failures.push("backend/crates/ontology/rest/BUCK must declare integration-test targets");
    return;
  }
  const wrappers = [...buildFile.matchAll(/sh_test\(\n([\s\S]*?)\n\)/g)].map(([, body]) => ({
    name: body.match(/^    name = "([^"]+)",$/m)?.[1],
    target: body.match(/^    args = \["\$\(location ([^)]+)\)"\],$/m)?.[1],
  }));
  for (const itest of itests) {
    const target = `//backend/crates/ontology/rest:${itest}`;
    const matching = wrappers.filter((wrapper) => wrapper.target === target);
    if (matching.length !== 1 || !matching[0].name) {
      failures.push(`tools/buck/BUCK must wrap ${itest} in exactly one PostgreSQL sh_test`);
      continue;
    }
    const mapNames = cargoPostgresWorkflowMapNames();
    const namedInWorkflow = new RegExp(`^\\s+//tools/buck:${matching[0].name}( \\\\)?$`, "m").test(workflow);
    const namedInCargoMap = mapNames.has(matching[0].name);
    if (!namedInWorkflow && !namedInCargoMap) {
      failures.push(`ci.yml or tools/ci/postgres-cargo-map.json must execute ${matching[0].name} or ${itest} runs nowhere`);
    }
  }
}

function requireUnconditionalMultilineRun(steps, commands, job, failures) {
  const matchingSteps = steps.filter((step) => multilineRunCommands(step).join("\n") === commands.join("\n"));
  if (matchingSteps.length === 0) {
    failures.push(`${job} must run the locked PostgreSQL reachability targets`);
  } else if (matchingSteps.some((step) => !hasOnlyExpectedCondition(step, runLivePostgresCondition))) {
    failures.push(`${job} must run the locked PostgreSQL reachability targets only when run_live_postgres`);
  }
}

function runCommand(step) {
  const scalar = runScalar(step);
  return scalar && scalar !== "|" ? scalar : (multilineRunCommands(step).join("\n") || null);
}

function requireOnlyLockedRuns(steps, commands, job, failures) {
  const actual = steps.map(runCommand).filter(Boolean);
  if (actual.length !== commands.length || actual.some((command, index) => command !== commands[index])) {
    failures.push(`${job} must contain only the locked ordered run steps`);
  }
}

function requirePostgresAggregateStateMachine(steps, failures) {
  const contracts = [
    {
      name: "Preflight failure non-evaluation",
      run: postgresPreflightNonEvaluationScript.join("\n"),
      if: postgresAggregateNonEvaluationCondition,
      shell: "bash",
    },
    {
      name: "Path-class skip proof",
      run: pathClassSkipProofScript.join("\n"),
      if: postgresAggregateSkipCondition,
      shell: "bash",
    },
    {
      name: "Require all PostgreSQL reachability facets",
      run: postgresDomainReachabilityAggregatorCommands.join("\n"),
      if: postgresAggregateHeavyCondition,
      shell: null,
    },
  ];
  const exact = steps.length === contracts.length && contracts.every((contract, index) => {
    const step = steps[index] ?? "";
    const shell = step.match(/^        shell: ([^\n]+)$/m)?.[1] ?? null;
    return stepName(step) === contract.name
      && runCommand(step) === contract.run
      && hasOnlyExpectedCondition(step, contract.if)
      && shell === contract.shell;
  });
  if (!exact) {
    failures.push("PostgreSQL aggregate must distinguish preflight failure, thin skip, and heavy facets");
  }
}

function stepName(step) {
  return step.match(/^name: ([^\n]+)$/m)?.[1] ?? null;
}

function stepWorkingDirectory(step) {
  return step.match(/^        working-directory: ([^\n]+)$/m)?.[1]?.trim() ?? null;
}

function hasOnlyExpectedCondition(step, expectedIf) {
  const conditions = [...step.matchAll(/^        if: ([^\n]+)$/gm)].map((match) => match[1]);
  return (expectedIf === null
    ? conditions.length === 0
    : conditions.length === 1 && conditions[0] === expectedIf)
    && !/^        continue-on-error:/m.test(step);
}

function requireOrderedStepContracts(steps, contracts, job, failures) {
  const indexes = [];
  for (const contract of contracts) {
    const matches = steps
      .map((step, index) => ({ step, index }))
      .filter(({ step }) => stepName(step) === contract.name);
    if (matches.length !== 1
      || (contract.run !== undefined && runCommand(matches[0]?.step) !== contract.run)
      || (contract.workingDirectory !== undefined
        && stepWorkingDirectory(matches[0]?.step ?? "") !== contract.workingDirectory)
      || !hasOnlyExpectedCondition(matches[0]?.step ?? "", contract.if)) {
      failures.push(`${job} must preserve the locked fail-fast step multiset and failure semantics`);
      indexes.push(-1);
    } else {
      indexes.push(matches[0].index);
    }
  }
  if (indexes.some((index) => index < 0) || indexes.some((index, position) => position > 0 && index <= indexes[position - 1])) {
    failures.push(`${job} must preserve the locked fail-fast step order`);
  }
  return indexes;
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function stableSerialize(value) {
  if (Array.isArray(value)) return `[${value.map(stableSerialize).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => (
      `${JSON.stringify(key)}:${stableSerialize(value[key])}`
    )).join(",")}}`;
  }
  return JSON.stringify(value);
}

function hasExactOptionalProperty(value, property, expected) {
  return expected === null
    ? !Object.hasOwn(value, property)
    : Object.hasOwn(value, property) && value[property] === expected;
}

function requireExactRequiredJobContracts(workflowModel, failures) {
  const workflowEnvelope = Object.fromEntries(
    Object.entries(workflowModel).filter(([property]) => property !== "jobs"),
  );
  if (sha256(stableSerialize(workflowEnvelope)) !== workflowExecutionEnvelopeSha256) {
    failures.push("CI workflow must preserve its exact trigger, permission, and concurrency execution envelope");
  }
  const actualJobIds = Object.keys(workflowModel.jobs ?? {}).sort();
  if (!isDeepStrictEqual(actualJobIds, exactCiJobIds)) {
    failures.push("CI workflow must preserve its exact job-id set");
  }
  if (!isDeepStrictEqual(workflowModel.jobs?.["required-ci"], requiredCiAggregator)) {
    failures.push("Required / CI must preserve its exact presubmit success aggregation contract");
  }

  for (const [jobName, contracts] of Object.entries(requiredJobRunContracts)) {
    const job = workflowModel.jobs && typeof workflowModel.jobs === "object"
      && Object.hasOwn(workflowModel.jobs, jobName)
      ? workflowModel.jobs[jobName]
      : undefined;
    if (!job || !Array.isArray(job.steps)) {
      failures.push(`${jobName} must define its complete ordered setup/proof/cleanup run contract`);
      continue;
    }

    const actionContracts = requiredJobActionContracts[jobName] ?? [];
    const jobMetadata = Object.fromEntries(
      Object.entries(job).filter(([property]) => property !== "steps"),
    );
    if (sha256(stableSerialize(jobMetadata)) !== requiredJobMetadataSha256[jobName]) {
      failures.push(`${jobName} must preserve its exact job execution envelope`);
    }
    if (job.steps.length !== contracts.length + actionContracts.length) {
      failures.push(
        `${jobName} must contain only its locked ordered action, setup, proof, and cleanup steps`,
      );
    }

    const actionSteps = job.steps.filter((step) => step && typeof step.uses === "string");
    if (actionSteps.length !== actionContracts.length) {
      failures.push(
        `${jobName} must preserve all ${actionContracts.length} ordered setup action steps; found ${actionSteps.length}`,
      );
    }
    for (const { index, step: expectedStep } of actionContracts) {
      if (!isDeepStrictEqual(job.steps[index], expectedStep)) {
        failures.push(
          `${jobName} setup action step ${index + 1} must preserve its exact name, identity, inputs, position, and execution semantics`,
        );
      }
    }

    const runSteps = job.steps.filter((step) => step && typeof step.run === "string");
    if (runSteps.length !== contracts.length) {
      failures.push(
        `${jobName} must preserve all ${contracts.length} ordered setup/proof run steps; found ${runSteps.length}`,
      );
    }

    for (let index = 0; index < Math.max(runSteps.length, contracts.length); index += 1) {
      const step = runSteps[index];
      const contract = contracts[index];
      if (!step || !contract) continue;

      const exactRun = Object.hasOwn(contract, "run")
        ? step.run === contract.run
        : sha256(step.run) === contract.runSha256;
      const exactExecutionMetadata = hasExactOptionalProperty(step, "if", contract.if)
        && hasExactOptionalProperty(step, "working-directory", contract.workingDirectory)
        && hasExactOptionalProperty(step, "shell", contract.shell)
        && !Object.hasOwn(step, "continue-on-error")
        && !Object.hasOwn(step, "timeout-minutes")
        && !Object.hasOwn(step, "uses")
        && !Object.hasOwn(step, "with");
      if (step.name !== contract.name || !exactRun || !exactExecutionMetadata) {
        failures.push(
          `${jobName} ${contract.kind} run step ${index + 1} must preserve its exact name, command, condition, and execution semantics`,
        );
      }
    }
  }
}

// The Swatinem/rust-cache step was REMOVED from this list on 2026-07-31 because Buck2
// never writes backend/target, so the job restored and saved a cache it could not use.
// The Buck2 app build itself was removed later: the only thing it fed was an app-boot
// comparison of the served /openapi/openapi.yaml against the file on disk, which is
// tautological because backend/app/src/lib.rs:214 include_str!s that exact file. What
// remains in this job is text-only, so it needs neither Rust nor a built binary.
const apiContractAllowedSteps = [
  "name: Path-class skip proof\n        if: ${{ needs.preflight.outputs.run_heavy != 'true' }}\n        shell: bash\n        run: |\n          set -euo pipefail\n          printf 'path-class skip proof: %s not required for class=%s\\n' \"${GITHUB_JOB}\" \"${{ needs.preflight.outputs.path_class }}\"",
  "name: Checkout\n        if: ${{ needs.preflight.outputs.run_heavy == 'true' }}\n        uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7",
  "name: Set up Node.js\n        if: ${{ needs.preflight.outputs.run_heavy == 'true' }}\n        uses: actions/setup-node@48b55a011bda9f5d6aeb4c2d9c7362e8dae4041e # v6.4.0\n        with:\n          node-version: \"24\"\n          cache: npm",
  "name: Install Node tooling\n        if: ${{ needs.preflight.outputs.run_heavy == 'true' }}\n        run: npm ci",
  "name: Platform contract drift gate\n        if: ${{ !cancelled() && needs.preflight.outputs.run_heavy == 'true' }}\n        run: npm run check:platform-contract-drift",
  "name: Employee import replay contract\n        if: ${{ !cancelled() && needs.preflight.outputs.run_heavy == 'true' }}\n        run: npm run test:employee-import-contract",
  "name: Ontology write precondition contract\n        if: ${{ !cancelled() && needs.preflight.outputs.run_heavy == 'true' }}\n        run: npm run test:ontology-write-precondition",
];
function hasOnlyAllowedApiContractSteps(steps) {
  return steps.length === apiContractAllowedSteps.length
    && steps.every((step, index) => step.trimEnd() === apiContractAllowedSteps[index]);
}

function requireReindeerToolchainBefore(steps, command, failures) {
  const commandIndex = steps.findIndex((step) => runScalar(step) === command);
  const toolchainIndex = steps.findIndex((step) => multilineRunCommands(step).includes(reindeerToolchainInstall));
  if (toolchainIndex < 0) {
    failures.push("generated-face-authority must install the lock-pinned Reindeer Rust toolchain before full generated-face closure");
    return;
  }
  const commands = multilineRunCommands(steps[toolchainIndex]);
  const strictModeIndex = commands.indexOf(strictShellMode);
  const sourceIndex = commands.indexOf(reindeerToolchainSource);
  const installIndex = commands.indexOf(reindeerToolchainInstall);
  if (sourceIndex < 0) {
    failures.push(`generated-face-authority must source ${reindeerToolchainLock} before installing the Reindeer Rust toolchain`);
  } else if (strictModeIndex < 0 || strictModeIndex > sourceIndex) {
    failures.push(`generated-face-authority must enable strict shell mode before sourcing ${reindeerToolchainLock}`);
  } else if (sourceIndex > installIndex) {
    failures.push(`generated-face-authority must source ${reindeerToolchainLock} before installing the Reindeer Rust toolchain`);
  }
  if (commands.some((entry) => reindeerToolchainOverride.test(entry))) {
    failures.push(`generated-face-authority must not override REINDEER_TOOLCHAIN after sourcing ${reindeerToolchainLock}`);
  }
  if (!hasOnlyExpectedCondition(steps[toolchainIndex], runHeavyCondition)) {
    failures.push("generated-face-authority must install the Reindeer Rust toolchain only when run_heavy");
  }
  if (commandIndex >= 0 && toolchainIndex > commandIndex) {
    failures.push("generated-face-authority must install the lock-pinned Reindeer Rust toolchain before full generated-face closure");
  }
}

function requirePreflightRustToolchainBefore(steps, failures) {
  const setupIndex = steps.findIndex((step) =>
    step.startsWith("name: Install Rust toolchain for Cargo.lock consistency\n"),
  );
  if (setupIndex < 0) {
    failures.push("preflight must install the pinned Rust toolchain before Cargo-dependent CI preflight tests");
    return;
  }
  if (!hasOnlyExpectedCondition(steps[setupIndex], preflightCheckoutHeavyCondition)) {
    failures.push("preflight must install the pinned Rust toolchain only when run_heavy");
    return;
  }
  const cargoMeta = "cargo metadata --manifest-path backend/Cargo.toml --locked --format-version=1 >/dev/null";
  const commandIndex = steps.findIndex((step) => runScalar(step) === cargoMeta);
  if (commandIndex >= 0 && setupIndex > commandIndex) {
    failures.push(`preflight must install the pinned Rust toolchain before ${cargoMeta}`);
  }
}

function requireDotSlashBefore(steps, command, job, failures) {
  const commandIndex = steps.findIndex((step) => runScalar(step) === command);
  const dotSlashIndex = steps.findIndex((step) => runScalar(step) === dotSlashBootstrap);
  const expectedIf = job === "preflight" ? preflightCheckoutHeavyCondition : runHeavyCondition;
  if (dotSlashIndex < 0) {
    failures.push(`${job} must install pinned DotSlash before Buck2`);
  } else if (!hasOnlyExpectedCondition(steps[dotSlashIndex], expectedIf)) {
    failures.push(`${job} must install DotSlash only when run_heavy`);
  } else if (commandIndex >= 0 && dotSlashIndex > commandIndex) {
    failures.push(`${job} must install DotSlash before ${command}`);
  }
}

function requireEffectiveDotSlashBootstrap(block, job, failures) {
  const workingDirectory = block.match(/^    defaults:\n      run:\n        working-directory: ([^\n]+)$/m)?.[1]?.trim();
  const bootstrap = workingDirectory === "backend" ? `../${dotSlashBootstrap}` : dotSlashBootstrap;
  const steps = stepBlocks(block);
  const bootstrapIndex = steps.findIndex((step) => runScalar(step) === bootstrap);
  if (bootstrapIndex < 0) {
    failures.push(`${job} must install pinned DotSlash from ${bootstrap}`);
    return;
  }
  const firstBuckInvocation = steps.findIndex((step, index) => {
    const run = runScalar(step);
    const command = run === "|" ? multilineRunCommands(step).join("\n") : run ?? "";
    return index !== bootstrapIndex
      && (
        /(?:^|[^A-Za-z0-9_])tools\/buck(?:2|\/)/.test(command)
        || /\bdotslash\b/i.test(command)
        || /(?:^|[\s;&|])node\s+scripts\/dev-up\.mjs\s+bootstrap(?:\s|$)/m.test(command)
      );
  });
  if (firstBuckInvocation >= 0 && bootstrapIndex > firstBuckInvocation) {
    failures.push(`${job} must install pinned DotSlash before its first Buck invocation`);
  }
}

export function evaluateCiPreflight(
  workflow,
  buckBuildFile = postgresWrapperBuildFile,
  freeRunnerDiskAction = freeRunnerDiskActionFile,
  executedTestsBaseline = domainUnitExecutedTestsBaseline,
) {
  const failures = [];
  let workflowModel;
  try {
    workflowModel = yaml.load(workflow);
  } catch (error) {
    return { failures: [`CI workflow must parse as YAML: ${error.message}`] };
  }
  if (!workflowModel || typeof workflowModel !== "object") {
    return { failures: ["CI workflow must parse as a YAML mapping"] };
  }
  try {
    const actionModel = yaml.load(freeRunnerDiskAction);
    if (sha256(stableSerialize(actionModel)) !== freeRunnerDiskActionSha256) {
      failures.push("free-runner-disk must preserve its exact composite action execution contract");
    }
  } catch (error) {
    failures.push(`free-runner-disk action must parse as YAML: ${error.message}`);
  }
  requireProtectedExecutionMetadata(workflowModel, failures);
  requireExactRequiredJobContracts(workflowModel, failures);

  for (const trigger of ["push", "pull_request"]) {
    const on = workflowModel.on && typeof workflowModel.on === "object" ? workflowModel.on : null;
    const triggerContract = on && Object.hasOwn(on, trigger) ? on[trigger] : undefined;
    if (triggerContract && typeof triggerContract === "object"
      && ["paths", "paths-ignore"].some((filter) => Object.hasOwn(triggerContract, filter))) {
      failures.push(`${trigger} must create required CI contexts for every change without path filters`);
    }
  }

  const preflight = jobBlock(workflow, "preflight");
  if (!preflight) {
    failures.push("CI must define a preflight job before expensive jobs");
    return { failures };
  }

  const preflightSteps = stepBlocks(preflight);
  if (/^    if:/m.test(preflight)) {
    failures.push("preflight must not define job-level if");
  }
  if (/^    continue-on-error:/m.test(preflight)) {
    failures.push("preflight must not define job-level continue-on-error");
  }
  if (hasJobDefaultShell(preflight)) {
    failures.push("preflight must not override defaults.run.shell");
  }
  if (hasUnsafeStepShell(preflight)) {
    failures.push("preflight may use only the default shell or canonical shell: bash");
  }
  const checkout = preflightSteps.find((step) => step.startsWith("name: Checkout\n"));
  if (!checkout || !/^        with:\n(?:          [^\n]+\n)*          fetch-depth: 0$/m.test(checkout)) {
    failures.push("preflight checkout must fetch full history with fetch-depth: 0");
  }
  requireDotSlashBefore(preflightSteps, "tools/buck/preflight.sh", "preflight", failures);
  requirePreflightRustToolchainBefore(preflightSteps, failures);
  for (const command of requiredAlwaysPreflightCommands) {
    requireRunWithCondition(
      preflightSteps,
      command,
      "preflight",
      preflightNpmCiDependentCondition,
      failures,
    );
  }
  const heavyPreflightCommandConditions = {
    "tools/buck/preflight.sh": preflightBuckHeavyCondition,
    [buckPostgresEnvironmentTestCommand]: preflightBuckHeavyCondition,
    [buckPostgresHarnessTestCommand]: preflightBuckHeavyCondition,
    "cargo metadata --manifest-path backend/Cargo.toml --locked --format-version=1 >/dev/null": preflightRustHeavyCondition,
    "npm run check:executed-tests": preflightNpmCiHeavyCondition,
  };
  for (const command of requiredHeavyPreflightCommands) {
    requireRunWithCondition(
      preflightSteps,
      command,
      "preflight",
      heavyPreflightCommandConditions[command],
      failures,
    );
  }
  {
    const classify = preflightSteps.filter((step) => stepName(step) === "Classify path class");
    if (
      classify.length !== 1
      || !hasOnlyExpectedCondition(classify[0], preflightNpmCiDependentCondition)
      || multilineRunCommands(classify[0]).join("\n") !== pathClassEmitScript.join("\n")
    ) {
      failures.push("preflight must classify path class before thin/heavy step gating");
    }
    if (!/^    outputs:\n(?:      [^\n]+\n)*      path_class:/m.test(preflight)
      || !/^      docs_only:/m.test(preflight)
      || !/^      run_heavy:/m.test(preflight)) {
      failures.push("preflight must expose path_class, docs_only, and run_heavy job outputs");
    }
  }
  requireReasoningLensManifest(preflightSteps, failures);
  requireConsoleRegressionCoverage(workflow, preflightSteps, failures);
  requireReleaseMetadataProofs(preflightSteps, failures);
  // One job, both crates. They share console-kernel-core, so two jobs recompiled
  // the same dependencies and paid two runner startups and two cache restores.
  const domainUnit = jobBlock(workflow, "domain-unit");
  if (domainUnit) {
    const steps = stepBlocks(domainUnit);
    const domainSteps = steps.filter((step) => stepName(step) === "Domain crate unit tests");
    const domainStep = domainSteps[0] ?? "";
    const parsedDomainCommands = shellCommandTokens(runScript(domainStep));
    // Fail-slow keep-going: the run body now carries wrapper commands (`set +e`,
    // `check_status`, the summary). Extract only the cargo/git invocations — the
    // exact surface check-executed-tests attributes — and compare them verbatim,
    // so a rewrapped, renamed, or merged invocation still fails here.
    const invocationCommands = domainUnitWorkflowInvocationTokens(workflow);
    const expectedInvocations = domainUnitExpectedCommands;
    const domainCommandsMatch = domainSteps.length === 1
      && invocationCommands.length === expectedInvocations.length
      && invocationCommands.every((tokens, index) => (
        JSON.stringify(tokens) === JSON.stringify(expectedInvocations[index])
      ));
    if (!domainCommandsMatch || !hasOnlyExpectedCondition(domainStep, runHeavyCondition)) {
      failures.push("domain-unit must execute the locked Cargo test commands directly when run_heavy");
    }
    if (!/ci-keep-going:/.test(domainStep) || !/exit 1/.test(domainStep)) {
      failures.push("domain-unit keep-going block must re-raise failures with a summary exit 1");
    }
    const currentInventory = parseDomainUnitIntegrationInvocations(invocationCommands);
    failures.push(...domainUnitInventoryFailures, ...currentInventory.failures);
    requireDomainUnitExecutedTestsBaseline(
      currentInventory.invocations,
      executedTestsBaseline,
      failures,
    );
    // Job-level env is REQUIRED and pinned to exactly the two cache-key variables:
    // without them this job's rust-cache key never matched the writer's and every
    // run was a cold 685s build. A blanket "no env" ban is what kept that miss in
    // place for 14 runs. Step-level env and any `defaults:` stay forbidden.
    const domainEnvLines = (domainUnit.match(/^    env:\n((?:      [^\n]*\n)*)/m)?.[1] ?? "")
      .split("\n").map((l) => l.trim()).filter((l) => l && !l.startsWith("#"));
    const expectedDomainEnv = ['CARGO_PROFILE_DEV_DEBUG: "0"', 'CARGO_PROFILE_TEST_DEBUG: "0"'];
    if (/^    defaults:/m.test(domainUnit)
      || /^        env:/m.test(domainUnit)
      || JSON.stringify(domainEnvLines) !== JSON.stringify(expectedDomainEnv)) {
      failures.push("domain-unit must use the default shell with no job or step env/defaults overrides");
    }
    const directTokens = parsedDomainCommands.flatMap((command) => command.tokens);
    for (const pkg of domainUnitPackages) {
      const present = directTokens.some((token, index) => token === "-p" && directTokens[index + 1] === pkg);
      if (!present) failures.push(`domain-unit must run -p ${pkg}`);
    }
    for (const t of domainUnitTestFiles) {
      const present = directTokens.some((token, index) => token === "--test" && directTokens[index + 1] === t);
      if (!present) failures.push(`domain-unit must run --test ${t}`);
    }
    if (invocationCommands[0]?.includes("--lib") !== true) {
      failures.push("domain-unit must pass --lib on its first cargo invocation");
    }

    const runStepNames = steps.filter((step) => runCommand(step) !== null).map(stepName);
    if (JSON.stringify(runStepNames) !== JSON.stringify(["Path-class skip proof", "Domain crate unit tests"])) {
      failures.push(`domain-unit must contain only the locked ordered run steps; found ${runStepNames.length} run steps`);
    }
  }

  requirePostgresWrapperContracts(buckBuildFile, failures);
  requireOntologyRestItestReachability(buckBuildFile, workflow, failures);

  for (const [jobName, command] of Object.entries(postgresReachabilityFacetCommands)) {
    const facet = jobBlock(workflow, jobName);
    if (!facet) {
      failures.push(`${jobName} job must exist`);
      continue;
    }
    const steps = stepBlocks(facet);
    requireUnconditionalMultilineRun(
      steps,
      [command],
      jobName,
      failures,
    );
    const facetSetup = postgresReachabilityFacetSetupCommands[jobName];
    requireOnlyLockedRuns(
      steps,
      facetSetup
        ? [pathClassSkipProofScript.join("\n"), facetSetup, command]
        : [pathClassSkipProofScript.join("\n"), command],
      jobName,
      failures,
    );
  }
  // The installer must fetch ONE pinned artifact and verify it BEFORE extracting.
  // Checked against the script, not the workflow: the step digest above proves
  // only that the job calls this file.
  if (!nextestInstallerFile.includes(NEXTEST_PINNED_URL)) {
    failures.push("tools/ci/install_nextest.sh must fetch the pinned cargo-nextest release URL");
  }
  if (!nextestInstallerFile.includes(NEXTEST_PINNED_SHA256)) {
    failures.push("tools/ci/install_nextest.sh must verify the pinned cargo-nextest sha256");
  }
  // Comment lines are excluded deliberately. A plain `includes` was satisfied by
  // the file's own prose ("uses the same curl + `sha256sum --check` pattern"),
  // so deleting the real verification would have left this pin green -- the same
  // shape of hole this gate exists to refuse.
  const nextestExecutableLines = nextestInstallerFile
    .split("\n")
    .filter((line) => !line.trim().startsWith("#"));
  const nextestVerifyIndex = nextestExecutableLines.findIndex((line) =>
    /\|\s*sha256sum --check\b/.test(line),
  );
  const nextestExtractIndex = nextestExecutableLines.findIndex((line) => /\btar -xzf\b/.test(line));
  if (nextestVerifyIndex === -1) {
    failures.push("tools/ci/install_nextest.sh must verify the archive with sha256sum --check");
  }
  if (nextestExtractIndex === -1) {
    failures.push("tools/ci/install_nextest.sh must extract the pinned archive with tar -xzf");
  }
  // Verification has to precede extraction, or a poisoned archive is unpacked
  // before anyone looks at it. Ordering is the property, so ordering is asserted.
  if (nextestVerifyIndex !== -1 && nextestExtractIndex !== -1 && nextestVerifyIndex > nextestExtractIndex) {
    failures.push("tools/ci/install_nextest.sh must verify the sha256 BEFORE extracting the archive");
  }

  {
    const backendBlock = jobBlock(workflow, "backend") ?? "";
    const declared = [...backendBlock.matchAll(/^\s+leg: \[([^\]]+)\]/gm)]
      .flatMap((m) => m[1].split(",").map((x) => x.trim()));
    const referenced = [...backendBlock.matchAll(/matrix\.leg == '([a-z-]+)'/g)].map((m) => m[1]);
    for (const leg of new Set(referenced)) {
      if (!declared.includes(leg)) {
        failures.push(`backend step condition names matrix leg '${leg}', which is not in strategy.matrix.leg -- that step would run on ZERO legs`);
      }
    }
    for (const leg of declared) {
      if (!referenced.includes(leg)) {
        failures.push(`backend matrix leg '${leg}' carries no step -- a leg that runs nothing is pure setup cost and gates nothing`);
      }
    }
    if (JSON.stringify(declared) !== JSON.stringify([...BACKEND_MATRIX_LEGS])) {
      failures.push(`backend strategy.matrix.leg must be exactly ${JSON.stringify([...BACKEND_MATRIX_LEGS])}; found ${JSON.stringify(declared)}`);
    }
  }

  const postgresDomainReachability = jobBlock(workflow, "postgres-domain-reachability");
  if (postgresDomainReachability) {
    const steps = stepBlocks(postgresDomainReachability);
    requirePostgresAggregateStateMachine(steps, failures);
    requireOnlyLockedRuns(
      steps,
      [
        postgresPreflightNonEvaluationScript.join("\n"),
        pathClassSkipProofScript.join("\n"),
        postgresDomainReachabilityAggregatorCommands.join("\n"),
      ],
      "postgres-domain-reachability",
      failures,
    );
    if (!/name:\s*Dispatch, attendance and ontology — disposable PostgreSQL reachability/.test(postgresDomainReachability)) {
      failures.push("postgres-domain-reachability must keep load-bearing display name");
    }
    // needs.preflight is not transitive from facet jobs; without a direct need,
    // run_heavy/path_class are empty and the aggregator always takes the skip-proof path.
    const needsBlock = postgresDomainReachability.match(/\n    needs:\n((?:      - .+\n)+)/);
    if (!needsBlock || !needsBlock[1].includes("      - preflight\n")) {
      failures.push("postgres-domain-reachability must declare preflight as a direct need");
    }
    requireCargoPostgresMapCoversWorkflow(failures);
  }

  const companyConformance = jobBlock(workflow, "company-conformance");
  if (companyConformance) {
    const steps = stepBlocks(companyConformance);
    requireOrderedStepContracts(
      steps,
      [{
        name: "Path-class skip proof",
        run: pathClassSkipProofScript.join("\n"),
        if: skipProofCondition,
      }, {
        name: "Install pinned DotSlash runtime",
        run: dotSlashBootstrap,
        if: runHeavyCondition,
      }, {
        name: "Company conformance against disposable PostgreSQL",
        run: companyConformanceCommands.join("\n"),
        if: runHeavyCondition,
      }],
      "company-conformance",
      failures,
    );
    requireOnlyLockedRuns(
      steps,
      [pathClassSkipProofScript.join("\n"), dotSlashBootstrap, companyConformanceCommands.join("\n")],
      "company-conformance",
      failures,
    );
  }

  const backend = jobBlock(workflow, "backend");
  if (backend) {
    const steps = stepBlocks(backend);
    const pr473ContractTestCommand = "python3 scripts/check-pr473-migration-operational.test.py -v";
    const sourceGateContracts = [
      ["Layer-boundary gate", "../tools/buck2 run //backend/ci/gates/layer-boundary:console-gate-layer-boundary"],
      ["Audit-coverage gate", "cargo run -p console-gate-audit-coverage"],
      ["Migration-safety gate", "cargo run -p console-gate-migration-safety"],
      ["Tenant-isolation gate", "cargo run -p console-gate-tenant-isolation"],
      ["PII-no-logs gate", "cargo run -p console-gate-pii-no-logs"],
      ["RLS-arming gate", "cargo run -p console-gate-rls-arming"],
      ["Dev-auth-absence gate", "cargo run -p console-gate-dev-auth-absence"],
      ["IaC tier-discipline gate", "cargo run -p console-gate-iac-tier"],
      ["Fabricated-branch gate", "cargo run -p console-gate-fabricated-branch"],
    ];
    // Fail-slow sweep: fmt/clippy/gates (and the topology reconcile they do not
    // depend on) run regardless of each other; the DB-dependent steps guard on
    // the topology reconcile's success so one root failure shows as skipped.
    const gateIndexes = requireOrderedStepContracts(
      steps,
      [
        { name: "clippy -D warnings", run: "SQLX_OFFLINE=true cargo clippy --all-targets -- -D warnings", if: backendLegCondition("cargo") },
        ...sourceGateContracts.map(([name, run]) => ({ name, run, if: backendLegCondition("cargo") })),
        { name: "PR 473 migration operational contract tests", run: pr473ContractTestCommand, if: backendLegCondition("cargo") },
        { name: "Reconcile portable PostgreSQL role topology", run: undefined, if: backendIndependentCondition },
        { name: "Boot smoke — migrate + serve + /readyz", run: undefined, if: backendLegTopologyCondition("cargo") },
      ],
      "backend",
      failures,
    );
    if (gateIndexes[1] !== gateIndexes[0] + 1) {
      failures.push("backend must run source-only gates immediately after clippy");
    }
    requireOrderedStepContracts(
      steps,
      [
        {
          name: "Buck2 dev-auth feature PostgreSQL suites",
          run: [
            "tools/buck/test_needs_postgres.sh --num-threads=1 \\",
            "//tools/buck:auth-rest-dev-auth-inline-postgres \\",
            "//tools/buck:auth-rest-dev-auth-session-postgres \\",
            "//tools/buck:auth-rest-dev-auth-group-admin-postgres \\",
            "//tools/buck:provisioning-dev-principal-upsert-race-postgres",
          ].join("\n"),
          workingDirectory: ".",
          if: backendLegTopologyCondition("buck-dev-auth"),
        },
        {
          // Locked so the step cannot be deleted silently: the crate's residual
          // lowering decides row visibility, and its unit target executed in no
          // workflow at all until this contract existed.
          name: "Buck2 platform-authz unit suite",
          run: "env -u DATABASE_URL tools/buck2 test //backend/crates/platform/authz:console-platform-authz-unit",
          workingDirectory: ".",
          if: backendLegCondition("buck-app"),
        },
        {
          name: "Cargo console-app unit suite",
          run: "env -u DATABASE_URL cargo test --locked --manifest-path backend/Cargo.toml -p console-app --lib",
          workingDirectory: ".",
          if: backendLegCondition("buck-app"),
        },
        {
          // The suite H-1 is *about*. `openapi_drift` is the only thing that inventories every
          // mounted route against openapi.yaml, and it was unprotected: deleting this `run:` line
          // left check:ci-preflight, check:foundation-gates and check:doc-citations all exiting 0.
          // check:request-body-contract closed H-1's request-body half but reads no route
          // inventory, so nothing else in CI covers what this step covers — a gate one line from
          // silent removal is the meta-finding this file exists to refuse.
          name: "Cargo console-app OpenAPI drift suite",
          run: "env -u DATABASE_URL cargo test --locked --manifest-path backend/Cargo.toml -p console-app --test openapi_drift",
          workingDirectory: ".",
          if: backendLegCondition("buck-app"),
        },
        {
          name: "Cargo console-app inline PostgreSQL suite",
          run: "export DATABASE_URL=\"${CONSOLE_BUCK_ADMIN_DATABASE_URL:?missing test bootstrap URL}\"\ncargo test --locked --manifest-path backend/Cargo.toml -p console-app --lib --features test-postgres -- --test-threads=1",
          workingDirectory: ".",
          if: backendLegTopologyCondition("buck-app"),
        },
        {
          name: "Cargo console-app dev-auth PostgreSQL suite",
          run: "export DATABASE_URL=\"${CONSOLE_BUCK_ADMIN_DATABASE_URL:?missing test bootstrap URL}\"\ncargo test --locked --manifest-path backend/Cargo.toml -p console-app --test dev_auth_persona_guard_feature --features dev-auth -- --test-threads=1",
          workingDirectory: ".",
          if: backendLegTopologyCondition("buck-app"),
        },
      ],
      "backend",
      failures,
    );
    const directCargoTestAnalysis = steps.reduce(
      (analysis, step) => mergeCargoAnalysis(analysis, cargoTestAnalysisInStep(step)),
      emptyCargoAnalysis(),
    );
    if (directCargoTestAnalysis.malformed) {
      failures.push("backend must not contain a malformed executable shell surface");
    }
    for (const packageName of ["console-platform-auth-rest", "console-platform-provisioning"]) {
      if (directCargoTestAnalysis.packages.has(packageName)) {
        failures.push("backend must not run direct Cargo PostgreSQL tests for " + packageName);
      }
    }
  }

  const fullGeneratedFaces = jobBlock(workflow, "generated-face-authority");
  if (fullGeneratedFaces) {
    const fullGeneratedFaceSteps = stepBlocks(fullGeneratedFaces);
    const fullGeneratedFaceCommand = "tools/buck/preflight.sh --full-generated-faces";
    const matchingFullGateSteps = fullGeneratedFaceSteps.filter((step) => runScalar(step) === fullGeneratedFaceCommand);
    if (matchingFullGateSteps.length === 0) {
      failures.push("generated-face-authority must run the complete generated-face closure");
    } else if (matchingFullGateSteps.some((step) => !hasOnlyExpectedCondition(step, runHeavyCondition))) {
      failures.push("generated-face-authority must run the complete generated-face closure only when run_heavy");
    }
    requireDotSlashBefore(
      fullGeneratedFaceSteps,
      fullGeneratedFaceCommand,
      "generated-face-authority",
      failures,
    );
    requireReindeerToolchainBefore(fullGeneratedFaceSteps, fullGeneratedFaceCommand, failures);
  }

  // repo-gates was entirely unlocked: deleting `run: npm run check:adrs` from it returned zero
  // preflight failures. A step wired into ci.yml is not thereby protected — it occupies a slot in
  // the job list and reads as coverage while being one line away from silent removal.
  const repoGates = jobBlock(workflow, "repo-gates");
  if (repoGates) {
    requireOrderedStepContracts(
      stepBlocks(repoGates),
      [{
        name: "Undeclared imports — every bare specifier must be declared",
        run: "npm run check:undeclared-imports",
        if: runHeavyUnlessCancelledCondition,
      }, {
        // Wired in 4e7da6b52 and unprotected until now: deleting this step returned zero
        // preflight failures, which is the same one-line-from-silent-removal state the
        // undeclared-imports step above was added to escape.
        name: "Request-body contract — spec fields must exist on the handler",
        run: "npm run check:request-body-contract",
        if: runHeavyUnlessCancelledCondition,
      }],
      "repo-gates",
      failures,
    );
  }

  const kubernetesManifests = jobBlock(workflow, "kubernetes-manifests");
  if (kubernetesManifests) {
    requireOrderedStepContracts(
      stepBlocks(kubernetesManifests),
      [{
        name: "Path-class skip proof",
        run: pathClassSkipProofScript.join("\n"),
        if: skipProofCondition,
      }, {
        name: "Production hardening contract",
        run: "npm run check:production-hardening",
        if: runHeavyUnlessCancelledCondition,
      }],
      "kubernetes-manifests",
      failures,
    );
  }

  const apiContract = jobBlock(workflow, "api-contract");
  if (apiContract) {
    const apiContractSteps = stepBlocks(apiContract);
    if (!hasOnlyAllowedApiContractSteps(apiContractSteps)) {
      failures.push("api-contract must contain only the approved ordered steps");
    }
    if (/^    (?:services|env):/m.test(apiContract)) {
      failures.push("api-contract is text-only and must not provision services or job-level environment");
    }
    const driftGateCount = apiContractSteps
      .filter((step) => runScalar(step) === "npm run check:platform-contract-drift").length;
    if (driftGateCount !== 1) {
      failures.push("api-contract must run exactly one npm run check:platform-contract-drift");
    }
    // api-contract is now text-only: every step reads backend/openapi/openapi.yaml and
    // asserts against it. Nothing builds or boots the app, so nothing may hand a binary
    // path (or anything else) to a later step through the environment file.
    if (apiContract.includes("GITHUB_ENV")) {
      failures.push("api-contract must not hand state to later steps through GITHUB_ENV");
    }
    if (apiContract.includes("CONSOLE_APP_BIN")) {
      failures.push("api-contract must not reference CONSOLE_APP_BIN; the job builds no app");
    }
  }

  for (const job of ["backend"]) {
    const block = jobBlock(workflow, job);
    if (block) requireEffectiveDotSlashBootstrap(block, job, failures);
  }

  // The two jobs that actually run cargo must SHARE one rust-cache entry.
  //
  // MEASURED 2026-07-31: six jobs carried Swatinem/rust-cache, all with
  // `workspaces: backend` and none with a shared-key. rust-cache keys on job name by
  // default, so those were six separate near-duplicate caches of one workspace competing
  // for a 10GB repository LRU budget — they evicted each other. Four of the six never ran
  // cargo at all (Buck2 does not write backend/target) and were removed outright.
  //
  // Without this assertion, deleting `shared-key` silently refragments them: CI stays
  // green, nothing reports it, and the caches quietly stop being shared. Verified that a
  // deletion passed every gate before this block existed.
  const cargoRustCacheJobs = [
    "domain-unit",
    "backend",
    // Restore-only. Was the undeclared SECOND writer of the shared entry: its
    // rust-cache step carried `save-if: main` under a comment reading "The ONLY
    // writer", copied from `backend` when the job was split out. Absent from this
    // list, the one-writer check below never saw it.
    "migration-expand-contract",
    "postgres-reachability-app",
    "postgres-reachability-platform",
    "postgres-reachability-ontology",
    "postgres-reachability-domain-a",
    "postgres-reachability-domain-b",
  ];
  for (const job of cargoRustCacheJobs) {
    const block = jobBlock(workflow, job);
    if (!block) continue;
    if (!/shared-key:\s*backend-cargo/.test(block)) {
      failures.push(`${job} must share the rust-cache entry via shared-key: backend-cargo`);
    }
    if (!/cache-all-crates:\s*"true"/.test(block)) {
      failures.push(`${job} must set cache-all-crates: "true" on rust-cache`);
    }
  }
  // Exactly ONE writer. Sharing a key without deciding who saves it means whichever job
  // finishes first publishes the cache — so domain-unit's three crates could become the
  // entry `backend` restores for a whole-workspace clippy. `backend` runs
  // clippy --all-targets, a strict superset, so it is the writer and every other cargo
  // job is restore-only.
  {
    const writers = cargoRustCacheJobs.filter((job) => {
      const block = jobBlock(workflow, job);
      return block && !/save-if:\s*false/.test(block);
    });
    // `backend` is a three-leg matrix. "One writer" is not enough when the one
    // job runs three times: three legs saving the same key race each other on
    // every main push. The textual `save-if: false` test above cannot tell one
    // leg saving from three, so the writer's save-if must name exactly one leg.
    {
      const backendBlock = jobBlock(workflow, "backend") ?? "";
      const legs = backendBlock.match(/save-if:[^\n]*matrix\.leg == '([a-z-]+)'/g) ?? [];
      if (legs.length !== 1) {
        failures.push(
          `backend's rust-cache save-if must name exactly ONE matrix leg as the writer; found ${legs.length}`,
        );
      }
    }
    for (const job of cargoRustCacheJobs) {
      if (job === "backend") continue;
      const block = jobBlock(workflow, job);
      if (block && !/save-if:\s*false/.test(block)) {
        failures.push(`${job} must be restore-only (save-if: false) on shared rust-cache`);
      }
    }
    if (writers.length !== 1 || writers[0] !== "backend") {
      failures.push(
        `exactly one cargo job may write the shared rust-cache and it must be backend; found: ${writers.join(", ") || "none"}`,
      );
    }
  }
  // Buck2 never writes backend/target, so a rust-cache step on a Buck2-only job is pure
  // transfer cost and an LRU slot taken from the jobs that do use it.
  for (const job of ["company-conformance", "api-contract"]) {
    const block = jobBlock(workflow, job);
    if (block && /Swatinem\/rust-cache/.test(block)) {
      failures.push(`${job} runs no cargo and must not carry a rust-cache step`);
    }
  }

  for (const job of protectedJobs) {
    const block = jobBlock(workflow, job);
    if (!block) {
      failures.push(`CI must define protected job ${job}`);
      continue;
    }
    if (!needsPreflight(block)) failures.push(`${job} must need preflight`);
    const jobIf = block.match(/^    if: (.+)$/m)?.[1] ?? null;
    if (postsubmitProtectedJobs.includes(job)) {
      if (jobIf !== postsubmitHeavyJobIf) {
        failures.push(`${job} must run only on push/workflow_dispatch when run_heavy`);
      }
    } else if (jobIf != null) {
      failures.push(`${job} must not define job-level if`);
    }
    if (/^    continue-on-error:/m.test(block)) failures.push(`${job} must not define job-level continue-on-error`);
    if (hasJobDefaultShell(block)) failures.push(`${job} must not override defaults.run.shell`);
    if (hasUnsafeStepShell(block)) failures.push(`${job} may use only the default shell or canonical shell: bash`);
  }

  const rustFmt = jobBlock(workflow, "rust-fmt");
  if (!rustFmt) {
    failures.push("CI must define protected job rust-fmt");
  } else {
    if (needsPreflight(rustFmt)) failures.push("rust-fmt must not need preflight");
    if (/^    if:/m.test(rustFmt)) failures.push("rust-fmt must not define job-level if");
    if (/^    continue-on-error:/m.test(rustFmt)) failures.push("rust-fmt must not define job-level continue-on-error");
    if (hasJobDefaultShell(rustFmt)) failures.push("rust-fmt must not override defaults.run.shell");
    if (hasUnsafeStepShell(rustFmt)) failures.push("rust-fmt may use only the default shell or canonical shell: bash");
  }

  return { failures };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (process.argv.includes("--emit-path-class")) {
    const resolved = emitPathClassGithubOutput();
    console.log(
      `path_class=${resolved.pathClass} docs_only=${resolved.docsOnly} run_heavy=${resolved.runHeavy} run_live_postgres=${resolved.runLivePostgres} reason=${resolved.reason}`,
    );
    process.exit(0);
  }
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const { failures } = evaluateCiPreflight(readFileSync(resolve(root, ".github/workflows/ci.yml"), "utf8"));
  if (failures.length > 0) {
    console.error("CI preflight contract failed:");
    for (const failure of failures) console.error(`- ${failure}`);
    process.exit(1);
  }
  console.log("CI preflight contract passed.");
}
