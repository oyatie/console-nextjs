import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  rmSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import yaml from "js-yaml";

import {
  gitFixtureEnvironment,
  installGitFixtureEnvironment,
} from "./lib/git-fixture-environment.mjs";

import {
  classifyChangedPaths,
  evaluateCiPreflight,
  emitPathClassGithubOutput,
  listChangedPathsForPathClass,
  parseNulDelimitedChangedPaths,
  resolvePathClassFromEnv,
} from "./check-ci-preflight.mjs";

installGitFixtureEnvironment();

const workflow = readFileSync(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8");
const postgresWrapperBuildFile = readFileSync(new URL("../tools/buck/BUCK", import.meta.url), "utf8");
const freeRunnerDiskAction = readFileSync(
  new URL("../.github/actions/free-runner-disk/action.yml", import.meta.url),
  "utf8",
);
const executedTestsBaseline = JSON.parse(readFileSync(
  new URL("../docs/program/executed-tests-baseline.json", import.meta.url),
  "utf8",
));
const cargoLockGate = "cargo metadata --manifest-path backend/Cargo.toml --locked --format-version=1 >/dev/null";
const ciPreflightTests = "node --test scripts/check-ci-preflight.test.mjs";
const reasoningLensManifestStep = `      - name: Reasoning lens manifest drift
        id: reasoning-lens-manifest
        if: \${{ !cancelled() && steps.npm-ci.outcome == 'success' }}
        run: node scripts/check-reasoning-lens-manifest.mjs

`;
const reachabilityPreflightCommands = [
  "node --test scripts/console/route-inventory.test.mjs",
  "tools/buck/run_test_with_postgres_env.test.sh",
  "tools/buck/test_needs_postgres.test.sh",
];
const preflightRustToolchainSetup = `      - name: Install Rust toolchain for Cargo.lock consistency
        id: rust-toolchain
        if: \${{ !cancelled() && steps.checkout.outcome == 'success' && steps.path_class.outputs.run_heavy == 'true' }}
        uses: dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8 # stable
        with:
          toolchain: "1.98.1"

`;
const runHeavyIf = "${{ needs.preflight.outputs.run_heavy == 'true' }}";
const preflightRunHeavyIf = "${{ steps.path_class.outputs.run_heavy == 'true' }}";
const runHeavyUnlessCancelledIf =
  "${{ !cancelled() && needs.preflight.outputs.run_heavy == 'true' }}";
const preflightCheckoutHeavyIf =
  "${{ !cancelled() && steps.checkout.outcome == 'success' && steps.path_class.outputs.run_heavy == 'true' }}";
const preflightBuckHeavyIf =
  "${{ !cancelled() && steps.dotslash.outcome == 'success' && steps.path_class.outputs.run_heavy == 'true' }}";
const preflightRustHeavyIf =
  "${{ !cancelled() && steps.rust-toolchain.outcome == 'success' && steps.path_class.outputs.run_heavy == 'true' }}";
const backendIndependentIf =
  "${{ !cancelled() && needs.preflight.outputs.run_heavy == 'true' }}";
// `backend` is a three-leg matrix. Steps that belong to ONE leg carry the leg in
// their guard; `backendIndependentIf` above is now only right for steps that run
// on EVERY leg (setup, topology, collect). A mutation that replaces a leg-gated
// step's `if:` must match the leg-gated text or it silently replaces nothing --
// which is how two of these tests went quiet when the matrix landed.
const backendCargoLegIf =
  "${{ !cancelled() && matrix.leg == 'cargo' && needs.preflight.outputs.run_heavy == 'true' }}";
const backendBuckAppLegIf =
  "${{ !cancelled() && matrix.leg == 'buck-app' && needs.preflight.outputs.run_heavy == 'true' }}";
const npmCiIf = "${{ !cancelled() && steps.npm-ci.outcome == 'success' }}";
const npmCiPrIf = "${{ !cancelled() && steps.derive.outcome == 'success' && steps.npm-ci.outcome == 'success' && github.event_name == 'pull_request' }}";

function expectFailure(
  source,
  message,
  buckBuildFile = postgresWrapperBuildFile,
  actionFile = freeRunnerDiskAction,
) {
  const { failures } = evaluateCiPreflight(source, buckBuildFile, actionFile);
  assert.ok(failures.some((failure) => failure.includes(message)), failures.join("\n"));
}

function replaceJob(source, job, mutate) {
  const start = source.indexOf(`  ${job}:\n`);
  assert.notEqual(start, -1, `missing workflow job ${job}`);
  const next = source.slice(start + 1).search(/^  [A-Za-z0-9_-]+:/m);
  const end = next < 0 ? source.length : start + 1 + next;
  return source.slice(0, start) + mutate(source.slice(start, end)) + source.slice(end);
}

function mutateNamedStep(source, job, name, mutate) {
  return replaceJob(source, job, (block) => {
    const anchor = `      - name: ${name}\n`;
    const start = block.indexOf(anchor);
    assert.notEqual(start, -1, `missing ${job} step ${name}`);
    const next = block.indexOf("      - ", start + anchor.length);
    const end = next < 0 ? block.length : next;
    const step = block.slice(start, end);
    const mutated = mutate(step);
    assert.notEqual(mutated, step, `mutation did not change ${job} step ${name}`);
    return block.slice(0, start) + mutated + block.slice(end);
  });
}

function addFalseCondition(step) {
  if (/^        if: /m.test(step)) {
    return step.replace(/^        if: .*$/m, "        if: false");
  }
  return step.replace(/^      - name: .*$/m, (name) => `${name}\n        if: false`);
}

function addContinueOnError(step) {
  assert.ok(!/^        continue-on-error:/m.test(step), "baseline step already continues on error");
  return step.replace(
    /^      - name: .*$/m,
    (name) => `${name}\n        continue-on-error: true`,
  );
}

function addRetainedTextEarlyExit(step) {
  if (/^        run: \|$/m.test(step)) {
    return step.replace(/^        run: \|$/m, "        run: |\n          exit 0");
  }
  return step.replace(
    /^        run: (.+)$/m,
    (_, command) => `        run: |\n          exit 0\n          ${command}`,
  );
}

function mutateActionReference(step) {
  return step.replace(
    /^(        uses: )(\S+)(.*)$/m,
    (_, prefix, action, suffix) => `${prefix}${action}-mutated${suffix}`,
  );
}

function addUnexpectedActionInput(step) {
  if (/^        with:$/m.test(step)) {
    return step.replace(/^        with:$/m, "        with:\n          proof-lock-extra: forbidden");
  }
  return step.replace(
    /^(        uses: .*?)$/m,
    "$1\n        with:\n          proof-lock-extra: forbidden",
  );
}

function mutateActionInput(step, input, replacement) {
  const escaped = input.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const pattern = new RegExp(`^          ${escaped}: .*\\n`, "m");
  assert.match(step, pattern, `missing action input ${input}`);
  return step.replace(pattern, replacement === null
    ? ""
    : `          ${input}: ${replacement}\n`);
}

function duplicateNamedStep(step, name) {
  return step + step.replace(
    `      - name: ${name}\n`,
    `      - name: ${name} duplicate\n`,
  );
}

function swapNamedSteps(source, job, firstName, secondName) {
  return replaceJob(source, job, (block) => {
    const starts = [...block.matchAll(/^      - name: /gm)].map((match) => match.index);
    const ranges = starts.map((start, index) => ({
      start,
      end: starts[index + 1] ?? block.length,
      text: block.slice(start, starts[index + 1] ?? block.length),
    }));
    const first = ranges.find((range) => range.text.startsWith(`      - name: ${firstName}\n`));
    const second = ranges.find((range) => range.text.startsWith(`      - name: ${secondName}\n`));
    assert.ok(first && second && first.start < second.start, `cannot swap ${job} actions`);
    return block.slice(0, first.start)
      + second.text
      + block.slice(first.end, second.start)
      + first.text
      + block.slice(second.end);
  });
}

describe("CI preflight contract", () => {
  it("ignores an inherited alternate Git index", () => {
    const repositoryRoot = fileURLToPath(new URL("../", import.meta.url));
    const temporaryRoot = mkdtempSync(join(tmpdir(), "ci-preflight-alternate-index-"));
    const alternateIndex = join(temporaryRoot, "index");
    const cleanGitEnvironment = gitFixtureEnvironment();
    const runGit = (args, env = cleanGitEnvironment) => {
      const result = spawnSync("git", ["-C", repositoryRoot, ...args], {
        encoding: "utf8",
        env,
        stdio: ["ignore", "pipe", "pipe"],
      });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout.trim();
    };
    const source = "backend/crates/attendance/application/tests/attendance_policy.rs";
    try {
      const preflightSource = readFileSync(
        new URL("./check-ci-preflight.mjs", import.meta.url),
        "utf8",
      );
      assert.equal(
        [...preflightSource.matchAll(
          /spawnSync\(\s*"git",\s*\[\s*"--no-replace-objects",\s*"-C",\s*rootDir\(\),/g,
        )].length,
        2,
        "both index-tree reads must disable replacement objects explicitly",
      );
      assert.match(
        preflightSource,
        /const gitEnvironment = gitFixtureEnvironment\(\);/,
        "the index-tree reads must use the shared sealed Git environment",
      );
      assert.match(
        readFileSync(new URL("./lib/git-fixture-environment.mjs", import.meta.url), "utf8"),
        /GIT_NO_REPLACE_OBJECTS: "1"/,
        "the shared sealed Git environment must disable replacement objects",
      );

      copyFileSync(
        runGit(["rev-parse", "--path-format=absolute", "--git-path", "index"]),
        alternateIndex,
      );
      const alternateEnvironment = { ...cleanGitEnvironment, GIT_INDEX_FILE: alternateIndex };
      runGit(
        ["update-index", "--force-remove", "--", source],
        alternateEnvironment,
      );
      assert.equal(runGit(["ls-files", "--", source], alternateEnvironment), "");

      const childProbe = `
        import { readFileSync } from "node:fs";
        const { evaluateCiPreflight } = await import(${JSON.stringify(
          new URL("./check-ci-preflight.mjs", import.meta.url).href,
        )});
        const failures = evaluateCiPreflight(
          readFileSync(${JSON.stringify(
            fileURLToPath(new URL("../.github/workflows/ci.yml", import.meta.url)),
          )}, "utf8"),
          readFileSync(${JSON.stringify(
            fileURLToPath(new URL("../tools/buck/BUCK", import.meta.url)),
          )}, "utf8"),
          readFileSync(${JSON.stringify(
            fileURLToPath(new URL("../.github/actions/free-runner-disk/action.yml", import.meta.url)),
          )}, "utf8"),
          JSON.parse(readFileSync(${JSON.stringify(
            fileURLToPath(new URL("../docs/program/executed-tests-baseline.json", import.meta.url)),
          )}, "utf8")),
        ).failures;
        if (failures.length > 0) {
          process.stderr.write(failures.join("\\n"));
          process.exitCode = 1;
        }
      `;
      const child = spawnSync(
        process.execPath,
        ["--input-type=module", "--eval", childProbe],
        {
          cwd: repositoryRoot,
          encoding: "utf8",
          env: {
            ...cleanGitEnvironment,
            GIT_INDEX_FILE: alternateIndex,
          },
          stdio: ["ignore", "pipe", "pipe"],
        },
      );
      assert.equal(child.status, 0, `${child.stdout}\n${child.stderr}`);
    } finally {
      rmSync(temporaryRoot, { recursive: true, force: true });
    }
  });

  it("isolates fixture Git from hostile external repository controls", () => {
    const repositoryRoot = fileURLToPath(new URL("../", import.meta.url));
    const temporaryRoot = mkdtempSync(join(tmpdir(), "ci-preflight-git-sentinel-"));
    const sentinel = join(temporaryRoot, "sentinel");
    const foreignWorktree = join(temporaryRoot, "foreign-worktree");
    const hostileGlobalConfig = join(temporaryRoot, "hostile-global-config");
    const hostileSystemConfig = join(temporaryRoot, "hostile-system-config");
    const hostileTrace = join(temporaryRoot, "hostile-trace.json");
    const safeEnvironment = gitFixtureEnvironment();
    const runGit = (args) => {
      const result = spawnSync("git", args, {
        encoding: "utf8",
        env: safeEnvironment,
        stdio: ["ignore", "pipe", "pipe"],
      });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout;
    };
    const worktreeSnapshot = (root) => {
      const entries = [];
      const visit = (relative = "") => {
        const absolute = join(root, relative);
        for (const name of readdirSync(absolute).sort()) {
          if (relative === "" && name === ".git") continue;
          const childRelative = relative === "" ? name : `${relative}/${name}`;
          const child = join(root, childRelative);
          const metadata = lstatSync(child);
          const mode = (metadata.mode & 0o777).toString(8);
          if (metadata.isSymbolicLink()) {
            entries.push(["symlink", childRelative, mode, readlinkSync(child)]);
          } else if (metadata.isDirectory()) {
            entries.push(["directory", childRelative, mode]);
            visit(childRelative);
          } else {
            entries.push([
              "file",
              childRelative,
              mode,
              createHash("sha256").update(readFileSync(child)).digest("hex"),
            ]);
          }
        }
      };
      visit();
      return entries;
    };
    const snapshot = () => ({
      config: readFileSync(join(sentinel, ".git/config")),
      globalConfig: readFileSync(hostileGlobalConfig),
      systemConfig: readFileSync(hostileSystemConfig),
      head: readFileSync(join(sentinel, ".git/HEAD")),
      resolvedHead: runGit(["-C", sentinel, "rev-parse", "HEAD"]),
      index: readFileSync(join(sentinel, ".git/index")),
      status: runGit(["-C", sentinel, "status", "--porcelain=v1", "--untracked-files=all"]),
      worktree: worktreeSnapshot(sentinel),
      foreignWorktree: worktreeSnapshot(foreignWorktree),
      worktreeList: runGit(["-C", sentinel, "worktree", "list", "--porcelain"]),
    });

    try {
      mkdirSync(sentinel);
      mkdirSync(foreignWorktree);
      mkdirSync(join(temporaryRoot, "hostile-hooks"));
      writeFileSync(hostileGlobalConfig, "[core]\n\thooksPath = /hostile/global/hooks\n");
      writeFileSync(hostileSystemConfig, "[core]\n\thooksPath = /hostile/system/hooks\n");
      runGit(["init", "--initial-branch=main", "--quiet", sentinel]);
      runGit(["-C", sentinel, "config", "user.name", "Issue 881 Sentinel"]);
      runGit(["-C", sentinel, "config", "user.email", "sentinel@example.invalid"]);
      writeFileSync(join(sentinel, "sentinel.txt"), "external sentinel bytes\n");
      runGit(["-C", sentinel, "add", "--", "sentinel.txt"]);
      runGit(["-C", sentinel, "commit", "--quiet", "-m", "external sentinel"]);
      const before = snapshot();

      const deliberate = gitFixtureEnvironment(
        {
          HOME: process.env.HOME,
          XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
          GIT_DIR: "/hostile/repository",
        },
        {
          GIT_ASKPASS: "/fixture/askpass",
          GIT_AUTHOR_NAME: "Fixture Author",
        },
      );
      assert.equal(deliberate.GIT_DIR, undefined);
      assert.equal(deliberate.GIT_ASKPASS, "/fixture/askpass");
      assert.equal(deliberate.GIT_AUTHOR_NAME, "Fixture Author");
      assert.equal(deliberate.HOME, process.env.HOME);
      assert.equal(deliberate.XDG_CONFIG_HOME, process.env.XDG_CONFIG_HOME);

      const child = spawnSync(
        process.execPath,
        ["--test", "scripts/check-prototype-chain-lookups.test.mjs"],
        {
          cwd: repositoryRoot,
          encoding: "utf8",
          env: {
            ...process.env,
            GIT_ALTERNATE_OBJECT_DIRECTORIES: join(sentinel, ".git/objects"),
            GIT_ASKPASS: join(temporaryRoot, "hostile-askpass"),
            GIT_CEILING_DIRECTORIES: temporaryRoot,
            GIT_COMMON_DIR: join(sentinel, ".git"),
            GIT_CONFIG_COUNT: "2",
            GIT_CONFIG_GLOBAL: hostileGlobalConfig,
            GIT_CONFIG_KEY_0: "core.hooksPath",
            GIT_CONFIG_KEY_1: "commit.gpgsign",
            GIT_CONFIG_NOSYSTEM: "0",
            GIT_CONFIG_SYSTEM: hostileSystemConfig,
            GIT_CONFIG_VALUE_0: join(temporaryRoot, "hostile-hooks"),
            GIT_CONFIG_VALUE_1: "true",
            GIT_DIR: join(sentinel, ".git"),
            GIT_DISCOVERY_ACROSS_FILESYSTEM: "1",
            GIT_INDEX_FILE: join(sentinel, ".git/index"),
            GIT_NAMESPACE: "hostile-namespace",
            GIT_OBJECT_DIRECTORY: join(sentinel, ".git/objects"),
            GIT_SSH_COMMAND: "/bin/false",
            GIT_TRACE2_EVENT: hostileTrace,
            GIT_WORK_TREE: foreignWorktree,
          },
          stdio: ["ignore", "pipe", "pipe"],
        },
      );
      assert.equal(child.status, 0, `${child.stdout}\n${child.stderr}`);
      assert.deepEqual(snapshot(), before);
      assert.equal(lstatSync(hostileTrace, { throwIfNoEntry: false }), undefined);
    } finally {
      rmSync(temporaryRoot, { recursive: true, force: true });
    }
  });

  it("classifies docs-only narrowly and keeps unmapped or product paths heavy", () => {
    assert.deepEqual(classifyChangedPaths(["docs/program/foo.md"]), {
      pathClass: "docs-only",
      docsOnly: true,
      runHeavy: false,
      runLivePostgres: false,
      reason: "docs-allowlist",
    });
    assert.equal(classifyChangedPaths(["README.md"]).docsOnly, true);
    assert.equal(classifyChangedPaths(["docs/a.md", "backend/app/src/lib.rs"]).pathClass, "mixed");
    assert.equal(classifyChangedPaths([".github/workflows/ci.yml"]).pathClass, "unknown");
    assert.equal(classifyChangedPaths(["docs/a.md", "../etc/passwd"]).pathClass, "unknown");
    assert.equal(classifyChangedPaths(["./README.md"]).pathClass, "unknown");
    assert.equal(classifyChangedPaths([]).runHeavy, true);
    assert.equal(
      resolvePathClassFromEnv({ PATH_CLASS_EVENT_NAME: "workflow_dispatch" }).runHeavy,
      true,
    );
    assert.equal(
      resolvePathClassFromEnv({ PATH_CLASS_EVENT_NAME: "workflow_dispatch" }).runLivePostgres,
      true,
    );
  });

  it("path-gates live Postgres like oyatie pg-gate", () => {
    assert.equal(classifyChangedPaths(["backend/app/src/lib.rs"]).runLivePostgres, false);
    assert.equal(classifyChangedPaths(["backend/app/src/lib.rs"]).runHeavy, true);
    assert.equal(
      classifyChangedPaths([
        "backend/crates/ontology/canonical-adapter-postgres/src/org_unit.rs",
      ]).runLivePostgres,
      true,
    );
    assert.equal(
      classifyChangedPaths(["backend/crates/platform/db/migrations/0215_org_units.sql"]).runLivePostgres,
      true,
    );
    assert.equal(classifyChangedPaths(["tools/ci/cargo_needs_postgres.sh"]).runLivePostgres, true);
    assert.equal(classifyChangedPaths([".github/workflows/ci.yml"]).runLivePostgres, true);
    assert.equal(classifyChangedPaths(["docs/program/foo.md"]).runLivePostgres, false);
    assert.equal(classifyChangedPaths(["ops/postgres-reconcile-topology.sh"]).runLivePostgres, true);
    assert.equal(classifyChangedPaths(["ops/postgres-topology.integration.test.sh"]).runLivePostgres, true);
    assert.equal(classifyChangedPaths(["backend/crates/platform/db/src/lib.rs"]).runLivePostgres, true);
    assert.equal(
      classifyChangedPaths(["backend/crates/platform/realtime/tests/postgres_bridge.rs"]).runLivePostgres,
      true,
    );
    assert.equal(classifyChangedPaths(["backend/.sqlx/query-0000.json"]).runLivePostgres, true);
    assert.equal(
      classifyChangedPaths(["backend/ci/gates/writer-ownership/canonical-enforce.sh"]).runLivePostgres,
      true,
    );
    assert.equal(classifyChangedPaths(["backend/ci/gates/rls-arming/src/main.rs"]).runLivePostgres, true);
    assert.equal(classifyChangedPaths(["backend/ci/gates/layer-boundary/src/lib.rs"]).runLivePostgres, false);

    const backendPr = resolvePathClassFromEnv({
      PATH_CLASS_EVENT_NAME: "pull_request",
      PATH_CLASS_PR_BASE_SHA: "b".repeat(40),
      PATH_CLASS_PR_HEAD_SHA: "h".repeat(40),
    });
    // Missing git range fails closed for live postgres as well as run_heavy.
    assert.equal(backendPr.runHeavy, true);
    assert.equal(backendPr.runLivePostgres, true);
  });

  it("resolves a merge group's diff range, and fails closed without its SHAs", () => {
    // A merge group is the queue's candidate merge commit, not a PR. Before the
    // classifier knew the event it fell to `unsupported-event`, which is
    // fail-closed (runHeavy=true) and therefore safe -- but it ran the full
    // matrix for every queue entry, capping throughput at the slowest shard.
    const calls = [];
    const runGit = (bin, args) => {
      calls.push(args);
      return { status: 0, stdout: Buffer.from("docs/current/PRODUCT.md\0") };
    };
    const listed = listChangedPathsForPathClass({
      PATH_CLASS_EVENT_NAME: "merge_group",
      PATH_CLASS_MERGE_GROUP_BASE_SHA: "b".repeat(40),
      PATH_CLASS_MERGE_GROUP_HEAD_SHA: "h".repeat(40),
    }, runGit);
    assert.equal(listed.ok, true);
    assert.deepEqual(listed.paths, ["docs/current/PRODUCT.md"]);
    assert.ok(
      calls[0].includes(`${"b".repeat(40)}...${"h".repeat(40)}`),
      "must diff the queued base against the queued head",
    );

    // Missing either SHA must not silently diff the wrong range.
    for (const env of [
      { PATH_CLASS_EVENT_NAME: "merge_group", PATH_CLASS_MERGE_GROUP_HEAD_SHA: "h".repeat(40) },
      { PATH_CLASS_EVENT_NAME: "merge_group", PATH_CLASS_MERGE_GROUP_BASE_SHA: "b".repeat(40) },
    ]) {
      const bad = listChangedPathsForPathClass(env, runGit);
      assert.equal(bad.ok, false);
      assert.equal(bad.reason, "missing-merge-group-shas");
      assert.equal(resolvePathClassFromEnv(env).runHeavy, true, "must fail closed");
    }
  });

  it("classifies only the complete release metadata shape as thin", () => {
    const releasePaths = [".release-please-manifest.json", "CHANGELOG.md"];
    const custodyPaths = [
      "docs/documentation-manifest.seed.json",
      "docs/documentation-index.json",
    ];
    const expected = {
      pathClass: "release-metadata-only",
      docsOnly: false,
      runHeavy: false,
      runLivePostgres: false,
      reason: "release-metadata-allowlist",
    };

    assert.deepEqual(classifyChangedPaths(releasePaths), expected);
    assert.deepEqual(classifyChangedPaths([...releasePaths, ...custodyPaths]), expected);

    const changelogOnly = classifyChangedPaths([releasePaths[1]]);
    assert.notEqual(changelogOnly.pathClass, "release-metadata-only");
    assert.equal(changelogOnly.pathClass, "docs-only");
    assert.equal(changelogOnly.runHeavy, false);
    for (const custodyPath of custodyPaths) {
      const custodyOnly = classifyChangedPaths([custodyPath]);
      assert.notEqual(custodyOnly.pathClass, "release-metadata-only");
      assert.equal(custodyOnly.pathClass, "docs-only");
      assert.equal(custodyOnly.runHeavy, false);
    }

    for (const paths of [
      [releasePaths[0]],
      [...releasePaths, custodyPaths[0]],
      [...releasePaths, custodyPaths[1]],
      [...releasePaths, "README.md"],
      [...releasePaths, ...custodyPaths, "backend/app/src/lib.rs"],
      [...releasePaths, releasePaths[0]],
      [releasePaths[0], ` ${releasePaths[1]}`],
      [releasePaths[0], `${releasePaths[1]} `],
      [`./${releasePaths[0]}`, releasePaths[1]],
      [releasePaths[0], `./${releasePaths[1]}`],
      [releasePaths[0], `${releasePaths[1]}\n`],
    ]) {
      const classified = classifyChangedPaths(paths);
      assert.notEqual(classified.pathClass, "release-metadata-only", paths.join(", "));
      assert.equal(classified.runHeavy, true, paths.join(", "));
    }
  });

  it("empty successful changed-path list keeps runHeavy / non-docs-only", () => {
    const empty = classifyChangedPaths([]);
    assert.equal(empty.runHeavy, true);
    assert.equal(empty.docsOnly, false);
    assert.notEqual(empty.pathClass, "docs-only");
    assert.equal(empty.reason, "empty-or-unreadable");
  });

  it("lists rename sources so product→docs rename is not docs-only", () => {
    const repo = mkdtempSync(join(tmpdir(), "ci-preflight-rename-inventory-"));
    const git = (args) => {
      const result = spawnSync("git", args, { cwd: repo, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr || result.stdout);
      return result;
    };
    try {
      git(["init", "-q"]);
      git(["config", "user.email", "ci-preflight@example.test"]);
      git(["config", "user.name", "ci-preflight"]);
      mkdirSync(join(repo, "docs"));
      mkdirSync(join(repo, "backend"));
      writeFileSync(join(repo, "docs/a.md"), "a\n");
      writeFileSync(join(repo, "backend/x.rs"), "fn main() {}\n");
      git(["add", "."]);
      git(["commit", "-qm", "base"]);
      const base = git(["rev-parse", "HEAD"]).stdout.trim();
      git(["mv", "backend/x.rs", "docs/x.md"]);
      git(["commit", "-qm", "rename product to docs"]);
      const head = git(["rev-parse", "HEAD"]).stdout.trim();

      // Hostile control: default rename detection drops the source path.
      const defaultListed = spawnSync(
        "git",
        ["diff", "--name-only", `${base}...${head}`],
        { cwd: repo, encoding: "utf8" },
      );
      assert.equal(defaultListed.status, 0, defaultListed.stderr || defaultListed.stdout);
      const defaultPaths = defaultListed.stdout.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
      assert.ok(
        defaultPaths.includes("docs/x.md") && !defaultPaths.includes("backend/x.rs"),
        `expected rename false-green inventory, got ${defaultPaths}`,
      );
      assert.equal(classifyChangedPaths(defaultPaths).pathClass, "docs-only");
      assert.equal(classifyChangedPaths(defaultPaths).runHeavy, false);

      const prev = process.cwd();
      process.chdir(repo);
      try {
        const listed = listChangedPathsForPathClass({
          PATH_CLASS_EVENT_NAME: "pull_request",
          PATH_CLASS_PR_BASE_SHA: base,
          PATH_CLASS_PR_HEAD_SHA: head,
        });
        assert.equal(listed.ok, true);
        assert.ok(listed.paths.includes("backend/x.rs"), `rename source missing: ${listed.paths}`);
        assert.ok(listed.paths.includes("docs/x.md"), `rename dest missing: ${listed.paths}`);
        const resolved = resolvePathClassFromEnv({
          PATH_CLASS_EVENT_NAME: "pull_request",
          PATH_CLASS_PR_BASE_SHA: base,
          PATH_CLASS_PR_HEAD_SHA: head,
        });
        assert.notEqual(resolved.pathClass, "docs-only");
        assert.equal(resolved.runHeavy, true);
        assert.equal(resolved.docsOnly, false);
      } finally {
        process.chdir(prev);
      }
    } finally {
      rmSync(repo, { recursive: true, force: true });
    }
  });

  it("lists deletions so docs edit + product delete is mixed / runHeavy", () => {
    const repo = mkdtempSync(join(tmpdir(), "ci-preflight-delete-inventory-"));
    const git = (args) => {
      const result = spawnSync("git", args, { cwd: repo, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr || result.stdout);
      return result;
    };
    try {
      git(["init", "-q"]);
      git(["config", "user.email", "ci-preflight@example.test"]);
      git(["config", "user.name", "ci-preflight"]);
      mkdirSync(join(repo, "docs"));
      mkdirSync(join(repo, "backend"));
      writeFileSync(join(repo, "docs/a.md"), "a\n");
      writeFileSync(join(repo, "backend/x.rs"), "fn main() {}\n");
      git(["add", "."]);
      git(["commit", "-qm", "base"]);
      const base = git(["rev-parse", "HEAD"]).stdout.trim();
      writeFileSync(join(repo, "docs/a.md"), "a edited\n");
      unlinkSync(join(repo, "backend/x.rs"));
      git(["add", "-A"]);
      git(["commit", "-qm", "docs edit + backend delete"]);
      const head = git(["rev-parse", "HEAD"]).stdout.trim();

      const prev = process.cwd();
      process.chdir(repo);
      try {
        const listed = listChangedPathsForPathClass({
          PATH_CLASS_EVENT_NAME: "pull_request",
          PATH_CLASS_PR_BASE_SHA: base,
          PATH_CLASS_PR_HEAD_SHA: head,
        });
        assert.equal(listed.ok, true);
        assert.ok(listed.paths.includes("backend/x.rs"), `delete missing from inventory: ${listed.paths}`);
        assert.ok(listed.paths.includes("docs/a.md"), `docs edit missing: ${listed.paths}`);
        const resolved = resolvePathClassFromEnv({
          PATH_CLASS_EVENT_NAME: "pull_request",
          PATH_CLASS_PR_BASE_SHA: base,
          PATH_CLASS_PR_HEAD_SHA: head,
        });
        assert.notEqual(resolved.pathClass, "docs-only");
        assert.equal(resolved.runHeavy, true);
        assert.equal(resolved.pathClass, "mixed");
      } finally {
        process.chdir(prev);
      }
    } finally {
      rmSync(repo, { recursive: true, force: true });
    }
  });

  it("preserves exact Git pathname bytes so whitespace cannot alias a release path", () => {
    const repo = mkdtempSync(join(tmpdir(), "ci-preflight-path-bytes-"));
    const git = (args) => {
      const result = spawnSync("git", args, { cwd: repo, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr || result.stdout);
      return result;
    };
    try {
      git(["init", "-q"]);
      git(["config", "user.email", "ci-preflight@example.test"]);
      git(["config", "user.name", "ci-preflight"]);
      writeFileSync(join(repo, "base.txt"), "base\n");
      git(["add", "."]);
      git(["commit", "-qm", "base"]);
      const base = git(["rev-parse", "HEAD"]).stdout.trim();

      writeFileSync(join(repo, ".release-please-manifest.json"), '{}\n');
      writeFileSync(join(repo, " CHANGELOG.md"), "not the canonical changelog\n");
      git(["add", "."]);
      git(["commit", "-qm", "hostile whitespace path"]);
      const head = git(["rev-parse", "HEAD"]).stdout.trim();

      const previous = process.cwd();
      process.chdir(repo);
      try {
        const environment = {
          PATH_CLASS_EVENT_NAME: "pull_request",
          PATH_CLASS_PR_BASE_SHA: base,
          PATH_CLASS_PR_HEAD_SHA: head,
        };
        const listed = listChangedPathsForPathClass(environment);
        assert.equal(listed.ok, true);
        assert.deepEqual(
          [...listed.paths].sort(),
          [" CHANGELOG.md", ".release-please-manifest.json"].sort(),
        );
        const resolved = resolvePathClassFromEnv(environment);
        assert.equal(resolved.pathClass, "unknown");
        assert.equal(resolved.runHeavy, true);
        assert.equal(resolved.reason, "unmapped-path");
      } finally {
        process.chdir(previous);
      }
    } finally {
      rmSync(repo, { recursive: true, force: true });
    }
  });

  it("fails closed on malformed or non-UTF-8 NUL-delimited Git path output", () => {
    assert.deepEqual(parseNulDelimitedChangedPaths(Buffer.from("docs/a.md\0")), {
      ok: true,
      reason: "ok",
      paths: ["docs/a.md"],
    });
    for (const [output, reason] of [
      [Buffer.from("docs/a.md"), "malformed-git-diff-output"],
      [Buffer.from("docs/a.md\0\0"), "malformed-git-diff-output"],
      [Buffer.from([0xff, 0x00]), "non-utf8-path"],
    ]) {
      assert.deepEqual(parseNulDelimitedChangedPaths(output), {
        ok: false,
        reason,
        paths: [],
      });
    }
  });

  it("invokes Git with an exact NUL-delimited no-helper path inventory", () => {
    const base = "a".repeat(40);
    const head = "b".repeat(40);
    const calls = [];
    const listed = listChangedPathsForPathClass({
      PATH_CLASS_EVENT_NAME: "pull_request",
      PATH_CLASS_PR_BASE_SHA: base,
      PATH_CLASS_PR_HEAD_SHA: head,
    }, (...args) => {
      calls.push(args);
      return {
        status: 0,
        stdout: Buffer.from(".release-please-manifest.json\0CHANGELOG.md\0"),
        stderr: Buffer.alloc(0),
      };
    });
    assert.deepEqual(calls, [[
      "git",
      [
        "diff",
        "--name-only",
        "-z",
        "--no-renames",
        "--no-ext-diff",
        `${base}...${head}`,
        "--",
      ],
    ]]);
    assert.deepEqual(listed, {
      ok: true,
      reason: "ok",
      paths: [".release-please-manifest.json", "CHANGELOG.md"],
    });
  });

  it("accepts the workflow's cheap preflight and protected expensive jobs", () => {
    assert.deepEqual(evaluateCiPreflight(workflow).failures, []);
  });

  it("locks Required / CI to the oyatie-style presubmit proofs", () => {
    const requiredDependencies = [
      "preflight",
      "rust-fmt",
      "postgres-domain-reachability",
      "repo-gates",
      "api-contract",
      "kubernetes-manifests",
    ];
    const model = yaml.load(workflow);
    // Presubmit fan-in (2026-08-28): Required / CI no longer waits on domain-unit,
    // backend, generated-faces, company-conformance, or migration expand/contract.
    // Those remain in ci.yml as push/workflow_dispatch postsubmit. rust-fmt is the
    // oyatie lint analog. postgres-domain-reachability stays, skip-proof when
    // run_live_postgres is false. 16 jobs + rust-fmt = 17.
    assert.equal(Object.keys(model.jobs).length, 17);
    assert.equal(model.jobs["required-ci"].name, "Required / CI");
    assert.deepEqual(model.jobs["required-ci"].needs, requiredDependencies);
    assert.equal(model.jobs["required-ci"]["timeout-minutes"], 5);

    const mutations = [
      ["aggregator deletion", replaceJob(workflow, "required-ci", () => "")],
      ["leaf job deletion", replaceJob(workflow, "kubernetes-manifests", () => "")],
      ["leaf job rename", workflow.replace("  preflight:\n", "  preflight-renamed:\n")],
      [
        "extra job",
        workflow.replace(
          "  required-ci:\n",
          "  candidate-shim:\n    runs-on: ubuntu-latest\n    steps: []\n\n  required-ci:\n",
        ),
      ],
      ["context rename", workflow.replace("    name: Required / CI", "    name: Required CI")],
      ["always weakening", workflow.replace("    if: ${{ always() }}", "    if: ${{ success() }}")],
      [
        "dependency deletion",
        workflow.replace("      - kubernetes-manifests\n", ""),
      ],
      [
        "dependency injection",
        workflow.replace("      - kubernetes-manifests\n", "      - kubernetes-manifests\n      - candidate-shim\n"),
      ],
      [
        "result deletion",
        workflow.replace('          test "${{ needs.preflight.result }}" = success &&\n', ""),
      ],
      [
        "skipped accepted",
        workflow.replace(
          '          test "${{ needs.preflight.result }}" = success',
          '          test "${{ needs.preflight.result }}" != failure',
        ),
      ],
      [
        "cancelled accepted",
        workflow.replace(
          '          test "${{ needs.rust-fmt.result }}" = success',
          '          test "${{ needs.rust-fmt.result }}" != cancelled',
        ),
      ],
      [
        "failure accepted",
        workflow.replace(
          '          test "${{ needs.repo-gates.result }}" = success',
          '          test "${{ needs.repo-gates.result }}" != skipped',
        ),
      ],
      [
        "checkout injection",
        workflow.replace(
          "    steps:\n      - name: Require every CI proof to succeed\n",
          "    steps:\n      - uses: actions/checkout@v7\n      - name: Require every CI proof to succeed\n",
        ),
      ],
      [
        "soft failure",
        workflow.replace(
          "      - name: Require every CI proof to succeed\n",
          "      - name: Require every CI proof to succeed\n        continue-on-error: true\n",
        ),
      ],
    ];

    for (const [label, mutated] of mutations) {
      assert.notEqual(mutated, workflow, `${label} fixture drifted`);
      assert.ok(
        evaluateCiPreflight(mutated).failures.length > 0,
        `${label} unexpectedly passed the CI preflight contract`,
      );
    }
  });

  it("keeps PostgreSQL reachability non-evaluated when preflight fails", () => {
    const model = yaml.load(workflow);
    const aggregate = model.jobs["postgres-domain-reachability"];
    assert.equal(aggregate.if, "${{ always() }}");
    assert.deepEqual(
      aggregate.steps.map((step) => ({ name: step.name, if: step.if })),
      [
        {
          name: "Preflight failure non-evaluation",
          if: "${{ needs.preflight.result != 'success' }}",
        },
        {
          name: "Path-class skip proof",
          if: "${{ needs.preflight.result == 'success' && needs.preflight.outputs.run_live_postgres != 'true' }}",
        },
        {
          name: "Require all PostgreSQL reachability facets",
          if: "${{ needs.preflight.result == 'success' && needs.preflight.outputs.run_live_postgres == 'true' }}",
        },
      ],
    );
    assert.match(
      aggregate.steps[0].run,
      /PostgreSQL reachability not evaluated because preflight result=/,
    );

    for (const condition of [
      "${{ needs.preflight.result != 'success' }}",
      "${{ needs.preflight.result == 'success' && needs.preflight.outputs.run_live_postgres != 'true' }}",
      "${{ needs.preflight.result == 'success' && needs.preflight.outputs.run_live_postgres == 'true' }}",
    ]) {
      expectFailure(
        workflow.replace(`        if: ${condition}\n`, "        if: ${{ always() }}\n"),
        "PostgreSQL aggregate must distinguish preflight failure, thin skip, and heavy facets",
      );
    }
  });

  it("rejects every run-step condition, soft-failure, and retained-text early-exit bypass", () => {
    const requiredRunStepCounts = {
      preflight: 33,
      "domain-unit": 2,
      // -1: the expand/contract rehearsal moved to its own job.
      backend: 26,
      "migration-expand-contract": 5,
      "kubernetes-manifests": 8,
      "repo-gates": 26,
      "api-contract": 5,
      "generated-face-authority": 5,
      "company-conformance": 3,
      "postgres-reachability-app": 3,
      "postgres-reachability-platform": 3,
      "postgres-reachability-ontology": 3,
      "postgres-reachability-domain-a": 3,
      // 3, not 2: domain-b is the cargo-nextest pilot and installs the pinned
      // runner before the harness runs. Every one of its run steps is still put
      // through the bypass mutations below, which is what this inventory is for.
      "postgres-reachability-domain-b": 3,
      "postgres-domain-reachability": 3,
      "required-ci": 1,
      "rust-fmt": 1,
    };
    const workflowModel = yaml.load(workflow);
    const bypasses = [
      ["if: false", addFalseCondition],
      ["continue-on-error: true", addContinueOnError],
      ["retained command text after exit 0", addRetainedTextEarlyExit],
    ];
    let runStepCount = 0;
    let mutationCount = 0;

    for (const [job, expectedCount] of Object.entries(requiredRunStepCounts)) {
      const runSteps = workflowModel.jobs[job].steps.filter((step) => typeof step.run === "string");
      assert.equal(runSteps.length, expectedCount, `${job} exhaustive mutation inventory drifted`);
      runStepCount += runSteps.length;

      for (const step of runSteps) {
        for (const [bypass, mutate] of bypasses) {
          const mutated = mutateNamedStep(workflow, job, step.name, mutate);
          const { failures } = evaluateCiPreflight(mutated);
          assert.ok(
            failures.some((failure) => job === "required-ci"
              ? failure.includes("Required / CI")
              : failure.startsWith(`${job} `) && failure.includes("run step")),
            `${job} :: ${step.name} accepted ${bypass}:\n${failures.join("\n")}`,
          );
          mutationCount += 1;
        }
      }
    }

    // 107 -> 121: path-class skip proofs on skip-proof jobs (+classify in preflight).
    // 121 -> 122: lane-receipt validator regression step in preflight.
    // 122 -> 124: fail-slow sweep collect-failures steps in preflight and backend.
    // 125 -> 127: release metadata semantic regression + exact-ref live gate.
    // 127 -> 128: dependency bootstrap for the image-release hardening suite.
    // 128 -> 127: retired the per-record reasoning-lens evidence gate. Two
    // preflight steps (its regression suite and its event/base admission
    // matrix) collapsed into one manifest drift check.
    // 127 -> 128: restored a regression suite for the replacement gate. Review
    // caught that the drift check shipped with no test, so a weakened parser
    // could stay green whenever the checked-in files happened to match.
    // 2026-08-18: dev-up-smoke (7 run steps, 4 setup actions) moved to Nightly,
    // so these ratchets step down by exactly its step counts and no more. A
    // shrink that does NOT match a job leaving ci.yml is still a regression.
    // 2026-08-18: +5 run steps / +3 setup actions from migration-expand-contract,
    // split out of `backend`. Coverage GREW; the ratchet moves up, never down.
    // 2026-08-19: +1 run step from the cargo-nextest install in
    // postgres-reachability-domain-b, the runner pilot.
    // 2026-08-20: +4 more as the pilot rolled out to the remaining four shards.
    // Coverage GREW; each new install step goes through the same three bypass
    // mutations as every other run step. Coverage GREW: the new
    // step goes through the same three bypass mutations as every other one.
    // 2026-08-25: +1 always-on Buck impact planner regression. This closes the
    // previously dark 13-test suite and subjects the new step to all bypasses.
    // 2026-08-28: +1 rust-fmt presubmit run step (oyatie lint analog).
    // +1: isolate the app dev-auth feature case so an earlier failure cannot hide it.
    assert.equal(runStepCount, 133, "required and planned job run-step coverage must not shrink");
    // Three mutations per run step: 133*3 = 399.
    assert.equal(mutationCount, 399, "exhaustive bypass matrix must not shrink");
  });

  it("rejects every setup-action condition and soft-failure bypass", () => {
    // 2026-08-18: the Free runner disk step was removed from every ci.yml job
    // (measured: 87G already free before it ran, 110G after, and zero ENOSPC in
    // any log). These counts step down by exactly one per job that carried it,
    // and by no more.
    const requiredActionStepCounts = {
      preflight: 3,
      "domain-unit": 3,
      backend: 3,
      "migration-expand-contract": 3,
      "kubernetes-manifests": 1,
      "repo-gates": 2,
      "api-contract": 2,
      "generated-face-authority": 4,
      "company-conformance": 2,
      "postgres-reachability-app": 3,
      "postgres-reachability-platform": 3,
      "postgres-reachability-ontology": 3,
      "postgres-reachability-domain-a": 3,
      "postgres-reachability-domain-b": 3,
      "rust-fmt": 2,
    };
    const workflowModel = yaml.load(workflow);
    const bypasses = [
      ["if: false", addFalseCondition],
      ["continue-on-error: true", addContinueOnError],
    ];
    let actionStepCount = 0;
    let mutationCount = 0;

    for (const [job, expectedCount] of Object.entries(requiredActionStepCounts)) {
      const actionSteps = workflowModel.jobs[job].steps.filter((step) => typeof step.uses === "string");
      assert.equal(actionSteps.length, expectedCount, `${job} setup-action inventory drifted`);
      actionStepCount += actionSteps.length;

      for (const step of actionSteps) {
        for (const [bypass, mutate] of bypasses) {
          const mutated = mutateNamedStep(workflow, job, step.name, mutate);
          const { failures } = evaluateCiPreflight(mutated);
          assert.ok(
            failures.some((failure) => failure.startsWith(`${job} setup action step`)),
            `${job} :: ${step.name} accepted ${bypass}:\n${failures.join("\n")}`,
          );
          mutationCount += 1;
        }
      }
    }

    // 2026-08-18: dev-up-smoke (7 run steps, 4 setup actions) moved to Nightly,
    // so these ratchets step down by exactly its step counts and no more. A
    // shrink that does NOT match a job leaving ci.yml is still a regression.
    // 2026-08-18: -7, exactly the Free runner disk steps removed from ci.yml
    // (5 postgres shards + backend + company-conformance) and no more.
    // 2026-08-28: +2 rust-fmt setup actions (checkout + rust-toolchain).
    assert.equal(actionStepCount, 40, "required and planned job setup-action coverage must not shrink");
    // 40 actions x 2 bypass mutations = 80.
    assert.equal(mutationCount, 80, "setup-action bypass matrix must not shrink");
  });

  it("locks every setup action's identity, inputs, totality, and interleaving", () => {
    const jobs = [
      "preflight",
      "domain-unit",
      "backend",
      "kubernetes-manifests",
      "repo-gates",
      "api-contract",
      "generated-face-authority",
      "company-conformance",
      "postgres-reachability-app",
      "postgres-reachability-platform",
      "postgres-reachability-ontology",
      "postgres-reachability-domain-a",
      "postgres-reachability-domain-b",
      "rust-fmt",
    ];
    const workflowModel = yaml.load(workflow);
    let mutationCount = 0;
    const assertActionFailure = (mutated, job, mutation) => {
      const { failures } = evaluateCiPreflight(mutated);
      assert.ok(
        failures.some((failure) => failure.startsWith(`${job} `)
          && (failure.includes("setup action step") || failure.includes("locked ordered action"))),
        `${job} accepted ${mutation}:\n${failures.join("\n")}`,
      );
      mutationCount += 1;
    };

    for (const job of jobs) {
      const actionSteps = workflowModel.jobs[job].steps.filter((step) => typeof step.uses === "string");
      for (const step of actionSteps) {
        assertActionFailure(
          mutateNamedStep(workflow, job, step.name, mutateActionReference),
          job,
          `${step.name} action-reference mutation`,
        );
        assertActionFailure(
          mutateNamedStep(workflow, job, step.name, addUnexpectedActionInput),
          job,
          `${step.name} extra input`,
        );
        assertActionFailure(
          mutateNamedStep(workflow, job, step.name, (text) => duplicateNamedStep(text, step.name)),
          job,
          `${step.name} duplicate action injection`,
        );

        for (const input of Object.keys(step.with ?? {})) {
          assertActionFailure(
            mutateNamedStep(workflow, job, step.name, (text) => mutateActionInput(text, input, null)),
            job,
            `${step.name} deleted ${input} input`,
          );
          assertActionFailure(
            mutateNamedStep(workflow, job, step.name, (text) => (
              mutateActionInput(text, input, '"proof-lock-mutated"')
            )),
            job,
            `${step.name} changed ${input} input`,
          );
        }
      }

      for (let index = 1; index < actionSteps.length; index += 1) {
        assertActionFailure(
          swapNamedSteps(workflow, job, actionSteps[index - 1].name, actionSteps[index].name),
          job,
          `${actionSteps[index - 1].name}/${actionSteps[index].name} order swap`,
        );
      }
    }

    // 2026-08-28: +13 from rust-fmt checkout+toolchain identity/input/order mutations.
    assert.equal(mutationCount, 262, "setup-action identity/input/interleaving matrix must not shrink");
  });

  it("locks the candidate-controlled local free-runner-disk action body", () => {
    expectFailure(
      workflow,
      "free-runner-disk must preserve its exact composite action execution contract",
      postgresWrapperBuildFile,
      freeRunnerDiskAction.replace("      run: |\n", "      run: |\n        exit 0\n"),
    );
    expectFailure(
      workflow,
      "free-runner-disk must preserve its exact composite action execution contract",
      postgresWrapperBuildFile,
      freeRunnerDiskAction.replace("docker system prune -af || true", "printf '%s\\n' skipped"),
    );
  });

  it("locks every required job's complete execution envelope", () => {
    for (const job of [
      "preflight",
      "domain-unit",
      "backend",
      "kubernetes-manifests",
      "repo-gates",
      "api-contract",
      "generated-face-authority",
      "company-conformance",
      "postgres-domain-reachability",
      "rust-fmt",
    ]) {
      expectFailure(
        replaceJob(workflow, job, (block) => block.replace(
          /^    runs-on: .*$/m,
          "    runs-on: ubuntu-24.04",
        )),
        `${job} must preserve its exact job execution envelope`,
      );
      expectFailure(
        replaceJob(workflow, job, (block) => block.replace(
          /^    timeout-minutes: (\d+)$/m,
          (_, timeout) => `    timeout-minutes: ${Number(timeout) + 1}`,
        )),
        `${job} must preserve its exact job execution envelope`,
      );
    }

    expectFailure(
      replaceJob(workflow, "backend", (block) => block.replace(
        "      image: postgres:18.4",
        "      image: postgres:latest",
      )),
      "backend must preserve its exact job execution envelope",
    );
    expectFailure(
      workflow.replace("  preflight:\n", "  preflight:\n    permissions:\n      contents: read\n"),
      "preflight must preserve its exact job execution envelope",
    );
  });

  it("locks the top-level trigger, permission, concurrency, and job-id envelope", () => {
    const envelopeFailure = "CI workflow must preserve its exact trigger, permission, and concurrency execution envelope";
    expectFailure(
      workflow.replace("  contents: read", "  contents: write"),
      envelopeFailure,
    );
    expectFailure(
      workflow.replace("  cancel-in-progress: ${{ github.event_name == 'pull_request' }}", "  cancel-in-progress: true"),
      envelopeFailure,
    );
    expectFailure(workflow.replace("name: CI", "name: Candidate CI"), envelopeFailure);
    expectFailure(
      workflow.replace(
        "  kubernetes-manifests:\n",
        "  candidate-shim:\n    runs-on: ubuntu-latest\n    steps: []\n\n  kubernetes-manifests:\n",
      ),
      "CI workflow must preserve its exact job-id set",
    );
  });

  it("forbids push and pull-request path filters so every required context is created", () => {
    for (const filter of ["paths", "paths-ignore"]) {
      expectFailure(
        workflow.replace(
          '    tags:\n      - "v*"\n',
          `    tags:\n      - "v*"\n    ${filter}:\n      - "backend/**"\n`,
        ),
        "push must create required CI contexts for every change without path filters",
      );
      expectFailure(
        workflow.replace(
          "  pull_request:\n",
          `  pull_request:\n    ${filter}:\n      - "backend/**"\n`,
        ),
        "pull_request must create required CI contexts for every change without path filters",
      );
    }
  });

  it("locks both reasoning-lens steps to one occurrence each, gated on npm ci", () => {
    // Two steps now: the regression suite that proves the gate can fail, and
    // the drift check itself. Review caught that the drift check had shipped
    // with no suite, so a weakened parser could stay green whenever the
    // checked-in files happened to match.
    assert.ok(workflow.includes(reasoningLensManifestStep), "manifest step fixture drifted");
    const message = "reasoning-lens drift check once and only after npm ci succeeds";
    expectFailure(workflow.replace(reasoningLensManifestStep, ""), message);
    expectFailure(
      workflow.replace(reasoningLensManifestStep, addFalseCondition(reasoningLensManifestStep)),
      message,
    );
    expectFailure(
      workflow.replace(reasoningLensManifestStep, addContinueOnError(reasoningLensManifestStep)),
      message,
    );
    // Deleting the regression suite must fail too: without it the drift check
    // is unfalsifiable, which is the defect this pair exists to prevent.
    expectFailure(
      workflow.replace("        run: node --test scripts/check-reasoning-lens-manifest.test.mjs\n", ""),
      "reasoning-lens regression suite once and only after npm ci succeeds",
    );
  });

  it("rejects Buck2 jobs that do not bootstrap pinned DotSlash before invocation", () => {
    expectFailure(
      workflow.replace(
        `      - name: Install pinned DotSlash runtime\n        id: dotslash\n        if: ${preflightCheckoutHeavyIf}\n        run: tools/buck/install_dotslash.sh\n`,
        "",
      ),
      "preflight must install pinned DotSlash before Buck2",
    );
  });

  it("resolves the backend DotSlash bootstrap from its effective working directory", () => {
    expectFailure(
      workflow.replace(
        "        run: ../tools/buck/install_dotslash.sh",
        "        run: tools/buck/install_dotslash.sh",
      ),
      "backend must install pinned DotSlash from ../tools/buck/install_dotslash.sh",
    );
  });

  it("requires backend test runners to run from the repository root", () => {
    for (const stepName of [
      "Buck2 dev-auth feature PostgreSQL suites",
      "Cargo console-app unit suite",
      "Cargo console-app inline PostgreSQL suite",
      "Cargo console-app dev-auth PostgreSQL suite",
    ]) {
      expectFailure(
        mutateNamedStep(workflow, "backend", stepName, (step) =>
          step.replace("        working-directory: .\n", "")),
        "backend must preserve the locked fail-fast step multiset and failure semantics",
      );
    }
  });

  it("rejects CONSOLE_APP_BIN anywhere in the text-only API contract job", () => {
    for (const path of [
      "${{ github.workspace }}/backend/target/debug/console-app",
      "${CARGO_TARGET_DIR}/debug/console-app",
      "/tmp/other-console-app",
    ]) {
      expectFailure(
        workflow.replace(
          "    timeout-minutes: 30\n\n    steps:\n",
          `    timeout-minutes: 30\n    env:\n      CONSOLE_APP_BIN: ${path}\n\n    steps:\n`,
        ),
        "api-contract must not reference CONSOLE_APP_BIN; the job builds no app",
      );
    }
    expectFailure(
      workflow.replace(
        "      - name: Employee import replay contract\n",
        "      - name: Employee import replay contract\n        env:\n          CONSOLE_APP_BIN: /tmp/other-console-app\n",
      ),
      "api-contract must not reference CONSOLE_APP_BIN; the job builds no app",
    );
  });

  it("rejects every GITHUB_ENV handoff surface in the API contract job", () => {
    for (const step of [
      "      - name: Redirected override\n        run: |\n          echo \"CONSOLE_APP_BIN=/tmp/other\" >> \"$GITHUB_ENV\" # still a write\n          :\n",
      "      - name: Tee override\n        run: printf 'CONSOLE_APP_BIN=/tmp/other\\n' | tee -a \"$GITHUB_ENV\"\n",
      "      - name: Programmatic override\n        run: node -e 'require(\"node:fs\").appendFileSync(process.env.GITHUB_ENV, \"X=1\\n\")'\n",
    ]) {
      expectFailure(
        workflow.replace(
          "      - name: Employee import replay contract\n",
          `${step}\n      - name: Employee import replay contract\n`,
        ),
        "api-contract must not hand state to later steps through GITHUB_ENV",
      );
    }
  });

  it("rejects any build or executable surface added to the text-only API contract job", () => {
    for (const command of [
      "tools/buck2 build //backend/app:console-app",
      "$(printf ./tools/buck2) build //backend/app:console-app",
      "command ./tools/buck2 --isolation-dir .tmp build --out .tmp/dup //backend/app:console-app",
      "cargo build -p console-app",
      "bash -c \"npm run check:platform-contract-drift\"",
      "node scripts/check-platform-contract-drift.mjs",
      "node --enable-source-maps scripts/check-platform-contract-drift.mjs",
      // Spelled so no literal GITHUB_ENV appears; the ordered-steps allowlist is
      // what fails it closed, which is why that rule exists alongside the string match.
      'env_name=GITHUB_$(printf ENV); printf "X=1\\n" >> "${!env_name}"',
    ]) {
      expectFailure(
        workflow.replace(
          "      - name: Employee import replay contract\n",
          `      - name: Unexpected executable surface\n        run: ${command}\n\n      - name: Employee import replay contract\n`,
        ),
        "api-contract must contain only the approved ordered steps",
      );
    }
  });

  it("rejects a duplicated platform contract drift gate", () => {
    expectFailure(
      workflow.replace(
        "      - name: Employee import replay contract\n",
        "      - name: Duplicate drift gate\n        run: npm run check:platform-contract-drift\n\n      - name: Employee import replay contract\n",
      ),
      "api-contract must run exactly one npm run check:platform-contract-drift",
    );
  });

  it("accepts the text-only API contract surface", () => {
    assert.deepEqual(evaluateCiPreflight(workflow).failures, []);
  });

  it("rejects services and job-level environment on the text-only API contract", () => {
    for (const block of [
      "    services:\n      postgres:\n        image: postgres:18.4\n",
      "    env:\n      CONTRACT_DATABASE_URL: postgres://postgres:postgres@localhost/db\n",
    ]) {
      expectFailure(
        workflow.replace(
          "    timeout-minutes: 30\n\n    steps:\n",
          `    timeout-minutes: 30\n${block}\n    steps:\n`,
        ),
        "api-contract is text-only and must not provision services or job-level environment",
      );
    }
  });

  it("requires backend DotSlash bootstrap before any Buck or DotSlash invocation", () => {
    const dotSlashStep =
      `      - name: Install pinned DotSlash runtime\n        id: dotslash\n        if: ${backendIndependentIf}\n        run: ../tools/buck/install_dotslash.sh\n`;
    for (const command of ["tools/buck2 --version", "dotslash run //backend/app:console-app"]) {
      expectFailure(
        workflow.replace(
          dotSlashStep,
          `      - name: First Buck invocation\n        if: ${backendIndependentIf}\n        run: ${command}\n\n${dotSlashStep}`,
        ),
        "backend must install pinned DotSlash before its first Buck invocation",
      );
    }
  });

  it("rejects a generated-face authority job without the complete closure", () => {
    expectFailure(
      workflow.replace(
        "tools/buck/preflight.sh --full-generated-faces",
        "tools/buck/preflight.sh --unexpected",
      ),
      "generated-face-authority must run the complete generated-face closure",
    );
  });

  it("requires the lock-sourced Reindeer toolchain before the full generated-face closure", () => {
    const toolchainSetup = `      - name: Install lock-pinned Reindeer Rust toolchain
        if: \${{ needs.preflight.outputs.run_heavy == 'true' }}
        shell: bash
        run: |
          set -euo pipefail
          # shellcheck source=third-party/rust/reindeer/upstream.lock
          source third-party/rust/reindeer/upstream.lock
          rustup toolchain install "$REINDEER_TOOLCHAIN" --profile minimal

`;
    const fullGate = `      - name: Full generated-face closure
        if: \${{ needs.preflight.outputs.run_heavy == 'true' }}
        run: tools/buck/preflight.sh --full-generated-faces
`;

    expectFailure(
      workflow.replace(toolchainSetup, ""),
      "must install the lock-pinned Reindeer Rust toolchain before full generated-face closure",
    );
    expectFailure(
      workflow.replace(
        "source third-party/rust/reindeer/upstream.lock",
        "REINDEER_TOOLCHAIN=hardcoded-not-lock-sourced",
      ),
      "must source third-party/rust/reindeer/upstream.lock",
    );
    expectFailure(
      workflow.replace(toolchainSetup, "").replace(fullGate, `${fullGate}${toolchainSetup}`),
      "must install the lock-pinned Reindeer Rust toolchain before full generated-face closure",
    );
    expectFailure(
      workflow.replace(
        "          set -euo pipefail\n          # shellcheck source=third-party/rust/reindeer/upstream.lock\n          source third-party/rust/reindeer/upstream.lock",
        "          source third-party/rust/reindeer/upstream.lock\n          set -euo pipefail",
      ),
      "must enable strict shell mode before sourcing third-party/rust/reindeer/upstream.lock",
    );
    expectFailure(
      workflow.replace(
        "          source third-party/rust/reindeer/upstream.lock\n          rustup toolchain install \"$REINDEER_TOOLCHAIN\" --profile minimal",
        "          rustup toolchain install \"$REINDEER_TOOLCHAIN\" --profile minimal\n          source third-party/rust/reindeer/upstream.lock",
      ),
      "must source third-party/rust/reindeer/upstream.lock before installing the Reindeer Rust toolchain",
    );
    expectFailure(
      workflow.replace(
        "          rustup toolchain install \"$REINDEER_TOOLCHAIN\" --profile minimal",
        "          export REINDEER_TOOLCHAIN=untrusted\n          rustup toolchain install \"$REINDEER_TOOLCHAIN\" --profile minimal",
      ),
      "must not override REINDEER_TOOLCHAIN after sourcing third-party/rust/reindeer/upstream.lock",
    );
  });

  it("requires the pinned Rust toolchain before Cargo-dependent preflight tests", () => {
    expectFailure(
      workflow.replace(preflightRustToolchainSetup, "").replace(
        `      - name: Cargo.lock consistency\n        id: cargo-lock\n        if: ${preflightRustHeavyIf}\n        run: ${cargoLockGate}\n`,
        `      - name: Cargo.lock consistency\n        id: cargo-lock\n        if: ${preflightRustHeavyIf}\n        run: ${cargoLockGate}\n\n${preflightRustToolchainSetup.trimEnd()}\n`,
      ),
      `preflight must install the pinned Rust toolchain before ${cargoLockGate}`,
    );
  });

  it("requires a full-history checkout for fail-closed path classification", () => {
    expectFailure(
      workflow.replace("          fetch-depth: 0\n", ""),
      "preflight checkout must fetch full history with fetch-depth: 0",
    );
  });

  it("keeps authority regressions without live C/T/M admission in general CI", () => {
    const npmCiIf = "${{ !cancelled() && steps.npm-ci.outcome == 'success' }}";
    const prOnlyIf = "${{ github.event_name == 'pull_request' }}";
    const model = yaml.load(workflow);
    const preflightSteps = model.jobs.preflight.steps;
    const forbiddenNames = new Set([
      "Derive exact console C/T/M train",
      "Console truth-ledger exact-M admission",
      "Console fanout planner exact-M admission",
    ]);
    assert.deepEqual(
      preflightSteps.filter((step) => forbiddenNames.has(step.name)).map((step) => step.name),
      [],
    );
    assert.doesNotMatch(
      JSON.stringify(preflightSteps),
      /CONSOLE_(?:CANDIDATE|AUTHORITY_TIP|SYNTHETIC_MERGE)_SHA/,
    );

    for (const command of [
      "node --test scripts/console/verify-console-authority-train.test.mjs",
      "node --test scripts/console/release-please-pr-envelope.test.mjs scripts/console/release-please-pr-fallback.test.mjs scripts/console/verify-console-pr-authority-bootstrap.test.mjs scripts/console/release-please-bot-candidate.test.mjs scripts/console/release-authority-proof.test.mjs scripts/console/converge-release-please-doc-custody.test.mjs scripts/console/verify-console-merge-group-authority.test.mjs",
      "node --test scripts/console/validate-console-truth-ledger.test.mjs",
      "node --test scripts/console/plan-fanout.test.mjs",
    ]) {
      const matching = preflightSteps.filter((step) => step.run === command);
      assert.equal(matching.length, 1, command);
      assert.equal(matching[0].if, npmCiIf, command);
    }

    const injectedLiveAdmission = workflow.replace(
      "      - name: CI preflight contract tests\n",
      `      - name: Console truth-ledger exact-M admission\n        if: ${npmCiIf}\n        run: npm run check:console-truth-ledger\n\n      - name: CI preflight contract tests\n`,
    );
    expectFailure(injectedLiveAdmission, "must not run live C/T/M admission");

    expectFailure(
      workflow.replace(`        if: ${npmCiIf}\n        run: node --test scripts/console/validate-console-truth-ledger.test.mjs`, `        if: ${prOnlyIf}\n        run: node --test scripts/console/validate-console-truth-ledger.test.mjs`),
      "validate-console-truth-ledger.test.mjs",
    );
    expectFailure(
      workflow.replace(`        if: ${npmCiIf}\n        run: node --test scripts/console/plan-fanout.test.mjs`, `        if: ${prOnlyIf}\n        run: node --test scripts/console/plan-fanout.test.mjs`),
      "plan-fanout.test.mjs",
    );
    expectFailure(
      workflow.replace(`      - name: Console authority-train regression\n        id: authority-train\n        if: ${npmCiIf}\n        run: node --test scripts/console/verify-console-authority-train.test.mjs\n\n`, ''),
      "verify-console-authority-train.test.mjs",
    );
    // This suite gates the `pull_request_target` bootstrap verifier — the highest-privilege
    // script in the repository — and executed NOWHERE: `package.json` declared
    // `test:console-authority-bootstrap` and no workflow invoked it, so breaking the verifier
    // turned every one of its tests red locally while CI stayed green. Wiring it into ci.yml is
    // not the same as protecting it, hence both halves below.
    assert.ok(
      workflow.includes(`      - name: Console PR authority bootstrap regression\n        id: pr-authority-bootstrap\n        if: ${npmCiIf}\n        run: node --test scripts/console/release-please-pr-envelope.test.mjs scripts/console/release-please-pr-fallback.test.mjs scripts/console/verify-console-pr-authority-bootstrap.test.mjs scripts/console/release-please-bot-candidate.test.mjs scripts/console/release-authority-proof.test.mjs scripts/console/converge-release-please-doc-custody.test.mjs scripts/console/verify-console-merge-group-authority.test.mjs\n`),
      "preflight does not run the console PR authority bootstrap regression",
    );
    expectFailure(
      workflow.replace(`      - name: Console PR authority bootstrap regression\n        id: pr-authority-bootstrap\n        if: ${npmCiIf}\n        run: node --test scripts/console/release-please-pr-envelope.test.mjs scripts/console/release-please-pr-fallback.test.mjs scripts/console/verify-console-pr-authority-bootstrap.test.mjs scripts/console/release-please-bot-candidate.test.mjs scripts/console/release-authority-proof.test.mjs scripts/console/converge-release-please-doc-custody.test.mjs scripts/console/verify-console-merge-group-authority.test.mjs\n\n`, ''),
      "verify-console-pr-authority-bootstrap.test.mjs",
    );
    expectFailure(
      workflow.replace(`        if: ${npmCiIf}\n        run: node --test scripts/console/release-please-pr-envelope.test.mjs scripts/console/release-please-pr-fallback.test.mjs scripts/console/verify-console-pr-authority-bootstrap.test.mjs scripts/console/release-please-bot-candidate.test.mjs scripts/console/release-authority-proof.test.mjs scripts/console/converge-release-please-doc-custody.test.mjs scripts/console/verify-console-merge-group-authority.test.mjs`, `        if: ${prOnlyIf}\n        run: node --test scripts/console/release-please-pr-envelope.test.mjs scripts/console/release-please-pr-fallback.test.mjs scripts/console/verify-console-pr-authority-bootstrap.test.mjs scripts/console/release-please-bot-candidate.test.mjs scripts/console/release-authority-proof.test.mjs scripts/console/converge-release-please-doc-custody.test.mjs scripts/console/verify-console-merge-group-authority.test.mjs`),
      "verify-console-pr-authority-bootstrap.test.mjs",
    );
  });

  it("runs release metadata semantics, custody, and links on the exact thin boundary", () => {
    const npmCiIf = "${{ !cancelled() && steps.npm-ci.outcome == 'success' }}";
    const releaseMetadataIf = "${{ !cancelled() && steps.npm-ci.outcome == 'success' && steps.path_class.outputs.path_class == 'release-metadata-only' }}";
    const expected = [
      {
        name: "Release metadata semantic regression",
        id: "release-metadata-regression",
        if: npmCiIf,
        run: "node --test scripts/check-release-metadata.test.mjs",
      },
      {
        name: "Release metadata semantic gate",
        id: "release-metadata",
        if: releaseMetadataIf,
        shell: "bash",
        env: {
          RELEASE_METADATA_BASE_SHA: "${{ github.event_name == 'pull_request' && github.event.pull_request.base.sha || github.event.merge_group.base_sha || github.event.before }}",
          RELEASE_METADATA_HEAD_SHA: "${{ github.event_name == 'pull_request' && github.event.pull_request.head.sha || github.event.merge_group.head_sha || github.sha }}",
        },
        run: [
          "set -euo pipefail",
          "node scripts/check-release-metadata.mjs \\",
          '  --base "$RELEASE_METADATA_BASE_SHA" \\',
          '  --head "$RELEASE_METADATA_HEAD_SHA"',
          "",
        ].join("\n"),
      },
      {
        name: "Release metadata documentation link tests",
        id: "release-doc-link-tests",
        if: releaseMetadataIf,
        run: "node --test scripts/check-doc-links.test.mjs",
      },
      {
        name: "Release metadata documentation manifest gate",
        id: "release-doc-manifest",
        if: releaseMetadataIf,
        run: "npm run check:doc-manifest",
      },
      {
        name: "Release metadata documentation local-link gate",
        id: "release-doc-links",
        if: releaseMetadataIf,
        run: "npm run check:doc-links",
      },
    ];
    const model = yaml.load(workflow);
    const actual = model.jobs.preflight.steps.filter((step) => (
      typeof step.name === "string" && step.name.startsWith("Release metadata ")
    ));
    assert.deepEqual(actual, expected);

    for (const proof of expected.slice(1)) {
      expectFailure(
        mutateNamedStep(workflow, "preflight", proof.name, (step) => (
          step.replace(`        if: ${releaseMetadataIf}\n`, "        if: ${{ !cancelled() }}\n")
        )),
        "preflight must preserve release-metadata-only semantic, custody, and link proofs",
      );
    }
    expectFailure(
      workflow.replace(
        `        if: ${npmCiIf}\n        run: node --test scripts/check-release-metadata.test.mjs`,
        `        if: ${releaseMetadataIf}\n        run: node --test scripts/check-release-metadata.test.mjs`,
      ),
      "release metadata semantic regression after npm ci",
    );
    expectFailure(
      workflow.replace(
        "github.event.pull_request.head.sha || github.event.merge_group.head_sha || github.sha",
        "github.sha || github.event.merge_group.head_sha || github.sha",
      ),
      "preflight must preserve its exact step environment allowlist",
    );
    expectFailure(
      workflow.replace(
        "github.event.pull_request.base.sha || github.event.merge_group.base_sha",
        "github.event.pull_request.head.sha || github.event.merge_group.base_sha",
      ),
      "preflight must preserve its exact step environment allowlist",
    );
  });

  it("rejects omission and comment-only reachability regressions", () => {
    const alwaysCommand = "node --test scripts/console/route-inventory.test.mjs";
    expectFailure(workflow.replace(`        run: ${alwaysCommand}\n`, ""), alwaysCommand);
    expectFailure(workflow.replace(`        run: ${alwaysCommand}\n`, `        # ${alwaysCommand}\n`), alwaysCommand);
    for (const command of [
      "tools/buck/run_test_with_postgres_env.test.sh",
      "tools/buck/test_needs_postgres.test.sh",
    ]) {
      const gated = `        if: ${preflightBuckHeavyIf}\n        run: ${command}\n`;
      assert.ok(workflow.includes(gated), `missing gated reachability command ${command}`);
      expectFailure(workflow.replace(gated, ""), command);
      expectFailure(workflow.replace(gated, `        if: ${preflightBuckHeavyIf}\n        # ${command}\n`), command);
    }
  });

  it("rejects conditional and continue-on-error reachability regressions", () => {
    const alwaysCommand = "node --test scripts/console/route-inventory.test.mjs";
    expectFailure(
      workflow.replace(`        if: ${npmCiIf}\n        run: ${alwaysCommand}\n`, `        if: \${{ false }}\n        run: ${alwaysCommand}\n`),
      "only when",
    );
    expectFailure(
      workflow.replace(`        if: ${npmCiIf}\n        run: ${alwaysCommand}\n`, `        if: ${npmCiIf}\n        continue-on-error: true\n        run: ${alwaysCommand}\n`),
      "only when",
    );
    for (const command of [
      "tools/buck/run_test_with_postgres_env.test.sh",
      "tools/buck/test_needs_postgres.test.sh",
    ]) {
      expectFailure(
        workflow.replace(
          `        if: ${preflightBuckHeavyIf}\n        run: ${command}\n`,
          `        if: \${{ false }}\n        run: ${command}\n`,
        ),
        "only when",
      );
      expectFailure(
        workflow.replace(
          `        if: ${preflightBuckHeavyIf}\n        run: ${command}\n`,
          `        if: ${preflightBuckHeavyIf}\n        continue-on-error: true\n        run: ${command}\n`,
        ),
        "only when",
      );
    }
  });

  it("rejects a preflight that does not run npm and Cargo lock consistency gates", () => {
    expectFailure(workflow.replace("npm run check:package-lock", "npm run check:root-workspaces"), "check:package-lock");
    expectFailure(
      workflow.replace(
        cargoLockGate,
        "cargo metadata --manifest-path backend/Cargo.toml --format-version=1 >/dev/null",
      ),
      cargoLockGate,
    );
  });

  it("rejects a dependency missing from Cargo.lock while the clean lock passes", () => {
    const root = mkdtempSync(join(tmpdir(), "maintenance-cargo-lock-"));
    const app = join(root, "app");
    const dependency = join(root, "dependency");
    const extra = join(root, "extra");
    try {
      for (const directory of [app, dependency, extra]) {
        mkdirSync(join(directory, "src"), { recursive: true });
      }
      writeFileSync(join(root, "Cargo.toml"), "[workspace]\nmembers = [\"app\", \"dependency\"]\nresolver = \"2\"\n");
      for (const [directory, name] of [[app, "fixture-app"], [dependency, "fixture-dependency"]]) {
        writeFileSync(join(directory, "Cargo.toml"), `[package]\nname = \"${name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n`);
        writeFileSync(join(directory, "src/lib.rs"), "pub fn fixture() {}\n");
      }
      writeFileSync(join(app, "Cargo.toml"), "[package]\nname = \"fixture-app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nfixture-dependency = { path = \"../dependency\" }\n");
      assert.equal(spawnSync("cargo", ["generate-lockfile"], { cwd: root }).status, 0);
      assert.equal(spawnSync("cargo", ["metadata", "--manifest-path", join(app, "Cargo.toml"), "--locked", "--format-version=1"], { cwd: root }).status, 0);

      writeFileSync(join(extra, "Cargo.toml"), "[package]\nname = \"fixture-extra\"\nversion = \"0.1.0\"\nedition = \"2024\"\n");
      writeFileSync(join(extra, "src/lib.rs"), "pub fn extra() {}\n");
      writeFileSync(join(app, "Cargo.toml"), "[package]\nname = \"fixture-app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nfixture-dependency = { path = \"../dependency\" }\nfixture-extra = { path = \"../extra\" }\n");
      assert.equal(spawnSync("cargo", ["metadata", "--manifest-path", join(app, "Cargo.toml"), "--locked", "--no-deps", "--format-version=1"], { cwd: root }).status, 0);
      assert.notEqual(spawnSync("cargo", ["metadata", "--manifest-path", join(app, "Cargo.toml"), "--locked", "--format-version=1"], { cwd: root }).status, 0);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("rejects a preflight command that appears only in a comment", () => {
    expectFailure(
      workflow.replace(
        `        if: ${npmCiIf}\n        run: npm run check:package-lock`,
        `        if: ${npmCiIf}\n        # npm run check:package-lock`,
      ),
      "check:package-lock",
    );
  });

  it("rejects a required preflight step guarded by a condition", () => {
    expectFailure(
      workflow.replace(
        `        if: ${npmCiIf}\n        run: npm run check:package-lock`,
        `        if: \${{ false }}\n        run: npm run check:package-lock`,
      ),
      "only when",
    );
  });

  it("rejects a required preflight step allowed to continue on error", () => {
    expectFailure(
      workflow.replace(
        `        if: ${npmCiIf}\n        run: npm run check:package-lock`,
        `        if: ${npmCiIf}\n        continue-on-error: true\n        run: npm run check:package-lock`,
      ),
      "only when",
    );
  });

  it("rejects any expensive job without the preflight dependency", () => {
    for (const job of ["backend", "repo-gates", "api-contract", "kubernetes-manifests", "company-conformance"]) {
      expectFailure(
        replaceJob(workflow, job, (block) => block.replace("    needs: preflight\n", "    needs: []\n")),
        `${job} must need preflight`,
      );
    }
  });

  it("rejects failure-insensitive job-level conditions on protected jobs", () => {
    const postsubmitIf =
      "    if: ${{ needs.preflight.result == 'success' && needs.preflight.outputs.run_heavy == 'true' && (github.event_name == 'push' || github.event_name == 'workflow_dispatch') }}\n";
    expectFailure(
      replaceJob(workflow, "backend", (block) => block.replace(postsubmitIf, "    if: always()\n")),
      "backend must run only on push/workflow_dispatch when run_heavy",
    );
    expectFailure(workflow.replace("  repo-gates:\n", "  repo-gates:\n    if: ${{ !cancelled() }}\n"), "repo-gates must not define job-level if");
    expectFailure(
      replaceJob(workflow, "company-conformance", (block) => block.replace(postsubmitIf, "    if: false\n")),
      "company-conformance must run only on push/workflow_dispatch when run_heavy",
    );
    expectFailure(
      replaceJob(workflow, "migration-expand-contract", (block) => block.replace(postsubmitIf, "    if: false\n")),
      "migration-expand-contract must run only on push/workflow_dispatch when run_heavy",
    );
    for (const job of ["backend", "api-contract", "company-conformance", "migration-expand-contract", "rust-fmt"]) {
      expectFailure(
        workflow.replace(`  ${job}:\n`, `  ${job}:\n    continue-on-error: true\n`),
        `${job} must not define job-level continue-on-error`,
      );
    }
    expectFailure(
      workflow.replace("  rust-fmt:\n", "  rust-fmt:\n    if: always()\n"),
      "rust-fmt must not define job-level if",
    );
    expectFailure(
      replaceJob(workflow, "rust-fmt", (block) => block.replace(
        "    runs-on: ubuntu-latest\n",
        "    runs-on: ubuntu-latest\n    needs: preflight\n",
      )),
      "rust-fmt must not need preflight",
    );
  });

  it("rejects job-level preflight failure bypasses", () => {
    expectFailure(
      workflow.replace("  preflight:\n", "  preflight:\n    if: always()\n"),
      "preflight must not define job-level if",
    );
    expectFailure(
      workflow.replace("  preflight:\n", "  preflight:\n    continue-on-error: true\n"),
      "preflight must not define job-level continue-on-error",
    );
    expectFailure(
      workflow.replace("  preflight:\n", "  preflight:\n    defaults:\n      run:\n        shell: bash -c 'exit 0' {0}\n"),
      "preflight must not override defaults.run.shell",
    );
    expectFailure(
      workflow.replace("        shell: bash\n", "        shell: bash -c 'exit 0' {0}\n"),
      "preflight may use only the default shell or canonical shell: bash",
    );
  });

  it("rejects workflow-level environment and shell overrides", () => {
    expectFailure(
      workflow.replace("permissions:\n", "env:\n  BASH_ENV: scripts/noop.sh\n\npermissions:\n"),
      "CI workflow must not define workflow-level env or defaults",
    );
    expectFailure(
      workflow.replace(
        "permissions:\n",
        "defaults:\n  run:\n    shell: bash -c 'exit 0' {0}\n\npermissions:\n",
      ),
      "CI workflow must not define workflow-level env or defaults",
    );
  });

  it("locks exact job-level environment and defaults metadata", () => {
    // Scoped to the backend block: the postgres shards now carry the same two
    // CARGO_PROFILE_* vars (so they can share backend's rust-cache entry), so a
    // bare workflow.replace() would mutate the first shard instead.
    expectFailure(
      replaceJob(workflow, "backend", (block) => block.replace(
        '      CARGO_PROFILE_TEST_DEBUG: "0"\n',
        '      CARGO_PROFILE_TEST_DEBUG: "0"\n      RUSTC_WRAPPER: scripts/noop.sh\n',
      )),
      "backend must preserve its exact job env/defaults execution metadata",
    );
    expectFailure(
      replaceJob(workflow, "postgres-reachability-domain-b", (block) => block.replace(
        '      CARGO_PROFILE_TEST_DEBUG: "0"\n',
        '      CARGO_PROFILE_TEST_DEBUG: "0"\n      RUSTC_WRAPPER: scripts/noop.sh\n',
      )),
      "postgres-reachability-domain-b must preserve its exact job env/defaults execution metadata",
    );
    // The shard env exists to match the cache writer; dropping it silently
    // restores the permanent cache miss this alignment was written to fix.
    expectFailure(
      replaceJob(workflow, "postgres-reachability-app", (block) => block.replace(
        '      CARGO_PROFILE_DEV_DEBUG: "0"\n',
        '',
      )),
      "postgres-reachability-app must preserve its exact job env/defaults execution metadata",
    );
    expectFailure(
      workflow.replace("  repo-gates:\n", "  repo-gates:\n    env:\n      PATH: /tmp/noop\n"),
      "repo-gates must preserve its exact job env/defaults execution metadata",
    );
    expectFailure(
      workflow.replace(
        "        working-directory: backend\n",
        "        working-directory: backend\n        shell: bash -c 'exit 0' {0}\n",
      ),
      "backend must preserve its exact job env/defaults execution metadata",
    );
  });

  it("rejects step environment injection across every protected proof class", () => {
    const mutations = [
      ["preflight", "      - name: CI preflight contract tests\n"],
      ["postgres-reachability-app", "      - name: Run disposable PostgreSQL integration targets\n"],
      ["company-conformance", "      - name: Company conformance against disposable PostgreSQL\n"],
      ["generated-face-authority", "      - name: Full generated-face closure\n"],
      ["backend", "      - name: Layer-boundary gate\n"],
      ["repo-gates", "      - name: Undeclared imports — every bare specifier must be declared\n"],
      ["api-contract", "      - name: Platform contract drift gate\n"],
      ["kubernetes-manifests", "      - name: Production hardening contract\n"],
    ];
    for (const [job, anchor] of mutations) {
      expectFailure(
        workflow.replace(anchor, `${anchor}        env:\n          BASH_ENV: scripts/noop.sh\n`),
        `${job} must preserve its exact step environment allowlist`,
      );
    }

    expectFailure(
      workflow.replace(
        '          KUBECTL_VERSION: "v1.36.2"\n',
        '          KUBECTL_VERSION: "v1.36.2"\n          BASH_ENV: scripts/noop.sh\n',
      ),
      "kubernetes-manifests must preserve its exact step environment allowlist",
    );
  });

  it("locks post-preflight test reachability targets and disallows added run surfaces", () => {
    expectFailure(
      workflow.replace(" -p console-payroll-adapter-postgres", ""),
      "domain-unit must run -p console-payroll-adapter-postgres",
    );
    expectFailure(
      workflow.replace(
        "tools/ci/cargo_needs_postgres.sh --workflow-only --shard-id app --num-threads=1",
        "tools/ci/cargo_needs_postgres.sh --num-threads=1",
      ),
      "postgres-reachability-app must run the locked PostgreSQL reachability targets",
    );
    expectFailure(
      workflow.replace(
        "tools/ci/cargo_needs_postgres.sh --workflow-only --shard-id app --num-threads=1",
        "tools/buck/test_needs_postgres.sh --num-threads=1",
      ),
      "postgres-reachability-app must run the locked PostgreSQL reachability targets",
    );
    expectFailure(
      workflow,
      "tools/buck/BUCK must bind PostgreSQL wrapper dispatch-p1-postgres to the loader and exact Rust binary",
      postgresWrapperBuildFile.replace(
        'name = "dispatch-p1-postgres",\n    test = "run_test_with_postgres_env.sh",',
        'name = "dispatch-p1-postgres",\n    test = "unexpected_loader.sh",',
      ),
    );
    expectFailure(
      workflow,
      "tools/buck/BUCK must bind PostgreSQL wrapper attendance-concurrency-postgres to the loader and exact Rust binary",
      postgresWrapperBuildFile.replace(
        'args = ["$(location //backend/crates/attendance/adapter-postgres:console-attendance-adapter-postgres-itest-concurrency)"]',
        'args = ["$(location //backend/crates/attendance/adapter-postgres:console-attendance-adapter-postgres-itest-cancel_substitution)"]',
      ),
    );
    expectFailure(
      workflow.replace(
        "      - name: Domain crate unit tests\n",
        "      - name: Unexpected Cargo test\n        run: cargo test -p console-support-domain\n\n      - name: Domain crate unit tests\n",
      ),
      "domain-unit must contain only the locked ordered run steps",
    );
    expectFailure(
      workflow.replace(
        "cargo test --locked --manifest-path backend/Cargo.toml -p console-app --lib --features test-postgres",
        "env -u DATABASE_URL cargo test --locked --manifest-path backend/Cargo.toml -p console-app --lib --features test-postgres",
      ),
      "backend must preserve the locked fail-fast step multiset and failure semantics",
    );
    expectFailure(
      workflow.replace(
        "cargo test --locked --manifest-path backend/Cargo.toml -p console-app --test dev_auth_persona_guard_feature --features dev-auth",
        "env -u DATABASE_URL cargo test --locked --manifest-path backend/Cargo.toml -p console-app --test dev_auth_persona_guard_feature --features dev-auth",
      ),
      "backend must preserve the locked fail-fast step multiset and failure semantics",
    );
    expectFailure(
      workflow.replace(
        "//tools/buck:auth-rest-dev-auth-group-admin-postgres",
        "//backend/crates/platform/auth-rest:console-platform-auth-rest-itest-group_admin_tenant_context",
      ),
      "backend must preserve the locked fail-fast step multiset and failure semantics",
    );
    expectFailure(
      workflow.replace(
        "//tools/buck:provisioning-dev-principal-upsert-race-postgres",
        "//backend/crates/platform/provisioning:console-platform-provisioning-itest-dev_principal_upsert_race",
      ),
      "backend must preserve the locked fail-fast step multiset and failure semantics",
    );
    expectFailure(
      workflow,
      "tools/buck/BUCK must bind PostgreSQL wrapper auth-rest-dev-auth-inline-postgres to the loader and exact Rust binary",
      postgresWrapperBuildFile.replace(
        'name = "auth-rest-dev-auth-inline-postgres",\n    test = "run_test_with_postgres_env.sh",',
        'name = "auth-rest-dev-auth-inline-postgres",\n    test = "unexpected_loader.sh",',
      ),
    );
    expectFailure(
      workflow,
      "tools/buck/BUCK must bind PostgreSQL wrapper provisioning-dev-principal-upsert-race-postgres to the loader and exact Rust binary",
      postgresWrapperBuildFile.replace(
        'deps = ["//backend/crates/platform/provisioning:console-platform-provisioning-itest-dev_principal_upsert_race"],',
        'deps = ["//backend/crates/platform/auth-rest:console-platform-auth-rest-itest-dev_auth_session"],',
      ),
    );
    expectFailure(
      workflow.replace(
        "      - name: Buck2 dev-auth feature PostgreSQL suites\n",
        "      - name: Direct Cargo dev-auth suite\n        run: cargo test -p console-platform-auth-rest --features dev-auth\n\n      - name: Buck2 dev-auth feature PostgreSQL suites\n",
      ),
      "backend must not run direct Cargo PostgreSQL tests for console-platform-auth-rest",
    );
    expectFailure(
      workflow.replace(
        "      - name: Buck2 dev-auth feature PostgreSQL suites\n",
        "      - name: Direct Cargo provisioning race\n        run: cargo test -p console-platform-provisioning --test dev_principal_upsert_race\n\n      - name: Buck2 dev-auth feature PostgreSQL suites\n",
      ),
      "backend must not run direct Cargo PostgreSQL tests for console-platform-provisioning",
    );
    const cargo = ["car", "go"].join("");
    const test = ["te", "st"].join("");
    const backendMarker = "      - name: Buck2 dev-auth feature PostgreSQL suites\n";
    const insertBackendRun = (command) => workflow.replace(
      backendMarker,
      "      - name: Adversarial direct Cargo target\n        run: |\n          "
        + command
        + "\n\n"
        + backendMarker,
    );
    for (const packageName of ["console-platform-auth-rest", "console-platform-provisioning"]) {
      for (const runner of [cargo + " " + test, cargo + " nextest run"]) {
        for (const packageArgument of [
          "-p " + packageName,
          "-p=" + packageName,
          "--package " + packageName,
          "--package=" + packageName,
        ]) {
          expectFailure(
            insertBackendRun(runner + " " + packageArgument),
            "backend must not run direct Cargo PostgreSQL tests for " + packageName,
          );
          for (const prefix of [
            "command env SQLX_OFFLINE=true ",
            "command -- env -- ",
            "command -p env SQLX_OFFLINE=true ",
            "command -p -- env -- ",
            "env -i command -- ",
          ]) {
            expectFailure(
              insertBackendRun(prefix + runner + " " + packageArgument),
              "backend must not run direct Cargo PostgreSQL tests for " + packageName,
            );
          }
          expectFailure(
            insertBackendRun("env -S 'command env " + runner + " " + packageArgument + "'"),
            "backend must not run direct Cargo PostgreSQL tests for " + packageName,
          );
          expectFailure(
            insertBackendRun("env -S 'command -p env " + runner + " " + packageArgument + "'"),
            "backend must not run direct Cargo PostgreSQL tests for " + packageName,
          );
          expectFailure(
            insertBackendRun("env -S 'command -p -- env -- " + runner + " " + packageArgument + "'"),
            "backend must not run direct Cargo PostgreSQL tests for " + packageName,
          );
        }
      }
    }
    for (const [packageName, command] of [
      ["console-platform-provisioning", cargo + " \\\n          " + test + " \\\n          --package \\\n          console-platform-provisioning"],
      ["console-platform-auth-rest", "env SQLX_OFFLINE=true " + cargo + " nextest run \\\n          -p=console-platform-auth-rest"],
      ["console-platform-auth-rest", "env -u DATABASE_URL -- " + cargo + " nextest \\\n          run --package=console-platform-auth-rest"],
      ["console-platform-provisioning", "command " + cargo + " " + test + " --package console-platform-provisioning"],
    ]) {
      expectFailure(
        insertBackendRun(command),
        "backend must not run direct Cargo PostgreSQL tests for " + packageName,
      );
    }
    for (const command of [
      "# " + cargo + " " + test + " -p console-platform-auth-rest",
      cargo + " run -p console-platform-auth-rest",
      "echo " + cargo + " " + test + " -p console-platform-provisioning",
      "command -v " + cargo + " " + test + " -p console-platform-auth-rest",
      "command -V " + cargo + " " + test + " -p console-platform-provisioning",
    ]) {
      const failures = evaluateCiPreflight(insertBackendRun(command)).failures;
      assert.ok(
        !failures.some((failure) => failure.startsWith("backend must not run direct Cargo PostgreSQL tests")),
        failures.join("\n"),
      );
    }
    for (const command of ["command --", "env -S", "env -S 'command --'"]) {
      expectFailure(
        insertBackendRun(command),
        "backend must not contain a malformed executable shell surface",
      );
    }
    expectFailure(
      insertBackendRun(
        "command -p " + cargo + " " + test + " -p console-platform-auth-rest \\",
      ),
      "backend must not contain a malformed executable shell surface",
    );
    expectFailure(
      workflow,
      "tools/buck/BUCK must bind PostgreSQL wrapper app-inline-postgres to the loader and exact Rust binary",
      postgresWrapperBuildFile.replace(
        'name = "app-inline-postgres",\n    test = "run_test_with_postgres_env.sh",\n    args = ["$(location //backend/app:console-app-itest-inline-postgres)"],\n    deps = ["//backend/app:console-app-itest-inline-postgres"],\n    labels = ["test.integration", "resource.postgres", "needs-postgres"],',
        'name = "app-inline-postgres",\n    test = "run_test_with_postgres_env.sh",\n    args = ["$(location //backend/app:console-app-itest-inline-postgres)"],\n    deps = ["//backend/app:console-app-itest-inline-postgres"],\n    labels = ["owner.backend.app", "domain.app", "test.integration", "resource.postgres", "needs-postgres"],',
      ),
    );
    expectFailure(
      workflow,
      "tools/buck/BUCK must bind PostgreSQL wrapper app-dev-auth-persona-guard-postgres to the loader and exact Rust binary",
      postgresWrapperBuildFile.replace(
        'name = "app-dev-auth-persona-guard-postgres",\n    test = "run_test_with_postgres_env.sh",',
        'name = "app-dev-auth-persona-guard-postgres",\n    test = "run_test_with_postgres_env.sh",\n    args = ["$(location //backend/app:console-app-itest-inline-postgres)"],',
      ),
    );
    expectFailure(
      workflow,
      "tools/buck/BUCK must bind PostgreSQL wrapper app-dev-auth-persona-guard-postgres to the loader and exact Rust binary",
      postgresWrapperBuildFile.replace(
        'deps = ["//backend/app:console-app-itest-dev_auth_persona_guard_feature"],',
        'deps = ["//backend/app:console-app-itest-inline-postgres"],',
      ),
    );
  });

  // Renamed 2026-07-31: #534 consolidated support-domain-unit and domain-unit
  // into one `domain-unit` job running both crates through a single cargo invocation,
  // because they share console-kernel-core and were recompiling it twice across two
  // runner startups. The assertion is unchanged in substance — the payroll release-gate
  // targets must stay reachable from a protected job — only the job's name moved.
  it("keeps both payroll release-gate halves reachable from domain-unit", () => {
    // Slice A wrote this against buck2 targets; #534 moved the job to cargo, so the
    // MECHANISM changed and the INTENT did not. The domain half decides whether a
    // parsed gate input is satisfied; the adapter half decides what a stored record
    // may parse INTO. Dropping either returns the release gate to half-proven, which
    // is what this job exists to stop. The adapter half is the one that ran nowhere
    // before 2026-07-31 — 12 pure #[test] cases, no workflow.
    expectFailure(
      workflow.replace(/\n  domain-unit:[\s\S]*?\n  postgres-domain-reachability:/, "\n  postgres-domain-reachability:"),
      "CI must define protected job domain-unit",
    );
    expectFailure(
      replaceJob(workflow, "domain-unit", (block) => block.replace("    needs: preflight\n", "    needs: []\n")),
      "domain-unit must need preflight",
    );
    // Dropping EITHER package must fail, which is the whole point of the pairing.
    expectFailure(
      workflow.replace(" -p console-payroll-adapter-postgres", ""),
      "domain-unit must run",
    );
    expectFailure(
      workflow.replace(" -p console-payroll-domain -p console-payroll-adapter-postgres", " -p console-payroll-adapter-postgres"),
      "domain-unit must run",
    );
    // --lib is load-bearing: without it the adapter's PostgreSQL integration suites
    // are pulled into a job that has no database.
    expectFailure(
      workflow.replace(" --lib \\", " \\"),
      "domain-unit must pass --lib on its first cargo invocation",
    );
    // Added 2026-07-31: the audit-relevant packages must stay named. Dropping one
    // silently returns its tests to executing nowhere.
    expectFailure(
      workflow.replace(" -p console-platform-audit-chain", ""),
      "domain-unit must run -p console-platform-audit-chain",
    );
    expectFailure(
      workflow.replace(" --test location_consent_fsm", ""),
      "domain-unit must run --test location_consent_fsm",
    );
  });

  it("rejects non-executing or non-gating domain-unit command surfaces", () => {
    expectFailure(
      mutateNamedStep(workflow, "domain-unit", "Domain crate unit tests", (step) =>
        step.replace(/^(\s*)(SQLX_OFFLINE=true cargo test)/gm, "$1echo $2")),
      "domain-unit must execute the locked Cargo test commands directly when run_heavy",
    );
    expectFailure(
      mutateNamedStep(workflow, "domain-unit", "Domain crate unit tests", (step) =>
        step.replace(/^        run: \|$/m, "        run: |\n          exit 0")),
      "domain-unit proof run step 2 must preserve",
    );
    for (const condition of ["false", "${{ false }}"]) {
      expectFailure(
        mutateNamedStep(workflow, "domain-unit", "Domain crate unit tests", (step) =>
          step.replace(/^        if: .*$/m, `        if: ${condition}`)),
        "domain-unit must execute the locked Cargo test commands directly when run_heavy",
      );
    }
    expectFailure(
      mutateNamedStep(workflow, "domain-unit", "Domain crate unit tests", (step) =>
        step.replace(/^        if: .*$/m, `        if: ${runHeavyIf}\n        continue-on-error: true`)),
      "domain-unit must execute the locked Cargo test commands directly when run_heavy",
    );
    expectFailure(
      mutateNamedStep(workflow, "domain-unit", "Domain crate unit tests", (step) =>
        step.replace(/^        if: .*$/m, `        if: ${runHeavyIf}\n        shell: bash -c 'exit 0' {0}`)),
      "domain-unit may use only the default shell or canonical shell: bash",
    );
    expectFailure(
      replaceJob(workflow, "domain-unit", (block) => block.replace(
        "  domain-unit:\n",
        "  domain-unit:\n    defaults:\n      run:\n        shell: bash -c 'exit 0' {0}\n",
      )),
      "domain-unit must use the default shell with no job or step env/defaults overrides",
    );
    expectFailure(
      mutateNamedStep(workflow, "domain-unit", "Domain crate unit tests", (step) =>
        step.replace(/^        if: .*$/m, `        if: ${runHeavyIf}\n        env:\n          RUSTFLAGS: --cfg skip_tests`)),
      "domain-unit must use the default shell with no job or step env/defaults overrides",
    );
    expectFailure(
      replaceJob(workflow, "domain-unit", (block) => block.replace(
        "      CARGO_PROFILE_TEST_DEBUG: \"0\"\n",
        "      CARGO_PROFILE_TEST_DEBUG: \"0\"\n      CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER: true\n",
      )),
      "domain-unit must use the default shell with no job or step env/defaults overrides",
    );
  });

  it("derives the ordered domain-unit integration inventory and checks its baseline", () => {
    const attendance = `          SQLX_OFFLINE=true cargo test --locked --manifest-path backend/Cargo.toml \\
            -p console-attendance-application --test attendance_policy
          check_status "attendance_policy"
`;
    const compliance = `          SQLX_OFFLINE=true cargo test --locked --manifest-path backend/Cargo.toml \\
            -p console-compliance-domain --test location_consent_fsm --test location_ping_policy
          check_status "location_consent_fsm + location_ping_policy"
`;
    assert.ok(workflow.includes(attendance));
    assert.ok(workflow.includes(compliance));

    const omit = mutateNamedStep(workflow, "domain-unit", "Domain crate unit tests", (step) =>
      step.replace(attendance, ""));
    expectFailure(omit, "domain-unit must execute the locked Cargo test commands directly when run_heavy");

    const insert = mutateNamedStep(workflow, "domain-unit", "Domain crate unit tests", (step) =>
      step.replace(attendance, `${attendance}          SQLX_OFFLINE=true cargo test --locked --manifest-path backend/Cargo.toml \\
            -p console-attendance-application --test phantom_binary
          check_status "phantom_binary"
`));
    expectFailure(
      insert,
      "domain-unit integration binary console-attendance-application --test phantom_binary must resolve exactly once through docs/program/executed-tests-baseline.json",
    );

    const reordered = mutateNamedStep(workflow, "domain-unit", "Domain crate unit tests", (step) =>
      step.replace(attendance, "<attendance-invocation>")
        .replace(compliance, attendance)
        .replace("<attendance-invocation>", compliance));
    expectFailure(reordered, "domain-unit must execute the locked Cargo test commands directly when run_heavy");

    const changedToken = mutateNamedStep(workflow, "domain-unit", "Domain crate unit tests", (step) =>
      step.replace("--test attendance_policy", "--test phantom_binary"));
    expectFailure(
      changedToken,
      "domain-unit integration binary console-attendance-application --test phantom_binary must resolve exactly once through docs/program/executed-tests-baseline.json",
    );

    const attendanceSource =
      "backend/crates/attendance/application/tests/attendance_policy.rs";
    const expectAttendanceBaselineFailure = (baselineDocument, mutation) => {
      const baselineFailures = evaluateCiPreflight(
        workflow,
        postgresWrapperBuildFile,
        freeRunnerDiskAction,
        baselineDocument,
      ).failures;
      assert.ok(
        baselineFailures.some((failure) => failure.includes(
          "domain-unit integration binary console-attendance-application --test attendance_policy must resolve exactly once through docs/program/executed-tests-baseline.json",
        )),
        `${mutation}:\n${baselineFailures.join("\n")}`,
      );
    };

    const missingBaselineEntry = structuredClone(executedTestsBaseline);
    delete missingBaselineEntry.test_attribute_baseline[attendanceSource];
    expectAttendanceBaselineFailure(missingBaselineEntry, "missing source");

    for (const alias of [
      "backend/crates/attendance/application/no_such_dir/../tests/attendance_policy.rs",
      "backend/crates/attendance/application//tests/attendance_policy.rs",
      "backend/crates/attendance/application/./tests/attendance_policy.rs",
      "backend/crates/attendance/Application/tests/attendance_policy.rs",
    ]) {
      const aliasedBaselineEntry = structuredClone(executedTestsBaseline);
      const count = aliasedBaselineEntry.test_attribute_baseline[attendanceSource];
      delete aliasedBaselineEntry.test_attribute_baseline[attendanceSource];
      aliasedBaselineEntry.test_attribute_baseline[alias] = count;
      expectAttendanceBaselineFailure(aliasedBaselineEntry, `noncanonical source ${alias}`);
    }

    const zeroTestBaselineEntry = structuredClone(executedTestsBaseline);
    zeroTestBaselineEntry.test_attribute_baseline[attendanceSource] = 0;
    expectAttendanceBaselineFailure(zeroTestBaselineEntry, "zero-test source");
  });

  it("preserves fail-fast backend ordering", () => {
    const sourceGateDisplaced = workflow
      .replace("      - name: Layer-boundary gate\n", "      - name: Displaced source gate\n")
      .replace("      - name: Reconcile portable PostgreSQL role topology\n", "      - name: Layer-boundary gate\n");
    expectFailure(sourceGateDisplaced, "backend must run source-only gates immediately after clippy");

    const unitAfterPostgres = workflow
      .replace("      - name: Cargo console-app unit suite\n", "      - name: Temporary Buck2 step\n")
      .replace("      - name: Cargo console-app inline PostgreSQL suite\n", "      - name: Cargo console-app unit suite\n");
    expectFailure(unitAfterPostgres, "backend must preserve the locked fail-fast step order");
  });

  it("locks the company-conformance proof and its execution semantics", () => {
    expectFailure(
      workflow.replace(
        "            //tools/buck:company-conformance-postgres",
        "            //tools/buck:unexpected-company-target",
      ),
      "company-conformance must preserve the locked fail-fast step multiset and failure semantics",
    );
    expectFailure(
      workflow.replace(
        "      - name: Company conformance against disposable PostgreSQL\n",
        "      - name: Company conformance against disposable PostgreSQL\n        continue-on-error: true\n",
      ),
      "company-conformance must preserve the locked fail-fast step multiset and failure semantics",
    );
    expectFailure(
      workflow.replace(
        "      - name: Company conformance against disposable PostgreSQL\n",
        "      - name: Company conformance against disposable PostgreSQL\n        shell: bash -c 'exit 0' {0}\n",
      ),
      "company-conformance may use only the default shell or canonical shell: bash",
    );
    expectFailure(
      workflow.replace(
        "      - name: Company conformance against disposable PostgreSQL\n",
        "      - name: Company conformance against disposable PostgreSQL\n        env:\n          BASH_ENV: scripts/noop.sh\n",
      ),
      "company-conformance must preserve its exact step environment allowlist",
    );
    expectFailure(
      workflow.replace(
        "  company-conformance:\n",
        "  company-conformance:\n    defaults:\n      run:\n        shell: bash -c 'exit 0' {0}\n",
      ),
      "company-conformance must preserve its exact job env/defaults execution metadata",
    );
  });

  it("locks the Kubernetes production-hardening proof", () => {
    expectFailure(
      workflow.replace(
        `      - name: Install production-hardening test dependencies\n        if: ${runHeavyUnlessCancelledIf}\n        run: npm ci --ignore-scripts\n\n`,
        "",
      ),
      "kubernetes-manifests must preserve all 8 ordered setup/proof run steps",
    );
    expectFailure(
      workflow.replace(
        "        run: npm ci --ignore-scripts\n",
        "        run: npm ci\n",
      ),
      "kubernetes-manifests setup run step 7 must preserve its exact name, command, condition, and execution semantics",
    );
    expectFailure(
      workflow.replace(
        `      - name: Production hardening contract\n        if: ${runHeavyUnlessCancelledIf}\n`,
        `      - name: Production hardening contract\n        if: ${runHeavyUnlessCancelledIf}\n        continue-on-error: true\n`,
      ),
      "kubernetes-manifests must preserve the locked fail-fast step multiset and failure semantics",
    );
    expectFailure(
      workflow.replace(
        "        run: npm run check:production-hardening\n",
        "        run: |\n          exit 0\n          npm run check:production-hardening\n",
      ),
      "kubernetes-manifests must preserve the locked fail-fast step multiset and failure semantics",
    );
  });

  it("fails closed when optimized gates or targets are commented, weakened, or duplicated", () => {
    expectFailure(
      workflow.replace(
        "        run: ../tools/buck2 run //backend/ci/gates/layer-boundary:console-gate-layer-boundary",
        "        # ../tools/buck2 run //backend/ci/gates/layer-boundary:console-gate-layer-boundary",
      ),
      "backend must preserve the locked fail-fast step multiset and failure semantics",
    );
    expectFailure(
      mutateNamedStep(workflow, "backend", "Audit-coverage gate", addFalseCondition),
      "backend must preserve the locked fail-fast step multiset and failure semantics",
    );
    expectFailure(
      mutateNamedStep(workflow, "backend", "Migration-safety gate", addContinueOnError),
      "backend must preserve the locked fail-fast step multiset and failure semantics",
    );
    expectFailure(
      replaceJob(workflow, "backend", (block) => block.replace(
        "      - name: Layer-boundary gate\n",
        "      - name: Layer-boundary gate\n        run: ../tools/buck2 run //backend/ci/gates/layer-boundary:console-gate-layer-boundary\n\n      - name: Layer-boundary gate\n",
      )),
      "backend must preserve the locked fail-fast step multiset and failure semantics",
    );
    expectFailure(
      workflow.replace(
        "        run: env -u DATABASE_URL cargo test --locked --manifest-path backend/Cargo.toml -p console-app --lib",
        "        # env -u DATABASE_URL cargo test --locked --manifest-path backend/Cargo.toml -p console-app --lib",
      ),
      "backend must preserve the locked fail-fast step multiset and failure semantics",
    );
  });

  it("keeps protected backend steps fail-slow and runs PR 473 contract tests before topology", () => {
    expectFailure(
      workflow.replace(
        `      - name: rustfmt check\n        id: fmt\n        if: ${backendCargoLegIf}\n`,
        `      - name: rustfmt check\n        id: fmt\n        if: \${{ !cancelled() }}\n`,
      ),
      "backend proof run step 3 must preserve",
    );
    expectFailure(
      workflow.replace(
        "        run: python3 scripts/check-pr473-migration-operational.test.py -v",
        "        # python3 scripts/check-pr473-migration-operational.test.py -v",
      ),
      "backend must preserve the locked fail-fast step multiset and failure semantics",
    );
    const pr473ContractAfterTopology = workflow
      .replace("      - name: PR 473 migration operational contract tests\n", "      - name: Deferred PR 473 contract tests\n")
      .replace("      - name: Reconcile portable PostgreSQL role topology\n", "      - name: PR 473 migration operational contract tests\n");
    expectFailure(pr473ContractAfterTopology, "backend must preserve the locked fail-fast step order");
  });

  it("locks the documentation manifest gate and its doc-test to doc-link position", () => {
    const step = "      - name: Documentation manifest gate\n"
      + "        if: ${{ !cancelled() }}\n"
      + "        run: npm run check:doc-manifest\n";
    const linkTests = "      - name: Documentation link tests\n"
      + "        if: ${{ !cancelled() }}\n"
      + "        run: node --test scripts/check-doc-links.test.mjs\n";
    const localLink = "      - name: Documentation local-link gate\n"
      + "        if: ${{ !cancelled() }}\n"
      + "        run: npm run check:doc-links\n";
    assert.equal(
      workflow.split(step).length - 1,
      1,
      "repo-gates must run exactly one documentation manifest gate",
    );
    assert.ok(
      workflow.includes(`${linkTests}${step}${localLink}`),
      "documentation gates must preserve doc-test -> manifest -> doc-link order",
    );

    expectFailure(
      workflow.replace(step, ""),
      "repo-gates must preserve all",
    );
    expectFailure(
      mutateNamedStep(workflow, "repo-gates", "Documentation manifest gate", (manifestStep) => (
        manifestStep.replace(
          "        run: npm run check:doc-manifest\n",
          "        # run: npm run check:doc-manifest\n",
        )
      )),
      "repo-gates must preserve all",
    );
    for (const condition of [
      "        if: false\n",
      "        if: ${{ success() }}\n",
      "",
    ]) {
      expectFailure(
        workflow.replace("        if: ${{ !cancelled() }}\n        run: npm run check:doc-manifest\n", `${condition}        run: npm run check:doc-manifest\n`),
        "repo-gates proof run step",
      );
    }
    expectFailure(
      workflow.replace(
        "        if: ${{ !cancelled() }}\n        run: npm run check:doc-manifest\n",
        "        if: ${{ !cancelled() }}\n        continue-on-error: true\n        run: npm run check:doc-manifest\n",
      ),
      "repo-gates proof run step",
    );
    expectFailure(
      workflow.replace(
        "      - name: Documentation manifest gate\n",
        "      - name: Documentation manifest check\n",
      ),
      "repo-gates proof run step",
    );
    expectFailure(
      workflow.replace(step, `${step}${step}`),
      "repo-gates must contain only its locked ordered action, setup, proof, and cleanup steps",
    );

    for (const reordered of [
      workflow.replace(`${linkTests}${step}`, `${step}${linkTests}`),
      workflow.replace(`${step}${localLink}`, `${localLink}${step}`),
    ]) {
      expectFailure(
        reordered,
        "repo-gates proof run step",
      );
    }
  });
  // repo-gates steps are otherwise unlocked: deleting `run: npm run check:adrs` from it today
  // yields zero preflight failures. Wiring a gate into ci.yml is not the same as protecting it,
  // and an unprotected step is a slot in the job list that reads as coverage.
  it("locks the undeclared-imports gate step in repo-gates", () => {
    const step = "      - name: Undeclared imports — every bare specifier must be declared\n"
      + `        if: ${runHeavyUnlessCancelledIf}\n`
      + "        run: npm run check:undeclared-imports\n";
    assert.ok(workflow.includes(step), "repo-gates does not run the undeclared-imports gate");

    expectFailure(
      workflow.replace(step, ""),
      "repo-gates must preserve the locked fail-fast step multiset and failure semantics",
    );
  });

  // Wired by 4e7da6b52 and unprotected until this lock: with the step present, deleting it
  // returned zero preflight failures. Being wired into ci.yml is not the same as being protected.
  it("locks the request-body-contract gate step in repo-gates", () => {
    const step = "      - name: Request-body contract — spec fields must exist on the handler\n"
      + `        if: ${runHeavyUnlessCancelledIf}\n`
      + "        run: npm run check:request-body-contract\n";
    assert.ok(workflow.includes(step), "repo-gates does not run the request-body-contract gate");

    expectFailure(
      workflow.replace(step, ""),
      "repo-gates must preserve the locked fail-fast step multiset and failure semantics",
    );
    // The order matters as much as the presence: this gate must not be moved above the cheap
    // undeclared-imports scan that fails in under a second.
    expectFailure(
      workflow.replace(step, "").replace(
        "      - name: Undeclared imports — every bare specifier must be declared\n",
        `${step}      - name: Undeclared imports — every bare specifier must be declared\n`,
      ),
      "repo-gates must preserve the locked fail-fast step order",
    );
  });

  // The suite H-1 is about. `openapi_drift` is the only thing inventorying every mounted route
  // against openapi.yaml, and it was unprotected: deleting this `run:` line left check:ci-preflight,
  // check:foundation-gates and check:doc-citations all exiting 0. check:request-body-contract closed
  // H-1's request-body half but reads no route inventory, so nothing else covers what this covers.
  it("locks the console-app OpenAPI drift suite step in backend", () => {
    const run = "        run: env -u DATABASE_URL cargo test --locked --manifest-path backend/Cargo.toml -p console-app --test openapi_drift\n";
    const step = "      - name: Cargo console-app OpenAPI drift suite\n"
      + "        id: openapi-drift\n"
      + `        if: ${backendBuckAppLegIf}\n`
      + "        working-directory: .\n"
      + run;
    assert.ok(workflow.includes(step), "backend does not run the openapi_drift suite");

    // The exact deletion this lock exists to refuse: the step keeps its slot, runs nothing.
    expectFailure(
      workflow.replace(run, ""),
      "backend must preserve the locked fail-fast step multiset and failure semantics",
    );
    // Quieter, and the reason `run` is pinned rather than just the name: the step still reads as
    // the drift suite in the job list while executing a target that inventories no routes.
    expectFailure(
      workflow.replace(run, "        run: env -u DATABASE_URL cargo test --locked --manifest-path backend/Cargo.toml -p console-app --lib\n"),
      "backend must preserve the locked fail-fast step multiset and failure semantics",
    );
    // Dropping the run_heavy half of the guard would let the drift suite run on thin classes.
    expectFailure(
      workflow.replace(
        `      - name: Cargo console-app OpenAPI drift suite\n        id: openapi-drift\n        if: ${backendBuckAppLegIf}\n`,
        `      - name: Cargo console-app OpenAPI drift suite\n        id: openapi-drift\n        if: \${{ !cancelled() }}\n`,
      ),
      "backend must preserve the locked fail-fast step multiset and failure semantics",
    );
  });

  it("locks app PostgreSQL source features and bootstrap identity in independent steps", () => {
    for (const [name, feature] of [
      ["Cargo console-app inline PostgreSQL suite", "test-postgres"],
      ["Cargo console-app dev-auth PostgreSQL suite", "dev-auth"],
    ]) {
      for (const mutate of [
        (step) => step.replace(` --features ${feature}`, ""),
        (step) => step.replace('          export DATABASE_URL="${CONSOLE_BUCK_ADMIN_DATABASE_URL:?missing test bootstrap URL}"\n', ""),
        (step) => step.replace("steps.topology.outcome == 'success' && ", ""),
        (step) => step.replace(" -- --test-threads=1", " -- --test-threads=1 || true"),
      ]) {
        expectFailure(
          mutateNamedStep(workflow, "backend", name, mutate),
          "backend must preserve the locked fail-fast step multiset and failure semantics",
        );
      }
    }
  });

});
