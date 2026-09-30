import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

import {
  evaluateCnpgContextChecks,
  evaluateDeployAutomationChecks,
  evaluateExpandContractReleaseChecks,
  evaluateOnPremHaContextChecks,
  evaluateProdOverlayImageChecks,
  evaluateSmtpDeploymentChecks,
  evaluateWorkflowHardeningChecks,
} from "./check-production-hardening.mjs";

describe("production authority blocked observation static integration", () => {
  it("requires the explicit-SHA evaluator package wiring and focused hardening suite inclusion", () => {
    const pkg = JSON.parse(
      readFileSync(new URL("../package.json", import.meta.url), "utf8"),
    );
    assert.equal(
      pkg.scripts["check:production-authority-blocked"],
      "node scripts/check-production-authority-blocked.mjs",
    );
    assert.match(
      pkg.scripts["test:production-hardening"],
      /scripts\/check-production-authority-blocked\.test\.mjs/,
    );
    assert.ok(
      existsSync(
        new URL("./check-production-authority-blocked.mjs", import.meta.url),
      ),
    );
  });
});

describe("post-pivot production operation HOLD", () => {
  const guardedOperations = [
    ["../ops/backup/backup.sh", "compose-production-backup", "compose_args=(-p"],
    ["../ops/backup/restore-drill.sh", "compose-restore-drill", "compose_args=(-p"],
    ["../ops/dr/pitr-drill.sh", "compose-pitr-drill", "source_compose_args=(-p"],
    ["../ops/dr/cnpg-restore-drill.sh", "cnpg-restore-drill", "recovery_cluster="],
  ];

  it("hard-fails every retained backup/restore entrypoint before substantive setup", () => {
    for (const [relativePath, operation, firstSubstantiveSetup] of guardedOperations) {
      const source = readFileSync(new URL(relativePath, import.meta.url), "utf8");
      const guard = source.indexOf("require-production-operation-authority.sh");
      assert.notEqual(guard, -1, `${relativePath}: missing shared authority guard`);
      assert.match(source.slice(guard, guard + 180), new RegExp(operation));
      assert.ok(
        guard < source.indexOf(firstSubstantiveSetup),
        `${relativePath}: authority guard must precede substantive setup`,
      );
    }
  });

  it("the shared guard has no success path on this candidate", () => {
    const guardUrl = new URL("./require-production-operation-authority.sh", import.meta.url);
    const source = readFileSync(guardUrl, "utf8");
    assert.match(source, /production_operation_authority=blocked/);
    assert.match(source, /exit 78\s*$/);

    const result = spawnSync("bash", [fileURLToPath(guardUrl), "test-operation"], {
      encoding: "utf8",
    });
    assert.equal(result.status, 78);
    assert.match(result.stderr, /operation=test-operation/);
    assert.match(result.stderr, /2026-08-03-disk-wipe-consolidation\.md/);
  });
});

const validFiles = {
  "deploy/apps/console/base/database.yaml": `apiVersion: postgresql.cnpg.io/v1
kind: Cluster
metadata:
  name: console-db
spec:
  instances: 1 # single oci-guest node
  env:
    - name: AWS_REQUEST_CHECKSUM_CALCULATION
      value: when_required
    - name: AWS_RESPONSE_CHECKSUM_VALIDATION
      value: when_required
  storage:
    size: 5Gi
`,
  "deploy/apps/console/overlays/prod/kustomization.yaml": `apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  - ../../base
images: []
`,
  "deploy/apps/console/overlays/on-prem/kustomization.yaml": `apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  - ../../base
patches:
  - target:
      group: postgresql.cnpg.io
      version: v1
      kind: Cluster
      name: console-db
    path: cnpg-ha-patch.yaml
  - target: { kind: ObjectStore, name: console-backups }
    patch: |-
      - op: replace
        path: /spec/configuration/endpointURL
        value: http://console-object-store-s3.console-object-store.svc.cluster.local:8333
`,
  "deploy/apps/console/overlays/on-prem/cnpg-ha-patch.yaml": `- op: remove
  path: /spec/env
- op: replace
  path: /spec/instances
  value: 3
- op: add
  path: /spec/storage/storageClass
  value: console-pg-hot
- op: add
  path: /spec/postgresql/synchronous
  value:
    failoverQuorum: true
- op: add
  path: /spec/topologySpreadConstraints
  value:
    - topologyKey: kubernetes.io/hostname
`,
  "deploy/apps/storage/manifests/storageclass-console-pg-hot.yaml": `apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: console-pg-hot
provisioner: driver.longhorn.io
parameters:
  numberOfReplicas: "3"
reclaimPolicy: Retain
volumeBindingMode: WaitForFirstConsumer
`,
};

function evaluate(overrides = {}) {
  const files = { ...validFiles, ...overrides };
  return evaluateCnpgContextChecks((path) => files[path] ?? "");
}

function assertHasFailure(result, fragment) {
  assert.ok(
    result.failures.some((failure) => failure.includes(fragment)),
    `expected failure containing ${JSON.stringify(fragment)}; got ${JSON.stringify(result.failures)}`,
  );
}

function replaceLast(text, needle, replacement) {
  const index = text.lastIndexOf(needle);
  return index < 0
    ? text
    : `${text.slice(0, index)}${replacement}${text.slice(index + needle.length)}`;
}

const smtpConfigMapWithRelayFields = `apiVersion: v1
kind: ConfigMap
metadata:
  name: console-config
data:
  CONSOLE_EMAIL_SMTP_HOST: "smtp.email.ap-chuncheon-1.oci.oraclecloud.com"
  CONSOLE_EMAIL_SMTP_PORT: "587"
  CONSOLE_EMAIL_FROM: "no-reply@knllogistic.com"
  CONSOLE_EMAIL_FROM_NAME: "Console 정비 콘솔"
`;

const smtpConfigMapForDevE2eStub = `apiVersion: v1
kind: ConfigMap
metadata:
  name: console-config
data:
  CONSOLE_HTTP_ADDR: "0.0.0.0:8080"
  CONSOLE_EMAIL_STUB_MODE: "e2e"
  # CONSOLE_EMAIL_* intentionally omitted: dev/e2e uses the explicit stub sender path.
`;

const smtpConfigMapWithoutRelayOrStub = `apiVersion: v1
kind: ConfigMap
metadata:
  name: console-config
data:
  CONSOLE_HTTP_ADDR: "0.0.0.0:8080"
`;

const workloadWithEnvFromOnly = `apiVersion: apps/v1
kind: Deployment
metadata:
  name: console-app
spec:
  template:
    spec:
      containers:
        - name: console-app
          envFrom:
            - configMapRef: { name: console-config }
            - secretRef: { name: console-secrets }
`;

const workloadWithRequiredSmtpSecretRefs = `apiVersion: apps/v1
kind: Deployment
metadata:
  name: console-app
spec:
  template:
    spec:
      containers:
        - name: console-app
          envFrom:
            - configMapRef: { name: console-config }
            - secretRef: { name: console-secrets }
          env:
            - name: CONSOLE_EMAIL_SMTP_USERNAME
              valueFrom:
                secretKeyRef: { name: console-secrets, key: CONSOLE_EMAIL_SMTP_USERNAME }
            - name: CONSOLE_EMAIL_SMTP_PASSWORD
              valueFrom:
                secretKeyRef:
                  name: console-secrets
                  key: CONSOLE_EMAIL_SMTP_PASSWORD
`;

function evaluateSmtp(overrides = {}) {
  const files = {
    "deploy/apps/console/base/configmap.yaml": smtpConfigMapWithRelayFields,
    "deploy/apps/console/base/backend.yaml": workloadWithEnvFromOnly,
    "deploy/apps/console/base/worker.yaml": workloadWithEnvFromOnly,
    ...overrides,
  };
  return evaluateSmtpDeploymentChecks((path) => files[path] ?? "");
}

describe("production hardening SMTP deployment config", () => {
  it("rejects production-like SMTP non-secret fields without explicit required credential refs", () => {
    const result = evaluateSmtp();

    assertHasFailure(
      result,
      "deploy/apps/console/base/backend.yaml must explicitly require CONSOLE_EMAIL_SMTP_USERNAME",
    );
    assertHasFailure(
      result,
      "deploy/apps/console/base/backend.yaml must explicitly require CONSOLE_EMAIL_SMTP_PASSWORD",
    );
    assertHasFailure(
      result,
      "deploy/apps/console/base/worker.yaml must explicitly require CONSOLE_EMAIL_SMTP_USERNAME",
    );
  });

  it("accepts complete SMTP config with required secret-backed credentials", () => {
    const result = evaluateSmtp({
      "deploy/apps/console/base/backend.yaml":
        workloadWithRequiredSmtpSecretRefs,
      "deploy/apps/console/base/worker.yaml":
        workloadWithRequiredSmtpSecretRefs,
    });

    assert.deepEqual(result.failures, []);
    assert.match(
      result.passes.join("\n"),
      /SMTP production credential refs: console-app, console-worker/,
    );
  });

  it("does not block explicit dev/e2e stub configs that omit SMTP relay fields", () => {
    const result = evaluateSmtp({
      "deploy/apps/console/base/configmap.yaml": smtpConfigMapForDevE2eStub,
    });

    assert.deepEqual(result.failures, []);
    assert.match(
      result.passes.join("\n"),
      /SMTP relay disabled for explicit stub mode CONSOLE_EMAIL_STUB_MODE=e2e/,
    );
  });

  it("rejects no-relay production-like configs without explicit stub mode", () => {
    const result = evaluateSmtp({
      "deploy/apps/console/base/configmap.yaml":
        smtpConfigMapWithoutRelayOrStub,
    });

    assertHasFailure(
      result,
      "must either configure non-secret CONSOLE_EMAIL_* SMTP relay fields or set CONSOLE_EMAIL_STUB_MODE",
    );
  });
});

describe("production hardening CNPG context checks", () => {
  it("accepts the oci-guest single-instance base and the on-prem HA overlay", () => {
    const result = evaluate();

    assert.deepEqual(result.failures, []);
    assert.match(result.passes.join("\n"), /oci-guest CNPG base instances: 1/);
    assert.match(result.passes.join("\n"), /on-prem-ha CNPG HA instances: 3/);
    assert.match(result.passes.join("\n"), /on-prem-ha storage replicas: 3/);
  });

  it("rejects an HA overlay with fewer than three CNPG instances", () => {
    const result = evaluate({
      "deploy/apps/console/overlays/on-prem/cnpg-ha-patch.yaml": `- op: replace
  path: /spec/instances
  value: 2
- op: add
  path: /spec/storage/storageClass
  value: console-pg-hot
`,
    });

    assert.ok(
      result.failures.some((failure) =>
        failure.includes("on-prem-ha CNPG HA instances"),
      ),
    );
  });

  it("rejects an HA overlay that points CNPG at local-path storage", () => {
    const result = evaluate({
      "deploy/apps/console/overlays/on-prem/cnpg-ha-patch.yaml": `- op: replace
  path: /spec/instances
  value: 3
- op: add
  path: /spec/storage/storageClass
  value: local-path
`,
    });

    assert.ok(
      result.failures.some((failure) =>
        failure.includes("must not use local-path"),
      ),
    );
  });

  it("rejects an on-prem overlay that inherits the OCI-only checksum workaround", () => {
    const result = evaluate({
      "deploy/apps/console/overlays/on-prem/cnpg-ha-patch.yaml": `- op: replace
  path: /spec/instances
  value: 3
- op: add
  path: /spec/storage/storageClass
  value: console-pg-hot
`,
    });

    assert.ok(
      result.failures.some((failure) => failure.includes("checksum behavior")),
    );
  });

  it("rejects removing /spec/env when the base env contains non-checksum entries", () => {
    const result = evaluate({
      "deploy/apps/console/base/database.yaml": validFiles[
        "deploy/apps/console/base/database.yaml"
      ].replace(
        "  storage:",
        `    - name: CONSOLE_REQUIRED_CLUSTER_SETTING
      value: keep
  storage:`,
      ),
    });

    assertHasFailure(result, "may remove /spec/env only while");
    assertHasFailure(result, "CONSOLE_REQUIRED_CLUSTER_SETTING");
  });

  it("rejects an oci-guest prod overlay that patches the CNPG instance shape", () => {
    const result = evaluate({
      "deploy/apps/console/overlays/prod/kustomization.yaml": `apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  - ../../base
patches:
  - target: { kind: Cluster, name: console-db }
    patch: |-
      - op: replace
        path: /spec/instances
        value: 3
`,
    });

    assert.ok(
      result.failures.some((failure) =>
        failure.includes("oci-guest prod overlay CNPG shape"),
      ),
    );
  });

  it("keeps the oci-guest/base CNPG posture single-instance", () => {
    const result = evaluate({
      "deploy/apps/console/base/database.yaml": `apiVersion: postgresql.cnpg.io/v1
kind: Cluster
metadata:
  name: console-db
spec:
  instances: 3
`,
    });

    assert.ok(
      result.failures.some((failure) =>
        failure.includes("oci-guest CNPG base instances"),
      ),
    );
  });
});

describe("production hardening on-prem authority identity", () => {
  it("rejects an on-prem runbook that does not explicitly identify ADR-0024", () => {
    const files = {
      "deploy/OPS-RUNBOOK-baremetal.md": "# On-prem bare-metal operations\n",
    };

    const result = evaluateOnPremHaContextChecks((path) => files[path] ?? "");

    assertHasFailure(result, "on-prem-ha runbook identity: explicit ADR-0024");
  });
});

function evaluateProdOverlay(text) {
  return evaluateProdOverlayImageChecks((path) =>
    path === "deploy/apps/console/overlays/prod/kustomization.yaml"
      ? text
      : "",
  );
}

describe("production hardening global image checks", () => {
  it("accepts immutable digest pins without mutable tags", () => {
    const digestA = "a".repeat(64);
    const result = evaluateProdOverlay(`images:
  - name: console-app
    digest: sha256:${digestA}
`);

    assert.deepEqual(result.failures, []);
    assert.match(result.passes.join("\n"), /prod overlay digest pins: 1/);
  });

  it("rejects missing digest pins and mutable image tags", () => {
    const result = evaluateProdOverlay(`images:
  - name: console-app
    newTag: latest
`);

    assert.ok(
      result.failures.some((failure) =>
        failure.includes("must pin at least console-app"),
      ),
    );
    assert.ok(
      result.failures.some((failure) =>
        failure.includes("must not use mutable newTag values"),
      ),
    );
  });

  it("rejects digest pins and mutable tags that appear only in comments", () => {
    const digestA = "a".repeat(64);
    const result = evaluateProdOverlay(`images:
  # - name: console-app
  #   digest: sha256:${digestA}
  # newTag: latest
`);

    assert.ok(
      result.failures.some((failure) =>
        failure.includes("must pin at least console-app"),
      ),
    );
    assert.deepEqual(
      result.failures.filter((failure) =>
        failure.includes("must not use mutable newTag values"),
      ),
      [],
    );
  });
});

const pr473ManifestText = readFileSync(
  new URL("../docs/release/PR-473-EXPAND-CONTRACT.gate.json", import.meta.url),
  "utf8",
);
const pr473Directives = `<!-- PR473-MIGRATION-GATE: release_phase=expand -->
<!-- PR473-MIGRATION-GATE: deployment_authorized=false -->
<!-- PR473-MIGRATION-GATE: command_only_claim_authorized=false -->
<!-- PR473-MIGRATION-GATE: production_authority=production_cardinality,old_runtime_drain,rollback_floor_raise -->`;
const validPr473Files = {
  "docs/release/PR-473-EXPAND-CONTRACT.gate.json": pr473ManifestText,
  "docs/release/PR-473-ONTOLOGY-EXPAND-CONTRACT.md": `# PR 473 Ontology Expand Contract\n\n${pr473Directives}\n\n`,
  "docs/release/PR-473-EMPLOYEE-IMPORT-EXPAND-CONTRACT.md": `# PR 473 Employee-Import Expand Contract\n\n${pr473Directives}\n\n`,
  "package.json": JSON.stringify({
    scripts: {
      "check:pr473-migration-operational":
        "python3 scripts/check-pr473-migration-operational.py",
      "test:pr473-migration-operational":
        "python3 scripts/check-pr473-migration-operational.test.py",
    },
  }),
  ".github/workflows/ci.yml": `jobs:
  migration-expand-contract:
    steps:
      - name: Reconcile portable PostgreSQL role topology
        run: |
          APP_PASSWORD="$(openssl rand -hex 32)"
          RT_PASSWORD="$(openssl rand -hex 32)"
          LEAVE_COMMAND_PASSWORD="$(openssl rand -hex 32)"
          ONTOLOGY_COMMAND_PASSWORD="$(openssl rand -hex 32)"
          PLATFORM_FORCE_COMMAND_PASSWORD="$(openssl rand -hex 32)"
          docker run --rm --network host \
            -v "$GITHUB_WORKSPACE/ops/postgres-reconcile-topology.sh:/usr/local/bin/postgres-reconcile-topology:ro" \
            -e POSTGRES_HOST=127.0.0.1 -e POSTGRES_DB=console_ci \
            -e POSTGRES_ADMIN_USER=postgres -e POSTGRES_ADMIN_PASSWORD=postgres \
            -e CONSOLE_APP_POSTGRES_PASSWORD="$APP_PASSWORD" \
            -e CONSOLE_RT_POSTGRES_PASSWORD="$RT_PASSWORD" \
            -e CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD="$LEAVE_COMMAND_PASSWORD" \
            -e CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD="$ONTOLOGY_COMMAND_PASSWORD" \
            -e CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD="$PLATFORM_FORCE_COMMAND_PASSWORD" \
            --entrypoint bash postgres:18.4@sha256:4aabea78cf39b90e834caf3af7d602a18565f6fe2508705c8d01aa63245c2e20 \
            /usr/local/bin/postgres-reconcile-topology

          docker run --rm --network host \
            -e PGPASSWORD=postgres \
            --entrypoint psql \
            postgres:18.4@sha256:4aabea78cf39b90e834caf3af7d602a18565f6fe2508705c8d01aa63245c2e20 \
            -h 127.0.0.1 -U postgres -d postgres -v ON_ERROR_STOP=1 \
            -c "DROP DATABASE IF EXISTS console_apalis_contract WITH (FORCE)" \
            -c "CREATE DATABASE console_apalis_contract OWNER console_app"

          BUCK_ADMIN_PASSWORD="$(openssl rand -hex 32)"
          umask 077
          printf "CREATE ROLE console_buck_admin SUPERUSER LOGIN PASSWORD '%s';\\n" \
            "$BUCK_ADMIN_PASSWORD" > "$RUNNER_TEMP/buck-admin.sql"
          docker run --rm --network host \
            -e PGPASSWORD=postgres \
            -v "$RUNNER_TEMP/buck-admin.sql:/buck-admin.sql:ro" \
            --entrypoint psql \
            postgres:18.4@sha256:4aabea78cf39b90e834caf3af7d602a18565f6fe2508705c8d01aa63245c2e20 \
            -h 127.0.0.1 -U postgres -d postgres -v ON_ERROR_STOP=1 -f /buck-admin.sql
          rm -f "$RUNNER_TEMP/buck-admin.sql"

          echo "::add-mask::$APP_PASSWORD"
          echo "::add-mask::$RT_PASSWORD"
          echo "::add-mask::$BUCK_ADMIN_PASSWORD"
          {
            echo "CONSOLE_BUCK_ADMIN_DATABASE_URL=postgres://console_buck_admin:\${BUCK_ADMIN_PASSWORD}@localhost:5432/console_ci?options%5Bconsole.sqlx_test_bootstrap%5D=buck-sqlx-superuser-v1"
            echo "CONSOLE_APALIS_OWNER_DATABASE_URL=postgres://console_app:\${APP_PASSWORD}@localhost:5432/console_apalis_contract"
            echo "CONSOLE_APALIS_RUNTIME_DATABASE_URL=postgres://console_rt:\${RT_PASSWORD}@localhost:5432/console_apalis_contract"
            echo "CONSOLE_APALIS_ADMIN_DATABASE_URL=postgres://postgres:postgres@localhost:5432/console_apalis_contract"
          } >> "$GITHUB_ENV"
      - name: Expand/contract migration rehearsal (0165, 0166)
        working-directory: .
        run: npm run check:pr473-migration-operational
`,
};

function evaluatePr473(overrides = {}) {
  const files = { ...validPr473Files, ...overrides };
  return evaluateExpandContractReleaseChecks((path) => files[path] ?? "");
}

describe("production hardening PR 473 typed operational gate", () => {
  it("accepts the canonical typed manifest, directives, alias, and ordered CI wrapper", () => {
    assert.deepEqual(evaluatePr473().failures, []);
  });

  it("rejects malformed and nondeploy-mutated manifests", () => {
    const malformed = evaluatePr473({
      "docs/release/PR-473-EXPAND-CONTRACT.gate.json": "{",
    });
    assertHasFailure(malformed, "must be valid JSON");

    const manifest = JSON.parse(pr473ManifestText);
    manifest.deployment_authorized = true;
    const mutated = evaluatePr473({
      "docs/release/PR-473-EXPAND-CONTRACT.gate.json": `${JSON.stringify(manifest, null, 2)}\n`,
    });
    assertHasFailure(mutated, "nondeploy must be exactly false");
  });

  it("rejects duplicate and substituted guarded test tuples", () => {
    const duplicate = JSON.parse(pr473ManifestText);
    duplicate.guarded_tests[10] = { ...duplicate.guarded_tests[0] };
    const duplicateResult = evaluatePr473({
      "docs/release/PR-473-EXPAND-CONTRACT.gate.json": `${JSON.stringify(duplicate, null, 2)}\n`,
    });
    assertHasFailure(duplicateResult, "11 unique tuples");
    assertHasFailure(
      duplicateResult,
      "exact expected 3 ontology and 8 leave tuples",
    );

    const substituted = JSON.parse(pr473ManifestText);
    substituted.guarded_tests[0].name = "invented_unique_test";
    const substitutedResult = evaluatePr473({
      "docs/release/PR-473-EXPAND-CONTRACT.gate.json": `${JSON.stringify(substituted, null, 2)}\n`,
    });
    assertHasFailure(
      substitutedResult,
      "exact expected 3 ontology and 8 leave tuples",
    );
  });

  it("rejects missing and duplicated canonical document directives", () => {
    const missing = evaluatePr473({
      "docs/release/PR-473-ONTOLOGY-EXPAND-CONTRACT.md":
        pr473Directives.replace(
          "<!-- PR473-MIGRATION-GATE: deployment_authorized=false -->",
          "",
        ),
    });
    assertHasFailure(missing, "deployment_authorized=false");

    const duplicated = evaluatePr473({
      "docs/release/PR-473-EMPLOYEE-IMPORT-EXPAND-CONTRACT.md": `${pr473Directives}\n${pr473Directives}`,
    });
    assertHasFailure(duplicated, "found 2");

    const nested = evaluatePr473({
      "docs/release/PR-473-ONTOLOGY-EXPAND-CONTRACT.md": `# PR 473 Ontology Expand Contract\n\n> ${pr473Directives.replaceAll("\n", "\n> ")}\n`,
    });
    assertHasFailure(nested, "canonical block immediately after");
  });

  it("rejects a commented-out or duplicated workflow invocation", () => {
    const commented = evaluatePr473({
      ".github/workflows/ci.yml": validPr473Files[
        ".github/workflows/ci.yml"
      ].replace(
        "        run: npm run check:pr473-migration-operational",
        "        # run: npm run check:pr473-migration-operational",
      ),
    });
    assertHasFailure(commented, "exactly one active");

    const duplicated = evaluatePr473({
      ".github/workflows/ci.yml": `${validPr473Files[".github/workflows/ci.yml"]}
      - name: duplicate
        run: npm run check:pr473-migration-operational
`,
    });
    assertHasFailure(duplicated, "found 2");

    for (const command of [
      "echo npm run check:pr473-migration-operational",
      "npm run check:pr473-migration-operational-evil",
    ]) {
      const spoofed = evaluatePr473({
        ".github/workflows/ci.yml": validPr473Files[
          ".github/workflows/ci.yml"
        ].replace("npm run check:pr473-migration-operational", command),
      });
      assertHasFailure(spoofed, "exactly one active");
    }

    const relocated = evaluatePr473({
      ".github/workflows/ci.yml": `${validPr473Files[
        ".github/workflows/ci.yml"
      ].replace("npm run check:pr473-migration-operational", "echo disabled")}
      - name: unrelated exact command
        run: npm run check:pr473-migration-operational
`,
    });
    assertHasFailure(relocated, "named CI wrapper step must run exactly");
  });

  it("rejects a wrapper step before topology and an inexact package alias", () => {
    const beforeTopology = evaluatePr473({
      ".github/workflows/ci.yml": `steps:
  - name: Expand/contract migration rehearsal (0165, 0166)
    working-directory: .
    run: npm run check:pr473-migration-operational
  - name: Reconcile portable PostgreSQL role topology
    run: ./ops/postgres-reconcile-topology.sh
`,
    });
    assertHasFailure(beforeTopology, "migration-expand-contract job must contain");

    const alias = evaluatePr473({
      "package.json": JSON.stringify({
        scripts: {
          "check:pr473-migration-operational":
            "python scripts/check-pr473-migration-operational.py",
        },
      }),
    });
    assertHasFailure(alias, "package alias must be exactly");
  });

  it("binds the exact topology command and wrapper ordering to the rehearsal job", () => {
    const crossJob = evaluatePr473({
      ".github/workflows/ci.yml": validPr473Files[".github/workflows/ci.yml"]
        .replace(
          "  migration-expand-contract:\n    steps:\n      - name: Reconcile portable PostgreSQL role topology",
          "  topology-only:\n    steps:\n      - name: Reconcile portable PostgreSQL role topology",
        )
        .replace(
          "      - name: Expand/contract migration rehearsal (0165, 0166)",
          "  migration-expand-contract:\n    steps:\n      - name: Expand/contract migration rehearsal (0165, 0166)",
        ),
    });
    assertHasFailure(crossJob, "migration-expand-contract job must contain");

    const fakeTopology = evaluatePr473({
      ".github/workflows/ci.yml": validPr473Files[
        ".github/workflows/ci.yml"
      ].replace(
        "          /usr/local/bin/postgres-reconcile-topology",
        "          echo topology-disabled",
      ),
    });
    assertHasFailure(fakeTopology, "must invoke the exact reconcile command");

    const missingPlatformForceTopology = evaluatePr473({
      ".github/workflows/ci.yml": validPr473Files[
        ".github/workflows/ci.yml"
      ]
        .replace(
          '          PLATFORM_FORCE_COMMAND_PASSWORD="$(openssl rand -hex 32)"\n',
          "",
        )
        .replace(
          '            -e CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD="$PLATFORM_FORCE_COMMAND_PASSWORD" \\\n',
          "",
        ),
    });
    assertHasFailure(
      missingPlatformForceTopology,
      "must invoke the exact reconcile command",
    );
  });

  it("rejects shell-control topology command bypasses", () => {
    const command = '          APP_PASSWORD="$(openssl rand -hex 32)"';
    for (const replacement of [
      `          true || ${command.trim()}`,
      `${command} && true`,
    ]) {
      const result = evaluatePr473({
        ".github/workflows/ci.yml": validPr473Files[
          ".github/workflows/ci.yml"
        ].replace(command, replacement),
      });
      assertHasFailure(result, "must invoke the exact reconcile command");
    }
  });

  it("rejects an inexact Apalis contract database name", () => {
    const result = evaluatePr473({
      ".github/workflows/ci.yml": validPr473Files[
        ".github/workflows/ci.yml"
      ].replace(
        "CREATE DATABASE console_apalis_contract OWNER console_app",
        "CREATE DATABASE apalis_contract OWNER console_app",
      ),
    });

    assertHasFailure(result, "Apalis database provisioning command");
  });

  it("rejects an Apalis contract database not owned by console_app", () => {
    const result = evaluatePr473({
      ".github/workflows/ci.yml": validPr473Files[
        ".github/workflows/ci.yml"
      ].replace(
        "CREATE DATABASE console_apalis_contract OWNER console_app",
        "CREATE DATABASE console_apalis_contract OWNER postgres",
      ),
    });

    assertHasFailure(result, "Apalis database provisioning command");
  });

  it("rejects an unpinned PostgreSQL image for Apalis provisioning", () => {
    const pinnedImage =
      "postgres:18.4@sha256:4aabea78cf39b90e834caf3af7d602a18565f6fe2508705c8d01aa63245c2e20";
    const result = evaluatePr473({
      ".github/workflows/ci.yml": replaceLast(
        validPr473Files[".github/workflows/ci.yml"],
        pinnedImage,
        "postgres:18.4",
      ),
    });

    assertHasFailure(result, "pinned PostgreSQL image");
  });

  it("requires all three exact Apalis database URL exports", () => {
    for (const variable of [
      "CONSOLE_APALIS_OWNER_DATABASE_URL",
      "CONSOLE_APALIS_RUNTIME_DATABASE_URL",
      "CONSOLE_APALIS_ADMIN_DATABASE_URL",
    ]) {
      const result = evaluatePr473({
        ".github/workflows/ci.yml": validPr473Files[
          ".github/workflows/ci.yml"
        ].replace(variable, `${variable}_RENAMED`),
      });

      assertHasFailure(result, "URL exports");
    }
  });

  it("requires both generated Apalis role passwords to be masked", () => {
    for (const password of ["APP_PASSWORD", "RT_PASSWORD"]) {
      const result = evaluatePr473({
        ".github/workflows/ci.yml": validPr473Files[
          ".github/workflows/ci.yml"
        ].replace(`          echo "::add-mask::$${password}"\n`, ""),
      });

      assertHasFailure(result, "masking");
    }
  });
});

const validProductionEvidenceText = `${JSON.stringify(
  {
    schema_version: 1,
    target: "production",
    release_phase: "expand",
    candidate_source_sha: "0".repeat(40),
    observed_running_revision: "0".repeat(40),
    observed_database_topology: {
      cluster_name: "TEMPLATE_NOT_EVIDENCE",
      namespace: "TEMPLATE_NOT_EVIDENCE",
      writer_endpoint: "TEMPLATE_NOT_EVIDENCE",
      reader_endpoint: "TEMPLATE_NOT_EVIDENCE",
      instances: [],
    },
    capacity_headroom: {
      window_started_at: "TEMPLATE_NOT_EVIDENCE",
      window_ended_at: "TEMPLATE_NOT_EVIDENCE",
      cpu_peak_percent: 0,
      memory_peak_percent: 0,
      storage_used_percent: 0,
      connection_peak: 0,
      connection_limit: 0,
      minimum_headroom_percent: 0,
    },
    backup_restore_proof: {
      backup_id: "TEMPLATE_NOT_EVIDENCE",
      backup_completed_at: "TEMPLATE_NOT_EVIDENCE",
      isolated_restore_id: "TEMPLATE_NOT_EVIDENCE",
      isolated_restore_completed_at: "TEMPLATE_NOT_EVIDENCE",
      restored_revision: "0".repeat(40),
      validation_checks: [],
    },
    evidence_author: {
      github_login: "TEMPLATE_NOT_EVIDENCE",
      identity_provider_subject: "TEMPLATE_NOT_EVIDENCE",
    },
    independent_reviewer: {
      github_login: "TEMPLATE_NOT_EVIDENCE",
      identity_provider_subject: "TEMPLATE_NOT_EVIDENCE",
      team_id: 0,
    },
    charter: {
      charter_id: "TEMPLATE_NOT_EVIDENCE",
      trust_domain_id: "TEMPLATE_NOT_EVIDENCE",
    },
    observed_at: "TEMPLATE_NOT_EVIDENCE",
    prepared_at: "TEMPLATE_NOT_EVIDENCE",
    reviewed_at: "TEMPLATE_NOT_EVIDENCE",
  },
  null,
  2,
)}\n`;

const validWorkflowFiles = {
  "scripts/check-production-authority-blocked.mjs": "#!/usr/bin/env node\n",
  "package.json": JSON.stringify({
    scripts: {
      "test:production-hardening":
        "npm run test:pr473-migration-operational && python3 scripts/check-production-promotion-authority.test.py && node --test scripts/check-production-authority-blocked.test.mjs scripts/check-production-hardening.test.mjs scripts/check-image-release-workflow.test.mjs",
      "check:production-authority-blocked":
        "node scripts/check-production-authority-blocked.mjs",
    },
  }),
  "docs/release/PR-473-PRODUCTION-CARDINALITY.evidence.json":
    validProductionEvidenceText,
  "docs/release/PR-473-PRODUCTION-PROMOTION.md":
    "This mechanism does **not** make mutable `main` a safe production desired-state authority. The `desired_state_authority_cutover` field is immutable `false`. Production activation remains **BLOCKED** pending a separate, higher-authority ADR. Evidence identities are self-asserted strings whose provenance is not authenticated; administrator bypass posture is unverified.\n",
  "docs/release/PR-473-PRODUCTION-PROMOTION.authorization.json": `${JSON.stringify(
    {
      schema_version: 2,
      pull_request: 473,
      target: "production",
      release_phase: "expand",
      rollback_floor: "f6ff236b9770c79301a3d07da6afb56be1e27bbf",
      desired_state_authority_cutover: false,
      deployment_authorized: false,
      command_only: false,
      production_cardinality_evidence: {
        path: "docs/release/PR-473-PRODUCTION-CARDINALITY.evidence.json",
        sha256: createHash("sha256")
          .update(validProductionEvidenceText)
          .digest("hex"),
        verified: false,
      },
      contract_authorities: {
        old_runtime_drain: false,
        rollback_floor_raise: false,
      },
    },
    null,
    2,
  )}\n`,
  ".github/workflows/ci.yml": `name: CI
jobs:
  backend:
    steps:
      - name: Production hardening contract
        run: npm run check:production-hardening
      - name: Kubernetes render and NetworkPolicy preflight
        run: npm run check:k8s
`,
  ".github/workflows/security.yml": `name: Security
jobs:
  iac:
    steps:
      - name: Render and scan production manifests
        run: |
          npm run check:production-hardening
          trivy config --severity HIGH,CRITICAL --exit-code 1 "$RUNNER_TEMP/rendered-k8s"
  filesystem:
    steps:
      - name: Verify canonical Trivy exception projection
        run: node scripts/generate-trivy-dev-codegen-exceptions.mjs --check
      - name: Trivy filesystem scan
        run: trivy fs --scanners vuln,secret --ignore-unfixed --ignorefile security/trivy-dev-codegen-exceptions.yaml --severity HIGH,CRITICAL --exit-code 1 .
  rust-advisories:
    steps:
      - name: Run cargo audit
        run: cargo audit
  rust-supply-chain:
    steps:
      - name: Run cargo deny
        run: cargo deny --manifest-path backend/Cargo.toml check
  node-advisories:
    steps:
      - name: npm audit
        run: |
          npm audit --omit=dev --audit-level=high --json > report.json
          node scripts/check-node-audit-exceptions.mjs --mode production --audit-report report.json
          npm audit --audit-level=high --json > report.json
          node scripts/check-node-audit-exceptions.mjs --mode dev-codegen --audit-report report.json
`,
  ".github/workflows/image-release.yml": `name: Image Release
on:
  workflow_dispatch:
    inputs:
      promote_production:
        description: Promote the signed digests to the production overlay
        required: true
        default: false
        type: boolean
jobs:
  release-probe:
    permissions:
      contents: read
      packages: read
    steps:
      - name: Checkout
        uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7
        with:
          ref: \${{ needs.ci-admission.outputs.release_sha }}
          persist-credentials: false
      - name: Provision topology
        run: cat ops/postgres-reconcile-topology.sh
  production-promotion-preflight:
    if: github.run_attempt == 1
    permissions:
      contents: read
    steps:
      - name: Checkout authorization
        uses: actions/checkout@${"c".repeat(40)}
        with:
          ref: \${{ github.sha }}
          persist-credentials: false
      - run: python3 scripts/check-production-promotion-authority.py initial --expected-sha "$GITHUB_SHA" --expected-ref "$GITHUB_REF"
  images:
    steps:
      - name: Trivy scan (fail on HIGH/CRITICAL)
        run: trivy image --severity HIGH,CRITICAL --exit-code 1 "$IMAGE_NAME@$DIGEST"
      - name: Sign the image
        run: cosign sign --yes "$IMAGE_NAME@$DIGEST"
      - name: Attest build provenance
        uses: actions/attest-build-provenance@v4
  bump-digests:
    needs: [production-promotion-preflight]
    environment: production
    if: >-
      github.event_name == 'workflow_dispatch' &&
      github.ref == 'refs/heads/dev' &&
      github.run_attempt == 1 &&
      inputs.promote_production == true
    permissions:
      contents: write
      actions: read
    steps:
      - name: Checkout exact authorization
        uses: actions/checkout@${"b".repeat(40)}
        with:
          ref: \${{ github.sha }}
          fetch-depth: 0
      - run: python3 scripts/check-production-promotion-authority.py initial --expected-sha "$GITHUB_SHA" --expected-ref "$GITHUB_REF"
      - name: Verify exact independent production reviewer team
        env:
          DISPATCHER: \${{ github.actor }}
          TRIGGERING_ACTOR: \${{ github.triggering_actor }}
          RUN_ATTEMPT: \${{ github.run_attempt }}
        run: |
          set -euo pipefail
          if [[ "\${RUN_ATTEMPT}" != "1" ]]; then
            echo "production promotion rejects workflow reruns"
            exit 1
          fi
          if [[ "\${DISPATCHER,,}" != "\${TRIGGERING_ACTOR,,}" ]]; then
            exit 1
          fi
          context="$(python3 scripts/check-production-promotion-authority.py reviewer-context --expected-sha "$GITHUB_SHA")"
          team_id="$(jq -er '.team_id' <<<"$context")"
          evidence_author_login="$(jq -er '.evidence_author_login' <<<"$context")"
          independent_reviewer_login="$(jq -er '.independent_reviewer_login' <<<"$context")"
          if [[ "\${DISPATCHER,,}" == "\${evidence_author_login,,}" || "\${DISPATCHER,,}" == "\${independent_reviewer_login,,}" ]]; then
            echo "production dispatcher must be distinct from the evidence author and independent evidence reviewer"
            exit 1
          fi
          environment="$(gh api "repos/\${REPO}/environments/production")"
          jq -e --argjson team_id "$team_id" '
            [.protection_rules[]? | select(.type == "required_reviewers")] as $rules
            | ($rules | length) == 1
              and $rules[0].prevent_self_review == true
              and ($rules[0].reviewers | length) == 1
              and $rules[0].reviewers[0].type == "Team"
              and $rules[0].reviewers[0].reviewer.id == $team_id
          ' <<<"$environment"
      - name: Bump prod overlay digests
        run: |
          bash scripts/bump-prod-digests.sh "$APP_DIGEST" "$WEB_DIGEST"
          python3 scripts/check-production-promotion-authority.py reset --expected-sha "$GITHUB_SHA"
      - name: Commit and push
        run: |
          git commit -m promote
          python3 scripts/check-production-promotion-authority.py pre-push --expected-sha "$GITHUB_SHA"
          git push origin "HEAD:dev"
`,
  "scripts/check-production-promotion-authority.py": `
from pathlib import PurePosixPath
import hashlib
AUTHORIZATION_PATH = "docs/release/PR-473-PRODUCTION-PROMOTION.authorization.json"
CANONICAL_EVIDENCE_PATH = "docs/release/PR-473-PRODUCTION-CARDINALITY.evidence.json"
schema_version = 2
desired_state_authority_cutover = False
def verify_authorization_schema(record, *, authorized: bool): pass
def verify_evidence_schema(record, candidate_source_sha): pass
def canonical_false(record): pass
def commit_parent(commit, label): pass
hashlib.sha256(b"")
pull_request = 473
production_cardinality = old_runtime_drain = rollback_floor_raise = True
candidate_source_sha = independent_reviewer = team_id = True
raise RuntimeError("keys are not exact")
git("rev-parse", "HEAD")
run(["git", "show", f"{expected_sha}:{path}"])
git("status", "--porcelain", "--untracked-files=no")
git("fetch", "--no-tags", "origin", "+refs/heads/dev:refs/remotes/origin/dev")
git("diff-tree")
raise RuntimeError("origin/dev advanced after authorization")
raise RuntimeError("activation requires a separate accepted higher-authority ADR/cutover")
`,

};

// The acceptance fixture is the checked-in release contract. Negative cases below
// mutate it; this prevents a toy workflow from drifting away from the real gate.
validWorkflowFiles["package.json"] = readFileSync(
  new URL("../package.json", import.meta.url),
  "utf8",
);
validWorkflowFiles[".github/workflows/image-release.yml"] = readFileSync(
  new URL("../.github/workflows/image-release.yml", import.meta.url),
  "utf8",
);
validWorkflowFiles["docs/program/executed-tests-baseline.json"] = readFileSync(
  new URL("../docs/program/executed-tests-baseline.json", import.meta.url),
  "utf8",
);

function evaluateWorkflows(overrides = {}) {
  const files = { ...validWorkflowFiles, ...overrides };
  return evaluateWorkflowHardeningChecks((path) => files[path] ?? "");
}

describe("production hardening wrapper-argv0 gate", () => {
  const realSecurity = readFileSync(
    new URL("../.github/workflows/security.yml", import.meta.url),
    "utf8",
  );
  const realCi = readFileSync(
    new URL("../.github/workflows/ci.yml", import.meta.url),
    "utf8",
  );
  const cargoAuditInvocation =
    '"${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" audit --ignore RUSTSEC-2023-0071';

  function evaluateRealWorkflows(securityText = realSecurity) {
    return evaluateWorkflows({
      ".github/workflows/security.yml": securityText,
      ".github/workflows/ci.yml": realCi,
    });
  }

  it("refuses source/./timeout argv0 wrappers around the cargo-audit binary (c236 residual)", () => {
    // Each decoy keeps the printf "running cargo audit" narrative and swaps the real
    // cargo-audit binary invocation for a wrapper argv0, which is the residual
    // false-green class the mechanism must fail closed on.
    const decoys = [
      ["timeout", 'timeout 30 "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" audit --ignore RUSTSEC-2023-0071'],
      ["source", 'source "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" audit --ignore RUSTSEC-2023-0071'],
      [".", '. "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" audit --ignore RUSTSEC-2023-0071'],
    ];
    for (const [wrapper, replacement] of decoys) {
      const decoy = realSecurity.replace(cargoAuditInvocation, replacement);
      assert.notEqual(
        decoy,
        realSecurity,
        `${wrapper} decoy must change the cargo-audit invocation`,
      );
      assertHasFailure(
        evaluateRealWorkflows(decoy),
        `must run cargo-audit as a real binary, not through the ${wrapper} wrapper`,
      );
    }
  });

  it("keeps intact workflows green and ignores the legit reindeer `source` setup", () => {
    const result = evaluateRealWorkflows(realSecurity);
    assert.equal(
      result.failures.filter((failure) => failure.includes("wrapper-argv0 gate"))
        .length,
      0,
      "intact security.yml must have no wrapper-argv0 findings (and the real ci.yml `source third-party/rust/reindeer/upstream.lock` must not false-fail)",
    );
  });

  it("scans literal block-scalar indicators for wrapped executors", () => {
    // run: |- / run: |+ keep each body line as a separate command, so a timeout
    // wrapper on its own line must still be refused.
    for (const indicator of ["|-", "|+"]) {
      const wrapped = realSecurity.replace(
        cargoAuditInvocation,
        `timeout 30 ${cargoAuditInvocation}`,
      );
      const decoy = wrapped.replace(
        "        run: |\n          printf '%s\\n' 'running cargo audit",
        `        run: ${indicator}\n          printf '%s\\n' 'running cargo audit`,
      );
      assert.notEqual(decoy, realSecurity, `${indicator} decoy must change the step`);
      assertHasFailure(
        evaluateRealWorkflows(decoy),
        "must run cargo-audit as a real binary, not through the timeout wrapper",
      );
    }
  });

  it("folds run: > bodies before classifying the wrapper", () => {
    // A folded scalar joins non-empty lines with a space, so `if timeout 30` on one
    // line and the cargo-audit binary on the next is ONE conditional command whose
    // argv0 is `timeout`, not two unrelated commands.
    const cargoAuditStep =
      "        run: |\n" +
      "          printf '%s\\n' 'running cargo audit through the directly installed cargo-audit binary'\n" +
      '          "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" audit --ignore RUSTSEC-2023-0071\n';
    const foldedStep =
      "        run: >\n" +
      "          if timeout 30\n" +
      '          "${RUNNER_TEMP}/cargo-security-tools/bin/cargo-audit" audit --ignore RUSTSEC-2023-0071; then true; fi\n';
    const decoy = realSecurity.replace(cargoAuditStep, foldedStep);
    assert.notEqual(decoy, realSecurity, "folded decoy must change the step");
    assertHasFailure(
      evaluateRealWorkflows(decoy),
      "must run cargo-audit as a real binary, not through the timeout wrapper",
    );
  });

  it("classifies wrappers behind shell control-flow prefixes", () => {
    for (const replacement of [
      `if timeout 30 ${cargoAuditInvocation}; then true; fi`,
      `! timeout 30 ${cargoAuditInvocation}`,
      `if env timeout 30 ${cargoAuditInvocation}; then true; fi`,
    ]) {
      const decoy = realSecurity.replace(cargoAuditInvocation, replacement);
      assert.notEqual(decoy, realSecurity, "control-flow decoy must change the invocation");
      assertHasFailure(
        evaluateRealWorkflows(decoy),
        "must run cargo-audit as a real binary, not through the timeout wrapper",
      );
    }
  });

  it("parses the `--` option terminator before classifying the sourced target", () => {
    for (const [argv0, prefix] of [["source", "source -- "], [".", ". -- "]]) {
      const decoy = realSecurity.replace(cargoAuditInvocation, prefix + cargoAuditInvocation);
      assert.notEqual(decoy, realSecurity, `${argv0} decoy must change the invocation`);
      assertHasFailure(
        evaluateRealWorkflows(decoy),
        `must run cargo-audit as a real binary, not through the ${argv0} wrapper`,
      );
    }
  });

  it("scans the image-release workflow for wrapped executors", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      'run: timeout 30 cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
    );
    assert.notEqual(decoy, realImageRelease, "cosign decoy must change the step");
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(
      result,
      ".github/workflows/image-release.yml must run cosign as a real binary, not through the timeout wrapper",
    );
  });

  it("fails closed on a malformed executed-test-source inventory", () => {
    for (const bad of [
      { test_attribute_baseline: "x" },
      { test_attribute_baseline: ["a.rs"] },
      { test_attribute_baseline: { fake: -7 } },
      { test_attribute_baseline: { "a.rs": "2" } },
    ]) {
      const result = evaluateWorkflows({
        "docs/program/executed-tests-baseline.json": JSON.stringify(bad),
      });
      assertHasFailure(
        result,
        "test_attribute_baseline must be an object mapping source paths to non-negative integer counts",
      );
    }
  });

  it("refuses a protected executor in a non-gating step", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      'if: false\n        run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
    );
    assert.notEqual(decoy, realImageRelease, "disabled-step decoy must change the step");
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(
      result,
      ".github/workflows/image-release.yml must run cosign in a gating step, not one dropped by if: false or continue-on-error",
    );
  });

  it("refuses a shell override on a protected executor", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      'shell: bash -c \'exit 0\' {0}\n        run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
    );
    assert.notEqual(decoy, realImageRelease, "shell-override decoy must change the step");
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(
      result,
      ".github/workflows/image-release.yml must run cosign with the default runner shell, not the bash -c",
    );
  });

  it("fails closed on an unparseable run command surface", () => {
    const decoy = realSecurity.replace(
      cargoAuditInvocation,
      `${cargoAuditInvocation} 'oops`,
    );
    assert.notEqual(decoy, realSecurity, "malformed decoy must change the invocation");
    assertHasFailure(
      evaluateRealWorkflows(decoy),
      "contains an unparseable run command",
    );
  });

  it("strips YAML comments before classifying the block-scalar indicator", () => {
    const wrapped = realSecurity.replace(
      cargoAuditInvocation,
      `timeout 30 ${cargoAuditInvocation}`,
    );
    const decoy = wrapped.replace(
      "        run: |\n          printf '%s\\n' 'running cargo audit",
      "        run: | # comment\n          printf '%s\\n' 'running cargo audit",
    );
    assert.notEqual(decoy, realSecurity, "commented-scalar decoy must change the step");
    assertHasFailure(
      evaluateRealWorkflows(decoy),
      "must run cargo-audit as a real binary, not through the timeout wrapper",
    );
  });

  it("reads workflow defaults.run.shell when accepting protected executors", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = "defaults:\n  run:\n    shell: bash -c 'exit 0' {0}\n\n" + realImageRelease;
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(
      result,
      ".github/workflows/image-release.yml must run cosign with the default runner shell",
    );
  });

  it("rejects a protected executor whose exit status is masked", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}" || true',
    );
    assert.notEqual(decoy, realImageRelease, "masked-status decoy must change the step");
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(
      result,
      ".github/workflows/image-release.yml must not mask the exit status of cosign",
    );
  });

  it("parses YAML indentation indicators in block scalars", () => {
    const wrapped = realSecurity.replace(
      cargoAuditInvocation,
      `timeout 30 ${cargoAuditInvocation}`,
    );
    const decoy = wrapped.replace(
      "        run: |\n          printf '%s\\n' 'running cargo audit",
      "        run: |2\n          printf '%s\\n' 'running cargo audit",
    );
    assert.notEqual(decoy, realSecurity, "indented-scalar decoy must change the step");
    assertHasFailure(
      evaluateRealWorkflows(decoy),
      "must run cargo-audit as a real binary, not through the timeout wrapper",
    );
  });

  it("fails closed on a wrapper whose target cannot be classified", () => {
    const decoy = realSecurity.replace(
      cargoAuditInvocation,
      'timeout 30 "$audit_bin" audit --ignore RUSTSEC-2023-0071',
    );
    assert.notEqual(decoy, realSecurity, "indirect-target decoy must change the invocation");
    assertHasFailure(
      evaluateRealWorkflows(decoy),
      "around an unclassifiable target",
    );
  });

  it("scans a workflow whose jobs key carries a YAML comment", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const wrapped = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      'run: timeout 30 cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
    );
    const decoy = wrapped.replace("jobs:", "jobs: # scanner bypass");
    assert.notEqual(decoy, realImageRelease, "commented-jobs decoy must change the workflow");
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(
      result,
      "must run cosign as a real binary, not through the timeout wrapper",
    );
  });

  it("inspects protected executors masked inside a nested shell -c command", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      "run: bash -c 'cosign sign --yes \"${IMAGE_NAME}@${DIGEST}\" || true'",
    );
    assert.notEqual(decoy, realImageRelease, "nested-shell decoy must change the step");
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(
      result,
      "must not mask the exit status of cosign inside a nested shell",
    );
  });

  it("gates the node audit-policy evaluator exit status", () => {
    const decoy = realSecurity.replace(
      "node scripts/check-node-audit-exceptions.mjs --mode production",
      "node scripts/check-node-audit-exceptions.mjs --mode production || true",
    );
    assert.notEqual(decoy, realSecurity, "node-audit decoy must change the command");
    assertHasFailure(
      evaluateRealWorkflows(decoy),
      "must not mask the exit status of node",
    );
  });

  it("classifies a wrapper behind the builtin prefix", () => {
    const decoy = realSecurity.replace(
      cargoAuditInvocation,
      `builtin source ${cargoAuditInvocation}`,
    );
    assertHasFailure(
      evaluateRealWorkflows(decoy),
      "not through the source wrapper",
    );
  });

  it("detects a multiline conditional masking a must-gate executor", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      'run: |\n          if cosign sign --yes "${IMAGE_NAME}@${DIGEST}"\n          then true\n          fi',
    );
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(result, "must not mask the exit status of cosign");
  });

  it("detects a must-gate executor masked inside a shell function", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      'run: |\n          sign() { cosign sign --yes "${IMAGE_NAME}@${DIGEST}" || true; }\n          sign',
    );
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(result, "must not mask the exit status of cosign");
  });

  it("does not prune commands after a conditional exit", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      'run: |\n          false && exit 0\n          cosign sign --yes "${IMAGE_NAME}@${DIGEST}" || true',
    );
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(result, "must not mask the exit status of cosign");
  });

  it("does not classify here-document data as commands", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      'run: |\n          cat <<\'EOF\'\n          timeout 30 cargo test\n          EOF\n          cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
    );
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assert.equal(
      result.failures.filter((failure) => failure.includes("timeout wrapper")).length,
      0,
      "here-document data must not be classified as a timeout wrapper",
    );
  });

  it("accepts a quoted shell: bash scalar", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      'shell: "bash"\n        run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
    );
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assert.equal(
      result.failures.filter((failure) => failure.includes("runner shell")).length,
      0,
      "a quoted shell: bash scalar must be treated as the default shell",
    );
  });

  it("unquotes an inline run scalar before classifying it", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      "run: 'cosign sign --yes \"${IMAGE_NAME}@${DIGEST}\" || true'",
    );
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(result, "must not mask the exit status of cosign");
  });

  it("recognizes a combined shell invocation flag with -c", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      "run: bash -ec 'cosign sign --yes \"${IMAGE_NAME}@${DIGEST}\" || true'",
    );
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(result, "must not mask the exit status of cosign inside a nested shell");
  });

  it("ignores set options on unexecuted conditional branches", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      'run: |\n          set +e\n          false && set -e\n          cosign sign --yes "${IMAGE_NAME}@${DIGEST}"\n          true',
    );
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(result, "must not mask the exit status of cosign");
  });

  it("classifies env -S split-string transparently", () => {
    const decoy = realSecurity.replace(
      cargoAuditInvocation,
      `env -S "source ${cargoAuditInvocation}"`,
    );
    assertHasFailure(
      evaluateRealWorkflows(decoy),
      "not through the source wrapper",
    );
  });

  it("decodes YAML escape sequences in a double-quoted run scalar", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      'run: "cosign sign --yes \\"${IMAGE_NAME}@${DIGEST}\\" \\u007c\\u007c true"',
    );
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(result, "must not mask the exit status of cosign");
  });

  it("classifies the command operand after command -p", () => {
    const realImageRelease = readFileSync(
      new URL("../.github/workflows/image-release.yml", import.meta.url),
      "utf8",
    );
    const decoy = realImageRelease.replace(
      'run: cosign sign --yes "${IMAGE_NAME}@${DIGEST}"',
      'run: if command -p timeout 30 cosign sign --yes "${IMAGE_NAME}@${DIGEST}"; then true; fi',
    );
    const result = evaluateWorkflows({
      ".github/workflows/security.yml": realSecurity,
      ".github/workflows/ci.yml": realCi,
      ".github/workflows/image-release.yml": decoy,
    });
    assertHasFailure(result, "not through the timeout wrapper");
  });
});

const documentedEnvironmentReviewerFilter = `
  [.protection_rules[]? | select(.type == "required_reviewers")] as $rules
  | ($rules | length) == 1
    and $rules[0].prevent_self_review == true
    and ($rules[0].reviewers | length) == 1
    and $rules[0].reviewers[0].type == "Team"
    and $rules[0].reviewers[0].reviewer.id == $team_id
`;

function evaluateDocumentedEnvironmentReviewers(environment) {
  return spawnSync(
    "jq",
    [
      "-e",
      "--argjson",
      "team_id",
      "424242",
      documentedEnvironmentReviewerFilter,
    ],
    { input: JSON.stringify(environment), encoding: "utf8" },
  );
}

describe("production hardening workflow gates", () => {
  it("rejects missing blocked-observation evaluator wiring, alias, and focused suite", () => {
    assertHasFailure(
      evaluateWorkflows({
        "scripts/check-production-authority-blocked.mjs": "",
      }),
      "blocked evaluator and exact package CLI wiring",
    );
    const withoutAlias = JSON.parse(validWorkflowFiles["package.json"]);
    delete withoutAlias.scripts["check:production-authority-blocked"];
    assertHasFailure(
      evaluateWorkflows({ "package.json": JSON.stringify(withoutAlias) }),
      "blocked evaluator and exact package CLI wiring",
    );
    const withoutFocusedTest = JSON.parse(validWorkflowFiles["package.json"]);
    withoutFocusedTest.scripts["test:production-hardening"] =
      "npm run test:pr473-migration-operational && python3 scripts/check-production-promotion-authority.test.py";
    assertHasFailure(
      evaluateWorkflows({ "package.json": JSON.stringify(withoutFocusedTest) }),
      "canonical fail-closed command",
    );
  });
  it("accepts active CI, security, and image-release workflow gates", () => {
    assert.deepEqual(evaluateWorkflows().failures, []);
  });

  it("pins the finite image-release admission convergence budget", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    assertHasFailure(
      evaluateWorkflows({
        ".github/workflows/image-release.yml": releaseWorkflow.replace(
          'ADMISSION_MAX_POLLS: "48"',
          'ADMISSION_MAX_POLLS: "12"',
        ),
      }),
      "image-release must use completed CI only as a wake-up",
    );
  });

  it("binds the documented GitHub environment Team response shape exactly", () => {
    const rule = {
      type: "required_reviewers",
      prevent_self_review: true,
      reviewers: [
        {
          type: "Team",
          reviewer: { id: 424242, slug: "production-reviewers" },
        },
      ],
    };
    assert.equal(
      evaluateDocumentedEnvironmentReviewers({ protection_rules: [rule] })
        .status,
      0,
    );
    assert.notEqual(
      evaluateDocumentedEnvironmentReviewers({
        protection_rules: [
          {
            ...rule,
            reviewers: [
              ...rule.reviewers,
              { type: "User", reviewer: { id: 7 } },
            ],
          },
        ],
      }).status,
      0,
    );
    assert.notEqual(
      evaluateDocumentedEnvironmentReviewers({
        protection_rules: [
          {
            ...rule,
            reviewers: [{ type: "Team", id: 424242, reviewer: { id: 7 } }],
          },
        ],
      }).status,
      0,
    );
  });

  it("rejects removing the canonical Python promotion suite", () => {
    const pkg = JSON.parse(validWorkflowFiles["package.json"]);
    pkg.scripts["test:production-hardening"] =
      "node --test scripts/check-production-hardening.test.mjs";
    assertHasFailure(
      evaluateWorkflows({ "package.json": JSON.stringify(pkg) }),
      "canonical fail-closed command",
    );
  });

  it("rejects a non-false production authorization or missing unprotected preflight", () => {
    const authorization = JSON.parse(
      validWorkflowFiles[
        "docs/release/PR-473-PRODUCTION-PROMOTION.authorization.json"
      ],
    );
    authorization.deployment_authorized = true;
    assertHasFailure(
      evaluateWorkflows({
        "docs/release/PR-473-PRODUCTION-PROMOTION.authorization.json": `${JSON.stringify(authorization, null, 2)}\n`,
      }),
      "canonical schema-v2 false-by-default",
    );
    assertHasFailure(
      evaluateWorkflows({
        ".github/workflows/image-release.yml": validWorkflowFiles[
          ".github/workflows/image-release.yml"
        ].replace("  production-promotion-preflight:", "  renamed-preflight:"),
      }),
      "unprotected read-only preflight",
    );

    authorization.deployment_authorized = false;
    authorization.desired_state_authority_cutover = true;
    assertHasFailure(
      evaluateWorkflows({
        "docs/release/PR-473-PRODUCTION-PROMOTION.authorization.json": `${JSON.stringify(authorization, null, 2)}\n`,
      }),
      "canonical schema-v2 false-by-default",
    );
  });

  it("rejects missing one-shot reset, immutable verifier inputs, or rebase behavior", () => {
    const workflow = validWorkflowFiles[".github/workflows/image-release.yml"];
    assertHasFailure(
      evaluateWorkflows({
        ".github/workflows/image-release.yml": workflow.replace(
          "check-production-promotion-authority.py reset",
          "renamed-authority.py reset",
        ),
      }),
      "reset one-shot authorization",
    );
    assertHasFailure(
      evaluateWorkflows({
        "scripts/check-production-promotion-authority.py": validWorkflowFiles[
          "scripts/check-production-promotion-authority.py"
        ].replace('["git", "show", f"{expected_sha}:{path}"]', '["cat", path]'),
      }),
      "immutable git-show inputs",
    );
    assertHasFailure(
      evaluateWorkflows({
        ".github/workflows/image-release.yml": workflow.replace(
          'git push origin "HEAD:dev"',
          'git pull --rebase origin dev\n          git push origin "HEAD:dev"',
        ),
      }),
      "must not pull, rebase, retry, or loop",
    );
  });

  it("rejects recovery without an exact required candidate SHA", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    const result = evaluateWorkflows({
      ".github/workflows/image-release.yml": releaseWorkflow.replace(
        `      candidate_sha:
        description: Exact current-dev SHA whose successful push CI authorizes recovery
        required: true
        type: string
`,
        "",
      ),
    });

    assertHasFailure(
      result,
      "workflow_dispatch recovery must require a lowercase 40-character candidate_sha",
    );
  });

  it("rejects production digest promotion without an explicit required false-by-default dispatch input", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    const result = evaluateWorkflows({
      ".github/workflows/image-release.yml": releaseWorkflow.replace(
        `      promote_production:
        description: Promote the signed digests to the production overlay
        required: true
        default: false
        type: boolean
`,
        "",
      ),
    });

    assertHasFailure(
      result,
      "must declare promote_production as a required false-by-default boolean",
    );
  });

  it("rejects production digest promotion on push or outside dev", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    const result = evaluateWorkflows({
      ".github/workflows/image-release.yml": releaseWorkflow
        .replaceAll(
          "github.event_name == 'workflow_dispatch'",
          "github.event_name == 'push'",
        )
        .replaceAll(
          "github.ref == 'refs/heads/dev'",
          "startsWith(github.ref, 'refs/heads/')",
        ),
    });

    assertHasFailure(
      result,
      "must run only for an explicit workflow_dispatch on refs/heads/dev",
    );
  });

  it("rejects production digest promotion without the production environment", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    const result = evaluateWorkflows({
      ".github/workflows/image-release.yml": releaseWorkflow.replace(
        "    environment: production\n",
        "",
      ),
    });

    assertHasFailure(
      result,
      "must bind the mutation job to the production environment",
    );
  });

  it("rejects production promotion without exact independent Team reviewer enforcement", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    for (const mutated of [
      releaseWorkflow.replace(
        "prevent_self_review == true",
        "prevent_self_review == false",
      ),
      releaseWorkflow.replace(
        '.reviewers[0].type == "Team"',
        '.reviewers[0].type == "User"',
      ),
      releaseWorkflow.replace(
        "($rules[0].reviewers | length) == 1",
        "($rules[0].reviewers | length) > 0",
      ),
      releaseWorkflow.replace(".reviewer.id == $team_id", ".id == $team_id"),
      releaseWorkflow.replace("reviewer-context", "removed-reviewer-context"),
      releaseWorkflow.replace(
        "          DISPATCHER: \${{ github.actor }}\n",
        "",
      ),
      releaseWorkflow.replace(
        "          TRIGGERING_ACTOR: \${{ github.triggering_actor }}\n",
        "",
      ),
      releaseWorkflow.replaceAll(
        "evidence_author_login",
        "removed_first_party",
      ),
      releaseWorkflow.replace(
        "production dispatcher must be distinct from the evidence author and independent evidence reviewer",
        "dispatcher independence disabled",
      ),
    ]) {
      assertHasFailure(
        evaluateWorkflows({ ".github/workflows/image-release.yml": mutated }),
        "exact immutable evidence Team ID",
      );
    }
  });

  it("rejects workflow reruns in both preflight and protected promotion jobs", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    assertHasFailure(
      evaluateWorkflows({
        ".github/workflows/image-release.yml": releaseWorkflow.replaceAll(
          "github.run_attempt == 1",
          "github.run_attempt >= 1",
        ),
      }),
      "explicit workflow_dispatch on refs/heads/dev",
    );
  });

  it("rejects release-probe checkout without explicit job-level contents read", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    const result = evaluateWorkflows({
      ".github/workflows/image-release.yml": releaseWorkflow.replaceAll(
        "      contents: read\n",
        "",
      ),
    });

    assertHasFailure(
      result,
      "release-probe permissions must explicitly grant contents: read for its checkout",
    );
  });

  it("rejects a mutable release-probe checkout reference", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    const result = evaluateWorkflows({
      ".github/workflows/image-release.yml": releaseWorkflow.replaceAll(
        `actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0`,
        "actions/checkout@v7",
      ),
    });

    assertHasFailure(
      result,
      "release-probe must perform a SHA-pinned actions/checkout",
    );
  });

  it("rejects release-probe topology use before checkout", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    const result = evaluateWorkflows({
      ".github/workflows/image-release.yml": releaseWorkflow.replaceAll(
        "ops/postgres-reconcile-topology.sh",
        "removed-topology-script.sh",
      ),
    });

    assertHasFailure(result, "before using ops/postgres-reconcile-topology.sh");
  });

  it("rejects release-probe without post-migration canonical-enforce", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    // Hostile: drop the soe step wholesale (the class proven green without a
    // static pin). Sibling rename of the script path is covered by the same
    // failure message.
    const withoutCanonicalEnforce = releaseWorkflow.replace(
      `      - name: Enforce canonical writer ownership on a migrated probe database
        env:
          POSTGRES_CONTAINER: \${{ job.services.postgres.id }}
        run: |
          docker cp ops/postgres-reconcile-topology.sh "\${POSTGRES_CONTAINER}:/topology.sh"
          docker cp "$RUNNER_TEMP/probe-topology.env" "\${POSTGRES_CONTAINER}:/topology.env"
          bash backend/ci/gates/writer-ownership/canonical-enforce.sh \\
            "$GITHUB_WORKSPACE" "$POSTGRES_CONTAINER" canonical_probe_release

`,
      "",
    );
    assert.notEqual(
      withoutCanonicalEnforce,
      releaseWorkflow,
      "fixture must actually remove the canonical-enforce step",
    );
    assertHasFailure(
      evaluateWorkflows({
        ".github/workflows/image-release.yml": withoutCanonicalEnforce,
      }),
      "canonical-enforce.sh on a migrated probe database after migrate and before boot",
    );
  });

  it("rejects release-probe canonical-enforce sequenced after boot", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    const enforceStep = `      - name: Enforce canonical writer ownership on a migrated probe database
        env:
          POSTGRES_CONTAINER: \${{ job.services.postgres.id }}
        run: |
          docker cp ops/postgres-reconcile-topology.sh "\${POSTGRES_CONTAINER}:/topology.sh"
          docker cp "$RUNNER_TEMP/probe-topology.env" "\${POSTGRES_CONTAINER}:/topology.env"
          bash backend/ci/gates/writer-ownership/canonical-enforce.sh \\
            "$GITHUB_WORKSPACE" "$POSTGRES_CONTAINER" canonical_probe_release

`;
    const bootStepMarker =
      "      - name: Boot the release image (the real published image — no dev-auth flag exists to pass)";
    const withoutEnforce = releaseWorkflow.replace(enforceStep, "");
    assert.notEqual(withoutEnforce, releaseWorkflow);
    const bootAt = withoutEnforce.indexOf(bootStepMarker);
    assert.ok(bootAt >= 0, "boot step must remain after removing enforce");
    // Find the end of the boot step's run block by locating the next top-level
    // step or job key is brittle; splice enforce immediately AFTER the boot
    // step name line so extractNamedWorkflowStep order is boot-then-enforce.
    const afterBootName = withoutEnforce.indexOf("\n", bootAt) + 1;
    const reordered =
      withoutEnforce.slice(0, afterBootName) +
      enforceStep +
      withoutEnforce.slice(afterBootName);
    assertHasFailure(
      evaluateWorkflows({
        ".github/workflows/image-release.yml": reordered,
      }),
      "canonical-enforce.sh on a migrated probe database after migrate and before boot",
    );
  });

  it("rejects persisted release-probe checkout credentials", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    const result = evaluateWorkflows({
      ".github/workflows/image-release.yml": releaseWorkflow.replaceAll(
        "          persist-credentials: false\n",
        "          persist-credentials: true\n",
      ),
    });

    assertHasFailure(result, "persist-credentials: false");
  });

  it("does not let the following preflight job satisfy release-probe checks", () => {
    const releaseWorkflow =
      validWorkflowFiles[".github/workflows/image-release.yml"];
    const releaseProbeWithoutGuards = releaseWorkflow
      .replace(
        `      contents: read
      packages: read
`,
        `      packages: read
`,
      )
      .replaceAll(
        "ops/postgres-reconcile-topology.sh",
        "removed-topology-script.sh",
      )
      .replace(
        "  production-promotion-preflight:\n",
        `  production-promotion-preflight:
    # These strings must not leak backward into release-probe validation.
    # contents: read; actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0; persist-credentials: false
    # ops/postgres-reconcile-topology.sh
`,
      );
    const result = evaluateWorkflows({
      ".github/workflows/image-release.yml": releaseProbeWithoutGuards,
    });
    assertHasFailure(
      result,
      "release-probe permissions must explicitly grant contents: read",
    );
    assertHasFailure(result, "before using ops/postgres-reconcile-topology.sh");
  });

  it("rejects workflow gates that only appear in comments or unused literals", () => {
    const result = evaluateWorkflows({
      ".github/workflows/ci.yml": `name: CI
env:
  UNUSED_GATE_TEXT: "npm run check:production-hardening npm run check:k8s"
# npm run check:production-hardening
# npm run check:k8s
jobs:
  backend:
    steps:
      - run: echo "CI has no active production hardening gate"
`,
      ".github/workflows/security.yml": `name: Security
env:
  UNUSED_SECURITY_TEXT: "npm run check:production-hardening trivy fs --scanners vuln,secret trivy config --severity HIGH,CRITICAL --exit-code 1 cargo audit cargo deny --manifest-path backend/Cargo.toml check npm audit --audit-level=high"
jobs:
  iac:
    steps:
      - run: echo "security gates are absent"
`,
      ".github/workflows/image-release.yml": `name: Image Release
env:
  UNUSED_RELEASE_TEXT: "completed CI admission Trivy scan (fail on HIGH/CRITICAL) cosign sign --yes attest-build-provenance bump-prod-digests"
# - name: completed CI admission
# - name: Trivy scan (fail on HIGH/CRITICAL)
# - run: cosign sign --yes "$IMAGE"
# - uses: actions/attest-build-provenance@v4
# - run: bash scripts/bump-prod-digests.sh "$APP" "$WEB"
jobs:
  images:
    steps:
      - run: echo "image release has no active gates"
`,
    });

    assertHasFailure(
      result,
      "CI must run npm run check:production-hardening as an active step",
    );
    assertHasFailure(
      result,
      "Security workflow must run npm run check:production-hardening as an active step",
    );
    assertHasFailure(
      result,
      "security workflow must actively run trivy fs --scanners vuln,secret",
    );
    assertHasFailure(
      result,
      "image-release must use completed CI only as a wake-up",
    );
    assertHasFailure(result, "image-release must actively cosign sign");
  });
});

function evaluateDeployScript(text) {
  return evaluateDeployAutomationChecks((path) =>
    path === "scripts/deploy.sh" ? text : "",
  );
}

describe("production hardening deploy automation checks", () => {
  it("accepts a deploy path that actively gates on kubectl, Argo refresh, both rollouts, and endpoints", () => {
    const result = evaluateDeployScript(`set -euo pipefail
MODE="deploy"
case "\${1:-}" in
  --digest-bump-only|--bump-only)
    MODE="digest-bump-only"
    shift
    ;;
esac
if [[ "\${MODE}" == "digest-bump-only" ]]; then
  log "done: \${SHORT_SHA} desired prod digests updated only (console-app=sha256:aaa); deployment, rollout, pod-image, and endpoint verification were NOT run."
  exit 0
fi
require kubectl
if ! kubectl version >/dev/null 2>&1; then
  echo "deploy: kubectl cannot reach the cluster" >&2
  exit 1
fi
kubectl -n "$ARGO_NS" annotate "application/$APP_NAME" "argocd.argoproj.io/refresh=hard" --overwrite
ROLLOUTS=(console-app)
for rollout in "\${ROLLOUTS[@]}"; do
  kubectl argo rollouts status "$rollout" -n "$NAMESPACE" --timeout 600s
done
for url in "\${ENDPOINTS[@]}"; do
  code="$(curl -fsS -o /dev/null -w '%{http_code}' --max-time 10 "$url")"
  if [[ "$code" != "200" ]]; then exit 1; fi
done
log "done: \${SHORT_SHA} deployed and verified"
`);

    assert.deepEqual(result.failures, []);
  });

  it("rejects deploy scripts that skip rollout verification but keep claiming success", () => {
    const result = evaluateDeployScript(`set -euo pipefail
# argocd.argoproj.io/refresh=hard
# kubectl argo rollouts status
if ! have kubectl || ! kubectl version >/dev/null 2>&1; then
  log "kubectl/cluster unreachable; skipping the in-cluster refresh + rollout wait."
else
  log "would wait for rollouts"
fi
curl -fsS https://console.example.test
log "done: \${SHORT_SHA} deployed and verified"
`);

    assertHasFailure(result, "must fail closed before endpoint checks");
    assertHasFailure(result, "must actively request an Argo hard refresh");
    assertHasFailure(
      result,
      "must actively wait for the console-app rollout",
    );
  });

  it("rejects digest-bump-only modes that claim verified rollout", () => {
    const result = evaluateDeployScript(`set -euo pipefail
MODE="deploy"
case "\${1:-}" in
  --digest-bump-only|--bump-only)
    MODE="digest-bump-only"
    shift
    ;;
esac
require kubectl
ROLLOUTS=(console-app)
if [[ "\${MODE}" == "digest-bump-only" ]]; then
  log "done: \${SHORT_SHA} deployed and verified (digest bump only)"
  exit 0
fi
kubectl version >/dev/null
kubectl -n "$ARGO_NS" annotate "application/$APP_NAME" "argocd.argoproj.io/refresh=hard" --overwrite
for rollout in "\${ROLLOUTS[@]}"; do
  kubectl argo rollouts status "$rollout" -n "$NAMESPACE" --timeout 600s
done
for url in "\${ENDPOINTS[@]}"; do
  code="$(curl -fsS -o /dev/null -w '%{http_code}' --max-time 10 "$url")"
  if [[ "$code" != "200" ]]; then exit 1; fi
done
log "done: \${SHORT_SHA} deployed and verified"
`);

    assertHasFailure(
      result,
      "digest-bump-only mode must not claim deployment, rollout, pod-image, or endpoint verification",
    );
  });

  it("rejects deploy scripts that swallow rollout status failures", () => {
    const result = evaluateDeployScript(`set -euo pipefail
require kubectl
kubectl version >/dev/null
kubectl -n "$ARGO_NS" annotate "application/$APP_NAME" "argocd.argoproj.io/refresh=hard" --overwrite
ROLLOUTS=(console-app)
for rollout in "\${ROLLOUTS[@]}"; do
  kubectl argo rollouts status "$rollout" -n "$NAMESPACE" --timeout 600s || true
done
log "done: \${SHORT_SHA} deployed and verified"
`);

    assertHasFailure(result, "must not swallow rollout status failures");
  });

  it("rejects removing one-shot, remote, or Argo revision guards from deploy automation", () => {
    const deploy = readFileSync(
      new URL("./deploy.sh", import.meta.url),
      "utf8",
    );
    assertHasFailure(
      evaluateDeployScript(
        deploy.replace(
          'scripts/check-production-promotion-authority.py" reset',
          'scripts/renamed-authority.py" reset',
        ),
      ),
      "reset the one-shot authorization",
    );
    assertHasFailure(
      evaluateDeployScript(
        deploy
          .replaceAll(
            'scripts/check-production-promotion-authority.py" remote',
            'scripts/renamed-authority.py" remote',
          )
          .replaceAll(
            "verify_argo_pre_refresh_revision",
            "renamed_argo_revision_check",
          ),
      ),
      "remote and Argo revisions before refresh",
    );
  });
});

function writeExecutable(path, content) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content, "utf8");
  chmodSync(path, 0o755);
}

function runDeployWithStubs({
  deployArgs,
  kubectl,
  authority,
  hideKubectl = false,
} = {}) {
  const dir = mkdtempSync(join(tmpdir(), "console-deploy-test-"));
  const scriptsDir = join(dir, "scripts");
  const stubDir = join(dir, "bin");
  const bashEnv = join(dir, "bash-env");
  mkdirSync(scriptsDir, { recursive: true });
  mkdirSync(stubDir, { recursive: true });
  mkdirSync(join(dir, "deploy/apps/console/overlays/prod"), {
    recursive: true,
  });
  writeFileSync(
    join(scriptsDir, "deploy.sh"),
    readFileSync(new URL("./deploy.sh", import.meta.url), "utf8"),
  );
  chmodSync(join(scriptsDir, "deploy.sh"), 0o755);
  writeExecutable(
    join(scriptsDir, "check-production-promotion-authority.py"),
    authority ??
      `#!/usr/bin/env python3
raise SystemExit(0)
`,
  );
  writeExecutable(
    join(scriptsDir, "bump-prod-digests.sh"),
    `#!/usr/bin/env bash
set -euo pipefail
exit 0
`,
  );
  writeFileSync(
    join(dir, "deploy/apps/console/overlays/prod/kustomization.yaml"),
    "images: []\n",
  );
  // Fresh executable files can be held at macOS provenance policy startup.
  // Keep command doubles non-executable and outside PATH; BASH_ENV functions
  // invoke them through /bin/bash and are therefore load-bearing on every host.
  writeFileSync(
    join(stubDir, "git"),
    `#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "diff" ]]; then exit 0; fi
if [[ "$1 $2" == "rev-parse --abbrev-ref" ]]; then echo main; exit 0; fi
if [[ "$1 $2" == "rev-parse HEAD" ]]; then echo ${"a".repeat(40)}; exit 0; fi
exit 0
`,
    "utf8",
  );
  writeFileSync(
    join(stubDir, "gh"),
    `#!/usr/bin/env bash
set -euo pipefail
if [[ "$1 $2" == "run list" ]]; then echo 12345; exit 0; fi
if [[ "$1 $2" == "run watch" ]]; then exit 0; fi
if [[ "$1 $2" == "run download" ]]; then
  name=""
  out=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --name) name="$2"; shift 2 ;;
      --dir) out="$2"; shift 2 ;;
      *) shift ;;
    esac
  done
  mkdir -p "$out"
  printf '%s' "sha256:${"c".repeat(64)}" > "$out/\${name}.txt"
  exit 0
fi
exit 0
`,
    "utf8",
  );
  writeFileSync(
    join(stubDir, "curl"),
    `#!/usr/bin/env bash
set -euo pipefail
printf '200'
`,
    "utf8",
  );
  if (kubectl) {
    writeFileSync(join(stubDir, "kubectl"), kubectl, "utf8");
  }
  writeFileSync(
    bashEnv,
    `git() { /bin/bash "\${CONSOLE_DEPLOY_TEST_STUB_DIR}/git" "$@"; }
gh() { /bin/bash "\${CONSOLE_DEPLOY_TEST_STUB_DIR}/gh" "$@"; }
curl() { /bin/bash "\${CONSOLE_DEPLOY_TEST_STUB_DIR}/curl" "$@"; }
${kubectl ? 'kubectl() { /bin/bash "${CONSOLE_DEPLOY_TEST_STUB_DIR}/kubectl" "$@"; }' : ""}
${
  hideKubectl
    ? `command() {
  if [[ "\${1:-}" == "-v" && "\${2:-}" == "kubectl" ]]; then
    return 1
  fi
  builtin command "$@"
}`
    : ""
}
`,
    "utf8",
  );

  const result = spawnSync(
    "bash",
    [join(scriptsDir, "deploy.sh"), ...(deployArgs ?? ["b".repeat(40)])],
    {
      cwd: dir,
      env: {
        ...process.env,
        PATH: "/usr/bin:/bin",
        HOME: dir,
        BASH_ENV: bashEnv,
        CONSOLE_DEPLOY_TEST_STUB_DIR: stubDir,
      },
      encoding: "utf8",
      timeout: 10_000,
    },
  );
  return { ...result, combined: `${result.stdout}\n${result.stderr}` };
}

function assertDeployHarnessCompleted(result) {
  assert.equal(
    result.error,
    undefined,
    `deploy test harness did not complete: ${result.error?.message ?? "unknown spawn error"}\n${result.combined}`,
  );
}

describe("deploy.sh rollout verification fail-closed behavior", () => {
  it("fails instead of claiming deployment success when kubectl is missing", () => {
    const result = runDeployWithStubs({ hideKubectl: true });

    assertDeployHarnessCompleted(result);
    assert.notEqual(result.status, 0, result.combined);
    assert.match(result.combined, /kubectl/i);
    assert.doesNotMatch(result.combined, /deployed and verified/);
  });

  it("fails instead of claiming deployment success when the cluster is unavailable", () => {
    const result = runDeployWithStubs({
      kubectl: `#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "version" ]]; then echo "cluster unreachable" >&2; exit 1; fi
exit 0
`,
    });

    assertDeployHarnessCompleted(result);
    assert.notEqual(result.status, 0, result.combined);
    assert.match(result.combined, /cluster|kubectl/i);
    assert.doesNotMatch(result.combined, /deployed and verified/);
  });

  it("exits successfully in explicit digest-bump-only modes without claiming verified rollout", () => {
    for (const flag of ["--digest-bump-only", "--bump-only"]) {
      const result = runDeployWithStubs({ deployArgs: [flag, "b".repeat(40)] });

      assertDeployHarnessCompleted(result);
      assert.equal(result.status, 0, `${flag}: ${result.combined}`);
      assert.match(result.combined, /desired prod digests updated only/);
      assert.match(
        result.combined,
        /deployment, rollout, pod-image, and endpoint verification were NOT run/,
      );
      assert.doesNotMatch(result.combined, /deployed and verified/);
    }
  });

  it("never reaches Argo refresh when main advances on the digest-no-op pre-refresh path", () => {
    const logPath = join(
      tmpdir(),
      `console-deploy-kubectl-${process.pid}-${Date.now()}.log`,
    );
    const authority = `#!/usr/bin/env python3
import pathlib, sys
counter = pathlib.Path("remote-count")
mode = sys.argv[1]
if mode == "remote":
    count = int(counter.read_text() if counter.exists() else "0") + 1
    counter.write_text(str(count))
    if count >= 2:
        print("origin/main advanced before Argo refresh", file=sys.stderr)
        raise SystemExit(1)
raise SystemExit(0)
`;
    const kubectl = `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >> ${JSON.stringify(logPath)}
if [[ "$*" == *"jsonpath="* ]]; then echo ${"b".repeat(40)}; fi
exit 0
`;
    const result = runDeployWithStubs({ authority, kubectl });
    const calls = existsSync(logPath) ? readFileSync(logPath, "utf8") : "";
    rmSync(logPath, { force: true });
    assertDeployHarnessCompleted(result);
    assert.notEqual(result.status, 0, result.combined);
    assert.match(result.combined, /advanced before Argo refresh/);
    assert.doesNotMatch(calls, /argocd\.argoproj\.io\/refresh=hard/);
    assert.doesNotMatch(result.combined, /deployed and verified/);
  });
});
