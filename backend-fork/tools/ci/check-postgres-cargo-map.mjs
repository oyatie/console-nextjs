#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { SHARD_ORDER, entriesForShard, partitionFailures } from "./postgres-partition.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
// Optional map path, matching the sibling `postgres-partition.mjs --check [map]`.
// Without it this gate could only ever be run against the real file, so no test
// could prove it goes red -- and an unfalsifiable gate is how the drift it now
// catches survived in the first place.
const mapPath = process.argv[2]
  ? resolve(process.argv[2])
  : resolve(root, "tools/ci/postgres-cargo-map.json");
const map = JSON.parse(readFileSync(mapPath, "utf8"));
const wf = readFileSync(resolve(root, ".github/workflows/ci.yml"), "utf8");
// S1: harness runs on facet jobs; still accept legacy monolith block for S0-only trees.
const startFacet = wf.indexOf("postgres-reachability-app:");
const startLegacy = wf.indexOf("postgres-domain-reachability:");
const start = startFacet >= 0 ? startFacet : startLegacy;
const end = wf.indexOf("\n  company-conformance:", start);
const block = start >= 0 && end > start ? wf.slice(start, end) : wf;
const failures = [];

// Package→facet partition must stay complete/disjoint (S0; used by --shard-id).
for (const msg of partitionFailures(map.entries ?? [], SHARD_ORDER.length)) {
  failures.push(`postgres-partition: ${msg}`);
}

const usesCargo = /tools\/ci\/cargo_needs_postgres\.sh\s+--workflow-only/.test(block);
const re = /\/\/tools\/buck:([a-zA-Z0-9_-]+)/g;
const needed = new Set();
let m;
while ((m = re.exec(block))) needed.add(m[1]);
const mapped = new Set(
  (map.entries ?? []).filter((e) => e.in_workflow_postgres_job).map((e) => e.name),
);

if (usesCargo) {
  if (needed.size > 0) {
    failures.push(
      `cargo harness job must not still list Buck wrappers (found ${needed.size}); map is the source of truth`,
    );
  }
  if (mapped.size < 180) {
    failures.push(`postgres-cargo-map in_workflow_postgres_job count ${mapped.size} < 180`);
  }
  for (const e of map.entries ?? []) {
    if (!e.in_workflow_postgres_job) continue;
    if (!Array.isArray(e.cargo_argv) || e.cargo_argv[0] !== "cargo") {
      failures.push(`map entry ${e.name} missing cargo argv`);
    }
  }
  for (const u of map.unmapped ?? []) {
    // unmapped wrappers must not claim workflow membership
    if (mapped.has(u.wrapper) || mapped.has(String(u.wrapper || "").replace(/^.*:/, ""))) {
      failures.push(`unmapped wrapper also marked workflow: ${u.wrapper}`);
    }
  }
} else {
  const missing = [...needed].filter((n) => !mapped.has(n)).sort();
  if (missing.length) failures.push(`workflow wrappers missing from map: ${missing.join(", ")}`);
  for (const u of map.unmapped ?? []) {
    const name = String(u.wrapper || "").replace(/^.*:/, "");
    if (needed.has(name) || needed.has(u.wrapper)) {
      failures.push(`unmapped but required by workflow: ${u.wrapper}`);
    }
  }
}

// `counts` is a hand-written block that nothing verified, so it drifted from
// the entries it describes and then reported the stale number as if measured:
// `postgres-shard.mjs --check` printed `partition ok (207 workflow targets)`
// while partitioning 209. History shows the drift is chronic, not a one-off --
// consistent at 1746bc2e/ede052d3/7b568df9, drifted by e391abb9, re-synced by
// hand at fb9ae31e, drifted again by 97a45cfc and 0da6c2fd. A number that
// describes the file it lives in must be derived from that file or checked
// against it; otherwise it is decoration that reads as evidence.
{
  const observed = {
    mapped: (map.entries ?? []).length,
    unmapped: (map.unmapped ?? []).length,
    workflow_targets: mapped.size,
    workflow_mapped: mapped.size,
    workflow_missing: 0,
  };
  const declared = map.counts ?? {};
  for (const [key, value] of Object.entries(observed)) {
    if (declared[key] !== value) {
      failures.push(
        `postgres-cargo-map counts.${key} declares ${JSON.stringify(declared[key])} `
        + `but the file contains ${value}`,
      );
    }
  }
}

if (failures.length) {
  console.error(failures.join("\n"));
  process.exit(1);
}
const parts = Object.fromEntries(
  SHARD_ORDER.map((id) => [id, entriesForShard(map.entries ?? [], id).map((e) => e.name)]),
);
const facetSummary = SHARD_ORDER.map((id) => `${id}=${parts[id].length}`).join(" ");
console.log(
  usesCargo
    ? `postgres-cargo-map OK (cargo harness; ${mapped.size} workflow entries; facets ${facetSummary})`
    : `postgres-cargo-map OK (workflow ${needed.size} wrappers fully mapped; facets ${facetSummary})`,
);
