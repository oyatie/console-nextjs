#!/usr/bin/env node
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
import {
  MAX_UNMEASURED_SHARE,
  SHARD_ORDER,
  balanceSummary,
  entriesForShard,
  packPackages,
  packageWeights,
  partitionByDuration,
  partitionFailures,
} from "./postgres-partition.mjs";

const e = (name, pkg, seconds, inWorkflow = true) => ({
  name,
  package: pkg,
  measured_seconds: seconds,
  in_workflow_postgres_job: inWorkflow,
});

test("postgres-partition", async (t) => {
  await t.test("sums measured seconds per package", () => {
    const { weights } = packageWeights([e("a", "p", 10), e("b", "p", 5), e("c", "q", 3)]);
    assert.equal(weights.get("p"), 15);
    assert.equal(weights.get("q"), 3);
  });

  await t.test("ignores entries outside the workflow set", () => {
    const { weights } = packageWeights([e("a", "p", 10), e("b", "q", 99, false)]);
    assert.equal(weights.has("q"), false);
  });

  await t.test("reports unmeasured entries and never treats them as free", () => {
    const { weights, unmeasured } = packageWeights([
      e("good", "p", 10),
      { name: "nope", package: "q", in_workflow_postgres_job: true },
      { name: "nan", package: "r", measured_seconds: "12", in_workflow_postgres_job: true },
      { name: "neg", package: "s", measured_seconds: -1, in_workflow_postgres_job: true },
    ]);
    assert.deepEqual(unmeasured, ["nan", "neg", "nope"]);
    // Placed at the imputed global mean, never 0: a free entry lands in the
    // lightest bin, which is exactly where a heavy newcomer hurts most.
    for (const pkg of ["q", "r", "s"]) assert.equal(weights.get(pkg), 10);
  });

  await t.test("a new test is imputed from its own package, not the global mean", () => {
    // A new suite usually resembles the suite it joins, so its siblings are a
    // better estimate than the repo-wide average.
    const { weights, unmeasured, imputedSeconds } = packageWeights([
      e("sib-a", "heavy", 100),
      e("sib-b", "heavy", 200),
      e("elsewhere", "light", 1),
      { name: "brand-new", package: "heavy", in_workflow_postgres_job: true },
    ]);
    assert.deepEqual(unmeasured, ["brand-new"]);
    assert.equal(imputedSeconds, 150, "mean of heavy's measured siblings, not of everything");
    assert.equal(weights.get("heavy"), 450);
  });

  await t.test("a new package with no measured sibling falls back to the global mean", () => {
    const { weights, imputedSeconds } = packageWeights([
      e("a", "p", 10),
      e("b", "q", 30),
      { name: "orphan", package: "brand-new-pkg", in_workflow_postgres_job: true },
    ]);
    assert.equal(imputedSeconds, 20);
    assert.equal(weights.get("brand-new-pkg"), 20);
  });

  await t.test("an unmeasured entry never skews the mean used to weigh it", () => {
    // Imputation bases are computed over measured entries only. If an imputed
    // value fed back into the mean, adding N new tests would drag the estimate
    // toward whatever the first one happened to get.
    const one = packageWeights([e("a", "p", 10), e("b", "p", 30),
      { name: "n1", package: "p", in_workflow_postgres_job: true }]);
    const two = packageWeights([e("a", "p", 10), e("b", "p", 30),
      { name: "n1", package: "p", in_workflow_postgres_job: true },
      { name: "n2", package: "p", in_workflow_postgres_job: true }]);
    assert.equal(one.imputedSeconds, 20, "one newcomer at the sibling mean");
    assert.equal(two.imputedSeconds, 40, "two newcomers, each still at the same mean");
  });

  await t.test("LPT beats the naive split it replaces", () => {
    const bins = packPackages(new Map([["a", 100], ["b", 60], ["c", 40]]), 2);
    assert.deepEqual(bins.map((b) => b.seconds).sort((x, y) => y - x), [100, 100]);
  });

  await t.test("packing is deterministic and sorted", () => {
    const w = new Map([["d", 10], ["a", 10], ["c", 10], ["b", 10]]);
    const once = packPackages(w, 2).map((b) => b.packages.join(","));
    const twice = packPackages(w, 2).map((b) => b.packages.join(","));
    assert.deepEqual(once, twice, "an unstable order would invalidate every cached target dir");
    assert.deepEqual(once, ["a,c", "b,d"]);
  });

  await t.test("conserves every package and the total weight", () => {
    const w = new Map([["a", 7], ["b", 3], ["c", 11], ["d", 2], ["e", 5]]);
    const bins = packPackages(w, 3);
    assert.deepEqual(bins.flatMap((b) => b.packages).sort(), ["a", "b", "c", "d", "e"]);
    assert.equal(bins.reduce((s, b) => s + b.seconds, 0), 28);
  });

  await t.test("rejects a nonsense shard count", () => {
    for (const bad of [0, -1, 2.5, "x", null]) {
      assert.throws(() => packPackages(new Map([["a", 1]]), bad), /positive integer/);
    }
  });

  await t.test("assigns every package to exactly one shard", () => {
    const entries = [e("a", "p", 10), e("b", "q", 5), e("c", "r", 1)];
    const { assignment, bins } = partitionByDuration(entries, 2);
    assert.equal(assignment.size, 3);
    const all = bins.flatMap((b) => b.packages);
    assert.equal(new Set(all).size, all.length, "a package in two shards would be compiled twice");
  });

  await t.test("a package never splits across shards", () => {
    // Two heavy entries of the SAME package must stay together, even when
    // splitting them would balance better -- that is the compile-duplication
    // this scheme exists to avoid.
    const { assignment } = partitionByDuration([e("a", "heavy", 100), e("b", "heavy", 100)], 2);
    assert.equal(assignment.size, 1);
    assert.equal(assignment.get("heavy"), 0);
  });

  await t.test("fails closed on an empty shard", () => {
    const failures = partitionFailures([e("a", "p", 10)], 3);
    assert.ok(
      failures.some((f) => /shard \d+ is empty/.test(f)),
      `expected an empty-shard failure, got ${JSON.stringify(failures)}`,
    );
  });

  await t.test("a whole map with no measurement at all fails closed", () => {
    const failures = partitionFailures(
      [{ name: "x", package: "p", in_workflow_postgres_job: true }],
      1,
    );
    assert.ok(
      failures.some((f) => /no basis to impute from/.test(f)),
      `expected an imputation-basis failure, got ${JSON.stringify(failures)}`,
    );
  });

  await t.test("ONE new test does not fail the partition", () => {
    // The regression this guards. Weights are harvested from a finished CI log,
    // so a test that has never run cannot have a measurement -- and it cannot
    // run until its PR is green. Failing closed here made every new PostgreSQL
    // test unmergeable.
    const entries = Array.from({ length: 20 }, (_, i) => e(`m${i}`, `pkg${i % 5}`, 10 + i));
    entries.push({ name: "brand-new", package: "pkg0", in_workflow_postgres_job: true });
    assert.deepEqual(partitionFailures(entries, 5), []);
  });

  await t.test("a package that keeps NO measurement fails closed", () => {
    // The hole the entry-count share guard cannot close. Cost is seconds and the
    // map's seconds are wildly non-uniform, so a share of ENTRIES cannot bound
    // the error a lost measurement introduces: on the real map, dropping the
    // single 318.8s writer-ownership-canonical-census-pg (sole member of its
    // package, 0.48% of entries) imputed it at 14.3s from the global mean and
    // reported "partition ok" while its shard really ran 904.1s against a
    // planned 599.6s. Within a package the sibling mean is a real estimate;
    // across packages it is a guess with no ceiling, so that is what is refused.
    const entries = Array.from({ length: 40 }, (_, i) => e(`m${i}`, `pkg${i % 5}`, 10));
    entries.push({ name: "lone", package: "solo-pkg", in_workflow_postgres_job: true });
    const failures = partitionFailures(entries, 5);
    assert.ok(
      failures.some((f) => /no measured entry at all/.test(f)),
      `expected an unmeasured-package failure, got ${JSON.stringify(failures)}`,
    );
    assert.ok(failures.some((f) => /solo-pkg/.test(f)), "the offending package must be named");
  });

  await t.test("a new test in a package that still has measured siblings passes", () => {
    // The case imputation exists to allow must keep working: this is exactly
    // #802's shape, one new suite joining a package full of measured ones.
    const entries = Array.from({ length: 40 }, (_, i) => e(`m${i}`, `pkg${i % 5}`, 10));
    entries.push({ name: "brand-new", package: "pkg0", in_workflow_postgres_job: true });
    assert.deepEqual(partitionFailures(entries, 5), []);
  });

  await t.test("packageWeights names the packages that kept no measurement", () => {
    const { unmeasuredPackages } = packageWeights([
      e("a", "measured-pkg", 10),
      { name: "n", package: "measured-pkg", in_workflow_postgres_job: true },
      { name: "x", package: "blind-pkg", in_workflow_postgres_job: true },
    ]);
    assert.deepEqual(unmeasuredPackages, ["blind-pkg"],
      "a package with a measured sibling is bounded; one without is not");
  });

  await t.test("a drifted map still fails closed", () => {
    // The case the guard was actually for: entries that silently lost weights
    // they once had. Distinguished from "new test" by share, not by kind.
    const entries = Array.from({ length: 10 }, (_, i) => e(`m${i}`, `pkg${i % 5}`, 10));
    for (let i = 0; i < 5; i += 1) {
      entries.push({ name: `lost${i}`, package: `pkg${i}`, in_workflow_postgres_job: true });
    }
    const failures = partitionFailures(entries, 5);
    assert.ok(
      failures.some((f) => /has drifted/.test(f)),
      `expected a drift failure, got ${JSON.stringify(failures)}`,
    );
  });

  await t.test("the drift threshold is the documented share", () => {
    assert.equal(MAX_UNMEASURED_SHARE, 0.1);
    const atLimit = Array.from({ length: 10 }, (_, i) =>
      i === 0 ? { name: "u", package: "p0", in_workflow_postgres_job: true } : e(`m${i}`, `p${i % 3}`, 10));
    // exactly 10% is allowed; the guard fires above the limit, not at it
    assert.deepEqual(partitionFailures(atLimit, 3), []);
  });

  await t.test("fails closed on an entry with no package", () => {
    const failures = partitionFailures(
      [e("a", "p", 5), { name: "orphan", in_workflow_postgres_job: true, measured_seconds: 1 }],
      1,
    );
    assert.ok(failures.some((f) => /not partitioned/.test(f)));
  });

  await t.test("a healthy partition reports no failures", () => {
    const entries = [e("a", "p", 10), e("b", "q", 9), e("c", "r", 8), e("d", "s", 7)];
    assert.deepEqual(partitionFailures(entries, 2), []);
  });

  await t.test("SHARD_ORDER matches the ci.yml job ids the harness selects by", () => {
    assert.deepEqual([...SHARD_ORDER], ["app", "platform", "ontology", "domain-a", "domain-b"]);
  });

  await t.test("entriesForShard partitions the real map completely and disjointly", () => {
    const map = JSON.parse(
      readFileSync(new URL("./postgres-cargo-map.json", import.meta.url), "utf8"),
    );
    const workflow = (map.entries ?? []).filter((e) => e.in_workflow_postgres_job);
    const seen = new Set();
    let total = 0;
    for (const id of SHARD_ORDER) {
      for (const entry of entriesForShard(map.entries ?? [], id)) {
        assert.equal(seen.has(entry.name), false, `${entry.name} appears in two shards`);
        seen.add(entry.name);
        total += 1;
      }
    }
    // Every workflow target runs exactly once. A shard scheme that drops or
    // duplicates targets is the false green this partitioner replaces.
    assert.equal(total, workflow.length);
    assert.equal(seen.size, workflow.length);
  });

  await t.test("an empty --only selection keeps the phrase the harness gate asserts", () => {
    // backend/ci/gates/writer-ownership/tests/census_executes_against_postgres.rs
    // (cargo_needs_postgres_harness_executes_the_enforcement) runs the harness with
    // a deliberately-absent --only name and asserts the log contains
    // "no map entries selected" -- that is how it proves canonical enforcement runs
    // BEFORE target selection. Wiring this partitioner into the harness replaced
    // that code path, so the phrase is now this module's contract to keep.
    const { execFileSync } = require("node:child_process");
    const cli = fileURLToPath(new URL("./postgres-partition.mjs", import.meta.url));
    let stderr = "";
    let status = 0;
    try {
      execFileSync(process.execPath, [cli, "--emit-shard=", "--only=console-canonical-wiring-probe"], {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
      });
    } catch (error) {
      status = error.status;
      stderr = String(error.stderr ?? "");
    }
    assert.equal(status, 1, "an unmatched --only must fail closed");
    assert.match(stderr, /no map entries selected/);
  });

  await t.test("entriesForShard rejects an unknown shard id", () => {
    assert.throws(() => entriesForShard([], "nope"), /unknown shard id nope/);
  });

  await t.test("balanceSummary reports the spread that matters", () => {
    const s = balanceSummary([{ seconds: 100 }, { seconds: 50 }]);
    assert.equal(s.max, 100);
    assert.equal(s.min, 50);
    assert.equal(s.spread, 2);
    assert.equal(s.total, 150);
  });

  await t.test("reproduces the measured rebalance on real observed weights", () => {
    // The five shards as actually observed on run 32115833327. Packing the same
    // work by duration must beat the 877.1s critical path it replaces.
    const observed = new Map([
      ["console-app", 552.2],
      ["console-gate-writer-ownership", 318.8],
      ["console-ontology-canonical-adapter-postgres", 177.0],
      ["console-ontology-rest", 161.4],
      ["console-platform-db", 154.0],
    ]);
    const bins = packPackages(observed, 5);
    const { max } = balanceSummary(bins);
    assert.equal(max, 552.2, "console-app alone is the floor at this shard count");
    assert.ok(max < 877.1, "must improve on the observed critical path");
  });
});
