import { createTextGate } from "./lib/text-gate.mjs";
import { beginGate, emitProvenanceIfRequested } from "./lib/gate-inputs.mjs";

beginGate({
  gate: "check:payroll-release-gate",
  script: "scripts/check-payroll-release-gate.mjs",
  documentInputs: [
    "docs/specs/payroll.md",
  ],
});

const { requireIncludes, requireMatches, requireAbsent, reportGate } = createTextGate({
  includeFailure: ({ path, needle, label }) => `${path} is missing ${label}: ${needle}`,
  matchFailure: ({ path, pattern, label }) => `${path} does not satisfy ${label}: ${pattern}`,
  absentFailure: ({ path, pattern, label }) => `${path} violates ${label}: ${pattern}`,
  passLabel: (label, kind) => `${label} ${kind === "absent" ? "absent" : "present"}`,
});

requireIncludes(
  "docs/specs/payroll.md",
  "Status: first regulated-kernel slice",
  "regulated payroll spec status",
);
requireIncludes(
  "docs/specs/payroll.md",
  "Production payroll calculations are disabled unless all are true",
  "production payroll release gate",
);
requireIncludes(
  "docs/specs/payroll.md",
  "G028 production-control contract",
  "G028 production-control contract",
);
requireIncludes(
  "docs/specs/payroll.md",
  "generic employee import/export can only preview masked values",
  "generic HR payroll masking rule",
);
requireIncludes(
  "docs/specs/payroll.md",
  "payroll/wage-statement mail may exist as an audited work-mail object",
  "payroll receipt mail boundary",
);
requireIncludes(
  "docs/specs/payroll.md",
  "passkey step-up",
  "payroll signing-equivalent step-up rule",
);

// T1-HYG (counts zero toward decoupling): Rust doc-comment prose assertions removed.
// Executable coverage remains in console-payroll-domain unit tests (golden case,
// professional validation, artifact_sha256, NTS fail-closed). K-4: the NTS prose
// row was dropped rather than strengthened to .message equality.
// T1-b: remaining docs/specs/payroll.md assertions describe HOLD/absent controls
// and stay as registered residuals — no control is implemented here.

requireMatches(
  "package.json",
  /"check:payroll"\s*:\s*"node scripts\/check-payroll-domain\.mjs"/,
  "payroll domain script",
);
requireMatches(
  "package.json",
  /"check:payroll-release-gate"\s*:\s*"node scripts\/check-payroll-release-gate\.mjs"/,
  "payroll release-gate script",
);
requireIncludes(
  ".github/workflows/ci.yml",
  "npm run check:payroll-release-gate",
  "CI payroll release-gate wiring",
);

emitProvenanceIfRequested();
reportGate("payroll release gate check passed");
