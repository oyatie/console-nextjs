import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const openapi = readFileSync(
  new URL("../backend/openapi/openapi.yaml", import.meta.url),
  "utf8",
);
const ciWorkflow = readFileSync(
  new URL("../.github/workflows/ci.yml", import.meta.url),
  "utf8",
);

// Each CAS-guarded ontology write gets its own slice: the paths are siblings in
// the document, not nested, so one extractor structurally cannot see another's
// operation — it stops at the next `\n  /api/`.
function operationSlice(spec, pathKey, method) {
  const start = spec.indexOf(`  ${pathKey}:`);
  assert.notEqual(start, -1, `missing path ${pathKey}`);
  const operation = spec.indexOf(`    ${method}:`, start);
  assert.notEqual(operation, -1, `missing ${method} on ${pathKey}`);
  const nextPath = spec.indexOf("\n  /api/", operation);
  return spec.slice(operation, nextPath);
}

function assertWritePreconditionContract(operation, label) {
  assert.match(
    operation,
    /name: If-Match[\s\S]*in: header[\s\S]*required: true/,
    label,
  );
  assert.match(operation, /'400':/, label);
  assert.match(operation, /'412':/, label);
  assert.match(operation, /'428':/, label);
  assert.match(operation, /headers:[\s\S]*ETag:/, label);
}

test("ontology stage declares strong required If-Match and exact precondition statuses", () => {
  assertWritePreconditionContract(
    operationSlice(openapi, "/api/v1/ontology/object-types/{key}", "put"),
    "stage revision",
  );
});

test("ontology lifecycle transition declares the same write precondition contract", () => {
  const operation = operationSlice(
    openapi,
    "/api/v1/ontology/object-types/{key}/lifecycle",
    "post",
  );
  assertWritePreconditionContract(operation, "lifecycle transition");
  // Key-only addressing resolves the published-preferred head, so a revision
  // staged behind a published one is only reachable through ?version=.
  assert.match(operation, /name: version[\s\S]*in: query/);
});

test("object type wire contract carries key revision", () => {
  const summaryStart = openapi.indexOf("    ObjectTypeSummary:");
  const summaryEnd = openapi.indexOf("\n    InstanceLifecycleState:", summaryStart);
  const summary = openapi.slice(summaryStart, summaryEnd);
  assert.match(summary, /required:[^\n]*key_write_revision/);
  assert.match(summary, /key_write_revision:/);
  assert.match(summary, /key_write_etag:/);
});

test("hosted CI runs the ontology write precondition contract", () => {
  assert.match(
    ciWorkflow,
    /^\s*run:\s+npm run test:ontology-write-precondition\s*$/m,
  );
});
