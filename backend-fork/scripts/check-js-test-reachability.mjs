#!/usr/bin/env node
/**
 * S3′ — candidate-bound JavaScript .test.mjs reachability ratchet.
 *
 * Unlike scripts/dev/mjs-test-reachability.mjs (diagnostic, reads origin/main),
 * this gate:
 * - enumerates suites via `git ls-files` at HEAD (candidate-bound)
 * - expands npm scripts from the candidate package.json to a fixed point
 * - counts a suite as reached only by exact path match in executable surface
 * - treats basename-only hits as diagnostic (stderr), never as green coverage
 * - fails closed when a workflow-invoked npm script cannot be resolved
 * - ratchets unexplained dark suites against docs/program/js-test-reachability-baseline.json
 */

import { execFileSync } from "node:child_process";
import { readFileSync, existsSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import process from "node:process";

import { executableWorkflowCommands } from "./lib/ci-workflow-executables.mjs";
import { sweepReachableText } from "../tools/ci/gate-sweep.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const baselinePath = "docs/program/js-test-reachability-baseline.json";
const jsonMode = process.argv.includes("--json");

function gitLsFiles() {
  return execFileSync("git", ["-C", root, "ls-files", "-z"], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  })
    .split("\0")
    .filter(Boolean);
}

function loadJson(rel) {
  return JSON.parse(readFileSync(join(root, rel), "utf8"));
}

function expandInvokedNpmScripts(runText, scripts) {
  const invoked = new Set();
  for (const name of Object.keys(scripts)) {
    const re = new RegExp(
      `npm\\s+(?:run\\s+)?${name.replace(/[.*+?^${}()|[\\]\\\\]/g, "\\$&")}(?![\\w:-])`,
    );
    if (re.test(runText)) invoked.add(name);
  }
  let changed = true;
  while (changed) {
    changed = false;
    for (const name of [...invoked]) {
      const body = scripts[name] ?? "";
      for (const other of Object.keys(scripts)) {
        if (invoked.has(other)) continue;
        const re = new RegExp(
          `npm\\s+(?:run\\s+)?${other.replace(/[.*+?^${}()|[\\]\\\\]/g, "\\$&")}(?![\\w:-])`,
        );
        if (re.test(body)) {
          invoked.add(other);
          changed = true;
        }
      }
    }
  }
  return invoked;
}

function resolveNpmInvocations(runText, scripts) {
  const failures = [];
  const re = /npm\s+(?:run\s+)?([A-Za-z0-9:_-]+)/g;
  let match;
  while ((match = re.exec(runText)) !== null) {
    const name = match[1];
    // ignore npm ci / npm install / npm audit etc. when not in scripts
    if (!Object.hasOwn(scripts, name)) {
      if (["ci", "install", "audit", "pack", "publish", "cache", "config", "version"].includes(name)) {
        continue;
      }
      // bare `npm test` is special
      if (name === "test" && scripts.test) continue;
      if (name === "test" && !scripts.test) {
        failures.push(`unresolvable npm script invoked by workflow surface: ${name}`);
      }
      // other unknown may be subcommands
      continue;
    }
  }
  return failures;
}

function main() {
  const failures = [];
  const files = gitLsFiles();
  const suites = files.filter((f) => f.endsWith(".test.mjs")).sort();
  const workflows = files.filter((f) => f.startsWith(".github/workflows/") && f.endsWith(".yml"));

  if (!existsSync(join(root, "package.json"))) {
    failures.push("package.json missing on candidate");
  }
  const pkg = loadJson("package.json");
  const scripts = pkg.scripts ?? {};

  let runText = "";
  for (const wf of workflows) {
    const text = readFileSync(join(root, wf), "utf8");
    try {
      const execs = executableWorkflowCommands(text);
      for (const entry of execs) {
        // Prefer the raw step body (includes `run:` lines); tokens alone lose paths.
        runText += `${entry.step ?? ""}\n${(entry.tokens ?? []).join(" ")}\n`;
      }
    } catch {
      // fall through — raw YAML still contributes npm tokens for expansion
    }
    runText += text;
  }

  const unresolvable = resolveNpmInvocations(runText, scripts);
  failures.push(...unresolvable);

  const invoked = expandInvokedNpmScripts(runText, scripts);
  // tools/ci/gate-sweep.json is a real execution path and therefore a real
  // reachability source; see sweepReachableText. Without it every suite the
  // sweep runs reports dark here.
  const expanded = runText + [...invoked].map((n) => scripts[n] ?? "").join("\n")
    + sweepReachableText(root);

  const wired = [];
  const basenameOnly = [];
  const dark = [];
  for (const suite of suites) {
    const base = suite.split("/").pop();
    const exact = expanded.includes(suite);
    const byBase = !exact && expanded.includes(base);
    if (exact) wired.push(suite);
    else if (byBase) {
      basenameOnly.push(suite);
      dark.push(suite); // basename is not coverage
    } else dark.push(suite);
  }

  if (!existsSync(join(root, baselinePath))) {
    failures.push(`${baselinePath}: missing (must ship with deferred buckets)`);
  } else {
    const doc = loadJson(baselinePath);
    if (doc.schema_version !== 1) {
      failures.push(`${baselinePath}: schema_version must be 1`);
    }
    const buckets = Object.entries(doc).filter(([k]) => k.startsWith("deferred_"));
    if (buckets.length === 0) {
      failures.push(`${baselinePath}: names no deferred_* buckets`);
    }
    const accepted = new Set();
    for (const [key, value] of buckets) {
      if (!Array.isArray(value) || !value.every((e) => typeof e === "string")) {
        failures.push(`${baselinePath}: malformed bucket ${key}`);
        continue;
      }
      for (const entry of value) accepted.add(entry);
    }
    // growth: dark not in accepted
    for (const suite of dark) {
      if (!accepted.has(suite)) {
        failures.push(
          `unregistered dark .test.mjs suite (not reached by exact path from CI executable surface): ${suite}`,
        );
      }
    }
    // decoration: accepted path not present on candidate
    for (const entry of accepted) {
      if (!suites.includes(entry)) {
        failures.push(
          `${baselinePath}: decoration — deferred suite not on candidate: ${entry}`,
        );
      }
    }
    // regression: suite was deferred but is now exact-wired — must leave baseline
    for (const suite of wired) {
      if (accepted.has(suite)) {
        failures.push(
          `${baselinePath}: stale deferred entry still listed but now exact-wired: ${suite}`,
        );
      }
    }
  }

  const report = {
    schema_version: 1,
    relation: "js_test_reachability",
    suite_count: suites.length,
    wired_exact: wired,
    basename_only_diagnostic: basenameOnly,
    dark,
    invoked_npm_scripts: [...invoked].sort(),
    failures,
  };

  if (jsonMode) {
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  } else {
    console.log(
      `js-test-reachability: ${suites.length} suites; exact-wired ${wired.length}; dark ${dark.length}; basename-only diagnostic ${basenameOnly.length}`,
    );
    for (const s of basenameOnly) {
      console.error(`diagnostic: basename-only hit (not coverage): ${s}`);
    }
    if (failures.length) {
      console.error(`js-test-reachability failed (${failures.length}):`);
      for (const f of failures) console.error(`- ${f}`);
    } else {
      console.log("js-test-reachability check passed");
    }
  }

  process.exit(failures.length ? 1 : 0);
}

main();
