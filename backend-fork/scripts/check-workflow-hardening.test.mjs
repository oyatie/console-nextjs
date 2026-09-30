import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import yaml from "js-yaml";

import {
  REQUIRED_SECURITY_CONTEXTS,
  evaluateSecurityWorkflowHardening,
} from "./check-workflow-hardening.mjs";

const workflow = readFileSync(
  new URL("../.github/workflows/security.yml", import.meta.url),
  "utf8",
);

function replaceOnce(source, needle, replacement) {
  assert.ok(source.includes(needle), `fixture is missing ${JSON.stringify(needle)}`);
  return source.replace(needle, replacement);
}

function replaceThroughMarker(source, startMarker, endMarker, replacement) {
  const start = source.indexOf(startMarker);
  assert.notEqual(start, -1, `fixture is missing ${JSON.stringify(startMarker)}`);
  const end = source.indexOf(endMarker, start + startMarker.length);
  assert.notEqual(end, -1, `fixture is missing ${JSON.stringify(endMarker)}`);
  return source.slice(0, start) + replacement + source.slice(end);
}

function rejected(source, label) {
  const result = evaluateSecurityWorkflowHardening(source);
  assert.ok(
    result.failures.length > 0,
    `${label} unexpectedly passed the workflow hardening gate`,
  );
}

function removeJob(source, job) {
  const start = source.indexOf(`  ${job}:\n`);
  assert.notEqual(start, -1, `fixture is missing job ${job}`);
  const next = source.slice(start + 1).search(/^  [A-Za-z0-9_-]+:/m);
  const end = next < 0 ? source.length : start + 1 + next;
  return source.slice(0, start) + source.slice(end);
}

test("accepts the live five-context security proof plan", () => {
  const result = evaluateSecurityWorkflowHardening(workflow);
  assert.deepEqual(result.failures, []);
  assert.equal(REQUIRED_SECURITY_CONTEXTS.length, 5);
  assert.deepEqual(
    REQUIRED_SECURITY_CONTEXTS.map(({ job }) => job),
    [
      "filesystem",
      "iac",
      "rust-advisories",
      "rust-supply-chain",
      "node-advisories",
    ],
  );
});

test("locks Required / Security to the exact five existing security proofs", () => {
  const requiredDependencies = [
    "filesystem",
    "iac",
    "rust-advisories",
    "rust-supply-chain",
    "node-advisories",
  ];
  const model = yaml.load(workflow);
  assert.equal(Object.keys(model.jobs).length, 6);
  assert.equal(model.jobs["required-security"].name, "Required / Security");
  assert.deepEqual(model.jobs["required-security"].needs, requiredDependencies);
  assert.equal(model.jobs["required-security"]["timeout-minutes"], 5);

  const mutations = [
    [
      "aggregator deletion",
      removeJob(workflow, "required-security"),
    ],
    ["leaf job deletion", removeJob(workflow, "node-advisories")],
    ["leaf job rename", replaceOnce(workflow, "  filesystem:\n", "  filesystem-renamed:\n")],
    [
      "extra job",
      replaceOnce(
        workflow,
        "  required-security:\n",
        "  candidate-shim:\n    runs-on: ubuntu-latest\n    steps: []\n\n  required-security:\n",
      ),
    ],
    ["context rename", replaceOnce(workflow, "name: Required / Security", "name: Required Security")],
    ["always weakening", replaceOnce(workflow, "    if: ${{ always() }}", "    if: ${{ success() }}")],
    ["dependency deletion", replaceOnce(workflow, "      - node-advisories\n", "")],
    [
      "dependency injection",
      replaceOnce(workflow, "      - node-advisories\n", "      - node-advisories\n      - candidate-shim\n"),
    ],
    [
      "result deletion",
      replaceOnce(workflow, '          test "${{ needs.filesystem.result }}" = success &&\n', ""),
    ],
    [
      "skipped accepted",
      replaceOnce(
        workflow,
        '          test "${{ needs.filesystem.result }}" = success',
        '          test "${{ needs.filesystem.result }}" != failure',
      ),
    ],
    [
      "cancelled accepted",
      replaceOnce(
        workflow,
        '          test "${{ needs.iac.result }}" = success',
        '          test "${{ needs.iac.result }}" != cancelled',
      ),
    ],
    [
      "failure accepted",
      replaceOnce(
        workflow,
        '          test "${{ needs.node-advisories.result }}" = success',
        '          test "${{ needs.node-advisories.result }}" != skipped',
      ),
    ],
    [
      "checkout injection",
      replaceOnce(
        workflow,
        "    steps:\n      - name: Require every security proof to succeed\n",
        "    steps:\n      - uses: actions/checkout@v7\n      - name: Require every security proof to succeed\n",
      ),
    ],
    [
      "soft failure",
      replaceOnce(
        workflow,
        "      - name: Require every security proof to succeed\n",
        "      - name: Require every security proof to succeed\n        continue-on-error: true\n",
      ),
    ],
  ];

  for (const [label, mutated] of mutations) rejected(mutated, label);
});

test("locks pull-request admission and the read-only workflow envelope", () => {
  rejected(
    replaceOnce(workflow, "  pull_request:\n", ""),
    "pull_request trigger deletion",
  );
  rejected(
    replaceOnce(workflow, "  contents: read\n", "  contents: write\n"),
    "workflow permission widening",
  );
  rejected(
    replaceOnce(
      workflow,
      "  cancel-in-progress: ${{ github.event_name == 'pull_request' }}",
      "  cancel-in-progress: true",
    ),
    "main-run cancellation widening",
  );
});

test("rejects a missing or conditionally skipped required security context", async (t) => {
  for (const { job } of REQUIRED_SECURITY_CONTEXTS) {
    await t.test(`${job}: deleted`, () => {
      const start = workflow.indexOf(`  ${job}:\n`);
      assert.notEqual(start, -1);
      const next = workflow.indexOf("\n  ", start + 3);
      const end = next === -1 ? workflow.length : next + 1;
      rejected(workflow.slice(0, start) + workflow.slice(end), `${job} deletion`);
    });
    await t.test(`${job}: if false`, () => {
      rejected(
        replaceOnce(
          workflow,
          `  ${job}:\n`,
          `  ${job}:\n    if: false\n`,
        ),
        `${job} if:false`,
      );
    });
    await t.test(`${job}: continue on error`, () => {
      rejected(
        replaceOnce(
          workflow,
          `  ${job}:\n`,
          `  ${job}:\n    continue-on-error: true\n`,
        ),
        `${job} continue-on-error`,
      );
    });
  }
});

test("rejects if and continue-on-error on required proof steps", () => {
  const marker = "      - name: Run cargo audit\n";
  rejected(
    replaceOnce(workflow, marker, `${marker}        if: false\n`),
    "proof step if:false",
  );
  rejected(
    replaceOnce(workflow, marker, `${marker}        continue-on-error: true\n`),
    "proof step continue-on-error",
  );
});

test("rejects workflow, job, and non-allowlisted step environments", () => {
  rejected(
    replaceOnce(workflow, "name: Security\n", "name: Security\nenv:\n  PATH: /tmp/attacker\n"),
    "workflow env",
  );
  rejected(
    replaceOnce(
      workflow,
      "  rust-advisories:\n",
      "  rust-advisories:\n    env:\n      PATH: /tmp/attacker\n",
    ),
    "job env",
  );
  rejected(
    replaceOnce(
      workflow,
      "      - name: Run cargo audit\n",
      "      - name: Run cargo audit\n        env:\n          PATH: /tmp/attacker\n",
    ),
    "step env",
  );
  rejected(
    replaceOnce(workflow, 'KUBECTL_VERSION: "v1.36.2"', 'KUBECTL_VERSION: "latest"'),
    "mutated kubectl env allowlist",
  );
});

test("rejects workflow, job, and step custom shells", () => {
  rejected(
    replaceOnce(
      workflow,
      "name: Security\n",
      "name: Security\ndefaults:\n  run:\n    shell: python\n",
    ),
    "workflow shell default",
  );
  rejected(
    replaceOnce(
      workflow,
      "  rust-advisories:\n",
      "  rust-advisories:\n    defaults:\n      run:\n        shell: bash {0}\n",
    ),
    "job shell default",
  );
  rejected(
    replaceOnce(
      workflow,
      "      - name: Run cargo audit\n",
      "      - name: Run cargo audit\n        shell: python\n",
    ),
    "unsafe step shell",
  );
});

test("locks each required context's executable proof rather than matching retained text", async (t) => {
  const mutations = [
    [
      "filesystem exit-zero prefix",
      "          /usr/local/bin/trivy fs --scanners vuln,secret --ignore-unfixed \\",
      "          exit 0\n          /usr/local/bin/trivy fs --scanners vuln,secret --ignore-unfixed \\",
    ],
    [
      "IaC production proof deletion",
      "          npm run check:production-hardening\n",
      "",
    ],
    [
      "cargo-audit masking",
      '          "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" audit --ignore RUSTSEC-2023-0071',
      '          "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" audit --ignore RUSTSEC-2023-0071 || true',
    ],
    [
      "cargo-audit direct-binary subcommand deletion",
      '          "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" audit --ignore RUSTSEC-2023-0071',
      '          "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" --ignore RUSTSEC-2023-0071',
    ],
    [
      "cargo-deny retained as echo text",
      '          "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-deny" --manifest-path backend/Cargo.toml check',
      "          : # direct cargo-deny proof deleted while the status text remains",
    ],
    [
      "Trivy projection regression deletion",
      "        run: node --test scripts/generate-trivy-dev-codegen-exceptions.test.mjs\n",
      "",
    ],
    [
      "workflow hardening regression deletion",
      "        run: node --test scripts/check-workflow-hardening.test.mjs\n",
      "",
    ],
    [
      "Node audit regression deletion",
      "        run: node --test scripts/check-node-audit-exceptions.test.mjs\n",
      "",
    ],
    [
      "production npm proof deletion",
      '          node scripts/check-node-audit-exceptions.mjs --mode production --audit-report "$report"\n',
      "",
    ],
    [
      "dev npm proof deletion",
      '          node scripts/check-node-audit-exceptions.mjs --mode dev-codegen --audit-report "$report"\n',
      "",
    ],
    [
      "npm audit text after an exit-zero prefix",
      '          if npm audit --omit=dev --audit-level=high --json > "$report"; then true; else true; fi',
      '          exit 0 # npm audit --omit=dev --audit-level=high --json > "$report"; then true; else true; fi',
    ],
  ];

  for (const [label, needle, replacement] of mutations) {
    await t.test(label, () => {
      rejected(replaceOnce(workflow, needle, replacement), label);
    });
  }
});

test("rejects candidate-controlled setup shims, missing checksums, lifecycle scripts, and Cargo aliases", () => {
  rejected(
    replaceThroughMarker(
      workflow,
      "      - name: Install checksum-pinned Trivy before candidate checkout\n",
      "      - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7\n",
      "      - uses: ./.github/actions/setup-trivy\n",
    ),
    "candidate-controlled setup-trivy shim",
  );
  rejected(
    replaceOnce(
      workflow,
      "3cbae37cd440cd8676e5ce9207fe460b5641c7579a17e9d00f8894928c41a88d",
      "0".repeat(64),
    ),
    "Trivy checksum mutation",
  );
  rejected(
    replaceOnce(workflow, "run: npm ci --ignore-scripts", "run: npm ci"),
    "candidate npm lifecycle execution",
  );
  rejected(
    replaceOnce(
      workflow,
      '          "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" audit --ignore RUSTSEC-2023-0071',
      "          cargo audit --ignore RUSTSEC-2023-0071",
    ),
    "root Cargo audit alias",
  );
  rejected(
    replaceOnce(
      workflow,
      '          "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-deny" --manifest-path backend/Cargo.toml check',
      "          cargo deny --manifest-path backend/Cargo.toml check",
    ),
    "root Cargo deny alias",
  );
});

test("locks proof order, action pins, context names, and duplicate YAML keys", () => {
  const checkout =
    "      - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7\n";
  const regression =
    "      - name: Trivy exception policy regression\n        run: node --test scripts/generate-trivy-dev-codegen-exceptions.test.mjs\n";
  rejected(
    replaceOnce(workflow, `${checkout}${regression}`, `${regression}${checkout}`),
    "reordered checkout and proof steps",
  );
  rejected(
    replaceOnce(
      workflow,
      "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0",
      "actions/checkout@main",
    ),
    "floating action pin",
  );
  rejected(
    replaceOnce(
      workflow,
      "name: cargo audit (RUSTSEC)",
      "name: npm audit",
    ),
    "spoofed required context name",
  );
  rejected(
    `${workflow}\n  filesystem:\n    name: duplicate\n    steps: []\n`,
    "duplicate job key",
  );
});
