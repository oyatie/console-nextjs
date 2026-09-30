#!/usr/bin/env node
// A database password must never reach a process argument list or a workflow log.
//
// THREAT MODEL
//
// Two ways a test credential escapes, both cheap and both silent:
//
//   1. argv. Every process on the host can read /proc/<pid>/cmdline (Linux) or
//      run `ps -ww` (macOS) for as long as the test runs. CI runners are shared
//      within a job and dev machines are shared with whatever else is running.
//   2. the workflow log. GitHub Actions masks REGISTERED secrets. A connection
//      URL assembled inline in a `run:` step is not one, so it is masked
//      nowhere; anything that echoes the command, or any tool that prints its
//      own argv on error, publishes it to a log that outlives the run.
//
// WHAT THIS IS AND IS NOT
//
// Under Buck2 the control is STRUCTURAL: tools/buck/test_needs_postgres.sh:26
// exits 2 on a raw //backend/... target, so a PostgreSQL test CANNOT be run
// except through a wrapper that passes the credential as a mode-0600 file path.
// Every PostgreSQL test in ci.yml goes through it because there is no other way
// in.
//
// What is checked here is NOT the same property, and saying so would be the
// interesting kind of wrong. It is two weaker halves:
//
//   STATIC   no test-runner invocation in any workflow may carry a credential.
//            This one does cover every workflow line, because it reads them all.
//   RUNTIME  tools/lanes/no-credential-in-argv.sh exits 2 rather than let a
//            command carrying one run. This one is OPT-IN: it fires only for a
//            command routed through tools/lanes/pgtest.sh, which today no
//            workflow uses at all — it is the local harness. Nothing forces a
//            `cargo test` through it, so after the Buck2 exit the structural
//            refusal is gone and the static half is what remains.
//
// NOT COVERED, stated rather than left to be discovered: a credential a test
// builds at runtime from environment parts; a credential in a script that ci.yml
// invokes rather than spelling out (the static half reads workflow YAML, not the
// scripts it calls); and anything on a line that names no test runner.
//
// The runtime half is EXECUTED here, in both directions, rather than asserted
// about. A guard that refuses everything and a guard that refuses nothing look
// identical from the outside if you only test one direction.
//
// Deliberately NOT scanned: shell scripts that build a URL and `export` it.
// tools/lanes/pgtest.sh and tools/buck/test_needs_postgres.sh do exactly that,
// and putting a credential in the environment is the fix, not the defect.

import { spawnSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import yaml from "js-yaml";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const GUARD = "tools/lanes/no-credential-in-argv.sh";
const HARNESS = "tools/lanes/pgtest.sh";

// A URI carrying a userinfo password. Anchored so the `:pw@` must sit in the
// authority: `https://host:8080/a@b` has a `/` before the `@` and is not a
// credential.
// libpq also accepts an empty user (`postgresql://:pw@host/db`), so the user
// part is `*`; the password remains `+` because an empty password is not a leak.
const CREDENTIAL_URI = /:\/\/[^/@\s"']*:[^/@\s"']+@/;
// `password=<literal>`, case-insensitive, which is three forms at once: the
// `PGPASSWORD=`/`POSTGRES_PASSWORD=` env spelling, libpq's keyword/value DSN
// (`host=… password=hunter2`), and the URI query parameter
// (`postgres://u@h/db?password=hunter2`). The uppercase-only match this replaced
// accepted the latter two.
//
// Quoting does not make a committed literal safe: `password='hunter2'` still
// becomes a password argument. An exact shell/GitHub variable reference is the
// one static exemption, so `PGPASSWORD="$X" cargo test` remains the encouraged
// environment-prefix idiom. This half checks committed literals; the runtime
// guard below sees expanded values for commands that opt into the harness.
//
// The runtime guard deliberately has no such exemption. By the time it sees argv
// the shell has already expanded `$X`, so what it is looking at really is the
// secret.
const PASSWORD_ASSIGNMENT = /password\s*=\s*(?:"(?:[^"\\]|\\.)*"|'[^']*'|[^\s"'\\]+)/gi;
const VARIABLE_VALUE = /^\$(?:[A-Za-z_][A-Za-z0-9_]*|\{[A-Za-z_][A-Za-z0-9_]*\}|\{\{\s*[^}]+\s*\}\})$/;
// The things that run tests. A credential on any other line is someone
// provisioning a database, which is a different job with different rules.
const TEST_RUNNERS = [
  // Cargo accepts toolchain selectors and global flags before its subcommand.
  // Keep this deliberately broader than a complete Cargo option parser so a
  // new harmless flag cannot make a credential-bearing test line invisible.
  /\bcargo\b(?=[^;&|\n]*\b(?:test|nextest\s+run)\b)/,
  // Buck2 likewise accepts universal flags (and their values) before `test`,
  // including --isolation-dir and -v. The command still has to reach `test`
  // before a shell separator so a flag cannot hide a credential-bearing run.
  /\bbuck2\b(?=[^;&|\n]*\btest\b)/,
  /test_needs_postgres\.sh\b/,
  /cargo_needs_postgres\.sh\b/,
  /pgtest\.sh\b/,
];

function literalPasswordAssignment(text) {
  for (const match of text.matchAll(PASSWORD_ASSIGNMENT)) {
    let value = match[0].slice(match[0].indexOf("=") + 1).trim();
    if ((value.startsWith('"') && value.endsWith('"'))
      || (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(1, -1);
    }
    if (!VARIABLE_VALUE.test(value)) return true;
  }
  return false;
}

/** Every shell command in every workflow `run:` scalar. */
function workflowCommandLines(root) {
  const dir = join(root, ".github/workflows");
  const lines = [];
  for (const file of readdirSync(dir).filter((name) => /\.ya?ml$/.test(name))) {
    const text = readFileSync(join(dir, file), "utf8");
    const doc = yaml.load(text);
    for (const [jobName, job] of Object.entries(doc?.jobs ?? {})) {
      for (const [stepIndex, step] of (job?.steps ?? []).entries()) {
        if (typeof step?.run !== "string") continue;
        // Parse YAML first. A folded scalar (`>-`) is one space-joined shell
        // command even though its runner and credential may occupy different
        // physical YAML lines. Literal scalars retain command-separating
        // newlines, while shell continuations are joined explicitly.
        const joined = step.run.replace(/\\\s*\n\s*/g, " ");
        joined.split(/\r?\n/).forEach((command) => {
          if (!command.trim() || /^\s*#/.test(command)) return;
          lines.push({
            file,
            location: step.name ?? `${jobName} step ${stepIndex + 1}`,
            text: command,
          });
        });
      }
    }
  }
  return lines;
}

export function staticFindings(root = ROOT) {
  const findings = [];
  for (const entry of workflowCommandLines(root)) {
    if (!TEST_RUNNERS.some((runner) => runner.test(entry.text))) continue;
    if (CREDENTIAL_URI.test(entry.text)) {
      findings.push(`${entry.file}:${entry.location} passes a connection URL with a password to a test runner; export it into the environment instead`);
    }
    if (literalPasswordAssignment(entry.text)) {
      findings.push(`${entry.file}:${entry.location} puts a password on a test runner's command line; export it into the environment instead`);
    }
  }
  return findings;
}

/**
 * The runtime half, proven by execution, in both directions.
 *
 * The guard is exec'd DIRECTLY. It used to be reached by running pgtest.sh with
 * CONSOLE_PGTEST_CHECK_ARGV_ONLY set, which made the harness itself carry a
 * zero-output exit-0 bypass: one line in a job's `env:` and every PostgreSQL
 * lane goes green having executed nothing. The guard is its own file precisely
 * so that testing it costs the harness no bypass.
 */
export function runtimeFindings(root = ROOT) {
  const findings = [];
  const run = (...args) => spawnSync("bash", [join(root, GUARD), ...args], { cwd: root, encoding: "utf8" });

  // One per escape shape, plus whitespace variants accepted by libpq. A gate
  // that knew only the first two accepted `password=hunter2`, `password =
  // hunter2`, and `…/db?password=hunter2`.
  const poisoned = [
    "postgres://console_app:hunter2@127.0.0.1:5432/db",
    "postgresql://:hunter2@127.0.0.1:5432/db",
    "PGPASSWORD=hunter2",
    "PGPASSWORD = hunter2",
    "host=127.0.0.1 dbname=db password=hunter2",
    "host=127.0.0.1 dbname=db password = hunter2",
    "postgres://console_app@127.0.0.1:5432/db?password=hunter2",
  ];
  for (const arg of poisoned) {
    const result = run("env", arg, "cargo", "test");
    if (result.status !== 2) {
      findings.push(`${GUARD} accepted a credential in argv (exit ${result.status}): ${arg}`);
    }
  }
  const clean = run("cargo", "test", "-p", "console-app", "--test", "config");
  if (clean.status !== 0) {
    findings.push(`${GUARD} refused a clean command line (exit ${clean.status}); a guard that refuses everything protects nothing`);
  }

  // A guard nothing calls is decoration. The one caller is asserted here rather
  // than assumed, because the guard living in its own file is exactly what makes
  // "it is no longer wired" a silent, one-line change.
  const harness = readFileSync(join(root, HARNESS), "utf8");
  if (!/^\s*source\s+.*no-credential-in-argv\.sh"?\s+"\$@"/m.test(harness)) {
    findings.push(`${HARNESS} no longer sources ${GUARD} with "$@"; the guard runs for nothing`);
  }
  if (/CONSOLE_PGTEST_CHECK_ARGV_ONLY/.test(harness)) {
    findings.push(`${HARNESS} has regained a check-only bypass; one line in a job's env: turns every PostgreSQL lane green having run nothing`);
  }
  return findings;
}

function main() {
  const findings = [...staticFindings(), ...runtimeFindings()];
  if (findings.length > 0) {
    console.error("Test-credential contract failed:");
    for (const finding of findings) console.error(`- ${finding}`);
    return 1;
  }
  console.log(`Test-credential contract passed: no workflow line spelling a test runner carries a literal password, and ${GUARD} refuses all seven exercised libpq password spellings while accepting a clean command line.`);
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) process.exit(main());
