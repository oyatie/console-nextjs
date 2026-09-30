import test from "node:test";
import assert from "node:assert/strict";
import {
  pathIsTipSerial,
  classifyPrFiles,
  assessTipContention,
  TIP_SERIAL_PATH_PREFIXES,
} from "./assess-tip-contention.mjs";

test("pathIsTipSerial matches manifest/baseline/ledger/ci roots", () => {
  // Authority JSON roots (Chesterton's Fence — not the scripts/lib helper prefix).
  assert.equal(pathIsTipSerial("docs/documentation-manifest.seed.json"), true);
  assert.equal(pathIsTipSerial("docs/program/executed-tests-baseline.json"), true);
  assert.equal(pathIsTipSerial("docs/documentation-index.json"), true);
  assert.equal(pathIsTipSerial("docs/program/ledger/2026-08-06.md"), true);
  assert.equal(pathIsTipSerial(".github/workflows/ci.yml"), true);
  assert.equal(pathIsTipSerial(".github/workflows/security.yml"), true);
  assert.equal(pathIsTipSerial(".github/workflows/console-authority-bootstrap.yml"), true);
  assert.equal(pathIsTipSerial(".github/trust/console.allowed_signers"), true);
  assert.equal(pathIsTipSerial(".github/actions/free-runner-disk/action.yml"), true);
  assert.equal(pathIsTipSerial("backend/Cargo.lock"), true);
  assert.equal(pathIsTipSerial("backend/Cargo.toml"), true);
  assert.equal(pathIsTipSerial("backend/.sqlx/query-0000.json"), true);
  assert.equal(pathIsTipSerial("backend/crates/leave/domain/src/lib.rs"), false);
  // scripts/ is a serial root (gate implementations, incl. shell/Python/nested .mjs);
  // the lib/ helper lives under that serial root, so it is also serial.
  assert.equal(pathIsTipSerial("scripts/lib/executed-tests-baseline"), true);
  assert.equal(pathIsTipSerial("scripts/lib/executed-tests-baseline/foo.js"), true);
  assert.ok(TIP_SERIAL_PATH_PREFIXES.includes("docs/documentation-manifest.seed.json"));
  assert.ok(TIP_SERIAL_PATH_PREFIXES.includes("docs/program/executed-tests-baseline.json"));
  assert.ok(TIP_SERIAL_PATH_PREFIXES.length >= 8);
});

test("pathIsTipSerial glob-matches migrations and openapi directories", () => {
  // `**` must match across path segments, not literally.
  assert.equal(
    pathIsTipSerial("backend/crates/platform/db/migrations/0216_x.sql"),
    true,
    "migration SQL under backend/**/migrations/ is tip-serial",
  );
  assert.equal(
    pathIsTipSerial("backend/crates/hr/rest/openapi/schemas/foo.yaml"),
    true,
    "OpenAPI fragment under backend/**/openapi/ is tip-serial",
  );
  // A migration in a nested crate path is also covered (zero or more segments).
  assert.equal(
    pathIsTipSerial("backend/crates/platform/db/migrations/nested/deep/0217_y.sql"),
    true,
  );
  // `*` matches within one segment; it must not cross a slash.
  assert.equal(pathIsTipSerial("backend/crates/platform/db/migration-not/0216.sql"), false);
  assert.equal(
    pathIsTipSerial("backend/crates/hr/rest/openapi-named-file.yaml"),
    false,
    "an OpenAPI-named file outside an openapi/ directory is not tip-serial",
  );
});

test("pathIsTipSerial covers capability registry, generated BUCK faces, and Reindeer lockfiles", () => {
  // Authority registry (the third authority document, previously omitted).
  assert.equal(pathIsTipSerial("docs/program/console-capability-registry.json"), true);
  // Enterprise roadmap (shared collision root declared in the capability registry).
  assert.equal(pathIsTipSerial("docs/program/console-enterprise-roadmap.md"), true);
  // Registered generated BUCK faces (per tools/buck/generated_face_registry.json).
  assert.equal(pathIsTipSerial("backend/crates/payroll/domain/BUCK"), true);
  assert.equal(pathIsTipSerial("backend/app/BUCK"), true);
  assert.equal(pathIsTipSerial("backend/ci/BUCK"), true);
  assert.equal(pathIsTipSerial("third-party/rust/BUCK"), true);
  // A hand-written non-generated file must NOT match the generated-face patterns.
  assert.equal(pathIsTipSerial("backend/crates/payroll/domain/src/lib.rs"), false);
  // Reindeer bootstrap source roots (whole dir + config).
  assert.equal(pathIsTipSerial("third-party/rust/reindeer/Cargo.lock"), true);
  assert.equal(pathIsTipSerial("third-party/rust/reindeer/bootstrap.sh"), true);
  assert.equal(pathIsTipSerial("third-party/rust/reindeer.toml"), true);
  // Always-full CI inputs (per scripts/check-ci-preflight.mjs).
  assert.equal(pathIsTipSerial("release-please-config.json"), true);
  assert.equal(pathIsTipSerial("renovate.json5"), true);
  assert.equal(pathIsTipSerial("backend/deny.toml"), true);
  assert.equal(pathIsTipSerial("backend/rust-toolchain.toml"), true);
  assert.equal(pathIsTipSerial("security/something"), true);
  assert.ok(TIP_SERIAL_PATH_PREFIXES.includes("docs/program/console-capability-registry.json"));
  assert.ok(TIP_SERIAL_PATH_PREFIXES.includes("docs/program/console-enterprise-roadmap.md"));
  assert.ok(TIP_SERIAL_PATH_PREFIXES.includes("**/BUCK"));
  assert.ok(TIP_SERIAL_PATH_PREFIXES.includes("third-party/rust/reindeer.toml"));
  assert.ok(TIP_SERIAL_PATH_PREFIXES.includes("third-party/rust/reindeer/"));
  assert.ok(TIP_SERIAL_PATH_PREFIXES.includes("release-please-config.json"));
  assert.ok(TIP_SERIAL_PATH_PREFIXES.includes("security/"));
});

test("classifyPrFiles marks tip writers", () => {
  const tip = classifyPrFiles([
    "backend/crates/leave/domain/src/lib.rs",
    "docs/program/console-program-ledger.md",
  ]);
  assert.equal(tip.is_tip_writer, true);
  assert.ok(tip.tip_files.includes("docs/program/console-program-ledger.md"));

  const pure = classifyPrFiles(["backend/crates/leave/domain/src/lib.rs"]);
  assert.equal(pure.is_tip_writer, false);
});

test("assessTipContention emits ops.tip-serial-contention when >=2 writers", () => {
  const report = assessTipContention([
    {
      number: 1,
      title: "tip a",
      mergeStateStatus: "CLEAN",
      files: ["docs/documentation-index.json"],
    },
    {
      number: 2,
      title: "tip b",
      mergeStateStatus: "BEHIND",
      files: [".github/workflows/ci.yml"],
    },
    {
      number: 3,
      title: "pure domain",
      mergeStateStatus: "CLEAN",
      files: ["backend/crates/leave/domain/src/lib.rs"],
    },
  ]);
  assert.equal(report.tip_writers, 2);
  assert.ok(report.class_ids.includes("ops.tip-serial-contention"));
  assert.ok(report.class_ids.includes("ops.missed-tip-sync"));
  assert.equal(report.behind_count, 1);
  assert.deepEqual(
    report.writers.map((w) => w.number),
    [1, 2],
  );
});

test("assessTipContention clean when zero or one tip writer", () => {
  const zero = assessTipContention([
    {
      number: 9,
      title: "domain",
      mergeStateStatus: "CLEAN",
      files: ["backend/crates/policy/domain/src/lib.rs"],
    },
  ]);
  assert.equal(zero.tip_writers, 0);
  assert.equal(zero.class_ids.includes("ops.tip-serial-contention"), false);

  const one = assessTipContention([
    {
      number: 10,
      title: "tip",
      mergeStateStatus: "CLEAN",
      files: ["package-lock.json"],
    },
  ]);
  assert.equal(one.tip_writers, 1);
  assert.equal(one.class_ids.includes("ops.tip-serial-contention"), false);
});
