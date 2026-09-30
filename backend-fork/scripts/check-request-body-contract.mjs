// H-1 gate: prove the request bodies that can be resolved mechanically, and name every body or
// enum direction that cannot. The register is an exact snapshot, not a suppression list: live
// mismatches are always findings, while every undecidable entry must match its source-derived
// operation, binding metadata, and reason byte-for-byte.

import { readFileSync, readdirSync } from "node:fs";
import {
  basename,
  dirname,
  extname,
  isAbsolute,
  join,
  relative,
  resolve,
  sep,
} from "node:path";
import { fileURLToPath } from "node:url";

import yaml from "js-yaml";

import { hasOwnKey, own } from "./own-property.mjs";

const ANCHORS = [
  "POST /api/v1/equipment-3r/rental-cases/{case_id}/handover",
  "POST /api/v1/inventory/items/{item_id}/consumptions",
  "POST /api/v1/inventory/items/{item_id}/receipts",
];

const ENUM_ANCHORS = [
  "POST /api/v1/benefit-catalog/items#category",
  "PATCH /api/v1/benefit-catalog/items/{benefit_id}#category",
  "POST /api/v1/evaluation/cycles#kind",
  "PUT /api/v1/evaluation/subjects/{subject_id}/reviews/{kind}#grade",
  "POST /api/v1/evaluation/subjects/{subject_id}/calibrate#final_grade",
  "POST /api/v1/inventory/cycle-counts/{count_id}/lines#reason",
  "POST /api/v1/inventory/cycle-counts/{count_id}/decision#decision",
];

const RESOLVED_FLOOR = 45;
const CENSUS_FLOOR = 291;
const ENUM_RESOLVED_FLOOR = 7;
const BODY_UNDECIDABLE_MAX = 238;
const ENUM_UNDECIDABLE_MAX = 15;
const REGISTER_VERSION = 1;
const REGISTER_PATH = "scripts/request-body-contract-undecidable.json";
const HTTP_METHODS = ["get", "put", "post", "delete", "options", "head", "patch", "trace"];

const BODY_REASONS = new Set([
  "non_json_request_body",
  "route_parser_unresolved",
  "no_direct_json_binding",
  "openapi_schema_ref_chain_unsupported",
  "openapi_schema_composition_unsupported",
  "rust_struct_not_strict",
  "no_openapi_request_body",
]);
const ENUM_REASONS = new Set([
  "string_backed_spec_enum",
  "tagged_or_data_enum",
  "rust_enum_unsupported",
  "rust_enum_ambiguous",
  "rust_enum_unresolved",
  "openapi_enum_schema_unsupported",
]);
const INERT_MODULE_ATTRIBUTES = new Set([
  "allow",
  "deprecated",
  "deny",
  "doc",
  "expect",
  "forbid",
  "macro_use",
  "no_implicit_prelude",
  "warn",
]);

// These expressions intentionally cover the concrete first-party route and handler forms. Any
// new syntax falls into the exact undecidable register instead of being guessed.
const CONST_PATH = /pub const ([A-Z0-9_]+): &str =\s*"([^"]+)"/g;
const ROUTE = /\.route\(\s*([A-Z0-9_]+)\s*,([\s\S]*?)\)\s*,?\s*\)/g;
const METHOD = /\b(get|post|put|patch|delete)\(\s*([a-z0-9_]+)/g;
const HANDLER = /async fn ([a-z0-9_]+)\s*\(([\s\S]*?)\)\s*->/g;
const JSON_BODY = /Json\(\s*\w+\s*\)\s*:\s*Json<\s*((?:[A-Za-z_][A-Za-z0-9_]*::)*[A-Za-z_][A-Za-z0-9_]*)\s*>/;
const ITEM = /((?:#\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?(struct|enum)\s+([A-Za-z_][A-Za-z0-9_]*)(?:\s*<[^>{;]*>)?\s*\{/g;
const USE = /\b(?:pub(?:\([^)]*\))?\s+)?use\s+([\s\S]*?);/g;
const RENAME_ALL = /rename_all\s*=\s*"([A-Za-z_-]+)"/;

function compareText(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

export function renameField(name, style) {
  const pascal = () => name.split("_").filter(Boolean)
    .map((word) => word[0].toUpperCase() + word.slice(1)).join("");
  switch (style) {
    case "camelCase":
      return pascal().replace(/^./, (first) => first.toLowerCase());
    case "PascalCase":
      return pascal();
    case "kebab-case":
      return name.replaceAll("_", "-");
    case "SCREAMING-KEBAB-CASE":
      return name.replaceAll("_", "-").toUpperCase();
    case "UPPERCASE":
    case "SCREAMING_SNAKE_CASE":
      return name.toUpperCase();
    default:
      return name;
  }
}

// Mirrors serde_derive_internals::case::RenameRule::apply_to_variant. Variant names begin in
// PascalCase, so these rules are deliberately not implemented through renameField.
export function renameVariant(name, style) {
  const snake = () => [...name].map((character, index) => {
    const separator = index > 0 && character >= "A" && character <= "Z" ? "_" : "";
    return `${separator}${character.toLowerCase()}`;
  }).join("");
  switch (style) {
    case "lowercase":
      return name.toLowerCase();
    case "UPPERCASE":
      return name.toUpperCase();
    case "camelCase":
      return name.replace(/^./, (first) => first.toLowerCase());
    case "snake_case":
      return snake();
    case "SCREAMING_SNAKE_CASE":
      return snake().toUpperCase();
    case "kebab-case":
      return snake().replaceAll("_", "-");
    case "SCREAMING-KEBAB-CASE":
      return snake().replaceAll("_", "-").toUpperCase();
    default:
      return name;
  }
}

function rustFiles(directory, collected = []) {
  let entries;
  try {
    entries = readdirSync(directory, { withFileTypes: true })
      .sort((left, right) => compareText(left.name, right.name));
  } catch {
    return collected;
  }
  for (const entry of entries) {
    if (entry.name === "target" || entry.name === "node_modules" || entry.name === ".git") continue;
    const path = join(directory, entry.name);
    if (entry.isDirectory()) rustFiles(path, collected);
    else if (entry.name.endsWith(".rs")) collected.push(path);
  }
  return collected;
}

function parseStructFields(body) {
  const fields = [];
  let pendingRename = null;
  let pendingDefault = false;
  for (const rawLine of body.split("\n")) {
    const line = rawLine.trim();
    if (line.startsWith("#[")) {
      pendingRename = line.match(/\brename\s*=\s*"([^"]+)"/)?.[1] ?? pendingRename;
      if (/\bdefault\b/.test(line)) pendingDefault = true;
      continue;
    }
    if (line.startsWith("//") || line === "") continue;
    const field = line.match(/^(?:pub(?:\([^)]*\))?\s+)?(?:r#)?([a-z0-9_]+)\s*:\s*(.+?),?$/);
    if (field) {
      fields.push({ name: field[1], type: field[2], rename: pendingRename, hasDefault: pendingDefault });
    }
    pendingRename = null;
    pendingDefault = false;
  }
  return fields;
}

// This deliberately small lexical scanner is shared by module/item discovery and enum-variant
// splitting. Rust comments are whitespace, block comments nest, literals are indivisible, and
// (), [], and {} must balance. Keeping those rules in one place prevents one caller from
// interpreting braces or commas that another caller correctly knows are literal bytes.
function codePointBefore(source, index) {
  if (index <= 0) return null;
  let start = index - 1;
  const trailing = source.charCodeAt(start);
  if (trailing >= 0xDC00 && trailing <= 0xDFFF && start > 0) {
    const leading = source.charCodeAt(start - 1);
    if (leading >= 0xD800 && leading <= 0xDBFF) start -= 1;
  }
  return source.slice(start, index);
}

function rawStringOpening(source, index) {
  let cursor;
  if (source[index] === "r") cursor = index + 1;
  else if ((source[index] === "b" || source[index] === "c") && source[index + 1] === "r") {
    cursor = index + 2;
  } else return null;
  if (/^[_\p{ID_Continue}]$/u.test(codePointBefore(source, index) ?? "")) return null;
  const hashStart = cursor;
  while (source[cursor] === "#") cursor += 1;
  if (source[cursor] !== '"') return null;
  const hashes = source.slice(hashStart, cursor);
  return hashes.length <= 255
    ? { valid: true, opening: source.slice(index, cursor + 1), closing: `"${hashes}` }
    : { valid: false };
}

function cookedStringClosing(source, opening) {
  let escaped = false;
  for (let index = opening + 1; index < source.length; index += 1) {
    const character = source[index];
    if (escaped) escaped = false;
    else if (character === "\\") escaped = true;
    else if (character === '"') return index;
  }
  return -1;
}

function characterLiteralClosing(source, opening) {
  let index = opening + 1;
  if (index >= source.length || source[index] === "\n" || source[index] === "\r") return -1;
  if (source[index] === "\\") {
    index += 1;
    if (source[index] === "u" && source[index + 1] === "{") {
      const closingBrace = source.indexOf("}", index + 2);
      if (closingBrace < 0 || /[\r\n]/.test(source.slice(index, closingBrace))) return -1;
      const digits = source.slice(index + 2, closingBrace);
      if (!/^[0-9A-Fa-f_]+$/.test(digits) || !/[0-9A-Fa-f]/.test(digits)) return -1;
      const scalar = Number.parseInt(digits.replaceAll("_", ""), 16);
      if (scalar > 0x10FFFF || (scalar >= 0xD800 && scalar <= 0xDFFF)) return -1;
      index = closingBrace + 1;
    } else if (source[index] === "x") {
      if (!/^[0-9A-Fa-f]{2}$/.test(source.slice(index + 1, index + 3))) return -1;
      index += 3;
    } else {
      if (!/[0nrt\\'\"]/.test(source[index] ?? "")) return -1;
      index += 1;
    }
  } else {
    const point = source.codePointAt(index);
    if (point === undefined) return -1;
    index += point > 0xFFFF ? 2 : 1;
  }
  return source[index] === "'" ? index : -1;
}

function rustIdentifierStartsAt(source, index) {
  const point = source.codePointAt(index);
  return point !== undefined && /^[_\p{ID_Start}]$/u.test(String.fromCodePoint(point));
}

function commentWhitespace(comment) {
  return comment.replace(/[^\r\n]/g, " ");
}

function rustFilePrelude(source) {
  let cursor = 0;
  if (source.startsWith("\uFEFF")) cursor += 1;
  if (source.startsWith("#!", cursor) && source[cursor + 2] !== "[") {
    const newline = source.indexOf("\n", cursor + 2);
    cursor = newline < 0 ? source.length : newline + 1;
  }
  return {
    cursor,
    maskedSource: commentWhitespace(source.slice(0, cursor)) + source.slice(cursor),
  };
}

function scanRustSyntax(source, {
  start = 0,
  rootDelimiter = null,
  splitTopLevel = false,
  projectCode = false,
} = {}) {
  const closes = { "(": ")", "[": "]", "{": "}" };
  const opens = new Set(Object.keys(closes));
  const closing = new Set(Object.values(closes));
  const delimiters = [];
  const segments = [];
  let segment = "";
  let projection = "";
  const append = (text) => {
    if (splitTopLevel) segment += text;
  };
  const project = (text) => {
    if (projectCode) projection += text;
  };
  const invalid = (error) => ({ valid: false, closing: -1, segments: [], projection: null, error });

  if (rootDelimiter !== null && source[start] !== rootDelimiter) {
    return invalid(`expected root delimiter ${rootDelimiter}`);
  }

  for (let index = start; index < source.length;) {
    const character = source[index];
    const next = source[index + 1];

    if (character === "/" && next === "/") {
      const newline = source.indexOf("\n", index + 2);
      const end = newline < 0 ? source.length : newline + 1;
      const whitespace = commentWhitespace(source.slice(index, end));
      append(whitespace);
      project(whitespace);
      index = end;
      continue;
    }
    if (character === "/" && next === "*") {
      let depth = 1;
      let cursor = index + 2;
      while (cursor < source.length && depth > 0) {
        if (source[cursor] === "/" && source[cursor + 1] === "*") {
          depth += 1;
          cursor += 2;
        } else if (source[cursor] === "*" && source[cursor + 1] === "/") {
          depth -= 1;
          cursor += 2;
        } else {
          cursor += 1;
        }
      }
      if (depth > 0) return invalid("unterminated block comment");
      const whitespace = commentWhitespace(source.slice(index, cursor));
      append(whitespace);
      project(whitespace);
      index = cursor;
      continue;
    }

    const raw = rawStringOpening(source, index);
    if (raw) {
      if (!raw.valid) return invalid("raw string delimiter exceeds Rust's 255-hash limit");
      const rawEnd = source.indexOf(raw.closing, index + raw.opening.length);
      if (rawEnd < 0) return invalid("unterminated raw string literal");
      const end = rawEnd + raw.closing.length;
      const literal = source.slice(index, end);
      append(literal);
      project(commentWhitespace(literal));
      index = end;
      continue;
    }

    if (character === '"') {
      const stringEnd = cookedStringClosing(source, index);
      if (stringEnd < 0) return invalid("unterminated cooked string literal");
      const literal = source.slice(index, stringEnd + 1);
      append(literal);
      project(commentWhitespace(literal));
      index = stringEnd + 1;
      continue;
    }

    if (character === "'") {
      const literalEnd = characterLiteralClosing(source, index);
      if (literalEnd >= 0) {
        const literal = source.slice(index, literalEnd + 1);
        append(literal);
        project(commentWhitespace(literal));
        index = literalEnd + 1;
        continue;
      }
      if (!rustIdentifierStartsAt(source, index + 1)) {
        return invalid("unsupported or unterminated character literal");
      }
    }

    if (opens.has(character)) {
      delimiters.push(character);
    } else if (closing.has(character)) {
      const opening = delimiters.at(-1);
      if (!opening || closes[opening] !== character) {
        return invalid(`unbalanced closing delimiter ${character}`);
      }
      delimiters.pop();
      if (rootDelimiter !== null && delimiters.length === 0) {
        return { valid: true, closing: index, segments: [], projection: null, error: null };
      }
    } else if (character === "," && splitTopLevel && delimiters.length === 0) {
      segments.push(segment.trim());
      segment = "";
      project(character);
      index += 1;
      continue;
    }
    append(character);
    project(character);
    index += 1;
  }

  if (rootDelimiter !== null) return invalid(`unterminated root delimiter ${rootDelimiter}`);
  if (delimiters.length > 0) return invalid(`unterminated delimiter ${delimiters.at(-1)}`);
  const tail = segment.trim();
  if (tail) segments.push(tail);
  return { valid: true, closing: -1, segments, projection, error: null };
}

function topLevelSegments(body) {
  return scanRustSyntax(body, { splitTopLevel: true });
}

function rustName(name) {
  return name.replace(/^r#/, "");
}

function rustPath(path) {
  const trimmed = path.trim().replace(/^::\s*/, "");
  if (!trimmed) return null;
  const pieces = trimmed.split(/\s*::\s*/).map(rustName);
  return pieces.every((piece) => /^(?:[A-Za-z_][A-Za-z0-9_]*|crate|self|super)$/.test(piece))
    ? pieces
    : null;
}

function parseUseTree(tree, prefix = []) {
  const parsed = topLevelSegments(tree);
  if (!parsed.valid) return [];
  const bindings = [];
  for (const rawSegment of parsed.segments) {
    const segment = rawSegment.trim();
    if (!segment) continue;
    const groupOpening = segment.indexOf("{");
    if (groupOpening >= 0) {
      const group = scanRustSyntax(segment, { start: groupOpening, rootDelimiter: "{" });
      const before = segment.slice(0, groupOpening).trim().replace(/::\s*$/, "");
      const beforePath = before ? rustPath(before) : [];
      if (!group.valid || group.closing !== segment.length - 1 || beforePath === null) continue;
      bindings.push(...parseUseTree(
        segment.slice(groupOpening + 1, group.closing),
        [...prefix, ...beforePath],
      ));
      continue;
    }
    const aliasMatch = segment.match(/^([\s\S]*?)\s+as\s+((?:r#)?[A-Za-z_][A-Za-z0-9_]*)$/);
    const path = rustPath(aliasMatch?.[1] ?? segment);
    if (!path || path.at(-1) === "*") continue;
    const combined = [...prefix, ...path];
    const selfImport = combined.at(-1) === "self";
    const target = selfImport ? combined.slice(0, -1) : combined;
    const name = rustName(aliasMatch?.[2] ?? target.at(-1) ?? "");
    if (name && target.length > 0) bindings.push({ name, path: target });
  }
  return bindings;
}

function skipProjectedWhitespace(projection, start) {
  let cursor = start;
  while (/\s/.test(projection[cursor] ?? "")) cursor += 1;
  return cursor;
}

function innerAttributeOpening(projection, start) {
  const prefix = projection.slice(start).match(/^#\s*!\s*(?=\[)/)?.[0];
  return prefix === undefined ? null : start + prefix.length;
}

function moduleAttributeInfo(attributes, file, moduleName) {
  let conditional = false;
  let path = null;
  for (const attribute of attributes) {
    const projected = scanRustSyntax(attribute, { projectCode: true });
    if (!projected.valid) {
      throw new Error(`cannot scan module attribute ${file}::${moduleName}: ${projected.error}`);
    }
    const name = projected.projection.match(/^#\[\s*([A-Za-z_][A-Za-z0-9_]*)\b/)?.[1];
    if (name === "cfg" || name === "cfg_attr") {
      conditional = true;
      if (name === "cfg_attr" && /\bpath\b/.test(projected.projection)) {
        throw new Error(
          `conditional path attribute for ${file}::${moduleName} has no single source identity`,
        );
      }
      continue;
    }
    if (name !== "path") {
      if (!INERT_MODULE_ATTRIBUTES.has(name)) {
        throw new Error(`unsupported module attribute for ${file}::${moduleName}`);
      }
      continue;
    }
    if (path !== null) {
      throw new Error(`duplicate path attributes for ${file}::${moduleName}`);
    }
    const cooked = attribute.match(/^#\[\s*path\s*=\s*"([^"\\\r\n]*)"\s*\]$/);
    const raw = attribute.match(/^#\[\s*path\s*=\s*r(#{0,255})"([\s\S]*?)"\1\s*\]$/);
    path = cooked?.[1] ?? raw?.[2] ?? null;
    if (path === null || path === "" || /[\r\n]/.test(path) || isAbsolute(path)) {
      throw new Error(`malformed or non-relative path attribute for ${file}::${moduleName}`);
    }
  }
  return { conditional, path };
}

function validateCfgBody(body, file, moduleName, attributeName) {
  const parsed = topLevelSegments(body);
  if (!parsed.valid || parsed.segments.length !== 1 || !parsed.segments[0]?.trim()) {
    throw new Error(`malformed inner ${attributeName} attribute for ${file}::${moduleName}`);
  }
}

function innerCfgAttrConditional(body, file, moduleName) {
  const parsed = topLevelSegments(body);
  if (!parsed.valid || parsed.segments.length < 2 || !parsed.segments[0]?.trim()) {
    throw new Error(`malformed inner cfg_attr attribute for ${file}::${moduleName}`);
  }
  let conditional = false;
  for (const nested of parsed.segments.slice(1)) {
    const name = nested.match(/^([A-Za-z_][A-Za-z0-9_]*)\b/)?.[1];
    if (name === "cfg") {
      const cfg = nested.match(/^cfg\s*\(([\s\S]*)\)$/);
      if (!cfg) throw new Error(`malformed inner cfg attribute for ${file}::${moduleName}`);
      validateCfgBody(cfg[1], file, moduleName, "cfg");
      conditional = true;
      continue;
    }
    if (name === "cfg_attr") {
      const cfgAttr = nested.match(/^cfg_attr\s*\(([\s\S]*)\)$/);
      if (!cfgAttr) {
        throw new Error(`malformed inner cfg_attr attribute for ${file}::${moduleName}`);
      }
      conditional = innerCfgAttrConditional(cfgAttr[1], file, moduleName) || conditional;
      continue;
    }
    if (!INERT_MODULE_ATTRIBUTES.has(name)) {
      throw new Error(`unsupported inner module attribute for ${file}::${moduleName}`);
    }
  }
  return conditional;
}

function innerModuleAttributeInfo(attributes, file, modulePath) {
  let conditional = false;
  const moduleName = modulePath.join("::") || "<crate>";
  for (const attribute of attributes) {
    const scanned = scanRustSyntax(attribute, { projectCode: true });
    if (!scanned.valid) {
      throw new Error(`cannot scan inner module attribute ${file}::${moduleName}: ${scanned.error}`);
    }
    const name = scanned.projection.match(
      /^#\s*!\s*\[\s*([A-Za-z_][A-Za-z0-9_]*)\b/,
    )?.[1];
    if (name === "cfg") {
      const cfg = scanned.projection.match(
        /^#\s*!\s*\[\s*cfg\s*\(([\s\S]*)\)\s*\]$/,
      );
      if (!cfg) throw new Error(`malformed inner cfg attribute for ${file}::${moduleName}`);
      validateCfgBody(cfg[1], file, moduleName, "cfg");
      conditional = true;
      continue;
    }
    if (name === "cfg_attr") {
      const cfgAttr = scanned.projection.match(
        /^#\s*!\s*\[\s*cfg_attr\s*\(([\s\S]*)\)\s*\]$/,
      );
      if (!cfgAttr) {
        throw new Error(`malformed inner cfg_attr attribute for ${file}::${moduleName}`);
      }
      conditional = innerCfgAttrConditional(cfgAttr[1], file, moduleName) || conditional;
      continue;
    }
    if (!INERT_MODULE_ATTRIBUTES.has(name)) {
      throw new Error(`unsupported inner module attribute for ${file}::${moduleName}`);
    }
  }
  return { conditional };
}

function moduleScopeHead(source, projection, start, end, file, modulePath) {
  let cursor = skipProjectedWhitespace(projection, start);
  const attributes = [];
  for (let opening = innerAttributeOpening(projection, cursor);
    opening !== null;
    opening = innerAttributeOpening(projection, cursor)) {
    const group = scanRustSyntax(projection, { start: opening, rootDelimiter: "[" });
    if (!group.valid || group.closing >= end) {
      throw new Error(
        `cannot scan Rust inner module attributes ${file}: ${group.error ?? "attribute escapes module"}`,
      );
    }
    attributes.push(source.slice(cursor, group.closing + 1));
    cursor = skipProjectedWhitespace(projection, group.closing + 1);
  }
  if (/^#\s*!/.test(projection.slice(cursor))) {
    throw new Error(`Rust shebang outside the start of a file in ${file}`);
  }
  return { cursor, ...innerModuleAttributeInfo(attributes, file, modulePath) };
}

function moduleHead(authored, projection, file, terminator) {
  let cursor = skipProjectedWhitespace(projection, 0);
  const attributes = [];
  while (projection.startsWith("#[", cursor)) {
    const group = scanRustSyntax(projection, { start: cursor + 1, rootDelimiter: "[" });
    if (!group.valid) {
      throw new Error(`cannot scan Rust module attributes ${file}: ${group.error}`);
    }
    attributes.push(authored.slice(cursor, group.closing + 1));
    cursor = skipProjectedWhitespace(projection, group.closing + 1);
  }
  if (projection.startsWith("#![", cursor)) {
    throw new Error(`inner module attribute outside the start of a scope in ${file}`);
  }
  const ending = terminator === ";" ? ";" : "";
  const declaration = projection.slice(cursor).match(new RegExp(
    `^(?:pub(?:\\s*\\([^)]*\\))?\\s+)?(?:unsafe\\s+)?mod\\s+((?:r#)?[A-Za-z_][A-Za-z0-9_]*)\\s*${ending}\\s*$`,
  ));
  if (!declaration) return null;
  const name = rustName(declaration[1]);
  return { name, ...moduleAttributeInfo(attributes, file, name) };
}

function pathIsWithin(root, candidate) {
  const path = relative(root, candidate);
  return path === "" || (!isAbsolute(path) && path !== ".." && !path.startsWith(`..${sep}`));
}

function moduleScopes(
  source,
  projection,
  file,
  absoluteFile,
  backendRoot,
  rootModulePath,
  rootConditional,
  rootPathRemapped,
  rootStart,
) {
  const scopes = [];
  const outlined = [];
  const modRs = ["lib.rs", "main.rs", "mod.rs"].includes(basename(absoluteFile));
  const fileStem = basename(absoluteFile, extname(absoluteFile));
  const rootModuleDirectory = modRs || rootPathRemapped
    ? dirname(absoluteFile)
    : join(dirname(absoluteFile), fileStem);
  const visit = (start, end, modulePath, conditional, moduleDirectory, insideInline) => {
    const surface = Array(end - start).fill(" ");
    const scopeHead = moduleScopeHead(source, projection, start, end, file, modulePath);
    const scopeConditional = conditional || scopeHead.conditional;
    let itemStart = scopeHead.cursor;
    for (let index = scopeHead.cursor; index < end;) {
      const character = projection[index];
      if (character === "(" || character === "[" || character === "{") {
        const group = scanRustSyntax(projection, { start: index, rootDelimiter: character });
        if (!group.valid || group.closing >= end) {
          throw new Error(`cannot scan Rust module scope ${file}: ${group.error ?? "delimiter escapes module"}`);
        }
        surface[index - start] = character;
        surface[group.closing - start] = projection[group.closing];
        if (character === "{") {
          const head = surface.slice(itemStart - start, index - start).join("");
          const authoredHead = source.slice(itemStart, index);
          const inlineModule = moduleHead(authoredHead, head, file, "{");
          const useGroup = /^\s*(?:#\[[\s\S]*?\]\s*)*(?:pub(?:\s*\([^)]*\))?\s+)?use\s+(?:(?:(?:r#)?[A-Za-z_][A-Za-z0-9_]*)\s*::\s*)*$/.test(head);
          if (inlineModule) {
            const pathBase = insideInline ? moduleDirectory : dirname(absoluteFile);
            const childDirectory = inlineModule.path === null
              ? join(moduleDirectory, inlineModule.name)
              : resolve(pathBase, inlineModule.path);
            if (!pathIsWithin(backendRoot, childDirectory)) {
              throw new Error(`module path for ${file}::${inlineModule.name} escapes backend`);
            }
            visit(
              index + 1,
              group.closing,
              [...modulePath, inlineModule.name],
              scopeConditional || inlineModule.conditional,
              childDirectory,
              true,
            );
          } else if (useGroup) {
            const contents = projection.slice(index + 1, group.closing);
            for (let offset = 0; offset < contents.length; offset += 1) {
              surface[index + 1 - start + offset] = contents[offset];
            }
          }
          if (!useGroup) itemStart = group.closing + 1;
        }
        index = group.closing + 1;
        continue;
      }
      surface[index - start] = character;
      if (character === ";") {
        const projectedHead = surface.slice(itemStart - start, index + 1 - start).join("");
        const authoredHead = source.slice(itemStart, index + 1);
        const declaration = moduleHead(authoredHead, projectedHead, file, ";");
        if (declaration) {
          outlined.push({
            ...declaration,
            modulePath,
            conditional: scopeConditional || declaration.conditional,
            moduleDirectory,
            pathBase: insideInline ? moduleDirectory : dirname(absoluteFile),
          });
        }
        itemStart = index + 1;
      }
      index += 1;
    }
    const joined = surface.join("");
    if (/#\s*!/.test(joined)) {
      throw new Error(`Rust shebang or inner attribute outside the start of a module in ${file}`);
    }
    const imports = [];
    USE.lastIndex = 0;
    let useMatch;
    while ((useMatch = USE.exec(joined)) !== null) imports.push(...parseUseTree(useMatch[1]));
    scopes.push({ start, end, modulePath, surface: joined, imports, conditional: scopeConditional });
  };
  visit(rootStart, source.length, rootModulePath, rootConditional, rootModuleDirectory, false);
  return { scopes, outlined };
}

function crateQualifier(file) {
  const crate = file.match(/^backend\/crates\/([^/]+)\/([^/]+)\//);
  if (crate) return `console_${crate[1].replaceAll("-", "_")}_${crate[2].replaceAll("-", "_")}`;
  const sourceRoot = file.match(/^(.+)\/src\//)?.[1] ?? file;
  return sourceRoot.replace(/[^A-Za-z0-9_]/g, "_");
}

function sourceModulePath(file) {
  const source = file.match(/\/src\/(.+)\.rs$/);
  if (!source) return [`<file:${file}>`];
  const path = source[1].split("/");
  if (path.length === 1 && (path[0] === "lib" || path[0] === "main")) return [];
  if (path.at(-1) === "mod") path.pop();
  return path;
}

function serdeAttributes(attributes) {
  return [...attributes.matchAll(/#\[serde\(([^\]]*)\)\]/g)].map((match) => match[1]).join(",");
}

function parseEnum(attributes, body) {
  const serde = serdeAttributes(attributes);
  const derivesDeserialize = [...attributes.matchAll(/#\[derive\(([^\]]*)\)\]/g)]
    .some((match) => /(?:^|[,:\s])(?:serde::)?Deserialize(?:$|[,:\s])/.test(match[1]));
  const parsed = topLevelSegments(body);
  const variants = [];
  let hasVariantAttribute = false;
  let hasData = false;
  let hasUnsupportedSyntax = !parsed.valid;
  for (const segment of parsed.segments) {
    const cleaned = segment.trim();
    if (!cleaned) continue;
    if (cleaned.startsWith("#")) {
      hasVariantAttribute = true;
      continue;
    }
    const variant = cleaned.match(/^([A-Za-z_][A-Za-z0-9_]*)/);
    if (!variant) {
      hasUnsupportedSyntax = true;
      continue;
    }
    variants.push(variant[1]);
    const suffix = cleaned.slice(variant[0].length).trim();
    if (suffix.startsWith("(") || suffix.startsWith("{")) hasData = true;
    else if (suffix && !suffix.startsWith("=")) hasUnsupportedSyntax = true;
  }
  let reason = null;
  if (hasData || /(?:^|,)\s*(?:tag|content|untagged)\b/.test(serde)) {
    reason = "tagged_or_data_enum";
  } else if (
    !derivesDeserialize
    || hasVariantAttribute
    || hasUnsupportedSyntax
    || /(?:^|,)\s*(?:remote|from|try_from)\b/.test(serde)
    || variants.length === 0
  ) {
    reason = "rust_enum_unsupported";
  }
  return {
    renameAll: serde.match(RENAME_ALL)?.[1] ?? null,
    variants,
    reason,
  };
}

function parseItems(source, file, absoluteFile, backendRoot, identity) {
  const structs = [];
  const enums = [];
  const prelude = rustFilePrelude(source);
  const projected = scanRustSyntax(prelude.maskedSource, { projectCode: true });
  if (!projected.valid) {
    throw new Error(`cannot scan Rust source ${file}: ${projected.error}`);
  }
  if (projected.projection.includes("\uFEFF")) {
    throw new Error(`UTF-8 BOM outside the start of a Rust file in ${file}`);
  }
  const { scopes, outlined } = moduleScopes(
    source,
    projected.projection,
    file,
    absoluteFile,
    backendRoot,
    identity.modulePath,
    identity.conditional,
    identity.pathRemapped,
    prelude.cursor,
  );
  for (const scope of scopes) {
    ITEM.lastIndex = 0;
    let match;
    while ((match = ITEM.exec(scope.surface)) !== null) {
      const opening = scope.start + ITEM.lastIndex - 1;
      const scan = scanRustSyntax(source, { start: opening, rootDelimiter: "{" });
      if (!scan.valid || scan.closing > scope.end) {
        throw new Error(`cannot scan Rust item body ${file}::${match[3]}: ${scan.error ?? "body escapes module"}`);
      }
      const itemStart = scope.start + match.index;
      const attributes = source.slice(itemStart, itemStart + match[1].length);
      const body = source.slice(opening + 1, scan.closing);
      const common = {
        file,
        modulePath: scope.modulePath,
        imports: scope.imports,
        crateQualifier: identity.crateQualifier,
        conditional: scope.conditional || /#\s*\[\s*cfg(?:_attr)?\s*\(/.test(attributes),
        name: match[3],
      };
      if (match[2] === "struct") {
        const serde = serdeAttributes(attributes);
        structs.push({
          ...common,
          denyUnknown: /\bdeny_unknown_fields\b/.test(serde),
          renameAll: serde.match(RENAME_ALL)?.[1] ?? null,
          fields: parseStructFields(body),
        });
      } else {
        enums.push({ ...common, ...parseEnum(attributes, body) });
      }
    }
  }
  const rootImports = scopes.find((scope) => sameModule(scope.modulePath, identity.modulePath))?.imports ?? [];
  return { structs, enums, rootImports, outlined };
}

function rustCrateRoot(file) {
  const normalized = file.split(sep).join("/");
  return /\/src\/(?:lib|main)\.rs$/.test(normalized)
    || /\/src\/bin\/(?:[^/]+\.rs|[^/]+\/main\.rs)$/.test(normalized)
    || /\/(?:tests|benches|examples)\/[^/]+\.rs$/.test(normalized)
    || /\/build\.rs$/.test(normalized);
}

function logicalModuleKey(crate, modulePath) {
  return `${crate}\0${modulePath.join("\0")}`;
}

function sourceIdentityKey(identity) {
  return `${identity.graphRoot}\0${identity.absolute}\0${logicalModuleKey(
    identity.crateQualifier,
    identity.modulePath,
  )}`;
}

function resolveOutlinedSource(declaration, fileSet, backendRoot, file) {
  const childModulePath = [...declaration.modulePath, declaration.name];
  const defaults = [
    resolve(declaration.moduleDirectory, `${declaration.name}.rs`),
    resolve(declaration.moduleDirectory, declaration.name, "mod.rs"),
  ];
  if (declaration.path !== null) {
    if (extname(declaration.path) !== ".rs") {
      return { childModulePath, target: null, unavailable: "non-Rust module target" };
    }
    const target = resolve(declaration.pathBase, declaration.path);
    if (!pathIsWithin(backendRoot, target)) {
      throw new Error(`module target for ${file}::${childModulePath.join("::")} escapes backend`);
    }
    if (!fileSet.has(target)) {
      return { childModulePath, target: null, unavailable: "missing module target" };
    }
    return { childModulePath, target, pathRemapped: true };
  }
  const matches = defaults.filter((candidate) => fileSet.has(candidate));
  if (matches.length !== 1) {
    const reason = matches.length === 0 ? "missing" : "ambiguous";
    return { childModulePath, target: null, unavailable: `${reason} ordinary module target` };
  }
  return { childModulePath, target: matches[0], pathRemapped: false };
}

// A Rust file has no module identity of its own: an enclosing `mod` declaration supplies it.
// Walk conventional crate roots so ordinary and `#[path]` modules receive their declared logical
// paths. Files outside those graphs are not Rust modules merely because their names look like a
// logical path. Missing/ambiguous targets therefore resolve nothing; they never reactivate a
// filename decoy.
function collectRustSourceIdentities(repoRoot) {
  const backendRoot = resolve(repoRoot, "backend");
  const absoluteFiles = rustFiles(backendRoot).map((file) => resolve(file));
  const fileSet = new Set(absoluteFiles);
  const sources = new Map(absoluteFiles.map((absolute) => [absolute, readFileSync(absolute, "utf8")]));
  const roots = new Set(absoluteFiles.filter((absolute) => rustCrateRoot(relative(repoRoot, absolute))));
  const mappings = new Map();
  const queued = new Set();
  const queue = [];
  const results = [];

  const enqueue = (identity) => {
    const key = sourceIdentityKey(identity);
    if (queued.has(key)) return;
    queued.add(key);
    queue.push(identity);
  };
  for (const absolute of roots) {
    const file = relative(repoRoot, absolute);
    enqueue({
      absolute,
      file,
      modulePath: sourceModulePath(file),
      crateQualifier: crateQualifier(file),
      conditional: false,
      pathRemapped: false,
      graphRoot: absolute,
      ancestors: [absolute],
    });
  }

  for (let cursor = 0; cursor < queue.length; cursor += 1) {
    const identity = queue[cursor];
    const source = sources.get(identity.absolute);
    const items = parseItems(source, identity.file, identity.absolute, backendRoot, identity);
    results.push({ identity, source, items });
    for (const declaration of items.outlined) {
      const resolved = resolveOutlinedSource(declaration, fileSet, backendRoot, identity.file);
      const mappingKey = `${identity.graphRoot}\0${logicalModuleKey(
        identity.crateQualifier,
        resolved.childModulePath,
      )}`;
      if (mappings.has(mappingKey)) {
        throw new Error(
          `duplicate or conflicting module declarations for ${identity.file}::${resolved.childModulePath.join("::")}`,
        );
      }
      mappings.set(mappingKey, resolved.target ?? resolved.unavailable);
      if (resolved.target === null) continue;
      if (identity.ancestors.includes(resolved.target)) {
        throw new Error(`cyclic module mapping through ${identity.file}::${resolved.childModulePath.join("::")}`);
      }
      enqueue({
        absolute: resolved.target,
        file: relative(repoRoot, resolved.target),
        modulePath: resolved.childModulePath,
        crateQualifier: identity.crateQualifier,
        conditional: declaration.conditional,
        pathRemapped: resolved.pathRemapped,
        graphRoot: identity.graphRoot,
        ancestors: [...identity.ancestors, resolved.target],
      });
    }
  }

  return results;
}

function collectSources(repoRoot) {
  const consts = new Map();
  const structs = [];
  const enums = [];
  const handlers = new Map();
  const rawRoutes = [];
  const sourceIdentities = collectRustSourceIdentities(repoRoot);

  for (const { identity, source, items } of sourceIdentities) {
    const { file } = identity;
    const identityKey = sourceIdentityKey(identity);
    for (const match of source.matchAll(CONST_PATH)) {
      const candidates = consts.get(match[1]) ?? [];
      candidates.push({ file, identityKey, path: match[2] });
      consts.set(match[1], candidates);
    }
    structs.push(...items.structs);
    enums.push(...items.enums);
    for (const match of source.matchAll(HANDLER)) {
      handlers.set(`${identityKey}::${match[1]}`, match[2].match(JSON_BODY)?.[1] ?? null);
    }
    for (const match of source.matchAll(ROUTE)) {
      for (const method of match[2].matchAll(METHOD)) {
        rawRoutes.push({
          file,
          identityKey,
          modulePath: identity.modulePath,
          crateQualifier: identity.crateQualifier,
          imports: items.rootImports,
          constName: match[1],
          method: method[1],
          handler: method[2],
        });
      }
    }
  }

  const routes = rawRoutes.map((route) => {
    const candidates = consts.get(route.constName) ?? [];
    const local = candidates.filter((candidate) => candidate.identityKey === route.identityKey);
    const path = local.length === 1 ? local[0].path : candidates.length === 1 ? candidates[0].path : null;
    return {
      ...route,
      path,
      bodyType: handlers.get(`${route.identityKey}::${route.handler}`) ?? null,
    };
  });
  return { structs, enums, routes };
}

function canonicalPath(path) {
  if (typeof path !== "string") return null;
  const placeholder = /\{[^/{}}]+\}/g;
  if (/[{}]/.test(path.replace(placeholder, ""))) return null;
  return path.replace(placeholder, "{}");
}

function operationKey(method, path) {
  const canonical = canonicalPath(path);
  return canonical ? `${method.toLowerCase()} ${canonical}` : null;
}

function namedType(type) {
  let remaining = type.trim();
  const option = remaining.match(/^Option\s*<\s*([\s\S]+)\s*>$/);
  if (option) remaining = option[1].trim();
  if (!/^(?:[A-Za-z_][A-Za-z0-9_]*::)*[A-Za-z_][A-Za-z0-9_]*$/.test(remaining)) return null;
  const pieces = remaining.split("::");
  return { qualified: remaining, pieces, name: pieces.at(-1) };
}

function sameModule(left, right) {
  return left.length === right.length && left.every((piece, index) => piece === right[index]);
}

function decideResolution(candidates) {
  if (candidates.some((candidate) => candidate.conditional)) {
    return { value: null, status: "ambiguous" };
  }
  if (candidates.length === 1) return { value: candidates[0], status: "resolved" };
  if (candidates.length > 1) return { value: null, status: "ambiguous" };
  return { value: null, status: "missing" };
}

function localModulePath(contextPath, path) {
  const remaining = [...path];
  let base = [...contextPath];
  if (remaining[0] === "crate") {
    base = [];
    remaining.shift();
  } else if (remaining[0] === "self") {
    remaining.shift();
  } else {
    while (remaining[0] === "super") {
      if (base.length === 0) return null;
      base.pop();
      remaining.shift();
    }
  }
  return [...base, ...remaining];
}

function resolvePath(items, context, path) {
  const name = path.at(-1);
  const prefix = path.slice(0, -1);
  if (!name) return { value: null, status: "missing" };

  const explicitLocal = ["crate", "self", "super"].includes(prefix[0]);
  const relativePath = localModulePath(context.modulePath, prefix);
  const local = relativePath === null ? [] : items.filter((candidate) => (
    candidate.crateQualifier === context.crateQualifier
    && candidate.name === name
    && sameModule(candidate.modulePath, relativePath)
  ));
  if (explicitLocal || local.length > 0) return decideResolution(local);

  const qualifier = prefix[0];
  const qualified = qualifier
    ? items.filter((candidate) => (
      candidate.name === name
      && candidate.crateQualifier === qualifier
      && sameModule(candidate.modulePath, prefix.slice(1))
    ))
    : [];
  if (qualified.length > 0) return decideResolution(qualified);

  if (prefix.length > 0) return { value: null, status: "missing" };

  return decideResolution(items.filter((candidate) => (
    (
      candidate.crateQualifier !== context.crateQualifier
      || !sameModule(candidate.modulePath, context.modulePath)
    )
    && candidate.name === name
  )));
}

function resolveNamed(items, context, named) {
  if (named.pieces.length > 1) {
    const aliases = context.imports.filter((binding) => binding.name === named.pieces[0]);
    if (aliases.length > 1) return { value: null, status: "ambiguous" };
    if (aliases.length === 1) {
      return resolvePath(items, context, [...aliases[0].path, ...named.pieces.slice(1)]);
    }
    return resolvePath(items, context, named.pieces);
  }

  const local = items.filter((candidate) => (
    candidate.crateQualifier === context.crateQualifier
    && candidate.name === named.name
    && sameModule(candidate.modulePath, context.modulePath)
  ));
  const imports = context.imports.filter((binding) => binding.name === named.name);
  if (local.length > 0 && imports.length > 0) return { value: null, status: "ambiguous" };
  if (local.length > 0) return decideResolution(local);
  if (imports.length > 1) return { value: null, status: "ambiguous" };
  if (imports.length === 1) return resolvePath(items, context, imports[0].path);

  return decideResolution(items.filter((candidate) => (
    (
      candidate.crateQualifier !== context.crateQualifier
      || !sameModule(candidate.modulePath, context.modulePath)
    )
    && candidate.name === named.name
  )));
}

function schemaReference(document, reference) {
  if (typeof reference !== "string" || !reference.startsWith("#/components/schemas/")) return null;
  const name = reference.slice("#/components/schemas/".length);
  if (!name || name.includes("/")) return null;
  const schema = own(own(own(document, "components"), "schemas"), name);
  return schema && typeof schema === "object" ? schema : null;
}

function requestBodySchema(document, path, method) {
  const requestBody = own(own(own(document, "paths"), path), method)?.requestBody;
  if (!requestBody || typeof requestBody !== "object") return { reason: "non_json_request_body" };
  const media = own(own(requestBody, "content"), "application/json");
  if (!media || typeof media !== "object") return { reason: "non_json_request_body" };
  const raw = own(media, "schema");
  if (!raw || typeof raw !== "object") return { reason: "openapi_schema_composition_unsupported" };
  let schema = raw;
  const reference = own(schema, "$ref");
  if (typeof reference === "string") {
    schema = schemaReference(document, reference);
    if (!schema || typeof own(schema, "$ref") === "string") {
      return { reason: "openapi_schema_ref_chain_unsupported" };
    }
  }
  if (own(schema, "oneOf") || own(schema, "allOf") || own(schema, "anyOf")) {
    return { reason: "openapi_schema_composition_unsupported" };
  }
  return { schema };
}

export function jsonRequestSchema(document, path, method) {
  return requestBodySchema(document, path, method).schema ?? null;
}

function schemaEnum(document, original, seen = new Set()) {
  if (!original || typeof original !== "object") return { kind: "none" };
  const reference = own(original, "$ref");
  if (typeof reference === "string") {
    if (seen.has(reference)) return { kind: "unsupported" };
    const resolved = schemaReference(document, reference);
    if (!resolved) return { kind: "unsupported" };
    return schemaEnum(document, resolved, new Set([...seen, reference]));
  }
  const oneOf = own(original, "oneOf");
  if (Array.isArray(oneOf)) {
    const nonNull = oneOf.filter((member) => own(member, "type") !== "null");
    const nulls = oneOf.length - nonNull.length;
    if (nonNull.length === 1 && nulls >= 1) return schemaEnum(document, nonNull[0], seen);
    return { kind: "unsupported" };
  }
  if (own(original, "allOf") || own(original, "anyOf")) return { kind: "unsupported" };
  const values = own(original, "enum");
  if (Array.isArray(values)) {
    return values.every((value) => typeof value === "string")
      ? { kind: "enum", values: [...new Set(values)].sort() }
      : { kind: "unsupported" };
  }
  if (own(original, "type") === "string") return { kind: "string" };
  return { kind: "none" };
}

function bodyEntry(operation, route, reason) {
  return {
    operation,
    rust_file: route?.file ?? null,
    handler: route?.handler ?? null,
    body_type: route?.bodyType ?? null,
    reason,
  };
}

function enumEntry(operation, wireField, rustFile, bodyType, rustType, reason) {
  return {
    operation,
    wire_field: wireField,
    rust_file: rustFile,
    body_type: bodyType,
    rust_type: rustType,
    reason,
  };
}

function bodyId(entry) {
  return entry.operation;
}

function enumId(entry) {
  return `${entry.operation}#${entry.wire_field}`;
}

function sortRegister(register) {
  register.body.sort((left, right) => compareText(bodyId(left), bodyId(right)));
  register.enum.sort((left, right) => compareText(enumId(left), enumId(right)));
  return register;
}

function validateEntryShape(entry, keys, nullableKeys) {
  if (!entry || typeof entry !== "object" || Array.isArray(entry)) return false;
  const actualKeys = Object.keys(entry).sort();
  if (JSON.stringify(actualKeys) !== JSON.stringify([...keys].sort())) return false;
  return keys.every((key) => {
    if (nullableKeys.has(key)) return entry[key] === null || typeof entry[key] === "string";
    return typeof entry[key] === "string" && entry[key].length > 0;
  });
}

function compareRegisterKind(kind, registered, observed, idFor) {
  const findings = [];
  const registeredById = new Map(registered.map((entry) => [idFor(entry), entry]));
  const observedById = new Map(observed.map((entry) => [idFor(entry), entry]));
  for (const [id, entry] of observedById) {
    const recorded = registeredById.get(id);
    if (!recorded) findings.push(`unregistered ${kind} undecidable: ${id}`);
    else if (JSON.stringify(recorded) !== JSON.stringify(entry)) {
      findings.push(`${kind} register metadata drift: ${id}`);
    }
  }
  for (const id of registeredById.keys()) {
    if (!observedById.has(id)) findings.push(`stale ${kind} register entry: ${id}`);
  }
  return findings;
}

function inspectRegister(repoRoot, observedRegister) {
  let registered;
  try {
    registered = JSON.parse(readFileSync(join(repoRoot, REGISTER_PATH), "utf8"));
  } catch (error) {
    return [error?.code === "ENOENT"
      ? `missing undecidable register: ${REGISTER_PATH}`
      : `malformed undecidable register: ${REGISTER_PATH}`];
  }
  if (
    !registered
    || typeof registered !== "object"
    || Array.isArray(registered)
    || JSON.stringify(Object.keys(registered).sort()) !== JSON.stringify(["body", "enum", "version"])
    || registered.version !== REGISTER_VERSION
    || !Array.isArray(registered.body)
    || !Array.isArray(registered.enum)
  ) {
    return [`malformed undecidable register: ${REGISTER_PATH}`];
  }

  const findings = [];
  const bodyKeys = ["operation", "rust_file", "handler", "body_type", "reason"];
  const enumKeys = ["operation", "wire_field", "rust_file", "body_type", "rust_type", "reason"];
  for (const entry of registered.body) {
    if (!validateEntryShape(entry, bodyKeys, new Set(["rust_file", "handler", "body_type"]))) {
      findings.push("malformed body register entry");
    } else if (!BODY_REASONS.has(entry.reason)) {
      findings.push(`unknown body undecidable reason: ${entry.reason}`);
    }
  }
  for (const entry of registered.enum) {
    if (!validateEntryShape(entry, enumKeys, new Set())) findings.push("malformed enum register entry");
    else if (!ENUM_REASONS.has(entry.reason)) findings.push(`unknown enum undecidable reason: ${entry.reason}`);
  }

  for (const [kind, entries, idFor] of [
    ["body", registered.body, bodyId],
    ["enum", registered.enum, enumId],
  ]) {
    const ids = entries.map(idFor);
    for (let index = 1; index < ids.length; index += 1) {
      if (ids[index] === ids[index - 1]) findings.push(`duplicate ${kind} register entry: ${ids[index]}`);
      else if (compareText(ids[index], ids[index - 1]) < 0) findings.push(`${kind} register is not sorted`);
    }
  }
  if (findings.length > 0) return [...new Set(findings)];
  return [
    ...compareRegisterKind("body", registered.body, observedRegister.body, bodyId),
    ...compareRegisterKind("enum", registered.enum, observedRegister.enum, enumId),
  ].sort();
}

function specOperations(document) {
  const operations = [];
  const paths = own(document, "paths");
  if (!paths || typeof paths !== "object") return operations;
  for (const [path, pathItem] of Object.entries(paths)) {
    if (!pathItem || typeof pathItem !== "object") continue;
    for (const method of HTTP_METHODS) {
      const operation = own(pathItem, method);
      if (!operation || typeof operation !== "object" || !hasOwnKey(operation, "requestBody")) continue;
      operations.push({ path, method, operation: `${method.toUpperCase()} ${path}` });
    }
  }
  return operations.sort((left, right) => compareText(left.operation, right.operation));
}

function compareBody({ operation, schema, struct, findings }) {
  const properties = own(schema, "properties");
  if (!properties || typeof properties !== "object") return false;
  const wireName = (field) => field.rename ?? renameField(field.name, struct.renameAll);
  const wireNames = new Set(struct.fields.map(wireName));
  const required = own(schema, "required");
  const specRequired = new Set(Array.isArray(required) ? required : []);

  for (const property of Object.keys(properties)) {
    if (!wireNames.has(property)) {
      findings.push({
        operation,
        message: `spec property "${property}" is not a field of ${struct.name} (deny_unknown_fields => 422)`,
      });
    }
  }
  for (const field of struct.fields) {
    if (/^Option\s*</.test(field.type) || field.hasDefault) continue;
    if (specRequired.has(wireName(field))) continue;
    if (wireName(field) !== field.name && hasOwnKey(properties, field.name)) continue;
    findings.push({
      operation,
      message: `${struct.name}.${field.name} is required by the handler but not in spec required[]`,
    });
  }
  return true;
}

function compareEnums({ document, operation, schema, struct, enums, findings, enumUndecidable }) {
  const properties = own(schema, "properties");
  if (!properties || typeof properties !== "object") return { candidates: 0, resolved: 0, ids: [] };
  let candidates = 0;
  let resolved = 0;
  const ids = [];
  for (const field of struct.fields) {
    const wireField = field.rename ?? renameField(field.name, struct.renameAll);
    const property = own(properties, wireField);
    if (!property || typeof property !== "object") continue;
    const spec = schemaEnum(document, property);
    const type = namedType(field.type);
    const rustName = type?.name ?? field.type.trim();
    let rust = { kind: "none" };
    if (type?.name === "String") rust = { kind: "string" };
    else if (type) {
      const resolution = resolveNamed(enums, struct, type);
      if (resolution.status === "ambiguous") rust = { kind: "ambiguous" };
      else if (resolution.status === "resolved") rust = { kind: "enum", value: resolution.value };
    }
    if (spec.kind !== "enum" && rust.kind !== "enum" && rust.kind !== "ambiguous") continue;
    candidates += 1;
    const id = `${operation}#${wireField}`;

    let reason = null;
    if (rust.kind === "string" && spec.kind === "enum") reason = "string_backed_spec_enum";
    else if (rust.kind === "ambiguous") reason = "rust_enum_ambiguous";
    else if (rust.kind === "none" && spec.kind === "enum") reason = "rust_enum_unresolved";
    else if (rust.kind === "enum" && rust.value.reason) reason = rust.value.reason;
    else if (rust.kind === "enum" && spec.kind === "unsupported") reason = "openapi_enum_schema_unsupported";
    else if (rust.kind === "enum" && spec.kind === "none") reason = "openapi_enum_schema_unsupported";

    if (reason) {
      enumUndecidable.push(enumEntry(
        operation,
        wireField,
        struct.file,
        struct.name,
        rustName,
        reason,
      ));
      continue;
    }
    if (rust.kind !== "enum") continue;
    resolved += 1;
    ids.push(id);
    const rustValues = rust.value.variants
      .map((variant) => renameVariant(variant, rust.value.renameAll))
      .sort(compareText);
    if (spec.kind === "string") {
      findings.push({
        operation,
        message: `spec property "${wireField}" does not constrain serde enum ${rust.value.name}`,
      });
      continue;
    }
    const specValues = spec.values;
    for (const value of specValues.filter((value) => !rustValues.includes(value))) {
      findings.push({ operation, message: `spec-only enum variant "${value}" for ${wireField}` });
    }
    for (const value of rustValues.filter((value) => !specValues.includes(value))) {
      findings.push({ operation, message: `Rust-only enum variant "${value}" for ${wireField}` });
    }
  }
  return { candidates, resolved, ids };
}

/**
 * Evaluate the source-first request-body and enum contract.
 *
 * @param {{ repoRoot: string }} options
 */
export function evaluateRequestBodyContract({ repoRoot }) {
  const document = yaml.load(readFileSync(join(repoRoot, "backend/openapi/openapi.yaml"), "utf8"));
  const { structs, enums, routes } = collectSources(repoRoot);
  const findings = [];
  const bodyUndecidable = [];
  const enumUndecidable = [];
  const resolvedOperations = new Set();
  const resolvedEnumIds = new Set();
  let enumCandidates = 0;
  let enumResolved = 0;

  const routeIndex = new Map();
  for (const route of routes) {
    if (!route.path) continue;
    const key = operationKey(route.method, route.path);
    if (!key) continue;
    const candidates = routeIndex.get(key) ?? [];
    if (!candidates.some((candidate) => (
      candidate.identityKey === route.identityKey && candidate.handler === route.handler
    ))) {
      candidates.push(route);
    }
    routeIndex.set(key, candidates);
  }

  const operations = specOperations(document);
  const specKeys = new Set();
  for (const candidate of operations) {
    const key = operationKey(candidate.method, candidate.path);
    if (!key || specKeys.has(key)) {
      findings.push({ operation: candidate.operation, message: "normalized OpenAPI operation collision" });
    } else specKeys.add(key);
  }

  for (const candidate of operations) {
    const key = operationKey(candidate.method, candidate.path);
    const matchingRoutes = key ? routeIndex.get(key) ?? [] : [];
    const route = matchingRoutes.length === 1 ? matchingRoutes[0] : null;
    const schemaResult = requestBodySchema(document, candidate.path, candidate.method);
    if (schemaResult.reason === "non_json_request_body") {
      bodyUndecidable.push(bodyEntry(candidate.operation, route, schemaResult.reason));
      continue;
    }
    if (!route) {
      bodyUndecidable.push(bodyEntry(candidate.operation, null, "route_parser_unresolved"));
      continue;
    }
    if (!route.bodyType) {
      bodyUndecidable.push(bodyEntry(candidate.operation, route, "no_direct_json_binding"));
      continue;
    }
    if (schemaResult.reason) {
      bodyUndecidable.push(bodyEntry(candidate.operation, route, schemaResult.reason));
      continue;
    }
    const bodyNamed = namedType(route.bodyType);
    const structResolution = bodyNamed
      ? resolveNamed(structs, {
        file: route.file,
        modulePath: route.modulePath,
        crateQualifier: route.crateQualifier,
        imports: route.imports,
      }, bodyNamed)
      : { value: null, status: "missing" };
    const struct = structResolution.value;
    if (!struct || !struct.denyUnknown || !compareBody({
      operation: candidate.operation,
      schema: schemaResult.schema,
      struct,
      findings,
    })) {
      bodyUndecidable.push(bodyEntry(candidate.operation, route, "rust_struct_not_strict"));
      continue;
    }
    resolvedOperations.add(candidate.operation);
    const enumReport = compareEnums({
      document,
      operation: candidate.operation,
      schema: schemaResult.schema,
      struct,
      enums,
      findings,
      enumUndecidable,
    });
    enumCandidates += enumReport.candidates;
    enumResolved += enumReport.resolved;
    for (const id of enumReport.ids) resolvedEnumIds.add(id);
  }

  const routeOnly = new Map();
  for (const route of routes) {
    if (!route.path || !route.bodyType) continue;
    const key = operationKey(route.method, route.path);
    if (!key || specKeys.has(key)) continue;
    const operation = `${route.method.toUpperCase()} ${route.path}`;
    const existing = routeOnly.get(key);
    if (existing && (existing.file !== route.file || existing.handler !== route.handler)) {
      findings.push({ operation, message: "normalized Rust route operation collision" });
      continue;
    }
    routeOnly.set(key, route);
  }
  for (const route of routeOnly.values()) {
    bodyUndecidable.push(bodyEntry(
      `${route.method.toUpperCase()} ${route.path}`,
      route,
      "no_openapi_request_body",
    ));
  }

  const observedRegister = sortRegister({
    version: REGISTER_VERSION,
    body: bodyUndecidable,
    enum: enumUndecidable,
  });
  const registerFindings = inspectRegister(repoRoot, observedRegister);
  findings.sort((left, right) => {
    const operationOrder = compareText(left.operation, right.operation);
    return operationOrder || compareText(left.message, right.message);
  });

  return {
    population: operations.length + routeOnly.size,
    resolved: resolvedOperations.size,
    skipped: bodyUndecidable.length,
    findings,
    unresolvedAnchors: ANCHORS.filter((anchor) => !resolvedOperations.has(anchor)),
    enumCandidates,
    enumResolved,
    enumSkipped: enumUndecidable.length,
    unresolvedEnumAnchors: ENUM_ANCHORS.filter((anchor) => !resolvedEnumIds.has(anchor)),
    observedRegister,
    registerFindings,
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const json = process.argv.includes("--json");
  const rootArgument = process.argv.slice(2).find((argument) => argument !== "--json");
  const repoRoot = rootArgument ?? fileURLToPath(new URL("..", import.meta.url));
  const report = evaluateRequestBodyContract({ repoRoot });
  if (json) console.log(JSON.stringify(report, null, 2));
  for (const finding of report.findings) console.error(`${finding.operation}: ${finding.message}`);
  for (const finding of report.registerFindings) console.error(finding);
  for (const anchor of report.unresolvedAnchors) {
    console.error(`anchor operation ${anchor} no longer resolves — the resolver has silently degraded`);
  }
  for (const anchor of report.unresolvedEnumAnchors) {
    console.error(`enum anchor ${anchor} no longer resolves — enum coverage has silently degraded`);
  }
  const belowFloor = report.resolved < RESOLVED_FLOOR;
  const belowCensus = report.population < CENSUS_FLOOR;
  const belowEnumFloor = report.enumResolved < ENUM_RESOLVED_FLOOR;
  const aboveBodyMaximum = report.skipped > BODY_UNDECIDABLE_MAX;
  const aboveEnumMaximum = report.enumSkipped > ENUM_UNDECIDABLE_MAX;
  if (belowFloor) {
    console.error(`resolved ${report.resolved} operations, below the floor of ${RESOLVED_FLOOR} — `
      + "the resolver compared less of the surface than it was built to compare");
  }
  if (belowCensus) {
    console.error(`request-body population ${report.population}, below the census floor of ${CENSUS_FLOOR}`);
  }
  if (belowEnumFloor) {
    console.error(`enum-resolved ${report.enumResolved}, below the floor of ${ENUM_RESOLVED_FLOOR}`);
  }
  if (aboveBodyMaximum) {
    console.error(`body-undecidable ${report.skipped}, above the maximum of ${BODY_UNDECIDABLE_MAX}`);
  }
  if (aboveEnumMaximum) {
    console.error(`enum-undecidable ${report.enumSkipped}, above the maximum of ${ENUM_UNDECIDABLE_MAX}`);
  }
  const failed = report.findings.length > 0
    || report.registerFindings.length > 0
    || report.unresolvedAnchors.length > 0
    || report.unresolvedEnumAnchors.length > 0
    || belowFloor
    || belowCensus
    || belowEnumFloor
    || aboveBodyMaximum
    || aboveEnumMaximum;
  if (failed) {
    console.error(`request body contract gate FAILED: ${report.findings.length} finding(s), `
      + `resolved ${report.resolved}, skipped ${report.skipped}, enum-resolved ${report.enumResolved}, `
      + `enum-skipped ${report.enumSkipped}`);
    process.exit(1);
  }
  if (!json) {
    console.log(`request body contract gate passed (resolved ${report.resolved}, skipped ${report.skipped})`);
    console.log(`request body census ${report.population}; enum candidates ${report.enumCandidates}, `
      + `resolved ${report.enumResolved}, skipped ${report.enumSkipped}`);
  }
}
