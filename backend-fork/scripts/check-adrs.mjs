#!/usr/bin/env node

import { existsSync, readFileSync, readdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ADR_ID_PATTERN = /^ADR-\d{4}$/;
const NOTE_ID_PATTERN = /^DN-\d{4}$/;
const ISO_DATE_PATTERN = /^\d{4}-\d{2}-\d{2}$/;
const ADR_STATUSES = new Set(["proposed", "accepted", "superseded", "rejected", "withdrawn"]);
const DOC_STATUSES = new Set(["review", "published", "archived"]);
const ADR_REQUIRED_FIELDS = ["id", "status", "doc_status", "date", "owner", "related"];
const NOTE_REQUIRED_FIELDS = ["id", "kind", "parent_adr", "authority", "activation", "date", "owner"];
const RELATIONSHIP_KEYS = [
  "amends",
  "amended_by",
  "supersedes",
  "superseded_by",
  "related",
  "proposes_amendments_to",
];
const RECIPROCAL_RELATIONSHIPS = [
  ["amends", "amended_by"],
  ["amended_by", "amends"],
  ["supersedes", "superseded_by"],
  ["superseded_by", "supersedes"],
];
const REPOSITORY_SCAN_EXCLUSIONS = new Set([
  ".git",
  ".next",
  ".omx",
  "build",
  "coverage",
  "dist",
  "node_modules",
  "target",
  "vendor",
]);
const STALE_ADR_0022_PATHS = [
  "ADR-0022-bare-metal-portability-and-ha",
  "ADR-0022-ha-workload-scheduling-expectations",
  "ADR-0022-on-prem-vip-ingress-approach",
];
const PORTABILITY_TERMS =
  /bare[- ]metal|on[- ]prem|portab|high availability|\bHA\b|Talos|Cilium|OpenBao|self[- ]host|multicluster|object[- ]store|observability|roadmap lane|production-hardening/i;

function parseScalar(raw) {
  const value = raw.trim();
  if (
    (value.startsWith('"') && value.endsWith('"')) ||
    (value.startsWith("'") && value.endsWith("'"))
  ) {
    return value.slice(1, -1);
  }
  return value;
}

function parseFrontmatterValue(raw) {
  const value = raw.trim();
  if (value.startsWith("[") && value.endsWith("]")) {
    const contents = value.slice(1, -1).trim();
    return contents === ""
      ? []
      : contents.split(",").map((entry) => parseScalar(entry));
  }
  return parseScalar(value);
}

function parseFrontmatter(text, path, failures) {
  const lines = text.split(/\r?\n/);
  if (lines[0]?.trim() !== "---") {
    failures.push(`${path}: missing YAML frontmatter`);
    return { data: {}, body: text };
  }

  const closing = lines.findIndex((line, index) => index > 0 && line.trim() === "---");
  if (closing === -1) {
    failures.push(`${path}: YAML frontmatter is not closed`);
    return { data: {}, body: "" };
  }

  const data = {};
  for (const [offset, line] of lines.slice(1, closing).entries()) {
    if (line.trim() === "" || line.trimStart().startsWith("#")) {
      continue;
    }
    const match = line.match(/^([a-z][a-z0-9_]*)\s*:\s*(.*)$/i);
    if (!match) {
      failures.push(`${path}:${offset + 2}: unsupported frontmatter syntax`);
      continue;
    }
    const [, key, rawValue] = match;
    if (Object.hasOwn(data, key)) {
      failures.push(`${path}:${offset + 2}: duplicate frontmatter field ${key}`);
      continue;
    }
    data[key] = parseFrontmatterValue(rawValue);
  }

  return { data, body: lines.slice(closing + 1).join("\n") };
}

function markdownFiles(directory, failures, label) {
  if (!existsSync(directory)) {
    failures.push(`${label}: missing directory`);
    return [];
  }
  return readdirSync(directory, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".md"))
    .map((entry) => join(directory, entry.name))
    .sort((left, right) => left.localeCompare(right));
}

function repositoryFiles(root) {
  if (existsSync(join(root, ".git"))) {
    try {
      return execFileSync("git", ["-C", root, "ls-files", "-z"], { encoding: "utf8" })
        .split("\0")
        .filter(Boolean)
        .map((path) => join(root, path));
    } catch {
      // Fall through to the dependency-free traversal used by fixtures and
      // environments where Git is unavailable.
    }
  }

  const files = [];
  const visit = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      if (entry.isDirectory()) {
        if (!REPOSITORY_SCAN_EXCLUSIONS.has(entry.name)) {
          visit(join(directory, entry.name));
        }
      } else if (entry.isFile()) {
        files.push(join(directory, entry.name));
      }
    }
  };
  visit(root);
  return files;
}

function validateRetiredAdrIdentities(root, decisionsDirectory, failures) {
  const checkerPaths = new Set([
    join(root, "scripts", "check-adrs.mjs"),
    join(root, "scripts", "check-adrs.test.mjs"),
  ]);
  for (const absolutePath of repositoryFiles(root)) {
    if (!existsSync(absolutePath) || checkerPaths.has(absolutePath)) {
      continue;
    }
    const display = displayPath(root, absolutePath);
    const contents = readFileSync(absolutePath);
    if (contents.includes(0)) {
      continue;
    }
    const text = contents.toString("utf8");
    for (const [index, line] of text.split(/\r?\n/).entries()) {
      const hasRetiredPath = STALE_ADR_0022_PATHS.some((retired) => line.includes(retired));
      const misidentifiesPortability =
        !isWithinDirectory(decisionsDirectory, absolutePath) &&
        line.includes("ADR-0022") &&
        PORTABILITY_TERMS.test(line);
      if (hasRetiredPath || misidentifiesPortability) {
        failures.push(
          `${display}:${index + 1}: ADR-0022 is the local-identity decision, not the portability/HA decision; use ADR-0024 or its DN-0001/DN-0002 notes`,
        );
      }
    }
  }
}

function isWithinDirectory(directory, candidate) {
  const pathFromDirectory = relative(directory, candidate);
  return (
    pathFromDirectory !== ".." &&
    !pathFromDirectory.startsWith(`..${sep}`) &&
    !isAbsolute(pathFromDirectory)
  );
}

function displayPath(root, path) {
  const pathFromRoot = relative(root, path);
  return isWithinDirectory(root, path) ? pathFromRoot || "." : path;
}

function isRealIsoCalendarDate(value) {
  if (typeof value !== "string" || !ISO_DATE_PATTERN.test(value)) {
    return false;
  }
  const [year, month, day] = value.split("-").map(Number);
  const date = new Date(Date.UTC(year, month - 1, day));
  return (
    date.getUTCFullYear() === year &&
    date.getUTCMonth() === month - 1 &&
    date.getUTCDate() === day
  );
}

function relationshipValues(record, key) {
  if (!Object.hasOwn(record.frontmatter, key)) {
    return [];
  }
  const value = record.frontmatter[key];
  return Array.isArray(value) ? value : [value];
}

function requireFields(record, fields, failures) {
  for (const field of fields) {
    if (
      !Object.hasOwn(record.frontmatter, field) ||
      record.frontmatter[field] === "" ||
      record.frontmatter[field] === undefined
    ) {
      failures.push(`${record.path}: missing required frontmatter field ${field}`);
    }
  }
}

function validateAdrRecordShape(record, failures) {
  requireFields(record, ADR_REQUIRED_FIELDS, failures);
  const { frontmatter, path } = record;

  if (frontmatter.id !== undefined && !ADR_ID_PATTERN.test(frontmatter.id)) {
    failures.push(`${path}: id ${JSON.stringify(frontmatter.id)} must match ADR-NNNN`);
  }
  const filenameId = record.filename.match(/^(ADR-\d{4})-/)?.[1];
  if (!filenameId) {
    failures.push(`${path}: filename must start with ADR-NNNN-`);
  } else if (ADR_ID_PATTERN.test(frontmatter.id ?? "") && filenameId !== frontmatter.id) {
    failures.push(`${path}: filename id ${filenameId} does not match frontmatter id ${frontmatter.id}`);
  }
  if (frontmatter.status !== undefined && !ADR_STATUSES.has(frontmatter.status)) {
    failures.push(`${path}: invalid status ${JSON.stringify(frontmatter.status)}`);
  }
  if (frontmatter.doc_status !== undefined && !DOC_STATUSES.has(frontmatter.doc_status)) {
    failures.push(`${path}: invalid doc_status ${JSON.stringify(frontmatter.doc_status)}`);
  }
  if (frontmatter.date !== undefined && !isRealIsoCalendarDate(frontmatter.date)) {
    failures.push(
      `${path}: date ${JSON.stringify(frontmatter.date)} must be a real calendar date in YYYY-MM-DD`,
    );
  }
  if (Array.isArray(frontmatter.owner)) {
    failures.push(`${path}: owner must be a non-empty scalar`);
  }
  if (Object.hasOwn(frontmatter, "related") && !Array.isArray(frontmatter.related)) {
    failures.push(`${path}: related must use an inline list, including [] when empty`);
  }
  for (const key of RELATIONSHIP_KEYS) {
    for (const reference of relationshipValues(record, key)) {
      if (typeof reference !== "string" || !ADR_ID_PATTERN.test(reference)) {
        failures.push(`${path}: ${key} reference ${String(reference)} must be a local ADR id`);
      }
    }
  }

  if (frontmatter.status === "proposed") {
    for (const key of ["amends", "supersedes"]) {
      if (relationshipValues(record, key).length > 0) {
        failures.push(`${path}: proposed ADR cannot declare ${key}`);
      }
    }
    const subjectIdPattern = ADR_ID_PATTERN.test(frontmatter.id ?? "")
      ? new RegExp(
          "[*_~`]*\\b" +
            frontmatter.id +
            "\\b[*_~`]*\\s+(?:amends|supersedes)\\s+(?:all\\s+of\\s+)?ADR-\\d{4}\\b",
          "i",
        )
      : null;
    const claimsActiveRelationship =
      /\bthis\s+ADR\s+(?:amends|supersedes)\s+(?:all\s+of\s+)?ADR-\d{4}\b/i.test(record.body) ||
      /(?:^|[.!?·]\s+)(?:Amends|Supersedes)\s+(?:all\s+of\s+)?ADR-\d{4}\b/m.test(record.body) ||
      subjectIdPattern?.test(record.body);
    if (claimsActiveRelationship) {
      failures.push(`${path}: proposed ADR prose claims active amendment or supersession`);
    }
  }
  if (frontmatter.status === "superseded" && relationshipValues(record, "superseded_by").length === 0) {
    failures.push(`${path}: superseded ADR must declare superseded_by`);
  }
  if (
    frontmatter.status !== "superseded" &&
    relationshipValues(record, "superseded_by").length > 0
  ) {
    failures.push(`${path}: ADR with superseded_by must have status superseded`);
  }
}

function validateDesignNoteShape(record, failures) {
  requireFields(record, NOTE_REQUIRED_FIELDS, failures);
  const { frontmatter, path } = record;
  if (frontmatter.id !== undefined && !NOTE_ID_PATTERN.test(frontmatter.id)) {
    failures.push(`${path}: id ${JSON.stringify(frontmatter.id)} must match DN-NNNN`);
  }
  const filenameId = record.filename.match(/^(DN-\d{4})-/)?.[1];
  if (!filenameId) {
    failures.push(`${path}: filename must start with DN-NNNN-`);
  } else if (NOTE_ID_PATTERN.test(frontmatter.id ?? "") && filenameId !== frontmatter.id) {
    failures.push(`${path}: filename id ${filenameId} does not match frontmatter id ${frontmatter.id}`);
  }
  if (frontmatter.kind !== undefined && frontmatter.kind !== "design-note") {
    failures.push(`${path}: kind must be design-note`);
  }
  if (frontmatter.authority !== undefined && frontmatter.authority !== "subordinate") {
    failures.push(`${path}: authority must be subordinate`);
  }
  if (frontmatter.parent_adr !== undefined && !ADR_ID_PATTERN.test(frontmatter.parent_adr)) {
    failures.push(`${path}: parent_adr ${JSON.stringify(frontmatter.parent_adr)} must match ADR-NNNN`);
  }
  if (frontmatter.date !== undefined && !isRealIsoCalendarDate(frontmatter.date)) {
    failures.push(
      `${path}: date ${JSON.stringify(frontmatter.date)} must be a real calendar date in YYYY-MM-DD`,
    );
  }
}

function indexRows(indexText) {
  const rows = new Map();
  const duplicates = [];
  for (const line of indexText.split(/\r?\n/)) {
    if (!line.trimStart().startsWith("|")) {
      continue;
    }
    const cells = line
      .trim()
      .replace(/^\|/, "")
      .replace(/\|$/, "")
      .split("|")
      .map((cell) => cell.trim());
    const id = cells[0]?.match(/\b(ADR-\d{4})\b/)?.[1];
    if (!id) {
      continue;
    }
    const statusCell = cells[1]?.replace(/[*_`]/g, "").toLowerCase() ?? "";
    const target = cells[0]?.match(/\[[^\]]+\]\(([^)#]+)(?:#[^)]+)?\)/)?.[1];
    const status = statusCell.startsWith("never issued")
      ? "never issued"
      : [...ADR_STATUSES].find((candidate) => statusCell.startsWith(candidate));
    if (rows.has(id)) {
      duplicates.push(id);
    } else {
      rows.set(id, { status, rawStatus: cells[1] ?? "", target });
    }
  }
  return { rows, duplicates };
}

function buildRecords(root, paths, failures) {
  return paths.map((absolutePath) => {
    const path = displayPath(root, absolutePath);
    const text = readFileSync(absolutePath, "utf8");
    const { data, body } = parseFrontmatter(text, path, failures);
    return {
      absolutePath,
      path,
      filename: basename(absolutePath),
      frontmatter: data,
      body,
    };
  });
}

function recordsById(records, idPattern, failures, label) {
  const byId = new Map();
  for (const record of records) {
    const id = record.frontmatter.id;
    if (!idPattern.test(id ?? "")) {
      continue;
    }
    if (byId.has(id)) {
      failures.push(`${record.path}: duplicate ${label} id ${id}; first declared by ${byId.get(id).path}`);
      continue;
    }
    byId.set(id, record);
  }
  return byId;
}

function validateRelationshipGraph(adrs, failures) {
  for (const record of adrs.values()) {
    const sourceId = record.frontmatter.id;
    for (const key of RELATIONSHIP_KEYS) {
      for (const targetId of relationshipValues(record, key)) {
        if (!ADR_ID_PATTERN.test(targetId)) {
          continue;
        }
        const target = adrs.get(targetId);
        if (!target) {
          failures.push(`${record.path}: ${key} reference ${targetId} does not resolve`);
        } else if (targetId === sourceId) {
          failures.push(`${record.path}: ${key} must not reference itself`);
        }
      }
    }

    for (const [key, reciprocalKey] of RECIPROCAL_RELATIONSHIPS) {
      for (const targetId of relationshipValues(record, key)) {
        const target = adrs.get(targetId);
        if (!target || targetId === sourceId) {
          continue;
        }
        if (!relationshipValues(target, reciprocalKey).includes(sourceId)) {
          failures.push(`${target.path}: ${targetId} must declare ${reciprocalKey}: ${sourceId}`);
        }
      }
    }

    for (const key of ["amended_by", "superseded_by"]) {
      for (const targetId of relationshipValues(record, key)) {
        const target = adrs.get(targetId);
        if (target && target.frontmatter.status !== "accepted") {
          failures.push(
            `${record.path}: ${key} target ${targetId} must be accepted, found ${target.frontmatter.status}`,
          );
        }
      }
    }
    for (const key of ["amends", "supersedes"]) {
      if (relationshipValues(record, key).length > 0 && record.frontmatter.status !== "accepted") {
        failures.push(`${record.path}: only an accepted ADR may declare ${key}`);
      }
    }
    for (const targetId of relationshipValues(record, "amends")) {
      const target = adrs.get(targetId);
      if (target && target.frontmatter.status !== "accepted") {
        failures.push(
          `${record.path}: amends target ${targetId} must be accepted, found ${target.frontmatter.status}`,
        );
      }
    }
  }
}

function validateIndex(root, decisionsDirectory, adrs, failures) {
  const indexPath = join(decisionsDirectory, "README.md");
  const display = displayPath(root, indexPath);
  if (!existsSync(indexPath)) {
    failures.push(`${display}: missing ADR index`);
    return;
  }

  const parsed = indexRows(readFileSync(indexPath, "utf8"));
  for (const id of parsed.duplicates) {
    failures.push(`${display}: duplicate index row for ${id}`);
  }
  const reserved = parsed.rows.get("ADR-0013");
  if (!reserved || reserved.status !== "never issued") {
    failures.push(`${display}: ADR-0013 must be indexed as never issued`);
  }
  if (adrs.has("ADR-0013")) {
    failures.push(`${adrs.get("ADR-0013").path}: ADR-0013 is reserved and must never be issued`);
  }

  for (const [id, record] of adrs) {
    const row = parsed.rows.get(id);
    if (!row) {
      failures.push(`${display}: missing index row for ${id}`);
    } else if (row.status !== record.frontmatter.status) {
      failures.push(
        `${display}: ${id} index status ${row.status ?? JSON.stringify(row.rawStatus)} does not match frontmatter status ${record.frontmatter.status}`,
      );
    } else if (row.target !== record.filename) {
      failures.push(
        `${display}: ${id} index link ${row.target ?? "<missing>"} does not match ${record.filename}`,
      );
    }
  }
  for (const id of parsed.rows.keys()) {
    if (id !== "ADR-0013" && !adrs.has(id)) {
      failures.push(`${display}: ${id} is indexed but has no ADR file`);
    }
  }
}

function validateDesignNoteParents(notes, adrs, failures) {
  for (const note of notes.values()) {
    const parent = note.frontmatter.parent_adr;
    if (ADR_ID_PATTERN.test(parent ?? "") && !adrs.has(parent)) {
      failures.push(`${note.path}: parent_adr ${parent} does not resolve`);
    }
  }
}

export function evaluateAdrGovernance(rootDirectory) {
  const root = resolve(rootDirectory);
  const decisionsDirectory = join(root, "docs", "decisions");
  const failures = [];

  const decisionMarkdown = markdownFiles(decisionsDirectory, failures, "docs/decisions");
  const noteMarkdown = markdownFiles(
    join(decisionsDirectory, "notes"),
    failures,
    "docs/decisions/notes",
  );
  const adrPaths = decisionMarkdown.filter((path) => /^ADR-\d{4}-.+\.md$/.test(basename(path)));
  const notePaths = noteMarkdown.filter((path) => /^DN-\d{4}-.+\.md$/.test(basename(path)));
  for (const path of decisionMarkdown) {
    if (basename(path) !== "README.md" && !adrPaths.includes(path)) {
      failures.push(`${displayPath(root, path)}: unexpected Markdown file; use ADR-NNNN-slug.md`);
    }
  }
  for (const path of noteMarkdown) {
    if (!notePaths.includes(path)) {
      failures.push(`${displayPath(root, path)}: unexpected Markdown file; use DN-NNNN-slug.md`);
    }
  }

  const adrRecords = buildRecords(root, adrPaths, failures);
  const noteRecords = buildRecords(root, notePaths, failures);
  for (const record of adrRecords) {
    validateAdrRecordShape(record, failures);
  }
  for (const record of noteRecords) {
    validateDesignNoteShape(record, failures);
  }

  const adrs = recordsById(adrRecords, ADR_ID_PATTERN, failures, "ADR");
  const notes = recordsById(noteRecords, NOTE_ID_PATTERN, failures, "design note");
  validateRelationshipGraph(adrs, failures);
  validateIndex(root, decisionsDirectory, adrs, failures);
  validateDesignNoteParents(notes, adrs, failures);
  validateRetiredAdrIdentities(root, decisionsDirectory, failures);

  return {
    failures: [...new Set(failures)].sort((left, right) => left.localeCompare(right)),
    adrCount: adrRecords.length,
    noteCount: noteRecords.length,
  };
}

function rootArgument(argv) {
  const rootIndex = argv.indexOf("--root");
  if (rootIndex === -1) {
    return resolve(dirname(fileURLToPath(import.meta.url)), "..");
  }
  if (!argv[rootIndex + 1]) {
    throw new Error("--root requires a directory");
  }
  return resolve(argv[rootIndex + 1]);
}

function runCli() {
  let root;
  try {
    root = rootArgument(process.argv.slice(2));
  } catch (error) {
    console.error(`ADR governance gate failed:\n- ${error.message}`);
    process.exitCode = 1;
    return;
  }

  const result = evaluateAdrGovernance(root);
  if (result.failures.length > 0) {
    console.error(`ADR governance gate failed (${result.failures.length}):`);
    for (const failure of result.failures) {
      console.error(`- ${failure}`);
    }
    process.exitCode = 1;
    return;
  }
  console.log(`ADR governance gate passed: ${result.adrCount} ADRs, ${result.noteCount} design notes.`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  runCli();
}
