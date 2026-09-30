/**
 * Hostile fixtures for check-platform-contract-drift.mjs.
 *
 * The load-bearing case is module-scope resolution: a route source that uses an
 * imported path constant must not bind a different same-named `const` from
 * another file via repo-wide fallback. That fail-open makes OpenAPI agree with
 * the gate while disagreeing with the server.
 */
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

import { checkOpenApiRouteDrift } from "./check-platform-contract-drift.mjs";

const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const cli = fileURLToPath(new URL("./check-platform-contract-drift.mjs", import.meta.url));
const fixtureRoots = [];

after(() => {
  for (const root of fixtureRoots) rmSync(root, { recursive: true, force: true });
});

function fixture(files) {
  const root = mkdtempSync(join(tmpdir(), "platform-contract-drift-"));
  fixtureRoots.push(root);
  for (const [relative, contents] of Object.entries(files)) {
    const absolute = join(root, relative);
    mkdirSync(dirname(absolute), { recursive: true });
    writeFileSync(absolute, contents);
  }
  return root;
}

function openApi(operations) {
  const lines = ["openapi: 3.1.0", "info:", "  title: Fixture", "  version: 0.0.1", "paths:"];
  for (const operation of operations) {
    const [method, path] = operation.split(" ");
    lines.push(`  ${path}:`, `    ${method.toLowerCase()}:`, "      responses:", "        '200':", "          description: ok");
  }
  return `${lines.join("\n")}\n`;
}

describe("module-scope path constant resolution", () => {
  it("fails closed when an imported PATH would bind an unrelated same-named declaration", () => {
    // served.rs: PATH is imported (real value would be /api/real). The gate must
    // not invent /api/unrelated from other.rs's local PATH.
    const root = fixture({
      "served.rs": `use other::PATH;

pub fn router() -> Router {
    Router::new().route(PATH, get(serve_real))
}
`,
      "other.rs": `pub const PATH: &str = "/api/unrelated";

pub fn router() -> Router {
    Router::new().route(PATH, get(serve_unrelated))
}
`,
      "openapi.yaml": openApi(["GET /api/unrelated"]),
    });

    assert.throws(
      () =>
        checkOpenApiRouteDrift({
          openApiPath: join(root, "openapi.yaml"),
          routeSourceFiles: [join(root, "served.rs"), join(root, "other.rs")],
        }),
      (error) => {
        assert.match(String(error.message), /not declared in this module/i);
        assert.match(String(error.message), /refusing repo-wide same-name fallback/i);
        assert.match(String(error.message), /served\.rs/);
        assert.match(String(error.message), /\bPATH\b/);
        return true;
      },
    );
  });

  it("does not report an unrelated same-named path as served under OpenAPI agreement", () => {
    // Same fixture as above: OpenAPI agrees with the wrong PATH. Before the
    // module-scope fix, the gate returned green with GET /api/unrelated harvested
    // from served.rs — OpenAPI∩gate∖server. After the fix it must refuse.
    const root = fixture({
      "served.rs": `use crate::paths::LIST_PATH;

fn router() -> Router {
    Router::new().route(LIST_PATH, get(handler))
}
`,
      "donor.rs": `pub const LIST_PATH: &str = "/api/unrelated";

fn router() -> Router {
    Router::new().route("/api/donor", get(donor))
}
`,
      "openapi.yaml": openApi(["GET /api/unrelated", "GET /api/donor"]),
    });

    let caught;
    try {
      const result = checkOpenApiRouteDrift({
        openApiPath: join(root, "openapi.yaml"),
        routeSourceFiles: [join(root, "served.rs"), join(root, "donor.rs")],
      });
      caught = result;
    } catch (error) {
      caught = error;
    }

    assert.ok(caught instanceof Error, "gate must fail closed, not return a false-green inventory");
    assert.match(String(caught.message), /refusing repo-wide same-name fallback/i);
    assert.doesNotMatch(String(caught.message), /openapi\.yaml is missing/i);
  });

  it("still resolves a path constant declared in the same module", () => {
    // UNDOCUMENTED_BY_DESIGN entries must appear as served or the gate treats
    // them as stale; keep them in the fixture so the local-const case is not
    // masked by exemption bookkeeping.
    const root = fixture({
      "local.rs": `pub const PATH: &str = "/api/local";

fn router() -> Router {
    Router::new()
        .route(PATH, get(handler))
        .route("/api/v1/dev-auth/session", post(dev_auth))
        .route("/api/v1/mail/mox/webhook", post(mox))
}
`,
      "openapi.yaml": openApi(["GET /api/local"]),
    });

    const result = checkOpenApiRouteDrift({
      openApiPath: join(root, "openapi.yaml"),
      routeSourceFiles: [join(root, "local.rs")],
    });
    assert.equal(result.backendOperations.has("GET /api/local"), true);
  });
});

describe("fail-closed inputs", () => {
  it("rejects an unreadable OpenAPI document", () => {
    const root = fixture({
      "routes.rs": `fn router() -> Router {
    Router::new().route("/api/x", get(handler))
}
`,
    });
    assert.throws(
      () =>
        checkOpenApiRouteDrift({
          openApiPath: join(root, "missing-openapi.yaml"),
          routeSourceFiles: [join(root, "routes.rs")],
        }),
      /ENOENT|no such file|missing-openapi/i,
    );
  });

  it("rejects zero /api/ backend operations", () => {
    const root = fixture({
      "routes.rs": `fn router() -> Router {
    Router::new().route("/healthz", get(handler))
}
`,
      "openapi.yaml": openApi(["GET /api/x"]),
    });
    assert.throws(
      () =>
        checkOpenApiRouteDrift({
          openApiPath: join(root, "openapi.yaml"),
          routeSourceFiles: [join(root, "routes.rs")],
        }),
      /examine zero subjects|no \/api\/ operations/i,
    );
  });
});

describe("real tree", () => {
  it("keeps the committed backend inventory sensible", () => {
    const run = spawnSync(process.execPath, [cli], {
      cwd: repoRoot,
      encoding: "utf8",
    });
    assert.equal(run.status, 0, run.stderr || run.stdout);
    assert.match(run.stdout, /OpenAPI route drift gate passed \(\d+ backend \/api\/ operations/);
  });
});
