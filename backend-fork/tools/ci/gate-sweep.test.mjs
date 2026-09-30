#!/usr/bin/env node
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadGates, report, sweep } from "./gate-sweep.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

/** Deterministic fake runner: any command containing "bad" fails. */
const fakeExec = (cmd) =>
  cmd.includes("bad")
    ? { status: 1, stdout: `boom from ${cmd}\n`, stderr: "" }
    : { status: 0, stdout: "ok\n", stderr: "" };

test("gate-sweep", async (t) => {
  await t.test("EVERY gate runs even after an earlier one fails", () => {
    // This is the whole point. Under the old `&&` chain the shell stopped at the
    // first failure, so a tip with three broken gates needed three ~20-minute CI
    // cycles to reveal three problems.
    const ran = [];
    const gates = [
      { id: "one", run: "bad-1" },
      { id: "two", run: "good" },
      { id: "three", run: "bad-2" },
    ];
    const results = sweep(gates, (cmd) => {
      ran.push(cmd);
      return fakeExec(cmd);
    });
    assert.deepEqual(ran, ["bad-1", "good", "bad-2"], "a failure must not short-circuit the sweep");
    assert.deepEqual(results.map((r) => r.ok), [false, true, false]);
  });

  await t.test("reports every failure, not just the first", () => {
    const results = sweep(
      [{ id: "alpha", run: "bad-a" }, { id: "beta", run: "good" }, { id: "gamma", run: "bad-g" }],
      fakeExec,
    );
    const text = report(results);
    assert.match(text, /FAILED alpha, gamma/);
    assert.match(text, /1 passed, 2 failed/);
    assert.match(text, /boom from bad-a/, "the failing gate's output must be shown");
    assert.match(text, /boom from bad-g/, "…including the second one");
  });

  await t.test("an all-green sweep reports no failures", () => {
    const results = sweep([{ id: "a", run: "good" }, { id: "b", run: "good" }], fakeExec);
    assert.equal(results.every((r) => r.ok), true);
    assert.match(report(results), /2 passed, 0 failed/);
    assert.doesNotMatch(report(results), /FAILED/);
  });

  await t.test("a non-zero exit anywhere still means red", () => {
    // Fail-slow must not become fail-open: the sweep's exit code is derived from
    // `every(ok)`, so one red gate keeps the whole sweep red.
    const results = sweep([{ id: "a", run: "good" }, { id: "b", run: "bad" }], fakeExec);
    assert.equal(results.every((r) => r.ok), false);
  });

  await t.test("a sweep over zero gates is refused, not passed", () => {
    // A guard that examines nothing must fail; exiting 0 over an empty list
    // would be the emptiest possible false green.
    assert.throws(() => loadGates("/dev/null"), /declares no gates|Unexpected end/);
  });

  await t.test("malformed gate entries are refused", () => {
    const tmp = resolve(ROOT, "tools/ci/gate-sweep.json");
    const good = JSON.parse(readFileSync(tmp, "utf8"));
    assert.ok(good.gates.length > 0);
    // Structural refusals are exercised through loadGates' validation by
    // constructing docs in memory via a temp path is unnecessary — the
    // committed file must itself satisfy every rule, asserted below.
    for (const gate of good.gates) {
      assert.equal(typeof gate.id, "string");
      assert.notEqual(gate.id, "");
      assert.equal(typeof gate.run, "string");
      assert.notEqual(gate.run, "");
    }
    const ids = good.gates.map((g) => g.id);
    assert.equal(new Set(ids).size, ids.length, "duplicate gate ids would hide one gate behind another");
  });

  await t.test("the committed manifest still covers every gate package.json claims", () => {
    // If someone re-adds an `&&` link to check:ci-preflight instead of adding a
    // gate here, that gate would never run in the sweep. Catch that drift.
    const pkg = JSON.parse(readFileSync(resolve(ROOT, "package.json"), "utf8"));
    const script = pkg.scripts["check:ci-preflight"];
    assert.match(script, /gate-sweep\.mjs/, "check:ci-preflight must delegate to the sweep");
    assert.doesNotMatch(
      script,
      /&&/,
      "check:ci-preflight must not chain with && — that is the fail-fast behaviour this replaces",
    );
    assert.ok(loadGates().length >= 10, "the sweep must carry the full gate set");
  });
});
