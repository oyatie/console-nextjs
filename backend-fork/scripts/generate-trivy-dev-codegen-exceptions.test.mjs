import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

const root = new URL("..", import.meta.url).pathname;
const script = join(root, "scripts/generate-trivy-dev-codegen-exceptions.mjs");
// Self-contained fixture registry. These tests prove the gate's accept/reject
// behaviour, so they must not depend on whether the repo currently ships any
// live exception (security/node-audit-exceptions.json is empty since the
// OpenAPI codegen devDependencies were removed).
const canonical = {
  schema_version: "node-audit-exceptions-v1",
  entries: [
    {
      advisory: "GHSA-mh99-v99m-4gvg",
      package: "brace-expansion",
      version: "2.1.2",
      path: "node_modules/@redocly/openapi-core/node_modules/brace-expansion",
      scope: "dev-codegen",
      owner: "platform-security",
      tracking: "SEC-FIXTURE-001",
      rationale: "Fixture entry exercising the dev-codegen exception path; not a live suppression.",
      trivy_statement: "Fixture dev-only matcher path; tracked by SEC-FIXTURE-001.",
      expires_on: "2099-01-01",
    },
    {
      advisory: "GHSA-mh99-v99m-4gvg",
      package: "brace-expansion",
      version: "5.0.7",
      path: "node_modules/brace-expansion",
      scope: "dev-codegen",
      owner: "platform-security",
      tracking: "SEC-FIXTURE-001",
      rationale: "Fixture entry exercising the dev-codegen exception path; not a live suppression.",
      trivy_statement: "Fixture dev-only CLI matcher path; tracked by SEC-FIXTURE-001.",
      expires_on: "2099-01-01",
    },
  ],
};
const expiry = new Date(Date.now() + 14 * 24 * 60 * 60 * 1000).toISOString().slice(0, 10);
function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "trivy-exception-parity-"));
  const registry = {
    ...canonical,
    entries: [{
      advisory: "GHSA-mh99-v99m-4gvg",
      package: "brace-expansion",
      version: "5.0.8",
      path: "node_modules/brace-expansion",
      scope: "dev-codegen",
      owner: "platform-security",
      tracking: "SEC-TEST-001",
      rationale: "Synthetic exact-match regression fixture.",
      expires_on: expiry,
      trivy_statement: "Synthetic exact-match regression fixture.",
    }],
  };
  const registryPath = join(dir, "registry.json");
  const outputPath = join(dir, "exceptions.yaml");
  writeFileSync(registryPath, JSON.stringify(registry));
  execFileSync(process.execPath, [script, "--write", "--registry", registryPath, "--output", outputPath], { cwd: root, stdio: "pipe" });
  return { registry, registryPath, outputPath };
}
function check(registryPath, outputPath) {
  return () => execFileSync(process.execPath, [script, "--check", "--registry", registryPath, "--output", outputPath], { cwd: root, encoding: "utf8", stdio: "pipe" });
}
test("Trivy YAML is the exact canonical JSON projection", () => {
  const { registryPath, outputPath } = fixture();
  assert.match(check(registryPath, outputPath)(), /PARITY_PASS/);
});
test("committed Trivy YAML represents an empty remediated registry", () => {
  assert.match(
    execFileSync(process.execPath, [script, "--check"], { cwd: root, encoding: "utf8", stdio: "pipe" }),
    /PARITY_PASS/,
  );
});
test("Trivy parity rejects a rogue YAML entry", () => {
  const { registryPath, outputPath } = fixture();
  writeFileSync(outputPath, `${readFileSync(outputPath, "utf8")}  - id: GHSA-aaaa-bbbb-cccc\n`);
  assert.throws(check(registryPath, outputPath), /differs from the canonical JSON projection/);
});
test("Trivy parity rejects widened or missing package constraints", () => {
  const { registryPath, outputPath } = fixture();
  const yaml = readFileSync(outputPath, "utf8");
  writeFileSync(outputPath, yaml.replace("pkg:npm/brace-expansion@5.0.8", "pkg:npm/brace-expansion@*"));
  assert.throws(check(registryPath, outputPath), /differs from the canonical JSON projection/);
  const second = fixture();
  writeFileSync(second.outputPath, readFileSync(second.outputPath, "utf8").replace("    paths:\n      - node_modules/brace-expansion\n", ""));
  assert.throws(check(second.registryPath, second.outputPath), /differs from the canonical JSON projection/);
});
test("Trivy parity rejects expired canonical entries and expiry mismatch", () => {
  const expired = fixture();
  expired.registry.entries[0].expires_on = "2000-01-01";
  writeFileSync(expired.registryPath, JSON.stringify(expired.registry));
  assert.throws(check(expired.registryPath, expired.outputPath), /expired/);
  const mismatch = fixture();
  writeFileSync(mismatch.outputPath, readFileSync(mismatch.outputPath, "utf8").replace(expiry, "2000-01-01"));
  assert.throws(check(mismatch.registryPath, mismatch.outputPath), /differs from the canonical JSON projection/);
});
