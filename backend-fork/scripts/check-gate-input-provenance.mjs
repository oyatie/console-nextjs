#!/usr/bin/env node
/**
 * S2 gate-input provenance instrument.
 *
 * Spawns each CI-run floor gate with GATE_INPUT_PROVENANCE_OUT set so the
 * bounded text-gate hook / K-2 private readers emit a trace. Compares traces
 * to declarations, joins class from docs/documentation-index.json, enforces
 * the exception register, and emits the gate_inputs relation.
 *
 * gate_inputs is intentionally separate from document class (K-1).
 */

import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import process from "node:process";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const jsonMode = process.argv.includes("--json");
const asOf = new Date().toISOString().slice(0, 10);

/** Floor scripts that must resolve under the K-2 admission. */
const FLOOR_GATES = [
  {
    gate: "check:foundation-gates",
    script: "scripts/check-foundation-gates.mjs",
    argv: ["node", "scripts/check-foundation-gates.mjs"],
  },
  {
    gate: "check:payroll-release-gate",
    script: "scripts/check-payroll-release-gate.mjs",
    argv: ["node", "scripts/check-payroll-release-gate.mjs"],
  },
  {
    gate: "check:g008-payroll-readiness",
    script: "scripts/check-g008-payroll-readiness.mjs",
    argv: ["node", "scripts/check-g008-payroll-readiness.mjs"],
  },
  {
    gate: "check:people-hr-maturity",
    script: "scripts/check-people-hr-maturity.mjs",
    argv: ["node", "scripts/check-people-hr-maturity.mjs"],
  },
  {
    gate: "check:g004-identity-foundation",
    script: "scripts/check-g004-identity-foundation.mjs",
    argv: ["node", "scripts/check-g004-identity-foundation.mjs"],
  },
  {
    gate: "check:g005-workflow-lifecycle",
    script: "scripts/check-g005-workflow-lifecycle.mjs",
    argv: ["node", "scripts/check-g005-workflow-lifecycle.mjs"],
  },
  {
    gate: "check:g006-asset-dispatch-lifecycle",
    script: "scripts/check-g006-asset-dispatch-lifecycle.mjs",
    argv: ["node", "scripts/check-g006-asset-dispatch-lifecycle.mjs"],
  },
  {
    gate: "check:g007-collaboration-mobile-lifecycle",
    script: "scripts/check-g007-collaboration-mobile-lifecycle.mjs",
    argv: ["node", "scripts/check-g007-collaboration-mobile-lifecycle.mjs"],
  },
  {
    gate: "check:doc-citations",
    script: "scripts/console/verify-doc-citations.mjs",
    // package.json runs two docs; instrument runs both and merges rows.
    argv: null,
    multi: [
      ["node", "scripts/console/verify-doc-citations.mjs", "docs/ideas/ecosystem-plan-DRAFT.md"],
      ["node", "scripts/console/verify-doc-citations.mjs", "docs/program/false-green-gate-holes.md"],
    ],
  },
];

const COUNTED_CLASSES = new Set(["historical", "quarry", "evidence"]);
const exceptionsPath = "docs/program/gate-input-exceptions.json";

function loadClassByPath() {
  const indexPath = join(root, "docs/documentation-index.json");
  const index = JSON.parse(readFileSync(indexPath, "utf8"));
  const map = new Map();
  for (const doc of index.documents ?? []) {
    if (doc?.path) map.set(doc.path, doc.class ?? null);
  }
  return map;
}

function loadExceptions() {
  const abs = join(root, exceptionsPath);
  if (!existsSync(abs)) {
    throw new Error(`${exceptionsPath}: missing (must ship populated)`);
  }
  const raw = JSON.parse(readFileSync(abs, "utf8"));
  if (raw.schema_version !== 1) {
    throw new Error(`${exceptionsPath}: schema_version must be 1`);
  }
  if (!Array.isArray(raw.exceptions)) {
    throw new Error(`${exceptionsPath}: exceptions must be an array`);
  }
  return raw;
}

function runOne(argv, outPath) {
  const result = spawnSync(argv[0], argv.slice(1), {
    cwd: root,
    encoding: "utf8",
    env: {
      ...process.env,
      GATE_INPUT_PROVENANCE_OUT: outPath,
    },
    maxBuffer: 32 * 1024 * 1024,
  });
  return result;
}

function collectTrace(argv) {
  const dir = mkdtempSync(join(tmpdir(), "gate-input-"));
  const outPath = join(dir, "trace.json");
  try {
    const result = runOne(argv, outPath);
    if (!existsSync(outPath)) {
      return {
        ok: false,
        untraceable: true,
        exitCode: result.status,
        stderr: (result.stderr || "").slice(0, 2000),
        stdout: (result.stdout || "").slice(0, 500),
      };
    }
    const trace = JSON.parse(readFileSync(outPath, "utf8"));
    return {
      ok: true,
      untraceable: false,
      exitCode: result.status,
      trace,
      stderr: result.stderr || "",
    };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function rowsFromTrace(trace, classByPath) {
  const declared = new Set(trace.declared ?? []);
  const assertions = trace.assertions ?? {};
  const paths = new Set([...declared, ...Object.keys(assertions)]);
  const rows = [];
  for (const input_path of [...paths].sort()) {
    if (!input_path.endsWith(".md")) continue;
    rows.push({
      gate: trace.gate,
      script: trace.script,
      input_path,
      class: classByPath.get(input_path) ?? null,
      assertion_count: Number(assertions[input_path] || 0),
    });
  }
  return rows;
}

function main() {
  const failures = [];
  const classByPath = loadClassByPath();
  const exceptionDoc = loadExceptions();
  const exceptionRows = exceptionDoc.exceptions;
  const exceptionKeys = new Set(
    exceptionRows.map((e) => `${e.gate}::${e.input_path}`),
  );

  /** @type {Array<Record<string, unknown>>} */
  const gate_inputs = [];
  const resolvedGates = [];

  for (const floor of FLOOR_GATES) {
    const argvs = floor.multi ? floor.multi : [floor.argv];
    let gateHadTrace = false;
    for (const argv of argvs) {
      const collected = collectTrace(argv);
      if (collected.untraceable) {
        failures.push(
          `${floor.gate}: untraceable reader (no GATE_INPUT_PROVENANCE_OUT emission); exit=${collected.exitCode}`,
        );
        continue;
      }
      gateHadTrace = true;
      const { trace } = collected;
      if (trace.gate !== floor.gate) {
        failures.push(
          `${floor.gate}: trace gate id mismatch (got ${trace.gate})`,
        );
      }
      const validation = trace.validation ?? { ok: false, errors: ["missing validation"] };
      if (!validation.ok) {
        for (const err of validation.errors ?? []) {
          failures.push(`${floor.gate}: ${err}`);
        }
      }
      // Gate scripts must still pass under instrumentation.
      if (collected.exitCode !== 0) {
        failures.push(
          `${floor.gate}: underlying gate exited ${collected.exitCode}`,
        );
      }
      gate_inputs.push(...rowsFromTrace(trace, classByPath));
    }
    if (gateHadTrace) resolvedGates.push(floor.gate);
  }

  // Merge duplicate (gate, input_path) rows by summing assertion_count.
  const merged = new Map();
  for (const row of gate_inputs) {
    const key = `${row.gate}::${row.input_path}`;
    const prev = merged.get(key);
    if (!prev) {
      merged.set(key, { ...row });
    } else {
      prev.assertion_count += row.assertion_count;
    }
  }
  const rows = [...merged.values()].sort((a, b) =>
    a.gate === b.gate
      ? a.input_path.localeCompare(b.input_path)
      : a.gate.localeCompare(b.gate),
  );

  const counted = rows.filter(
    (r) => COUNTED_CLASSES.has(r.class) && r.assertion_count > 0,
  );

  // Exception register: growth / expiry / decoration.
  const baselineKeys = new Set(exceptionKeys);
  const countedKeys = new Set(counted.map((r) => `${r.gate}::${r.input_path}`));

  for (const ex of exceptionRows) {
    for (const field of ["gate", "input_path", "owner", "reason", "first_seen", "remove_by"]) {
      if (typeof ex[field] !== "string" || ex[field].length === 0) {
        failures.push(`${exceptionsPath}: exception missing ${field}`);
      }
    }
    if (ex.remove_by && ex.remove_by < asOf) {
      failures.push(
        `${exceptionsPath}: exception expired remove_by=${ex.remove_by} for ${ex.gate} ${ex.input_path}`,
      );
    }
    const key = `${ex.gate}::${ex.input_path}`;
    // decoration: exception names a pair the instrument no longer sees as counted
    if (!countedKeys.has(key)) {
      // Allow exceptions for counted rows that were converted (no longer asserted).
      // Decoration are exceptions for pairs with zero current assertions AND not
      // present as any row — or present with assertion_count 0 and class still counted.
      const stillRow = rows.find(
        (r) => r.gate === ex.gate && r.input_path === ex.input_path,
      );
      if (!stillRow) {
        failures.push(
          `${exceptionsPath}: decoration — ${ex.gate} ${ex.input_path} not observed by instrument`,
        );
      }
    }
  }

  // Unregistered counted rows fail.
  for (const row of counted) {
    const key = `${row.gate}::${row.input_path}`;
    if (!baselineKeys.has(key) && !row.converted) {
      // converted flag is not on rows; conversion removes assertions so they
      // leave `counted`. Unregistered counted rows are the fail condition.
      // Presence on the exception register is required for every counted row
      // that remains unconverted.
      if (!baselineKeys.has(key)) {
        failures.push(
          `unregistered counted row: ${row.gate} ${row.input_path} class=${row.class} assertions=${row.assertion_count}`,
        );
      }
    }
  }

  // Growth beyond committed baseline: exception file is the baseline; the
  // instrument does not invent new exception entries. Growth is enforced by
  // refusing extra exception keys beyond a frozen baseline_count field when set.
  if (typeof exceptionDoc.baseline_count === "number") {
    if (exceptionRows.length > exceptionDoc.baseline_count) {
      failures.push(
        `${exceptionsPath}: growth — exceptions.length ${exceptionRows.length} > baseline_count ${exceptionDoc.baseline_count}`,
      );
    }
  }

  const report = {
    schema_version: 1,
    relation: "gate_inputs",
    as_of: asOf,
    resolved_gates: resolvedGates,
    rows,
    counted_rows: counted,
    counted_total_assertions: counted.reduce((n, r) => n + r.assertion_count, 0),
    exception_count: exceptionRows.length,
    failures,
  };

  if (jsonMode) {
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  } else {
    console.log(
      `gate_inputs: ${rows.length} document rows; counted ${counted.length} rows / ${report.counted_total_assertions} assertions; exceptions ${exceptionRows.length}`,
    );
    for (const row of counted) {
      console.log(
        `  COUNTED ${row.gate} ${row.input_path} class=${row.class} assertions=${row.assertion_count}`,
      );
    }
    if (failures.length) {
      console.error(`gate-input provenance failed (${failures.length}):`);
      for (const f of failures) console.error(`- ${f}`);
    } else {
      console.log("gate-input provenance check passed");
    }
  }

  process.exit(failures.length ? 1 : 0);
}

main();
