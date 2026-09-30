#!/usr/bin/env node
/**
 * Argument-contract tests for tools/ci/cargo_needs_postgres.sh.
 *
 * Only the fail-closed paths are exercised, because every accepting path boots
 * Docker. That is the half worth locking anyway: a harness that silently
 * accepts an unknown runner would run the cargo path while the workflow
 * believed it had asked for nextest, and the two select targets differently.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const harness = resolve(dirname(fileURLToPath(import.meta.url)), "cargo_needs_postgres.sh");

const run = (...args) => spawnSync(harness, args, { encoding: "utf8" });

test("cargo_needs_postgres argument contract", async (t) => {
  await t.test("rejects an unknown runner before touching Docker", () => {
    const r = run("--runner", "bogus", "--shard-id", "domain-b");
    assert.equal(r.status, 2);
    assert.match(r.stderr, /invalid --runner bogus \(want cargo\|nextest\)/);
  });

  await t.test("rejects an empty runner rather than defaulting silently", () => {
    const r = run("--runner", "", "--shard-id", "domain-b");
    assert.equal(r.status, 2, "an empty runner must not fall through to the cargo path");
  });

  await t.test("accepts both supported runners at the validation stage", () => {
    // A valid runner must get PAST runner validation. It then fails on Docker
    // or the map, never with the runner message.
    for (const runner of ["cargo", "nextest"]) {
      const r = run("--runner", runner, "--shard-id", "nope");
      assert.equal(r.status, 2);
      assert.match(r.stderr, /invalid --shard-id/, `${runner} must pass runner validation`);
      assert.doesNotMatch(r.stderr, /invalid --runner/);
    }
  });

  await t.test("supports both --runner X and --runner=X spellings", () => {
    const spaced = run("--runner", "bogus", "--shard-id", "domain-b");
    const equals = run("--runner=bogus", "--shard-id", "domain-b");
    assert.equal(spaced.status, 2);
    assert.equal(equals.status, 2);
    assert.match(equals.stderr, /invalid --runner bogus/);
  });

  await t.test("still rejects the retired domain shard id", () => {
    const r = run("--shard-id", "domain");
    assert.equal(r.status, 2);
    assert.match(r.stderr, /retired in S2/);
  });

  await t.test("usage documents the runner flag", () => {
    const r = run("--help");
    assert.equal(r.status, 2);
    assert.match(r.stderr, /--runner cargo\|nextest/);
  });
});
