import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

function read(relativePath) {
  return readFileSync(new URL(`../${relativePath}`, import.meta.url), "utf8");
}

/** Extract one components.schemas entry by exact key; order-independent. */
function schemaBlock(openapi, name) {
  const start = `    ${name}:\n`;
  const startIndex = openapi.indexOf(start);
  assert.notEqual(startIndex, -1, `missing schema: ${name}`);
  // Next schema key at the same indent, or end of components.schemas / file.
  const rest = openapi.slice(startIndex + start.length);
  const next = rest.search(/\n    [A-Za-z0-9_]+:\n/);
  const endIndex = next === -1 ? openapi.length : startIndex + start.length + next;
  return openapi.slice(startIndex, endIndex);
}

test("employee import replay accounting is required in the OpenAPI wire contract", () => {
  const openapi = read("backend/openapi/openapi.yaml");
  // Fragment compose may alphabetize schema keys; do not depend on neighbor order.
  for (const name of ["EmployeeImportCompanySummary", "EmployeeImportReport"]) {
    const schema = schemaBlock(openapi, name);
    assert.match(schema, /required:\n(?:      - [^\n]+\n)*      - skipped\n/);
    // Trailing newline may be absent when this is the last property in the block.
    assert.match(schema, /        skipped:\n          type: integer\n          minimum: 0(?:\n|$)/);
  }
});

test("hosted CI runs the employee import replay contract", () => {
  const workflow = read(".github/workflows/ci.yml");
  assert.match(workflow, /^\s*run:\s+npm run test:employee-import-contract\s*$/m);
});
