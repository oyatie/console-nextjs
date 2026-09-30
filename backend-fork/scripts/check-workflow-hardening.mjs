#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { load } from "js-yaml";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const securityWorkflowPath = ".github/workflows/security.yml";

const CHECKOUT =
  "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0";
const SETUP_NODE =
  "actions/setup-node@48b55a011bda9f5d6aeb4c2d9c7362e8dae4041e";
const TRIVY_INSTALL = [
  'archive="${RUNNER_TEMP}/trivy_0.71.1_Linux-64bit.tar.gz"',
  'install_dir="${RUNNER_TEMP}/trivy-install"',
  "curl --disable --fail --silent --show-error --location \\",
  '  "https://github.com/aquasecurity/trivy/releases/download/v0.71.1/trivy_0.71.1_Linux-64bit.tar.gz" \\',
  '  --output "$archive"',
  "printf '%s  %s\\n' '3cbae37cd440cd8676e5ce9207fe460b5641c7579a17e9d00f8894928c41a88d' \"$archive\" | sha256sum --check",
  'mkdir -p "$install_dir"',
  'tar -xzf "$archive" -C "$install_dir" trivy',
  'sudo install -m 0755 "$install_dir/trivy" /usr/local/bin/trivy',
  "/usr/local/bin/trivy --version",
].join("\n");

const REQUIRED_WORKFLOW_ENVELOPE = Object.freeze({
  name: "Security",
  on: {
    // Required context: without merge_group it stays Pending for every merge-queue
    // entry and merges deadlock waiting on a check that never starts.
    merge_group: null,
    push: { branches: ["dev"] },
    pull_request: null,
    workflow_dispatch: null,
    schedule: [{ cron: "23 4 * * 1" }],
  },
  permissions: { contents: "read" },
  concurrency: {
    group:
      "${{ github.workflow }}-${{ github.event.pull_request.number || github.ref }}",
    "cancel-in-progress": "${{ github.event_name == 'pull_request' }}",
  },
});

const REQUIRED_SECURITY_JOBS = Object.freeze({
  filesystem: {
    name: "Trivy — dependencies + secrets",
    "runs-on": "ubuntu-latest",
    "timeout-minutes": 15,
    steps: [
      {
        uses: SETUP_NODE,
        with: { "node-version": "24" },
      },
      {
        name: "Install checksum-pinned Trivy before candidate checkout",
        run: TRIVY_INSTALL,
      },
      { uses: CHECKOUT },
      {
        name: "Trivy exception policy regression",
        run: "node --test scripts/generate-trivy-dev-codegen-exceptions.test.mjs",
      },
      {
        name: "Verify canonical Trivy exception projection",
        run: "node scripts/generate-trivy-dev-codegen-exceptions.mjs --check",
      },
      {
        name: "Trivy filesystem scan (vuln + secret, dev/codegen exceptions only)",
        run: [
          "/usr/local/bin/trivy fs --scanners vuln,secret --ignore-unfixed \\",
          "  --ignorefile security/trivy-dev-codegen-exceptions.yaml \\",
          "  --severity HIGH,CRITICAL --exit-code 1 .",
        ].join("\n"),
      },
    ],
  },
  iac: {
    name: "Trivy — IaC / manifest misconfig",
    "runs-on": "ubuntu-latest",
    "timeout-minutes": 15,
    steps: [
      {
        name: "Install checksum-pinned Trivy before candidate checkout",
        run: TRIVY_INSTALL,
      },
      { uses: CHECKOUT },
      {
        name: "Install kubectl (for kustomize renderer)",
        env: { KUBECTL_VERSION: "v1.36.2" },
        run: [
          'curl -fsSLO "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/linux/amd64/kubectl"',
          'curl -fsSLO "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/linux/amd64/kubectl.sha256"',
          'echo "$(cat kubectl.sha256)  kubectl" | sha256sum --check',
          "sudo install -m 0755 kubectl /usr/local/bin/kubectl",
          "kubectl version --client=true",
        ].join("\n"),
      },
      {
        name: "Trivy config scan (Dockerfiles + k8s manifests)",
        run: "/usr/local/bin/trivy config --severity HIGH,CRITICAL --exit-code 1 .",
      },
      {
        name: "Render and scan production manifests",
        run: [
          'scripts/render-k8s-manifests.sh "${RUNNER_TEMP}/rendered-k8s"',
          "npm run check:production-hardening",
          '/usr/local/bin/trivy config --severity HIGH,CRITICAL --exit-code 1 "${RUNNER_TEMP}/rendered-k8s"',
        ].join("\n"),
      },
    ],
  },
  "rust-advisories": {
    name: "cargo audit (RUSTSEC)",
    "runs-on": "ubuntu-latest",
    "timeout-minutes": 20,
    steps: [
      {
        name: "Install cargo-audit before candidate checkout",
        run: 'cargo install cargo-audit --locked --version 0.22.2 --root "${RUNNER_TEMP}/cargo-security-tools"',
      },
      { uses: CHECKOUT },
      {
        name: "Run cargo audit",
        "working-directory": "backend",
        run: [
          "printf '%s\\n' 'running cargo audit through the directly installed cargo-audit binary'",
          '"${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" audit --ignore RUSTSEC-2023-0071',
        ].join("\n"),
      },
    ],
  },
  "rust-supply-chain": {
    name: "cargo deny (licenses + sources + advisories)",
    "runs-on": "ubuntu-latest",
    "timeout-minutes": 20,
    steps: [
      {
        name: "Install cargo-deny before candidate checkout",
        run: 'cargo install cargo-deny --locked --version 0.19.9 --root "${RUNNER_TEMP}/cargo-security-tools"',
      },
      { uses: CHECKOUT },
      {
        run: [
          "printf '%s\\n' 'running cargo deny through the directly installed cargo-deny binary'",
          '"${RUNNER_TEMP}/cargo-security-tools/bin/cargo-deny" --manifest-path backend/Cargo.toml check',
        ].join("\n"),
      },
    ],
  },
  "node-advisories": {
    name: "npm audit",
    "runs-on": "ubuntu-latest",
    "timeout-minutes": 10,
    steps: [
      { uses: CHECKOUT },
      {
        uses: SETUP_NODE,
        with: { "node-version": "24", cache: "npm" },
      },
      { run: "npm ci --ignore-scripts" },
      {
        name: "Security workflow hardening regression",
        run: "node --test scripts/check-workflow-hardening.test.mjs",
      },
      {
        name: "Node audit exception policy regression",
        run: "node --test scripts/check-node-audit-exceptions.test.mjs",
      },
      {
        name: "Production dependency audit (no exceptions)",
        run: [
          'report="$(mktemp)"',
          'if npm audit --omit=dev --audit-level=high --json > "$report"; then true; else true; fi',
          'node scripts/check-node-audit-exceptions.mjs --mode production --audit-report "$report"',
        ].join("\n"),
      },
      {
        name: "Full dependency audit (strict dev/codegen exceptions only)",
        run: [
          'report="$(mktemp)"',
          'if npm audit --audit-level=high --json > "$report"; then true; else true; fi',
          'node scripts/check-node-audit-exceptions.mjs --mode dev-codegen --audit-report "$report"',
        ].join("\n"),
      },
    ],
  },
});

const REQUIRED_SECURITY_AGGREGATOR = Object.freeze({
  name: "Required / Security",
  needs: [
    "filesystem",
    "iac",
    "rust-advisories",
    "rust-supply-chain",
    "node-advisories",
  ],
  if: "${{ always() }}",
  "runs-on": "ubuntu-latest",
  "timeout-minutes": 5,
  steps: [{
    name: "Require every security proof to succeed",
    run: [
      'test "${{ needs.filesystem.result }}" = success &&',
      '  test "${{ needs.iac.result }}" = success &&',
      '  test "${{ needs.rust-advisories.result }}" = success &&',
      '  test "${{ needs.rust-supply-chain.result }}" = success &&',
      '  test "${{ needs.node-advisories.result }}" = success',
    ].join("\n"),
  }],
});

export const REQUIRED_SECURITY_CONTEXTS = Object.freeze(
  Object.entries(REQUIRED_SECURITY_JOBS).map(([job, definition]) =>
    Object.freeze({ job, name: definition.name }),
  ),
);

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function normalizedScalar(value, key) {
  if (key === "run" && typeof value === "string") {
    return value.replace(/\r\n/g, "\n").replace(/\n+$/, "");
  }
  return value;
}

function compareExact(actual, expected, path, failures, key = "") {
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual)) {
      failures.push(`${path} must be an ordered step list`);
      return;
    }
    if (actual.length !== expected.length) {
      failures.push(
        `${path} must contain exactly ${expected.length} entries in the locked order (found ${actual.length})`,
      );
    }
    const sharedLength = Math.min(actual.length, expected.length);
    for (let index = 0; index < sharedLength; index += 1) {
      compareExact(actual[index], expected[index], `${path}[${index}]`, failures);
    }
    return;
  }

  if (isObject(expected)) {
    if (!isObject(actual)) {
      failures.push(`${path} must be a mapping`);
      return;
    }
    const actualKeys = Object.keys(actual).sort();
    const expectedKeys = Object.keys(expected).sort();
    if (JSON.stringify(actualKeys) !== JSON.stringify(expectedKeys)) {
      failures.push(
        `${path} keys must be exactly ${expectedKeys.join(", ")} (found ${actualKeys.join(", ") || "none"})`,
      );
    }
    for (const expectedKey of expectedKeys) {
      compareExact(
        actual[expectedKey],
        expected[expectedKey],
        `${path}.${expectedKey}`,
        failures,
        expectedKey,
      );
    }
    return;
  }

  const normalizedActual = normalizedScalar(actual, key);
  const normalizedExpected = normalizedScalar(expected, key);
  if (normalizedActual !== normalizedExpected) {
    failures.push(
      `${path} must equal ${JSON.stringify(normalizedExpected)} (found ${JSON.stringify(normalizedActual)})`,
    );
  }
}

function requireAbsent(mapping, key, path, failures) {
  if (isObject(mapping) && Object.hasOwn(mapping, key)) {
    failures.push(`${path}.${key} is forbidden on a required security proof`);
  }
}

export function evaluateSecurityWorkflowHardening(workflowText) {
  const failures = [];
  const passes = [];
  let workflow;
  try {
    workflow = load(workflowText, { json: false });
  } catch (error) {
    return {
      failures: [`${securityWorkflowPath}: invalid or duplicate-key YAML (${error.message})`],
      passes,
    };
  }

  if (!isObject(workflow)) {
    return {
      failures: [`${securityWorkflowPath}: workflow root must be a mapping`],
      passes,
    };
  }

  requireAbsent(workflow, "env", "security workflow", failures);
  requireAbsent(workflow, "defaults", "security workflow", failures);
  const topLevelKeys = Object.keys(workflow).sort();
  const expectedTopLevelKeys = [
    ...Object.keys(REQUIRED_WORKFLOW_ENVELOPE),
    "jobs",
  ].sort();
  if (JSON.stringify(topLevelKeys) !== JSON.stringify(expectedTopLevelKeys)) {
    failures.push(
      `security workflow keys must be exactly ${expectedTopLevelKeys.join(", ")} (found ${topLevelKeys.join(", ")})`,
    );
  }
  for (const [key, expected] of Object.entries(REQUIRED_WORKFLOW_ENVELOPE)) {
    compareExact(workflow[key], expected, `security.${key}`, failures, key);
  }

  const jobs = workflow.jobs;
  if (!isObject(jobs)) {
    failures.push("security workflow jobs must be a mapping");
    return { failures, passes };
  }

  const actualJobIds = Object.keys(jobs).sort();
  const requiredJobIds = [
    ...Object.keys(REQUIRED_SECURITY_JOBS),
    "required-security",
  ].sort();
  if (JSON.stringify(actualJobIds) !== JSON.stringify(requiredJobIds)) {
    failures.push(
      `security workflow job ids must be exactly ${requiredJobIds.join(", ")} (found ${actualJobIds.join(", ") || "none"})`,
    );
  }

  compareExact(
    jobs["required-security"],
    REQUIRED_SECURITY_AGGREGATOR,
    "security.jobs.required-security",
    failures,
  );

  const contextNames = new Set();
  for (const { job, name } of REQUIRED_SECURITY_CONTEXTS) {
    const definition = jobs[job];
    if (!isObject(definition)) {
      failures.push(`security.jobs.${job} is missing`);
      continue;
    }

    if (contextNames.has(definition.name)) {
      failures.push(`security.jobs.${job}.name duplicates required context ${JSON.stringify(definition.name)}`);
    }
    contextNames.add(definition.name);
    if (definition.name !== name) {
      failures.push(`security.jobs.${job}.name must remain the required context ${JSON.stringify(name)}`);
    }

    for (const forbidden of [
      "if",
      "continue-on-error",
      "env",
      "defaults",
      "container",
      "services",
      "strategy",
      "needs",
      "permissions",
    ]) {
      requireAbsent(definition, forbidden, `security.jobs.${job}`, failures);
    }

    if (Array.isArray(definition.steps)) {
      for (const [index, step] of definition.steps.entries()) {
        if (!isObject(step)) {
          failures.push(`security.jobs.${job}.steps[${index}] must be a mapping`);
          continue;
        }
        for (const forbidden of ["if", "continue-on-error", "shell"]) {
          requireAbsent(step, forbidden, `security.jobs.${job}.steps[${index}]`, failures);
        }
        if (
          Object.hasOwn(step, "env") &&
          !(job === "iac" &&
            index === 2 &&
            JSON.stringify(step.env) === JSON.stringify({ KUBECTL_VERSION: "v1.36.2" }))
        ) {
          failures.push(
            `security.jobs.${job}.steps[${index}].env is not the exact kubectl-version allowlist`,
          );
        }
      }
    }

    compareExact(
      definition,
      REQUIRED_SECURITY_JOBS[job],
      `security.jobs.${job}`,
      failures,
    );
  }

  if (failures.length === 0) {
    passes.push(
      `security workflow hardening: ${REQUIRED_SECURITY_CONTEXTS.length} required contexts have exact ordered proofs`,
    );
  }
  return { failures: [...new Set(failures)], passes };
}

function runCli() {
  const workflow = readFileSync(resolve(root, securityWorkflowPath), "utf8");
  const result = evaluateSecurityWorkflowHardening(workflow);
  if (result.failures.length > 0) {
    console.error(
      `Workflow hardening check failed:\n${result.failures.map((failure) => `- ${failure}`).join("\n")}`,
    );
    process.exitCode = 1;
    return;
  }
  console.log(result.passes.join("\n"));
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  runCli();
}
