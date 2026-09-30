#!/usr/bin/env node
/**
 * Reasoning-lens MANIFEST drift check.
 *
 * `AGENTS.md` is the source. `CLAUDE.md` and `README.md` each carry an
 * identifier-only projection of its lens list, and each tells the reader that
 * projection is drift-checked. This enforces that claim.
 *
 * Three properties, each learned from a way an earlier revision of this file
 * failed to hold them:
 *
 * 1. The expected list is PARSED FROM `AGENTS.md`, not from an array in this
 *    module. A hardcoded copy means renaming a lens in AGENTS.md and nowhere
 *    else leaves every projection stale with the gate green -- it would check
 *    a duplicate against a duplicate.
 * 2. The whole marked block is compared, not searched for. `includes()` accepts
 *    a block that keeps all sixteen lines and appends a seventeenth, so
 *    additions, duplicates and trailing drift pass unnoticed.
 * 3. Every file carrying the marker is checked. Dropping README from the sweep
 *    leaves the repository's entry point stale while the gate passes.
 *
 * This replaces `check-reasoning-lens-contract.mjs`, which also required a
 * per-record `lens_contract` evidence block on every governed document. That
 * half is retired: it could only verify the JSON's shape -- replacing all six
 * rationales in a real ledger with "banana banana banana" left it green -- so
 * it asserted that reasoning happened, which is not decidable from text.
 */
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const START = "<!-- SHARED:REASONING-LENSES:START -->";
const END = "<!-- SHARED:REASONING-LENSES:END -->";

export const SOURCE = "AGENTS.md";
export const PROJECTIONS = Object.freeze(["CLAUDE.md", "README.md"]);

const PREAMBLE = "## Reasoning lens manifest\n\nCanonical definitions and routing rules live in "
  + "[AGENTS.md](AGENTS.md#task-selected-reasoning-lenses). This identifier-only projection is "
  + "drift-checked and does not duplicate policy.";

/**
 * The marked block's interior, or null when the markers are missing, inverted,
 * or repeated. Exactly one pair is required: a second complete block after the
 * first END marker is invisible to whole-block equality, so a projection could
 * carry 32 identifiers while the first 16 still matched.
 */
export function markedBlock(text) {
  const starts = [...text.matchAll(new RegExp(START.replace(/[-[\]{}()*+?.,\\^$|#\s]/g, "\\$&"), "g"))];
  const ends = [...text.matchAll(new RegExp(END.replace(/[-[\]{}()*+?.,\\^$|#\s]/g, "\\$&"), "g"))];
  if (starts.length !== 1 || ends.length !== 1) return null;
  const from = starts[0].index + START.length;
  const to = ends[0].index;
  if (to < from) return null;
  return text.slice(from, to).trim();
}

/** Canonical lens names, in order, parsed from the source's numbered bold entries. */
export function canonicalLenses(agentsText) {
  const block = markedBlock(agentsText);
  if (block === null) return null;
  const names = [];
  for (const line of block.split("\n")) {
    const trimmed = line.trim();
    // Any numbered entry must parse, whatever ordered-list delimiter it uses.
    // Skipping unrecognised ones let a new lens written with an ASCII hyphen
    // instead of an em dash be treated as prose; keying the guard on `.` alone
    // then let `17) **Lens** — definition`, which Markdown renders identically,
    // slip through the same way.
    if (!/^\d+\s*[.)\]:]/.test(trimmed)) continue;
    const match = /^(\d+)\.\s+\*\*(.+?)\*\*\s+—\s+\S/.exec(trimmed);
    if (!match) return null;
    if (Number(match[1]) !== names.length + 1) return null;
    // A name that trims to nothing, or repeats an earlier one, leaves the
    // manifest with a blank or ambiguous identifier while every count still
    // matches. Selection has to name a lens unambiguously, so both fail closed.
    const name = match[2].trim();
    if (!name || names.includes(name)) return null;
    names.push(name);
  }
  return names.length ? names : null;
}

export function expectedProjection(names) {
  return `${PREAMBLE}\n\n${names.map((name, i) => `${i + 1}. ${name}`).join("\n")}`;
}

export function manifestFailures(agentsText, projections) {
  const names = canonicalLenses(agentsText);
  if (!names) {
    return [`${SOURCE}: cannot parse the canonical lens list from the marked block`];
  }
  const expected = expectedProjection(names);
  const failures = [];
  for (const [path, text] of Object.entries(projections)) {
    const block = markedBlock(text);
    if (block === null) {
      failures.push(`${path}: the SHARED:REASONING-LENSES markers are missing or inverted`);
      continue;
    }
    // Whole-block equality: a projection that keeps every canonical line and
    // adds one more must fail.
    if (block !== expected) {
      const actual = block.split("\n");
      const wanted = expected.split("\n");
      const at = wanted.findIndex((line, i) => actual[i] !== line);
      const detail = at >= 0
        ? `line ${at + 1}: expected ${JSON.stringify(wanted[at])}, found ${JSON.stringify(actual[at] ?? "(end of block)")}`
        : `${actual.length - wanted.length} unexpected trailing line(s), first ${JSON.stringify(actual[wanted.length])}`;
      failures.push(`${path}: reasoning lens projection has drifted from ${SOURCE} — ${detail}`);
    }
  }
  return failures;
}

const isMain = process.argv[1] && process.argv[1].endsWith("check-reasoning-lens-manifest.mjs");
if (isMain) {
  const read = (p) => readFileSync(resolve(root, p), "utf8");
  const failures = manifestFailures(
    read(SOURCE),
    Object.fromEntries(PROJECTIONS.map((p) => [p, read(p)])),
  );
  if (failures.length) {
    console.error(failures.join("\n"));
    console.error(`Regenerate the numbered lists in ${PROJECTIONS.join(" and ")} from ${SOURCE}.`);
    process.exitCode = 1;
  } else {
    const count = canonicalLenses(read(SOURCE)).length;
    console.log(`reasoning lens manifest OK (${count} lenses, ${PROJECTIONS.length} projections)`);
  }
}
