import { createHash } from 'node:crypto';

function stable(value) { if (Array.isArray(value)) return value.map(stable); if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).sort(([a],[b]) => a.localeCompare(b, 'en')).map(([key, child]) => [key, stable(child)])); return value; }
function canonicalReceiptDigest(value) { return createHash('sha256').update(JSON.stringify(stable(value))).digest('hex'); }

function skipJsonWhitespace(text, index) { while (index < text.length && /\s/.test(text[index])) index += 1; return index; }
function scanJsonString(text, index, label) {
  if (text[index] !== '"') throw new Error(`${label}: expected JSON string`);
  const start = index; index += 1;
  while (index < text.length) {
    const character = text[index];
    if (character === '"') return { end: index + 1, value: JSON.parse(text.slice(start, index + 1)) };
    if (character === '\\') { index += 2; continue; }
    if (character.charCodeAt(0) < 0x20) throw new Error(`${label}: invalid JSON string`);
    index += 1;
  }
  throw new Error(`${label}: unterminated JSON string`);
}
function scanJsonValue(text, index, label) {
  index = skipJsonWhitespace(text, index);
  if (text[index] === '"') return scanJsonString(text, index, label).end;
  if (text[index] === '{') {
    index = skipJsonWhitespace(text, index + 1); const keys = new Set();
    if (text[index] === '}') return index + 1;
    while (true) {
      const key = scanJsonString(text, index, label);
      if (keys.has(key.value)) throw new Error(`${label}: duplicate JSON key: ${key.value}`);
      keys.add(key.value); index = skipJsonWhitespace(text, key.end);
      if (text[index] !== ':') throw new Error(`${label}: expected JSON object colon`);
      index = scanJsonValue(text, index + 1, label); index = skipJsonWhitespace(text, index);
      if (text[index] === '}') return index + 1;
      if (text[index] !== ',') throw new Error(`${label}: expected JSON object delimiter`);
      index = skipJsonWhitespace(text, index + 1);
    }
  }
  if (text[index] === '[') {
    index = skipJsonWhitespace(text, index + 1);
    if (text[index] === ']') return index + 1;
    while (true) {
      index = scanJsonValue(text, index, label); index = skipJsonWhitespace(text, index);
      if (text[index] === ']') return index + 1;
      if (text[index] !== ',') throw new Error(`${label}: expected JSON array delimiter`);
      index = skipJsonWhitespace(text, index + 1);
    }
  }
  const end = text.slice(index).search(/[\s,}\]]/);
  if (end === 0 || index >= text.length) throw new Error(`${label}: invalid JSON value`);
  return end < 0 ? text.length : index + end;
}
export function parseImmutableJson(text, label = 'immutable JSON') {
  if (typeof text !== 'string') throw new Error(`${label}: immutable JSON must be text`);
  const end = scanJsonValue(text, 0, label);
  if (skipJsonWhitespace(text, end) !== text.length) throw new Error(`${label}: trailing JSON data`);
  const value = JSON.parse(text);
  return { value, raw_sha256: createHash('sha256').update(text).digest('hex'), canonical_sha256: canonicalReceiptDigest(value) };
}
