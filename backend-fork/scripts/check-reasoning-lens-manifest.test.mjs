#!/usr/bin/env node
/**
 * Mutation tests for the retained lens-manifest invariant.
 *
 * The gate this replaced shipped with 620 lines of test. Its replacement
 * shipped with none, which is how two evasions survived review: a duplicate
 * marked block, and a canonical entry written with an ASCII hyphen. Every case
 * below is one of those, or one a reviewer named before it could be exploited.
 *
 * Each test mutates a valid fixture and asserts the gate goes RED. A gate that
 * only ever sees the checked-in files is not a gate -- it is a green light that
 * happens to be pointed at working code.
 */
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { canonicalLenses, expectedProjection, manifestFailures, markedBlock, PROJECTIONS, SOURCE }
  from "./check-reasoning-lens-manifest.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (p) => readFileSync(resolve(root, p), "utf8");

const AGENTS = read(SOURCE);
const LIVE = Object.fromEntries(PROJECTIONS.map((p) => [p, read(p)]));

const START = "<!-- SHARED:REASONING-LENSES:START -->";
const END = "<!-- SHARED:REASONING-LENSES:END -->";

test("the checked-in repository satisfies the invariant", () => {
  assert.deepEqual(manifestFailures(AGENTS, LIVE), []);
  assert.equal(canonicalLenses(AGENTS).length, 16);
});

test("a rename in the source alone is caught", () => {
  // The first implementation compared a duplicate array against the projection,
  // so the source could drift with every projection stale and the gate green.
  const mutated = AGENTS.replace("**Red Team**", "**Purple Team**");
  assert.notEqual(mutated, AGENTS);
  const failures = manifestFailures(mutated, LIVE);
  assert.ok(failures.length >= PROJECTIONS.length, "every projection must fail");
  assert.match(failures[0], /drifted from AGENTS\.md/);
});

test("an appended lens in a projection is caught", () => {
  // `includes()` accepted this: all 16 canonical lines were still present.
  const claude = LIVE["CLAUDE.md"].replace(
    "16. Zero-trust / defense-in-depth",
    "16. Zero-trust / defense-in-depth\n17. Unapproved extra lens",
  );
  const failures = manifestFailures(AGENTS, { ...LIVE, "CLAUDE.md": claude });
  assert.equal(failures.length, 1);
  assert.match(failures[0], /CLAUDE\.md/);
});

test("a duplicated marked block is caught", () => {
  // Outside the first marker pair, so whole-block equality never saw it: the
  // projection carried 32 identifiers while the first 16 still matched.
  const claude = LIVE["CLAUDE.md"];
  const block = claude.slice(claude.indexOf(START), claude.indexOf(END) + END.length);
  const failures = manifestFailures(AGENTS, { ...LIVE, "CLAUDE.md": `${claude}\n\n${block}\n` });
  assert.equal(failures.length, 1);
  assert.match(failures[0], /markers are missing or inverted|drifted/);
});

test("an unparseable canonical entry fails closed rather than being skipped", () => {
  // An ASCII hyphen instead of an em dash made the regex miss the entry, so a
  // newly required lens never reached the projections and nothing went red.
  const mutated = AGENTS.replace(
    END,
    "17. **New required lens** - definition written with an ASCII hyphen\n" + END,
  );
  const failures = manifestFailures(mutated, LIVE);
  assert.equal(failures.length, 1);
  assert.match(failures[0], /cannot parse the canonical lens list/);
});

test("a blank canonical identifier fails closed", () => {
  // Renaming a lens to whitespace keeps the count at 16 and, if both
  // projections are updated to match, leaves every gate green with an
  // unnameable lens in the manifest.
  const mutated = AGENTS.replace("**Socratic**", "**   **");
  assert.match(manifestFailures(mutated, LIVE)[0], /cannot parse the canonical lens list/);
});

test("a duplicated canonical identifier fails closed", () => {
  // 16 entries, 15 distinct choices: selection becomes ambiguous while the
  // count assertion still passes.
  const mutated = AGENTS.replace("**Socratic**", "**Red Team**");
  assert.match(manifestFailures(mutated, LIVE)[0], /cannot parse the canonical lens list/);
});

test("a parenthesized ordered-list entry fails closed", () => {
  // `17)` renders as an ordered-list item exactly like `17.`, so a guard keyed
  // on the dot alone treated it as prose and the new lens never reached either
  // projection.
  for (const delimiter of [")", "]", ":"]) {
    const mutated = AGENTS.replace(
      "<!-- SHARED:REASONING-LENSES:END -->",
      `17${delimiter} **New required lens** — definition\n<!-- SHARED:REASONING-LENSES:END -->`,
    );
    assert.match(
      manifestFailures(mutated, LIVE)[0] ?? "",
      /cannot parse the canonical lens list/,
      `delimiter ${delimiter} must fail closed`,
    );
  }
});

test("renumbering the canonical list fails closed", () => {
  const mutated = AGENTS.replace("2. **Essentialism / YAGNI**", "3. **Essentialism / YAGNI**");
  assert.match(manifestFailures(mutated, LIVE)[0], /cannot parse the canonical lens list/);
});

test("a projection missing its markers is caught, not skipped", () => {
  const failures = manifestFailures(AGENTS, { ...LIVE, "README.md": "# no markers here" });
  assert.equal(failures.length, 1);
  assert.match(failures[0], /README\.md: the SHARED:REASONING-LENSES markers/);
});

test("both projections are checked, so drift in either is caught", () => {
  for (const path of PROJECTIONS) {
    const mutated = { ...LIVE, [path]: LIVE[path].replace("7. Red Team", "7. Blue Team") };
    const failures = manifestFailures(AGENTS, mutated);
    assert.equal(failures.length, 1, `${path} drift must be caught`);
    assert.match(failures[0], new RegExp(path.replace(".", "\\.")));
  }
});

test("markedBlock and expectedProjection agree on the live files", () => {
  const expected = expectedProjection(canonicalLenses(AGENTS));
  for (const path of PROJECTIONS) assert.equal(markedBlock(LIVE[path]), expected);
});
