import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { createTextGate } from "./text-gate.mjs";

function withFixture(files) {
  const root = mkdtempSync(join(tmpdir(), "text-gate-"));
  for (const [path, contents] of Object.entries(files)) {
    writeFileSync(join(root, path), contents);
  }
  return root;
}

test("createTextGate records pass labels for include/not-include/match/absent checks", () => {
  const root = withFixture({
    "sample.txt": "alpha\nbeta\nready: true\n",
  });
  const gate = createTextGate({
    root,
    passLabel: (label, kind) => `${label}:${kind}`,
  });

  gate.requireIncludes("sample.txt", "alpha", "has alpha");
  gate.requireNotIncludes("sample.txt", "gamma", "has no gamma");
  gate.requireMatches("sample.txt", /ready:\s*true/, "is ready");
  gate.requireAbsent("sample.txt", /TODO|demo/i, "no placeholder copy");

  assert.deepEqual(gate.checks, [
    "has alpha:include",
    "has no gamma:notInclude",
    "is ready:match",
    "no placeholder copy:absent",
  ]);
});

test("createTextGate preserves fail-fast errors for each check type", () => {
  const root = withFixture({
    "sample.txt": "alpha\nbeta\n",
  });
  const gate = createTextGate({ root });

  assert.throws(
    () => gate.requireIncludes("sample.txt", "gamma", "missing gamma"),
    /missing gamma: expected sample\.txt to include "gamma"/,
  );
  assert.throws(
    () => gate.requireNotIncludes("sample.txt", "alpha", "reject alpha"),
    /reject alpha: sample\.txt must not include "alpha"/,
  );
  assert.throws(
    () => gate.requireMatches("sample.txt", /ready:\s*true/, "ready marker"),
    /ready marker: expected sample\.txt to match \/ready:\\s\*true\//,
  );
  assert.throws(
    () => gate.requireAbsent("sample.txt", /alpha/, "alpha absent"),
    /alpha absent: sample\.txt must not match \/alpha\//,
  );
});

test("read memoizes file contents by resolved path for repeated checks", () => {
  const root = withFixture({
    "sample.txt": "first",
  });
  const gate = createTextGate({ root });

  assert.equal(gate.read("sample.txt"), "first");
  writeFileSync(join(root, "sample.txt"), "second");

  assert.equal(gate.read("sample.txt"), "first");
  assert.doesNotThrow(() => gate.requireIncludes("sample.txt", "first", "cached first content"));
});

test("payroll-style custom options shape pass labels and failure messages", () => {
  const root = withFixture({
    "payroll.txt": "release gate ready\n",
  });
  const gate = createTextGate({
    root,
    includeFailure: ({ path, needle, label }) => `${path} is missing ${label}: ${needle}`,
    matchFailure: ({ path, pattern, label }) => `${path} does not satisfy ${label}: ${pattern}`,
    absentFailure: ({ path, pattern, label }) => `${path} violates ${label}: ${pattern}`,
    passLabel: (label, kind) => `${label} ${kind === "absent" ? "absent" : "present"}`,
  });

  gate.requireIncludes("payroll.txt", "release gate", "production release gate");
  gate.requireMatches("payroll.txt", /ready/, "ready marker");
  gate.requireAbsent("payroll.txt", /TODO/, "no TODO marker");

  assert.deepEqual(gate.checks, [
    "production release gate present",
    "ready marker present",
    "no TODO marker absent",
  ]);
  assert.throws(
    () => gate.requireAbsent("payroll.txt", /release gate/, "release gate duplicate"),
    /payroll\.txt violates release gate duplicate: \/release gate\//,
  );
});

test("people HR-style custom absent failures can omit the pattern", () => {
  const root = withFixture({
    "people.txt": "placeholder",
  });
  const gate = createTextGate({
    root,
    includeFailure: ({ path, needle, label }) => `${path} is missing ${label}: ${needle}`,
    absentFailure: ({ path, label }) => `${path} still contains ${label}`,
    passLabel: (label, kind) => `${label} ${kind === "absent" ? "absent" : "present"}`,
  });

  assert.throws(
    () => gate.requireAbsent("people.txt", /placeholder/, "dead/demo HR product copy"),
    /people\.txt still contains dead\/demo HR product copy/,
  );
  assert.throws(
    () => gate.requireIncludes("people.txt", "operations", "people operations command panel"),
    /people\.txt is missing people operations command panel: operations/,
  );
});

test("requireMatches and requireAbsent reset global regex state", () => {
  const root = withFixture({
    "sample.txt": "alpha ready",
  });
  const gate = createTextGate({ root });
  const readyPattern = /ready/g;
  const forbiddenPattern = /alpha/g;

  assert.doesNotThrow(() => gate.requireMatches("sample.txt", readyPattern, "first ready check"));
  assert.doesNotThrow(() => gate.requireMatches("sample.txt", readyPattern, "second ready check"));
  assert.equal(readyPattern.lastIndex, 0);

  assert.throws(
    () => gate.requireAbsent("sample.txt", forbiddenPattern, "first alpha absence check"),
    /first alpha absence check: sample\.txt must not match \/alpha\/g/,
  );
  assert.throws(
    () => gate.requireAbsent("sample.txt", forbiddenPattern, "second alpha absence check"),
    /second alpha absence check: sample\.txt must not match \/alpha\/g/,
  );
  assert.equal(forbiddenPattern.lastIndex, 0);
});

test("reportGate prints the gate message with recorded check count", () => {
  const root = withFixture({
    "sample.txt": "alpha",
  });
  const gate = createTextGate({ root });
  gate.requireIncludes("sample.txt", "alpha", "has alpha");

  const originalLog = console.log;
  const logs = [];
  console.log = (message) => logs.push(message);
  try {
    gate.reportGate("custom gate passed");
  } finally {
    console.log = originalLog;
  }

  assert.deepEqual(logs, ["custom gate passed (1 checks)"]);
});
