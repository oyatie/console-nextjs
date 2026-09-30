#!/usr/bin/env node
import { constants } from "node:fs";
import { access, stat } from "node:fs/promises";

const BLOCKER_SUMMARY = "BLOCKED_PENDING_NON_OCI_TALOS_CREDENTIALS";
const BLOCKER = "blocked_missing_non_oci_talos_or_bridge_credentials";
const MIN_BRIDGE_TOKEN_LENGTH = 20;

const args = new Set(process.argv.slice(2));

if (!args.has("--dry-run")) {
  console.error("usage: node scripts/check-non-oci-mail-imessage-relay.mjs --dry-run");
  process.exit(64);
}

const fileInputs = [
  "NON_OCI_TALOS_KUBECONFIG",
  "NON_OCI_TALOSCONFIG",
  "MESSAGES_BRIDGE_TLS_CA_PATH",
  "MESSAGES_BRIDGE_TLS_CERT_PATH",
  "MESSAGES_BRIDGE_TLS_KEY_PATH",
];

const valueInputs = [
  "MESSAGES_BRIDGE_URL",
  "MESSAGES_BRIDGE_TOKEN",
];

const missing = [];

for (const name of valueInputs) {
  if (!hasValue(process.env[name])) {
    missing.push(name);
  }
}

if (hasValue(process.env.MESSAGES_BRIDGE_URL) && !isHttpsUrl(process.env.MESSAGES_BRIDGE_URL)) {
  missing.push("MESSAGES_BRIDGE_URL");
}

if (
  hasValue(process.env.MESSAGES_BRIDGE_TOKEN) &&
  process.env.MESSAGES_BRIDGE_TOKEN.trim().length < MIN_BRIDGE_TOKEN_LENGTH
) {
  missing.push("MESSAGES_BRIDGE_TOKEN");
}

for (const name of fileInputs) {
  const value = process.env[name];
  if (!hasValue(value) || !(await canRead(value))) {
    missing.push(name);
  }
}

if (missing.length > 0) {
  console.log(BLOCKER_SUMMARY);
  console.log(BLOCKER);
  process.exit(2);
}

console.log("non_oci_talos_mail_imessage_relay_dry_run_ready");

function hasValue(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function isHttpsUrl(value) {
  try {
    return new URL(value).protocol === "https:";
  } catch {
    return false;
  }
}

async function canRead(path) {
  try {
    await access(path, constants.R_OK);
    const entry = await stat(path);
    return entry.isFile();
  } catch {
    return false;
  }
}
