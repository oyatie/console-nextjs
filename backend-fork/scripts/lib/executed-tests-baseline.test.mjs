import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  countDeclaredTestAttributes,
  evaluateBaseline,
  evaluateTestAttributeBaseline,
} from "./executed-tests-baseline.mjs";
import { directExecutable, executableWorkflowCommands } from "./ci-workflow-executables.mjs";

const LABEL = "executed-tests-baseline.json";

// Mirrors the committed shape: a derived count plus named buckets carrying the reason.
function baseline(buckets, darkBaseline) {
  const doc = { recorded_on: "2026-07-31", note: "why these are accepted", ...buckets };
  const named = Object.values(buckets).flat().length;
  doc.dark_baseline = darkBaseline ?? named;
  return doc;
}

describe("evaluateBaseline", () => {
  it("passes when the measured dark set equals the named set", () => {
    const doc = baseline({ deferred_fixture_or_defect: ["a/tests/x.rs", "b/tests/y.rs"] });
    const { fatal, advisory } = evaluateBaseline(["a/tests/x.rs", "b/tests/y.rs"], doc, LABEL);
    assert.equal(fatal, null);
    assert.equal(advisory, null);
  });

  it("is order-independent across buckets", () => {
    const doc = baseline({
      deferred_fixture_or_defect: ["b/tests/y.rs"],
      deferred_shared_fixture_not_in_mapped_srcs: ["a/tests/x.rs"],
    });
    assert.equal(evaluateBaseline(["a/tests/x.rs", "b/tests/y.rs"], doc, LABEL).fatal, null);
  });

  it("fails a newly dark file that is not named", () => {
    const doc = baseline({ deferred_fixture_or_defect: ["a/tests/x.rs"] });
    const { fatal } = evaluateBaseline(["a/tests/x.rs", "new/tests/unwired.rs"], doc, LABEL);
    assert.match(fatal, /new\/tests\/unwired\.rs/);
    assert.match(fatal, /execute nowhere and are not named/);
  });

  // THE REGRESSION THIS FILE EXISTS FOR. The previous gate compared only a count, so
  // swapping one dark file for another kept it green while the repository changed which
  // test could not fail. The set must reject the substitution even though the count holds.
  it("fails substitution even when the count is unchanged", () => {
    const doc = baseline({ deferred_fixture_or_defect: ["a/tests/x.rs", "b/tests/y.rs"] });
    const measured = ["a/tests/x.rs", "c/tests/swapped_in.rs"];
    assert.equal(measured.length, doc.dark_baseline, "count is unchanged — a count-only gate sees nothing");

    const { fatal } = evaluateBaseline(measured, doc, LABEL);
    assert.match(fatal, /c\/tests\/swapped_in\.rs/);
  });

  it("fails closed when a named binary starts executing until the baseline is updated", () => {
    const doc = baseline({ deferred_fixture_or_defect: ["a/tests/x.rs", "b/tests/y.rs"] });
    const { fatal, advisory } = evaluateBaseline(["a/tests/x.rs"], doc, LABEL);
    assert.match(fatal, /b\/tests\/y\.rs/);
    assert.match(fatal, /lock the gain in/);
    assert.equal(advisory, null);
  });

  it("rejects a count that disagrees with the named set", () => {
    const doc = baseline({ deferred_fixture_or_defect: ["a/tests/x.rs"] }, 11);
    const { fatal } = evaluateBaseline(["a/tests/x.rs"], doc, LABEL);
    assert.match(fatal, /dark_baseline is 11 but 1 files are named/);
  });

  it("rejects a baseline that names nothing, rather than falling back to a count", () => {
    const { fatal } = evaluateBaseline(["a/tests/x.rs"], { dark_baseline: 1 }, LABEL);
    assert.match(fatal, /names no accepted-dark files/);
  });

  it("ignores unrelated arrays of non-strings", () => {
    const doc = { dark_baseline: 1, deferred_fixture: ["a/tests/x.rs"], counts_by_year: [2025, 2026] };
    assert.equal(evaluateBaseline(["a/tests/x.rs"], doc, LABEL).fatal, null);
  });

  it("does not treat defined feature variants as accepted-dark binaries", () => {
    const doc = {
      dark_baseline: 1,
      deferred_fixture: ["a/tests/x.rs"],
      defined_feature_variants: ["dark/tests/unnamed.rs --features dev-auth"],
    };
    const { fatal } = evaluateBaseline(
      ["a/tests/x.rs", "dark/tests/unnamed.rs --features dev-auth"],
      doc,
      LABEL,
    );
    assert.match(fatal, /dark\/tests\/unnamed\.rs --features dev-auth/);
  });

  it("rejects a malformed deferred bucket rather than ignoring it", () => {
    const doc = { dark_baseline: 1, deferred_fixture: [2025] };
    assert.match(evaluateBaseline([], doc, LABEL).fatal, /malformed accepted-dark bucket/);
  });

  describe("the derived count is mandatory", () => {
    it("rejects a missing dark_baseline", () => {
      const doc = { deferred_fixture: ["a/tests/x.rs"], reviewers: ["dark/tests/unnamed.rs"] };
      const { fatal } = evaluateBaseline(["dark/tests/unnamed.rs"], doc, LABEL);
      assert.match(fatal, /no non-negative integer dark_baseline/);
    });

    it("rejects a non-numeric dark_baseline", () => {
      const doc = { dark_baseline: "11", deferred_fixture: ["a/tests/x.rs"] };
      const { fatal } = evaluateBaseline(["dark/tests/unnamed.rs"], doc, LABEL);
      assert.match(fatal, /no non-negative integer dark_baseline/);
    });
  });
});

describe("countDeclaredTestAttributes", () => {
  it("is explicitly cfg- and ignore-unaware static source evidence", () => {
    const source = `
#[cfg(any())]
#[test]
fn compiled_out() {}

#[ignore = "requires a service"]
#[tokio::test]
async fn ignored_by_default() {}
`;
    assert.equal(countDeclaredTestAttributes(source), 2);
  });
});

describe("evaluateTestAttributeBaseline", () => {
  it("passes only an exact static-attribute baseline", () => {
    assert.deepEqual(
      evaluateTestAttributeBaseline({ "a.rs": 2 }, { "a.rs": 2 }, "baseline.json"),
      { fatal: null },
    );
  });

  it("fails closed on a gain until --update locks it", () => {
    assert.match(
      evaluateTestAttributeBaseline({ "a.rs": 3 }, { "a.rs": 2 }, "baseline.json").fatal,
      /gained declared test attributes.*--update/,
    );
  });

  it("fails closed on a newly reachable source until --update locks it", () => {
    assert.match(
      evaluateTestAttributeBaseline(
        { "a.rs": 2, "new.rs": 1 },
        { "a.rs": 2 },
        "baseline.json",
      ).fatal,
      /gained declared test attributes.*--update/,
    );
  });
});

describe("workflow executable command extraction", () => {
  const workflow = (body, stepFields = "") => `jobs:\n  domain-unit:\n    steps:\n      - name: Test\n${stepFields}        run: |\n${body.split("\n").map((line) => `          ${line}`).join("\n")}\n`;
  const commands = (source) => executableWorkflowCommands(source).map((entry) => ({
    ...entry,
    direct: directExecutable(entry.tokens).tokens,
  }));

  it("admits a direct assignment-prefixed cargo test", () => {
    const [command] = commands(workflow("SQLX_OFFLINE=true cargo test -p console-kernel-core --lib"));
    assert.deepEqual(command.direct.slice(0, 4), ["cargo", "test", "-p", "console-kernel-core"]);
    assert.equal(command.controlFlow, false);
  });

  it("does not truncate a run block at an empty line", () => {
    const extracted = commands(workflow("echo setup\n\ncargo test -p console-kernel-core --lib"));
    assert.deepEqual(extracted.map(({ direct }) => direct), [
      ["echo", "setup"],
      ["cargo", "test", "-p", "console-kernel-core", "--lib"],
    ]);
  });

  it("scans every block-scalar indicator, not only run: |", () => {
    for (const indicator of ["|-", "|+", ">"]) {
      const source = `jobs:\n  domain-unit:\n    steps:\n      - name: Test\n        run: ${indicator}\n          timeout 30 cargo-audit audit --ignore RUSTSEC-2023-0071\n`;
      const [command] = executableWorkflowCommands(source);
      assert.ok(command, `${indicator} body must be scanned`);
      assert.equal(directExecutable(command.tokens).tokens[0], "timeout");
    }
  });

  it("extracts steps when the steps key carries a YAML comment", () => {
    const source = `jobs:\n  domain-unit:\n    steps: # scanner bypass\n      - name: Test\n        run: |\n          timeout 30 cargo-audit audit --ignore RUSTSEC-2023-0071\n`;
    const [command] = executableWorkflowCommands(source);
    assert.ok(command, "steps with a trailing comment must still be extracted");
    assert.equal(directExecutable(command.tokens).tokens[0], "timeout");
  });

  it("preserves commands after a here-document redirection", () => {
    const source = `jobs:\n  domain-unit:\n    steps:\n      - name: Test\n        run: |\n          cat <<'EOF' && timeout 30 cargo-audit audit --ignore RUSTSEC-2023-0071\n          EOF\n`;
    const commands = executableWorkflowCommands(source);
    const timeout = commands.find((command) => directExecutable(command.tokens).tokens[0] === "timeout");
    assert.ok(timeout, "the command after the here-document redirection must be extracted");
  });

  it("does not mistake echo text for an executable cargo test", () => {
    const [command] = commands(workflow("echo SQLX_OFFLINE=true cargo test -p console-kernel-core --lib"));
    assert.equal(command.direct[0], "echo");
  });

  it("drops literal-false and continue-on-error steps", () => {
    assert.deepEqual(commands(workflow("cargo test -p console-kernel-core", "        if: false\n")), []);
    assert.deepEqual(commands(workflow("cargo test -p console-kernel-core", "        if: ${{ false }}\n")), []);
    assert.deepEqual(commands(workflow("cargo test -p console-kernel-core", "        continue-on-error: true\n")), []);
  });

  it("stops at an unconditional exit before command-shaped text", () => {
    const extracted = commands(workflow("exit 0\ncargo test -p console-kernel-core --lib"));
    assert.equal(extracted.length, 1);
    assert.equal(extracted[0].direct[0], "exit");
  });

  it("marks shell masks and disabled errexit as non-gating control flow", () => {
    for (const body of [
      "cargo test -p console-kernel-core --lib || true",
      "false && cargo test -p console-kernel-core --lib",
      "cargo test -p console-kernel-core --lib &",
      "set +e\ncargo test -p console-kernel-core --lib",
    ]) {
      const cargo = commands(workflow(body)).find((entry) => entry.direct[0] === "cargo");
      assert.ok(cargo, body);
      assert.equal(cargo.controlFlow, true, body);
    }
  });
});
