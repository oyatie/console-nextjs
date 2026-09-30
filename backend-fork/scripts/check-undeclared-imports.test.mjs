import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, describe, it } from "node:test";
import { fileURLToPath } from "node:url";

import { SCANNED_FLOOR, evaluateUndeclaredImports } from "./check-undeclared-imports.mjs";
import { installGitFixtureEnvironment } from "./lib/git-fixture-environment.mjs";

installGitFixtureEnvironment();

const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const cli = fileURLToPath(new URL("./check-undeclared-imports.mjs", import.meta.url));
const fixtureRoots = [];

after(() => {
  for (const root of fixtureRoots) rmSync(root, { recursive: true, force: true });
});

// A fixture is a real git worktree: the gate discovers files through the index, so an
// unstaged fixture would silently scan nothing and every assertion below would read green.
function fixture(files) {
  const root = mkdtempSync(join(tmpdir(), "undeclared-imports-"));
  fixtureRoots.push(root);
  for (const [relative, contents] of Object.entries(files)) {
    const absolute = join(root, relative);
    mkdirSync(dirname(absolute), { recursive: true });
    writeFileSync(absolute, contents);
  }
  const init = spawnSync("git", ["-c", "init.defaultBranch=main", "init", "-q", root], { encoding: "utf8" });
  assert.equal(init.status, 0, init.stderr);
  const add = spawnSync("git", ["-C", root, "add", "--", ...Object.keys(files)], { encoding: "utf8" });
  assert.equal(add.status, 0, add.stderr);
  return root;
}

const emptyManifest = JSON.stringify({ name: "fixture", devDependencies: {} });

describe("undeclared import gate", () => {
  it("reports a bare specifier that the nearest package.json does not declare", () => {
    const root = fixture({
      "package.json": emptyManifest,
      "scripts/thing.mjs": 'import openapiTS from "openapi-typescript";\n\nexport default openapiTS;\n',
    });

    const { findings } = evaluateUndeclaredImports(root);

    assert.deepEqual(findings, [{ file: "scripts/thing.mjs", line: 1, specifier: "openapi-typescript" }]);
  });

  it("accepts the same import once the manifest declares it", () => {
    const root = fixture({
      "package.json": JSON.stringify({ name: "fixture", devDependencies: { "openapi-typescript": "7.10.0" } }),
      "scripts/thing.mjs": 'import openapiTS from "openapi-typescript";\n\nexport default openapiTS;\n',
    });

    const { findings } = evaluateUndeclaredImports(root);

    assert.deepEqual(findings, []);
  });

  it("resolves against the nearest package.json, not only the root one", () => {
    const root = fixture({
      "package.json": emptyManifest,
      "tools/npm/compat/package.json": JSON.stringify({
        name: "compat",
        dependencies: { "minimatch-modern": "npm:minimatch@10.2.5" },
      }),
      "tools/npm/compat/index.cjs": 'const minimatch = require("minimatch-modern");\n\nmodule.exports = minimatch;\n',
    });

    const { findings } = evaluateUndeclaredImports(root);

    assert.deepEqual(findings, []);
  });

  it("does not report node: builtins or relative specifiers", () => {
    const root = fixture({
      "package.json": emptyManifest,
      "scripts/thing.mjs": [
        'import { readFileSync } from "node:fs";',
        'import assert from "node:assert/strict";',
        'import helper from "./helper.mjs";',
        'import shared from "../lib/shared.js";',
        "",
        "export default { readFileSync, assert, helper, shared };",
        "",
      ].join("\n"),
      "scripts/helper.mjs": "export default 1;\n",
      "lib/shared.js": "export default 2;\n",
    });

    const { findings } = evaluateUndeclaredImports(root);

    assert.deepEqual(findings, []);
  });

  it("covers require() and dynamic import(), not just static import", () => {
    const root = fixture({
      "package.json": emptyManifest,
      "scripts/required.cjs": 'const a = require("phantom-required");\n\nmodule.exports = a;\n',
      "scripts/dynamic.mjs": 'export const load = () => import("phantom-dynamic");\n',
    });

    const { findings } = evaluateUndeclaredImports(root);

    assert.deepEqual(
      findings.map((finding) => finding.specifier).sort(),
      ["phantom-dynamic", "phantom-required"],
    );
  });

  // The bare side-effect form is the only thing the fourth specifier pattern exists for, and it
  // had no test. The pattern starts at the statement delimiter before `import`, so with a newline
  // delimiter match.index landed on the PREVIOUS line: a bare import under a `//` comment was
  // discarded as commented, and one after any `;`-terminated line was reported against the wrong
  // line. Both fixture files below were invisible or misreported before the anchor fix.
  it("reports a side-effect-only bare import under a comment and after a statement", () => {
    const root = fixture({
      "package.json": emptyManifest,
      "scripts/commented.mjs": '// side-effect polyfill, loaded for its registration hook\nimport "phantom-bare";\n',
      "scripts/after-statement.mjs": 'export const marker = 1;\nimport "phantom-after";\n',
    });

    const { findings } = evaluateUndeclaredImports(root);

    assert.deepEqual(findings, [
      { file: "scripts/after-statement.mjs", line: 2, specifier: "phantom-after" },
      { file: "scripts/commented.mjs", line: 2, specifier: "phantom-bare" },
    ]);
  });

  it("reduces a subpath import to the package name before consulting the manifest", () => {
    const root = fixture({
      "package.json": JSON.stringify({
        name: "fixture",
        devDependencies: { "js-yaml": "4.3.0", "@scope/pkg": "1.0.0" },
      }),
      "scripts/thing.mjs": [
        'import load from "js-yaml/dist/js-yaml.mjs";',
        'import scoped from "@scope/pkg/sub/deep.js";',
        "",
        "export default { load, scoped };",
        "",
      ].join("\n"),
    });

    const { findings } = evaluateUndeclaredImports(root);

    assert.deepEqual(findings, []);
  });

  it("reports a scoped package that is absent from the manifest", () => {
    const root = fixture({
      "package.json": emptyManifest,
      "scripts/thing.mjs": 'import scoped from "@scope/absent/sub.js";\n',
    });

    const { findings } = evaluateUndeclaredImports(root);

    assert.deepEqual(findings.map((finding) => finding.specifier), ["@scope/absent"]);
  });

  it("does not mistake prose or SQL inside a string literal for an import specifier", () => {
    // The first three forms below never reach isQuotedOrCommented at all — none of them matches
    // SPECIFIER_PATTERNS, so on their own this test passed for a reason unrelated to its name and
    // stayed green with the suppression stubbed out. The last two do match, and are the only
    // lines here that exercise the comment check and the quote-parity check respectively.
    const root = fixture({
      "package.json": emptyManifest,
      "scripts/thing.mjs": [
        "export const sql = `",
        "  SELECT id from employees",
        "`;",
        'export const note = "import the roster from HR before payroll runs";',
        'export const sentinel = "RAW_GIT_SENTINEL_DO_NOT_LEAK";',
        '// import legacy from "phantom-commented";',
        // Written with the quotes escaped rather than as a template literal: as a template
        // literal this very line matched, and the gate reported "phantom-in-a-string" against
        // its own test file. That is the ceiling isQuotedOrCommented documents — a match on a
        // line whose quote count is even is not suppressed — and it fails loud, as claimed.
        'const quoted = "import x from \'phantom-in-a-string\'";',
        "",
      ].join("\n"),
    });

    const { findings } = evaluateUndeclaredImports(root);

    assert.deepEqual(findings, []);
  });

  // The exclusion is this gate's only escape hatch, and its edge is the one place a real finding
  // can vanish without anything being printed. A prefix of "docs/evidence" rather than
  // "docs/evidence/" silently takes the sibling with it.
  it("excludes docs/evidence/ without swallowing a sibling directory", () => {
    const root = fixture({
      "package.json": emptyManifest,
      "docs/evidence/archived.mjs": 'import { chromium } from "playwright";\n',
      "docs/evidence-notes/live.mjs": 'import { chromium } from "playwright";\n',
    });

    const { excluded, findings } = evaluateUndeclaredImports(root);

    assert.deepEqual(excluded, ["docs/evidence/archived.mjs"]);
    assert.deepEqual(findings, [{ file: "docs/evidence-notes/live.mjs", line: 1, specifier: "playwright" }]);
  });

  // A manifest ABOVE the scan root must not declare a finding away. `stop = resolve(root)` is the
  // only thing preventing that, and no other fixture could reach it: they all carry a root
  // package.json, so the walk always terminates before it can escape. This is the gate's one
  // false-GREEN vector — the directory above a checkout very often does have a package.json, and
  // a finding suppressed from outside the tree is suppressed silently.
  it("does not resolve a specifier against a package.json above the scan root", () => {
    const outer = mkdtempSync(join(tmpdir(), "undeclared-imports-outer-"));
    fixtureRoots.push(outer);
    writeFileSync(
      join(outer, "package.json"),
      JSON.stringify({ name: "outside-the-tree", devDependencies: { "openapi-typescript": "7.10.0" } }),
    );
    const root = join(outer, "repo");
    mkdirSync(join(root, "scripts"), { recursive: true });
    writeFileSync(join(root, "scripts/thing.mjs"), 'import openapiTS from "openapi-typescript";\n');
    const init = spawnSync("git", ["-c", "init.defaultBranch=main", "init", "-q", root], { encoding: "utf8" });
    assert.equal(init.status, 0, init.stderr);
    const add = spawnSync("git", ["-C", root, "add", "--", "scripts/thing.mjs"], { encoding: "utf8" });
    assert.equal(add.status, 0, add.stderr);

    const { findings } = evaluateUndeclaredImports(root);

    assert.deepEqual(findings, [{ file: "scripts/thing.mjs", line: 1, specifier: "openapi-typescript" }]);
  });

  it("counts the files it scanned, so an empty scan cannot read as coverage", () => {
    const root = fixture({
      "package.json": emptyManifest,
      "scripts/a.mjs": "export default 1;\n",
      "scripts/b.cjs": "module.exports = 2;\n",
      "scripts/c.js": "export default 3;\n",
      "docs/notes.md": "not a script\n",
    });

    const { scanned } = evaluateUndeclaredImports(root);

    assert.equal(scanned, 3);
  });

  it("exits 1 and names the offending specifier when run as a CLI", () => {
    const root = fixture({
      "package.json": emptyManifest,
      "scripts/thing.mjs": 'import openapiTS from "openapi-typescript";\n',
    });

    const result = spawnSync(process.execPath, [cli, root], { encoding: "utf8" });

    assert.equal(result.status, 1, `${result.stdout}${result.stderr}`);
    assert.match(`${result.stdout}${result.stderr}`, /openapi-typescript/);
  });

  // A bare exit 0 is not evidence: a gate that scanned nothing exits 0 too. Stating the count was
  // the first half; refusing to pass on it is the second. This fixture declares every specifier it
  // imports, so findings are empty and the floor is the only thing standing between it and green.
  it("exits 1 on the floor alone, with every specifier declared and no findings", () => {
    const root = fixture({
      "package.json": JSON.stringify({ name: "fixture", devDependencies: { "js-yaml": "4.3.0" } }),
      "scripts/thing.mjs": 'import yaml from "js-yaml";\n\nexport default yaml;\n',
    });

    assert.deepEqual(evaluateUndeclaredImports(root).findings, [], "the fixture must be clean");

    const result = spawnSync(process.execPath, [cli, root], { encoding: "utf8" });

    assert.equal(result.status, 1, `${result.stdout}${result.stderr}`);
    assert.match(`${result.stdout}${result.stderr}`, /scanned 1 files, below the floor of 90/);
  });

  // The degenerate case the floor was added for, reproduced exactly as it was found: point the
  // gate at a subtree git tracks no scripts under and it used to print
  // `undeclared imports gate passed (0 files scanned)` and exit 0.
  it("exits 1 rather than passing on a scan that covered nothing", () => {
    const root = fixture({ "package.json": emptyManifest, "docs/notes.md": "not a script\n" });

    const result = spawnSync(process.execPath, [cli, root], { encoding: "utf8" });

    assert.equal(result.status, 1, `${result.stdout}${result.stderr}`);
    assert.doesNotMatch(`${result.stdout}${result.stderr}`, /gate passed/);
    assert.match(`${result.stdout}${result.stderr}`, /scanned 0 files, below the floor of 90/);
  });

  it("exits 0 stating what it scanned, against this repository", () => {
    const result = spawnSync(process.execPath, [cli, repoRoot], { encoding: "utf8" });

    assert.equal(result.status, 0, `${result.stdout}${result.stderr}`);
    assert.match(`${result.stdout}${result.stderr}`, /gate passed \(\d+ files scanned\)/);
  });

  // The archived-evidence classification, tested from both sides. It is an exception, not an
  // omission, and the difference is that an exception is itself covered by a test whose red has
  // been observed. Added in the implementation phase after the gate found a SECOND live instance
  // of H-4 that the RED phase had not anticipated — see the header of check-undeclared-imports.mjs.
  it("classifies docs/evidence as archived rather than scanning it", () => {
    const { excluded, findings } = evaluateUndeclaredImports(repoRoot);

    assert.ok(
      excluded.includes("docs/evidence/console/wave4/L-F1/browser-window-host.mjs"),
      `the classification must name what it excludes, got ${JSON.stringify(excluded)}`,
    );
    assert.deepEqual(findings, []);
  });

  // The header of check-undeclared-imports.mjs names the condition that makes the exclusion safe:
  // "the class is safe only because nothing under docs/evidence/ is invoked by any workflow —
  // check that before adding a prefix". That was an instruction to a human. The set has already
  // grown from the single file the header names to two — verify-new-rows.mjs arrived without
  // anyone re-checking the condition — so the condition is enforced here instead of asked for.
  it("excludes only evidence artifacts that no workflow executes", () => {
    const { excluded } = evaluateUndeclaredImports(repoRoot);
    const workflowDirectory = join(repoRoot, ".github/workflows");
    const invoked = [];
    for (const workflow of readdirSync(workflowDirectory)) {
      const source = readFileSync(join(workflowDirectory, workflow), "utf8");
      for (const file of excluded) if (source.includes(file)) invoked.push(`${workflow} runs ${file}`);
    }

    assert.ok(excluded.length > 0, "the exclusion matches nothing; delete it rather than leave it unproven");
    assert.deepEqual(invoked, [], "an excluded evidence artifact that CI executes is a real hole, not an exception");
  });

  it("goes red on the archived artifact the moment the classification is removed", () => {
    const { findings } = evaluateUndeclaredImports(repoRoot, []);

    assert.deepEqual(findings, [
      { file: "docs/evidence/console/wave4/L-F1/browser-window-host.mjs", line: 30, specifier: "playwright" },
    ]);
  });

  // The gate's own subject. Red today: scripts/lib/kotlin-discriminator-unions.mjs:4 imports
  // openapi-typescript, which is in neither package.json nor package-lock.json, and its sibling
  // test dies with ERR_MODULE_NOT_FOUND — a LOAD failure with zero assertion failures, which is
  // exactly the H-4 signature.
  it("finds no undeclared import anywhere in this repository", () => {
    const { scanned, findings } = evaluateUndeclaredImports(repoRoot);

    // One floor, shared with the gate binary. Two independently chosen numbers drift, and the
    // lower of them is the one that actually holds.
    assert.ok(
      scanned >= SCANNED_FLOOR,
      `expected the repo scan to cover the tracked script surface, scanned ${scanned}`,
    );
    assert.deepEqual(findings, []);
  });
});
