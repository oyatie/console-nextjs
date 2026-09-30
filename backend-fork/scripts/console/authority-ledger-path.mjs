/**
 * The one path prefix at which the authority tip T may ADD a file, and the diff flags every
 * reader of the C..T diff must use.
 *
 * THREE validators consume that one diff — `verify-console-authority-train.mjs`,
 * `verify-console-pr-authority-bootstrap.mjs` (the protected-target verifier that is the current
 * live merge authority), and `validate-console-truth-ledger.mjs`. General CI exercises the other
 * validators as regressions rather than live admission. They held three copies of the predicate
 * and two different flag sets, and that is not a style problem: with
 * `--find-renames --find-copies-harder` a new ledger entry ≥50% similar to a file already in
 * the tree is reported as status `C` with two paths, and with `--no-renames` as status `A` with
 * one. The same commit was therefore refused by one consumer and accepted by another.
 * One definition, imported three times, is the only shape in which they cannot
 * disagree again.
 */

export const LEDGER_DIRECTORY = 'docs/program/ledger/';

/**
 * `--no-renames` is the strict reading. Rename and copy detection can only ever RELABEL an
 * added file as `R`/`C`, and every reader here refuses `R` and `C` outright — so detection
 * turns an admissible entry into a refusal, in whichever reader happens to enable it.
 */
export const AUTHORITY_DIFF_ARGS = Object.freeze(['diff', '--raw', '-z', '--abbrev=40', '--no-renames', '--no-ext-diff']);

/**
 * A FLAT directory of lowercase `.md` entries. Both halves are load-bearing:
 *
 * - Flat. A bare `startsWith` is a string test, not a path test, so it accepted
 *   `docs/program/ledger/../../evil` — a path that names a location outside the prefix
 *   entirely. Refusing any `/` in the remainder removes traversal and nesting in one clause:
 *   a segment cannot be `..` when there are no segments.
 * - `.md`. This prefix is the only place the authority tip may add a file at all. Without an
 *   extension constraint the same allowance admits a new `.mjs` under `docs/` — executable
 *   content, added by the one commit that is otherwise forbidden to touch a product path, and
 *   reviewed as if it were prose. The directory exists to hold prose; saying so costs a regex.
 */
const LEDGER_ENTRY_NAME = /^[a-z0-9][a-z0-9.-]*\.md$/;

export function isLedgerEntryPath(value) {
  if (typeof value !== 'string' || !value.startsWith(LEDGER_DIRECTORY)) return false;
  const name = value.slice(LEDGER_DIRECTORY.length);
  return LEDGER_ENTRY_NAME.test(name) && !name.includes('..');
}
