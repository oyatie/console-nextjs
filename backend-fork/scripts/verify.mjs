#!/usr/bin/env node
// Local mirror of the CI jobs whose commands have safe local equivalents.
//
// Why this exists: the mirrored jobs run many commands in a specific order, several
// of which need a PostgreSQL identity that migration 0196 restricts to
// `console_buck_admin`. Nobody can hold that in their head, so defects were reaching
// CI and costing a 45-minute round trip *each*, and because failures mask one
// another the round trips serialise: one masked layer per run.
//
// The plan below is checked against `.github/workflows/ci.yml` on every run. A
// CI step that is not classified here is a hard error, so this file cannot
// quietly fall behind the pipeline it claims to mirror -- which is exactly how
// the workspace-test run went missing for many commits (H-8 in
// docs/program/false-green-gate-holes.md).
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import yaml from "js-yaml";

const WORKFLOW = ".github/workflows/ci.yml";
/**
 * Every job in `ci.yml`, declared exactly once. `true` means its run-steps are
 * classified in PLAN below; a string means deliberately not mirrored and says
 * why. A job present in CI but absent here is a hard error -- otherwise the
 * step-level completeness check silently scopes itself to whatever this list
 * happened to contain, and a whole new job could land uncovered while
 * `npm run verify` still printed "passed".
 */
const JOBS = new Map([
  ["preflight", true],
  ["backend", true],
  // Split out of `backend` 2026-08-18; it was one step of that job, so it keeps
  // the same local-mirror disposition rather than acquiring a new exemption.
  ["migration-expand-contract", true],
  ["repo-gates", true],
  ["kubernetes-manifests", true],
  ["domain-unit", true],
  // Facet jobs: hosted disposable PG + cargo shard partitions. Local `db` tier
  // already exercises the harness; mirror would re-run the full wall.
  ["postgres-reachability-app", "hosted PostgreSQL facet (app packages); the `db` tier already exercises that harness"],
  ["postgres-reachability-platform", "hosted PostgreSQL facet (platform packages); the `db` tier already exercises that harness"],
  ["postgres-reachability-ontology", "hosted PostgreSQL facet (ontology packages); the `db` tier already exercises that harness"],
  ["postgres-reachability-domain-a", "hosted PostgreSQL facet (domain adapters A); the `db` tier already exercises that harness"],
  ["postgres-reachability-domain-b", "hosted PostgreSQL facet (domain adapters B); the `db` tier already exercises that harness"],
  // Load-bearing aggregator: its three hosted-only run states consume Actions `needs` results.
  ["postgres-domain-reachability", "load-bearing hosted state machine over preflight and PG facet needs/results; exact non-evaluation/thin/heavy contract is enforced by check-ci-preflight"],
  ["generated-face-authority", "needs pinned Java + Reindeer toolchains to rebuild the full generated-face closure"],
  ["api-contract", true],
  ["company-conformance", true],
  ["rust-fmt", true],
  ["required-ci", "terminal status aggregate; its exact needs/result contract is enforced by check-ci-preflight"],
]);
const MIRRORED_JOBS = [...JOBS].filter(([, v]) => v === true).map(([name]) => name);
const POSTGRES_IMAGE =
  "postgres:18.4@sha256:4aabea78cf39b90e834caf3af7d602a18565f6fe2508705c8d01aa63245c2e20";
const BOOTSTRAP_GUC = "options%5Bconsole.sqlx_test_bootstrap%5D=buck-sqlx-superuser-v1";

/**
 * Every `run:` step in the mirrored jobs, keyed by its CI step name.
 *   tier "fast"    -- no Docker, no network
 *   tier "db"      -- needs Docker
 *   tier "ci-only" -- deliberately not mirrored; `why` must say what covers it
 * `run` overrides the CI command when the local equivalent differs.
 */
const PLAN = new Map([
  // ---- preflight ---------------------------------------------------------
  ["Install pinned DotSlash runtime", {
    tier: "ci-only",
    why: "runner bootstrap; a local checkout already has the tools/buck2 shim",
  }],
  ["Install workspace dependencies", {
    tier: "ci-only",
    why: "`npm ci` deletes node_modules; `Canonical npm lockfile` below covers lockfile drift",
  }],
  // Emits path_class / docs_only / run_heavy for hosted job scheduling. Locally
  // the PATH_CLASS_* event SHAs are absent, so --emit-path-class always lands
  // on unsupported-event; classification totality lives in `CI preflight
  // contract` (+ its unit tests), and local verify always takes the heavy path.
  ["Classify path class", {
    tier: "ci-only",
    why: "Actions-only PATH_CLASS_* event SHAs; `CI preflight contract` covers classification; local mirror always exercises the heavy path",
  }],
  ["Release metadata semantic regression", { tier: "fast" }],
  ["Release metadata semantic gate", {
    tier: "ci-only",
    why: "requires the exact hosted event base/head commits after release-only classification; run manually with `node scripts/check-release-metadata.mjs --base <base-sha> --head <release-tip-sha>`",
  }],
  ["Release metadata documentation link tests", { tier: "fast" }],
  ["Release metadata documentation manifest gate", { tier: "fast" }],
  ["Release metadata documentation local-link gate", { tier: "fast" }],
  ["Cheap Buck2 generated-face admission", { tier: "fast" }],
  ["Foundation gate contract", { tier: "fast" }],
  ["Reasoning lens manifest regression", { tier: "fast" }],
  ["Reasoning lens manifest drift", { tier: "fast" }],
  ["CI preflight contract tests", { tier: "fast" }],
  ["Buck impact planner regression", { tier: "fast" }],
  ["Console route inventory regression", { tier: "fast" }],
  ["Console authority-train regression", { tier: "fast" }],
  ["Console lane-receipt validator regression", { tier: "fast" }],
  // Wired into `ci.yml` by #556, which is the first thing this mirror had to
  // learn after it started running: the step existed on main and was declared
  // nowhere, so the job-completeness check would have failed closed on it.
  ["Console PR authority bootstrap regression", { tier: "fast" }],
  ["Executed-tests baseline set regression", { tier: "fast" }],
  // The guard that keeps THIS file honest. It ran in no workflow until 2026-08-01,
  // which is why `support-domain-unit` outlived its rename to `domain-unit` on main
  // and `npm run verify` was red for everyone.
  ["Local CI mirror contract", { tier: "fast" }],
  ["Console truth-ledger validator exact-M regression", { tier: "fast" }],
  ["Console fanout planner exact-M regression", { tier: "fast" }],
  ["Buck PostgreSQL environment wrapper regression", { tier: "fast" }],
  ["Buck disposable PostgreSQL harness regression", { tier: "fast" }],
  ["CI preflight contract", { tier: "fast" }],
  ["Canonical npm lockfile", { tier: "fast" }],
  ["Cargo.lock consistency", { tier: "fast" }],
  // The Buck2-exit safety net. It lives in preflight and not in repo-gates
  // because it now resolves `cargo metadata` as well as the BUCK graph, and
  // preflight is the job that already has both a pinned Rust toolchain and npm —
  // the step above it is `cargo metadata` on the same manifest. Pure analysis,
  // no Docker, so `fast`.
  ["Executed-tests ratchet — a test binary must have a path from a workflow step", { tier: "fast" }],
  ["JavaScript test reachability ratchet", { tier: "fast" }],
  ["JavaScript test reachability unit tests", { tier: "fast" }],
  // Runs node directly on the harness preflight: no container, no bypass env var, so `fast`.
  ["Lane fan-out harness preflight", { tier: "fast" }],
  // Execs tools/lanes/no-credential-in-argv.sh directly. No container, no
  // bypass env var, so `fast`.
  ["Workflow test-runner credential literals", { tier: "fast" }],

  // ---- domain-unit -------------------------------------------------------
  // This is the exact no-database Cargo selection from CI. A former exemption
  // incorrectly claimed fast verification ran a workspace suite; it did not.
  ["Domain crate unit tests", { tier: "fast" }],
  // Hosted skip proofs fire only when preflight sets run_heavy!=true (a thin
  // path class). Local verify never thin-schedules mirrored jobs — it
  // always runs the heavy command path — so mirroring the printf proof is
  // meaningless theater. One PLAN key covers every mirrored job that carries
  // the identically-named step.
  ["Path-class skip proof", {
    tier: "ci-only",
    why: "skip proofs run only for thin path classes on hosted CI; local mirror always exercises the heavy path",
  }],
  // Fail-slow sweep collector: reads the GHA `steps` context (toJSON(steps)),
  // which exists only on a hosted runner; the same job-level red is produced
  // locally by the run step itself.
  ["Collect failures", {
    tier: "ci-only",
    why: "reads the hosted steps context (toJSON(steps)); no local equivalent",
  }],

  // ---- backend -----------------------------------------------------------
  ["rustfmt check", { tier: "fast" }],
  ["clippy -D warnings", { tier: "fast" }],
  ["Layer-boundary gate", { tier: "fast" }],
  ["Audit-coverage gate", { tier: "fast" }],
  ["Migration-safety gate", { tier: "fast" }],
  ["Tenant-isolation gate", { tier: "fast" }],
  ["PII-no-logs gate", { tier: "fast" }],
  ["RLS-arming gate", { tier: "fast" }],
  ["Dev-auth-absence gate", { tier: "fast" }],
  ["IaC tier-discipline gate", { tier: "fast" }],
  ["Fabricated-branch gate", { tier: "fast" }],
  ["Personal-data-classification gate", { tier: "fast" }],
  // Static half only. The load-bearing database half of writer ownership is the
  // capability census in ops/postgres-reconcile-topology.sh, executed by
  // console-gate-writer-ownership's census binary in the PostgreSQL facets.
  ["Writer-ownership gate", { tier: "fast" }],
  // The eleven `cargo run -p console-gate-*` steps above prove only that each
  // gate exits 0 against THIS tree — which a gate scanning an empty directory
  // also does. These eight targets plant a violation in a throwaway tree and
  // assert the gate rejects it, so they are what distinguishes "scanned and
  // found nothing" from "scanned nothing". Unsets DATABASE_URL: none of them
  // touches a database. writer-ownership's equivalent suite is not here: it
  // reads the real checkout, so it runs under `Domain crate unit tests`.
  ["Buck2 CI-gate mutation suites — every gate proven to still reject", { tier: "fast" }],
  ["PR 473 migration operational contract tests", { tier: "fast" }],
  ["Reconcile portable PostgreSQL role topology", {
    tier: "db",
    provision: true,
    why: "replaced locally by a disposable container carrying the same seven roles plus console_buck_admin",
  }],
  ["Expand/contract migration rehearsal (0165, 0166)", { tier: "db" }],
  ["Boot smoke — migrate + serve + /readyz", {
    tier: "db",
    run: "CONSOLE_APP_ROLE=migrate SQLX_OFFLINE=true cargo run -q -p console-app",
    why: "migrate half only; the serve/readyz half needs the full CI keypair fixture",
  }],
  // These suites moved off direct Cargo onto generator-owned Buck targets and
  // the disposable role-topology harness, which supplies the
  // migration-0196-authorized `console_buck_admin` identity. The harness brings up
  // its own PostgreSQL, so this needs Docker but not a provisioned database.
  ["Buck2 dev-auth feature PostgreSQL suites", { tier: "db" }],
  // Unsets DATABASE_URL and needs no Docker: the crate's unit target is pure
  // residual-lowering and authoring logic. Mirrored locally because it is the
  // only place the row-visibility safety properties execute at all.
  ["Buck2 platform-authz unit suite", { tier: "fast" }],
  ["Cargo console-app unit suite", { tier: "fast" }],
  // Unsets DATABASE_URL, so it is a no-Docker step despite living among the
  // PostgreSQL suites. It is the only inventory of mounted routes against
  // openapi.yaml, so a local miss here is a client-contract miss.
  ["Cargo console-app OpenAPI drift suite", { tier: "fast" }],
  ["Cargo console-app inline PostgreSQL suite", { tier: "db" }],
  ["Cargo console-app dev-auth PostgreSQL suite", { tier: "db" }],

  // ---- repo-gates --------------------------------------------------------
  // Pure `npm run` gate binaries: no Docker, no Rust toolchain, no network.
  // `Install workspace dependencies`, `Foundation gate contract` and
  // `Canonical npm lockfile` are byte-identical to their preflight twins, so
  // the entries above cover both jobs.
  ["ADR governance tests", { tier: "fast" }],
  ["ADR governance gate", { tier: "fast" }],
  ["Documentation link tests", { tier: "fast" }],
  ["Documentation manifest gate", { tier: "fast" }],
  ["Documentation local-link gate", { tier: "fast" }],
  ["Shared text gate unit tests", { tier: "fast" }],
  ["Gate-input provenance instrument", { tier: "fast" }],
  ["Gate-input provenance unit tests", { tier: "fast" }],
  ["G004 identity group org people policy foundation gate", { tier: "fast" }],
  ["G005 workflow approval Work Hub lifecycle gate", { tier: "fast" }],
  ["Workflow runtime spine gate", { tier: "fast" }],
  ["Workflow runtime M2 strangler dark-landing gate", { tier: "fast" }],
  ["Workflow runtime M2 Cedar-guard observe-and-record gate", { tier: "fast" }],
  ["Workflow runtime M2 flag-ON runtime gate", { tier: "fast" }],
  ["Workflow runtime M2 outbox-drainer transactional-idempotency gate", { tier: "fast" }],
  ["G006 asset equipment dispatch lifecycle gate", { tier: "fast" }],
  ["G007 collaboration mail calendar poll mobile lifecycle gate", { tier: "fast" }],
  ["G008 import HR payroll readiness gate", { tier: "fast" }],
  ["People HR lifecycle maturity gate", { tier: "fast" }],
  ["Payroll release-gate contract", { tier: "fast" }],
  ["Undeclared imports — every bare specifier must be declared", { tier: "fast" }],
  ["Request-body contract — spec fields must exist on the handler", { tier: "fast" }],
  // The step name promises the whole repository; the gate resolves a declared
  // subset. Mirrored as-is so local matches CI exactly; renaming the step is
  // what would make the promise true.
  ["Doc citations — every code citation must resolve", { tier: "fast" }],

  // ---- api-contract ------------------------------------------------------
  // This job became text-only when the tautological app-served OpenAPI comparison
  // and its Buck2 binary handoff were deleted, so every remaining command is safe
  // to mirror locally.
  ["Install Node tooling", {
    tier: "ci-only",
    why: "`npm ci` deletes node_modules; `Canonical npm lockfile` covers lockfile drift",
  }],
  ["Platform contract drift gate", { tier: "fast" }],
  ["Employee import replay contract", { tier: "fast" }],
  ["Ontology write precondition contract", { tier: "fast" }],

  // ---- kubernetes-manifests ---------------------------------------------
  // Mirrored because `check:production-hardening` pins the exact text of the
  // backend topology step: editing ci.yml fails a job most people would never
  // think to run. That cost a full CI cycle to discover.
  ["Install kubectl (for kustomize renderer)", {
    tier: "ci-only",
    why: "downloads a pinned kubectl; local checkouts use the one already on PATH",
  }],
  ["Install kustomize (NetworkPolicy static render proof)", {
    tier: "ci-only",
    why: "downloads a pinned kustomize; local checkouts use the one already on PATH, and the script falls back to `kubectl kustomize`",
  }],
  ["Governed command-database DARK wiring regression", { tier: "fast" }],
  ["Render manifests and NetworkPolicy enforcement preflight", { tier: "fast" }],
  ["Production hardening contract", { tier: "fast" }],
  ["Install production-hardening test dependencies", {
    tier: "ci-only",
    why: "hosted dependency bootstrap; `npm ci --ignore-scripts` would replace an already-installed local node_modules tree",
  }],
  ["Production hardening regression tests", { tier: "fast" }],

  // ---- company-conformance ------------------------------------------------
  // Mirrored as of the fan-out's last commit. It was declared not-mirrored while
  // the suite was expected RED, because running it locally would have failed
  // every verify for the same reason it was deliberately not a required check.
  // All five lane types now exist and it is green at 12/12, so it is a normal
  // `db`-tier step and the local mirror runs the real thing.
  ["Company conformance against disposable PostgreSQL", { tier: "db" }],
]);

/** Fail closed when ci.yml grows or loses a job, in either direction. */
export function jobMirrorDisposition(job) {
  return JOBS.get(job);
}

export function assertJobsDeclared(jobNames = Object.keys(yaml.load(readFileSync(WORKFLOW, "utf8")).jobs ?? {})) {
  const undeclared = jobNames.filter((name) => !JOBS.has(name));
  const vanished = [...JOBS.keys()].filter((name) => !jobNames.includes(name));
  const problems = [];
  if (undeclared.length) {
    problems.push(
      `CI jobs missing from scripts/verify.mjs (mirror it, or declare why not):\n  ${undeclared.join("\n  ")}`,
    );
  }
  if (vanished.length) {
    problems.push(`Declared jobs with no matching CI job:\n  ${vanished.join("\n  ")}`);
  }
  if (problems.length) throw new Error(problems.join("\n\n"));
  return jobNames;
}

function ciSteps() {
  const doc = yaml.load(readFileSync(WORKFLOW, "utf8"));
  assertJobsDeclared(Object.keys(doc.jobs ?? {}));
  const steps = [];
  for (const job of MIRRORED_JOBS) {
    const defined = doc.jobs?.[job]?.steps;
    if (!Array.isArray(defined)) throw new Error(`${WORKFLOW} has no ${job} job steps`);
    const jobCwd = doc.jobs[job].defaults?.run?.["working-directory"] ?? ".";
    for (const step of defined) {
      if (typeof step.run !== "string") continue;
      if (!step.name) throw new Error(`${job}: every mirrored run-step needs a name`);
      // Honour the same working directory CI uses; the backend job defaults to
      // `backend/`, so running these from the repo root finds no Cargo.toml.
      const cwd = step["working-directory"] ?? jobCwd;
      steps.push({ job, name: step.name, run: step.run.trim(), cwd });
    }
  }
  return steps;
}

/** Fail closed when CI and this plan disagree, in either direction. */
export function assertPlanCoversCi(steps = ciSteps()) {
  const seen = new Set();
  const unclassified = [];
  for (const step of steps) {
    seen.add(step.name);
    if (!PLAN.has(step.name)) unclassified.push(`${step.job}: ${step.name}`);
  }
  const stale = [...PLAN.keys()].filter((name) => !seen.has(name));
  const problems = [];
  if (unclassified.length) {
    problems.push(
      `CI steps missing from scripts/verify.mjs (classify each as fast/db/ci-only):\n  ${unclassified.join("\n  ")}`,
    );
  }
  if (stale.length) {
    problems.push(`Plan entries with no matching CI step:\n  ${stale.join("\n  ")}`);
  }
  if (problems.length) throw new Error(problems.join("\n\n"));
  return steps;
}

export function reasoningLensManifestTierFromPlan() {
  return PLAN.get("Reasoning lens manifest drift")?.tier ?? null;
}

export function stepMirrorDisposition(name) {
  return PLAN.get(name) ?? null;
}

function run(command, env, cwd = ".") {
  const result = spawnSync("bash", ["-o", "pipefail", "-c", command], {
    stdio: "inherit",
    env,
    cwd,
  });
  return result.status ?? 1;
}

function captureFile(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`failed: ${command} ${args.join(" ")}\n${result.stderr}`);
  }
  return result.stdout.trim();
}

export function writePrivateFile(path, contents) {
  writeFileSync(path, contents, { flag: "wx", mode: 0o600 });
}

export function topologyEnvContents(credentials) {
  const entries = [
    ["POSTGRES_ADMIN_PASSWORD", credentials.admin],
    ["CONSOLE_APP_POSTGRES_PASSWORD", credentials.app],
    ["CONSOLE_RT_POSTGRES_PASSWORD", credentials.runtime],
    ["CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD", credentials.leaveCommand],
    ["CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD", credentials.ontologyCommand],
    ["CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD", credentials.platformForce],
  ];
  if (new Set(entries.map(([, password]) => password)).size !== entries.length) {
    throw new Error("PostgreSQL topology credentials must be pairwise distinct");
  }
  return [
    "POSTGRES_HOST=127.0.0.1",
    "POSTGRES_DB=console_ci",
    "POSTGRES_ADMIN_USER=postgres",
    ...entries.map(([name, password]) => `${name}=${password}`),
    "",
  ].join("\n");
}

function generateDistinctPasswords(count) {
  const passwords = [];
  while (passwords.length < count) {
    const candidate = captureFile("openssl", ["rand", "-hex", "16"]);
    if (!passwords.includes(candidate)) passwords.push(candidate);
  }
  return passwords;
}

/** Disposable PostgreSQL carrying the seven app roles plus the `console_buck_admin`
 *  superuser that migration 0196 requires before any migration may be applied.
 *  Always `--rm`, always torn down: a previous incident left 707 orphaned
 *  volumes and filled the machine. */
function withPostgres(body) {
  const name = `console-verify-pg-${process.pid}`;
  const scratch = mkdtempSync(join(tmpdir(), "console-verify-"));
  const [
    adminPassword,
    appPassword,
    runtimePassword,
    leaveCommandPassword,
    ontologyCommandPassword,
    platformForceCommandPassword,
    buckAdminPassword,
  ] = generateDistinctPasswords(7);
  try {
    const postgresEnv = join(scratch, "postgres.env");
    writePrivateFile(postgresEnv, [
      "POSTGRES_DB=console_ci",
      "POSTGRES_USER=postgres",
      `POSTGRES_PASSWORD=${adminPassword}`,
      "",
    ].join("\n"));
    captureFile("docker", [
      "run", "-d", "--rm", "--name", name,
      "-p", "127.0.0.1:0:5432",
      "--env-file", postgresEnv,
      POSTGRES_IMAGE,
    ]);

    let ready = false;
    for (let attempt = 0; attempt < 60; attempt += 1) {
      const result = spawnSync(
        "docker",
        ["exec", name, "pg_isready", "-U", "postgres", "-d", "console_ci"],
        { stdio: "ignore" },
      );
      if (result.status === 0) {
        ready = true;
        break;
      }
      spawnSync("sleep", ["1"], { stdio: "ignore" });
    }
    if (!ready) throw new Error(`PostgreSQL container ${name} did not become ready`);

    const portOutput = captureFile("docker", ["port", name, "5432/tcp"]);
    const port = portOutput.split(/\r?\n/).map((line) => line.match(/:(\d+)$/)?.[1]).find(Boolean);
    if (!port) throw new Error(`could not parse PostgreSQL port from: ${portOutput}`);

    captureFile("docker", ["cp", "ops/postgres-reconcile-topology.sh", `${name}:/topology.sh`]);
    const topologyEnv = join(scratch, "topology.env");
    writePrivateFile(topologyEnv, topologyEnvContents({
      admin: adminPassword,
      app: appPassword,
      runtime: runtimePassword,
      leaveCommand: leaveCommandPassword,
      ontologyCommand: ontologyCommandPassword,
      platformForce: platformForceCommandPassword,
    }));
    captureFile("docker", ["exec", "--env-file", topologyEnv, name, "bash", "/topology.sh"]);

    // The password reaches psql through a mode-0600 file, never argv.
    const sql = join(scratch, "buck-admin.sql");
    writePrivateFile(sql, `CREATE ROLE console_buck_admin SUPERUSER LOGIN PASSWORD '${buckAdminPassword}';\n`);
    captureFile("docker", ["cp", sql, `${name}:/buck-admin.sql`]);
    const adminEnv = join(scratch, "admin.env");
    writePrivateFile(adminEnv, `PGPASSWORD=${adminPassword}\n`);
    captureFile("docker", [
      "exec", "--env-file", adminEnv, name,
      "psql", "-U", "postgres", "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-f", "/buck-admin.sql",
    ]);

    const buckAdminUrl = `postgres://console_buck_admin:${buckAdminPassword}@127.0.0.1:${port}/console_ci?${BOOTSTRAP_GUC}`;
    return body({
      DATABASE_URL: buckAdminUrl,
      CONSOLE_BUCK_ADMIN_DATABASE_URL: buckAdminUrl,
    });
  } finally {
    spawnSync("docker", ["rm", "-f", name], { stdio: "ignore" });
    rmSync(scratch, { recursive: true, force: true });
  }
}

function main() {
  const requested = process.argv[2] ?? "fast";
  if (!["fast", "db", "all"].includes(requested)) {
    console.error("usage: node scripts/verify.mjs [fast|db|all]");
    return 2;
  }
  const steps = assertPlanCoversCi();
  const wanted = requested === "all" ? ["fast", "db"] : [requested];

  const skipped = [];
  const failures = [];
  const baseEnv = { ...process.env, SQLX_OFFLINE: "true" };

  // preflight and repo-gates both run `check:foundation-gates` and
  // `check:package-lock`. Same command, same cwd -- run it once. Keyed on the
  // command, not the step name, so a future divergence still runs both.
  const ran = new Set();

  const execute = (dbEnv) => {
    for (const step of steps) {
      const plan = PLAN.get(step.name);
      if (plan.tier === "ci-only") {
        skipped.push(`${step.name} -- ${plan.why}`);
        continue;
      }
      if (!wanted.includes(plan.tier)) continue;
      // PostgreSQL provisioning is setup, not a check; its local equivalent ran
      // before the loop in withPostgres.
      if (plan.provision) continue;
      const command = plan.run ?? step.run;
      const key = `${step.cwd} ${command}`;
      if (ran.has(key)) continue;
      ran.add(key);
      console.log(`\n── [${plan.tier}] ${step.name}  (cwd: ${step.cwd})`);
      const status = run(command, { ...baseEnv, ...(dbEnv ?? {}) }, step.cwd);
      if (status !== 0) failures.push(step.name);
    }
  };

  if (wanted.includes("db")) {
    withPostgres((dbEnv) => execute(dbEnv));
  } else {
    execute(undefined);
  }

  if (skipped.length) {
    console.log(`\nNot mirrored locally (${skipped.length}):`);
    for (const line of skipped) console.log(`  • ${line}`);
  }
  if (failures.length) {
    console.error(`\nFAILED (${failures.length}):`);
    for (const name of failures) console.error(`  ✗ ${name}`);
    return 1;
  }
  console.log(`\nverify(${requested}) passed.`);
  return 0;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    process.exit(main());
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
