#!/usr/bin/env node
import { lstatSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { execFileSync } from "node:child_process";
import {
  documentationArchiveAdmissionFailures,
  validateDocumentationArchives,
} from "./validate-documentation-archive.mjs";

const root = realpathSync(process.cwd());
const defaultManifest = "docs/documentation-manifest.seed.json";
const indexName = "docs/documentation-index.json";
// Keep these literals synchronized with scripts/check-doc-links.mjs.
const excludedDocumentationPrefixes = [
  "buck-out/",
  "node_modules/",
  "target/",
  "third-party/",
  // Agent delivery harness (workflows/briefs) — not product documentation custody.
  ".grok/",
];
const classVocabulary = [
  "current",
  "decision",
  "executable-contract",
  "evidence",
  "historical",
  "quarry",
];
const statusVocabulary = ["active", "frozen", "redirect"];
const retentionVocabulary = [
  "retain",
  "retain as context",
  "one-release redirect",
  "retain as historical reconciliation",
];
const recordFields = [
  "path",
  "class",
  "owner",
  "status",
  "replacement",
  "retention",
  "blob_sha",
  "archive_tag",
];
const rootIndexFields = [
  "schema_version",
  "coverage",
  "class_vocabulary",
  "future_full_manifest_fields",
  "entry",
  "authorities",
  "transitions",
  "documents",
];

function usage() {
  console.error("usage: generate-documentation-manifest.mjs [--check|--write] [--pilot <file>|--file <file>]");
  process.exitCode = 2;
}

function parseArguments(args) {
  let mode = "check";
  let sawMode = false;
  let manifest = defaultManifest;
  let scope = null;
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--check" || arg === "--write") {
      if (sawMode) return null;
      mode = arg.slice(2);
      sawMode = true;
    } else if (arg === "--pilot" || arg === "--file") {
      const value = args[index + 1];
      if (scope || !value || value.startsWith("--")) return null;
      scope = arg.slice(2);
      manifest = value;
      index += 1;
    } else {
      return null;
    }
  }
  return { mode, manifest, scope };
}

function safeExistingPath(path, kind) {
  if (isAbsolute(path)) throw new Error(`${kind} path must be relative to the worktree`);
  const candidate = resolve(root, path);
  const fromRoot = relative(root, candidate);
  if (!fromRoot || fromRoot === ".." || fromRoot.startsWith(`..${sep}`) || isAbsolute(fromRoot)) {
    throw new Error(`${kind} path must remain inside the worktree`);
  }
  if (!lstatSync(candidate).isFile()) {
    throw new Error(`${kind} path must be a regular file inside the worktree`);
  }
  const realPath = realpathSync(candidate);
  const realFromRoot = relative(root, realPath);
  if (!realFromRoot || realFromRoot === ".." || realFromRoot.startsWith(`..${sep}`) || isAbsolute(realFromRoot)) {
    throw new Error(`${kind} path must remain inside the worktree`);
  }
  return realPath;
}

function shellArgument(value) {
  return /^[A-Za-z0-9_./-]+$/.test(value)
    ? value
    : `'${value.replaceAll("'", "'\\''")}'`;
}

function regenerationCommand(options) {
  const command = ["node", "scripts/console/generate-documentation-manifest.mjs", "--write"];
  if (options.scope) command.push(`--${options.scope}`, shellArgument(options.manifest));
  return command.join(" ");
}

const gitEnvironment = Object.fromEntries(
  Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_")),
);
gitEnvironment.LC_ALL = "C";
gitEnvironment.GIT_NO_REPLACE_OBJECTS = "1";

function git(args) {
  return execFileSync("git", ["-C", root, ...args], {
    encoding: "utf8",
    env: gitEnvironment,
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function exactGitIndexTree() {
  const treeOid = git(["write-tree"]).trim();
  if (!/^[0-9a-f]{40,64}$/.test(treeOid)) throw new Error("git write-tree returned an unsupported object ID");
  const tracked = new Map();
  for (const record of git(["ls-tree", "-r", "-z", treeOid]).split("\0")) {
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

function readTrackedBlob(entry) {
  return git(["cat-file", "blob", entry.oid]);
}

function isFirstPartyDocumentation(path) {
  return /\.md$/i.test(path)
    && !excludedDocumentationPrefixes.some((prefix) => path.startsWith(prefix));
}

function isRegularBlob(entry) {
  return entry?.type === "blob" && (entry.mode === "100644" || entry.mode === "100755");
}

function readJson(path, bytes, kind) {
  try {
    return JSON.parse(bytes);
  } catch (error) {
    throw new Error(`${path}: cannot read ${kind}: ${error.message}`);
  }
}

function readManifest(path, manifestPath) {
  const value = readJson(path, readFileSync(manifestPath, "utf8"), "manifest");
  if (!Array.isArray(value)) throw new Error(`${path}: cannot read manifest: top-level JSON value must be an array`);
  return value;
}

function readCustodiedIndex(tracked) {
  const entry = tracked.get(indexName);
  if (!isRegularBlob(entry)) {
    throw new Error(`${indexName}: must be a regular blob in the exact Git index tree`);
  }
  const value = readJson(indexName, readTrackedBlob(entry), "index");
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${indexName}: root record must be an object`);
  }
  return value;
}

function targetPaths(records, tracked, pilot) {
  if (!pilot) {
    return new Set(
      [...tracked.keys()].filter((path) => isFirstPartyDocumentation(path)),
    );
  }
  const paths = new Set();
  for (const record of records) {
    if (record && typeof record === "object" && !Array.isArray(record) && typeof record.path === "string") {
      paths.add(record.path);
    }
  }
  return paths;
}

function validate(records, tracked, targets, manifest) {
  const failures = [];
  const recordsByPath = new Map();
  const expectedFields = new Set(recordFields);
  for (const [index, record] of records.entries()) {
    const label = `${manifest}: record ${index + 1}`;
    if (!record || typeof record !== "object" || Array.isArray(record)) {
      failures.push(`${label} must be an object`);
      continue;
    }
    for (const field of recordFields) {
      if (!Object.hasOwn(record, field)) failures.push(`${label} is missing ${field}`);
    }
    for (const field of Object.keys(record)) {
      if (!expectedFields.has(field)) failures.push(`${label} has unexpected field: ${field}`);
    }
    if (typeof record.path !== "string" || !record.path) {
      failures.push(`${label} requires a path`);
      continue;
    }
    const entries = recordsByPath.get(record.path) ?? [];
    entries.push(record);
    recordsByPath.set(record.path, entries);
  }

  for (const path of targets) {
    const entries = recordsByPath.get(path) ?? [];
    if (entries.length !== 1) {
      failures.push(`${manifest}: ${path} must have exactly one record (found ${entries.length})`);
      continue;
    }
    const record = entries[0];
    const trackedEntry = tracked.get(path);
    if (!isRegularBlob(trackedEntry) || !isFirstPartyDocumentation(path)) {
      failures.push(`${manifest}: ${path} must be a tracked regular first-party markdown blob`);
      continue;
    }
    if (record.blob_sha !== trackedEntry.oid) {
      failures.push(`${manifest}: ${path} blob_sha does not match the exact Git index-tree blob OID`);
    }
    if (!classVocabulary.includes(record.class)) {
      failures.push(`${manifest}: ${path} class must be one of ${classVocabulary.join(", ")}`);
    }
    if (record.owner !== "repository maintainers") {
      failures.push(`${manifest}: ${path} owner must be repository maintainers`);
    }
    if (!statusVocabulary.includes(record.status)) {
      failures.push(`${manifest}: ${path} status must be one of ${statusVocabulary.join(", ")}`);
    }
    if (!retentionVocabulary.includes(record.retention)) {
      failures.push(`${manifest}: ${path} retention must be one of ${retentionVocabulary.join(", ")}`);
    }
    if (record.replacement !== null && (typeof record.replacement !== "string" || !record.replacement.trim())) {
      failures.push(`${manifest}: ${path} replacement must be null or a non-empty string`);
    }
    if (record.status === "active" && record.replacement !== null) {
      failures.push(`${manifest}: ${path} active record must not have a replacement`);
    }
    if (record.status === "redirect" && (typeof record.replacement !== "string" || !record.replacement.trim())) {
      failures.push(`${manifest}: ${path} redirect record requires a replacement`);
    }
    failures.push(...documentationArchiveAdmissionFailures(
      record,
      trackedEntry,
      `${manifest}: ${path}`,
    ));
  }

  for (const path of recordsByPath.keys()) {
    if (!targets.has(path)) failures.push(`${manifest}: record path is outside this manifest scope: ${path}`);
  }
  return failures;
}

function updatedManifest(records, tracked, targets) {
  const updated = records.map((record) => (
    record && typeof record === "object" && !Array.isArray(record) ? { ...record } : record
  ));
  const recordsByPath = new Map();
  for (const record of updated) {
    if (record && typeof record === "object" && !Array.isArray(record) && typeof record.path === "string") {
      const entries = recordsByPath.get(record.path) ?? [];
      entries.push(record);
      recordsByPath.set(record.path, entries);
    }
  }
  for (const path of targets) {
    const entry = tracked.get(path);
    if (!isRegularBlob(entry)) continue;
    const entries = recordsByPath.get(path) ?? [];
    if (entries.length === 0) {
      updated.push({ path, blob_sha: entry.oid });
    } else {
      for (const record of entries) {
        record.path = path;
        record.blob_sha = entry.oid;
      }
    }
  }
  updated.sort((left, right) => {
    const leftPath = String(left?.path ?? "");
    const rightPath = String(right?.path ?? "");
    return (leftPath > rightPath) - (leftPath < rightPath);
  });
  return updated;
}

function generatedIndex(index, records) {
  return {
    schema_version: 2,
    coverage: "first-party-manifest",
    class_vocabulary: [...classVocabulary],
    future_full_manifest_fields: [...recordFields],
    entry: index.entry,
    authorities: index.authorities,
    transitions: index.transitions,
    documents: records.map((record) => (
      record && typeof record === "object" && !Array.isArray(record) ? { ...record } : record
    )),
  };
}

function jsonBytes(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}

function validateGeneratedIndex(index, expectedBytes) {
  const failures = [];
  const fields = Object.keys(index);
  if (fields.length !== rootIndexFields.length || !rootIndexFields.every((field) => fields.includes(field))) {
    failures.push(`${indexName}: generated root fields are stale`);
  }
  const actualBytes = jsonBytes(index);
  if (actualBytes !== expectedBytes) {
    failures.push(`${indexName}: generated bytes or preserved semantics are stale`);
  }
  return failures;
}

const options = parseArguments(process.argv.slice(2));
if (!options) {
  usage();
} else {
  try {
    const manifestPath = safeExistingPath(options.manifest, "manifest");
    const records = readManifest(options.manifest, manifestPath);
    const snapshot = exactGitIndexTree();
    const { tracked } = snapshot;
    const targets = targetPaths(records, tracked, options.scope === "pilot");
    const index = options.scope === "pilot" ? null : readCustodiedIndex(tracked);
    if (options.mode === "write") {
      const updated = updatedManifest(records, tracked, targets);
      writeFileSync(manifestPath, jsonBytes(updated), "utf8");
      if (index) {
        const generatedPath = safeExistingPath(indexName, "index");
        writeFileSync(generatedPath, jsonBytes(generatedIndex(index, updated)), "utf8");
      }
    } else {
      const failures = validate(records, tracked, targets, options.manifest);
      if (index) {
        const expectedBytes = jsonBytes(generatedIndex(index, records));
        failures.push(...validateGeneratedIndex(index, expectedBytes));
      }
      if (failures.length === 0) {
        const archiveValidation = validateDocumentationArchives({
          root,
          index: index ?? { documents: records },
          tracked,
          treeOid: snapshot.treeOid,
        });
        failures.push(...archiveValidation.failures);
      }
      if (failures.length) {
        console.error([
          ...failures,
          `Regenerate with: ${regenerationCommand(options)}`,
        ].join("\n"));
        process.exitCode = 1;
      } else {
        console.log(`documentation manifest OK (${targets.size} markdown files)`);
      }
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
