import test from "node:test";
import assert from "node:assert/strict";
import { softRedsFromPr, mergeItems } from "./ingest-soft-reds.mjs";

test("softRedsFromPr enqueues BEHIND", () => {
  const items = softRedsFromPr({
    number: 591,
    title: "admit gates",
    mergeStateStatus: "BEHIND",
    mergeable: "MERGEABLE",
    statusCheckRollup: [],
  });
  assert.ok(items.some((i) => i.source_key === "soft_red:pr:591:behind"));
});

test("softRedsFromPr enqueues CONFLICTING as hard_block", () => {
  const items = softRedsFromPr({
    number: 592,
    title: "apr",
    mergeStateStatus: "DIRTY",
    mergeable: "CONFLICTING",
    statusCheckRollup: [],
  });
  assert.ok(items.some((i) => i.kind === "hard_block" && i.source_key.includes("dirty")));
});

test("softRedsFromPr enqueues failing Required as hard_block", () => {
  const items = softRedsFromPr({
    number: 593,
    title: "eff",
    mergeStateStatus: "BLOCKED",
    mergeable: "MERGEABLE",
    statusCheckRollup: [
      { name: "Required / CI", conclusion: "FAILURE", status: "COMPLETED" },
      { name: "Some optional lint", conclusion: "FAILURE", status: "COMPLETED" },
    ],
  });
  assert.ok(items.some((i) => i.source_key.includes("required") && i.kind === "hard_block"));
  assert.ok(items.some((i) => i.source_key.includes("optional") || i.kind === "soft_red"));
});

test("mergeItems dedupes by source_key", () => {
  const { items, added, updated } = mergeItems(
    [{ source_key: "soft_red:pr:1:behind", status: "ready", title: "old" }],
    [{ source_key: "soft_red:pr:1:behind", status: "ready", title: "new", evidence: "x" }],
  );
  assert.equal(items.length, 1);
  assert.equal(added.length, 0);
  assert.equal(updated.length, 1);
  assert.equal(items[0].title, "new");
});

test("mergeItems expires derived PR observations absent from the new scan", () => {
  const { items } = mergeItems(
    [
      { source_key: "soft_red:pr:1:behind", derived: true, status: "ready", title: "stale pr" },
      { source_key: "block:pr:2:dirty", derived: true, status: "ready", title: "stale block" },
      { source_key: "manual:keep", status: "ready", title: "manual entry" },
    ],
    [{ source_key: "manual:keep", status: "ready", title: "manual entry" }],
  );
  const keys = items.map((i) => i.source_key);
  assert.deepEqual(keys, ["manual:keep"]);
  assert.equal(items.length, 1);
});

test("mergeItems preserves custom PR observations absent from the new scan", () => {
  const { items } = mergeItems(
    [
      { source_key: "soft_red:pr:591:auth_bootstrap_fail", status: "ready", title: "custom" },
      { source_key: "soft_red:pr:3:behind", derived: true, status: "ready", title: "derived stale" },
    ],
    [],
  );
  const keys = items.map((i) => i.source_key);
  assert.deepEqual(keys, ["soft_red:pr:591:auth_bootstrap_fail"]);
  assert.equal(items.length, 1);
});
