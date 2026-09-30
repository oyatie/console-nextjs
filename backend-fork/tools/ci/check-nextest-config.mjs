#!/usr/bin/env node
/**
 * DN-0005 P3: fail-closed presence of nextest serial-group config.
 * Does not run nextest — only asserts the durable process control exists.
 */
import { readFileSync, existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const path = resolve(root, ".config/nextest.toml");

const REQUIRED_FILTERS = [
  "leave_migration_expand_contract",
  "key_revision_migration_upgrade",
  "attendance_console_migration_contract",
  "apalis_adapter",
  "apalis_schema_contract",
  "group_of_one_expand_contract",
];

/**
 * The cluster-global override's filter value, or null.
 * Scoped deliberately: comments elsewhere in the file may legitimately quote the
 * broken test(...) form while explaining why it is broken.
 *
 * @param {string} tomlText
 * @returns {string|null}
 */
export function extractOverrideFilter(tomlText) {
  const text = String(tomlText ?? "");
  // TOML permits multi-line literal, single-quoted, and basic strings. Accept
  // all three: a filter is no less wrong for being written on one line.
  const multi = /filter\s*=\s*'''([\s\S]*?)'''/.exec(text);
  if (multi) return multi[1];
  const single = /filter\s*=\s*'([^'\n]*)'/.exec(text);
  if (single) return single[1];
  const basic = /filter\s*=\s*"([^"\n]*)"/.exec(text);
  return basic ? basic[1] : null;
}

export function checkNextestConfig(tomlText) {
  const failures = [];
  if (!tomlText.includes("[test-groups.cluster-global]")) {
    failures.push("missing [test-groups.cluster-global]");
  }
  if (!/max-threads\s*=\s*1/.test(tomlText)) {
    failures.push("cluster-global must set max-threads = 1");
  }
  if (!tomlText.includes("test-group = 'cluster-global'") && !tomlText.includes('test-group = "cluster-global"')) {
    failures.push("override must assign test-group = cluster-global");
  }
  // The names below are test BINARY names, so they must be matched with
  // `binary(...)`. `test(...)` matches TEST names, and the difference is not
  // cosmetic: measured 2026-08-18 with `cargo nextest show-config test-groups`,
  // the old test-name form put 1 of 4 tests of apalis_adapter in the serial
  // group and 0 of 9 for leave_migration_expand_contract, while the control read
  // green throughout because every name was present as a substring. Assert the
  // form that actually groups, not the spelling.
  const filterBlock = extractOverrideFilter(tomlText);
  if (filterBlock === null) {
    failures.push("cluster-global override must declare a filter block");
  } else {
    for (const name of REQUIRED_FILTERS) {
      if (!filterBlock.includes(`binary(${name})`)) {
        failures.push(`filter must group serial suite by binary(${name})`);
      }
      if (new RegExp(String.raw`test\(/?${name}`).test(filterBlock)) {
        failures.push(
          `filter uses test(${name}); that matches test names, not the binary, `
          + `and silently under-groups it -- use binary(${name})`,
        );
      }
    }
  }
  if (!tomlText.includes("0.9.138")) {
    failures.push("pin cargo-nextest 0.9.138 must be documented in nextest.toml comments");
  }
  return failures;
}

const isMain =
  process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1]);

if (isMain) {
  if (!existsSync(path)) {
    console.error(`missing ${path}`);
    process.exit(1);
  }
  const failures = checkNextestConfig(readFileSync(path, "utf8"));
  if (failures.length) {
    console.error(failures.join("\n"));
    process.exit(1);
  }
  console.log("nextest-config OK (cluster-global serial group + pin 0.9.138 documented)");
}
