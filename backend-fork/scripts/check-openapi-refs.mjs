// Ref totality gate over the published OpenAPI document.
//
// The hole this closes: the compose-time check in backend/crates/contracts/src/lib.rs is two
// TEXT scans, and its own module doc records four positions proven to fail open (PR #620
// review): a flow-style $ref (including the foreign-URL case the check exists to close), a
// quoted pointer with an internal space that truncates to a resolvable prefix, an implicit
// discriminator.mapping schema NAME, and a foreign prefix ending in characters the backward
// scan treats as delimiters. Two text scans cannot be total over YAML; the total primitive is
// parsing and walking every node. That crate has zero dependencies, so the parser lives here,
// where js-yaml is already vendored, and runs over the artifact every fail-open lands in:
// backend/openapi/openapi.yaml, the file include_str! serves and generated clients consume.
//
// Totality argument, stated so it is checkable: js-yaml resolves flow style, quoting, block
// scalars, anchors and aliases at load, so authoring style is invisible by construction — the
// walk sees VALUES, and every mapping node in the document is visited exactly once. There is
// no position list to fall off of, which is what killed the previous two mechanisms.
//
// The rule per reference is the crate's rule, unchanged: exactly `#/components/<section>/<key>`
// with a section OpenAPI 3.1 defines and a key matching [A-Za-z0-9._-]+, else UNRESOLVABLE;
// well-formed with no target in this document, DANGLING. One improvement the published
// document makes possible: every component section is resolved here, not only schemas —
// compose() accepts non-schema pointers on shape alone because fragments cannot see the
// published sections, and this gate can.
//
// The invariant against silent degradation is the FLOORS, same shape as the request-body
// gate's: a walker that visits nothing reports nothing, so seeing fewer refs or mapping
// entries than the floor is itself a failure. Measured 4964 refs and 27 mapping entries
// against the current document; the floors sit under those, not at zero.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import yaml from "js-yaml";

// The component sections OpenAPI 3.1 defines — the version backend/openapi/openapi.yaml
// declares. Closed by the specification: a section a later version adds is rejected until it
// is listed here, which is the fail-closed direction.
const COMPONENT_SECTIONS = new Set([
  "callbacks",
  "examples",
  "headers",
  "links",
  "parameters",
  "pathItems",
  "requestBodies",
  "responses",
  "schemas",
  "securitySchemes",
]);

const COMPONENT_KEY = /^[A-Za-z0-9._-]+$/;
const POINTER = /^#\/components\/([^/]+)\/([^/]+)$/;

export const REF_FLOOR = 4500;
export const MAPPING_FLOOR = 24;

function isPlainObject(node) {
  return typeof node === "object" && node !== null && !Array.isArray(node);
}

/**
 * @param {{ repoRoot: string }} options
 * @returns {{
 *   refs: number,
 *   mappingEntries: number,
 *   findings: { location: string, message: string }[],
 * }}
 */
export function evaluateOpenapiRefs({ repoRoot }) {
  const document = yaml.load(
    readFileSync(join(repoRoot, "backend/openapi/openapi.yaml"), "utf8"),
  );
  // Every read from parsed YAML below is an OWN-property read (Object.hasOwn), never a plain
  // index or `in`: js-yaml returns prototype-ful objects, so `components[section][key] !==
  // undefined` answered `constructor`, `toString` and `__proto__` from Object.prototype and a
  // dangling ref by any such name resolved against the language runtime instead of the
  // document — proven fail-open on 9f5804a8d (critic receipt ann-critic.json). Resolution
  // decided by anything other than document content is the class this gate exists to close.
  const own = (node, key) =>
    isPlainObject(node) && Object.hasOwn(node, key) ? node[key] : undefined;
  const components = isPlainObject(document) ? own(document, "components") ?? {} : {};
  const findings = [];
  let refs = 0;
  let mappingEntries = 0;

  const hasTarget = (section, key) => {
    const entries = own(components, section);
    return isPlainObject(entries) && Object.hasOwn(entries, key);
  };

  // Whole value in the message, never a resolved-looking tail: the prefix is the defect in
  // `common.yaml#/components/schemas/Uuid`, and reporting only the tail is the truncation
  // fail-open wearing an error message.
  const checkPointer = (value, location) => {
    if (typeof value !== "string") {
      findings.push({
        location,
        message: `reference value is not a string (YAML read ${JSON.stringify(value)}) — `
          + "an unquoted `#/...` pointer is a comment and publishes null",
      });
      return;
    }
    const pointer = POINTER.exec(value);
    if (!pointer || !COMPONENT_SECTIONS.has(pointer[1]) || !COMPONENT_KEY.test(pointer[2])) {
      findings.push({
        location,
        message: `unresolvable reference \`${value}\`: must be exactly `
          + "'#/components/<section>/<key>' with a section OpenAPI 3.1 defines and a key "
          + "matching [A-Za-z0-9._-]+",
      });
      return;
    }
    if (!hasTarget(pointer[1], pointer[2])) {
      findings.push({
        location,
        message: `dangling reference \`${value}\`: this document defines no `
          + `components.${pointer[1]}.${pointer[2]}`,
      });
    }
  };

  // Aliases make the object graph shared and possibly cyclic; visiting a node once is both
  // the termination proof and the dedup — one authored node is one subject.
  const seen = new WeakSet();

  const visit = (node, location) => {
    if (Array.isArray(node)) {
      node.forEach((item, index) => visit(item, `${location}/${index}`));
      return;
    }
    if (!isPlainObject(node)) return;
    if (seen.has(node)) return;
    seen.add(node);

    if (Object.hasOwn(node, "$ref")) {
      refs += 1;
      checkPointer(node.$ref, `${location}/$ref`);
    }

    // OpenAPI 3.1 allows discriminator.mapping values in two forms: a pointer, or a bare
    // schema NAME that implies #/components/schemas/<name>. The name form carries no pointer
    // for any scan to find; after parsing it is just a value in a known position. A component
    // key can never contain `/` or `#`, so the two forms cannot be confused.
    const mapping = own(own(node, "discriminator"), "mapping");
    if (isPlainObject(mapping)) {
      for (const [name, value] of Object.entries(mapping)) {
        mappingEntries += 1;
        const at = `${location}/discriminator/mapping/${name}`;
        if (typeof value === "string" && COMPONENT_KEY.test(value)) {
          if (!hasTarget("schemas", value)) {
            findings.push({
              location: at,
              message: `discriminator maps to implicit schema name \`${value}\`, which this `
                + "document does not define — the subtype vanishes from every generated client",
            });
          }
        } else {
          checkPointer(value, at);
        }
      }
    }

    for (const [key, value] of Object.entries(node)) {
      visit(value, `${location}/${key}`);
    }
  };

  visit(document, "#");
  return { refs, mappingEntries, findings };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const repoRoot = process.argv[2] ?? fileURLToPath(new URL("..", import.meta.url));
  let result;
  try {
    result = evaluateOpenapiRefs({ repoRoot });
  } catch (error) {
    console.error(`backend/openapi/openapi.yaml cannot be parsed: ${error.message}`);
    process.exit(1);
  }
  const { refs, mappingEntries, findings } = result;
  for (const finding of findings) console.error(`${finding.location}: ${finding.message}`);
  const belowFloor = refs < REF_FLOOR || mappingEntries < MAPPING_FLOOR;
  if (belowFloor) {
    console.error(`saw ${refs} refs (floor ${REF_FLOOR}) and ${mappingEntries} discriminator `
      + `mapping entries (floor ${MAPPING_FLOOR}) — below the floor, the walker examined less `
      + "of the document than it was built to examine");
  }
  if (findings.length > 0 || belowFloor) {
    console.error(`openapi ref gate FAILED: ${findings.length} finding(s), `
      + `${refs} refs, ${mappingEntries} mapping entries`);
    process.exit(1);
  }
  console.log(`openapi ref gate passed (${refs} refs, ${mappingEntries} mapping entries, `
    + "0 findings)");
}
