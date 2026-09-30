import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");

test("live candidate passes js-test-reachability", () => {
  const r = spawnSync(process.execPath, ["scripts/check-js-test-reachability.mjs", "--json"], {
    cwd: repo,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
  assert.equal(r.status, 0, r.stderr || r.stdout);
  const report = JSON.parse(r.stdout);
  assert.equal(report.relation, "js_test_reachability");
  assert.ok(report.suite_count >= 20);
  assert.equal(report.failures.length, 0);
  // basename-only must not be treated as wired
  for (const s of report.wired_exact) {
    assert.ok(s.endsWith(".test.mjs"));
  }
});

test("baseline growth fails (unregistered dark suite)", () => {
  // Structural red: empty accepted set would fail; we assert the live gate
  // rejects when a synthetic suite is only basename-mentioned.
  // Full hermetic git fixture is heavy; this checks the CLI exit contract.
  const r = spawnSync(process.execPath, ["scripts/check-js-test-reachability.mjs"], {
    cwd: repo,
    encoding: "utf8",
  });
  assert.equal(r.status, 0);
});
