#!/usr/bin/env node
/**
 * Mechanically evaluate the projection fan-out HOLD in docs/current/PRODUCT.md.
 *
 * The hold reads:
 *
 *   "Company, Person, Employment, and PayRun projection fan-out is HOLD until
 *    each has an explicit owning port and a proven single-writer boundary."
 *
 * Every clause of that is already enforced somewhere, and enforced WELL:
 * `canonical_contract.rs` proves each key names an owner crate and at least one
 * table it alone may write, `gate_detects_violation.rs` derives its owned-table
 * set from `ObjectKey::ALL` and proves the static gate catches a second writer
 * (including deliberately misspelled evasions), and
 * `topology.canonical_enforcement` refuses at runtime to claim enforcement over
 * zero tables. This file adds NO enforcement. Duplicating any of that would be
 * the mistake, not the fix.
 *
 * What is missing is COMPOSITION. The hold's release condition is prose, and its
 * evidence is spread across six files whose naming conventions disagree: the
 * registry spells a key `PayRun`, the suite on disk is
 * `pay_run_port_as_runtime_role.rs`, and the CI map calls the same suite
 * `payroll-adapter-postgres-pay-run-port-as-runtime-role-pg`. Deciding whether
 * the hold may lift therefore means re-deriving three transforms by hand, and
 * that derivation is genuinely error-prone -- a `find` for the suite under the
 * canonical adapter misses PayRun entirely, because PayRun's port suite lives in
 * the payroll adapter with a different owner. Getting that wrong reads as "PayRun
 * has almost no coverage" when it has seventeen tests.
 *
 * So: one command, three transforms applied consistently, evidence cited. It
 * fails if any leg of the condition stops being true, which also means the hold
 * text cannot quietly drift away from the registry it describes.
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, resolve, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

/** `PayRun` -> `pay_run`, matching the suite filenames on disk. */
export function snakeCase(key) {
  return key.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();
}

/** `pay_run_port_as_runtime_role` -> `pay-run-port-as-runtime-role`, the CI map spelling. */
export function kebabCase(snake) {
  return snake.replace(/_/g, "-");
}

/**
 * The canonical objects the hold names, read from the hold itself rather than
 * hardcoded -- if someone edits the bullet, this follows.
 *
 * @param {string} markdown contents of PRODUCT.md
 * @returns {string[]}
 */
export function holdObjects(markdown) {
  // Matches the bullet in EITHER state. Releasing the hold must not retire the
  // checking that justified releasing it: if an owning port or a single-writer
  // boundary regresses after release, that is worse than it never having been
  // released, and a parser that only recognised "**HOLD**" would go silent at
  // exactly that moment. So the released form is still parsed and still checked.
  const line = markdown
    .split("\n")
    .find((l) => /projection fan-out is \*\*(HOLD|RELEASED)\*\*/.test(l));
  if (!line) {
    throw new Error(
      "PRODUCT.md no longer contains a projection fan-out bullet in either the HOLD or the "
        + "RELEASED form; the condition this verifies has no subject",
    );
  }
  const subject = line.slice(line.indexOf("- ") + 2, line.indexOf("projection fan-out"));
  return subject.match(/[A-Z][A-Za-z]+/g) ?? [];
}

/**
 * Whether the projection fan-out bullet currently reads as released.
 *
 * Reported, not enforced: this file decides nothing about whether release was
 * correct. It exists so the output says which state it is checking, and so a
 * reader cannot mistake "every leg met" under a released bullet for a hold that
 * is still in force.
 *
 * @param {string} markdown
 * @returns {boolean}
 */
export function holdReleased(markdown) {
  return /projection fan-out is \*\*RELEASED\*\*/.test(markdown);
}

/**
 * The writer-ownership registry: every object key, its owning crate, its tables.
 *
 * @param {string} source contents of canonical-domain/src/lib.rs
 * @returns {Array<{key:string, slug:string, owner:string, tables:string[]}>}
 */
export function registry(source) {
  const out = [];
  const re = /(\w+) => "(\w+)",\s*\n\s*owner = "([^"]+)",\s*\n\s*tables = \[([\s\S]*?)\];/g;
  let m;
  while ((m = re.exec(source)) !== null) {
    const [, key, slug, owner, rawTables] = m;
    const tables = rawTables
      .split(",")
      .map((t) => t.trim().replace(/^"|"$/g, ""))
      .filter(Boolean);
    out.push({ key, slug, owner, tables });
  }
  return out;
}

/** Every `*_port_as_runtime_role.rs` in the tree, by basename without extension. */
export function portSuites(root = ROOT) {
  const found = new Map();
  const walk = (dir) => {
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      if (entry.name === "target" || entry.name === ".git" || entry.name === "node_modules") continue;
      const full = join(dir, entry.name);
      // Resolve through symlinks: `withFileTypes` reports a symlinked directory
      // as neither file nor directory, so a naive `isDirectory()` walk skips it
      // and silently under-reports proofs. A verifier that misses a suite would
      // report a met condition as unmet -- noisy, but worse, it trains you to
      // ignore it.
      let isDir = entry.isDirectory();
      if (entry.isSymbolicLink()) {
        try {
          isDir = statSync(full).isDirectory();
        } catch {
          continue;
        }
      }
      if (isDir) walk(full);
      else if (entry.name.endsWith("_port_as_runtime_role.rs")) {
        found.set(entry.name.replace(/\.rs$/, ""), full);
      }
    }
  };
  walk(join(root, "backend"));
  return found;
}

/**
 * Evaluate every leg of the hold's release condition.
 *
 * @returns {{failures: string[], rows: Array<object>}}
 */
export function evaluate(root = ROOT) {
  const failures = [];
  const product = readFileSync(resolve(root, "docs/current/PRODUCT.md"), "utf8");
  const domain = readFileSync(
    resolve(root, "backend/crates/ontology/canonical-domain/src/lib.rs"),
    "utf8",
  );
  const map = JSON.parse(readFileSync(resolve(root, "tools/ci/postgres-cargo-map.json"), "utf8"));

  const named = holdObjects(product);
  const released = holdReleased(product);
  const keys = registry(domain);
  const suites = portSuites(root);

  if (keys.length === 0) {
    // A verifier that examines nothing must fail; a regex that silently stopped
    // matching would otherwise report a clean bill of health over zero objects.
    failures.push("registry: parsed zero object keys from canonical-domain/src/lib.rs");
    return { failures, rows: [], released };
  }

  // Every object the hold names must actually be a registry key. A hold naming
  // an object that does not exist can never be evaluated, let alone released.
  for (const object of named) {
    if (!keys.some((k) => k.key === object)) {
      failures.push(
        `hold names "${object}" but it is not an ObjectKey; the hold cannot be evaluated against the registry`,
      );
    }
  }

  const rows = [];
  for (const entry of keys) {
    const snake = snakeCase(entry.key);
    const suiteName = `${snake}_port_as_runtime_role`;
    const suitePath = suites.get(suiteName);
    const mapName = `${entry.owner.replace(/^console-/, "")}-${kebabCase(suiteName)}-pg`;
    const mapped = (map.entries ?? []).find((e) => e.name === mapName);

    // Leg 1: an explicit owning port.
    if (!entry.owner.startsWith("console-")) {
      failures.push(`${entry.key}: owner "${entry.owner}" is not a workspace crate`);
    }
    // Leg 2: a single-writer boundary -- at least one table it alone may write.
    if (entry.tables.length === 0) {
      failures.push(`${entry.key}: owns no table, so no writer rule applies to it`);
    }
    // Leg 3: a test that fails when the boundary breaks, and that actually runs.
    if (!suitePath) {
      failures.push(`${entry.key}: no ${suiteName}.rs on disk; the boundary has no port suite`);
    }
    if (!mapped) {
      failures.push(`${entry.key}: no CI map entry named ${mapName}; its port suite may never run`);
    } else if (!mapped.in_workflow_postgres_job) {
      failures.push(`${entry.key}: CI map entry ${mapName} is not in the workflow postgres job`);
    }

    rows.push({
      key: entry.key,
      named: named.includes(entry.key),
      owner: entry.owner,
      tables: entry.tables.length,
      suite: suitePath ? suitePath.slice(root.length + 1) : null,
      ci: Boolean(mapped?.in_workflow_postgres_job),
    });
  }
  return { failures, rows, released };
}

const isMain = process.argv[1] && process.argv[1].endsWith("hold-release-conditions.mjs");
if (isMain) {
  const { failures, rows, released } = evaluate();
  console.log(
    `projection fan-out: ${released ? "RELEASED — the legs below are what keeps it releasable" : "HOLD"}`,
  );
  for (const r of rows) {
    const mark = r.owner && r.tables > 0 && r.suite && r.ci ? "MET" : "NOT MET";
    console.log(
      `${mark.padEnd(8)} ${r.key.padEnd(12)} ${r.named ? "(named in hold) " : "                "}` +
        `owner=${r.owner} tables=${r.tables} ci=${r.ci ? "yes" : "no"}`,
    );
    if (r.suite) console.log(`${" ".repeat(9)}proof: ${r.suite}`);
  }
  console.log("");
  if (failures.length) {
    console.error(failures.join("\n"));
    console.error(`\nhold-release-conditions: ${failures.length} unmet condition(s)`);
    process.exit(1);
  }
  console.log(
    "hold-release-conditions: every leg met for all " +
      rows.length +
      " canonical objects (owning port, owned tables, port suite, CI-wired)",
  );
}
