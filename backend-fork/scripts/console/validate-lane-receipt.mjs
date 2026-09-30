#!/usr/bin/env node
/**
 * Tracked lane-receipt validator. Schema file is the field-list SSOT.
 *
 * Usage:
 *   node scripts/console/validate-lane-receipt.mjs <receipt.json...> [--schema lane|critic]
 */
import { readFileSync, readdirSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const SCHEMA_URL = new URL('./lane-receipt.schema.json', import.meta.url);
const SCHEMA = JSON.parse(readFileSync(SCHEMA_URL, 'utf8'));
// Parity with the tracked incumbent (scripts/cursor/validate-lane-receipt.mjs) and
// scripts/console/workflows/lane-fanout.js BUILD_SCHEMA: case-insensitive prefix, not exact equality.
const NA_ENFORCEMENT = /^n\/a\s*-\s*adds no enforcement\b/i;

// Every JSON Schema keyword the walker implements. A schema edit that introduces a keyword
// outside this set must throw at load time instead of silently enforcing nothing.
const IMPLEMENTED_KEYWORDS = new Set([
  '$ref', 'oneOf', 'type', 'const', 'enum', 'minLength', 'pattern',
  'properties', 'required', 'minItems', 'items',
]);
const METADATA_KEYS = new Set(['$schema', '$id', 'title', '$defs', 'description']);

function assertSchemaKeywords(node, path) {
  if (!node || typeof node !== 'object' || Array.isArray(node)) return;
  for (const [key, child] of Object.entries(node)) {
    if (!IMPLEMENTED_KEYWORDS.has(key) && !METADATA_KEYS.has(key)) {
      throw new Error(`schema keyword "${key}" at ${path} is not implemented by this validator`);
    }
    if (key === '$defs' || key === 'properties') {
      for (const [name, sub] of Object.entries(child)) assertSchemaKeywords(sub, `${path}/${key}/${name}`);
    } else if (key === 'oneOf') {
      child.forEach((sub, i) => assertSchemaKeywords(sub, `${path}/oneOf/${i}`));
    } else if (key === 'items') {
      assertSchemaKeywords(child, `${path}/items`);
    }
  }
}
assertSchemaKeywords(SCHEMA, '#');

// Kind registry derived from the root oneOf so the schema file stays the single source
// of which receipt kinds exist; no JS-side duplicate list.
function buildShapes() {
  const shapes = new Map();
  for (const branch of SCHEMA.oneOf) {
    const shape = resolveRef(branch, SCHEMA);
    const kinds = shape?.properties?.kind?.enum;
    if (!Array.isArray(kinds) || kinds.length !== 1) {
      throw new Error('every root oneOf branch must declare properties.kind.enum with exactly one kind');
    }
    shapes.set(kinds[0], shape);
  }
  if (shapes.size === 0) throw new Error('schema root oneOf declares zero receipt kinds');
  return shapes;
}

function pointer(base, key) {
  if (base === '') return String(key);
  if (/^\d+$/.test(String(key))) return `${base}[${key}]`;
  return `${base}.${key}`;
}

function resolveRef(node, root) {
  if (!node || typeof node !== 'object' || Array.isArray(node) || !('$ref' in node)) return node;
  const ref = node.$ref;
  if (typeof ref !== 'string' || !ref.startsWith('#/')) {
    throw new Error(`unsupported $ref: ${ref}`);
  }
  let target = root;
  for (const part of ref.slice(2).split('/')) {
    if (target == null || typeof target !== 'object') {
      throw new Error(`unresolved $ref: ${ref}`);
    }
    target = target[part];
  }
  if (target == null || typeof target !== 'object') {
    throw new Error(`unresolved $ref: ${ref}`);
  }
  const { $ref: _ignored, ...rest } = node;
  return resolveRef({ ...target, ...rest }, root);
}

function jsType(value) {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  return typeof value;
}

function typeMatches(value, expected) {
  const actual = jsType(value);
  if (expected === 'integer') return actual === 'number' && Number.isInteger(value);
  return actual === expected;
}

function walk(value, node, field, defects, root) {
  const schema = resolveRef(node, root);
  if (!schema || typeof schema !== 'object' || Array.isArray(schema)) return;

  if (Array.isArray(schema.oneOf)) {
    const branchFailures = [];
    let matched = false;
    for (const branch of schema.oneOf) {
      const inner = [];
      walk(value, branch, field, inner, root);
      if (inner.length === 0) {
        matched = true;
        break;
      }
      branchFailures.push(inner);
    }
    if (!matched) {
      for (const inner of branchFailures[0] ?? [{ field, reason: 'does not match any oneOf branch' }]) {
        defects.push(inner);
      }
      return;
    }
    // fall through: sibling keywords on the same node still apply after a oneOf match
  }

  if (schema.type !== undefined) {
    const types = Array.isArray(schema.type) ? schema.type : [schema.type];
    if (!types.some((type) => typeMatches(value, type))) {
      defects.push({ field, reason: `must be ${types.join('|')}` });
      return;
    }
  }

  if (Object.hasOwn(schema, 'const') && value !== schema.const) {
    defects.push({ field, reason: `must be ${JSON.stringify(schema.const)}` });
  }

  if (Array.isArray(schema.enum) && !schema.enum.includes(value)) {
    defects.push({ field, reason: `must be one of ${schema.enum.join('|')}` });
  }

  if (typeof schema.minLength === 'number' && typeof value === 'string' && value.length < schema.minLength) {
    defects.push({
      field,
      reason: schema.minLength === 1 ? 'must be a non-empty string' : `must have minLength ${schema.minLength}`,
    });
  }

  if (typeof schema.pattern === 'string' && typeof value === 'string' && !new RegExp(schema.pattern).test(value)) {
    let reason = `must match ${schema.pattern}`;
    if (schema.pattern === '^[0-9a-fA-F]{40}$') reason = 'must be 40-hex';
    if (schema.pattern === '\\S') reason = 'must not be blank (whitespace-only strings are a false green)';
    defects.push({ field, reason });
  }

  if (jsType(value) === 'object' && schema.properties) {
    const required = Array.isArray(schema.required) ? schema.required : [];
    for (const key of required) {
      if (!Object.hasOwn(value, key)) {
        defects.push({ field: pointer(field, key), reason: 'missing required field' });
      }
    }
    for (const [key, child] of Object.entries(schema.properties)) {
      if (!Object.hasOwn(value, key)) continue;
      walk(value[key], child, pointer(field, key), defects, root);
    }
  }

  if (Array.isArray(value)) {
    if (typeof schema.minItems === 'number' && value.length < schema.minItems) {
      defects.push({ field, reason: `must have at least ${schema.minItems} item(s)` });
    }
    if (schema.items) {
      value.forEach((item, index) => {
        walk(item, schema.items, pointer(field, index), defects, root);
      });
    }
  }
}

function enforcementAnswersContract(text) {
  // Exact parity with scripts/cursor/validate-lane-receipt.mjs: the answer must name
  // WHERE/sequence/subject; no third keyword contract spelling.
  const lowered = text.toLowerCase();
  return lowered.includes('where') || lowered.includes('sequence') || lowered.includes('subject');
}

function blockingFindings(findings) {
  return findings.filter((f) => {
    if (!f || typeof f !== 'object' || f.ownerLease === true) return false;
    if (f.severity === 'blocker') return true;
    return f.severity === 'major' && f.provenByExecution === true;
  });
}

function nonEmptyCommands(value) {
  return Array.isArray(value) && value.length > 0
    && value.every((c) => typeof c === 'string' && c.trim().length > 0);
}

function applyConditionalRules(receipt, kind, defects) {
  if (kind === 'lane' && typeof receipt.enforcementPlacement === 'string' && receipt.enforcementPlacement.length > 0) {
    if (!NA_ENFORCEMENT.test(receipt.enforcementPlacement) && !enforcementAnswersContract(receipt.enforcementPlacement)) {
      defects.push({
        field: 'enforcementPlacement',
        reason: 'must start with "n/a - adds no enforcement" or answer WHERE the gate runs / its subject (see agent ritual card)',
      });
    }
  }
  if (kind === 'lane' && receipt.status === 'done' && !nonEmptyCommands(receipt.commands)) {
    defects.push({
      field: 'commands',
      reason: 'status=done requires commands: non-empty string[] with no blank entries (commands:[""] is a false green)',
    });
  }
  if (kind === 'critic' && Array.isArray(receipt.findings)) {
    if (receipt.verdict === 'BLOCK' && receipt.findings.length === 0) {
      defects.push({ field: 'findings', reason: 'empty findings array is valid only with verdict APPROVE' });
    }
    if (receipt.verdict === 'APPROVE') {
      const blocking = blockingFindings(receipt.findings);
      if (blocking.length > 0) {
        defects.push({
          field: 'verdict',
          reason: `APPROVE conflicts with ${blocking.length} blocking finding(s) (blocker, or major+provenByExecution, ownerLease excluded)`,
        });
      }
    }
  }
}

const SHAPES = buildShapes();
const KINDS = new Set(SHAPES.keys());

function selectShape(kind) {
  return SHAPES.get(kind) ?? null;
}

function detectKind(receipt, forced) {
  if (forced) return forced;
  if (receipt && typeof receipt === 'object' && !Array.isArray(receipt) && KINDS.has(receipt.kind)) {
    return receipt.kind;
  }
  return null;
}

export function validateReceipt(receipt, forcedKind = null) {
  const defects = [];
  if (receipt === null || typeof receipt !== 'object' || Array.isArray(receipt)) {
    return [{ field: '', reason: 'must be a JSON object' }];
  }
  const kind = detectKind(receipt, forcedKind);
  if (!kind) {
    defects.push({ field: 'kind', reason: 'must be lane|critic' });
    return defects;
  }
  if (forcedKind && Object.hasOwn(receipt, 'kind') && receipt.kind !== forcedKind) {
    defects.push({ field: 'kind', reason: `does not match --schema ${forcedKind}` });
  }
  const shape = selectShape(kind);
  if (!shape) {
    defects.push({ field: 'kind', reason: 'must be lane|critic' });
    return defects;
  }
  walk(receipt, shape, '', defects, SCHEMA);
  applyConditionalRules(receipt, kind, defects);
  return defects;
}

export function parseArgs(argv) {
  let schemaKind = null;
  let dir = null;
  const files = [];
  const defects = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--schema') {
      const value = argv[++i];
      if (!KINDS.has(value)) {
        defects.push({ path: '', field: '--schema', reason: 'must be lane|critic' });
      } else {
        schemaKind = value;
      }
    } else if (arg === '--dir') {
      const value = argv[++i];
      if (typeof value !== 'string' || value.length === 0) {
        defects.push({ path: '', field: '--dir', reason: 'requires a directory path' });
      } else {
        dir = value;
      }
    } else if (arg.startsWith('-')) {
      defects.push({ path: '', field: arg, reason: 'unknown option' });
    } else {
      files.push(arg);
    }
  }
  return { schemaKind, dir, files, defects };
}

// Directory scan: validate every kind-bearing receipt under dir. Only receipts with NO own
// `kind` property predate this schema and stay under the incumbent validators until the
// consolidation lane migrates them; a receipt that carries a `kind` — recognised or not —
// is examined, so a misspelled or incumbent kind (e.g. "build") FAILS instead of slipping
// past the gate. A scan that examines ZERO kind-bearing receipts FAILS.
export function scanDir(dir, lines) {
  let entries;
  try {
    entries = readdirSync(dir).filter((name) => name.endsWith('.json')).sort();
  } catch (error) {
    lines.push(formatDefect(dir, '', `cannot read directory: ${error.message}`));
    return 0;
  }
  let examined = 0;
  for (const name of entries) {
    const file = join(dir, name);
    let receipt;
    try {
      receipt = JSON.parse(readFileSync(file, 'utf8'));
    } catch (error) {
      lines.push(formatDefect(file, '', `invalid JSON: ${error.message}`));
      continue;
    }
    if (!receipt || typeof receipt !== 'object' || Array.isArray(receipt) || !Object.hasOwn(receipt, 'kind')) {
      continue; // legacy pre-schema receipt (no kind discriminator): incumbent validators own it until consolidation
    }
    examined += 1;
    for (const defect of validateReceipt(receipt)) {
      lines.push(formatDefect(file, defect.field, defect.reason));
    }
  }
  if (examined === 0) {
    lines.push(formatDefect(dir, '', 'examined zero kind-bearing receipts (examined-zero must fail, never pass)'));
  }
  return examined;
}

function formatDefect(path, field, reason) {
  const bits = [path, field, reason].filter((part) => part !== '' && part != null);
  return bits.join(' ');
}

export function main(argv = process.argv.slice(2)) {
  const { schemaKind, dir, files, defects: argDefects } = parseArgs(argv);
  if (files.length === 0 && dir === null) {
    console.error('examined zero receipts');
    return 1;
  }
  if (dir !== null) {
    const lines = argDefects.map((d) => formatDefect(d.path, d.field, d.reason));
    for (const file of files) lines.push(formatDefect(file, '', 'positional receipts cannot be combined with --dir'));
    scanDir(dir, lines);
    if (lines.length) {
      for (const line of lines) console.error(line);
      return 1;
    }
    return 0;
  }
  const lines = argDefects.map((d) => formatDefect(d.path, d.field, d.reason));
  for (const file of files) {
    let raw;
    try {
      raw = readFileSync(file, 'utf8');
    } catch (error) {
      lines.push(formatDefect(file, '', `cannot read: ${error.message}`));
      continue;
    }
    let receipt;
    try {
      receipt = JSON.parse(raw);
    } catch (error) {
      lines.push(formatDefect(file, '', `invalid JSON: ${error.message}`));
      continue;
    }
    for (const defect of validateReceipt(receipt, schemaKind)) {
      lines.push(formatDefect(file, defect.field, defect.reason));
    }
  }
  if (lines.length) {
    for (const line of lines) console.error(line);
    return 1;
  }
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  process.exit(main());
}
