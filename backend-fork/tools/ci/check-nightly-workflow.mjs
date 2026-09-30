#!/usr/bin/env node
/**
 * Lock the Nightly workflow's contract.
 *
 * `dev-up-smoke` moved out of ci.yml, which means it also moved out of
 * `check-ci-preflight.mjs` -- and that mirror was the only thing holding its
 * step order, action pins, and failure semantics in place. Moving a tier-2 job
 * to a tier-2 schedule is a deliberate governance choice; letting its contract
 * silently evaporate on the way is not. This is the replacement lock.
 *
 * It is deliberately smaller than the CI mirror. Nightly proofs do not gate a
 * merge, so the property that matters is "this job still really runs what it
 * claims to run", not the full injection-surface analysis ci.yml needs.
 */
import { readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import yaml from "js-yaml";

const ACTION_SHA = /^[^@]+@[0-9a-f]{40}$/;
const LOCAL_ACTION = /^\.\/\.github\/actions\//;

/**
 * Ordered contract for the nightly dev-up smoke. `run` is the exact command;
 * `uses` marks a setup action whose identity must stay SHA-pinned.
 */
export const NIGHTLY_DEV_UP_STEPS = Object.freeze([
  { name: "Checkout", uses: true },
  { name: "dev-up compose contract unit test", run: "node --test scripts/dev-up-compose.test.mjs" },
  { name: "Install pinned DotSlash runtime", run: "tools/buck/install_dotslash.sh" },
  { name: "Free runner disk for Rust backend", uses: true },
  { name: "Install Rust toolchain (pinned via rust-toolchain.toml)", uses: true },
  { name: "Set up Node.js", uses: true },
  { name: "PostgreSQL topology integration regression", run: "ops/postgres-topology.integration.test.sh" },
  {
    name: "dev-up bootstrap (compose deps + migrate + backend readyz)",
    run: "node scripts/dev-up.mjs bootstrap",
  },
  {
    name: "Confirm /readyz reachable",
    run: 'curl -fsS "http://127.0.0.1:${CONSOLE_DEV_HTTP_PORT:-8090}/readyz"',
  },
  { name: "dev-up down", run: "node scripts/dev-up.mjs down" },
]);

/**
 * @param {string} text raw nightly.yml
 * @returns {string[]} failures (empty = ok)
 */
export function evaluateNightlyWorkflow(text) {
  const failures = [];
  let model;
  try {
    model = yaml.load(text);
  } catch (error) {
    return [`nightly.yml is not parseable YAML: ${error.message}`];
  }
  if (!model || typeof model !== "object") return ["nightly.yml must be a mapping"];

  // `on` is parsed as boolean true by YAML 1.1 unless quoted; accept both.
  const triggers = model.on ?? model[true];
  if (!triggers || typeof triggers !== "object") {
    failures.push("nightly.yml must declare triggers");
  } else {
    // The schedule is the decay backstop; push-on-dev is what keeps detection
    // at minutes rather than up to 24 hours. Losing either silently changes the
    // risk this move was justified on.
    if (!Array.isArray(triggers.schedule) || triggers.schedule.length === 0) {
      failures.push("Nightly must keep its schedule trigger");
    }
    const branches = triggers.push?.branches;
    if (!Array.isArray(branches) || !branches.includes("dev")) {
      failures.push("Nightly must run on push to dev so a break is caught within minutes");
    }
  }

  const job = model.jobs?.["dev-up-smoke"];
  if (!job || typeof job !== "object") {
    failures.push("Nightly must define the dev-up-smoke job");
    return failures;
  }
  if (Object.hasOwn(job, "continue-on-error")) {
    failures.push("dev-up-smoke must not define job-level continue-on-error");
  }

  const steps = Array.isArray(job.steps) ? job.steps : [];
  const actualNames = steps.map((step) => step?.name);
  const expectedNames = NIGHTLY_DEV_UP_STEPS.map((step) => step.name);
  if (JSON.stringify(actualNames) !== JSON.stringify(expectedNames)) {
    failures.push(
      "dev-up-smoke must preserve its exact ordered step list; "
      + `got ${JSON.stringify(actualNames)}`,
    );
    return failures;
  }

  for (const [index, contract] of NIGHTLY_DEV_UP_STEPS.entries()) {
    const step = steps[index];
    if (Object.hasOwn(step, "continue-on-error")) {
      failures.push(`${contract.name} must not be soft-failed with continue-on-error`);
    }
    if (contract.run !== undefined) {
      if (String(step.run ?? "").trim() !== contract.run) {
        failures.push(`${contract.name} must run exactly: ${contract.run}`);
      }
    }
    if (contract.uses) {
      const uses = String(step.uses ?? "");
      if (!ACTION_SHA.test(uses) && !LOCAL_ACTION.test(uses)) {
        failures.push(`${contract.name} must use a SHA-pinned or local action, got "${uses}"`);
      }
    }
  }

  // The teardown must run even when the bootstrap failed, or a red nightly
  // leaves containers behind on the runner.
  const down = steps[steps.length - 1];
  if (!/always\(\)/.test(String(down?.if ?? ""))) {
    failures.push("dev-up down must be conditioned on always() so teardown survives a failure");
  }

  return failures;
}

const isMain = process.argv[1] && process.argv[1].endsWith("check-nightly-workflow.mjs");
if (isMain) {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
  const path = process.argv[2]
    ? resolve(process.argv[2])
    : resolve(root, ".github/workflows/nightly.yml");
  const failures = evaluateNightlyWorkflow(readFileSync(path, "utf8"));
  if (failures.length) {
    console.error(failures.join("\n"));
    process.exit(1);
  }
  console.log(`nightly workflow contract OK (${NIGHTLY_DEV_UP_STEPS.length} locked steps)`);
}
