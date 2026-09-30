import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import test from "node:test";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const checker = resolve(root, "scripts/check-foundation-gates.mjs");
const liveAuthorityContracts = [
  "docs/specs/foundation-gates.md",
  "docs/specs/review-fix-merge-governance.md",
];

function runChecker() {
  return spawnSync(process.execPath, [checker], {
    cwd: root,
    encoding: "utf8",
  });
}

function withInjectedContractText(path, text, callback) {
  const absolutePath = resolve(root, path);
  const original = readFileSync(absolutePath, "utf8");
  writeFileSync(absolutePath, `${original}\n${text}\n`);
  try {
    callback();
  } finally {
    writeFileSync(absolutePath, original);
  }
}

function withInjectedTextBefore(path, marker, text, callback) {
  const absolutePath = resolve(root, path);
  const original = readFileSync(absolutePath, "utf8");
  assert.ok(original.includes(marker), `${path} is missing test marker ${marker}`);
  writeFileSync(absolutePath, original.replace(marker, `${text}\n\n${marker}`));
  try {
    callback();
  } finally {
    writeFileSync(absolutePath, original);
  }
}

function withReplacedContractText(path, needle, replacement, callback) {
  const absolutePath = resolve(root, path);
  const original = readFileSync(absolutePath, "utf8");
  assert.ok(original.includes(needle), `${path} is missing test needle ${needle}`);
  writeFileSync(absolutePath, original.replace(needle, replacement));
  try {
    callback();
  } finally {
    writeFileSync(absolutePath, original);
  }
}

test("foundation gate rejects retired authority reentry in every live authority contract", () => {
  const hostileLiterals = [
    "hermes kanban",
    "Hermes agent authority",
    "hermes session authority",
    "OMC",
    "GJC",
    "NousResearch Hermes",
    "~/.codex/agents executor authority",
  ];

  for (const path of liveAuthorityContracts) {
    for (const hostileLiteral of hostileLiterals) {
      withInjectedContractText(path, hostileLiteral, () => {
        const result = runChecker();
        assert.notEqual(result.status, 0, `${path} accepted ${hostileLiteral}`);
        assert.match(result.stderr, /excludes retired|React Native Hermes JS engine/i);
      });
    }
  }
});

test("foundation gate permits the React Native Hermes JS engine as a technical dependency", () => {
  for (const path of liveAuthorityContracts) {
    withInjectedContractText(path, "React Native Hermes JS engine technical dependency", () => {
      const result = runChecker();
      assert.equal(result.status, 0, `${path} incorrectly rejected the React Native Hermes JS engine`);
    });
  }
});

test("foundation gate rejects retired API-contract citations in live CI docs", () => {
  for (const [path, retiredCitation] of [
    ["docs/CI-GATES.md", "npm run test:contract"],
    ["docs/CI-GATES.md", "npm run check:openapi-app"],
    ["docs/CI-GATES.md", "CONTRACT_DATABASE_URL=postgres://example.invalid/contract"],
  ]) {
    withInjectedContractText(path, retiredCitation, () => {
      const result = runChecker();
      assert.notEqual(result.status, 0, `${path} accepted ${retiredCitation}`);
      assert.match(result.stderr, /retired (?:generated-client round-trip|app-served OpenAPI|contract database handoff)/i);
    });
  }
});

test("foundation gate rejects a reintroduced retired OpenAPI package script", () => {
  withInjectedTextBefore(
    "package.json",
    '    "check:platform-contract-drift":',
    '    "check:openapi-app": "node scripts/check-openapi-app.mjs",',
    () => {
      const result = runChecker();
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /package\.json: scripts must not define retired check:openapi-app/i);
    },
  );
});

test("historical GO-LIVE prose no longer controls the foundation gate", () => {
  withInjectedContractText(
    "docs/GO-LIVE-CHECKLIST.md",
    "Historical note: the retired `check:openapi-app` command was once required.",
    () => {
      const result = runChecker();
      assert.equal(result.status, 0, result.stderr);
    },
  );
});

test("foundation gate rejects a nonexistent package command in the live CI runbook", () => {
  withInjectedTextBefore(
    "docs/CI-GATES.md",
    "## Backend gates",
    "Run `npm run check:nonexistent-live-gate` before merging.",
    () => {
      const result = runChecker();
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /live CI gate documentation package scripts.*check:nonexistent-live-gate/i);
    },
  );
});

test("foundation gate executes the structural security-workflow hardening gate", () => {
  withReplacedContractText(
    ".github/workflows/security.yml",
    '          "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" audit --ignore RUSTSEC-2023-0071',
    '          "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" audit --ignore RUSTSEC-2023-0071 || true',
    () => {
      const result = runChecker();
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /security\.jobs\.rust-advisories/i);
    },
  );
});

test("foundation gate rejects required-context path filters", () => {
  withInjectedTextBefore(
    ".github/workflows/ci.yml",
    "  workflow_dispatch:",
    "    paths:\n      - \"backend/**\"",
    () => {
      const result = runChecker();
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /must not define paths or paths-ignore filters/i);
    },
  );
});
