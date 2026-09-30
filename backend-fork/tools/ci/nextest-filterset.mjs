#!/usr/bin/env node
/**
 * Translate the postgres cargo map into a single cargo-nextest filterset.
 *
 * Today each shard runs its targets as N separate `cargo test` invocations, each
 * with `--test-threads=1`. Measured 2026-08-18 on run 32097786166, shard
 * domain-b: 44 invocations, 1096s of test execution against 195s of compile --
 * test execution is 78% of the job and it is entirely serial.
 *
 * `.config/nextest.toml` already encodes the parallel/serial split this repo
 * decided on (ADR-0039 / DN-0005 P3): only the `cluster-global` group is
 * `max-threads = 1`, and its comment says outright that the rest stay parallel.
 * That config has never been invoked by anything -- the ledger records the
 * runner swap as a deferred follow-up blocked on "preflight command locks", not
 * as a safety concern.
 *
 * This module is the translation half, kept pure so it can be tested without
 * Docker, cargo, or a runner.
 *
 * Mapping (nextest binary IDs):
 *   cargo test -p P --test Y   ->  binary_id(P::Y)
 *   cargo test -p P --lib      ->  binary_id(P)
 */

/**
 * Parse one map entry's cargo argv into its nextest binary id.
 *
 * Returns null for argv shapes this translation does not cover, so an
 * unrecognised entry is dropped loudly by the caller rather than silently
 * running nothing -- a filterset that quietly omits a target is a false green.
 *
 * @param {{package?: string, cargo_argv?: string[]}} entry
 * @returns {string|null}
 */
export function binaryIdForEntry(entry) {
  const pkg = String(entry?.package ?? "");
  const argv = Array.isArray(entry?.cargo_argv) ? entry.cargo_argv : [];
  if (!pkg) return null;
  const testIndex = argv.indexOf("--test");
  if (testIndex >= 0) {
    const name = argv[testIndex + 1];
    if (typeof name !== "string" || name === "" || name.startsWith("-")) return null;
    return `${pkg}::${name}`;
  }
  if (argv.includes("--lib")) return `${pkg}`;
  return null;
}

/**
 * Render a binary id as an exact-match nextest matcher.
 *
 * The `=` prefix is exact match, and it must be UNQUOTED. Measured 2026-08-18
 * against cargo-nextest 0.9.138: `binary_id(=pkg::name)` matches 4 tests while
 * `binary_id(="pkg::name")` matches 0 and the run dies with "operator didn't
 * match any binary IDs". The earlier quoted form looked safer and selected
 * nothing.
 *
 * Because the value cannot be quoted, an id carrying expression syntax would
 * corrupt the filterset rather than be escaped. nextest binary ids are
 * `<cargo package>::<target>`, both of which are restricted to word characters
 * and hyphens, so anything else is rejected outright rather than emitted.
 *
 * @param {string} id
 * @returns {string}
 */
export function quoteBinaryId(id) {
  const value = String(id);
  if (!/^[A-Za-z0-9_.-]+(::[A-Za-z0-9_.-]+)?$/.test(value)) {
    throw new Error(`binary id is not safe to embed unquoted in a filterset: ${JSON.stringify(value)}`);
  }
  return `binary_id(=${value})`;
}

/**
 * Build the filterset for a set of map entries.
 *
 * @param {Array<object>} entries
 * @returns {{expression: string, ids: string[], unmapped: string[]}}
 */
export function filtersetFromEntries(entries) {
  const ids = [];
  const unmapped = [];
  const seen = new Set();
  for (const entry of entries ?? []) {
    const id = binaryIdForEntry(entry);
    if (id === null) {
      unmapped.push(String(entry?.name ?? "(unnamed)"));
      continue;
    }
    // Two map entries can name the same binary (different filters); nextest
    // selects a binary once, so duplicates must collapse rather than repeat.
    if (seen.has(id)) continue;
    seen.add(id);
    ids.push(id);
  }
  ids.sort();
  return {
    expression: ids.map(quoteBinaryId).join(" + "),
    ids,
    unmapped,
  };
}

const isMain = process.argv[1] && process.argv[1].endsWith("nextest-filterset.mjs");
if (isMain) {
  // Reads the same JSONL the cargo runner consumes: one {name,package,argv}
  // object per line. `argv` is accepted as an alias for `cargo_argv` because
  // that is the key cargo_needs_postgres.sh already emits.
  const text = await new Promise((resolve) => {
    let buffer = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (chunk) => { buffer += chunk; });
    process.stdin.on("end", () => resolve(buffer));
  });
  const entries = text
    .split("\n")
    .filter((line) => line.trim() !== "")
    .map((line) => {
      const row = JSON.parse(line);
      return { name: row.name, package: row.package, cargo_argv: row.cargo_argv ?? row.argv };
    });
  const { expression, ids, unmapped } = filtersetFromEntries(entries);
  if (unmapped.length) {
    console.error(`nextest-filterset: ${unmapped.length} entr(ies) could not be translated:`);
    for (const name of unmapped) console.error(`  ${name}`);
    process.exit(1);
  }
  if (ids.length === 0) {
    console.error("nextest-filterset: no entries selected");
    process.exit(1);
  }
  process.stdout.write(expression);
}
