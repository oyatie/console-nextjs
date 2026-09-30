#!/usr/bin/env node
/**
 * Detector for js-test-reachability.dark-suite.
 *
 * Wraps scripts/dev/mjs-test-reachability.mjs logic against the current tree
 * (not only origin/main) so ProcessHealth / ProductBuild can fail closed on
 * *growth* of dark suites. Existing dark suites are listed; --check fails only
 * when dark suites exist under tools/ci/** or when --strict is passed.
 *
 * Usage:
 *   node tools/ci/check-mjs-dark-suites.mjs           # report JSON; exit 0
 *   node tools/ci/check-mjs-dark-suites.mjs --strict # exit 2 if any dark suite
 *   node tools/ci/check-mjs-dark-suites.mjs --check  # exit 2 if tools/ci dark
 */
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { sweepReachableText } from "./gate-sweep.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const args = new Set(process.argv.slice(2));
const strict = args.has("--strict");
const check = args.has("--check");

const SKIP_DIRS = new Set([
  "node_modules",
  "target",
  "buck-out",
  ".git",
  ".claude",
  ".worktrees",
  "third-party",
]);

function listTestMjs(dir, baseRoot, acc = []) {
  // Prefer git inventory so worktrees / scratch clones do not inflate dark counts.
  // Include untracked (e.g. new tools/ci tests) via --others --exclude-standard.
  try {
    const gitOpts = {
      encoding: "utf8",
      maxBuffer: 8 * 1024 * 1024,
      stdio: ["ignore", "pipe", "pipe"],
    };
    const tracked = execFileSync(
      "git",
      ["-C", baseRoot, "ls-files", "*.test.mjs"],
      gitOpts,
    );
    const others = execFileSync(
      "git",
      ["-C", baseRoot, "ls-files", "--others", "--exclude-standard", "*.test.mjs"],
      gitOpts,
    );
    const paths = `${tracked}\n${others}`
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean)
      .filter((p) => !p.startsWith(".claude/") && !p.startsWith(".worktrees/"));
    if (paths.length) return [...new Set(paths)].sort();
  } catch {
    /* fall through to filesystem walk (fixtures / non-git) */
  }

  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return acc;
  }
  for (const name of entries) {
    if (SKIP_DIRS.has(name)) continue;
    const full = join(dir, name);
    let st;
    try {
      st = statSync(full);
    } catch {
      continue;
    }
    if (st.isDirectory()) listTestMjs(full, baseRoot, acc);
    else if (name.endsWith(".test.mjs")) {
      acc.push(relative(baseRoot, full).split("\\").join("/"));
    }
  }
  return acc;
}

export function resolveDarkSuites(repoRoot = root) {
  const pkg = JSON.parse(readFileSync(join(repoRoot, "package.json"), "utf8"));
  const scripts = pkg.scripts ?? {};

  // Workflow run text (best-effort; missing dir → empty)
  let runText = "";
  const wfDir = join(repoRoot, ".github/workflows");
  try {
    for (const f of readdirSync(wfDir)) {
      if (!/\.ya?ml$/i.test(f)) continue;
      runText += readFileSync(join(wfDir, f), "utf8") + "\n";
    }
  } catch {
    /* no workflows */
  }

  const invoked = new Set();
  for (const name of Object.keys(scripts)) {
    const re = new RegExp(
      `npm\\s+(?:run\\s+)?${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?![\\w:-])`,
    );
    if (re.test(runText)) invoked.add(name);
  }
  for (let changed = true; changed; ) {
    changed = false;
    for (const name of [...invoked]) {
      for (const other of Object.keys(scripts)) {
        if (invoked.has(other)) continue;
        const re = new RegExp(
          `npm\\s+(?:run\\s+)?${other.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?![\\w:-])`,
        );
        if (re.test(scripts[name] || "")) {
          invoked.add(other);
          changed = true;
        }
      }
    }
  }

  // tools/ci/gate-sweep.json is a real execution path, so it is a real
  // reachability source. `check:ci-preflight` used to be an `&&` chain that
  // named its test files inline in package.json, which is where this scan
  // looks; the chain became a fail-slow sweep and moved those commands into a
  // manifest. Without reading it, 20 suites that DO run every CI cycle report
  // as dark -- and the natural "fix" is to baseline them, which would retire
  // live coverage on the strength of a scanner blind spot.
  const sweepText = sweepReachableText(repoRoot);
  const expanded = runText + [...invoked].map((n) => scripts[n]).join("\n") + sweepText;
  const suites = listTestMjs(repoRoot, repoRoot);
  const dark = [];
  const wired = [];
  for (const suite of suites) {
    const base = suite.split("/").pop();
    const direct = expanded.includes(suite);
    const byBase = expanded.includes(base);
    const owner = Object.entries(scripts).find(
      ([, value]) => value.includes(suite) || value.includes(base),
    );
    if (direct || byBase) wired.push(suite);
    else {
      dark.push({
        suite,
        npm_script: owner ? owner[0] : null,
        under_tools_ci: suite.startsWith("tools/ci/"),
      });
    }
  }

  const tools_ci_orphan = dark.filter((d) => d.under_tools_ci && !d.npm_script);

  return {
    class_id: "js-test-reachability.dark-suite",
    suite_count: suites.length,
    wired_count: wired.length,
    dark_count: dark.length,
    dark,
    wired,
    tools_ci_orphan,
    npm_scripts_reachable_from_workflow: [...invoked].sort(),
  };
}

function main() {
  const report = resolveDarkSuites(root);
  console.log(JSON.stringify({ ok: true, ...report }, null, 2));

  if (strict && report.dark_count > 0) {
    console.error(
      JSON.stringify(
        {
          class_id: report.class_id,
          message: "js-test-reachability.dark-suite: dark suites remain (strict)",
          dark_count: report.dark_count,
        },
        null,
        2,
      ),
    );
    process.exit(2);
  }
  // Process gate: tools/ci tests must at least have an npm script owner (test:ci-tools).
  // Full CI wiring is product/process work; orphans are process defects.
  if (check && report.tools_ci_orphan.length > 0) {
    console.error(
      JSON.stringify(
        {
          class_id: report.class_id,
          message: "js-test-reachability.dark-suite: tools/ci tests need npm script owner",
          tools_ci_orphan: report.tools_ci_orphan,
        },
        null,
        2,
      ),
    );
    process.exit(2);
  }
}

if (
  import.meta.url === `file://${process.argv[1]}` ||
  process.argv[1]?.endsWith("check-mjs-dark-suites.mjs")
) {
  main();
}
