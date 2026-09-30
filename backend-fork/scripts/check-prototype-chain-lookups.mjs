#!/usr/bin/env node
/**
 * Prototype-chain lookup census (console-g14a / console-i91.1).
 *
 * Mechanical fail-closed scan over scripts/check-*.mjs and scripts/console/**
 * for untrusted-keyed map reads that consult Object.prototype:
 *
 *   - obj?.[key] / obj?.[key.prop]     (optional computed member, non-literal key)
 *   - obj[key] !== undefined           (including obj.prop[key] !== undefined)
 *   - key in obj                       (identifier `in` identifier; not for-in)
 *
 * Literal keys ('x', "x", `x`, 0) are out of class. A named residual register
 * (scripts/prototype-chain-lookup-baseline.json) admits known pre-sweep sites;
 * unknown findings fail, stale register entries fail, missing listed subjects
 * fail closed, and examined-zero fails on files actually read (not declared).
 *
 * This is the total primitive that replaces another spelling-list patch round
 * on the i91 tip. Call-site ownership for clearing residuals remains with the
 * sweep bead (console-i91) or a follow-up that shrinks the register.
 */

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const BASELINE_REL = "scripts/prototype-chain-lookup-baseline.json";
/** Measured 50+ against scripts/check-*.mjs + scripts/console/** at g14a tip; collapse, not trim. */
export const SCANNED_FLOOR = 45;
export const SELF_REL = "scripts/check-prototype-chain-lookups.mjs";

// Top-level console/*.mjs is NOT matched by console/**/*.mjs under git pathspec rules.
const SUBJECT_GLOBS = [
  "scripts/check-*.mjs",
  "scripts/console/*.mjs",
  "scripts/console/**/*.mjs",
];

/** Optional computed member with a non-literal key. */
const OPTIONAL_COMPUTED = /\?\.\s*\[([^\]]+)\]/g;
/** Bracket read compared to undefined with a non-literal key. */
const UNDEFINED_COMPARE = /([\w$.]+)\s*\[\s*([^\]]+?)\]\s*!==\s*undefined/g;
/** `key in obj` — identifier operands only (excludes most prose / for-in headers). */
const IN_OPERATOR = /(?<![.\w$])([A-Za-z_$][\w$]*)\s+in\s+([A-Za-z_$][\w$]*)\b/g;

/**
 * @param {string} keyExpr
 * @returns {boolean}
 */
export function isLiteralKeyExpr(keyExpr) {
  const key = keyExpr.trim();
  if (/^(['"])(?:\\.|(?!\1).)*\1$/.test(key)) return true;
  if (/^`(?:\\.|[^`\\])*`$/.test(key)) return true;
  if (/^\d+(?:\.\d+)?$/.test(key)) return true;
  return false;
}

/**
 * @param {string} source
 * @param {number} index
 */
function isQuotedOrCommented(source, index) {
  const lineStart = source.lastIndexOf("\n", index - 1) + 1;
  const before = source.slice(lineStart, index);
  if (/^\s*(\/\/|\/?\*)/.test(before)) return true;
  return (before.match(/["'`]/g) ?? []).length % 2 === 1;
}

/**
 * @param {string} source
 * @param {number} index
 */
function lineOf(source, index) {
  return source.slice(0, index).split("\n").length;
}

/**
 * @param {string} source
 * @param {number} index
 */
function lineText(source, index) {
  const lineStart = source.lastIndexOf("\n", index - 1) + 1;
  let lineEnd = source.indexOf("\n", index);
  if (lineEnd === -1) lineEnd = source.length;
  return source.slice(lineStart, lineEnd).trim();
}

/**
 * for-in headers look like `for (const key in obj)` — skip those `in` matches.
 * @param {string} source
 * @param {number} index
 */
function isForInHeader(source, index) {
  const lineStart = source.lastIndexOf("\n", index - 1) + 1;
  const before = source.slice(lineStart, index);
  return /\bfor\s*\(\s*(?:await\s+)?(?:const|let|var)?\s*$/.test(before)
    || /\bfor\s*\(\s*(?:await\s+)?(?:const|let|var)\s+[A-Za-z_$][\w$]*\s+$/.test(before);
}

/**
 * Skip `in` matches that sit inside a same-line /regex literal/ (ponytail: quote/slash parity).
 * @param {string} source
 * @param {number} index
 */
function isInsideRegexLiteral(source, index) {
  const lineStart = source.lastIndexOf("\n", index - 1) + 1;
  let lineEnd = source.indexOf("\n", index);
  if (lineEnd === -1) lineEnd = source.length;
  const line = source.slice(lineStart, lineEnd);
  const local = index - lineStart;
  let i = 0;
  while (i < line.length) {
    const ch = line[i];
    if (ch === "/" && line[i + 1] === "/") break;
    if (ch === "'" || ch === '"' || ch === "`") {
      const quote = ch;
      i += 1;
      while (i < line.length) {
        if (line[i] === "\\") {
          i += 2;
          continue;
        }
        if (line[i] === quote) {
          i += 1;
          break;
        }
        i += 1;
      }
      continue;
    }
    if (ch === "/") {
      const start = i;
      i += 1;
      while (i < line.length) {
        if (line[i] === "\\") {
          i += 2;
          continue;
        }
        if (line[i] === "/") {
          if (local > start && local < i) return true;
          i += 1;
          break;
        }
        i += 1;
      }
      continue;
    }
    i += 1;
  }
  return false;
}

/**
 * Line is required so identical snippets at distinct sites stay distinct
 * (e.g. check-ci-preflight workflowModel.jobs?.[jobName] at two lines).
 * @param {{ file: string, line: number, kind: string, key: string, snippet: string }} finding
 */
export function findingId(finding) {
  return `${finding.file}|${finding.line}|${finding.kind}|${finding.key}|${finding.snippet}`;
}

/**
 * @param {string} source
 * @param {string} file
 */
function scanSource(source, file) {
  /** @type {{ file: string, line: number, kind: string, key: string, snippet: string }[]} */
  const findings = [];

  for (const match of source.matchAll(OPTIONAL_COMPUTED)) {
    const key = match[1].trim();
    if (isLiteralKeyExpr(key)) continue;
    if (isQuotedOrCommented(source, match.index)) continue;
    findings.push({
      file,
      line: lineOf(source, match.index),
      kind: "optional-computed",
      key,
      snippet: lineText(source, match.index),
    });
  }

  for (const match of source.matchAll(UNDEFINED_COMPARE)) {
    const key = match[2].trim();
    if (isLiteralKeyExpr(key)) continue;
    if (isQuotedOrCommented(source, match.index)) continue;
    findings.push({
      file,
      line: lineOf(source, match.index),
      kind: "undefined-compare",
      key,
      snippet: lineText(source, match.index),
    });
  }

  for (const match of source.matchAll(IN_OPERATOR)) {
    if (isQuotedOrCommented(source, match.index)) continue;
    if (isForInHeader(source, match.index)) continue;
    if (isInsideRegexLiteral(source, match.index)) continue;
    const key = match[1];
    const obj = match[2];
    // English prose leftovers that still match identifier-in-identifier.
    if (key === "listed" || key === "required" || obj === "this") continue;
    findings.push({
      file,
      line: lineOf(source, match.index),
      kind: "in-operator",
      key,
      snippet: lineText(source, match.index),
    });
  }

  return findings;
}

/**
 * @param {string} root
 * @returns {string[]}
 */
export function listSubjectFiles(root) {
  const listed = execFileSync("git", ["-C", root, "ls-files", "-z", "--", ...SUBJECT_GLOBS], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  return listed
    .split("\0")
    .filter(Boolean)
    .filter((file) => !file.endsWith(".test.mjs"))
    .filter((file) => file !== SELF_REL)
    .filter((file) => file !== BASELINE_REL)
    .sort();
}

/**
 * @param {string} root
 * @param {{ baselinePath?: string, subjects?: string[] }} [options]
 */
export function evaluatePrototypeChainLookups(root, options = {}) {
  const baselinePath = options.baselinePath ?? join(root, BASELINE_REL);
  const subjects = options.subjects ?? listSubjectFiles(root);
  /** @type {{ file: string, line: number, kind: string, key: string, snippet: string }[]} */
  const findings = [];
  /** @type {string[]} */
  const missingSubjects = [];
  let scanned = 0;

  for (const file of subjects) {
    const abs = resolve(root, file);
    if (!existsSync(abs)) {
      missingSubjects.push(file);
      continue;
    }
    findings.push(...scanSource(readFileSync(abs, "utf8"), file));
    scanned += 1;
  }

  findings.sort((a, b) =>
    a.file.localeCompare(b.file)
    || a.line - b.line
    || a.kind.localeCompare(b.kind)
    || a.key.localeCompare(b.key));

  const belowFloor = scanned < SCANNED_FLOOR;

  if (!existsSync(baselinePath)) {
    return {
      scanned,
      findings,
      unknown: findings,
      stale: [],
      missingSubjects,
      baselineMissing: true,
      belowFloor,
    };
  }

  const baseline = JSON.parse(readFileSync(baselinePath, "utf8"));
  if (baseline.schema_version !== 1 || !Array.isArray(baseline.residuals)) {
    throw new Error(`${BASELINE_REL}: schema_version must be 1 with residuals[]`);
  }

  for (const entry of baseline.residuals) {
    if (!Number.isInteger(entry.line) || entry.line < 1) {
      throw new Error(`${BASELINE_REL}: residual for ${entry.file} requires positive integer line`);
    }
  }

  const residualIds = new Set(
    baseline.residuals.map((entry) => findingId({
      file: entry.file,
      line: entry.line,
      kind: entry.kind,
      key: entry.key,
      snippet: entry.snippet,
    })),
  );
  const observedIds = new Set(findings.map(findingId));

  const unknown = findings.filter((finding) => !residualIds.has(findingId(finding)));
  const stale = baseline.residuals.filter((entry) => !observedIds.has(findingId(entry)));

  return {
    scanned,
    findings,
    unknown,
    stale,
    missingSubjects,
    baselineMissing: false,
    belowFloor,
    baselineCount: baseline.residuals.length,
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = resolve(process.argv[2] ?? join(dirname(fileURLToPath(import.meta.url)), ".."));
  const result = evaluatePrototypeChainLookups(root);

  if (result.baselineMissing) {
    console.error(`${BASELINE_REL}: missing (must ship with named residuals or residuals: [])`);
  }
  if (result.missingSubjects.length > 0) {
    console.error(
      `listed subject path(s) missing on disk (fail closed; scanned counts only files read): `
        + result.missingSubjects.join(", "),
    );
  }
  if (result.belowFloor) {
    console.error(
      `scanned ${result.scanned} subject files, below the floor of ${SCANNED_FLOOR} — `
        + "the census examined less of scripts/check-* + scripts/console than it was built to cover",
    );
  }
  for (const finding of result.unknown) {
    console.error(
      `${finding.file}:${finding.line}: ${finding.kind} key=${JSON.stringify(finding.key)} `
        + `(not in ${BASELINE_REL}): ${finding.snippet}`,
    );
  }
  for (const entry of result.stale) {
    console.error(
      `${BASELINE_REL}: stale residual no longer observed: `
        + `${entry.file}:${entry.line} ${entry.kind} key=${JSON.stringify(entry.key)}`,
    );
  }

  const failed = result.baselineMissing
    || result.missingSubjects.length > 0
    || result.belowFloor
    || result.unknown.length > 0
    || result.stale.length > 0;

  if (failed) {
    console.error(
      `prototype-chain lookup census FAILED: scanned=${result.scanned} `
        + `missing=${result.missingSubjects.length} `
        + `findings=${result.findings.length} unknown=${result.unknown.length} `
        + `stale=${result.stale.length}`,
    );
    process.exit(1);
  }

  console.log(
    `prototype-chain lookup census passed `
      + `(scanned ${result.scanned}, residuals ${result.baselineCount}, findings ${result.findings.length})`,
  );
}
