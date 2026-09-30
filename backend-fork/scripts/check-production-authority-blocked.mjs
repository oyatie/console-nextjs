#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { realpathSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const PATHS = Object.freeze([
  "deploy/argocd/apps/console.yaml",
  "deploy/argocd/project.yaml",
  "deploy/argocd/root.yaml",
  "docs/release/PR-473-PRODUCTION-CARDINALITY.evidence.json",
  "docs/release/PR-473-PRODUCTION-PROMOTION.authorization.json",
]);
const ZERO_SHA = "0".repeat(40),
  TEMPLATE = "TEMPLATE_NOT_EVIDENCE";
const CODES = Object.freeze([
  "APP_PROJECT_WILDCARD_AUTHORITY",
  "CARDINALITY_TEMPLATE_NOT_EVIDENCE",
  "MAINTENANCE_MUTABLE_MAIN",
  "PR473_CUTOVER_AND_DEPLOYMENT_FALSE",
  "ROOT_MUTABLE_MAIN",
]);
const CLAIM_LIMITS = Object.freeze([
  "ARCHITECTURE_ADOPTION_NOT_ESTABLISHED",
  "CNREL_CONFORMANCE_NOT_ESTABLISHED",
  "I3_INDEPENDENCE_NOT_ESTABLISHED",
  "LEGAL_CLEARANCE_NOT_ESTABLISHED",
  "PRODUCTION_ACTIVATION_NOT_AUTHORIZED",
  "PRODUCTION_AUTHORITY_NOT_ESTABLISHED",
  "PRODUCTION_READINESS_NOT_ESTABLISHED",
  "SAME_REPOSITORY_CI_NOT_BYPASS_RESISTANT",
]);
const APP_ROUTE = {
  apiVersion: "scalar",
  kind: "scalar",
  metadata: { name: "scalar" },
  spec: { source: { targetRevision: "scalar" } },
};
const PROJECT_ROUTE = {
  apiVersion: "scalar",
  kind: "scalar",
  metadata: { name: "scalar" },
  spec: {
    destinations: ["server", "namespace"],
    clusterResourceWhitelist: ["group", "kind"],
    namespaceResourceWhitelist: ["group", "kind"],
  },
};
class Failure extends Error {
  constructor(code) {
    super(code);
    this.code = code;
  }
}
const fail = (code) => {
  throw new Failure(code);
};
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const git = (args) =>
  spawnSync("git", args, {
    cwd: ROOT,
    encoding: "buffer",
    stdio: ["ignore", "pipe", "ignore"],
  });
const bytesOf = (value) =>
  Buffer.isBuffer(value) ? value : Buffer.from(value ?? "");
const textOf = (bytes, stage) => {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    fail(stage);
  }
};

function jsonSyntax(text) {
  try {
    return JSON.parse(text);
  } catch {
    fail("INPUT_JSON_MALFORMED");
  }
}
function rejectJsonDuplicates(text) {
  let index = 0;
  let rootState = "value";
  const stack = [];
  const whitespace = () => {
    while (/\s/.test(text[index] ?? "")) index += 1;
  };
  const current = () => stack.at(-1);
  const completeValue = () => {
    if (!stack.length) {
      rootState = "done";
      return;
    }
    current().state = "comma-or-end";
  };
  const readString = () => {
    const start = index++;
    let escaped = false;
    while (index < text.length) {
      const char = text[index++];
      if (!escaped && char === '"') return JSON.parse(text.slice(start, index));
      escaped = !escaped && char === "\\";
    }
    fail("INPUT_JSON_MALFORMED");
  };
  while (true) {
    whitespace();
    if (index === text.length) break;
    const frame = current();
    const state = frame?.state ?? rootState;
    const char = text[index];
    if (state === "key-or-end") {
      if (char === "}") {
        index += 1;
        stack.pop();
        completeValue();
        continue;
      }
      if (char !== '"') fail("INPUT_JSON_MALFORMED");
      const key = readString();
      if (frame.keys.has(key)) fail("INPUT_DUPLICATE_KEY");
      frame.keys.add(key);
      frame.state = "colon";
      continue;
    }
    if (state === "colon") {
      if (char !== ":") fail("INPUT_JSON_MALFORMED");
      index += 1;
      frame.state = "value";
      continue;
    }
    if (state === "comma-or-end") {
      const end = frame.type === "object" ? "}" : "]";
      if (char === end) {
        index += 1;
        stack.pop();
        completeValue();
        continue;
      }
      if (char !== ",") fail("INPUT_JSON_MALFORMED");
      index += 1;
      frame.state = frame.type === "object" ? "key-or-end" : "value-or-end";
      continue;
    }
    if (state === "value-or-end" && char === "]") {
      index += 1;
      stack.pop();
      completeValue();
      continue;
    }
    if (state !== "value" && state !== "value-or-end")
      fail("INPUT_JSON_MALFORMED");
    if (char === "{") {
      index += 1;
      stack.push({ type: "object", state: "key-or-end", keys: new Set() });
      continue;
    }
    if (char === "[") {
      index += 1;
      stack.push({ type: "array", state: "value-or-end" });
      continue;
    }
    if (char === '"') {
      readString();
      completeValue();
      continue;
    }
    const primitive = text
      .slice(index)
      .match(
        /^(?:true|false|null|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?)/,
      );
    if (!primitive) fail("INPUT_JSON_MALFORMED");
    index += primitive[0].length;
    completeValue();
  }
  if (stack.length || rootState !== "done") fail("INPUT_JSON_MALFORMED");
}
const AUTH_SCHEMA = {
  schema_version: "number",
  pull_request: "number",
  target: "string",
  release_phase: "string",
  rollback_floor: "string",
  desired_state_authority_cutover: "boolean",
  deployment_authorized: "boolean",
  command_only: "boolean",
  production_cardinality_evidence: {
    path: "string",
    sha256: "string",
    verified: "boolean",
  },
  contract_authorities: {
    old_runtime_drain: "boolean",
    rollback_floor_raise: "boolean",
  },
};
const CARD_SCHEMA = {
  schema_version: "number",
  target: "string",
  release_phase: "string",
  candidate_source_sha: "string",
  observed_running_revision: "string",
  observed_database_topology: {
    cluster_name: "string",
    namespace: "string",
    writer_endpoint: "string",
    reader_endpoint: "string",
    instances: "array",
  },
  capacity_headroom: {
    window_started_at: "string",
    window_ended_at: "string",
    cpu_peak_percent: "number",
    memory_peak_percent: "number",
    storage_used_percent: "number",
    connection_peak: "number",
    connection_limit: "number",
    minimum_headroom_percent: "number",
  },
  backup_restore_proof: {
    backup_id: "string",
    backup_completed_at: "string",
    isolated_restore_id: "string",
    isolated_restore_completed_at: "string",
    restored_revision: "string",
    validation_checks: "array",
  },
  evidence_author: {
    github_login: "string",
    identity_provider_subject: "string",
  },
  independent_reviewer: {
    github_login: "string",
    identity_provider_subject: "string",
    team_id: "number",
  },
  charter: { charter_id: "string", trust_domain_id: "string" },
  observed_at: "string",
  prepared_at: "string",
  reviewed_at: "string",
};
function unknown(value, schema) {
  if (!value || Array.isArray(value) || typeof value !== "object") return false;
  return (
    Object.keys(value).some((key) => !Object.hasOwn(schema, key)) ||
    Object.keys(schema).some(
      (key) =>
        typeof schema[key] === "object" && unknown(value[key], schema[key]),
    )
  );
}
function schema(value, expected) {
  if (!value || Array.isArray(value) || typeof value !== "object") return false;
  const keys = Object.keys(expected);
  if (
    Object.keys(value).length !== keys.length ||
    // Own-property presence only — `key in value` answers true for Object.prototype names.
    keys.some((key) => !Object.hasOwn(value, key))
  )
    return false;
  return keys.every((key) =>
    typeof expected[key] === "object"
      ? schema(value[key], expected[key])
      : expected[key] === "array"
        ? Array.isArray(value[key])
        : typeof value[key] === expected[key],
  );
}
function jsonDocuments(cardBytes, authBytes) {
  const cardText = textOf(cardBytes, "INPUT_JSON_MALFORMED"),
    authText = textOf(authBytes, "INPUT_JSON_MALFORMED");
  const card = jsonSyntax(cardText),
    auth = jsonSyntax(authText);
  rejectJsonDuplicates(cardText);
  rejectJsonDuplicates(authText);
  if (unknown(card, CARD_SCHEMA) || unknown(auth, AUTH_SCHEMA))
    fail("INPUT_UNKNOWN_KEY");
  if (!schema(card, CARD_SCHEMA) || !schema(auth, AUTH_SCHEMA))
    fail("INPUT_SCHEMA_MISMATCH");
  return { card, auth };
}

function decomment(line) {
  let quote = "",
    escaped = false;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (quote === '"' && escaped) {
      escaped = false;
      continue;
    }
    if (quote === '"' && c === "\\") {
      escaped = true;
      continue;
    }
    if (quote) {
      if (c === quote) quote = "";
      continue;
    }
    if (c === '"' || c === "'") quote = c;
    else if (c === "#" && (i === 0 || /\s/.test(line[i - 1])))
      return line.slice(0, i);
  }
  return line;
}
function splitPair(body) {
  let quote = "",
    escaped = false;
  for (let i = 0; i < body.length; i++) {
    const c = body[i];
    if (quote === '"' && escaped) {
      escaped = false;
      continue;
    }
    if (quote === '"' && c === "\\") {
      escaped = true;
      continue;
    }
    if (quote) {
      if (c === quote) quote = "";
      continue;
    }
    if (c === '"' || c === "'") quote = c;
    else if (c === ":") return [body.slice(0, i), body.slice(i + 1)];
  }
  return null;
}
function decode(raw) {
  const value = raw.trim();
  if (!value) return undefined;
  if (value.startsWith('"')) {
    try {
      if (!/^"(?:[^"\\]|\\(?:["\\/bfnrt]|u[0-9a-fA-F]{4}))*"$/.test(value))
        fail("INPUT_YAML_AMBIGUOUS");
      return JSON.parse(value);
    } catch {
      fail("INPUT_YAML_AMBIGUOUS");
    }
  }
  if (value.startsWith("'")) {
    if (!/^'(?:[^']|'')*'$/.test(value)) fail("INPUT_YAML_AMBIGUOUS");
    return value.slice(1, -1).replace(/''/g, "'");
  }
  if (/[\[\]{}]|^[!&*]|^[>|]/.test(value)) fail("INPUT_YAML_AMBIGUOUS");
  return value;
}
function decodeKey(raw) {
  const key = decode(raw);
  if (
    key === undefined ||
    typeof key !== "string" ||
    /[\[\]{}!&*]/.test(raw.trim())
  )
    fail("INPUT_YAML_AMBIGUOUS");
  return key;
}
function yamlLines(text) {
  const lines = text
    .split(/\r?\n/)
    .map((original) => {
      if (/\t/.test(original)) fail("INPUT_YAML_AMBIGUOUS");
      const line = decomment(original);
      return {
        indent: (line.match(/^ */) ?? [""])[0].length,
        body: line.trim(),
        raw: line,
      };
    })
    .filter((line) => line.body);
  let seen = false,
    terminal = false;
  const out = [];
  for (const line of lines) {
    if (/^(---|\.\.\.)$/.test(line.body)) {
      if (line.indent !== 0) fail("INPUT_YAML_AMBIGUOUS");
      if (line.body === "---" && !seen && !out.length) {
        seen = true;
        continue;
      }
      if (line.body === "..." && !terminal) {
        terminal = true;
        continue;
      }
      fail("INPUT_YAML_AMBIGUOUS");
    }
    if (terminal) fail("INPUT_YAML_AMBIGUOUS");
    seen = true;
    out.push(line);
  }
  return out;
}
function unsafe(raw) {
  return /[\[\]{}]|^[!&*]|^[>|]/.test(raw.trim());
}
// This is intentionally a route trie scanner, not a YAML parser. Unknown direct
// members are skipped as opaque indentation-bounded subtrees; only route members
// receive syntax, duplicate, and relative-indentation interpretation.
function scanYaml(text, route) {
  const lines = yamlLines(text);
  let at = 0;
  const nextIndent = (start) => {
    for (let i = start; i < lines.length; i++)
      if (lines[i].body) return lines[i].indent;
    return null;
  };
  const map = (indent, trie, limit = lines.length) => {
    const node = Object.create(null),
      seen = new Set();
    while (at < limit) {
      const line = lines[at];
      if (line.indent < indent) break;
      if (line.indent > indent) {
        fail("INPUT_YAML_AMBIGUOUS");
      }
      if (line.body.startsWith("- ")) fail("INPUT_YAML_AMBIGUOUS");
      const pair = splitPair(line.body);
      if (!pair) fail("INPUT_YAML_AMBIGUOUS");
      const key = decodeKey(pair[0]),
        raw = pair[1];
      at++;
      if (key === "<<") fail("INPUT_YAML_AMBIGUOUS");
      const target = trie[key];
      if (!target) {
        while (at < limit && lines[at].indent > indent) at++;
        continue;
      }
      if (seen.has(key)) fail("INPUT_YAML_AMBIGUOUS");
      seen.add(key);
      if (target === "scalar") {
        if (unsafe(raw)) fail("INPUT_YAML_AMBIGUOUS");
        node[key] = decode(raw);
        if (at < limit && lines[at].indent > indent)
          fail("INPUT_YAML_AMBIGUOUS");
        continue;
      }
      if (Array.isArray(target)) {
        if (unsafe(raw)) fail("INPUT_YAML_AMBIGUOUS");
        if (raw.trim()) {
          node[key] = null;
          continue;
        }
        const childIndent = nextIndent(at);
        if (childIndent === null || childIndent <= indent) {
          node[key] = null;
          continue;
        }
        node[key] = sequence(childIndent, target, limit);
        continue;
      }
      if (unsafe(raw)) fail("INPUT_YAML_AMBIGUOUS");
      if (raw.trim()) {
        node[key] = null;
        continue;
      }
      const childIndent = nextIndent(at);
      if (childIndent === null || childIndent <= indent) {
        node[key] = null;
        continue;
      }
      node[key] = map(childIndent, target, limit);
    }
    return node;
  };
  const sequence = (indent, members, limit) => {
    const items = [];
    while (at < limit && lines[at].indent >= indent) {
      const line = lines[at];
      if (line.indent !== indent || !line.body.startsWith("- "))
        fail("INPUT_YAML_AMBIGUOUS");
      const pair = splitPair(line.body.slice(2));
      if (!pair) fail("INPUT_YAML_AMBIGUOUS");
      const item = Object.create(null),
        seen = new Set();
      const add = (key, raw) => {
        if (key === "<<" || seen.has(key) || unsafe(raw))
          fail("INPUT_YAML_AMBIGUOUS");
        seen.add(key);
        if (!members.includes(key)) item.__extra = true;
        const value = decode(raw);
        if (value === undefined) fail("INPUT_YAML_AMBIGUOUS");
        item[key] = value;
      };
      const key = decodeKey(pair[0]),
        raw = pair[1];
      at++;
      add(key, raw);
      const memberIndent =
        at < limit && lines[at].indent > indent ? lines[at].indent : null;
      // A compact sequence item (`- key: value`) starts its first member at
      // `indent + 2`; all continuation members must use that same column.
      if (memberIndent !== null && memberIndent !== indent + 2)
        fail("INPUT_YAML_AMBIGUOUS");
      while (
        memberIndent !== null &&
        at < limit &&
        lines[at].indent === memberIndent &&
        !lines[at].body.startsWith("- ")
      ) {
        const more = splitPair(lines[at].body);
        if (!more) fail("INPUT_YAML_AMBIGUOUS");
        const moreKey = decodeKey(more[0]),
          moreRaw = more[1];
        at++;
        add(moreKey, moreRaw);
      }
      if (at < limit && lines[at].indent > indent) fail("INPUT_YAML_AMBIGUOUS");
      items.push(item);
    }
    return items;
  };
  if (lines.length && lines[0].indent !== 0) fail("INPUT_YAML_AMBIGUOUS");
  const result = map(0, route);
  if (at !== lines.length) fail("INPUT_YAML_AMBIGUOUS");
  return result;
}
function appObserved(text, name) {
  const root = scanYaml(text, APP_ROUTE);
  return (
    root.apiVersion === "argoproj.io/v1alpha1" &&
    root.kind === "Application" &&
    root.metadata?.name === name &&
    root.spec?.source?.targetRevision === "main"
  );
}
function exactItem(items, keys, values) {
  return (
    Array.isArray(items) &&
    items.length === 1 &&
    !items[0].__extra &&
    Object.keys(items[0]).length === keys.length &&
    keys.every((key) => items[0][key] === values[key])
  );
}
function projectObserved(text) {
  const root = scanYaml(text, PROJECT_ROUTE),
    spec = root.spec;
  return (
    root.apiVersion === "argoproj.io/v1alpha1" &&
    root.kind === "AppProject" &&
    root.metadata?.name === "console" &&
    exactItem(spec?.destinations, ["server", "namespace"], {
      server: "https://kubernetes.default.svc",
      namespace: "*",
    }) &&
    exactItem(spec?.clusterResourceWhitelist, ["group", "kind"], {
      group: "*",
      kind: "*",
    }) &&
    exactItem(spec?.namespaceResourceWhitelist, ["group", "kind"], {
      group: "*",
      kind: "*",
    })
  );
}
function observations(card, auth, cardDigest) {
  const authOk =
    auth.schema_version === 2 &&
    auth.pull_request === 473 &&
    auth.target === "production" &&
    auth.release_phase === "expand" &&
    auth.rollback_floor === "f6ff236b9770c79301a3d07da6afb56be1e27bbf" &&
    auth.desired_state_authority_cutover === false &&
    auth.deployment_authorized === false &&
    auth.command_only === false &&
    auth.production_cardinality_evidence.path === PATHS[3] &&
    auth.production_cardinality_evidence.sha256 === cardDigest &&
    auth.production_cardinality_evidence.verified === false &&
    auth.contract_authorities.old_runtime_drain === false &&
    auth.contract_authorities.rollback_floor_raise === false;
  const templates = [
    card.observed_database_topology.cluster_name,
    card.observed_database_topology.namespace,
    card.observed_database_topology.writer_endpoint,
    card.observed_database_topology.reader_endpoint,
    card.capacity_headroom.window_started_at,
    card.capacity_headroom.window_ended_at,
    card.backup_restore_proof.backup_id,
    card.backup_restore_proof.backup_completed_at,
    card.backup_restore_proof.isolated_restore_id,
    card.backup_restore_proof.isolated_restore_completed_at,
    card.evidence_author.github_login,
    card.evidence_author.identity_provider_subject,
    card.independent_reviewer.github_login,
    card.independent_reviewer.identity_provider_subject,
    card.charter.charter_id,
    card.charter.trust_domain_id,
    card.observed_at,
    card.prepared_at,
    card.reviewed_at,
  ];
  const numbers = [
    card.capacity_headroom.cpu_peak_percent,
    card.capacity_headroom.memory_peak_percent,
    card.capacity_headroom.storage_used_percent,
    card.capacity_headroom.connection_peak,
    card.capacity_headroom.connection_limit,
    card.capacity_headroom.minimum_headroom_percent,
    card.independent_reviewer.team_id,
  ];
  return (
    authOk &&
    card.schema_version === 1 &&
    card.target === "production" &&
    card.release_phase === "expand" &&
    [
      card.candidate_source_sha,
      card.observed_running_revision,
      card.backup_restore_proof.restored_revision,
    ].every((value) => value === ZERO_SHA) &&
    card.observed_database_topology.instances.length === 0 &&
    card.backup_restore_proof.validation_checks.length === 0 &&
    templates.every((value) => value === TEMPLATE) &&
    numbers.every((value) => value === 0)
  );
}
export function evaluate(argv, runGit = git) {
  if (argv.length !== 1) fail("ARGUMENT_COUNT");
  const commit = argv[0];
  if (!/^[0-9a-f]{40}$/.test(commit)) fail("COMMIT_SHA_FORMAT");
  if (commit === ZERO_SHA) fail("COMMIT_SHA_ZERO");
  const object = runGit(["cat-file", "-t", commit]);
  if (object.status !== 0) fail("GIT_OBJECT_UNAVAILABLE");
  if (bytesOf(object.stdout).toString().trim() !== "commit")
    fail("GIT_OBJECT_NOT_COMMIT");
  const blobs = PATHS.map((path) => runGit(["show", `${commit}:${path}`]));
  if (blobs.some((blob) => blob.status !== 0)) fail("INPUT_BLOB_UNAVAILABLE");
  const bytes = blobs.map((blob) => bytesOf(blob.stdout));
  const { card, auth } = jsonDocuments(bytes[3], bytes[4]);
  const yamlTexts = [
    textOf(bytes[0], "INPUT_YAML_AMBIGUOUS"),
    textOf(bytes[1], "INPUT_YAML_AMBIGUOUS"),
    textOf(bytes[2], "INPUT_YAML_AMBIGUOUS"),
  ];
  const yaml = [
    appObserved(yamlTexts[0], "console"),
    projectObserved(yamlTexts[1]),
    appObserved(yamlTexts[2], "root"),
  ];
  if (!yaml.every(Boolean) || !observations(card, auth, hash(bytes[3])))
    fail("OBSERVATION_NOT_ESTABLISHED");
  return {
    schema_version: 1,
    artifact_identity: "repository_blocked_observation",
    state: "BLOCKED",
    activation_capable: false,
    independence: "I1_NON_INDEPENDENT",
    evaluated_commit_sha: commit,
    input_digests: PATHS.map((path, index) => ({
      path,
      sha256: hash(bytes[index]),
    })).sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0)),
    codes: [...CODES],
    claim_limits: [...CLAIM_LIMITS],
  };
}
function main() {
  try {
    process.stdout.write(
      `${JSON.stringify(evaluate(process.argv.slice(2)))}\n`,
    );
  } catch (error) {
    process.stderr.write(
      `ERROR ${error instanceof Failure ? error.code : "INTERNAL_ERROR"}\n`,
    );
    process.exitCode = 1;
  }
}
if (
  process.argv[1] &&
  realpathSync(resolve(process.argv[1])) ===
    realpathSync(fileURLToPath(import.meta.url))
)
  main();
