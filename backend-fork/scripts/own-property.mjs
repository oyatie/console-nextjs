// Own-property reads for maps that inherit Object.prototype (js-yaml / JSON.parse).
//
// The hole this closes: `obj[key] !== undefined`, `obj?.[key]`, and `key in obj` consult the
// prototype chain, so untrusted keys named `constructor`, `toString`, `__proto__`, … resolve
// against the language runtime instead of the document. Proven fail-open in
// scripts/check-openapi-refs.mjs (ann-critic on 9f5804a8d); this module is the shared total
// primitive for the class sweep (console-i91).

/**
 * @param {unknown} node
 * @returns {node is Record<string, unknown>}
 */
export function isPlainObject(node) {
  return typeof node === "object" && node !== null && !Array.isArray(node);
}

/**
 * Read an own property. Never returns a value inherited from Object.prototype.
 *
 * @param {unknown} node
 * @param {PropertyKey} key
 * @returns {unknown}
 */
export function own(node, key) {
  return isPlainObject(node) && Object.hasOwn(node, key) ? node[key] : undefined;
}

/**
 * @param {unknown} node
 * @param {PropertyKey} key
 * @returns {boolean}
 */
export function hasOwnKey(node, key) {
  return isPlainObject(node) && Object.hasOwn(node, key);
}

/** Keys that false-resolve on a prototype-ful empty object — shared regression seeds. */
export const PROTOTYPE_CHAIN_KEYS = Object.freeze([
  "constructor",
  "toString",
  "__proto__",
  "valueOf",
  "hasOwnProperty",
]);
