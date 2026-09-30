import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { resolveDarkSuites } from "./check-mjs-dark-suites.mjs";

test("resolveDarkSuites classifies wired vs dark", () => {
  const dir = mkdtempSync(join(tmpdir(), "dark-suite-"));
  mkdirSync(join(dir, "scripts"), { recursive: true });
  mkdirSync(join(dir, ".github/workflows"), { recursive: true });
  mkdirSync(join(dir, "tools/ci"), { recursive: true });

  writeFileSync(
    join(dir, "package.json"),
    JSON.stringify({
      scripts: {
        "test:wired": "node --test scripts/wired.test.mjs",
        "test:orphan": "node --test scripts/orphan.test.mjs",
        "test:ci-tools": "node --test tools/ci/local.test.mjs",
      },
    }),
  );
  writeFileSync(
    join(dir, ".github/workflows/ci.yml"),
    "jobs:\n  t:\n    steps:\n      - run: npm run test:wired\n",
  );
  writeFileSync(join(dir, "scripts/wired.test.mjs"), "import test from 'node:test';\n");
  writeFileSync(join(dir, "scripts/orphan.test.mjs"), "import test from 'node:test';\n");
  writeFileSync(join(dir, "scripts/nowhere.test.mjs"), "import test from 'node:test';\n");
  writeFileSync(join(dir, "tools/ci/local.test.mjs"), "import test from 'node:test';\n");

  const report = resolveDarkSuites(dir);
  assert.equal(report.class_id, "js-test-reachability.dark-suite");
  assert.ok(report.wired.includes("scripts/wired.test.mjs"));
  assert.ok(report.dark.some((d) => d.suite === "scripts/nowhere.test.mjs" && d.npm_script === null));
  assert.ok(report.dark.some((d) => d.suite === "scripts/orphan.test.mjs" && d.npm_script === "test:orphan"));
  // tools/ci suite with npm owner is still dark (not CI-invoked) but not orphan
  assert.ok(report.dark.some((d) => d.suite === "tools/ci/local.test.mjs" && d.npm_script === "test:ci-tools"));
  assert.equal(report.tools_ci_orphan.length, 0);
});
