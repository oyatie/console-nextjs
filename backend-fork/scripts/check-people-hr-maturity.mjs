import { createTextGate } from "./lib/text-gate.mjs";
import { beginGate, emitProvenanceIfRequested } from "./lib/gate-inputs.mjs";

beginGate({
  gate: "check:people-hr-maturity",
  script: "scripts/check-people-hr-maturity.mjs",
  documentInputs: [
    "docs/specs/data-exchange-import-export.md",
  ],
});

const { requireIncludes, requireAbsent, reportGate } = createTextGate({
  includeFailure: ({ path, needle, label }) => `${path} is missing ${label}: ${needle}`,
  absentFailure: ({ path, label }) => `${path} still contains ${label}`,
  passLabel: (label, kind) => `${label} ${kind === "absent" ? "absent" : "present"}`,
});

requireIncludes(
  "backend/app/src/hr.rs",
  "cross-company transfer requires payroll cutoff and retirement-settlement signoffs",
  "backend transfer payroll/severance guard",
);
requireIncludes(
  "backend/openapi/openapi.yaml",
  "Record an employee lifecycle transition with legal signoffs",
  "OpenAPI lifecycle signoff contract",
);
requireIncludes(
  "docs/specs/data-exchange-import-export.md",
  "Do **not** coerce payroll, bank/account, resident registration number, disability status, or retirement-settlement fields into general `users` columns",
  "payroll-sensitive import boundary",
);
requireIncludes(
  "docs/benchmarks/enterprise-ui-route-audit.json",
  "G004-identity-group-org-people-policy-fou",
  "G004 ownership in enterprise UI audit",
);

emitProvenanceIfRequested();
reportGate("people HR maturity gate passed");
