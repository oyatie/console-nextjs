#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const args = new Map();
for (let index = 2; index < process.argv.length; index += 2) args.set(process.argv[index], process.argv[index + 1]);
const mode = args.get("--mode");
const reportPath = args.get("--audit-report");
const registryPath = args.get("--registry") ?? "security/node-audit-exceptions.json";
// Parameterized like --registry so the gate's own tests can assert the
// lockfile-mismatch branch against a fixture instead of the repo lockfile.
const lockPath = args.get("--lockfile") ?? "package-lock.json";
if (!["dev-codegen", "production"].includes(mode) || !reportPath) {
  throw new Error("usage: check-node-audit-exceptions.mjs --mode <dev-codegen|production> --audit-report <path> [--registry <path>] [--lockfile <path>]");
}
const report = JSON.parse(readFileSync(reportPath, "utf8"));
const registry = JSON.parse(readFileSync(resolve(root, registryPath), "utf8"));
const lock = JSON.parse(readFileSync(resolve(root, lockPath), "utf8"));
if (!Number.isInteger(report.auditReportVersion) || typeof report.vulnerabilities !== "object" || report.vulnerabilities === null || typeof report.metadata?.vulnerabilities !== "object") {
  throw new Error("npm audit report is incomplete; refusing to treat an audit execution failure as clean");
}
if (registry.schema_version !== "node-audit-exceptions-v1" || !Array.isArray(registry.entries)) {
  throw new Error("node audit exception registry must be node-audit-exceptions-v1 with an entries array");
}
const today = new Date().toISOString().slice(0, 10);
const failures = [];
const used = new Set();
// Own-property only: package-lock maps inherit Object.prototype, so a hostile
// exception path named `constructor`/`toString` must not resolve to a Function.
const packages = lock.packages && typeof lock.packages === "object" ? lock.packages : null;
for (const [index, entry] of registry.entries.entries()) {
  const label = `exception[${index}]`;
  for (const key of ["advisory", "package", "version", "path", "scope", "owner", "tracking", "rationale", "trivy_statement", "expires_on"]) {
    if (typeof entry[key] !== "string" || !entry[key].trim()) failures.push(`${label} missing ${key}`);
  }
  if (entry.scope !== "dev-codegen") failures.push(`${label} must be dev-codegen scoped`);
  if (!/^GHSA-[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{4}$/i.test(entry.advisory ?? "")) failures.push(`${label} has invalid advisory`);
  if (!/^\d{4}-\d{2}-\d{2}$/.test(entry.expires_on ?? "") || entry.expires_on <= today) failures.push(`${label} is expired`);
  const expiresAt = Date.parse(`${entry.expires_on}T00:00:00Z`);
  if (!Number.isFinite(expiresAt) || expiresAt - Date.now() > 30 * 24 * 60 * 60 * 1000) failures.push(`${label} exceeds 30-day TTL`);
  const installed = packages && Object.hasOwn(packages, entry.path)
    ? packages[entry.path]?.version
    : undefined;
  if (installed !== entry.version) failures.push(`${label} lockfile mismatch: expected ${entry.version}, got ${installed ?? "missing"}`);
}
const vulnerabilities = report.vulnerabilities ?? {};
const directFindings = [];
const unmatchableFindings = [];
for (const [name, vulnerability] of Object.entries(vulnerabilities)) {
  if (!["high", "critical"].includes(vulnerability.severity)) continue;
  const advisories = (Array.isArray(vulnerability.via) ? vulnerability.via : [])
    .filter((via) => typeof via === "object" && via !== null && typeof via.url === "string")
    .map((via) => via.url.match(/GHSA-[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{4}/i)?.[0])
    .filter(Boolean);
  const paths = (Array.isArray(vulnerability.nodes) ? vulnerability.nodes : [])
    .filter((path) => typeof path === "string" && path.trim());
  if (!advisories.length || !paths.length) {
    unmatchableFindings.push({ package: name, severity: vulnerability.severity });
    continue;
  }
  for (const advisory of advisories) {
    for (const path of paths) directFindings.push({ advisory, package: name, path, severity: vulnerability.severity });
  }
}
if (mode === "production") {
  if (directFindings.length || Object.values(vulnerabilities).some((v) => ["high", "critical"].includes(v.severity))) failures.push("production npm audit contains HIGH/CRITICAL findings; exceptions are forbidden");
} else {
  for (const finding of unmatchableFindings) {
    failures.push(`unmatchable ${finding.severity} vulnerability: ${finding.package} lacks an exact structured GHSA and lockfile path`);
  }
  for (const finding of directFindings) {
    const version = packages && Object.hasOwn(packages, finding.path)
      ? packages[finding.path]?.version
      : undefined;
    const matchIndex = registry.entries.findIndex((entry) => entry.advisory === finding.advisory && entry.package === finding.package && entry.version === version && entry.path === finding.path);
    if (matchIndex < 0) failures.push(`unmatched ${finding.severity} finding: ${finding.advisory} ${finding.package}@${version ?? "missing"} ${finding.path}`);
    else used.add(matchIndex);
  }
  for (const [index, entry] of registry.entries.entries()) if (!used.has(index)) failures.push(`stale exception: ${entry.advisory} ${entry.package}@${entry.version} ${entry.path}`);
}
if (failures.length) throw new Error(`Node audit exception gate failed:\n- ${failures.join("\n- ")}`);
console.log(`NODE_AUDIT_${mode.toUpperCase().replace("-", "_")}_PASS`);
