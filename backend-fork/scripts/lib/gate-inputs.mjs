/**
 * Bounded gate-input provenance helpers (S2 / K-2).
 *
 * Not a process-wide filesystem tracer. Only scripts that opt in via
 * beginGate()/noteRead()/noteAssertion() (the text-gate hook and the five
 * private-reader floor scripts) participate. Uninstrumented CI-run gates are
 * unresolved and fail the provenance check.
 */

import { writeFileSync } from "node:fs";
import process from "node:process";

/** @typedef {{ gate: string, script: string, documentInputs: string[] }} GateBegin */

/** @type {null | {
 *   gate: string,
 *   script: string,
 *   declared: Set<string>,
 *   reads: Set<string>,
 *   assertions: Map<string, number>,
 * }} */
let active = null;

export function beginGate(/** @type {GateBegin} */ meta) {
  if (!meta || typeof meta.gate !== "string" || typeof meta.script !== "string") {
    throw new Error("beginGate requires { gate, script, documentInputs }");
  }
  if (!Array.isArray(meta.documentInputs)) {
    throw new Error("beginGate.documentInputs must be an array of repo-relative paths");
  }
  active = {
    gate: meta.gate,
    script: meta.script,
    declared: new Set(meta.documentInputs),
    reads: new Set(),
    assertions: new Map(),
  };
}

export function noteRead(path) {
  if (!active || typeof path !== "string" || path.length === 0) return;
  active.reads.add(path);
}

export function noteAssertion(path) {
  if (!active || typeof path !== "string" || path.length === 0) return;
  noteRead(path);
  active.assertions.set(path, (active.assertions.get(path) || 0) + 1);
}

export function getActiveGateState() {
  if (!active) return null;
  return {
    gate: active.gate,
    script: active.script,
    declared: [...active.declared].sort(),
    reads: [...active.reads].sort(),
    assertions: Object.fromEntries(
      [...active.assertions.entries()].sort((a, b) => a[0].localeCompare(b[0])),
    ),
  };
}

/**
 * Compare declaration vs traced document reads. Only repo-relative paths that
 * look like documents (*.md) participate in declared-but-unread /
 * read-but-undeclared checks. Non-document reads (source, workflow, package)
 * are allowed without declaration.
 */
export function validateActiveGateTrace() {
  if (!active) {
    return { ok: false, errors: ["no active gate trace — untraceable reader"] };
  }
  const errors = [];
  const isDoc = (p) => p.endsWith(".md");
  const declaredDocs = [...active.declared].filter(isDoc).sort();
  const readDocs = [...active.reads].filter(isDoc).sort();

  for (const path of declaredDocs) {
    if (!active.reads.has(path)) {
      errors.push(`declared-but-unread: ${path}`);
    }
  }
  for (const path of readDocs) {
    if (!active.declared.has(path)) {
      errors.push(`read-but-undeclared: ${path}`);
    }
  }
  return { ok: errors.length === 0, errors, declaredDocs, readDocs };
}

/**
 * Emit rows for the instrument. One row per document path that was asserted
 * against (assertion_count > 0), plus declared-only paths with count 0 after
 * validation failure surfaces separately.
 */
export function rowsFromActiveGate(classByPath = new Map()) {
  if (!active) return [];
  const paths = new Set([...active.declared, ...active.assertions.keys()]);
  const rows = [];
  for (const input_path of [...paths].sort()) {
    if (!input_path.endsWith(".md")) continue;
    rows.push({
      gate: active.gate,
      script: active.script,
      input_path,
      class: classByPath.get(input_path) ?? null,
      assertion_count: active.assertions.get(input_path) || 0,
    });
  }
  return rows;
}

/**
 * When GATE_INPUT_PROVENANCE_OUT is set, write the active trace JSON there.
 * Used by scripts/check-gate-input-provenance.mjs while spawning each gate.
 */
export function emitProvenanceIfRequested() {
  const out = process.env.GATE_INPUT_PROVENANCE_OUT;
  if (!out || !active) return;
  const validation = validateActiveGateTrace();
  const payload = {
    ...getActiveGateState(),
    validation,
  };
  writeFileSync(out, `${JSON.stringify(payload, null, 2)}\n`, "utf8");
}

export function resetGateInputsForTests() {
  active = null;
}
