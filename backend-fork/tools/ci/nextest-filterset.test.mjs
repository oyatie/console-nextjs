#!/usr/bin/env node
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { binaryIdForEntry, filtersetFromEntries, quoteBinaryId } from "./nextest-filterset.mjs";

const entry = (pkg, ...argv) => ({ name: `${pkg}-entry`, package: pkg, cargo_argv: argv });

test("nextest-filterset", async (t) => {
  await t.test("maps an integration target to package::binary", () => {
    assert.equal(
      binaryIdForEntry(entry("console-hr", "cargo", "test", "--locked", "--test", "payroll_rls")),
      "console-hr::payroll_rls",
    );
  });

  await t.test("maps a --lib target to the bare package", () => {
    assert.equal(
      binaryIdForEntry(entry("console-platform-authz", "cargo", "test", "--locked", "--lib")),
      "console-platform-authz",
    );
  });

  await t.test("refuses an entry with no package", () => {
    assert.equal(binaryIdForEntry({ package: "", cargo_argv: ["cargo", "test", "--lib"] }), null);
  });

  await t.test("refuses --test with a missing or flag-shaped name", () => {
    assert.equal(binaryIdForEntry(entry("p", "cargo", "test", "--test")), null);
    assert.equal(binaryIdForEntry(entry("p", "cargo", "test", "--test", "--locked")), null);
    assert.equal(binaryIdForEntry(entry("p", "cargo", "test", "--test", "")), null);
  });

  await t.test("refuses an argv that selects neither --test nor --lib", () => {
    // e.g. a bare `cargo test -p x`, which would run everything in the package.
    assert.equal(binaryIdForEntry(entry("p", "cargo", "test", "--locked")), null);
  });

  await t.test("renders the UNQUOTED exact-match form nextest actually matches", () => {
    // Measured against cargo-nextest 0.9.138: binary_id(=pkg::name) matches,
    // binary_id("pkg::name") matches nothing and aborts the run.
    assert.equal(quoteBinaryId("a::b"), "binary_id(=a::b)");
    assert.equal(quoteBinaryId("solo"), "binary_id(=solo)");
  });

  await t.test("rejects ids that cannot be embedded unquoted", () => {
    // The value cannot be quoted, so expression syntax must be refused rather
    // than escaped -- otherwise it corrupts the filterset silently.
    for (const bad of ["we*rd", 'has"quote', "has space", "a)b", "a+b", "a,b", ""]) {
      assert.throws(() => quoteBinaryId(bad), /not safe to embed unquoted/, `should reject ${bad}`);
    }
  });

  await t.test("joins ids with nextest union syntax, sorted", () => {
    const { expression, ids } = filtersetFromEntries([
      entry("zeta", "cargo", "test", "--test", "b"),
      entry("alpha", "cargo", "test", "--test", "a"),
    ]);
    assert.deepEqual(ids, ["alpha::a", "zeta::b"]);
    assert.equal(expression, "binary_id(=alpha::a) + binary_id(=zeta::b)");
  });

  await t.test("collapses duplicate binaries instead of repeating them", () => {
    const { ids } = filtersetFromEntries([
      entry("p", "cargo", "test", "--test", "same"),
      entry("p", "cargo", "test", "--test", "same"),
    ]);
    assert.deepEqual(ids, ["p::same"]);
  });

  await t.test("reports untranslatable entries rather than dropping them silently", () => {
    const { ids, unmapped } = filtersetFromEntries([
      entry("good", "cargo", "test", "--test", "t"),
      { name: "mystery", package: "p", cargo_argv: ["cargo", "test", "--benches"] },
    ]);
    assert.deepEqual(ids, ["good::t"]);
    assert.deepEqual(unmapped, ["mystery"], "a silently omitted target is a false green");
  });

  await t.test("every real map entry translates", () => {
    // The whole point is that no target is left behind by the swap. If this
    // goes red, the filterset would run less than the cargo path does.
    const map = JSON.parse(
      readFileSync(new URL("./postgres-cargo-map.json", import.meta.url), "utf8"),
    );
    const workflow = (map.entries ?? []).filter((e) => e.in_workflow_postgres_job);
    assert.ok(workflow.length >= 180, `expected the full workflow set, got ${workflow.length}`);
    const { ids, unmapped } = filtersetFromEntries(workflow);
    assert.deepEqual(unmapped, [], "every workflow entry must translate to a binary id");
    assert.ok(ids.length > 0);
  });
});
