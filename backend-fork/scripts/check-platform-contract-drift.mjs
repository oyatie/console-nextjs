/**
 * Route/contract drift gate for the whole `/api/` REST surface.
 *
 * SUBJECT: the (METHOD, path) operations the backend registers with axum,
 * harvested from every committed `backend/**\/src/**.rs` that contains a
 * `.route(` call, compared against `backend/openapi/openapi.yaml` in BOTH
 * directions:
 *   - backend serves an operation the contract does not declare, and
 *   - the contract declares an operation the backend does not serve.
 *
 * WHY IT LOOKS LIKE THIS. Three OpenAPI contract suites carried these drift
 * assertions until they were deleted: each read `clients/ts/src/schema.d.ts` at
 * module load, and PIVOT-2026-07-28 removed `clients/` with nothing left to
 * regenerate it, so all three were guaranteed ENOENT. This gate reasserts the
 * same property with no client artifact in the loop — it reads only
 * `openapi.yaml` and Rust router sources.
 *
 * WHAT IT CAN AND CANNOT SEE. The finest distinction available to a source-text
 * harvest is (HTTP method, path template) per file. It CANNOT see whether a
 * router is merged into the production app — a `Router` that is built and never
 * mounted still counts as served — and it cannot evaluate `#[cfg]`, so a router
 * assembled inside a `#[cfg(test)]` module is indistinguishable from a mounted
 * one. Scoping the comparison to `/api/` paths is what keeps that second limit
 * harmless: the in-repo test routers all register non-`/api/` paths.
 * `backend/app/tests/openapi_drift.rs` is the compiled counterpart and remains
 * the authority on which surfaces are actually mounted.
 *
 * FAIL-CLOSED. Examining zero subjects is a failure, not a pass: no discovered
 * route sources, no backend operations, or no contract operations each throw. A
 * `.route(` call whose path argument does not resolve to a literal is an error
 * rather than a silently dropped route, which is what keeps a spelling this
 * lexer does not understand from reading as "no drift". Path *constants* resolve
 * only inside the registering module: a same-named `const` in another route
 * source is never used as a fallback (that fail-open made OpenAPI agree with
 * the gate while disagreeing with the server).
 */
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const defaultOpenApiPath = resolve(root, "backend/openapi/openapi.yaml");
const httpMethods = ["get", "put", "post", "delete", "options", "head", "patch", "trace"];
const httpMethodSet = new Set(httpMethods);
const methodConstructor = new RegExp(`\\b(${httpMethods.join("|")})\\s*\\(`, "g");

/**
 * Operations the backend serves on purpose and the customer contract must not
 * declare. Both already carry the same exemption, for the same reasons, in
 * `backend/app/tests/openapi_drift.rs`. Each entry is checked for staleness
 * below: an exemption for a route the backend no longer serves is an error, so
 * this list cannot quietly outlive its subject.
 */
const UNDOCUMENTED_BY_DESIGN = new Map([
  [
    "POST /api/v1/dev-auth/session",
    "feature-gated `dev-auth`, deliberately absent from the production contract",
  ],
  [
    "POST /api/v1/mail/mox/webhook",
    "provider HMAC webhook, not a customer session route",
  ],
]);

export function checkOpenApiRouteDrift({
  openApiPath = defaultOpenApiPath,
  routeSourceFiles = discoverRouteSourceFiles(),
} = {}) {
  const backendOperations = backendRouteOperations(routeSourceFiles);
  const openApiOperations = openApiApiOperations(readFileSync(openApiPath, "utf8"));

  if (backendOperations.size === 0) {
    throw new Error(
      `parsed ${routeSourceFiles.length} backend route sources but found no /api/ operations: the gate would examine zero subjects`,
    );
  }
  if (openApiOperations.size === 0) {
    throw new Error(`${openApiPath} declares no /api/ operations: the gate would examine zero subjects`);
  }

  const staleExemptions = [...UNDOCUMENTED_BY_DESIGN.keys()]
    .filter((operation) => !backendOperations.has(operation))
    .sort();

  const missing = [...backendOperations]
    .filter((operation) => !openApiOperations.has(operation))
    .filter((operation) => !UNDOCUMENTED_BY_DESIGN.has(operation))
    .sort();
  const unexpected = [...openApiOperations]
    .filter((operation) => !backendOperations.has(operation))
    .sort();

  const sections = [];
  if (missing.length > 0) {
    sections.push(
      [
        "openapi.yaml is missing /api/ operations the backend serves:",
        ...missing.map((operation) => `  - ${operation}`),
      ].join("\n"),
    );
  }
  if (unexpected.length > 0) {
    sections.push(
      [
        "openapi.yaml declares /api/ operations the backend does not serve:",
        ...unexpected.map((operation) => `  - ${operation}`),
      ].join("\n"),
    );
  }
  if (staleExemptions.length > 0) {
    sections.push(
      [
        "UNDOCUMENTED_BY_DESIGN exempts operations the backend no longer serves; delete them:",
        ...staleExemptions.map((operation) => `  - ${operation}`),
      ].join("\n"),
    );
  }
  if (sections.length > 0) {
    throw new Error(sections.join("\n\n"));
  }

  return { backendOperations, openApiOperations, routeSourceFiles };
}

function discoverRouteSourceFiles() {
  return execFileSync("git", ["-C", root, "ls-files", "-z", "--", "backend"], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  })
    .split("\0")
    .filter((file) => file.endsWith(".rs") && file.includes("/src/"))
    .map((file) => resolve(root, file))
    // Discover after stripping comments/raw/char literals. A raw `.includes(".route(")` on
    // the file text admitted modules whose only hit lived in a doc comment; the later
    // strip+parse then saw zero registrations and aborted the gate on an otherwise valid tree.
    // Do not mask ordinary string literals here — route path arguments live in them, and a
    // whole-file string mask over real routers can swallow `.route(` call sites.
    .filter((file) =>
      stripRustCommentsAndLiterals(readFileSync(file, "utf8")).includes(".route("),
    );
}

function backendRouteOperations(routeSourceFiles) {
  const sources = routeSourceFiles.map((path) => ({
    path,
    source: stripRustCommentsAndLiterals(readFileSync(path, "utf8")),
  }));

  // Path constants must resolve inside the module that registers the route.
  // A repo-wide same-name fallback (even for "distinctive" names, and even when
  // the value is unique) binds an imported `PATH` / `LIST_PATH` to an unrelated
  // declaration elsewhere — OpenAPI then agrees with the gate while disagreeing
  // with the server. Names that are not declared among route sources at all
  // (imports from a non-router module) stay unresolved and follow the deferred
  // / routes-table path; they are never satisfied by a coincidental peer file.
  const declaredElsewhere = new Set();
  for (const { source } of sources) {
    for (const name of routePathConstants(source).keys()) {
      declaredElsewhere.add(name);
    }
  }

  const operations = new Set();
  for (const { path: sourcePath, source } of sources) {
    const localConstants = routePathConstants(source);
    const resolvePath = (name) => {
      const local = localConstants.get(name);
      if (local !== undefined) {
        return local;
      }
      if (declaredElsewhere.has(name)) {
        throw new Error(
          `${sourcePath}: route path constant ${name} is not declared in this module; refusing repo-wide same-name fallback`,
        );
      }
      return undefined;
    };

    let registered = 0;
    const record = (routePath, methodExpression) => {
      // Method discovery must not see prose inside string literals. A handler body or
      // comment-turned-string that says `documentation says get() here` would otherwise
      // invent a GET (or any other verb) that the router never registered.
      const methods = new Set(
        [...maskStringLiterals(methodExpression).matchAll(methodConstructor)].map(
          (match) => match[1],
        ),
      );
      if (methods.size === 0) {
        throw new Error(`${sourcePath}: route ${routePath} has no recognized HTTP method`);
      }
      for (const method of methods) {
        const key = operationKey(method, routePath);
        if (routePath.startsWith("/api/")) {
          operations.add(key);
        }
      }
      registered += 1;
    };

    const deferred = [];
    for (const routeCall of extractRouteCalls(source)) {
      const split = splitTopLevelRouteArguments(routeCall);
      if (!split) {
        throw new Error(
          `${sourcePath}: .route(${routeCall.trim()}) has no path/method argument pair`,
        );
      }
      const [pathExpression, methodExpression] = split;
      const routePath = resolveRouteArgument(pathExpression, resolvePath);
      if (routePath === undefined) {
        // The only shape left is a binding fed from a routes table in this file.
        deferred.push(routeCall.trim());
        continue;
      }
      record(routePath, methodExpression);
    }

    // Only consulted when a `.route(` call could not be resolved directly, so a
    // router built the ordinary way is never scanned twice. A deferral the table
    // cannot account for is an error: dropping it would let an unreadable route
    // registration read as "this route is not served".
    if (deferred.length > 0) {
      let fromTable = 0;
      for (const [routePath, methodExpression] of routeTableEntries(source, resolvePath)) {
        record(routePath, methodExpression);
        fromTable += 1;
      }
      if (fromTable === 0) {
        throw new Error(
          `${sourcePath}: unresolved route path argument(s) ${deferred.join(", ")} and no routes table to account for them`,
        );
      }
    }

    if (registered === 0) {
      throw new Error(
        `${sourcePath}: contains .route( but no route registration could be parsed; the gate must not treat an unparsed router as "no routes"`,
      );
    }
  }

  return operations;
}

function resolveRouteArgument(pathExpression, resolvePath) {
  const trimmed = pathExpression.trim();
  const literal = trimmed.match(/^"([^"]*)"$/);
  if (literal) {
    return literal[1];
  }
  if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(trimmed)) {
    return resolvePath(trimmed);
  }
  return undefined;
}

/**
 * `(PATH, get(handler).post(handler))` tuples in a routes table, the one route
 * registration shape that is not a direct `.route(...)` call: `console-todos-rest`
 * folds such a table into the router so `route_paths()` and `router()` cannot
 * disagree. Matching the tuple rather than the fold keeps this to one extra rule.
 */
function routeTableEntries(source, resolvePath) {
  const entries = [];
  const pattern = new RegExp(
    `[([]\\s*(?:"([^"]*)"|([A-Za-z_][A-Za-z0-9_]*))\\s*,\\s*((?:axum::routing::)?(?:${httpMethods.join("|")})\\s*\\()`,
    "g",
  );
  for (const match of source.matchAll(pattern)) {
    const routePath = match[1] ?? resolvePath(match[2]);
    if (routePath === undefined || !routePath.startsWith("/")) {
      continue;
    }
    const methodStart = match.index + match[0].length - match[3].length;
    entries.push([routePath, balancedExpression(source, methodStart)]);
  }
  return entries;
}

function maskStringLiterals(expression) {
  return expression
    .replace(/"(?:\\.|[^"\\])*"/g, '""')
    .replace(/'(?:\\.|[^'\\])*'/g, "''");
}

function balancedExpression(source, start) {
  let depth = 0;
  for (let index = start; index < source.length; index += 1) {
    const char = source[index];
    if (char === "(" || char === "[" || char === "{") {
      depth += 1;
    } else if (char === ")" || char === "]" || char === "}") {
      if (depth === 0) {
        return source.slice(start, index);
      }
      depth -= 1;
    } else if (char === "," && depth === 0) {
      return source.slice(start, index);
    }
  }
  return source.slice(start);
}

function routePathConstants(source) {
  const constants = new Map();
  for (const match of source.matchAll(
    /const\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*&(?:'static\s+)?str\s*=\s*"([^"]*)"/g,
  )) {
    constants.set(match[1], match[2]);
  }
  return constants;
}

/**
 * Removes line comments, nested block comments, raw strings and char literals,
 * replacing each with a space. Without this the lexer is defeated by ordinary
 * Rust: a doc comment in `console-todos-rest` that mentions `.route()` parses as
 * a route call with no arguments, and a `'"'` char literal opens a string that
 * never closes. String literals are preserved because route paths live in them.
 */
function stripRustCommentsAndLiterals(source) {
  let output = "";
  let index = 0;
  while (index < source.length) {
    const char = source[index];
    const next = source[index + 1];

    if (char === "/" && next === "/") {
      while (index < source.length && source[index] !== "\n") index += 1;
      output += " ";
      continue;
    }
    if (char === "/" && next === "*") {
      let depth = 1;
      index += 2;
      while (index < source.length && depth > 0) {
        if (source[index] === "/" && source[index + 1] === "*") {
          depth += 1;
          index += 2;
        } else if (source[index] === "*" && source[index + 1] === "/") {
          depth -= 1;
          index += 2;
        } else {
          index += 1;
        }
      }
      output += " ";
      continue;
    }
    if (char === "r" && (next === '"' || next === "#")) {
      const raw = source.slice(index).match(/^r(#*)"/);
      if (raw) {
        const terminator = `"${raw[1]}`;
        const end = source.indexOf(terminator, index + raw[0].length);
        index = end === -1 ? source.length : end + terminator.length;
        output += " ";
        continue;
      }
    }
    if (char === "'") {
      // A char literal is `'x'` or `'\x'`; anything else starting with `'` is a
      // lifetime, which must pass through untouched.
      const literal = source.slice(index).match(/^'(?:\\.|[^\\'])'/);
      if (literal) {
        index += literal[0].length;
        output += " ";
        continue;
      }
    }
    if (char === '"') {
      output += char;
      index += 1;
      while (index < source.length) {
        const inner = source[index];
        output += inner;
        index += 1;
        if (inner === "\\") {
          if (index < source.length) {
            output += source[index];
            index += 1;
          }
          continue;
        }
        if (inner === '"') break;
      }
      continue;
    }

    output += char;
    index += 1;
  }
  return output;
}

function extractRouteCalls(source) {
  const routeCalls = [];
  let searchFrom = 0;

  while (searchFrom < source.length) {
    const routeStart = source.indexOf(".route(", searchFrom);
    if (routeStart === -1) {
      break;
    }

    const openParen = source.indexOf("(", routeStart);
    let depth = 0;
    let closeParen = -1;
    for (let index = openParen; index < source.length; index += 1) {
      const char = source[index];
      if (char === "(") {
        depth += 1;
      } else if (char === ")") {
        depth -= 1;
        if (depth === 0) {
          closeParen = index;
          break;
        }
      }
    }
    if (closeParen === -1) {
      throw new Error("unterminated .route(...) call in backend route source");
    }

    routeCalls.push(source.slice(openParen + 1, closeParen));
    searchFrom = closeParen + 1;
  }

  return routeCalls;
}

function splitTopLevelRouteArguments(routeCall) {
  let depth = 0;
  for (let index = 0; index < routeCall.length; index += 1) {
    const char = routeCall[index];
    if (char === "(" || char === "[" || char === "{") {
      depth += 1;
    } else if (char === ")" || char === "]" || char === "}") {
      depth -= 1;
    } else if (char === "," && depth === 0) {
      return [routeCall.slice(0, index), routeCall.slice(index + 1)];
    }
  }
  return undefined;
}

function openApiApiOperations(yaml) {
  const operations = new Set();
  let currentPath = null;

  for (const line of yaml.split(/\r?\n/)) {
    const trimmedRight = line.trimEnd();
    // A blank line does not close a path item. Treating it as one silently
    // dropped every operation after the first blank line inside a path.
    if (trimmedRight === "") {
      continue;
    }
    const pathMatch = trimmedRight.match(/^ {2}(\/.+):$/);
    if (pathMatch) {
      currentPath = pathMatch[1];
      continue;
    }
    if (!line.startsWith(" ")) {
      currentPath = null;
      continue;
    }
    if (!currentPath?.startsWith("/api/")) {
      continue;
    }

    const methodMatch = trimmedRight.match(/^ {4}([a-z]+):$/);
    if (methodMatch && httpMethodSet.has(methodMatch[1])) {
      operations.add(operationKey(methodMatch[1], currentPath));
    }
  }

  return operations;
}

function operationKey(method, path) {
  return `${method.toUpperCase()} ${normalizePathParameters(path)}`;
}

function normalizePathParameters(path) {
  return path.replaceAll(/\{[^}/]+\}/g, "{}");
}

function isMainModule() {
  return process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
}

if (isMainModule()) {
  try {
    const openApiPath = process.argv[2] ? resolve(process.argv[2]) : defaultOpenApiPath;
    const { backendOperations, routeSourceFiles } = checkOpenApiRouteDrift({ openApiPath });
    console.log(
      `OpenAPI route drift gate passed (${backendOperations.size} backend /api/ operations across ${routeSourceFiles.length} route sources).`,
    );
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
