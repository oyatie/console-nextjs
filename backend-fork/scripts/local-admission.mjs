#!/usr/bin/env node
/**
 * Local admission before push / PR open.
 *
 * Process gap this closes (ops.skip-admit): agents opened tip PRs while
 * product was fine but hosted Required stayed red for days of wall time
 * on product bugs that would have failed `cargo test -p … --lib` in seconds,
 * OR thrash-classified Actions outages as product reds.
 *
 * Tier A = contracts (always when tip/contract paths change; always safe).
 * Tier B = path-aware pure cargo tests for touched domain crates.
 * Does NOT run disposable PG facets (hosted only).
 *
 * Usage:
 *   node scripts/local-admission.mjs
 *   node scripts/local-admission.mjs --base origin/main
 *   node scripts/local-admission.mjs --json
 *   SKIP_LOCAL_ADMISSION=1 node scripts/local-admission.mjs   # escape hatch
 */
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const ROOT = resolve(process.cwd());

/** @typedef {{ id: string, cmd: string[], cwd?: string, when: string }} Gate */

export function parseArgs(argv = process.argv.slice(2)) {
  let base = "origin/main";
  let json = false;
  let dryRun = false;
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === "--base") {
      base = argv[++i];
      if (!base) throw new Error("--base requires a revision");
    } else if (a === "--json") {
      json = true;
    } else if (a === "--dry-run") {
      dryRun = true;
    } else if (a === "--help" || a === "-h") {
      return { help: true, base, json, dryRun };
    } else {
      throw new Error(`unknown argument: ${a}`);
    }
  }
  return { help: false, base, json, dryRun };
}

export function gitChangedFiles(base, root = ROOT) {
  try {
    execFileSync("git", ["rev-parse", "--verify", base], {
      cwd: root,
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch {
    throw new Error(`admission base ${JSON.stringify(base)} is not a valid revision`);
  }
  const out = execFileSync("git", ["diff", "--name-only", `${base}...HEAD`], {
    cwd: root,
    encoding: "utf8",
  });
  return out
    .split("\n")
    .map((s) => s.trim())
    .filter(Boolean);
}

export function classifyPaths(files) {
  const flags = {
    ci: false,
    scripts: false,
    toolsCi: false,
    ledger: false,
    docsIndex: false,
    baseline: false,
    rustDomainLib: [],
    rustAny: false,
  };
  for (const f of files) {
    if (f.startsWith(".github/workflows/") || f === "scripts/check-ci-preflight.mjs") {
      flags.ci = true;
    }
    if (f.startsWith("scripts/")) flags.scripts = true;
    if (f.startsWith("tools/ci/")) flags.toolsCi = true;
    if (f.startsWith("docs/program/ledger/") || f === "docs/program/console-program-ledger.md") {
      flags.ledger = true;
    }
    if (
      f === "docs/documentation-index.json" ||
      f === "docs/documentation-manifest.seed.json"
    ) {
      flags.docsIndex = true;
    }
    if (f === "docs/program/executed-tests-baseline.json") flags.baseline = true;
    if (f.endsWith(".rs") && f.startsWith("backend/")) flags.rustAny = true;
    const m = f.match(
      /^backend\/crates\/([^/]+(?:\/[^/]+)*)\/domain\/src\/lib\.rs$/,
    );
    if (m) flags.rustDomainLib.push(f);
  }
  return flags;
}

export function cargoPackageNameFromDomainLib(libPath, root = ROOT) {
  const cargoToml = join(root, dirname(libPath), "..", "Cargo.toml");
  // domain/src/lib.rs → domain/Cargo.toml
  const domainToml = join(root, dirname(libPath), "..", "Cargo.toml");
  const path = existsSync(domainToml)
    ? domainToml
    : join(root, dirname(libPath), "Cargo.toml");
  const resolved = existsSync(path) ? path : cargoToml;
  if (!existsSync(resolved)) return null;
  const text = readFileSync(resolved, "utf8");
  const m = text.match(/^\s*name\s*=\s*"([^"]+)"/m);
  return m ? m[1] : null;
}

/**
 * Build ordered gates. Collect-all: callers run all and report every failure.
 * @param {string[]} files
 * @param {{ root?: string }} [opts]
 * @returns {Gate[]}
 */
export function planGates(files, opts = {}) {
  const root = opts.root ?? ROOT;
  const flags = classifyPaths(files);
  /** @type {Gate[]} */
  const gates = [];

  // Always: tip-serial contention report (non-blocking unless --check elsewhere)
  // Always-light: nothing zero-cost forced.

  if (flags.ci || flags.scripts || flags.toolsCi) {
    gates.push({
      id: "check:js-test-reachability",
      cmd: ["node", "scripts/check-js-test-reachability.mjs"],
      when: "ci/scripts/tools.ci changed — dark .test.mjs must be exact-wired",
    });
    gates.push({
      id: "check:ci-preflight",
      cmd: ["node", "scripts/check-ci-preflight.mjs"],
      when: "ci/scripts/tools.ci changed",
    });
    gates.push({
      id: "test:verify",
      cmd: ["node", "--test", "scripts/verify.test.mjs"],
      when: "ci/scripts/tools.ci changed",
    });
    // Process closed-loop unit tests (ops.skip-admit / false-green prevention for tooling).
    if (existsSync(join(root, "scripts/local-admission.test.mjs"))) {
      gates.push({
        id: "test:local-admission",
        cmd: ["node", "--test", "scripts/local-admission.test.mjs"],
        when: "scripts/tools.ci changed",
      });
    }
    if (existsSync(join(root, "tools/ci/assess-tip-contention.test.mjs"))) {
      gates.push({
        id: "test:ci-tools",
        cmd: [
          "node",
          "--test",
          "tools/ci/assess-tip-contention.test.mjs",
          "tools/ci/check-mjs-dark-suites.test.mjs",
          "tools/ci/classify-ci-failure.test.mjs",
        ],
        when: "tools.ci changed",
      });
    }
  }

  // AGENTS.md is the source; CLAUDE.md and README.md each carry a projection of
  // it and each says so. Any of the three can cause drift. The retired gate
  // keyed off ledger/docs-index/baseline changes, none of which can move it.
  const lensInputs = ["AGENTS.md", "CLAUDE.md", "README.md"];
  const lensImpl = [
    "scripts/check-reasoning-lens-manifest.mjs",
    "scripts/check-reasoning-lens-manifest.test.mjs",
  ];
  if (files.some((path) => lensInputs.includes(path) || lensImpl.includes(path))) {
    // Editing the checker itself has to run it. Otherwise a weakened parser
    // passes local admission and fails only in hosted preflight -- and the
    // regression suite is the only thing that proves the parser still rejects.
    if (files.some((path) => lensImpl.includes(path))) {
      gates.push({
        id: "test:reasoning-lens-manifest",
        cmd: ["node", "--test", "scripts/check-reasoning-lens-manifest.test.mjs"],
        when: "the manifest checker or its test changed",
      });
    }
    gates.push({
      id: "check:reasoning-lens-manifest",
      cmd: ["node", "scripts/check-reasoning-lens-manifest.mjs"],
      when: "AGENTS.md, CLAUDE.md, README.md, or the checker changed",
    });
  }

  if (flags.docsIndex || flags.ledger) {
    // doc-links is the durable custody surface when available
    if (existsSync(join(root, "scripts/check-doc-links.mjs"))) {
      gates.push({
        id: "check:doc-links",
        cmd: ["node", "scripts/check-doc-links.mjs"],
        when: "docs index/ledger changed",
      });
    }
  }

  if (flags.baseline) {
    gates.push({
      id: "check:executed-tests",
      cmd: ["node", "scripts/check-executed-tests.mjs"],
      when: "executed-tests-baseline changed",
    });
  }

  // Domain pure-test fan-out: every touched domain lib gets cargo test --lib
  const pkgs = new Set();
  for (const lib of flags.rustDomainLib) {
    const name = cargoPackageNameFromDomainLib(lib, root);
    if (name) pkgs.add(name);
  }
  for (const pkg of [...pkgs].sort()) {
    gates.push({
      id: `cargo-test:${pkg}`,
      cmd: [
        "cargo",
        "test",
        "--locked",
        "--manifest-path",
        "backend/Cargo.toml",
        "-p",
        pkg,
        "--lib",
        "--quiet",
      ],
      when: `domain lib for ${pkg} changed`,
    });
  }

  // Tip authority trains always include ledger → lens already. If only rust
  // domain without ledger (local pure work before tip bind), still run cargo.

  return gates;
}

function runGate(gate, root = ROOT) {
  const started = Date.now();
  const result = spawnSync(gate.cmd[0], gate.cmd.slice(1), {
    cwd: gate.cwd ?? root,
    encoding: "utf8",
    env: process.env,
  });
  const ms = Date.now() - started;
  return {
    id: gate.id,
    when: gate.when,
    cmd: gate.cmd.join(" "),
    ok: result.status === 0,
    status: result.status,
    ms,
    stdout: (result.stdout || "").slice(-4000),
    stderr: (result.stderr || "").slice(-4000),
  };
}

export function main(argv = process.argv.slice(2)) {
  if (process.env.SKIP_LOCAL_ADMISSION === "1") {
    const msg = "SKIP_LOCAL_ADMISSION=1 — local admission bypassed";
    console.error(msg);
    return 0;
  }

  let options;
  try {
    options = parseArgs(argv);
  } catch (err) {
    console.error(String(err?.message || err));
    return 2;
  }
  if (options.help) {
    console.log(`Usage: node scripts/local-admission.mjs [--base origin/main] [--json] [--dry-run]
Escape: SKIP_LOCAL_ADMISSION=1
`);
    return 0;
  }

  let files;
  try {
    files = gitChangedFiles(options.base, ROOT);
  } catch (err) {
    console.error(String(err?.message || err));
    return 2;
  }

  if (files.length === 0) {
    const payload = {
      ok: true,
      base: options.base,
      files: [],
      gates: [],
      note: "no files changed vs base — nothing to admit",
    };
    if (options.json) console.log(JSON.stringify(payload, null, 2));
    else console.log(payload.note);
    return 0;
  }

  const gates = planGates(files, { root: ROOT });
  if (gates.length === 0) {
    const payload = {
      ok: true,
      base: options.base,
      files,
      gates: [],
      note: "changed paths do not match admission tiers A/B (hosted-only surface?)",
    };
    if (options.json) console.log(JSON.stringify(payload, null, 2));
    else {
      console.log(`admit: ${files.length} files; no local gates selected`);
      for (const f of files) console.log(`  - ${f}`);
    }
    return 0;
  }

  if (options.dryRun) {
    const payload = { ok: true, dryRun: true, base: options.base, files, gates };
    if (options.json) console.log(JSON.stringify(payload, null, 2));
    else {
      console.log(`admit dry-run base=${options.base} files=${files.length} gates=${gates.length}`);
      for (const g of gates) console.log(`  [${g.id}] ${g.cmd.join(" ")}  (${g.when})`);
    }
    return 0;
  }

  const results = [];
  for (const gate of gates) {
    process.stderr.write(`admit: run ${gate.id}…\n`);
    results.push(runGate(gate, ROOT));
  }

  const failed = results.filter((r) => !r.ok);
  const payload = {
    ok: failed.length === 0,
    base: options.base,
    files,
    results: results.map((r) => ({
      id: r.id,
      ok: r.ok,
      status: r.status,
      ms: r.ms,
      cmd: r.cmd,
      when: r.when,
    })),
    failures: failed.map((r) => ({
      id: r.id,
      status: r.status,
      cmd: r.cmd,
      stderr_tail: r.stderr,
      stdout_tail: r.stdout,
    })),
  };

  if (options.json) {
    console.log(JSON.stringify(payload, null, 2));
  } else {
    console.log(
      `admit: base=${options.base} files=${files.length} gates=${results.length} failed=${failed.length}`,
    );
    for (const r of results) {
      console.log(`  ${r.ok ? "OK  " : "FAIL"} ${r.id} (${r.ms}ms)`);
    }
    if (failed.length) {
      console.error("\nadmit FAILED — fix before push/PR (ops.skip-admit):");
      for (const f of failed) {
        console.error(`\n## ${f.id}\n$ ${f.cmd}`);
        if (f.stderr) console.error(f.stderr);
        if (f.stdout) console.error(f.stdout);
      }
    } else {
      console.log("admit OK — safe to push/open PR (hosted still required for PG/full matrix)");
    }
  }

  return failed.length === 0 ? 0 : 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  process.exit(main());
}
