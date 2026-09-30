#!/usr/bin/env node
/**
 * Classify a GitHub Actions run/job as ops.gha-infra-flake vs product failure.
 *
 * Process gap: "PR all red" was treated as product CI failure during Actions
 * major_outage when every job died in Set up job (action download). That is
 * not a product red and must not trigger tip restack or code "fixes".
 *
 * Usage:
 *   node tools/ci/classify-ci-failure.mjs --run-id 123
 *   node tools/ci/classify-ci-failure.mjs --job-id 456
 *   node tools/ci/classify-ci-failure.mjs --log-file /tmp/log.txt
 * Exit 0 always when classification succeeds; --check-product-only exits 1 if product.
 */
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const INFRA_NEEDLES = [
  "Failed to resolve action download info",
  "Service Unavailable",
  "github-launch service unavailable",
  "Internal Server Error",
  "The job was not started because",
  "Runner failed to start",
];

export function classifyLogText(log) {
  const text = log || "";
  const infraHits = INFRA_NEEDLES.filter((n) => text.includes(n));
  // Product signals: actual test/compile after setup
  const productNeedles = [
    "error[E",
    "FAILED",
    "test result: FAILED",
    "assertion failed",
    "clippy::",
    "cargo test",
  ];
  // If only setup failure and no product body, infra
  const setupOnly =
    infraHits.length > 0 &&
    !/##\[group\]Run /.test(text) &&
    !/Compiling |error\[E\d+\]|test result:/.test(text);

  const hasProduct =
    /test result: FAILED|error\[E\d+\]|assertion `/.test(text) ||
    (productNeedles.some((n) => text.includes(n)) && infraHits.length === 0);

  if (setupOnly || (infraHits.length > 0 && !hasProduct)) {
    return {
      class_id: "ops.gha-infra-flake",
      product: false,
      infra_hits: infraHits,
      action: "wait for Actions healthy then gh run rerun — do not restack or product-change",
    };
  }
  if (hasProduct) {
    return {
      class_id: "product.ci-failure",
      product: true,
      infra_hits: infraHits,
      action: "run npm run admit locally; fix product; do not blind-rerun forever",
    };
  }
  return {
    class_id: "ops.unknown-ci-failure",
    product: null,
    infra_hits: infraHits,
    action: "inspect full logs; prefer npm run admit before tip surgery",
  };
}

function fetchJobLog(jobId) {
  const out = execFileSync(
    "gh",
    ["api", `repos/{owner}/{repo}/actions/jobs/${jobId}/logs`],
    { encoding: "buffer", maxBuffer: 20 * 1024 * 1024 },
  );
  return out.toString("utf8");
}

function parseArgs(argv) {
  let runId = null;
  let jobId = null;
  let logFile = null;
  let checkProductOnly = false;
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === "--run-id") runId = argv[++i];
    else if (a === "--job-id") jobId = argv[++i];
    else if (a === "--log-file") logFile = argv[++i];
    else if (a === "--check-product-only") checkProductOnly = true;
    else if (a === "--help") return { help: true };
    else throw new Error(`unknown arg ${a}`);
  }
  return { help: false, runId, jobId, logFile, checkProductOnly };
}

export function main(argv = process.argv.slice(2)) {
  const opts = parseArgs(argv);
  if (opts.help) {
    console.log(
      "Usage: node tools/ci/classify-ci-failure.mjs (--job-id ID | --log-file PATH) [--check-product-only]",
    );
    return 0;
  }
  let log = "";
  if (opts.logFile) {
    log = readFileSync(opts.logFile, "utf8");
  } else if (opts.jobId) {
    log = fetchJobLog(opts.jobId);
  } else if (opts.runId) {
    // Prefer first failed job
    const jobs = JSON.parse(
      execFileSync("gh", ["run", "view", opts.runId, "--json", "jobs"], {
        encoding: "utf8",
      }),
    );
    const fail = (jobs.jobs || []).find((j) => j.conclusion === "failure");
    if (!fail) {
      console.log(JSON.stringify({ class_id: "ops.no-failed-jobs", product: false }, null, 2));
      return 0;
    }
    log = fetchJobLog(String(fail.databaseId));
  } else {
    console.error("need --job-id, --run-id, or --log-file");
    return 2;
  }

  const result = classifyLogText(log);
  console.log(JSON.stringify(result, null, 2));
  if (opts.checkProductOnly && result.product === true) return 1;
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  process.exit(main());
}
