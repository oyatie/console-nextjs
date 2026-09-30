#!/usr/bin/env node
// Which Rust test BINARIES actually execute in CI, and which are merely defined.
//
// WHY THIS EXISTS
//
// Five times this repository has shipped a correct-looking test that executed nowhere.
// A `rust_test` target is not wiring; a crate is not wiring; only a path from a workflow
// step to a test binary is wiring. Nothing computed that path, so nobody could answer
// "does this test run?" without tracing it by hand — and tracing by hand is what failed
// five times.
//
// A TEST BINARY IS (FILE, FEATURE SET), NEVER A FILE ALONE.
//
// Two `rust_test` targets in this repository share one crate_root and differ only in
// features: `//backend/app:console-app-unit` and `//backend/app:console-app-itest-inline-postgres`
// are both `backend/app/src/lib.rs`, and the second adds `test-postgres`, which is what
// compiles 172 `cfg(feature = "test-postgres")` sites in that crate. Keyed on the file
// alone, deleting the second wrapper from ci.yml changes no number here — the whole
// inline-PostgreSQL suite stops running with zero observable effect. The same holds for
// `backend/crates/platform/auth-rest/src/lib.rs` and `dev-auth`. So the key carries the
// feature set, and one file with two feature sets is two entries.
//
// Features are not cosmetic in the other direction either:
// `cargo test -p console-app --test dev_auth_persona_guard_feature` runs 0 tests, because
// the file is `#![cfg(feature = "dev-auth")]` and the crate has no default features. With
// `--features dev-auth` it runs 1. An invocation printed without its features is an
// invocation that runs nothing.
//
// THE CHAIN, resolved from BOTH build systems:
//
//   .github/workflows/ci.yml
//     ├─ //tools/buck:<wrapper>          → tools/buck/BUCK sh_test
//     │     └─ args = ["$(location //crate:target)"]
//     │           └─ <crate>/BUCK rust_test → (crate_root, features)
//     ├─ //backend/...:<target> (direct)  → same rust_test lookup
//     └─ cargo test -p <pkg> [--lib|--test <name>] [--features …]
//           └─ `cargo metadata` package/target src_path → (file, features)
//
// The Cargo half is resolved through `cargo metadata`, not by matching package names
// against Buck2 target names as this file used to: `[[test]]` sections and non-default
// layouts are cargo's business, and re-deriving them here would be a second resolver free
// to disagree with the first. It also means the `executed` half keeps working after the
// Buck2 graph is deleted — branches 1 and 2 simply stop matching anything.
//
// `defined` stays derived from the FILESYSTEM (every `backend/**/tests/*.rs`, plus the lib
// binary of every crate carrying `#[cfg(test)]` ANYWHERE under `src/` — not in `src/lib.rs`
// alone, which hid nine crates including `console-kernel-core`), which is what makes the
// ratchet independent of either build system. Its one input neither filesystem nor build
// system can supply durably is the FEATURE DIMENSION — see `definedBinaries`.
//
// SILENT DEGRADATION IS THE FAILURE MODE, not a wrong number. A resolver that quietly
// stops resolving reports a smaller executed set and a larger gap, which reads as
// "we found more problems" rather than "the tool broke". So every link that fails to
// resolve is reported as UNRESOLVED and exits non-zero, and named anchors below must
// resolve or the run fails regardless of counts.
//
// Usage:
//   node scripts/check-executed-tests.mjs            # report; exit 1 on unresolved, or on
//                                                    # any difference between the dark set
//                                                    # and the baseline's, in EITHER direction
//   node scripts/check-executed-tests.mjs --json
//   node scripts/check-executed-tests.mjs --map      # every rust_test → its cargo invocation
//   node scripts/check-executed-tests.mjs --gap      # still reachable only through Buck2
//   node scripts/check-executed-tests.mjs --update   # rewrite the static source-attribute baseline

import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, statSync, existsSync, writeFileSync } from "node:fs";
import { join, dirname, resolve, relative } from "node:path";
import { fileURLToPath } from "node:url";

import {
  countDeclaredTestAttributes,
  evaluateBaseline,
  evaluateTestAttributeBaseline,
} from "./lib/executed-tests-baseline.mjs";
import { directExecutable, executableWorkflowCommands } from "./lib/ci-workflow-executables.mjs";
import { unitTestedCrateSrcRoots } from "./check-executed-tests-cfg.mjs";
import { cargoTestKind } from "./lib/cargo-test-kind.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const CI = join(ROOT, ".github/workflows/ci.yml");
const WRAPPER_BUCK = join(ROOT, "tools/buck/BUCK");
const SKIP = ["node_modules", ".git", "buck-out", "target"];

/**
 * The identity of a test binary, and simultaneously the invocation suffix that runs it.
 * Sorted, so `["b","a"]` and `["a","b"]` are the same binary.
 */
const key = (file, features) =>
  features.length === 0 ? file : `${file} --features ${[...features].sort().join(",")}`;

// Each of these MUST resolve end to end. They are chosen to cover one instance of every
// link type, so a resolver that degrades on any single shape fails loudly here rather
// than returning a plausible smaller number.
const ANCHORS = [
  // wrapper → itest target → tests/*.rs
  "backend/crates/ontology/rest/tests/object_policy_attach_as_runtime_role.rs",
  // wrapper → adapter itest
  "backend/crates/attendance/adapter-postgres/tests/concurrency.rs",
  // cargo test -p … --test … → tests/*.rs, resolved through `cargo metadata`
  "backend/ci/gates/tenant-isolation/tests/owner_only_acl_postgres18.rs",
  // the feature dimension: same file as //backend/app:console-app-unit, different binary
  "backend/app/src/lib.rs --features test-postgres",
];

function walk(dir, out = [], want = (entry) => entry === "BUCK") {
  for (const entry of readdirSync(dir)) {
    if (SKIP.includes(entry)) continue;
    const p = join(dir, entry);
    if (statSync(p).isDirectory()) walk(p, out, want);
    else if (want(entry)) out.push(p);
  }
  return out;
}

/** Every rust_test in the repo: fully-qualified target → { root, features }. */
function indexRustTests() {
  const byTarget = new Map();
  for (const file of walk(join(ROOT, "backend"))) {
    const text = readFileSync(file, "utf8");
    const cellPath = relative(ROOT, file).replace(/\/BUCK$/, "");
    for (const m of text.matchAll(/rust_test\(([\s\S]*?)\n\)/g)) {
      const body = m[1];
      const name = body.match(/name\s*=\s*"([^"]+)"/)?.[1];
      const root = body.match(/crate_root\s*=\s*"([^"]+)"/)?.[1];
      if (!name || !root) continue;
      const declared = body.match(/\n\s*features\s*=\s*\[([^\]]*)\]/)?.[1] ?? "";
      const features = [...declared.matchAll(/"([^"]+)"/g)].map((x) => x[1]);
      byTarget.set(`//${cellPath}:${name}`, { root, features });
    }
  }
  return byTarget;
}

/** Wrapper name → the //backend target it locates. */
function indexWrappers() {
  const byName = new Map();
  const text = readFileSync(WRAPPER_BUCK, "utf8");
  for (const m of text.matchAll(/sh_test\(([\s\S]*?)\n\)/g)) {
    const body = m[1];
    const name = body.match(/name\s*=\s*"([^"]+)"/)?.[1];
    const target = body.match(/\$\(location (\/\/[^)]+)\)/)?.[1];
    if (name && target) byName.set(name, target);
  }
  return byName;
}

/** Every cargo test target: repo-relative src_path → { pkg, kind, name }, and by package. */
function cargoGraph() {
  const raw = execFileSync(
    "cargo",
    ["metadata", "--no-deps", "--locked", "--format-version", "1", "--manifest-path", join(ROOT, "backend/Cargo.toml")],
    { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  );
  const bySrc = new Map();
  const byPackage = new Map();
  for (const pkg of JSON.parse(raw).packages) {
    const targets = [];
    for (const target of pkg.targets) {
      if (!target.test) continue; // `test = false` in Cargo.toml means no test binary
      const kind = cargoTestKind(target.kind);
      if (!kind) continue;
      const entry = { pkg: pkg.name, kind, name: target.name, src: relative(ROOT, target.src_path) };
      bySrc.set(entry.src, entry);
      targets.push(entry);
    }
    byPackage.set(pkg.name, targets);
  }
  return { bySrc, byPackage };
}

const invocation = (entry, features) =>
  `cargo test -p ${entry.pkg} ${entry.kind === "lib" ? "--lib" : `--test ${entry.name}`}` +
  (features.length ? ` --features ${[...features].sort().join(",")}` : "");

const rustTests = indexRustTests();
const wrappers = indexWrappers();
const cargo = cargoGraph();

// Resolve executable workflow step bodies before looking for targets. Text presence is not
// reachability: `echo cargo test ...`, `exit 0`, `if: false`, and
// `continue-on-error: true` have all been real false-green shapes in this repository.
const ci = readFileSync(CI, "utf8");
const workflowCommands = executableWorkflowCommands(ci).map((command) => ({
  ...command,
  executable: command.malformed || command.controlFlow
    ? { tokens: [], malformed: true }
    : directExecutable(command.tokens),
}));

const executed = new Map();   // key(file, features) → how it is reached
const unresolved = [];

// 1. //tools/buck:<wrapper>, but only as an argument to a command that can actually invoke
// the wrapper or Buck2. An echo/comment/disabled step is not a path to a test binary.
const buckInvocations = workflowCommands.filter(({ executable }) => {
  const [program, verb] = executable.tokens;
  return program === "tools/buck/test_needs_postgres.sh"
    || program === "tools/buck2" && verb === "test";
});
for (const wrapperArg of buckInvocations.flatMap(({ executable }) => executable.tokens)
  .filter((token) => /^\/\/tools\/buck:[a-z0-9_-]+$/.test(token))) {
  const wrapper = wrapperArg.slice("//tools/buck:".length);
  const target = wrappers.get(wrapper);
  if (!target) { unresolved.push(`ci.yml names //tools/buck:${wrapper}, absent from tools/buck/BUCK`); continue; }
  const test = rustTests.get(target);
  if (!test) { unresolved.push(`wrapper ${wrapper} locates ${target}, which is not a rust_test`); continue; }
  executed.set(key(test.root, test.features), `//tools/buck:${wrapper}`);
}

// 1b. tools/ci/cargo_needs_postgres.sh --workflow-only expands tools/ci/postgres-cargo-map.json.
// The map is the execution source of truth after the Buck cold-cache exit for PR PG CI.
{
  const cargoPg = workflowCommands.filter(({ executable }) => {
    const [program] = executable.tokens;
    return program === "tools/ci/cargo_needs_postgres.sh"
      || program?.endsWith("/cargo_needs_postgres.sh");
  });
  if (cargoPg.length) {
    const mapPath = join(ROOT, "tools/ci/postgres-cargo-map.json");
    if (!existsSync(mapPath)) {
      unresolved.push("ci.yml invokes cargo_needs_postgres.sh but tools/ci/postgres-cargo-map.json is missing");
    } else {
      const map = JSON.parse(readFileSync(mapPath, "utf8"));
      const workflowOnly = cargoPg.some(({ executable }) => executable.tokens.includes("--workflow-only"));
      const entries = (map.entries ?? []).filter((e) => (workflowOnly ? e.in_workflow_postgres_job : true));
      for (const entry of entries) {
        const argv = entry.cargo_argv ?? [];
        // Reuse cargo attribution: synthetic tokens as if the workflow ran cargo test directly.
        const lineTokens = argv;
        if (lineTokens[0] !== "cargo" || lineTokens[1] !== "test") {
          unresolved.push(`postgres-cargo-map entry ${entry.name} has non-cargo argv`);
          continue;
        }
        const optionValues = (short, long) => {
          const values = [];
          for (let index = 2; index < lineTokens.length; index += 1) {
            const token = lineTokens[index];
            if (token === short || token === long) {
              if (lineTokens[index + 1]) values.push(lineTokens[index + 1]);
              index += 1;
            }
          }
          return values;
        };
        const pkgs = optionValues("-p", "--package");
        const tests = optionValues("--test", "--test");
        const lib = lineTokens.includes("--lib");
        const features = optionValues("--features", "--features").flatMap((v) => v.split(","));
        for (const pkg of pkgs) {
          const targets = cargo.byPackage.get(pkg);
          if (!targets) {
            unresolved.push(`postgres-cargo-map entry ${entry.name} package ${pkg} unknown to cargo metadata`);
            continue;
          }
          for (const target of targets) {
            const selected = tests.length === 0 && !lib
              ? true
              : (lib && target.kind === "lib") || (target.kind === "test" && tests.includes(target.name));
            if (selected) executed.set(key(target.src, features), `cargo-map:${entry.name}`);
          }
        }
      }
    }
  }
}

// 2. direct //backend/...:target in a workflow step.
// FAILS CLOSED. This branch used to be `if (root) executed.set(...)` with no else while its
// sibling above pushed to `unresolved`, so renaming a target in ci.yml orphaned its test
// file and reported nothing at all. All eight `//backend/...` references in ci.yml today are
// rust_test targets; a future step naming a rust_binary here would have to be classified
// rather than ignored, which is the intended cost.
for (const target of buckInvocations.flatMap(({ executable }) => executable.tokens)
  .filter((token) => /^\/\/backend\/[A-Za-z0-9_\/-]+:[A-Za-z0-9_-]+$/.test(token))) {
  const test = rustTests.get(target);
  if (!test) { unresolved.push(`ci.yml names ${target}, which is not a rust_test in any BUCK file`); continue; }
  executed.set(key(test.root, test.features), target);
}

// 3. cargo test -p <pkg> [--lib | --test <name>] [--features …], resolved through
// `cargo metadata`. Runs last on purpose: where both build systems reach the same binary,
// the cargo attribution wins, which is what makes `--gap` mean "still Buck2-only".
for (const { executable } of workflowCommands) {
  const lineTokens = executable.tokens;
  if (lineTokens[0] !== "cargo" || lineTokens[1] !== "test") continue;
  const line = lineTokens.join(" ");
  // --doc runs doctests, which live in `src/**` and are not in the `defined` population.
  // Attributing a package's whole test set to a --doc line would report every one of its
  // files as executed.
  if (/\s--doc(\s|$)/.test(line)) continue;
  if (/--all-features|--no-default-features/.test(line)) {
    unresolved.push(`ci.yml runs "${line.trim()}"; this resolver models explicit --features only`);
    continue;
  }
  const optionValues = (short, long) => {
    const values = [];
    for (let index = 2; index < lineTokens.length; index += 1) {
      const token = lineTokens[index];
      if (token === short || token === long) {
        if (lineTokens[index + 1]) values.push(lineTokens[index + 1]);
        index += 1;
      } else if (token.startsWith(`${short}=`)) {
        values.push(token.slice(short.length + 1));
      } else if (token.startsWith(`${long}=`)) {
        values.push(token.slice(long.length + 1));
      }
    }
    return values;
  };
  const pkgs = optionValues("-p", "--package");
  if (pkgs.length === 0) {
    unresolved.push(`ci.yml runs "${line.trim()}" with no -p; this resolver cannot say what it executes`);
    continue;
  }
  const tests = optionValues("--test", "--test");
  const lib = lineTokens.includes("--lib");
  const features = optionValues("--features", "--features").flatMap((value) => value.split(","));
  for (const pkg of pkgs) {
    const targets = cargo.byPackage.get(pkg);
    if (!targets) { unresolved.push(`ci.yml runs cargo test -p ${pkg}, which cargo metadata does not know`); continue; }
    for (const target of targets) {
      // No --lib and no --test: cargo runs the package's whole test set.
      const selected = tests.length === 0 && !lib
        ? true
        : (lib && target.kind === "lib") || (target.kind === "test" && tests.includes(target.name));
      if (selected) executed.set(key(target.src, features), `cargo test -p ${pkg}`);
    }
  }
}

/**
 * The population that must be reachable. FILE dimension: the filesystem, so it survives
 * both build systems. FEATURE dimension: declarations, because nothing in the filesystem
 * says which feature sets a file's tests are meant to compile under —
 * `backend/crates/platform/auth-rest/tests/group_admin_tenant_context.rs` contains no `cfg`
 * at all and still requires `dev-auth`. Three declaration sources, in decreasing durability:
 *
 *   - `defined_feature_variants` in the baseline, a committed list. It is here because of
 *     the hole the other two cannot close: BOTH are artifacts a deletion PR deletes. A
 *     commit that removes `//tools/buck:auth-rest-dev-auth-inline-postgres` from ci.yml
 *     together with its `sh_test` and its `rust_test` — the exact shape of a wrapper-removal
 *     PR — dropped `(auth-rest/src/lib.rs, dev-auth)` from `defined` and from `executed` in
 *     the same instant, so `dark` did not move and this gate exited 0 while an entire
 *     dev-auth PostgreSQL suite stopped running. `defined` must not be derived from the
 *     thing being deleted. A pinned entry survives, goes dark, and names itself.
 *   - a file-level `#![cfg(feature = "X")]`, which is the file saying that WITHOUT X it is
 *     empty. This one is filesystem-derived and outlives Buck2; it also replaces the bare
 *     entry rather than adding to it, since running such a file with no features runs
 *     nothing at all.
 *   - the `features = [...]` of any rust_test whose crate_root is this file. Adds new
 *     variants for free; cannot defend an existing one, per the first bullet.
 *
 * A file no declaration mentions gets the bare entry, which is every ordinary test file.
 *
 * ponytail: the pin is a hand-kept list of two, not a generator, because repo-wide there
 * are exactly two crate roots carrying a second feature-bearing binary. A pinned entry that
 * stops being `defined` is UNRESOLVED below, so the list cannot rot into decoration.
 *
 * ponytail: a tests/*.rs whose ITEMS (not the file) are `#[cfg(feature = "X")]` still takes
 * its feature dimension from `rust_test features` alone, and will lose it at the Buck2 exit.
 * Unreachable today. The fix when it lands is another `defined_feature_variants` entry, which
 * is why that list is keyed on the binary and not on lib.rs.
 */
function definedBinaries() {
  const declared = new Map();
  const add = (root, variant) => {
    if (!declared.has(root)) declared.set(root, new Set());
    declared.get(root).add(variant);
  };
  for (const { root, features } of rustTests.values()) add(root, key(root, features));
  for (const variant of pinnedVariants) add(variant.split(" --features ")[0], variant);

  const files = walk(join(ROOT, "backend"), [], (entry) => entry.endsWith(".rs"))
    .map((path) => [relative(ROOT, path), readFileSync(path, "utf8")]);

  // A crate has a lib test binary if a *live* `#[cfg(test)]` appears ANYWHERE under its
  // `src/`, not in `src/lib.rs` alone. Nine crates keep every one of theirs in
  // `src/<module>.rs`, and `console-kernel-core` — 152 dependents, and the subject of the
  // PR that landed four commits before this one for having run its tests nowhere — is one
  // of them, with eleven such files and zero `#[cfg(test)]` in its own lib.rs. Keyed on
  // lib.rs alone those nine binaries were not in `defined`, could never be `dark`, and
  // deleting `-p console-kernel-core` from ci.yml moved no number here at all.
  //
  // Live means outside comments and string literals. A doc comment that pastes the
  // attribute (process.doc-comment-cfg-test-false-dark) must not invent a dark lib binary;
  // a raw substring scan of the file text did exactly that. Examined-zero fails closed
  // inside unitTestedCrateSrcRoots.
  let unitTested;
  try {
    unitTested = unitTestedCrateSrcRoots(files);
  } catch (error) {
    unresolved.push(error instanceof Error ? error.message : String(error));
    unitTested = new Set();
  }
  const crateSrc = (rel) => rel.slice(0, rel.indexOf("/src/") + 4);

  const out = new Set();
  for (const [rel, text] of files) {
    const isTest = /\/tests\/[^/]+\.rs$/.test(rel);
    const isLib = /\/src\/lib\.rs$/.test(rel);
    if (!isTest && !isLib) continue;
    if (isLib && !unitTested.has(crateSrc(rel))) continue;
    const variants = new Set(declared.get(rel) ?? []);
    const fileLevel = text.match(/^#!\[cfg\(feature = "([^"]+)"\)\]/m)?.[1];
    if (fileLevel) { variants.delete(rel); variants.add(key(rel, [fileLevel])); }
    if (variants.size === 0) variants.add(rel);
    for (const variant of variants) out.add(variant);
  }
  return [...out].sort();
}

const baselineRel = "docs/program/executed-tests-baseline.json";
const baselinePath = join(ROOT, baselineRel);
const baseline = existsSync(baselinePath) ? JSON.parse(readFileSync(baselinePath, "utf8")) : {};
const pinnedVariants = baseline.defined_feature_variants ?? [];

const defined = definedBinaries();
const executedBinaries = [...executed.keys()].sort();
const dark = defined.filter((f) => !executed.has(f));
const buckOnly = [...executed].filter(([, via]) => via.startsWith("//")).map(([k]) => k).sort();

// Count declared test attributes once per reachable SOURCE. This is a cheap lexical
// deletion ratchet, not runtime case evidence: it deliberately does not evaluate cfg,
// feature selection, macro expansion, or `#[ignore]`. Feature-bearing BINARY reachability
// is protected separately by the exact (file, feature set) dark-set ratchet above. A lib
// source represents its whole src/ tree because sibling modules hold tests.
function countAttributes(rel) {
  const abs = join(ROOT, rel);
  if (!existsSync(abs)) return 0;
  const files = rel.endsWith("/src/lib.rs")
    ? walk(dirname(abs), [], (entry) => entry.endsWith(".rs"))
    : [abs];
  return files.reduce(
    (count, file) => count + countDeclaredTestAttributes(readFileSync(file, "utf8")),
    0,
  );
}

const reachableSources = [...new Set(executedBinaries.map((binary) => binary.split(" --features ")[0]))].sort();
const testAttributes = Object.fromEntries(
  reachableSources.map((source) => [source, countAttributes(source)]),
);
const totalTestAttributes = Object.values(testAttributes).reduce((sum, count) => sum + count, 0);

for (const a of ANCHORS.filter((a) => !executed.has(a))) {
  unresolved.push(`ANCHOR ${a} no longer resolves — the resolver has silently degraded`);
}
// A pin that names a binary the population does not contain defends nothing, and would sit
// in the baseline reading as though it did.
for (const v of pinnedVariants.filter((v) => !defined.includes(v))) {
  unresolved.push(`defined_feature_variants pins ${v}, which is not in the defined population — the pin is decoration`);
}

if (process.argv.includes("--json")) {
  console.log(JSON.stringify({
    defined: defined.length,
    executed: executedBinaries.length,
    dark,
    buckOnly,
    testAttributes,
    unresolved,
  }, null, 2));
} else if (process.argv.includes("--map")) {
  // Every rust_test and the cargo invocation that replaces it, features included: an
  // invocation printed without them is an invocation that runs zero tests.
  for (const [target, { root, features }] of [...rustTests].sort((a, b) => a[0].localeCompare(b[0]))) {
    const entry = cargo.bySrc.get(root);
    console.log(`${target}\n  file  ${root}\n  cargo ${entry ? invocation(entry, features) : "NONE — no Cargo target owns this file"}`);
  }
} else if (process.argv.includes("--gap")) {
  for (const f of buckOnly) console.log(f);
} else {
  console.log(`test binaries defined         : ${defined.length}`);
  console.log(`reachable from a CI step      : ${executedBinaries.length}`);
  console.log(`declared test attrs in reachable sources: ${totalTestAttributes}  (static; cfg/ignore-unaware)`);
  console.log(`executed nowhere              : ${dark.length}`);
  console.log(`reachable only through buck2  : ${buckOnly.length}  (--gap to list; the Buck2 exit has to wire these)`);
  for (const f of dark.slice(0, 25)) console.log(`  dark  ${f}`);
  if (dark.length > 25) console.log(`  … and ${dark.length - 25} more`);
  for (const u of unresolved) console.log(`  UNRESOLVED  ${u}`);
}

// THE RATCHET. The dark SET is pinned by name, in both directions.
//
// This states plainly what it implies, because a ratchet whose implication is unstated is
// unimplementable: FROM NOW ON, A NEW TEST FILE MUST BE WIRED INTO CI IN THE SAME PULL
// REQUEST THAT ADDS IT. Adding backend/**/tests/foo.rs without a workflow path to it fails
// here. That is the intended cost — this repository has shipped five tests that executed
// nowhere, and every one of them was added without wiring.
//
// The baseline is a committed SET of named files, not a first-run measurement, so a
// regression in the resolver cannot quietly raise the bar to whatever it currently reports.
//
// It is a set and not a count because a count is blind to substitution: with `10` as the
// bar, wiring one dark test and letting a different one go dark keeps the gate green while
// the repository silently swaps which test cannot fail. Naming them also lets independent
// lanes append to their own bucket instead of contending on one shared integer.
// Unresolved links are a tool failure and must never read as a finding. This is checked
// BEFORE the baseline, because a degraded resolver reports a larger dark set, and the
// baseline's failure message would then instruct someone to wire tests that are already
// wired — a confident, wrong instruction that hides a broken tool.
if (unresolved.length > 0) {
  console.error(`\n${unresolved.length} unresolved link(s): the resolver could not follow the chain. Fix the resolver before trusting any count above.`);
  process.exit(1);
}

// A missing baseline must fail, not silently disable the ratchet: deleting one file is a
// strictly easier bypass than any the comparison itself guards against.
if (!existsSync(baselinePath)) {
  console.error(`\n${baselineRel} is missing. It is the ratchet's contract; without it nothing constrains which tests may execute nowhere.`);
  process.exit(1);
}
const { fatal, advisory } = evaluateBaseline(
  dark,
  baseline,
  baselineRel,
);
if (fatal) {
  console.error(`\n${fatal}`);
  process.exit(1);
}
// stderr, not stdout: under --json this line follows the JSON document, and a consumer
// doing JSON.parse on stdout gets "Unexpected non-whitespace character after JSON".
// The tool that measures whether tests run must itself be parseable.
if (advisory) console.error(`\n${advisory}`);

if (process.argv.includes("--update")) {
  // Deliberate, human-committed, and reviewable as a diff. Never run in CI: it would let a
  // degraded resolver relabel its own regression as the new normal. Resolution and the
  // named dark-set contract have already passed above before this write is reachable.
  const updatedBaseline = { ...baseline, test_attribute_baseline: testAttributes };
  delete updatedBaseline.case_baseline;
  writeFileSync(baselinePath, `${JSON.stringify(updatedBaseline, null, 2)}\n`);
  console.error(`\ntest_attribute_baseline rewritten: ${reachableSources.length} reachable sources, ${totalTestAttributes} declared test attributes. This static metric does not evaluate cfg or ignore. Commit it with the change that moved the numbers.`);
  process.exit(0);
}

// THE STATIC ATTRIBUTE RATCHET. Per source, because a total hides a loss behind an
// unrelated gain. This sees lexical deletion from a still-reachable source. It does not
// prove that a test is compiled, selected, unignored, or run; the binary ratchet above is
// the executable-reachability evidence.
const attributeResult = evaluateTestAttributeBaseline(
  testAttributes,
  baseline.test_attribute_baseline,
  baselineRel,
);
if (attributeResult.fatal) {
  console.error(`\n${attributeResult.fatal}`);
  process.exit(1);
}
