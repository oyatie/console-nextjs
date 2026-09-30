#!/usr/bin/env node
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { evaluateNightlyWorkflow, NIGHTLY_DEV_UP_STEPS } from "./check-nightly-workflow.mjs";

const workflow = readFileSync(
  new URL("../../.github/workflows/nightly.yml", import.meta.url),
  "utf8",
);

/** Assert a mutated workflow produces a failure matching `pattern`. */
function expectFailure(mutated, pattern) {
  const failures = evaluateNightlyWorkflow(mutated);
  assert.ok(
    failures.some((failure) => pattern.test(failure)),
    `expected a failure matching ${pattern}, got ${JSON.stringify(failures)}`,
  );
}

test("check-nightly-workflow", async (t) => {
  await t.test("the committed nightly workflow satisfies its contract", () => {
    assert.deepEqual(evaluateNightlyWorkflow(workflow), []);
  });

  await t.test("rejects a workflow that is not parseable YAML", () => {
    expectFailure("jobs:\n  - [unclosed\n", /not parseable YAML/);
  });

  await t.test("rejects a workflow with the job removed entirely", () => {
    expectFailure(workflow.replace("  dev-up-smoke:", "  something-else:"), /must define the dev-up-smoke job/);
  });

  await t.test("requires the schedule backstop", () => {
    expectFailure(
      workflow.replace(/  schedule:\n    - cron: "[^"]+"[^\n]*\n/, ""),
      /must keep its schedule trigger/,
    );
  });

  await t.test("requires push-on-dev so detection stays at minutes", () => {
    expectFailure(
      workflow.replace("  push:\n    branches: [dev]\n", ""),
      /must run on push to dev/,
    );
    expectFailure(
      workflow.replace("branches: [dev]", "branches: [some-other-branch]"),
      /must run on push to dev/,
    );
  });

  await t.test("rejects a soft-failed job", () => {
    expectFailure(
      workflow.replace("  dev-up-smoke:\n", "  dev-up-smoke:\n    continue-on-error: true\n"),
      /job-level continue-on-error/,
    );
  });

  await t.test("rejects a soft-failed step", () => {
    expectFailure(
      workflow.replace(
        "      - name: dev-up bootstrap (compose deps + migrate + backend readyz)\n",
        "      - name: dev-up bootstrap (compose deps + migrate + backend readyz)\n        continue-on-error: true\n",
      ),
      /must not be soft-failed/,
    );
  });

  await t.test("rejects a neutered proof command", () => {
    expectFailure(
      workflow.replace("run: node scripts/dev-up.mjs bootstrap", "run: true"),
      /must run exactly: node scripts\/dev-up\.mjs bootstrap/,
    );
    expectFailure(
      workflow.replace("run: ops/postgres-topology.integration.test.sh", "run: true"),
      /must run exactly: ops\/postgres-topology\.integration\.test\.sh/,
    );
  });

  await t.test("rejects a reordered step list", () => {
    const reordered = workflow
      .replace("      - name: dev-up compose contract unit test\n", "      - name: TEMP\n")
      .replace("      - name: Install pinned DotSlash runtime\n", "      - name: dev-up compose contract unit test\n")
      .replace("      - name: TEMP\n", "      - name: Install pinned DotSlash runtime\n");
    expectFailure(reordered, /exact ordered step list/);
  });

  await t.test("rejects a dropped step", () => {
    expectFailure(
      workflow.replace(
        "      - name: Confirm /readyz reachable\n        run: curl -fsS \"http://127.0.0.1:${CONSOLE_DEV_HTTP_PORT:-8090}/readyz\"\n",
        "",
      ),
      /exact ordered step list/,
    );
  });

  await t.test("rejects an unpinned setup action", () => {
    expectFailure(
      workflow.replace(
        "uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7",
        "uses: actions/checkout@v7",
      ),
      /must use a SHA-pinned or local action/,
    );
  });

  await t.test("requires teardown to survive a failed bootstrap", () => {
    expectFailure(
      workflow.replace(
        "      - name: dev-up down\n        if: ${{ always() }}\n",
        "      - name: dev-up down\n",
      ),
      /teardown survives a failure/,
    );
  });

  await t.test("the locked contract covers every step the workflow declares", () => {
    // A contract shorter than the workflow would silently stop checking the tail.
    const declared = workflow.split("\n").filter((line) => /^      - name: /.test(line)).length;
    assert.equal(declared, NIGHTLY_DEV_UP_STEPS.length);
  });
});
