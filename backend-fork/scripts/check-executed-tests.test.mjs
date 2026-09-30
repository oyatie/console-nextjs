/**
 * Hostile fixtures for process.doc-comment-cfg-test-false-dark.
 *
 * RED shape (pre-fix): `text.includes("#[cfg(test)]")` treats a doc-comment-only
 * paste as a live unit-test binary. GREEN shape: comment-/literal-aware scan.
 */
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import {
  hasLiveCfgTestAttribute,
  unitTestedCrateSrcRoots,
} from "./check-executed-tests-cfg.mjs";
import { cargoTestKind } from "./lib/cargo-test-kind.mjs";

const gateSource = readFileSync(
  fileURLToPath(new URL("./check-executed-tests.mjs", import.meta.url)),
  "utf8",
);

const DOC_ONLY = `/// Mask \`#[cfg(test)]\` / \`#[cfg(all(test, …))]\` modules so inline fixture SQL
/// under \`app/src/**\` does not false-positive as unaudited handlers.
pub fn compute_test_mask() {}
`;

const LINE_COMMENT_ONLY = `// #[cfg(test)]
pub fn not_a_test_module() {}
`;

const BLOCK_COMMENT_ONLY = `/* #[cfg(test)] */
pub fn not_a_test_module() {}
`;

const STRING_ONLY = `pub const PROSE: &str = "#[cfg(test)]";
`;

const LIVE = `#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {}
}
`;

const LIVE_IN_SIBLING_STYLE = `pub fn production() {}

#[cfg(test)]
mod tests {
    #[test]
    fn unit() { assert_eq!(1, 1); }
}
`;

describe("hasLiveCfgTestAttribute — false-dark controls", () => {
  it("documents the RED substring defect the durable fix replaces", () => {
    // Pre-fix oracle: raw includes lights up on doc-comment prose alone.
    assert.equal(DOC_ONLY.includes("#[cfg(test)]"), true);
    assert.equal(LINE_COMMENT_ONLY.includes("#[cfg(test)]"), true);
    assert.equal(BLOCK_COMMENT_ONLY.includes("#[cfg(test)]"), true);
    assert.equal(STRING_ONLY.includes("#[cfg(test)]"), true);
  });

  it("does not treat doc-comment-only #[cfg(test)] as live", () => {
    assert.equal(hasLiveCfgTestAttribute(DOC_ONLY), false);
  });

  it("does not treat line- or block-comment-only #[cfg(test)] as live", () => {
    assert.equal(hasLiveCfgTestAttribute(LINE_COMMENT_ONLY), false);
    assert.equal(hasLiveCfgTestAttribute(BLOCK_COMMENT_ONLY), false);
  });

  it("does not treat string-literal-only #[cfg(test)] as live", () => {
    assert.equal(hasLiveCfgTestAttribute(STRING_ONLY), false);
  });

  it("still detects a real #[cfg(test)] attribute (fails closed on live tests)", () => {
    assert.equal(hasLiveCfgTestAttribute(LIVE), true);
    assert.equal(hasLiveCfgTestAttribute(LIVE_IN_SIBLING_STYLE), true);
  });

  it("still detects live attribute when a doc comment also mentions it", () => {
    const mixed = `${DOC_ONLY}\n${LIVE}`;
    assert.equal(hasLiveCfgTestAttribute(mixed), true);
  });
});

describe("unitTestedCrateSrcRoots — inventory + examined-zero", () => {
  it("excludes crates whose only hit is a doc comment", () => {
    const roots = unitTestedCrateSrcRoots([
      ["backend/ci/gates/audit-coverage/src/lib.rs", DOC_ONLY],
    ]);
    assert.equal(roots.size, 0);
  });

  it("includes crates with a live #[cfg(test)] under src/", () => {
    const roots = unitTestedCrateSrcRoots([
      ["backend/crates/example/src/lib.rs", "pub fn ok() {}"],
      ["backend/crates/example/src/foo.rs", LIVE_IN_SIBLING_STYLE],
    ]);
    assert.deepEqual([...roots], ["backend/crates/example/src"]);
  });

  it("fails closed when examined src inventory is empty", () => {
    assert.throws(
      () => unitTestedCrateSrcRoots([]),
      /examined zero backend\/\*\*\/src\/\*\*\/\*\.rs files/,
    );
    assert.throws(
      () => unitTestedCrateSrcRoots([["backend/crates/example/tests/it.rs", LIVE]]),
      /examined zero/,
    );
  });
});

describe("cargoTestKind — cargo metadata lib aliases", () => {
  it("maps default lib and explicit rlib/cdylib to cargo test --lib", () => {
    assert.equal(cargoTestKind(["lib"]), "lib");
    assert.equal(cargoTestKind(["rlib"]), "lib");
    assert.equal(cargoTestKind(["rlib", "cdylib"]), "lib");
    assert.equal(cargoTestKind(["cdylib"]), "lib");
    assert.equal(cargoTestKind(["proc-macro"]), "lib");
  });

  it("keeps integration tests and drops bins", () => {
    assert.equal(cargoTestKind(["test"]), "test");
    assert.equal(cargoTestKind(["bin"]), null);
    assert.equal(cargoTestKind(["custom-build"]), null);
  });
});

describe("gate wiring", () => {
  it("routes definedBinaries through the comment-aware helper, not raw includes", () => {
    assert.match(gateSource, /unitTestedCrateSrcRoots\(files\)/);
    assert.match(gateSource, /from "\.\/check-executed-tests-cfg\.mjs"/);
    assert.match(gateSource, /cargoTestKind\(target\.kind\)/);
    // Live predicate must not be a raw file-text includes call (comment prose may still
    // describe that historical defect without reintroducing it as code).
    const codeLines = gateSource
      .split("\n")
      .filter((line) => !/^\s*\/\//.test(line) && !/^\s*\*/.test(line));
    assert.equal(
      codeLines.some((line) => line.includes('includes("#[cfg(test)]")')),
      false,
    );
  });
});
