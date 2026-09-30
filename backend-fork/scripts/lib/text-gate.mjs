import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { noteAssertion, noteRead } from "./gate-inputs.mjs";

export function createTextGate(options = {}) {
  const {
    root = process.cwd(),
    gateName = "text gate",
    includeFailure = ({ path, needle, label }) =>
      `${label}: expected ${path} to include ${JSON.stringify(needle)}`,
    notIncludeFailure = ({ path, needle, label }) =>
      `${label}: ${path} must not include ${JSON.stringify(needle)}`,
    matchFailure = ({ path, pattern, label }) =>
      `${label}: expected ${path} to match ${pattern}`,
    absentFailure = ({ path, pattern, label }) =>
      `${label}: ${path} must not match ${pattern}`,
    passLabel = (label) => label,
  } = options;
  const checks = [];
  const cache = new Map();

  function read(path) {
    const resolved = resolve(root, path);
    if (!cache.has(resolved)) {
      cache.set(resolved, readFileSync(resolved, "utf8"));
    }
    noteRead(path);
    return cache.get(resolved);
  }

  function record(label, kind, path) {
    checks.push(passLabel(label, kind));
    if (path) noteAssertion(path);
  }

  function requireIncludes(path, needle, label) {
    const text = read(path);
    if (!text.includes(needle)) {
      throw new Error(includeFailure({ path, needle, label }));
    }
    record(label, "include", path);
  }

  function requireNotIncludes(path, needle, label) {
    const text = read(path);
    if (text.includes(needle)) {
      throw new Error(notIncludeFailure({ path, needle, label }));
    }
    record(label, "notInclude", path);
  }

  function testPattern(pattern, text) {
    pattern.lastIndex = 0;
    const matched = pattern.test(text);
    pattern.lastIndex = 0;
    return matched;
  }

  function requireMatches(path, pattern, label) {
    const text = read(path);
    if (!testPattern(pattern, text)) {
      throw new Error(matchFailure({ path, pattern, label }));
    }
    record(label, "match", path);
  }

  function requireAbsent(path, pattern, label) {
    const text = read(path);
    if (testPattern(pattern, text)) {
      throw new Error(absentFailure({ path, pattern, label }));
    }
    record(label, "absent", path);
  }

  function reportGate(message = `${gateName} gate passed`) {
    console.log(`${message} (${checks.length} checks)`);
  }

  return {
    checks,
    read,
    requireIncludes,
    requireNotIncludes,
    requireMatches,
    requireAbsent,
    reportGate,
  };
}
