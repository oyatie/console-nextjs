import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const scriptPath = new URL("./check-non-oci-mail-imessage-relay.mjs", import.meta.url).pathname;
const deploymentPath = new URL("../deploy/apps/console/components/imessage-relay/deployment.yaml", import.meta.url).pathname;
const networkPolicyPath = new URL("../deploy/apps/console/components/imessage-relay/networkpolicy.yaml", import.meta.url).pathname;
const BLOCKER = "blocked_missing_non_oci_talos_or_bridge_credentials";
const BLOCKER_SUMMARY = "BLOCKED_PENDING_NON_OCI_TALOS_CREDENTIALS";

function cleanEnv() {
  const env = { ...process.env };
  for (const name of [
    "NON_OCI_TALOS_KUBECONFIG",
    "NON_OCI_TALOSCONFIG",
    "MESSAGES_BRIDGE_TLS_CA_PATH",
    "MESSAGES_BRIDGE_TLS_CERT_PATH",
    "MESSAGES_BRIDGE_TLS_KEY_PATH",
    "MESSAGES_BRIDGE_URL",
    "MESSAGES_BRIDGE_TOKEN",
  ]) {
    delete env[name];
  }
  return env;
}

function makeReadableInputs() {
  const dir = mkdtempSync(join(tmpdir(), "non-oci-relay-"));
  const files = {
    NON_OCI_TALOS_KUBECONFIG: join(dir, "kubeconfig"),
    NON_OCI_TALOSCONFIG: join(dir, "talosconfig"),
    MESSAGES_BRIDGE_TLS_CA_PATH: join(dir, "ca.crt"),
    MESSAGES_BRIDGE_TLS_CERT_PATH: join(dir, "client.crt"),
    MESSAGES_BRIDGE_TLS_KEY_PATH: join(dir, "client.key"),
  };
  for (const filePath of Object.values(files)) {
    writeFileSync(filePath, "test-placeholder\n", { mode: 0o600 });
  }
  return files;
}

function runDryRun(extraEnv = {}) {
  return spawnSync(process.execPath, [scriptPath, "--dry-run"], {
    env: { ...cleanEnv(), ...extraEnv },
    encoding: "utf8",
  });
}

test("fails closed with the documented blocker when required inputs are absent", () => {
  const result = runDryRun();

  assert.equal(result.status, 2);
  assert.deepEqual(result.stdout.trim().split(/\r?\n/), [BLOCKER_SUMMARY, BLOCKER]);
  assert.equal(result.stderr, "");
});

test("rejects a non-HTTPS macOS Messages bridge URL even when files are readable", () => {
  const result = runDryRun({
    ...makeReadableInputs(),
    MESSAGES_BRIDGE_URL: "http://bridge.internal.example",
    MESSAGES_BRIDGE_TOKEN: "test-token-value-0000",
  });

  assert.equal(result.status, 2);
  assert.deepEqual(result.stdout.trim().split(/\r?\n/), [BLOCKER_SUMMARY, BLOCKER]);
  assert.doesNotMatch(result.stdout, /test-token-value-0000/);
  assert.equal(result.stderr, "");
});

test("rejects unreadable Talos or bridge credential files", {
  skip: process.getuid?.() === 0 ? "root can read chmod 000 files on some runners" : false,
}, () => {
  const inputs = makeReadableInputs();
  chmodSync(inputs.MESSAGES_BRIDGE_TLS_KEY_PATH, 0o000);
  try {
    const result = runDryRun({
      ...inputs,
      MESSAGES_BRIDGE_URL: "https://bridge.internal.example",
      MESSAGES_BRIDGE_TOKEN: "test-token-value-0000",
    });

    assert.equal(result.status, 2);
    assert.deepEqual(result.stdout.trim().split(/\r?\n/), [BLOCKER_SUMMARY, BLOCKER]);
    assert.equal(result.stderr, "");
  } finally {
    chmodSync(inputs.MESSAGES_BRIDGE_TLS_KEY_PATH, 0o600);
  }
});

test("rejects credential paths that are directories", () => {
  const inputs = makeReadableInputs();
  const directoryCredentialPath = mkdtempSync(join(tmpdir(), "non-oci-relay-dir-"));

  try {
    const result = runDryRun({
      ...inputs,
      NON_OCI_TALOS_KUBECONFIG: directoryCredentialPath,
      MESSAGES_BRIDGE_URL: "https://bridge.internal.example",
      MESSAGES_BRIDGE_TOKEN: "test-token-value-0000",
    });

    assert.equal(result.status, 2);
    assert.deepEqual(result.stdout.trim().split(/\r?\n/), [BLOCKER_SUMMARY, BLOCKER]);
    assert.equal(result.stderr, "");
  } finally {
    rmSync(directoryCredentialPath, { recursive: true, force: true });
  }
});

test("passes dry-run with an HTTPS bridge URL and readable Talos plus mTLS files", () => {
  const result = runDryRun({
    ...makeReadableInputs(),
    MESSAGES_BRIDGE_URL: "https://bridge.internal.example",
    MESSAGES_BRIDGE_TOKEN: "test-token-value-0000",
  });

  assert.equal(result.status, 0);
  assert.equal(result.stdout.trim(), "non_oci_talos_mail_imessage_relay_dry_run_ready");
  assert.equal(result.stderr, "");
});

test("relay component is stateless by default and fails closed on caller and bridge CIDRs", () => {
  const deployment = readFileSync(deploymentPath, "utf8");
  const networkPolicy = readFileSync(networkPolicyPath, "utf8");

  assert.doesNotMatch(deployment, /DATABASE_URL|IMESSAGE_RELAY_DATABASE_URL|console-db-rt/);
  assert.doesNotMatch(networkPolicy, /allow-imessage-relay-egress-postgres|cnpg\.io\/cluster|port: 5432/);
  assert.match(deployment, /name: IMESSAGE_RELAY_RECIPIENT_SOURCE\s+value: static/);
  const allowedRecipientsEntry = deployment.match(
    /- name: IMESSAGE_RELAY_ALLOWED_RECIPIENTS\n(?: {14}.+\n?){1,3}/,
  )?.[0] ?? "";
  assert.match(
    allowedRecipientsEntry,
    /secretKeyRef: \{ name: imessage-relay-secrets, key: allowed-recipients \}/,
  );
  assert.doesNotMatch(allowedRecipientsEntry, /relay-token|messages-bridge-token/);
  assert.doesNotMatch(deployment, /- name: IMESSAGE_RELAY_TOKEN\n\s+valueFrom:/);
  assert.doesNotMatch(deployment, /- name: MESSAGES_BRIDGE_TOKEN\n\s+valueFrom:/);
  assert.match(deployment, /name: IMESSAGE_RELAY_TOKEN_FILE\s+value: \/var\/run\/imessage-relay\/secrets\/relay-token/);
  assert.match(
    deployment,
    /name: MESSAGES_BRIDGE_TOKEN_FILE\s+value: \/var\/run\/imessage-relay\/secrets\/messages-bridge-token/,
  );
  assert.match(deployment, /- name: relay-secrets\n\s+mountPath: \/var\/run\/imessage-relay\/secrets\n\s+readOnly: true/);
  assert.match(deployment, /limits: \{ cpu: 100m, memory: 192Mi \}/);
  assert.match(networkPolicy, /name: allow-imessage-relay-ingress-private-callers/);
  assert.match(networkPolicy, /cidr: 192\.0\.2\.0\/24/);
  assert.doesNotMatch(networkPolicy, /10\.0\.0\.0\/8/);
});

test("relay image must be replaced by a non-OCI overlay immutable digest", () => {
  const deployment = readFileSync(deploymentPath, "utf8");

  assert.match(deployment, /image: .+@sha256:[0-9a-f]{64}/);
  assert.doesNotMatch(deployment, /^\s*image:\s*console-imessage-relay\s*$/m);
});
