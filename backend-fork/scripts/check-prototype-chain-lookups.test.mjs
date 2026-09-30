import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, describe, it } from "node:test";
import { fileURLToPath } from "node:url";

import {
  BASELINE_REL,
  SCANNED_FLOOR,
  evaluatePrototypeChainLookups,
  findingId,
  isLiteralKeyExpr,
  listSubjectFiles,
} from "./check-prototype-chain-lookups.mjs";
import { installGitFixtureEnvironment } from "./lib/git-fixture-environment.mjs";

installGitFixtureEnvironment();

const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const cli = fileURLToPath(new URL("./check-prototype-chain-lookups.mjs", import.meta.url));
const fixtureRoots = [];

after(() => {
  for (const root of fixtureRoots) rmSync(root, { recursive: true, force: true });
});

function fixture(files) {
  const root = mkdtempSync(join(tmpdir(), "proto-chain-census-"));
  fixtureRoots.push(root);
  for (const [relative, contents] of Object.entries(files)) {
    const absolute = join(root, relative);
    mkdirSync(dirname(absolute), { recursive: true });
    writeFileSync(absolute, contents);
  }
  const init = spawnSync("git", ["-c", "init.defaultBranch=main", "init", "-q", root], {
    encoding: "utf8",
  });
  assert.equal(init.status, 0, init.stderr);
  const add = spawnSync("git", ["-C", root, "add", "--", ...Object.keys(files)], {
    encoding: "utf8",
  });
  assert.equal(add.status, 0, add.stderr);
  return root;
}

function emptyBaseline(residuals = []) {
  return `${JSON.stringify({ schema_version: 1, residuals }, null, 2)}\n`;
}

describe("prototype-chain lookup census", () => {
  it("classifies literal keys as out of class", () => {
    assert.equal(isLiteralKeyExpr("'constructor'"), true);
    assert.equal(isLiteralKeyExpr('"toString"'), true);
    assert.equal(isLiteralKeyExpr("1"), true);
    assert.equal(isLiteralKeyExpr("name"), false);
    assert.equal(isLiteralKeyExpr("lane.laneId"), false);
  });

  it("reports optional-computed and undefined-compare on untrusted keys (RED control)", () => {
    const root = fixture({
      "scripts/check-hostile.mjs": [
        "export function read(map, key) {",
        "  if (map?.[key]) return map[key];",
        "  if (map.properties[key] !== undefined) return map.properties[key];",
        "  return null;",
        "}",
        "",
      ].join("\n"),
      [BASELINE_REL]: emptyBaseline(),
    });

    const { findings, unknown, scanned } = evaluatePrototypeChainLookups(root);
    assert.ok(scanned >= 1);
    assert.equal(findings.length, 2);
    assert.equal(unknown.length, 2);
    assert.deepEqual(
      findings.map((f) => f.kind).sort(),
      ["optional-computed", "undefined-compare"],
    );
  });

  it("reports identifier in-operator and ignores for-in", () => {
    const root = fixture({
      "scripts/check-in.mjs": [
        "export function has(key, value) {",
        "  if (key in value) return true;",
        "  for (const child in value) {",
        "    if (child === key) return true;",
        "  }",
        "  return false;",
        "}",
        "",
      ].join("\n"),
      [BASELINE_REL]: emptyBaseline(),
    });

    const { findings } = evaluatePrototypeChainLookups(root);
    assert.equal(findings.length, 1);
    assert.equal(findings[0].kind, "in-operator");
    assert.equal(findings[0].key, "key");
  });

  it("ignores literal optional keys and match?.[1] numerics", () => {
    const root = fixture({
      "scripts/check-safe.mjs": [
        'const name = pkg.scripts?.["check:safe"];',
        "const id = body.match(/x/)?.[1];",
        "export default { name, id };",
        "",
      ].join("\n"),
      [BASELINE_REL]: emptyBaseline(),
    });

    const { findings } = evaluatePrototypeChainLookups(root);
    assert.deepEqual(findings, []);
  });

  it("passes when every finding is named in the residual register", () => {
    const snippet = "if (map?.[key]) return map[key];";
    const residual = {
      file: "scripts/check-hostile.mjs",
      line: 2,
      kind: "optional-computed",
      key: "key",
      snippet,
      reason: "fixture residual",
    };
    const root = fixture({
      "scripts/check-hostile.mjs": `export function read(map, key) {\n  ${snippet}\n  return null;\n}\n`,
      [BASELINE_REL]: emptyBaseline([residual]),
    });

    const result = evaluatePrototypeChainLookups(root);
    assert.equal(result.unknown.length, 0);
    assert.equal(result.stale.length, 0);
    assert.equal(findingId(result.findings[0]), findingId(residual));
  });

  it("fails closed on stale residuals and on examined-zero", () => {
    const root = fixture({
      "scripts/check-clean.mjs": "export const ok = true;\n",
      [BASELINE_REL]: emptyBaseline([{
        file: "scripts/check-missing.mjs",
        line: 1,
        kind: "optional-computed",
        key: "ghost",
        snippet: "map?.[ghost]",
        reason: "stale on purpose",
      }]),
    });

    const result = evaluatePrototypeChainLookups(root);
    assert.equal(result.stale.length, 1);
    assert.equal(result.findings.length, 0);
    assert.deepEqual(result.missingSubjects, []);

    const emptyRoot = fixture({
      "README.md": "no subjects\n",
      [BASELINE_REL]: emptyBaseline(),
    });
    const empty = evaluatePrototypeChainLookups(emptyRoot);
    assert.equal(empty.scanned, 0);
    assert.equal(empty.belowFloor, true);
    assert.ok(empty.scanned < SCANNED_FLOOR);
  });

  it("fails closed when a listed subject path is missing on disk (unread ≠ scanned)", () => {
    const root = fixture({
      "scripts/check-present.mjs": "export const ok = true;\n",
      [BASELINE_REL]: emptyBaseline(),
    });
    const missing = Array.from(
      { length: SCANNED_FLOOR },
      (_, i) => `scripts/check-ghost-${i}.mjs`,
    );
    const result = evaluatePrototypeChainLookups(root, {
      subjects: ["scripts/check-present.mjs", ...missing],
    });
    assert.equal(result.scanned, 1, "scanned counts only files actually read");
    assert.equal(result.missingSubjects.length, SCANNED_FLOOR);
    assert.equal(result.belowFloor, true);
    // Critic PROOF_A inverted: declared-but-unread subjects must not clear the floor.
    const wouldCliFail = result.baselineMissing
      || result.missingSubjects.length > 0
      || result.belowFloor
      || result.unknown.length > 0
      || result.stale.length > 0;
    assert.equal(wouldCliFail, true);
  });

  it("CLI exits non-zero on unknown findings", () => {
    const root = fixture({
      "scripts/check-hostile.mjs": "export const read = (m, k) => m?.[k];\n",
      [BASELINE_REL]: emptyBaseline(),
    });
    const run = spawnSync(process.execPath, [cli, root], { encoding: "utf8" });
    assert.notEqual(run.status, 0, run.stdout + run.stderr);
    assert.match(run.stderr, /prototype-chain lookup census FAILED/);
  });

  it("live repo subjects exceed the scanned floor", () => {
    const subjects = listSubjectFiles(repoRoot);
    assert.ok(
      subjects.length >= SCANNED_FLOOR,
      `expected >= ${SCANNED_FLOOR} subjects, got ${subjects.length}`,
    );
  });

  it("live repo census is green against the committed residual register", () => {
    const run = spawnSync(process.execPath, [cli, repoRoot], { encoding: "utf8" });
    assert.equal(run.status, 0, run.stdout + run.stderr);
    assert.match(run.stdout, /prototype-chain lookup census passed/);
  });
});
