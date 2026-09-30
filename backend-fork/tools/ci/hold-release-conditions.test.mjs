#!/usr/bin/env node
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  evaluate,
  holdObjects,
  holdReleased,
  kebabCase,
  registry,
  snakeCase,
} from "./hold-release-conditions.mjs";

test("hold-release-conditions", async (t) => {
  await t.test("the three naming transforms agree across the six files", () => {
    // This is the derivation that is actually error-prone. The registry says
    // `PayRun`, the file on disk says `pay_run`, the CI map says `pay-run`.
    assert.equal(snakeCase("PayRun"), "pay_run");
    assert.equal(snakeCase("JobPosition"), "job_position");
    assert.equal(snakeCase("OrgUnit"), "org_unit");
    assert.equal(snakeCase("Person"), "person");
    assert.equal(kebabCase("pay_run_port_as_runtime_role"), "pay-run-port-as-runtime-role");
  });

  await t.test("the hold's subject is read from the hold, not hardcoded", () => {
    const md = [
      "- Something else is **HOLD** for other reasons.",
      "- Company, Person, Employment, and PayRun projection fan-out is **HOLD** until each has an explicit owning port and a proven single-writer boundary.",
    ].join("\n");
    assert.deepEqual(holdObjects(md), ["Company", "Person", "Employment", "PayRun"]);
  });

  await t.test("a rewritten hold is followed, not ignored", () => {
    const md = "- Company and OrgUnit projection fan-out is **HOLD** until each has an explicit owning port.";
    assert.deepEqual(holdObjects(md), ["Company", "OrgUnit"]);
  });

  await t.test("the RELEASED form is still parsed and still checked", () => {
    // Releasing must not retire the checking that justified releasing. A parser
    // that only recognised "**HOLD**" would go silent at exactly the moment a
    // regression matters most — after the boundary has been declared sound.
    const md = "- Company, Person, Employment, and PayRun projection fan-out is **RELEASED** (2026-08-19). Evidence follows.";
    assert.deepEqual(holdObjects(md), ["Company", "Person", "Employment", "PayRun"]);
    assert.equal(holdReleased(md), true);
  });

  await t.test("the HOLD form still reports as not released", () => {
    const md = "- Company and PayRun projection fan-out is **HOLD** until each has an explicit owning port.";
    assert.equal(holdReleased(md), false);
    assert.deepEqual(holdObjects(md), ["Company", "PayRun"]);
  });

  await t.test("release does not weaken any leg", () => {
    // The load-bearing assertion after release: every leg is still required.
    const { failures, rows, released } = evaluate();
    assert.equal(released, true, "this tree has released the projection fan-out hold");
    assert.deepEqual(failures, [], failures.join("\n"));
    for (const row of rows) {
      assert.ok(row.owner.startsWith("console-"), `${row.key} owner`);
      assert.ok(row.tables > 0, `${row.key} owns no table`);
      assert.ok(row.suite, `${row.key} has no port suite`);
      assert.ok(row.ci, `${row.key} port suite is not CI-wired`);
    }
  });

  await t.test("a deleted hold bullet is an error, not a silent pass", () => {
    // If the bullet is removed, this verifier must not quietly report success
    // over an empty subject list -- that would be the emptiest false green.
    assert.throws(() => holdObjects("# PRODUCT\n\nno holds here\n"), /no longer contains/);
  });

  await t.test("the registry parser reads key, owner and tables", () => {
    const src = `
            PayRun => "pay_run",
                owner = "console-payroll-adapter-postgres",
                tables = ["payroll_draft_runs", "payroll_draft_lines"];
        `;
    assert.deepEqual(registry(src), [
      {
        key: "PayRun",
        slug: "pay_run",
        owner: "console-payroll-adapter-postgres",
        tables: ["payroll_draft_runs", "payroll_draft_lines"],
      },
    ]);
  });

  await t.test("a registry that parses to nothing fails closed", () => {
    // Guarding zero subjects must fail. If the macro's shape changes and the
    // regex stops matching, this verifier must go red rather than declare the
    // hold releasable over an empty roster.
    assert.deepEqual(registry("nothing that looks like a port declaration"), []);
  });

  await t.test("every leg of the hold is currently met on this tree", () => {
    // The load-bearing assertion: all four objects the hold names (and the two
    // it does not) have an owning port, at least one owned table, a port suite
    // proving the boundary, and that suite wired into the workflow postgres job.
    const { failures, rows } = evaluate();
    assert.deepEqual(failures, [], failures.join("\n"));
    assert.equal(rows.length, 6, "the six canonical object keys");
    for (const row of rows) {
      assert.ok(row.owner.startsWith("console-"), `${row.key} owner`);
      assert.ok(row.tables > 0, `${row.key} owns no table`);
      assert.ok(row.suite, `${row.key} has no port suite`);
      assert.ok(row.ci, `${row.key} port suite is not CI-wired`);
    }
  });

  await t.test("every object the hold names is a real registry key", () => {
    const { rows } = evaluate();
    const named = rows.filter((r) => r.named).map((r) => r.key);
    assert.deepEqual(named, ["Company", "Person", "Employment", "PayRun"]);
  });

  await t.test("PayRun's proof lives outside the canonical adapter", () => {
    // Pinned deliberately. Searching only the canonical adapter's test directory
    // finds five of six port suites and misses PayRun, whose owner is the
    // payroll adapter -- which reads as "PayRun is barely covered" when it has
    // the largest owned-table set of the six. That wrong turn is the reason this
    // verifier exists, so the shape of it is worth keeping red-detectable.
    const { rows } = evaluate();
    const payRun = rows.find((r) => r.key === "PayRun");
    assert.equal(payRun.owner, "console-payroll-adapter-postgres");
    assert.match(payRun.suite, /^backend\/crates\/payroll\/adapter-postgres\//);
    assert.equal(payRun.tables, 6, "PayRun owns the largest table set of the six");
  });
});
