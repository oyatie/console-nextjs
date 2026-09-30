import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const gate = resolve(here, "check-postgres-cargo-map.mjs");
const realMapPath = resolve(root, "tools/ci/postgres-cargo-map.json");
const realMap = JSON.parse(readFileSync(realMapPath, "utf8"));

const run = (mapPath) => spawnSync(process.execPath, [gate, ...(mapPath ? [mapPath] : [])], {
  encoding: "utf8",
  cwd: root,
});

const withMap = (mutate) => {
  const dir = mkdtempSync(join(tmpdir(), "pg-cargo-map-"));
  try {
    const copy = JSON.parse(JSON.stringify(realMap));
    mutate(copy);
    const path = join(dir, "map.json");
    writeFileSync(path, `${JSON.stringify(copy, null, 2)}\n`);
    return run(path);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

test("the committed map satisfies the gate", () => {
  const result = run();
  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.match(result.stdout, /postgres-cargo-map OK/);
});

test("declared counts are checked against the file, field by field", () => {
  // Each field is asserted separately: a single aggregate check would let one
  // stale field hide behind another that happened to be right.
  for (const field of [
    "mapped",
    "unmapped",
    "workflow_targets",
    "workflow_mapped",
    "workflow_missing",
  ]) {
    const result = withMap((map) => {
      map.counts[field] = map.counts[field] + 1;
    });
    assert.notEqual(result.status, 0, `counts.${field} drift must fail the gate`);
    assert.match(
      result.stderr,
      new RegExp(`counts\\.${field} declares`),
      `counts.${field} drift must name the field it caught`,
    );
  }
});

test("a missing counts block is drift, not an exemption", () => {
  // The absent-metadata path is the one that silently passes if the check reads
  // `map.counts ?? {}` and compares undefined against undefined.
  const result = withMap((map) => {
    delete map.counts;
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /counts\.mapped declares/);
});

test("adding an entry without updating counts is caught", () => {
  // The exact historical failure: entries grew, counts did not, and the stale
  // number was then printed as if it had been measured.
  const result = withMap((map) => {
    const template = map.entries.find((entry) => entry.in_workflow_postgres_job);
    map.entries.push({ ...template, name: `${template.name}-drift-fixture` });
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /counts\.mapped declares \d+ but the file contains \d+/);
});
