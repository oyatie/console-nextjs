#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  CONSOLE_CANDIDATE_SIGNING_AUTHORITY,
  CONSOLE_SSH_ALLOWED_SIGNERS_PATH,
  sshSignatureMatchesAuthority,
} from "./ssh-signature-policy.mjs";

export const DOCUMENTATION_ARCHIVE_REMOTE = "https://github.com/oyatie/console.git";
export const DOCUMENTATION_ARCHIVE_KIND = "console-documentation-archive";
export const DOCUMENTATION_ARCHIVE_SCHEMA_VERSION = 1;
export const DOCUMENTATION_ARCHIVE_REF_PREFIX = "refs/tags/archive/documentation/v1/";

const INDEX_PATH = "docs/documentation-index.json";
const ARCHIVE_TAG_FIELDS = ["ref", "tag_oid"];
const ARCHIVABLE_CLASSES = new Set(["historical", "quarry"]);
const ARCHIVABLE_STATUSES = new Set(["frozen", "redirect"]);
const OBJECT_ID = /^[0-9a-f]{40}$/;
const POLICY_LINE = /^([^\s]+) (ssh-(?:ed25519|rsa|ecdsa-[A-Za-z0-9@._+-]+)) ([A-Za-z0-9+/]+={0,2})$/;
const MAX_GIT_OUTPUT = 32 * 1024 * 1024;

function gitEnvironment() {
  const environment = Object.fromEntries(
    Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_")),
  );
  environment.LC_ALL = "C";
  environment.LANG = "C";
  environment.GIT_CONFIG_GLOBAL = "/dev/null";
  environment.GIT_CONFIG_NOSYSTEM = "1";
  environment.GIT_NO_REPLACE_OBJECTS = "1";
  environment.GIT_OPTIONAL_LOCKS = "0";
  environment.GIT_TERMINAL_PROMPT = "0";
  environment.GCM_INTERACTIVE = "never";
  return environment;
}

function command(program, args, options = {}) {
  const result = spawnSync(program, args, {
    cwd: options.cwd,
    env: gitEnvironment(),
    input: options.input,
    encoding: Object.hasOwn(options, "encoding") ? options.encoding : "utf8",
    maxBuffer: MAX_GIT_OUTPUT,
    timeout: options.timeout ?? 60_000,
    stdio: [options.input === undefined ? "ignore" : "pipe", "pipe", "pipe"],
  });
  if (result.error || result.status !== 0) {
    const error = new Error(options.failure ?? `${program} failed`);
    error.code = options.code ?? "COMMAND_FAILED";
    throw error;
  }
  return result.stdout;
}

function git(root, args, options = {}) {
  return command("git", ["-C", root, ...args], options);
}

function isRegularBlob(entry) {
  return entry?.type === "blob" && (entry.mode === "100644" || entry.mode === "100755");
}

function readTrackedBlob(root, entry, label) {
  if (!isRegularBlob(entry)) throw new Error(`${label} is not a tracked regular blob`);
  return git(root, ["cat-file", "blob", entry.oid], {
    failure: `${label} cannot be read from the exact Git index tree`,
  });
}

function exactGitIndexTree(root) {
  const treeOid = git(root, ["write-tree"], {
    failure: "cannot establish the exact Git index tree",
  }).trim();
  if (!/^[0-9a-f]{40,64}$/.test(treeOid)) {
    throw new Error("exact Git index tree has an unsupported object ID");
  }
  const tracked = new Map();
  const records = git(root, ["ls-tree", "-r", "-z", treeOid], {
    failure: "cannot enumerate the exact Git index tree",
  }).split("\0");
  for (const record of records) {
    if (!record) continue;
    const tab = record.indexOf("\t");
    const [mode, type, oid] = record.slice(0, tab).split(" ");
    const path = record.slice(tab + 1);
    if (
      tab < 0
      || !/^\d{6}$/.test(mode ?? "")
      || !type
      || !/^[0-9a-f]{40,64}$/.test(oid ?? "")
      || !path
      || tracked.has(path)
    ) {
      throw new Error("exact Git index tree contains an unsupported entry");
    }
    tracked.set(path, { mode, type, oid });
  }
  return { treeOid, tracked };
}

function indexedDocumentation(root, tracked) {
  const entry = tracked.get(INDEX_PATH);
  const bytes = readTrackedBlob(root, entry, INDEX_PATH);
  let index;
  try {
    index = JSON.parse(bytes);
  } catch {
    throw new Error(`${INDEX_PATH} is not valid indexed JSON`);
  }
  if (!index || typeof index !== "object" || Array.isArray(index) || !Array.isArray(index.documents)) {
    throw new Error(`${INDEX_PATH} has no document array`);
  }
  return index;
}

export function documentationArchiveRef(record) {
  const digest = createHash("sha256")
    .update([DOCUMENTATION_ARCHIVE_KIND, "v1", record.path, record.blob_sha, "100644"].join("\0"))
    .digest("hex");
  return `${DOCUMENTATION_ARCHIVE_REF_PREFIX}${digest}`;
}

export function documentationArchiveMessage(record) {
  return JSON.stringify({
    schema_version: DOCUMENTATION_ARCHIVE_SCHEMA_VERSION,
    kind: DOCUMENTATION_ARCHIVE_KIND,
    path: record.path,
    blob_sha: record.blob_sha,
    mode: "100644",
  });
}

export function documentationArchiveAdmissionFailures(record, trackedEntry, label = record?.path ?? "document") {
  if (record?.archive_tag === null) return [];
  const failures = [];
  const prefix = `${label}: archive_tag`;
  const value = record?.archive_tag;
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return [`${prefix} must be null or an exact pinned object`];
  }
  const fields = Object.keys(value);
  for (const field of ARCHIVE_TAG_FIELDS) {
    if (!Object.hasOwn(value, field)) failures.push(`${prefix} is missing ${field}`);
  }
  for (const field of fields) {
    if (!ARCHIVE_TAG_FIELDS.includes(field)) failures.push(`${prefix} has unexpected field: ${field}`);
  }
  if (typeof record?.path !== "string" || !record.path) {
    failures.push(`${prefix} requires a document path`);
  }
  if (!OBJECT_ID.test(record?.blob_sha ?? "")) {
    failures.push(`${prefix} requires a 40-character lowercase blob_sha`);
  }
  if (!ARCHIVABLE_CLASSES.has(record?.class)) {
    failures.push(`${prefix} requires class historical or quarry`);
  }
  if (!ARCHIVABLE_STATUSES.has(record?.status)) {
    failures.push(`${prefix} requires status frozen or redirect`);
  }
  if (trackedEntry?.type !== "blob" || trackedEntry?.mode !== "100644") {
    failures.push(`${prefix} requires a live 100644 regular blob`);
  }
  if (trackedEntry?.oid !== record?.blob_sha) {
    failures.push(`${prefix} blob_sha must match the exact live indexed blob`);
  }
  if (typeof value.ref !== "string" || value.ref !== documentationArchiveRef(record)) {
    failures.push(`${prefix} ref does not match the derived documentation archive ref`);
  }
  if (typeof value.tag_oid !== "string" || !OBJECT_ID.test(value.tag_oid)) {
    failures.push(`${prefix} tag_oid must be a 40-character lowercase annotated-tag object ID`);
  }
  return failures;
}

function authorityIsValid(authority) {
  return authority?.format === "ssh"
    && typeof authority.principal === "string"
    && authority.principal !== ""
    && authority.principal.trim() === authority.principal
    && /^SHA256:[A-Za-z0-9+/]+={0,2}$/.test(authority.fingerprint ?? "");
}

function trustedPolicy(root, tracked, authority) {
  if (!authorityIsValid(authority)) throw new Error("archive signing authority is invalid");
  const entry = tracked.get(CONSOLE_SSH_ALLOWED_SIGNERS_PATH);
  if (entry?.mode !== "100644" || entry?.type !== "blob") {
    throw new Error("indexed SSH allowed-signers policy is missing or not a 100644 blob");
  }
  const policy = readTrackedBlob(root, entry, CONSOLE_SSH_ALLOWED_SIGNERS_PATH);
  const lines = policy.split(/\r?\n/).filter(Boolean);
  const match = lines.length === 1 && lines[0].match(POLICY_LINE);
  if (!match || match[1] !== authority.principal) {
    throw new Error("indexed SSH allowed-signers policy principal is not trusted");
  }
  const fingerprint = command("ssh-keygen", ["-lf", "-", "-E", "sha256"], {
    input: `${match[2]} ${match[3]}\n`,
    failure: "indexed SSH allowed-signers policy key cannot be fingerprinted",
  }).trim().split(/\s+/)[1];
  if (fingerprint !== authority.fingerprint) {
    throw new Error("indexed SSH allowed-signers policy fingerprint is not trusted");
  }
  return { bytes: policy, oid: entry.oid };
}

function tagObject(tagBytes, expectedRef, expectedMessage) {
  const separator = tagBytes.indexOf("\n\n");
  if (separator < 0) throw new Error("annotated tag object has no header/message boundary");
  const headerLines = tagBytes.slice(0, separator).split("\n");
  if (headerLines.length !== 4) throw new Error("annotated tag object has non-canonical headers");
  const objectMatch = headerLines[0].match(/^object ([0-9a-f]{40})$/);
  const typeMatch = headerLines[1].match(/^type ([a-z]+)$/);
  const nameMatch = headerLines[2].match(/^tag (.+)$/);
  const taggerMatch = headerLines[3].match(/^tagger .+ [0-9]+ [+-][0-9]{4}$/);
  if (!objectMatch || !typeMatch || !nameMatch || !taggerMatch) {
    throw new Error("annotated tag object has non-canonical headers");
  }
  if (typeMatch[1] !== "commit") throw new Error("annotated tag must target a commit directly");
  if (nameMatch[1] !== expectedRef.slice("refs/tags/".length)) {
    throw new Error("annotated tag internal name does not match the pinned ref");
  }
  const body = tagBytes.slice(separator + 2);
  const begin = "-----BEGIN SSH SIGNATURE-----";
  const end = "-----END SSH SIGNATURE-----";
  const beginCount = body.split(begin).length - 1;
  const endCount = body.split(end).length - 1;
  const signatureOffset = body.indexOf(begin);
  if (beginCount !== 1 || endCount !== 1 || signatureOffset < 0) {
    throw new Error("annotated tag must contain exactly one SSH signature");
  }
  if (body.slice(0, signatureOffset) !== `${expectedMessage}\n`) {
    throw new Error("annotated tag message does not match canonical archive metadata");
  }
  if (!body.slice(signatureOffset).endsWith(`${end}\n`)) {
    throw new Error("annotated tag SSH signature envelope is not canonical");
  }
  return { targetOid: objectMatch[1] };
}

function verifyTagSignature(repository, tagOid, policy, authority) {
  const directory = mkdtempSync(join(tmpdir(), "console-doc-archive-policy-"));
  const policyPath = join(directory, "allowed_signers");
  try {
    writeFileSync(policyPath, policy, { mode: 0o600 });
    chmodSync(policyPath, 0o600);
    const result = spawnSync(
      "git",
      [
        "-C",
        repository,
        "-c",
        "gpg.format=ssh",
        "-c",
        "gpg.ssh.program=ssh-keygen",
        "-c",
        `gpg.ssh.allowedSignersFile=${policyPath}`,
        "verify-tag",
        "--raw",
        tagOid,
      ],
      {
        env: gitEnvironment(),
        encoding: "utf8",
        maxBuffer: MAX_GIT_OUTPUT,
        timeout: 60_000,
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    const status = `${result.stdout ?? ""}${result.stderr ?? ""}`;
    if (result.error || result.status !== 0 || !sshSignatureMatchesAuthority(status, authority)) {
      throw new Error("annotated tag is not signed exactly once by the indexed trusted SSH authority");
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function validateTreeEntry(repository, targetOid, record) {
  const targetType = git(repository, ["cat-file", "-t", targetOid], {
    failure: "annotated tag target is unavailable",
  }).trim();
  if (targetType !== "commit") throw new Error("annotated tag must target a commit directly");
  const output = git(
    repository,
    ["--literal-pathspecs", "ls-tree", "-z", "--full-tree", targetOid, "--", record.path],
    { failure: "archive commit tree cannot be inspected" },
  );
  const rows = output.split("\0").filter(Boolean);
  if (rows.length !== 1) throw new Error("archive commit must contain exactly the pinned document path");
  const tab = rows[0].indexOf("\t");
  const [mode, type, oid] = rows[0].slice(0, tab).split(" ");
  const path = rows[0].slice(tab + 1);
  if (tab < 0 || mode !== "100644" || type !== "blob" || path !== record.path) {
    throw new Error("archive commit path must be the exact 100644 document blob");
  }
  if (oid !== record.blob_sha) throw new Error("archive commit blob does not match the pinned blob_sha");
  const bytes = git(repository, ["cat-file", "blob", oid], {
    encoding: null,
    failure: "archive document blob cannot be read",
  });
  const rehashed = git(repository, ["hash-object", "--stdin"], {
    input: bytes,
    failure: "archive document blob cannot be rehashed",
  }).trim();
  if (rehashed !== oid) throw new Error("archive document blob failed exact hash read-back");
}

function validateCandidate(record, remote, policy, authority, allowLocalRemote) {
  const ref = record.archive_tag.ref;
  const tagOid = record.archive_tag.tag_oid;
  const repository = mkdtempSync(join(tmpdir(), "console-doc-archive-fetch-"));
  try {
    git(repository, ["init", "--bare", "-q"], {
      failure: "cannot initialize fresh archive verification repository",
    });
    const protocolConfig = allowLocalRemote ? ["-c", "protocol.file.allow=always"] : ["-c", "protocol.file.allow=never"];
    git(
      repository,
      [
        ...protocolConfig,
        "fetch",
        "--no-tags",
        "--depth=1",
        "--filter=blob:none",
        "--",
        remote,
        `+${ref}:${ref}`,
      ],
      { failure: "fresh fetch of the pinned archive ref failed" },
    );
    const refs = git(repository, ["for-each-ref", "--format=%(refname)"], {
      failure: "fresh archive refs cannot be enumerated",
    }).split(/\r?\n/).filter(Boolean);
    if (refs.length !== 1 || refs[0] !== ref) {
      throw new Error("fresh fetch admitted refs outside the single pinned archive ref");
    }
    const fetchedOid = git(repository, ["show-ref", "--verify", "--hash", ref], {
      failure: "freshly fetched archive ref is missing",
    }).trim();
    if (fetchedOid !== tagOid) throw new Error("freshly fetched archive ref does not match pinned tag_oid");
    const type = git(repository, ["cat-file", "-t", tagOid], {
      failure: "pinned archive tag object is unavailable",
    }).trim();
    if (type !== "tag") throw new Error("pinned archive ref is not an annotated tag object");
    const tagBytes = git(repository, ["cat-file", "tag", tagOid], {
      failure: "pinned annotated tag object cannot be read",
    });
    const { targetOid } = tagObject(tagBytes, ref, documentationArchiveMessage(record));
    verifyTagSignature(repository, tagOid, policy, authority);
    validateTreeEntry(repository, targetOid, record);
  } finally {
    rmSync(repository, { recursive: true, force: true });
  }
}

function report(treeOid, indexEntry, policyOid, candidates, validated, failures, results) {
  return {
    failures,
    telemetry: {
      schema_version: DOCUMENTATION_ARCHIVE_SCHEMA_VERSION,
      kind: "console-documentation-archive-validation",
      index_tree_oid: treeOid ?? null,
      index_blob_oid: indexEntry?.oid ?? null,
      signer_policy_blob_oid: policyOid ?? null,
      candidate_count: candidates,
      validated_count: validated,
      failure_count: failures.length,
      results,
    },
  };
}

export function validateDocumentationArchives(options = {}) {
  const root = resolve(options.root ?? process.cwd());
  let snapshot;
  let index;
  try {
    snapshot = options.tracked && options.treeOid
      ? { tracked: options.tracked, treeOid: options.treeOid }
      : exactGitIndexTree(root);
    index = options.index ?? indexedDocumentation(root, snapshot.tracked);
  } catch (error) {
    const failures = [`${INDEX_PATH}: archive validation cannot establish indexed custody: ${error.message}`];
    return report(snapshot?.treeOid, snapshot?.tracked?.get(INDEX_PATH), null, 0, 0, failures, []);
  }
  if (!Array.isArray(index?.documents)) {
    const failures = [`${INDEX_PATH}: archive validation requires an indexed document array`];
    return report(snapshot.treeOid, snapshot.tracked.get(INDEX_PATH), null, 0, 0, failures, []);
  }
  const records = index.documents;
  const candidates = records.filter((record) => record?.archive_tag !== null);
  if (candidates.length === 0) {
    return report(snapshot.treeOid, snapshot.tracked.get(INDEX_PATH), null, 0, 0, [], []);
  }

  const failures = [];
  const results = [];
  const refs = new Map();
  const tagOids = new Map();
  for (const record of candidates) {
    const label = `${INDEX_PATH}: ${record?.path ?? "document"}`;
    const admission = documentationArchiveAdmissionFailures(record, snapshot.tracked.get(record?.path), label);
    failures.push(...admission);
    if (admission.length) continue;
    for (const [kind, value, seen] of [
      ["ref", record.archive_tag.ref, refs],
      ["tag_oid", record.archive_tag.tag_oid, tagOids],
    ]) {
      if (seen.has(value)) failures.push(`${label}: archive_tag duplicates ${kind} claimed by ${seen.get(value)}`);
      else seen.set(value, record.path);
    }
  }
  if (failures.length) {
    return report(snapshot.treeOid, snapshot.tracked.get(INDEX_PATH), null, candidates.length, 0, failures, results);
  }

  const authority = options.testOnlyAuthority ?? CONSOLE_CANDIDATE_SIGNING_AUTHORITY;
  let policy;
  try {
    policy = trustedPolicy(root, snapshot.tracked, authority);
  } catch (error) {
    failures.push(`${CONSOLE_SSH_ALLOWED_SIGNERS_PATH}: ${error.message}`);
    return report(snapshot.treeOid, snapshot.tracked.get(INDEX_PATH), null, candidates.length, 0, failures, results);
  }
  const remote = options.testOnlyRemoteUrl ?? DOCUMENTATION_ARCHIVE_REMOTE;
  const allowLocalRemote = Object.hasOwn(options, "testOnlyRemoteUrl");
  let validated = 0;
  for (const record of candidates) {
    try {
      validateCandidate(record, remote, policy.bytes, authority, allowLocalRemote);
      validated += 1;
      results.push({ path: record.path, ref: record.archive_tag.ref, tag_oid: record.archive_tag.tag_oid, status: "validated" });
    } catch (error) {
      failures.push(`${INDEX_PATH}: ${record.path}: ${error.message}`);
      results.push({ path: record.path, ref: record.archive_tag.ref, tag_oid: record.archive_tag.tag_oid, status: "rejected" });
    }
  }
  return report(
    snapshot.treeOid,
    snapshot.tracked.get(INDEX_PATH),
    policy.oid,
    candidates.length,
    validated,
    failures,
    results,
  );
}

function main() {
  if (process.argv.length > 3) {
    console.error("usage: validate-documentation-archive.mjs [worktree]");
    process.exitCode = 2;
    return;
  }
  const validation = validateDocumentationArchives({ root: process.argv[2] ?? process.cwd() });
  console.log(JSON.stringify(validation.telemetry));
  if (validation.failures.length) {
    console.error(validation.failures.join("\n"));
    process.exitCode = 1;
  }
}

const invokedPath = process.argv[1] ? resolve(process.argv[1]) : null;
if (invokedPath === fileURLToPath(import.meta.url)) main();
