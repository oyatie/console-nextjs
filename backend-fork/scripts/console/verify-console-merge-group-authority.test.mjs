#!/usr/bin/env node
import test from "node:test";
import assert from "node:assert/strict";
import { evaluateCheckRuns, parseCheckRunLines, pullRequestNumberFromHeadRef } from "./verify-console-merge-group-authority.mjs";

const SHA = "a".repeat(40);

test("recognises a single-entry merge-group ref", () => {
  assert.equal(pullRequestNumberFromHeadRef(`refs/heads/gh-readonly-queue/main/pr-793-${SHA}`), 793);
  assert.equal(pullRequestNumberFromHeadRef(`gh-readonly-queue/main/pr-1-${SHA}`), 1);
});

test("refuses any ref it cannot resolve to exactly one pull request", () => {
  // The batched case matters most: raising max_entries_to_build without
  // teaching this script to enumerate the group must fail closed, not pass.
  for (const ref of [
    "",
    null,
    "refs/heads/main",
    "refs/heads/gh-readonly-queue/main/pr-793",              // no sha
    `refs/heads/gh-readonly-queue/main/pr-abc-${SHA}`,        // non-numeric
    `refs/heads/gh-readonly-queue/main/pr-0-${SHA}`,          // not a real PR number
    "refs/heads/gh-readonly-queue/main/batch-793-794",        // hypothetical batch shape
  ]) {
    assert.equal(pullRequestNumberFromHeadRef(ref), null, `must refuse ${JSON.stringify(ref)}`);
  }
});

test("accepts only a check that concluded success", () => {
  const name = "authenticate-console-authority";
  assert.equal(evaluateCheckRuns([{ name, conclusion: "success" }], name).ok, true);

  for (const conclusion of ["failure", "cancelled", "timed_out", "neutral", "skipped", null]) {
    const verdict = evaluateCheckRuns([{ name, conclusion }], name);
    assert.equal(verdict.ok, false, `must refuse conclusion ${conclusion}`);
    assert.match(verdict.reason, /did not conclude success/);
  }
});

test("a missing or unreadable check is refused, not treated as absent-therefore-fine", () => {
  const name = "authenticate-console-authority";
  assert.equal(evaluateCheckRuns([], name).ok, false);
  assert.match(evaluateCheckRuns([], name).reason, /no authenticate-console-authority run/);
  assert.equal(evaluateCheckRuns([{ name: "Required / CI", conclusion: "success" }], name).ok, false);
  assert.equal(evaluateCheckRuns(null, name).ok, false);
  assert.equal(evaluateCheckRuns(undefined, name).ok, false);
});

test("a re-run that produced one success and one failure is refused", () => {
  // Taking the newest run would let a green re-run paper over a red one; the
  // queue's guarantee is that the PR passed, not that it passed once.
  const name = "authenticate-console-authority";
  const verdict = evaluateCheckRuns(
    [{ name, conclusion: "success" }, { name, conclusion: "failure" }],
    name,
  );
  assert.equal(verdict.ok, false);
});

test("check-run pagination", async (t) => {
  const run = (name, conclusion = "success") => ({ name, conclusion });

  await t.test("an incomplete read is not reported as a missing run", () => {
    // The #820 ejection. The API defaults to 30 check runs per page and the PR
    // had 32, so authenticate-console-authority fell off page one and the gate
    // said "no authenticate-console-authority run on the queued head" about a
    // pull request that had passed it. A short read must be named as a short
    // read -- "missing page" and "missing gate" demand opposite responses.
    const page = Array.from({ length: 30 }, (_, i) => run(`filler-${i}`));
    const verdict = evaluateCheckRuns(page, "authenticate-console-authority", 32);
    assert.equal(verdict.ok, false);
    assert.match(verdict.reason, /read only 30 of 32/);
    assert.doesNotMatch(verdict.reason, /no authenticate-console-authority run/);
  });

  await t.test("a complete read that truly lacks the run still says so", () => {
    const runs = [run("something-else")];
    const verdict = evaluateCheckRuns(runs, "authenticate-console-authority", 1);
    assert.equal(verdict.ok, false);
    assert.match(verdict.reason, /no authenticate-console-authority run/);
  });

  await t.test("a complete read containing the run passes", () => {
    const runs = [...Array.from({ length: 31 }, (_, i) => run(`filler-${i}`)),
      run("authenticate-console-authority")];
    assert.equal(evaluateCheckRuns(runs, "authenticate-console-authority", 32).ok, true);
  });

  await t.test("an unknown total does not manufacture a short-read failure", () => {
    // total_count is a best-effort probe; if it cannot be read the gate must
    // still work off what it has rather than refusing everything.
    const runs = [run("authenticate-console-authority")];
    assert.equal(evaluateCheckRuns(runs, "authenticate-console-authority", null).ok, true);
  });

  await t.test("more runs than the reported total is not a short read", () => {
    // Check runs can be added between the two API calls; only a SHORT read is
    // suspicious.
    const runs = [run("authenticate-console-authority"), run("x"), run("y")];
    assert.equal(evaluateCheckRuns(runs, "authenticate-console-authority", 2).ok, true);
  });

  await t.test("parses the NDJSON gh --paginate emits across pages", () => {
    const stdout = '{"name":"a","conclusion":"success"}\n{"name":"b","conclusion":"failure"}\n';
    assert.deepEqual(parseCheckRunLines(stdout), [
      { name: "a", conclusion: "success" },
      { name: "b", conclusion: "failure" },
    ]);
  });

  await t.test("blank lines between pages are tolerated", () => {
    assert.deepEqual(parseCheckRunLines('\n{"name":"a","conclusion":"success"}\n\n'),
      [{ name: "a", conclusion: "success" }]);
  });

  await t.test("a truncated page yields null, not a partial list", () => {
    // Returning a partial list here would re-create the original bug one layer
    // down: a truncated read that looks like a complete one.
    assert.equal(parseCheckRunLines('{"name":"a","conclusion":"success"}\n{"name":"b"'), null);
    assert.equal(evaluateCheckRuns(null, "authenticate-console-authority").ok, false);
  });
});
