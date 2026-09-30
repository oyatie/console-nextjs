#!/usr/bin/env node
/**
 * Fail-SLOW gate sweep: run every preflight gate, report every failure once.
 *
 * `check:ci-preflight` was an 11-link `&&` chain, so the shell stopped at the
 * first failing gate and the remaining ten never ran. A tip with three broken
 * gates therefore needed three full CI cycles to discover three problems — at
 * this repo's measured ~20 minute wall clock, an hour to learn what one sweep
 * already knew.
 *
 * The workflow LAYER was already fail-slow: 80 steps in ci.yml carry
 * `!cancelled()` and `scripts/ci-collect-failures.mjs` re-asserts the job red
 * with every failing step id. That sweep stopped at the step boundary — inside
 * one step, `&&` still short-circuited. This closes that gap with the same
 * shape, one level down.
 *
 * Red is preserved exactly: a non-zero exit from any gate still exits non-zero
 * here. The only thing that changes is how much you learn per run.
 *
 * Precedent in-tree: `tools/ci/cargo-test-runner.sh` runs a stream of cargo
 * invocations keep-going for the same reason, and its comment says it plainly —
 * "one red binary never hides the rest".
 */
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

/**
 * Load the gate list. Kept as data in a sibling JSON file so the sweep's
 * contents are diffable and a gate cannot be dropped inside a shell string
 * where no reviewer would see it.
 * @returns {Array<{id: string, run: string}>}
 */
export function loadGates(path = resolve(ROOT, "tools/ci/gate-sweep.json")) {
  const doc = JSON.parse(readFileSync(path, "utf8"));
  const gates = Array.isArray(doc?.gates) ? doc.gates : null;
  if (!gates || gates.length === 0) {
    // A sweep over zero gates would exit 0 and prove nothing.
    throw new Error(`gate-sweep: ${path} declares no gates`);
  }
  const seen = new Set();
  for (const gate of gates) {
    if (typeof gate?.id !== "string" || gate.id === "") {
      throw new Error("gate-sweep: every gate needs a non-empty id");
    }
    if (typeof gate?.run !== "string" || gate.run === "") {
      throw new Error(`gate-sweep: gate ${gate.id} has no command`);
    }
    if (seen.has(gate.id)) {
      throw new Error(`gate-sweep: duplicate gate id ${gate.id}`);
    }
    seen.add(gate.id);
  }
  return gates;
}

/**
 * Run every gate, regardless of earlier failures.
 *
 * @param {Array<{id: string, run: string}>} gates
 * @param {(cmd: string) => {status: number, stdout: string, stderr: string}} exec
 * @returns {Array<{id: string, ok: boolean, seconds: number, output: string}>}
 */
export function sweep(gates, exec) {
  const results = [];
  for (const gate of gates) {
    const started = Date.now();
    const done = exec(gate.run);
    results.push({
      id: gate.id,
      ok: done.status === 0,
      seconds: Math.round((Date.now() - started) / 100) / 10,
      output: `${done.stdout ?? ""}${done.stderr ?? ""}`,
    });
  }
  return results;
}

/** @param {Array<{id:string,ok:boolean,seconds:number,output:string}>} results */
export function report(results) {
  const lines = [];
  for (const r of results) {
    lines.push(`${r.ok ? "PASS" : "FAIL"}  ${r.id}  (${r.seconds}s)`);
  }
  const failed = results.filter((r) => !r.ok);
  lines.push("");
  lines.push(`gate-sweep: ${results.length - failed.length} passed, ${failed.length} failed`);
  if (failed.length > 0) {
    // Every failure, not just the first: the whole point of the sweep.
    for (const r of failed) {
      lines.push("");
      lines.push(`--- ${r.id} ---`);
      lines.push(r.output.trimEnd());
    }
    lines.push("");
    lines.push(`gate-sweep: FAILED ${failed.map((r) => r.id).join(", ")}`);
  }
  return lines.join("\n");
}

/**
 * The sweep's commands as one blob of text, for reachability scanners.
 *
 * `check:ci-preflight` used to name its test files inline in package.json, which
 * is where both reachability scanners look. The chain became this sweep and the
 * commands moved into a manifest, so a scanner that reads only package.json now
 * reports live suites as dark -- and the natural response is to baseline them,
 * retiring real coverage on the strength of a blind spot.
 *
 * Exported rather than duplicated: there are TWO scanners with this same blind
 * spot (tools/ci/check-mjs-dark-suites.mjs and
 * scripts/check-js-test-reachability.mjs). Patching them independently is how
 * the second one gets missed, which is exactly what happened.
 *
 * Takes a root because check-js-test-reachability validates a CANDIDATE tree,
 * not necessarily this one. A missing manifest yields "" rather than throwing:
 * these scanners must keep working against a tree that predates the sweep.
 *
 * @param {string} root
 * @returns {string}
 */
export function sweepReachableText(root) {
  try {
    const doc = JSON.parse(readFileSync(resolve(root, "tools/ci/gate-sweep.json"), "utf8"));
    return "\n" + (doc.gates ?? []).map((gate) => gate?.run ?? "").join("\n");
  } catch {
    return "";
  }
}

const isMain = process.argv[1] && process.argv[1].endsWith("gate-sweep.mjs");
if (isMain) {
  const gates = loadGates();
  const results = sweep(gates, (cmd) => {
    const done = spawnSync(cmd, {
      cwd: ROOT,
      shell: true,
      encoding: "utf8",
      env: { ...process.env },
      maxBuffer: 64 * 1024 * 1024,
    });
    return { status: done.status ?? 1, stdout: done.stdout ?? "", stderr: done.stderr ?? "" };
  });
  console.log(report(results));
  process.exit(results.every((r) => r.ok) ? 0 : 1);
}
