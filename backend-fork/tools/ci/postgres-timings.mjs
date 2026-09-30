#!/usr/bin/env node
/**
 * Harvest `cargo-postgres-timing:` lines and re-pack the PostgreSQL shards by
 * measured duration instead of entry count.
 *
 * `postgres-shard.mjs` currently bin-packs domain packages by how many workflow
 * entries each owns, which assumes every entry costs the same. Measured
 * 2026-08-18 on run 32097786166 the five shards ran 1407/1094/1062/974/845s --
 * a 1.66x spread, with the slowest shard alone accounting for 92% of the run's
 * 1534s wall clock. Entry count is the wrong weight; seconds are the right one.
 *
 * Input is the raw job log (or several concatenated). Usage:
 *   gh api .../jobs/<id>/logs | node tools/ci/postgres-timings.mjs --shards 5
 *   node tools/ci/postgres-timings.mjs --shards 8 log-a.txt log-b.txt
 *
 * Pure functions are exported for the adjacent test; the CLI is a thin wrapper.
 */

const TIMING_PREFIX = "cargo-postgres-timing:";

/**
 * Extract timing records from arbitrary log text.
 *
 * Log lines carry a leading ISO timestamp and may carry ANSI colour, so the
 * prefix is located anywhere in the line rather than anchored. A malformed or
 * truncated JSON payload is skipped rather than throwing: logs get truncated at
 * the tail, and one unusable line must not discard the other 43.
 *
 * @param {string} text
 * @returns {Array<{name:string,package:string,seconds:number,status:string,shard:string}>}
 */
export function parseTimingLines(text) {
  const out = [];
  for (const raw of String(text ?? "").split("\n")) {
    const at = raw.indexOf(TIMING_PREFIX);
    if (at < 0) continue;
    const payload = raw.slice(at + TIMING_PREFIX.length).trim();
    let row;
    try {
      row = JSON.parse(payload);
    } catch {
      continue;
    }
    if (!row || typeof row !== "object") continue;
    if (typeof row.seconds !== "number" || !Number.isFinite(row.seconds)) continue;
    if (typeof row.name !== "string" || row.name === "") continue;
    out.push({
      name: row.name,
      package: typeof row.package === "string" ? row.package : "",
      seconds: row.seconds,
      status: row.status === "fail" ? "fail" : "pass",
      shard: typeof row.shard === "string" ? row.shard : "",
    });
  }
  return out;
}

/**
 * Sum measured seconds per package.
 *
 * Failed invocations are excluded from the weights but counted separately: a
 * suite that aborted early is fast for the wrong reason, and packing on that
 * number would under-weight the package exactly when it starts passing again.
 *
 * @param {Array<{package:string,seconds:number,status:string}>} rows
 * @returns {{weights: Map<string, number>, excluded: number}}
 */
export function aggregateByPackage(rows) {
  const weights = new Map();
  let excluded = 0;
  for (const row of rows ?? []) {
    if (row.status === "fail") {
      excluded += 1;
      continue;
    }
    const key = row.package || row.name;
    weights.set(key, (weights.get(key) || 0) + row.seconds);
  }
  return { weights, excluded };
}

/**
 * Greedy longest-processing-time bin-pack: sort descending by weight, assign
 * each package to the lightest bin so far. Ties break on name so the assignment
 * is deterministic across runs -- a shard map that reshuffles on equal input
 * would invalidate every cached target dir it touches.
 *
 * LPT is within 4/3 of optimal, which is far inside the noise of CI timing.
 *
 * @param {Map<string, number>|Array<[string, number]>} weights
 * @param {number} shardCount
 * @returns {Array<{index:number, packages:string[], seconds:number}>}
 */
export function packByDuration(weights, shardCount) {
  const count = Number(shardCount);
  if (!Number.isInteger(count) || count < 1) {
    throw new Error(`shard count must be a positive integer, got ${shardCount}`);
  }
  const ordered = [...(weights instanceof Map ? weights.entries() : weights)]
    .sort((a, b) => (b[1] !== a[1] ? b[1] - a[1] : a[0].localeCompare(b[0])));
  const bins = Array.from({ length: count }, (_, index) => ({
    index,
    packages: [],
    seconds: 0,
  }));
  for (const [pkg, seconds] of ordered) {
    // Lightest bin; ties go to the lowest index for determinism.
    let target = bins[0];
    for (const bin of bins) {
      if (bin.seconds < target.seconds) target = bin;
    }
    target.packages.push(pkg);
    target.seconds += seconds;
  }
  return bins;
}

/**
 * Observed per-shard totals, from the `shard` field the harness stamps.
 * @param {Array<{shard:string,seconds:number,status:string}>} rows
 * @returns {Map<string, number>}
 */
export function observedShardTotals(rows) {
  const totals = new Map();
  for (const row of rows ?? []) {
    const key = row.shard || "(unattributed)";
    totals.set(key, (totals.get(key) || 0) + row.seconds);
  }
  return totals;
}

/**
 * @param {string} text raw log(s)
 * @param {number} shardCount
 * @returns {string} human-readable report
 */
export function report(text, shardCount) {
  const rows = parseTimingLines(text);
  if (rows.length === 0) {
    return "no `cargo-postgres-timing:` lines found -- is the harness change deployed?";
  }
  const { weights, excluded } = aggregateByPackage(rows);
  const bins = packByDuration(weights, shardCount);
  const observed = observedShardTotals(rows);

  const lines = [];
  lines.push(`parsed ${rows.length} invocations across ${weights.size} packages`);
  if (excluded) lines.push(`excluded ${excluded} failed invocation(s) from the weights`);

  if (observed.size > 0) {
    lines.push("", "observed (as currently sharded):");
    const obs = [...observed.entries()].sort((a, b) => b[1] - a[1]);
    for (const [shard, seconds] of obs) {
      lines.push(`  ${shard.padEnd(12)} ${seconds.toFixed(1)}s`);
    }
    const max = obs[0][1];
    const min = obs[obs.length - 1][1];
    if (min > 0) lines.push(`  spread ${(max / min).toFixed(2)}x  max ${max.toFixed(1)}s`);
  }

  lines.push("", `proposed (${shardCount} shards, packed by duration):`);
  for (const bin of bins) {
    lines.push(`  shard-${bin.index}      ${bin.seconds.toFixed(1)}s  (${bin.packages.length} packages)`);
  }
  const proposedMax = Math.max(...bins.map((b) => b.seconds));
  const total = bins.reduce((sum, b) => sum + b.seconds, 0);
  lines.push(`  max ${proposedMax.toFixed(1)}s  perfect ${(total / shardCount).toFixed(1)}s`);

  const observedMax = observed.size ? Math.max(...observed.values()) : 0;
  if (observedMax > 0) {
    lines.push(
      "",
      `critical path ${observedMax.toFixed(1)}s -> ${proposedMax.toFixed(1)}s `
      + `(${(observedMax - proposedMax).toFixed(1)}s off the slowest shard)`,
    );
  }
  return lines.join("\n");
}

const isMain = process.argv[1] && process.argv[1].endsWith("postgres-timings.mjs");
if (isMain) {
  const args = process.argv.slice(2);
  let shards = 5;
  const files = [];
  for (let i = 0; i < args.length; i += 1) {
    if (args[i] === "--shards") {
      shards = Number(args[i + 1]);
      i += 1;
    } else if (args[i].startsWith("--shards=")) {
      shards = Number(args[i].slice("--shards=".length));
    } else {
      files.push(args[i]);
    }
  }
  const { readFileSync } = await import("node:fs");
  const text = files.length
    ? files.map((f) => readFileSync(f, "utf8")).join("\n")
    : readFileSync(0, "utf8");
  console.log(report(text, shards));
}
