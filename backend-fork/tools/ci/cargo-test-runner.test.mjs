#!/usr/bin/env node
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { parseTimingLines } from "./postgres-timings.mjs";

const runner = resolve(dirname(fileURLToPath(import.meta.url)), "cargo-test-runner.sh");

/**
 * Build a temp dir containing a stubbed `cargo` on `bin/`. The stub logs every
 * invocation and exits 1 when any argument contains `fail`, else 0 — so a fake
 * map of JSONL rows can exercise the keep-going loop without Docker or cargo.
 */
function setupFixture() {
  const root = mkdtempSync(join(tmpdir(), "cargo-test-runner-"));
  const bin = join(root, "bin");
  mkdirSync(bin, { recursive: true });
  const cargo = join(bin, "cargo");
  writeFileSync(
    cargo,
    [
      "#!/usr/bin/env bash",
      'printf \'%s\\n\' "$*" >> "${CARGO_INVOCATION_LOG}"',
      'case "$*" in',
      "  *fail*) exit 1 ;;",
      "  *) exit 0 ;;",
      "esac",
      "",
    ].join("\n"),
  );
  chmodSync(cargo, 0o755);
  return { root, bin, log: join(root, "cargo.log") };
}

function runRunner({ root, bin, log, rows, args = [] }) {
  return spawnSync(runner, args, {
    input: rows.map((row) => JSON.stringify(row)).join("\n") + "\n",
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      CARGO_REPO_ROOT: root,
      RUST_TEST_THREADS: "1",
      CARGO_INVOCATION_LOG: log,
    },
  });
}

const ROWS = [
  { name: "pass-a", package: "pkg-pass-a", argv: ["cargo", "test", "-p", "pkg-pass-a"] },
  { name: "fail-b", package: "pkg-fail-b", argv: ["cargo", "test", "-p", "pkg-fail-b"] },
  { name: "pass-c", package: "pkg-pass-c", argv: ["cargo", "test", "-p", "pkg-pass-c"] },
];

test("keep-going (default) runs every invocation, collects failures, exits 1", () => {
  const { root, bin, log } = setupFixture();
  try {
    const result = runRunner({ root, bin, log, rows: ROWS });
    const logLines = readFileSync(log, "utf8").trim().split("\n");
    assert.equal(result.status, 1, result.stdout + result.stderr);
    // every binary ran despite the middle failure
    assert.deepEqual(logLines, [
      "test -p pkg-pass-a",
      "test -p pkg-fail-b",
      "test -p pkg-pass-c",
    ]);
    assert.match(result.stdout, /PASS {2}pass-a/);
    assert.match(result.stdout, /FAIL {2}fail-b/);
    assert.match(result.stdout, /PASS {2}pass-c/);
    assert.match(result.stdout, /2 passed, 1 failed/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("--fail-fast aborts at the first failure and skips the rest", () => {
  const { root, bin, log } = setupFixture();
  try {
    const result = runRunner({ root, bin, log, rows: ROWS, args: ["--fail-fast"] });
    const logLines = readFileSync(log, "utf8").trim().split("\n");
    assert.equal(result.status, 1, result.stdout + result.stderr);
    assert.deepEqual(logLines, ["test -p pkg-pass-a", "test -p pkg-fail-b"]);
    assert.match(result.stdout, /PASS {2}pass-a/);
    assert.match(result.stdout, /FAIL {2}fail-b/);
    assert.match(result.stdout, /1 passed, 1 failed/);
    assert.doesNotMatch(result.stdout, /pass-c/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("exit 0 only when every invocation passed", () => {
  const { root, bin, log } = setupFixture();
  try {
    const rows = ROWS.filter((row) => !row.package.includes("fail"));
    const result = runRunner({ root, bin, log, rows });
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stdout, /2 passed, 0 failed/);
    assert.match(result.stdout, /all 2 invocations passed/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("emits one parseable timing line per invocation, tagged pass/fail", () => {
  const { root, bin, log } = setupFixture();
  try {
    const result = runRunner({ root, bin, log, rows: ROWS });
    const timings = parseTimingLines(result.stdout);
    assert.equal(timings.length, 3, "one line per invocation, failures included");
    assert.deepEqual(timings.map((row) => row.name), ["pass-a", "fail-b", "pass-c"]);
    assert.deepEqual(timings.map((row) => row.status), ["pass", "fail", "pass"]);
    assert.deepEqual(timings.map((row) => row.package), [
      "pkg-pass-a",
      "pkg-fail-b",
      "pkg-pass-c",
    ]);
    // A duration that is absent, negative, or non-finite would silently
    // under-weight a package in the bin-pack rather than fail the harvest.
    for (const row of timings) {
      assert.equal(typeof row.seconds, "number");
      assert.ok(row.seconds >= 0, `duration must be non-negative, got ${row.seconds}`);
      assert.ok(Number.isFinite(row.seconds));
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("stamps the shard id from the environment so logs stay attributable", () => {
  const { root, bin, log } = setupFixture();
  try {
    const result = spawnSync(runner, [], {
      input: JSON.stringify(ROWS[0]) + "\n",
      encoding: "utf8",
      env: {
        ...process.env,
        PATH: `${bin}:${process.env.PATH}`,
        CARGO_REPO_ROOT: root,
        RUST_TEST_THREADS: "1",
        CARGO_INVOCATION_LOG: log,
        CARGO_POSTGRES_SHARD_ID: "domain-b",
      },
    });
    const timings = parseTimingLines(result.stdout);
    assert.equal(timings.length, 1);
    assert.equal(timings[0].shard, "domain-b");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
