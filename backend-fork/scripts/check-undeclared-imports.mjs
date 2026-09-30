// H-4 gate: every bare import specifier must be declared in the nearest package.json.
//
// The hole this closes: a file imports a package that is in no manifest and no lockfile.
// `node --test` then dies with ERR_MODULE_NOT_FOUND — a LOAD failure, zero assertion failures,
// and a CI step that reads as a suite that ran. The gate is static: it reads the tracked script
// surface and the manifests, never installed node_modules, so a stale local install cannot make
// it green.
//
// ONE EXCEPTION, and it is named rather than hidden: ARCHIVED_EVIDENCE below. An evidence
// snapshot under docs/evidence/ is not the test suite — it is the instrument of a recorded
// verification result, and its imports are a historical record of tooling that WAS declared when
// the run happened, not a live dependency claim. docs/evidence/console/wave4/L-F1/
// browser-window-host.mjs imports `playwright`, which `web/package.json` declared until 962fb98b7
// deleted the whole frontend; four citations (report.md:101,107 and verification.md:43,197) rest
// on that script, one of them recording `10/10 checks passed`. Deleting it to make this gate
// green would trade audit evidence for a green light.
//
// WHAT WOULD MAKE AN ENTRY IN THIS CLASS A REAL DEFECT: an archived evidence artifact that CI
// executes. The class is safe only because nothing under docs/evidence/ is invoked by any
// workflow — check that before adding a prefix. The exclusion is counted in every run's output
// and is covered by two tests, one of which passes archived: [] and observes the gate go red on
// the real artifact.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

export const ARCHIVED_EVIDENCE = ["docs/evidence/"];

// Measured 98 against the tracked script surface, leaving headroom for ordinary deletion; a
// collapse, not a trim, is what this catches. It lives in the GATE and not only in the sibling
// test because the test is reachable through exactly one unlocked string: the `node --test ... &&`
// prefix of `check:undeclared-imports` in package.json. Deleting that prefix was verified to leave
// check:foundation-gates and check:ci-preflight both exiting 0 — neither compares script bodies —
// and the gate binary then printed `undeclared imports gate passed (0 files scanned)` and exited 0
// when pointed at a subtree with no tracked scripts. A gate whose green says it looked at nothing
// is the false green this slice exists to close.
export const SCANNED_FLOOR = 90;

// `import x from "p"`, `export * from "p"`, `import("p")`, `require("p")`, bare `import "p";` —
// all reduce to a quoted specifier in a position ordinary prose does not occupy.
const SPECIFIER_PATTERNS = [
  // `[^;]` and not `[\s\S]`: the gap may wrap lines, but never a statement boundary. With the
  // permissive class this file's own `export const ARCHIVED_EVIDENCE = [...];` bonded to the
  // `from "p"` two lines below it and reported a package named "p".
  /\b(?:import|export)\s[^;]{0,400}?\bfrom\s*["']([^"']+)["']/g,
  /\bimport\s*\(\s*["']([^"']+)["']\s*\)/g,
  /\brequire\s*\(\s*["']([^"']+)["']\s*\)/g,
  /(?:^|[;{}\n])\s*import\s+["']([^"']+)["']\s*;/gm,
];

const PACKAGE_NAME = /^(@[a-z0-9-~][\w.-]*\/)?[a-z0-9-~][\w.-]*$/;

function packageName(specifier) {
  const segments = specifier.split("/");
  return specifier.startsWith("@") ? segments.slice(0, 2).join("/") : segments[0];
}

function declaredIn(manifestPath) {
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  return new Set([
    ...Object.keys(manifest.dependencies ?? {}),
    ...Object.keys(manifest.devDependencies ?? {}),
    ...Object.keys(manifest.optionalDependencies ?? {}),
    ...Object.keys(manifest.peerDependencies ?? {}),
  ]);
}

// Nearest manifest wins: a nested package.json may declare a dependency the root does not, and
// resolving files under it against the root manifest would report a false finding. Only the root
// manifest exists at HEAD, so this walk currently always lands there; it stays so that adding a
// nested manifest does not silently start producing false findings.
function nearestDeclarations(root, file, cache) {
  let directory = dirname(resolve(root, file));
  const stop = resolve(root);
  for (;;) {
    const manifestPath = join(directory, "package.json");
    if (existsSync(manifestPath)) {
      if (!cache.has(manifestPath)) cache.set(manifestPath, declaredIn(manifestPath));
      return cache.get(manifestPath);
    }
    if (directory === stop || dirname(directory) === directory) return new Set();
    directory = dirname(directory);
  }
}

function lineOf(source, index) {
  return source.slice(0, index).split("\n").length;
}

// ponytail: quote parity, not a JS tokenizer. A match whose line begins a comment, or which is
// preceded on its line by an odd number of quotes, is text about an import rather than one —
// which is how this file's own header comment and the test fixtures' inline sources read. Both
// misjudgements fail RED (a spurious finding), never green, except for a real import preceded on
// the same line by an unbalanced quote. Upgrade path if that ever appears: a real string/comment
// mask over the source.
function isQuotedOrCommented(source, index) {
  const lineStart = source.lastIndexOf("\n", index - 1) + 1;
  const before = source.slice(lineStart, index);
  if (/^\s*(\/\/|\/?\*)/.test(before)) return true;
  return (before.match(/["'`]/g) ?? []).length % 2 === 1;
}

/**
 * @param {string} root repository root to scan
 * @param {string[]} archived path prefixes classified as archived evidence rather than live code.
 *   Pass [] to scan everything — that is how the test proves the classification is load-bearing.
 * @returns {{
 *   scanned: number,
 *   excluded: string[],
 *   findings: { file: string, line: number, specifier: string }[],
 * }}
 */
export function evaluateUndeclaredImports(root, archived = ARCHIVED_EVIDENCE) {
  const listed = execFileSync("git", ["-C", root, "ls-files", "--", "*.mjs", "*.js", "*.cjs"], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  const tracked = listed.split("\n").filter(Boolean).filter((file) => !file.split(sep).includes("node_modules"));
  const excluded = tracked.filter((file) => archived.some((prefix) => file.startsWith(prefix)));
  const files = tracked.filter((file) => !excluded.includes(file));
  const manifests = new Map();
  const findings = [];

  for (const file of files) {
    const source = readFileSync(resolve(root, file), "utf8");
    const declared = nearestDeclarations(root, file, manifests);
    const seen = new Set();
    for (const pattern of SPECIFIER_PATTERNS) {
      for (const match of source.matchAll(pattern)) {
        const specifier = match[1];
        // Anchor on the keyword, not on match.index. The bare-import pattern begins at the
        // statement delimiter BEFORE `import`, and when that delimiter is a newline match.index
        // sits on the previous line: `isQuotedOrCommented` then judged that line, so
        // `// polyfill\nimport "phantom";` was discarded as commented — a false GREEN on the
        // only form pattern 4 exists to catch — and `lineOf` named the line above the import.
        const at = match.index + match[0].search(/[^\s;{}]/);
        if (specifier.startsWith(".") || specifier.startsWith("/") || specifier.startsWith("node:")) continue;
        if (isQuotedOrCommented(source, at)) continue;
        const name = packageName(specifier);
        // A specifier that is not a legal package name is prose caught by a loose regex, not a
        // dependency. False findings get allowlisted, and an allowlist is how a gate dies.
        if (!PACKAGE_NAME.test(name) || declared.has(name) || seen.has(name)) continue;
        seen.add(name);
        findings.push({ file, line: lineOf(source, at), specifier: name });
      }
    }
  }

  findings.sort((a, b) => a.file.localeCompare(b.file) || a.line - b.line || a.specifier.localeCompare(b.specifier));
  return { scanned: files.length, excluded, findings };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = process.argv[2] ?? fileURLToPath(new URL("..", import.meta.url));
  const { scanned, excluded, findings } = evaluateUndeclaredImports(root);
  for (const finding of findings) {
    console.error(`${finding.file}:${finding.line}: imports "${finding.specifier}", `
      + "which no package.json above it declares");
  }
  // Printed on every run, pass or fail. Silent truncation reads as "we covered everything".
  console.log(`excluded ${excluded.length} archived evidence file${excluded.length === 1 ? "" : "s"}`
    + `${excluded.length > 0 ? `: ${excluded.join(", ")}` : ""}`);
  const belowFloor = scanned < SCANNED_FLOOR;
  if (belowFloor) {
    console.error(`scanned ${scanned} files, below the floor of ${SCANNED_FLOOR} — the scan covered `
      + "less of the script surface than it was built to cover; re-measure before lowering this");
  }
  if (findings.length > 0 || belowFloor) {
    console.error(`undeclared imports gate FAILED: ${findings.length} undeclared specifier(s) across ${scanned} files`);
    process.exit(1);
  }
  console.log(`undeclared imports gate passed (${scanned} file${scanned === 1 ? "" : "s"} scanned)`);
}
