import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { checkNextestConfig, extractOverrideFilter } from "./check-nextest-config.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

test("repo nextest.toml satisfies DN-0005 P3 control", () => {
  const text = readFileSync(resolve(root, ".config/nextest.toml"), "utf8");
  assert.deepEqual(checkNextestConfig(text), []);
});

test("missing serial group fails", () => {
  assert.ok(checkNextestConfig("[profile.ci]\n").length > 0);
});

test("missing one filter fails", () => {
  const text = `
[test-groups.cluster-global]
max-threads = 1
[[profile.default.overrides]]
filter = 'test(/leave_migration_expand_contract/)'
test-group = 'cluster-global'
# pin 0.9.138
`;
  const fails = checkNextestConfig(text);
  assert.ok(fails.some((f) => f.includes("key_revision")));
});

test("the pre-fix test() form is rejected, not merely tolerated", () => {
  // This is the exact shape that shipped: every required name present as a
  // substring, so the old gate was green, while cargo-nextest grouped 1 of 4
  // apalis_adapter tests and 0 of 9 leave_migration_expand_contract tests.
  const good = readFileSync(resolve(root, ".config/nextest.toml"), "utf8");
  const broken = good.replace(/binary\((\w+)\)/g, "test(/$1/)");
  const fails = checkNextestConfig(broken);
  assert.ok(fails.length > 0, "the under-grouping form must fail");
  assert.ok(
    fails.some((f) => /use binary\(apalis_adapter\)/.test(f)),
    `expected a binary() remedy, got ${JSON.stringify(fails)}`,
  );
});

test("a serial suite dropped from the filter fails", () => {
  const good = readFileSync(resolve(root, ".config/nextest.toml"), "utf8");
  const dropped = good.replace("binary(apalis_adapter)\n  + ", "");
  assert.ok(
    checkNextestConfig(dropped).some((f) => f.includes("binary(apalis_adapter)")),
  );
});

test("the filter scope excludes prose, so comments may quote the broken form", () => {
  // The committed file explains the defect using a literal test(...) example.
  // Scanning the whole file instead of the filter block would fail on its own
  // documentation.
  const good = readFileSync(resolve(root, ".config/nextest.toml"), "utf8");
  assert.match(good, /test\(\/apalis_adapter\/\)/, "fixture assumes the comment exists");
  assert.deepEqual(checkNextestConfig(good), []);
});

test("extractOverrideFilter returns null when no filter block exists", () => {
  assert.equal(extractOverrideFilter("[profile.ci]\n"), null);
  assert.equal(extractOverrideFilter(null), null);
});
