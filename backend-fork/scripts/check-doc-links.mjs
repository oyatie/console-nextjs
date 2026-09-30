#!/usr/bin/env node
import { existsSync, lstatSync, readFileSync, readdirSync } from "node:fs";
import { join, resolve, relative, isAbsolute } from "node:path";
import { execFileSync } from "node:child_process";
import { validateDocumentationArchives } from "./console/validate-documentation-archive.mjs";

const root = resolve(process.argv[2] ?? process.cwd());
const failures = [];

const indexPath = join(root, "docs/documentation-index.json");
const entryPath = "README.md";
const allowedAuthorities = new Map([
  ["product", "docs/current/PRODUCT.md"],
  ["roadmap", "docs/current/ROADMAP.md"],
  ["delivery", "docs/current/DELIVERY.md"],
]);
const expectedTransitions = [
  {
    path: "SPEC.md",
    class: "historical",
    owner: "repository maintainers",
    status: "redirect",
    replacement: "docs/current/PRODUCT.md",
    retention: "one-release redirect",
  },
  {
    path: "DESIGN.md",
    class: "historical",
    owner: "repository maintainers",
    status: "redirect",
    replacement: "docs/current/PRODUCT.md",
    retention: "one-release redirect",
  },
  {
    path: "docs/PIVOT-2026-07-28.md",
    class: "historical",
    owner: "repository maintainers",
    status: "frozen",
    replacement: "docs/current/PRODUCT.md",
    retention: "retain as historical reconciliation",
  },
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
const fullManifestFields = [
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
const entryRecordFields = ["path", "class", "owner", "status", "replacement", "retention"];
const authorityRecordFields = ["concern", ...entryRecordFields];
const transitionRecordFields = entryRecordFields;
const excludedDocumentationPrefixes = [
  "buck-out/",
  "node_modules/",
  "target/",
  "third-party/",
  // Agent delivery harness — not product documentation custody.
  ".grok/",
];

function isFirstPartyDocumentation(path) {
  return /\.md$/i.test(path)
    && !excludedDocumentationPrefixes.some((prefix) => path.startsWith(prefix));
}

function sameMembers(actual, expected) {
  return Array.isArray(actual)
    && actual.length === expected.length
    && new Set(actual).size === actual.length
    && expected.every((value) => actual.includes(value));
}

const gitEnvironment = Object.fromEntries(
  Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_")),
);
gitEnvironment.LC_ALL = "C";
gitEnvironment.GIT_NO_REPLACE_OBJECTS = "1";

function git(args) {
  return execFileSync(
    "git",
    ["-C", root, ...args],
    { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], env: gitEnvironment },
  );
}

function exactGitIndexTree() {
  try {
    const treeOid = git(["write-tree"]).trim();
    const records = git(["ls-tree", "-r", "-z", treeOid]).split("\0").filter(Boolean);
    const tracked = new Map();
    for (const record of records) {
      const tab = record.indexOf("\t");
      const [mode, type, oid] = record.slice(0, tab).split(" ");
      const path = record.slice(tab + 1);
      if (tab < 0 || !/^\d{6}$/.test(mode ?? "") || !/^[0-9a-f]{40,64}$/.test(oid ?? "") || !type || !path || tracked.has(path)) {
        failures.push("docs/documentation-index.json: exact Git index tree contains an unsupported entry");
        continue;
      }
      tracked.set(path, { mode, type, oid });
    }
    return { treeOid, tracked };
  } catch {
    failures.push("docs/documentation-index.json: cannot establish exact Git index-tree custody");
    return null;
  }
}

function isGitWorktree() {
  try {
    return git(["rev-parse", "--is-inside-work-tree"]).trim() === "true";
  } catch (error) {
    if (/not a git repository/i.test(String(error?.stderr ?? ""))) return false;
    failures.push("docs/documentation-index.json: cannot determine Git worktree custody");
    return null;
  }
}

function isRegularBlob(entry) {
  return entry?.type === "blob" && (entry.mode === "100644" || entry.mode === "100755");
}

function readTrackedBlob(entry) {
  try {
    return git(["cat-file", "blob", entry.oid]);
  } catch {
    return null;
  }
}

function requireTrackedRegular(path, tracked, kind) {
  const entry = tracked.get(path);
  if (!entry) {
    const message = kind === "index"
      ? "index must be a regular blob in the exact Git index tree"
      : `${kind} path is not in the exact Git index tree: ${path}`;
    failures.push(`docs/documentation-index.json: ${message}`);
    return null;
  }
  if (!isRegularBlob(entry)) {
    failures.push(`docs/documentation-index.json: ${kind} path must be a regular blob in the exact Git index tree: ${path}`);
    return null;
  }
  return entry;
}

function requireExactRecordFields(record, label, fields) {
  if (!record || typeof record !== "object" || Array.isArray(record)) {
    failures.push(`docs/documentation-index.json: ${label} must be an object`);
    return false;
  }
  const allowed = new Set(fields);
  for (const field of fields) {
    if (!Object.hasOwn(record, field)) {
      failures.push(`docs/documentation-index.json: ${label} is missing ${field}`);
    }
  }
  for (const field of Object.keys(record)) {
    if (!allowed.has(field)) {
      failures.push(`docs/documentation-index.json: ${label} has unexpected field: ${field}`);
    }
  }
  return true;
}

function validateReadmeAuthorityDeclaration(contents) {
  const lines = contents.split(/\r?\n/);
  const headings = lines
    .map((line, index) => line.trim() === "## Current authority" ? index : -1)
    .filter((index) => index !== -1);
  const expected = [...allowedAuthorities.values()];
  if (headings.length !== 1) {
    failures.push("README.md: exactly one Current authority section is required");
    return;
  }

  const start = headings[0] + 1;
  const end = lines.findIndex((line, index) => index >= start && /^##\s/.test(line));
  const section = lines
    .slice(start, end === -1 ? lines.length : end)
    .filter((line) => line.trim());
  let valid = section.length === expected.length;
  for (let index = 0; index < section.length; index += 1) {
    const line = section[index];
    const match = line.match(/^\s*(\d+)\.\s+!?\[[^\]]*\]\(\s*(<[^>]+>|[^\s)]+)/);
    const links = [...line.matchAll(/!?\[[^\]]*\]\(\s*(<[^>]+>|[^\s)]+)/g)];
    const path = match?.[2]?.replace(/^<|>$/g, "");
    if (!match || Number(match[1]) !== index + 1 || links.length !== 1 || path !== expected[index]) {
      valid = false;
    }
  }
  if (!valid) {
    failures.push(`README.md: current authority list must contain exactly ${expected.join(", ")}`);
  }
}

function validateDocumentationIndex(worktree, snapshot) {
  if (worktree !== true) {
    if (existsSync(indexPath)) {
      if (worktree === false) failures.push("docs/documentation-index.json: index requires a Git worktree");
    } else if (worktree === null) {
      // isGitWorktree already recorded a fail-closed diagnostic.
    }
    return;
  }

  if (!snapshot) return;
  const { tracked } = snapshot;
  if (!tracked.has("docs/documentation-index.json")) {
    failures.push(existsSync(indexPath)
      ? "docs/documentation-index.json: index must be a regular blob in the exact Git index tree"
      : "docs/documentation-index.json: required in a Git worktree");
    return;
  }

  const indexEntry = requireTrackedRegular("docs/documentation-index.json", tracked, "index");
  if (!indexEntry) return;

  let index;
  try {
    const bytes = readTrackedBlob(indexEntry);
    if (bytes === null) throw new Error("cannot read the indexed Git blob");
    index = JSON.parse(bytes);
  } catch (error) {
    failures.push(`docs/documentation-index.json: invalid JSON: ${error.message}`);
    return;
  }

  if (!requireExactRecordFields(index, "root record", rootIndexFields)) return;

  if (index.schema_version !== 2) {
    failures.push("docs/documentation-index.json: schema_version must be 2");
  }
  if (!sameMembers(index.class_vocabulary, classVocabulary)) {
    failures.push(`docs/documentation-index.json: class_vocabulary must be exactly ${classVocabulary.join(", ")}`);
  }
  if (!sameMembers(index.future_full_manifest_fields, fullManifestFields)) {
    failures.push(`docs/documentation-index.json: future_full_manifest_fields must be exactly ${fullManifestFields.join(", ")}`);
  }
  if (index.coverage === "complete") {
    failures.push("docs/documentation-index.json: coverage complete is not admitted by Phase-A archive validation");
  } else if (index.coverage !== "authority-slice" && index.coverage !== "first-party-manifest") {
    failures.push("docs/documentation-index.json: coverage must be authority-slice or first-party-manifest");
  }

  const entry = index.entry;
  const authorities = Array.isArray(index.authorities) ? index.authorities : [];
  const transitions = Array.isArray(index.transitions) ? index.transitions : [];
  const documentRecords = Array.isArray(index.documents) ? index.documents : [];
  if (!Array.isArray(index.authorities)) failures.push("docs/documentation-index.json: authorities must be an array");
  if (!Array.isArray(index.transitions)) failures.push("docs/documentation-index.json: transitions must be an array");
  if (!Array.isArray(index.documents)) failures.push("docs/documentation-index.json: documents must be an array");

  const concerns = new Set();
  const admittedPaths = new Set();
  const activeAuthorityPaths = new Set();
  if (requireExactRecordFields(
    entry,
    "entry record",
    entryRecordFields,
  )) {
    if (entry.path !== entryPath || entry.class !== "current" || entry.status !== "active" || entry.replacement !== null || entry.owner !== "repository maintainers" || entry.retention !== "retain") {
      failures.push("docs/documentation-index.json: entry must be the active retained README.md record");
    }
    admittedPaths.add(entry.path);
    const readmeEntry = requireTrackedRegular(entry.path, tracked, "entry");
    if (readmeEntry) {
      const contents = readTrackedBlob(readmeEntry);
      if (contents === null) {
        failures.push("README.md: cannot read entry blob from the exact Git index tree");
      } else {
        validateReadmeAuthorityDeclaration(contents);
      }
    }
  }
  for (const record of authorities) {
    if (!requireExactRecordFields(
      record,
      "authority record",
      authorityRecordFields,
    )) continue;
    if (concerns.has(record.concern)) {
      failures.push(`docs/documentation-index.json: duplicate concern: ${record.concern}`);
    }
    concerns.add(record.concern);
    const expectedPath = allowedAuthorities.get(record.concern);
    if (!expectedPath) {
      failures.push(`docs/documentation-index.json: unexpected authority concern: ${record.concern}`);
    } else if (record.path !== expectedPath) {
      failures.push(`docs/documentation-index.json: authority ${record.concern} must use ${expectedPath}`);
    }
    if (admittedPaths.has(record.path)) {
      failures.push(`docs/documentation-index.json: duplicate admitted path: ${record.path}`);
    }
    admittedPaths.add(record.path);
    requireTrackedRegular(record.path, tracked, "authority");
    if (record.class !== "current" || record.status !== "active" || record.replacement !== null) {
      failures.push(`docs/documentation-index.json: authority ${record.concern} must be current, active, and unreplaced`);
    }
    if (record.owner !== "repository maintainers" || record.retention !== "retain") {
      failures.push(`docs/documentation-index.json: authority ${record.concern} must be maintainer-owned and retained`);
    }
    activeAuthorityPaths.add(record.path);
  }
  if (authorities.length !== allowedAuthorities.size) {
    failures.push(`docs/documentation-index.json: exactly ${allowedAuthorities.size} authorities are required`);
  }
  for (const [concern, path] of allowedAuthorities) {
    if (!authorities.some((record) => record?.concern === concern && record.path === path)) {
      failures.push(`docs/documentation-index.json: missing authority: ${concern} -> ${path}`);
    }
  }

  for (const record of transitions) {
    if (!requireExactRecordFields(
      record,
      "transition record",
      transitionRecordFields,
    )) continue;
    if (admittedPaths.has(record.path)) {
      failures.push(`docs/documentation-index.json: duplicate admitted path: ${record.path}`);
    }
    admittedPaths.add(record.path);
    requireTrackedRegular(record.path, tracked, "admitted");
    if (record.class !== "historical") {
      failures.push(`docs/documentation-index.json: transition ${record.path} must be historical`);
    }
    if (record.status !== "redirect" && record.status !== "frozen") {
      failures.push(`docs/documentation-index.json: transition ${record.path} must be redirect or frozen`);
    }
    if (!activeAuthorityPaths.has(record.replacement)) {
      failures.push(`docs/documentation-index.json: ${record.status} replacement is not an active authority: ${record.replacement}`);
    }
    if (typeof record.owner !== "string" || !record.owner.trim() || typeof record.retention !== "string" || !record.retention.trim()) {
      failures.push(`docs/documentation-index.json: transition ${record.path} requires owner and retention`);
    }
  }
  if (transitions.length !== expectedTransitions.length) {
    failures.push(`docs/documentation-index.json: exactly ${expectedTransitions.length} transitions are required`);
  }
  for (const expected of expectedTransitions) {
    const record = transitions.find((candidate) => candidate?.path === expected.path);
    if (!record || !entryRecordFields.every((field) => Object.is(record[field], expected[field]))) {
      failures.push(`docs/documentation-index.json: transition must remain byte-for-value: ${expected.path}`);
    }
  }

  const recordsByPath = new Map();
  let previousPath = null;
  for (const [recordIndex, record] of documentRecords.entries()) {
    const label = `document record ${recordIndex + 1}`;
    if (!requireExactRecordFields(record, label, fullManifestFields)) continue;
    if (typeof record.path !== "string" || !record.path) {
      failures.push(`docs/documentation-index.json: ${label} requires a path`);
      continue;
    }
    if (previousPath !== null && record.path <= previousPath) {
      failures.push("docs/documentation-index.json: documents must be strictly path-sorted");
    }
    previousPath = record.path;
    const entries = recordsByPath.get(record.path) ?? [];
    entries.push(record);
    recordsByPath.set(record.path, entries);

    const trackedEntry = tracked.get(record.path);
    if (!isFirstPartyDocumentation(record.path) || !isRegularBlob(trackedEntry)) {
      failures.push(`docs/documentation-index.json: document path must be a tracked regular first-party markdown blob: ${record.path}`);
      continue;
    }
    if (record.blob_sha !== trackedEntry.oid) {
      failures.push(`docs/documentation-index.json: ${record.path} blob_sha does not match the exact Git index-tree blob OID`);
    }
    if (!classVocabulary.includes(record.class)) {
      failures.push(`docs/documentation-index.json: ${record.path} class must be one of ${classVocabulary.join(", ")}`);
    }
    if (record.owner !== "repository maintainers") {
      failures.push(`docs/documentation-index.json: ${record.path} owner must be repository maintainers`);
    }
    if (!statusVocabulary.includes(record.status)) {
      failures.push(`docs/documentation-index.json: ${record.path} status must be one of ${statusVocabulary.join(", ")}`);
    }
    if (!retentionVocabulary.includes(record.retention)) {
      failures.push(`docs/documentation-index.json: ${record.path} retention must be one of ${retentionVocabulary.join(", ")}`);
    }
    if (record.replacement !== null && (typeof record.replacement !== "string" || !record.replacement.trim())) {
      failures.push(`docs/documentation-index.json: ${record.path} replacement must be null or a non-empty string`);
    }
    if (record.status === "active" && record.replacement !== null) {
      failures.push(`docs/documentation-index.json: ${record.path} active record must not have a replacement`);
    }
    if (record.status === "redirect" && (typeof record.replacement !== "string" || !record.replacement.trim())) {
      failures.push(`docs/documentation-index.json: ${record.path} redirect record requires a replacement`);
    }
  }

  const projections = [
    { label: "entry", record: entry },
    ...authorities.map((record) => ({ label: `authority ${record?.concern}`, record })),
    ...transitions.map((record) => ({ label: `transition ${record?.path}`, record })),
  ];
  for (const projection of projections) {
    const documents = recordsByPath.get(projection.record?.path) ?? [];
    if (documents.length !== 1) {
      failures.push(`docs/documentation-index.json: ${projection.label} requires exactly one document projection`);
      continue;
    }
    for (const field of entryRecordFields) {
      if (!Object.is(documents[0][field], projection.record[field])) {
        failures.push(`docs/documentation-index.json: ${projection.label} document projection differs at ${field}`);
      }
    }
  }

  if (index.coverage === "first-party-manifest") {
    const universe = [...tracked.entries()]
      .filter(([path]) => isFirstPartyDocumentation(path))
      .map(([path]) => path);
    for (const path of universe) {
      const records = recordsByPath.get(path) ?? [];
      if (records.length !== 1) {
        failures.push(`docs/documentation-index.json: ${path} must have exactly one document record (found ${records.length})`);
      }
    }
    for (const path of recordsByPath.keys()) {
      if (!universe.includes(path)) {
        failures.push(`docs/documentation-index.json: document path is outside first-party manifest coverage: ${path}`);
      }
    }
  }

  if (failures.length === 0) {
    const archiveValidation = validateDocumentationArchives({
      root,
      index,
      tracked,
      treeOid: snapshot.treeOid,
    });
    failures.push(...archiveValidation.failures);
  }

}
function walk(dir, files) {
  for (const name of readdirSync(dir)) {
    if (
      name === ".git"
      || name === "buck-out"
      || name === "node_modules"
      || name === "target"
      || name === "third-party"
    ) continue;
    const path = join(dir, name);
    const st = lstatSync(path);
    if (st.isSymbolicLink()) continue;
    if (st.isDirectory()) walk(path, files);
    else if (/\.md$/i.test(name)) files.push({
      path: relative(root, path).split("\\").join("/"),
      contents: readFileSync(path, "utf8"),
    });
  }
}

function localTarget(raw) {
  const value = raw.trim().replace(/^<|>$/g, "");
  if (!value || value.startsWith("#") || /^[a-z][a-z0-9+.-]*:/i.test(value) || value.startsWith("//")) return null;
  const pathPart = value.split("#", 1)[0].split("?", 1)[0];
  if (!pathPart) return null;
  return decodeURIComponent(pathPart);
}

function withoutInlineCode(line) {
  let visible = "";
  let cursor = 0;
  while (cursor < line.length) {
    if (line[cursor] !== "`") {
      visible += line[cursor];
      cursor += 1;
      continue;
    }

    let endOfOpeningRun = cursor + 1;
    while (line[endOfOpeningRun] === "`") endOfOpeningRun += 1;
    const delimiter = line.slice(cursor, endOfOpeningRun);
    const closingRun = line.indexOf(delimiter, endOfOpeningRun);
    if (closingRun === -1) {
      visible += delimiter;
      cursor = endOfOpeningRun;
      continue;
    }
    cursor = closingRun + delimiter.length;
  }
  return visible;
}

const worktree = isGitWorktree();
const snapshot = worktree === true ? exactGitIndexTree() : null;
validateDocumentationIndex(worktree, snapshot);

const documents = [];
if (snapshot) {
  for (const [path, entry] of snapshot.tracked) {
    if (!isFirstPartyDocumentation(path)) continue;
    if (!isRegularBlob(entry)) {
      failures.push(`${path}: tracked documentation must be a regular blob in the exact Git index tree`);
      continue;
    }
    const contents = readTrackedBlob(entry);
    if (contents === null) {
      failures.push(`${path}: cannot read tracked documentation blob ${entry.oid}`);
      continue;
    }
    documents.push({ path, contents });
  }
} else if (worktree === false) {
  walk(root, documents);
}

function snapshotContains(path) {
  if (!snapshot) return false;
  if (!path) return true;
  const exact = snapshot.tracked.get(path);
  if (exact) return isRegularBlob(exact);
  const prefix = `${path.replace(/\/+$/, "")}/`;
  return [...snapshot.tracked].some(
    ([candidate, entry]) => candidate.startsWith(prefix) && isRegularBlob(entry),
  );
}

for (const document of documents) {
  const lines = document.contents.split(/\r?\n/);
  let fenced = false;
  for (let lineNo = 0; lineNo < lines.length; lineNo++) {
    const line = lines[lineNo];
    if (/^\s*(```|~~~)/.test(line)) { fenced = !fenced; continue; }
    if (fenced) continue;
    const visibleLine = withoutInlineCode(line);
    // Markdown inline links/images; reference-style definitions are handled too.
    const links = [...visibleLine.matchAll(/!?\[[^\]]*\]\(\s*([^\s)]+|<[^>]+>)(?:\s+[^)]*)?\s*\)/g)];
    const reference = visibleLine.match(/^\s*\[[^\]]+\]:\s*(\S+)/);
    if (reference) links.push({ 1: reference[1] });
    for (const match of links) {
      const target = localTarget(match[1]);
      if (!target) continue;
      const candidate = resolve(root, document.path, "..", target);
      if (isAbsolute(candidate) && !candidate.startsWith(root + "/") && candidate !== root) {
        failures.push(`${document.path}:${lineNo + 1}: link escapes repository: ${target}`);
      } else {
        const repositoryPath = relative(root, candidate).split("\\").join("/");
        const exists = snapshot ? snapshotContains(repositoryPath) : existsSync(candidate);
        if (!exists) failures.push(`${document.path}:${lineNo + 1}: missing target: ${target}`);
      }
    }
  }
}
if (failures.length) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
} else {
  console.log(`doc links OK (${documents.length} markdown files)`);
}
