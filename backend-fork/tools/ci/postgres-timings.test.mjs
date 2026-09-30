#!/usr/bin/env node
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  aggregateByPackage,
  observedShardTotals,
  packByDuration,
  parseTimingLines,
  report,
} from "./postgres-timings.mjs";

const line = (obj) => `2026-08-18T04:10:07.1234567Z cargo-postgres-timing: ${JSON.stringify(obj)}`;

test("postgres-timings", async (t) => {
  await t.test("parses timing lines out of timestamped log noise", () => {
    const text = [
      "2026-08-18T04:10:00.0000000Z cargo-postgres: === alpha ===",
      line({ name: "alpha", package: "pkg-a", seconds: 12.5, status: "pass", shard: "domain-a" }),
      "   Compiling serde v1.0.0",
      line({ name: "beta", package: "pkg-b", seconds: 3.25, status: "pass", shard: "domain-b" }),
    ].join("\n");
    const rows = parseTimingLines(text);
    assert.equal(rows.length, 2);
    assert.deepEqual(rows[0], {
      name: "alpha",
      package: "pkg-a",
      seconds: 12.5,
      status: "pass",
      shard: "domain-a",
    });
  });

  await t.test("survives a truncated tail instead of discarding the whole log", () => {
    const text = [
      line({ name: "alpha", package: "pkg-a", seconds: 10, status: "pass", shard: "x" }),
      "2026-08-18T04:10:07Z cargo-postgres-timing: {\"name\":\"beta\",\"seco",
    ].join("\n");
    const rows = parseTimingLines(text);
    assert.equal(rows.length, 1, "one good row must survive a malformed one");
  });

  await t.test("rejects rows without a usable duration or name", () => {
    const text = [
      line({ name: "no-seconds", package: "p", status: "pass" }),
      line({ name: "nan", package: "p", seconds: "12", status: "pass" }),
      line({ name: "", package: "p", seconds: 5, status: "pass" }),
      line({ name: "ok", package: "p", seconds: 5, status: "pass" }),
    ].join("\n");
    assert.deepEqual(parseTimingLines(text).map((r) => r.name), ["ok"]);
  });

  await t.test("ignores lines that merely mention the prefix in prose", () => {
    assert.deepEqual(parseTimingLines("see cargo-postgres-timing: for details"), []);
  });

  await t.test("excludes failed invocations from the weights but counts them", () => {
    const rows = [
      { name: "a", package: "pkg-a", seconds: 10, status: "pass" },
      { name: "b", package: "pkg-a", seconds: 999, status: "fail" },
      { name: "c", package: "pkg-b", seconds: 4, status: "pass" },
    ];
    const { weights, excluded } = aggregateByPackage(rows);
    assert.equal(excluded, 1);
    assert.equal(weights.get("pkg-a"), 10, "a failed run's duration must not be packed on");
    assert.equal(weights.get("pkg-b"), 4);
  });

  await t.test("sums multiple invocations of the same package", () => {
    const { weights } = aggregateByPackage([
      { name: "a", package: "pkg", seconds: 3, status: "pass" },
      { name: "b", package: "pkg", seconds: 4.5, status: "pass" },
    ]);
    assert.equal(weights.get("pkg"), 7.5);
  });

  await t.test("falls back to the entry name when a package is missing", () => {
    const { weights } = aggregateByPackage([
      { name: "orphan", package: "", seconds: 2, status: "pass" },
    ]);
    assert.equal(weights.get("orphan"), 2);
  });

  await t.test("LPT packing beats the naive split it replaces", () => {
    // Entry-count packing would put the two 1-entry packages together; by
    // duration the right answer is 100 alone against 60+40.
    const bins = packByDuration(new Map([["a", 100], ["b", 60], ["c", 40]]), 2);
    const totals = bins.map((b) => b.seconds).sort((x, y) => y - x);
    assert.deepEqual(totals, [100, 100]);
  });

  await t.test("packing is deterministic under equal weights", () => {
    const w = new Map([["b", 10], ["a", 10], ["d", 10], ["c", 10]]);
    const once = packByDuration(w, 2).map((b) => b.packages.join(","));
    const twice = packByDuration(w, 2).map((b) => b.packages.join(","));
    assert.deepEqual(once, twice);
    assert.deepEqual(once, ["a,c", "b,d"], "ties must break on name, not insertion order");
  });

  await t.test("packing conserves every package and the total weight", () => {
    const w = new Map([["a", 7], ["b", 3], ["c", 11], ["d", 2], ["e", 5]]);
    const bins = packByDuration(w, 3);
    const packed = bins.flatMap((b) => b.packages).sort();
    assert.deepEqual(packed, ["a", "b", "c", "d", "e"]);
    assert.equal(bins.reduce((s, b) => s + b.seconds, 0), 28);
  });

  await t.test("more shards than packages leaves empty bins rather than throwing", () => {
    const bins = packByDuration(new Map([["a", 5]]), 3);
    assert.equal(bins.length, 3);
    assert.equal(bins.filter((b) => b.packages.length === 0).length, 2);
  });

  await t.test("rejects a nonsense shard count", () => {
    for (const bad of [0, -1, 2.5, "x", null]) {
      assert.throws(() => packByDuration(new Map([["a", 1]]), bad), /positive integer/);
    }
  });

  await t.test("observed totals group by the stamped shard id", () => {
    const totals = observedShardTotals([
      { shard: "domain-a", seconds: 10, status: "pass" },
      { shard: "domain-a", seconds: 5, status: "pass" },
      { shard: "domain-b", seconds: 2, status: "pass" },
    ]);
    assert.equal(totals.get("domain-a"), 15);
    assert.equal(totals.get("domain-b"), 2);
  });

  await t.test("report says so plainly when the harness emitted nothing", () => {
    assert.match(report("no timings here", 5), /no `cargo-postgres-timing:` lines found/);
  });

  await t.test("report quantifies the critical-path saving", () => {
    const text = [
      line({ name: "a", package: "pkg-a", seconds: 100, status: "pass", shard: "domain-a" }),
      line({ name: "b", package: "pkg-b", seconds: 60, status: "pass", shard: "domain-a" }),
      line({ name: "c", package: "pkg-c", seconds: 40, status: "pass", shard: "domain-b" }),
    ].join("\n");
    const out = report(text, 2);
    assert.match(out, /parsed 3 invocations across 3 packages/);
    // observed max is 160 (domain-a); LPT over {100,60,40} across 2 bins is 100.
    assert.match(out, /critical path 160\.0s -> 100\.0s/);
  });
});
