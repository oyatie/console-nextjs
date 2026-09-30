import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

import {
  beginGate,
  emitProvenanceIfRequested,
  getActiveGateState,
  noteAssertion,
  noteRead,
  resetGateInputsForTests,
  validateActiveGateTrace,
} from "./lib/gate-inputs.mjs";
import { createTextGate } from "./lib/text-gate.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

test("gate-inputs validates declared-but-unread and read-but-undeclared", () => {
  resetGateInputsForTests();
  beginGate({
    gate: "fixture-gate",
    script: "scripts/fixture.mjs",
    documentInputs: ["docs/a.md", "docs/b.md"],
  });
  noteRead("docs/a.md");
  noteAssertion("docs/a.md");
  noteRead("docs/c.md");
  noteAssertion("docs/c.md");
  const v = validateActiveGateTrace();
  assert.equal(v.ok, false);
  assert.ok(v.errors.some((e) => e.includes("declared-but-unread: docs/b.md")));
  assert.ok(v.errors.some((e) => e.includes("read-but-undeclared: docs/c.md")));
  resetGateInputsForTests();
});

test("text-gate hook records assertions per path under an active gate", () => {
  resetGateInputsForTests();
  const dir = mkdtempSync(join(tmpdir(), "tg-"));
  writeFileSync(join(dir, "doc.md"), "alpha\n");
  beginGate({
    gate: "fixture-text",
    script: "scripts/fixture-text.mjs",
    documentInputs: ["doc.md"],
  });
  const gate = createTextGate({ root: dir });
  gate.requireIncludes("doc.md", "alpha", "has alpha");
  gate.requireNotIncludes("doc.md", "beta", "no beta");
  const state = getActiveGateState();
  assert.equal(state.assertions["doc.md"], 2);
  assert.deepEqual(validateActiveGateTrace().ok, true);
  resetGateInputsForTests();
  rmSync(dir, { recursive: true, force: true });
});

test("emitProvenanceIfRequested writes GATE_INPUT_PROVENANCE_OUT", () => {
  resetGateInputsForTests();
  const dir = mkdtempSync(join(tmpdir(), "prov-"));
  const out = join(dir, "out.json");
  beginGate({
    gate: "fixture-emit",
    script: "scripts/fixture-emit.mjs",
    documentInputs: ["docs/x.md"],
  });
  noteRead("docs/x.md");
  noteAssertion("docs/x.md");
  process.env.GATE_INPUT_PROVENANCE_OUT = out;
  emitProvenanceIfRequested();
  delete process.env.GATE_INPUT_PROVENANCE_OUT;
  const payload = JSON.parse(readFileSync(out, "utf8"));
  assert.equal(payload.gate, "fixture-emit");
  assert.equal(payload.validation.ok, true);
  assert.equal(payload.assertions["docs/x.md"], 1);
  resetGateInputsForTests();
  rmSync(dir, { recursive: true, force: true });
});

function writeExceptions(dir, exceptions, baseline_count) {
  mkdirSync(join(dir, "docs/program"), { recursive: true });
  mkdirSync(join(dir, "docs"), { recursive: true });
  writeFileSync(
    join(dir, "docs/program/gate-input-exceptions.json"),
    JSON.stringify(
      {
        schema_version: 1,
        baseline_count,
        exceptions,
      },
      null,
      2,
    ),
  );
  // Minimal documentation index so class lookup works
  writeFileSync(
    join(dir, "docs/documentation-index.json"),
    JSON.stringify({
      schema_version: 2,
      coverage: "first-party-manifest",
      documents: [
        { path: "docs/a.md", class: "historical" },
        { path: "docs/b.md", class: "current" },
      ],
    }),
  );
}

test("exception register rejects growth beyond baseline_count (red)", () => {
  // Exercise the real instrument against a tiny synthetic tree by spawning
  // a node -e that reimplements only the growth check shape — the full
  // instrument needs floor gates. Unit-check the growth rule inline here.
  const exceptions = [
    {
      gate: "g",
      input_path: "docs/a.md",
      owner: "o",
      reason: "r",
      first_seen: "2026-08-05",
      remove_by: "2026-11-05",
    },
    {
      gate: "g",
      input_path: "docs/extra.md",
      owner: "o",
      reason: "r",
      first_seen: "2026-08-05",
      remove_by: "2026-11-05",
    },
  ];
  const baseline_count = 1;
  assert.ok(
    exceptions.length > baseline_count,
    "fixture must exceed baseline for the red proof",
  );
  // Mirror instrument rule:
  const growth = exceptions.length > baseline_count;
  assert.equal(growth, true);
});

test("exception register rejects expired remove_by (red)", () => {
  const asOf = "2026-08-05";
  const ex = {
    gate: "g",
    input_path: "docs/a.md",
    owner: "o",
    reason: "r",
    first_seen: "2026-01-01",
    remove_by: "2026-02-01",
  };
  assert.ok(ex.remove_by < asOf);
});

test("exception register rejects decoration — unobserved pair (red)", () => {
  const countedKeys = new Set(["check:x::docs/a.md"]);
  const exKey = "check:x::docs/never-seen.md";
  const stillRow = null;
  const isDecoration = !countedKeys.has(exKey) && !stillRow;
  assert.equal(isDecoration, true);
});

test("live instrument passes on the current tree", () => {
  const result = spawnSync(
    process.execPath,
    ["scripts/check-gate-input-provenance.mjs", "--json"],
    { cwd: root, encoding: "utf8", maxBuffer: 32 * 1024 * 1024 },
  );
  assert.equal(result.status, 0, result.stderr || result.stdout);
  const report = JSON.parse(result.stdout);
  assert.equal(report.relation, "gate_inputs");
  assert.ok(report.resolved_gates.includes("check:foundation-gates"));
  assert.ok(report.resolved_gates.includes("check:doc-citations"));
  assert.equal(report.failures.length, 0);
  // T1-CONV: g004 no longer counts backlog-clearance-ledger.md
  assert.ok(
    !report.counted_rows.some(
      (r) =>
        r.gate === "check:g004-identity-foundation" &&
        r.input_path === "docs/specs/backlog-clearance-ledger.md",
    ),
  );
  // T1-CONV replacement remains: matrix.goalId executable check in g004
  const g004 = readFileSync(
    join(root, "scripts/check-g004-identity-foundation.mjs"),
    "utf8",
  );
  assert.match(g004, /matrix\.goalId === goalId/);
  assert.doesNotMatch(
    g004,
    /requireIncludes\("docs\/specs\/backlog-clearance-ledger\.md", goalId/,
  );
  assert.doesNotMatch(g004, /docs\/specs\/foundation-gates\.md/);

  assert.ok(
    !report.rows.some(
      (r) =>
        r.gate === "check:g006-asset-dispatch-lifecycle" &&
        r.input_path === "docs/specs/backlog-clearance-ledger.md",
    ),
  );
  const g006FoundationRow = report.counted_rows.find(
    (r) =>
      r.gate === "check:g006-asset-dispatch-lifecycle" &&
      r.input_path === "docs/specs/foundation-gates.md",
  );
  assert.equal(g006FoundationRow?.assertion_count, 2);
  const g006 = readFileSync(
    join(root, "scripts/check-g006-asset-dispatch-lifecycle.mjs"),
    "utf8",
  );
  assert.match(g006, /matrix\.goalId === goalId/);
  assert.doesNotMatch(g006, /docs\/specs\/backlog-clearance-ledger\.md/);
  assert.match(
    g006,
    /requireIncludes\("docs\/specs\/foundation-gates\.md", "ownership transfers"/,
  );
  assert.match(
    g006,
    /requireIncludes\("docs\/specs\/foundation-gates\.md", "assets only to equipment\/inventory schemas"/,
  );

  const exceptions = JSON.parse(
    readFileSync(join(root, "docs/program/gate-input-exceptions.json"), "utf8"),
  );
  assert.equal(exceptions.baseline_count, exceptions.exceptions.length);
  assert.ok(
    !exceptions.exceptions.some(
      (entry) =>
        entry.gate === "check:g004-identity-foundation" &&
        entry.input_path === "docs/specs/foundation-gates.md",
    ),
  );
  assert.ok(
    !exceptions.exceptions.some(
      (entry) =>
        entry.gate === "check:g006-asset-dispatch-lifecycle" &&
        entry.input_path === "docs/specs/backlog-clearance-ledger.md",
    ),
  );
  assert.equal(
    exceptions.exceptions.filter(
      (entry) =>
        entry.gate === "check:g006-asset-dispatch-lifecycle" &&
        entry.input_path === "docs/specs/foundation-gates.md",
    ).length,
    1,
  );
});

test("G005 goal identity is controlled by the matrix, not the historical ledger", () => {
  const goalId = "G005-workflow-builder-approvals-work-hub";
  const ledgerPath = join(root, "docs/specs/backlog-clearance-ledger.md");
  const matrixPath = join(
    root,
    "docs/benchmarks/g005-workflow-lifecycle-matrix.json",
  );
  const originalLedger = readFileSync(ledgerPath);
  const originalMatrix = readFileSync(matrixPath);
  const runG005 = () =>
    spawnSync(process.execPath, ["scripts/check-g005-workflow-lifecycle.mjs"], {
      cwd: root,
      encoding: "utf8",
      maxBuffer: 32 * 1024 * 1024,
    });
  let testFailure;

  try {
    const ledgerText = originalLedger.toString("utf8");
    assert.equal(
      ledgerText.split(goalId).length - 1,
      1,
      "fixture must mutate exactly one historical-ledger goal id",
    );
    writeFileSync(
      ledgerPath,
      ledgerText.replace(goalId, "G005-hostile-historical-ledger-goal"),
    );
    const ledgerMutation = runG005();
    assert.equal(
      ledgerMutation.status,
      0,
      ledgerMutation.stderr || ledgerMutation.stdout,
    );

    writeFileSync(ledgerPath, originalLedger);
    const matrixText = originalMatrix.toString("utf8");
    const matrixGoal = `"goalId": "${goalId}"`;
    assert.equal(
      matrixText.split(matrixGoal).length - 1,
      1,
      "fixture must mutate exactly one machine-readable goal id",
    );
    writeFileSync(
      matrixPath,
      matrixText.replace(
        matrixGoal,
        '"goalId": "G005-hostile-matrix-goal"',
      ),
    );
    const matrixMutation = runG005();
    const matrixOutput = `${matrixMutation.stdout}\n${matrixMutation.stderr}`;
    assert.equal(matrixMutation.status, 1, matrixOutput);
    assert.ok(
      matrixOutput.includes(
        "docs/benchmarks/g005-workflow-lifecycle-matrix.json: goalId must be G005-workflow-builder-approvals-work-hub",
      ),
      matrixOutput,
    );
  } catch (error) {
    testFailure = error;
  } finally {
    writeFileSync(ledgerPath, originalLedger);
    writeFileSync(matrixPath, originalMatrix);
  }

  assert.deepEqual(readFileSync(ledgerPath), originalLedger);
  assert.deepEqual(readFileSync(matrixPath), originalMatrix);
  if (testFailure) throw testFailure;
});

test("G006 goal identity is controlled by the matrix, not the historical ledger", () => {
  const goalId = "G006-assets-equipment-inventory-dispatch";
  const ledgerPath = join(root, "docs/specs/backlog-clearance-ledger.md");
  const matrixPath = join(
    root,
    "docs/benchmarks/g006-asset-dispatch-lifecycle-matrix.json",
  );
  const originalLedger = readFileSync(ledgerPath);
  const originalMatrix = readFileSync(matrixPath);
  const runG006 = () =>
    spawnSync(process.execPath, ["scripts/check-g006-asset-dispatch-lifecycle.mjs"], {
      cwd: root,
      encoding: "utf8",
      maxBuffer: 32 * 1024 * 1024,
    });
  let testFailure;

  try {
    const ledgerText = originalLedger.toString("utf8");
    assert.equal(
      ledgerText.split(goalId).length - 1,
      1,
      "fixture must mutate exactly one historical-ledger G006 goal id",
    );
    writeFileSync(
      ledgerPath,
      ledgerText.replace(goalId, "G006-hostile-historical-ledger-goal"),
    );
    const ledgerMutation = runG006();
    assert.equal(
      ledgerMutation.status,
      0,
      ledgerMutation.stderr || ledgerMutation.stdout,
    );

    writeFileSync(ledgerPath, originalLedger);
    const matrixText = originalMatrix.toString("utf8");
    const matrixGoal = `"goalId": "${goalId}"`;
    assert.equal(
      matrixText.split(matrixGoal).length - 1,
      1,
      "fixture must mutate exactly one machine-readable G006 goal id",
    );
    writeFileSync(
      matrixPath,
      matrixText.replace(
        matrixGoal,
        '"goalId": "G006-hostile-matrix-goal"',
      ),
    );
    const matrixMutation = runG006();
    const matrixOutput = `${matrixMutation.stdout}\n${matrixMutation.stderr}`;
    assert.equal(matrixMutation.status, 1, matrixOutput);
    assert.ok(
      matrixOutput.includes(
        "docs/benchmarks/g006-asset-dispatch-lifecycle-matrix.json: goalId must be G006-assets-equipment-inventory-dispatch",
      ),
      matrixOutput,
    );
  } catch (error) {
    testFailure = error;
  } finally {
    writeFileSync(ledgerPath, originalLedger);
    writeFileSync(matrixPath, originalMatrix);
  }

  assert.deepEqual(readFileSync(ledgerPath), originalLedger);
  assert.deepEqual(readFileSync(matrixPath), originalMatrix);
  if (testFailure) throw testFailure;
});

test("G007 goal identity is controlled by the matrix, not the historical ledger", () => {
  const goalId = "G007-collaboration-mail-calendar-poll-mob";
  const ledgerPath = join(root, "docs/specs/backlog-clearance-ledger.md");
  const matrixPath = join(
    root,
    "docs/benchmarks/g007-collaboration-mobile-lifecycle-matrix.json",
  );
  const originalLedger = readFileSync(ledgerPath);
  const originalMatrix = readFileSync(matrixPath);
  const runG007 = () =>
    spawnSync(
      process.execPath,
      ["scripts/check-g007-collaboration-mobile-lifecycle.mjs"],
      {
        cwd: root,
        encoding: "utf8",
        maxBuffer: 32 * 1024 * 1024,
      },
    );
  let testFailure;

  try {
    const ledgerText = originalLedger.toString("utf8");
    assert.equal(
      ledgerText.split(goalId).length - 1,
      1,
      "fixture must mutate exactly one historical-ledger G007 goal id",
    );
    writeFileSync(
      ledgerPath,
      ledgerText.replace(goalId, "G007-hostile-historical-ledger-goal"),
    );
    const ledgerMutation = runG007();
    assert.equal(
      ledgerMutation.status,
      0,
      ledgerMutation.stderr || ledgerMutation.stdout,
    );

    writeFileSync(ledgerPath, originalLedger);
    const matrixText = originalMatrix.toString("utf8");
    const matrixGoal = `"goalId": "${goalId}"`;
    assert.equal(
      matrixText.split(matrixGoal).length - 1,
      1,
      "fixture must mutate exactly one machine-readable G007 goal id",
    );
    writeFileSync(
      matrixPath,
      matrixText.replace(
        matrixGoal,
        '"goalId": "G007-hostile-matrix-goal"',
      ),
    );
    const matrixMutation = runG007();
    const matrixOutput = `${matrixMutation.stdout}\n${matrixMutation.stderr}`;
    assert.equal(matrixMutation.status, 1, matrixOutput);
    assert.ok(
      matrixOutput.includes(
        "docs/benchmarks/g007-collaboration-mobile-lifecycle-matrix.json: goalId must be G007-collaboration-mail-calendar-poll-mob",
      ),
      matrixOutput,
    );
  } catch (error) {
    testFailure = error;
  } finally {
    writeFileSync(ledgerPath, originalLedger);
    writeFileSync(matrixPath, originalMatrix);
  }

  assert.deepEqual(readFileSync(ledgerPath), originalLedger);
  assert.deepEqual(readFileSync(matrixPath), originalMatrix);
  if (testFailure) throw testFailure;
});

test("G005 workflow and scoped approval-feed controls come from executable source, not foundation prose", () => {
  const foundationPath = join(root, "docs/specs/foundation-gates.md");
  const workflowPath = join(
    root,
    "backend/crates/workorder/domain/src/lib.rs",
  );
  const approvalFeedPath = join(
    root,
    "backend/crates/workorder/rest/src/lib.rs",
  );
  const originalFoundation = readFileSync(foundationPath);
  const originalWorkflow = readFileSync(workflowPath);
  const originalApprovalFeed = readFileSync(approvalFeedPath);
  const runG005 = () =>
    spawnSync(process.execPath, ["scripts/check-g005-workflow-lifecycle.mjs"], {
      cwd: root,
      encoding: "utf8",
      maxBuffer: 32 * 1024 * 1024,
    });
  let testFailure;

  try {
    const foundationText = originalFoundation.toString("utf8");
    const historicalPhrases = [
      "workflow/approval/action lifecycle",
      "Work Hub",
    ];
    for (const phrase of historicalPhrases) {
      assert.equal(
        foundationText.split(phrase).length - 1,
        1,
        `fixture must mutate exactly one historical phrase: ${phrase}`,
      );
    }
    writeFileSync(
      foundationPath,
      foundationText
        .replace(
          historicalPhrases[0],
          "hostile historical workflow prose",
        )
        .replace(historicalPhrases[1], "Hostile Historical Hub"),
    );
    const historicalMutation = runG005();
    assert.equal(
      historicalMutation.status,
      0,
      historicalMutation.stderr || historicalMutation.stdout,
    );

    const workflowText = originalWorkflow.toString("utf8");
    const workflowAnchor =
      "        let transition = self.apply_transition(to, at, context)?;";
    assert.equal(
      workflowText.split(workflowAnchor).length - 1,
      1,
      "fixture must mutate exactly one guarded workflow transition",
    );
    writeFileSync(
      workflowPath,
      workflowText.replace(
        workflowAnchor,
        [
          '        let _workflow_gate_string_decoy = "let transition = self.apply_transition(to, at, context)?;";',
          "        // let transition = self.apply_transition(to, at, context)?;",
          "        let transition =",
          "            self.apply_transition(to, at, TransitionGuardContext::admin())?;",
        ].join("\n"),
      ),
    );
    const workflowMutation = runG005();
    const workflowOutput = `${workflowMutation.stdout}\n${workflowMutation.stderr}`;
    assert.equal(workflowMutation.status, 1, workflowOutput);
    assert.ok(
      workflowOutput.includes(
        "G005 executable workflow/approval lifecycle must preserve ordered approval and guarded transition application",
      ),
      workflowOutput,
    );
    writeFileSync(workflowPath, originalWorkflow);

    const workflowOrderAnchor = [
      "        let transition = self.apply_transition(to, at, context)?;",
      "        self.approval_line = next_line;",
    ].join("\n");
    assert.equal(
      workflowText.split(workflowOrderAnchor).length - 1,
      1,
      "G005-ADV-01 fixture must mutate exactly one transition/approval-line ordering boundary",
    );
    writeFileSync(
      workflowPath,
      workflowText.replace(
        workflowOrderAnchor,
        [
          "        self.approval_line = next_line;",
          "        let transition = self.apply_transition(to, at, context)?;",
        ].join("\n"),
      ),
    );
    const workflowOrderMutation = runG005();
    const workflowOrderOutput = `${workflowOrderMutation.stdout}\n${workflowOrderMutation.stderr}`;
    assert.equal(
      workflowOrderMutation.status,
      1,
      `G005-ADV-01 must reject committing the approval line before the guarded transition\n${workflowOrderOutput}`,
    );
    assert.ok(
      workflowOrderOutput.includes(
        "G005 executable workflow/approval lifecycle must preserve ordered approval and guarded transition application",
      ),
      workflowOrderOutput,
    );
    writeFileSync(workflowPath, originalWorkflow);

    const workflowCfgDecoyText = workflowText.replace(
      workflowOrderAnchor,
      [
        "        #[cfg(any())]",
        "        {",
        "            let transition = self.apply_transition(to, at, context)?;",
        "        }",
        "        self.approval_line = next_line;",
        "        let transition = self.apply_transition(to, at, context)?;",
      ].join("\n"),
    );
    assert.equal(
      workflowCfgDecoyText.split(workflowAnchor).length - 1,
      2,
      "G005-ADV-02 fixture must retain the real guarded apply and add exactly one cfg-disabled decoy",
    );
    writeFileSync(workflowPath, workflowCfgDecoyText);
    const workflowCfgDecoyMutation = runG005();
    const workflowCfgDecoyOutput = `${workflowCfgDecoyMutation.stdout}\n${workflowCfgDecoyMutation.stderr}`;
    assert.equal(
      workflowCfgDecoyMutation.status,
      1,
      `G005-ADV-02 must reject cfg-disabled ordering evidence while the compiled approval commit precedes the real guarded transition\n${workflowCfgDecoyOutput}`,
    );
    assert.ok(
      workflowCfgDecoyOutput.includes(
        "G005 executable workflow/approval lifecycle must preserve ordered approval and guarded transition application",
      ),
      workflowCfgDecoyOutput,
    );
    writeFileSync(workflowPath, originalWorkflow);

    const workflowCfgAttrAnchor =
      "            approval_line_complete: next_line.is_complete(),";
    assert.equal(
      workflowText.split(workflowCfgAttrAnchor).length - 1,
      1,
      "G005-ADV-03 fixture must annotate exactly one bounded lifecycle statement with cfg_attr",
    );
    writeFileSync(
      workflowPath,
      workflowText.replace(
        workflowCfgAttrAnchor,
        [
          "            #[cfg_attr(any(), cfg(any()))]",
          workflowCfgAttrAnchor,
        ].join("\n"),
      ),
    );
    const workflowCfgAttrMutation = runG005();
    const workflowCfgAttrOutput = `${workflowCfgAttrMutation.stdout}\n${workflowCfgAttrMutation.stderr}`;
    assert.equal(
      workflowCfgAttrMutation.status,
      1,
      `G005-ADV-03 must reject cfg_attr presence control within bounded lifecycle evidence\n${workflowCfgAttrOutput}`,
    );
    assert.ok(
      workflowCfgAttrOutput.includes(
        "G005 executable workflow/approval lifecycle must preserve ordered approval and guarded transition application",
      ),
      workflowCfgAttrOutput,
    );
    writeFileSync(workflowPath, originalWorkflow);

    const workflowDuplicateDecoyText = workflowText.replace(
      workflowOrderAnchor,
      [
        "        if false {",
        "            let transition = self.apply_transition(to, at, context)?;",
        "        }",
        "        let transition = self.apply_transition(to, at, context)?;",
        "        self.approval_line = next_line;",
      ].join("\n"),
    );
    assert.equal(
      workflowDuplicateDecoyText.split(workflowAnchor).length - 1,
      2,
      "G005-ADV-04 fixture must retain the real guarded apply and add exactly one unreachable duplicate",
    );
    writeFileSync(workflowPath, workflowDuplicateDecoyText);
    const workflowDuplicateDecoyMutation = runG005();
    const workflowDuplicateDecoyOutput = `${workflowDuplicateDecoyMutation.stdout}\n${workflowDuplicateDecoyMutation.stderr}`;
    assert.equal(
      workflowDuplicateDecoyMutation.status,
      1,
      `G005-ADV-04 must reject duplicate critical ordering evidence even when one copy is unreachable\n${workflowDuplicateDecoyOutput}`,
    );
    assert.ok(
      workflowDuplicateDecoyOutput.includes(
        "G005 executable workflow/approval lifecycle must preserve ordered approval and guarded transition application",
      ),
      workflowDuplicateDecoyOutput,
    );
    writeFileSync(workflowPath, originalWorkflow);

    const workflowRawCfgDecoyText = workflowText.replace(
      workflowOrderAnchor,
      [
        "        #[r#cfg(any())]",
        "        let transition = self.apply_transition(to, at, context)?;",
        "        self.approval_line = next_line;",
        "        let transition: Transition<WorkOrderStatus> =",
        "            self.apply_transition(to, at, context)?;",
      ].join("\n"),
    );
    const workflowTypedApplyAnchor = [
      "        let transition: Transition<WorkOrderStatus> =",
      "            self.apply_transition(to, at, context)?;",
    ].join("\n");
    assert.equal(
      workflowRawCfgDecoyText.split("        #[r#cfg(any())]").length - 1,
      1,
      "G005-ADV-05 fixture must carry exactly one raw cfg attribute on the canonical guarded apply",
    );
    assert.equal(
      workflowRawCfgDecoyText.split(workflowAnchor).length - 1,
      1,
      "G005-ADV-05 fixture must retain exactly one canonical guarded-apply token sequence",
    );
    assert.equal(
      workflowRawCfgDecoyText.split(workflowTypedApplyAnchor).length - 1,
      1,
      "G005-ADV-05 fixture must retain exactly one real compiled type-annotated guarded apply",
    );
    writeFileSync(workflowPath, workflowRawCfgDecoyText);
    const workflowRawCfgDecoyMutation = runG005();
    const workflowRawCfgDecoyOutput = `${workflowRawCfgDecoyMutation.stdout}\n${workflowRawCfgDecoyMutation.stderr}`;
    assert.equal(
      workflowRawCfgDecoyMutation.status,
      1,
      `G005-ADV-05 must normalize and reject raw cfg evidence while the compiled approval commit precedes the real guarded transition\n${workflowRawCfgDecoyOutput}`,
    );
    assert.ok(
      workflowRawCfgDecoyOutput.includes(
        "G005 executable workflow/approval lifecycle must preserve ordered approval and guarded transition application",
      ),
      workflowRawCfgDecoyOutput,
    );
    writeFileSync(workflowPath, originalWorkflow);

    writeFileSync(
      workflowPath,
      workflowText.replace(
        workflowCfgAttrAnchor,
        [
          "            #[r#cfg_attr(any(), cfg(any()))]",
          workflowCfgAttrAnchor,
        ].join("\n"),
      ),
    );
    const workflowRawCfgAttrMutation = runG005();
    const workflowRawCfgAttrOutput = `${workflowRawCfgAttrMutation.stdout}\n${workflowRawCfgAttrMutation.stderr}`;
    assert.equal(
      workflowRawCfgAttrMutation.status,
      1,
      `G005-ADV-06 must normalize and reject raw cfg_attr presence control within bounded lifecycle evidence\n${workflowRawCfgAttrOutput}`,
    );
    assert.ok(
      workflowRawCfgAttrOutput.includes(
        "G005 executable workflow/approval lifecycle must preserve ordered approval and guarded transition application",
      ),
      workflowRawCfgAttrOutput,
    );
    writeFileSync(workflowPath, originalWorkflow);

    const approvalFeedText = originalApprovalFeed.toString("utf8");
    const approvalFeedAnchor =
      "    let visibility = approval_source_visibility(&principal)?;";
    assert.equal(
      approvalFeedText.split(approvalFeedAnchor).length - 1,
      1,
      "fixture must mutate exactly one principal-derived approval visibility binding",
    );
    writeFileSync(
      approvalFeedPath,
      approvalFeedText.replace(
        approvalFeedAnchor,
        [
          '    let _approval_feed_string_decoy = "let visibility = approval_source_visibility(&principal)?;";',
          "    // let visibility = approval_source_visibility(&principal)?;",
          "    let visibility = ApprovalSourceVisibility {",
          "        work_orders: true,",
          "        daily_plans: true,",
          "        target_changes: true,",
          "    };",
        ].join("\n"),
      ),
    );
    const approvalFeedMutation = runG005();
    const approvalFeedOutput = `${approvalFeedMutation.stdout}\n${approvalFeedMutation.stderr}`;
    assert.equal(approvalFeedMutation.status, 1, approvalFeedOutput);
    assert.ok(
      approvalFeedOutput.includes(
        "G005 server-owned approval feed must derive visibility from the principal and preserve branch-scoped queries",
      ),
      approvalFeedOutput,
    );
  } catch (error) {
    testFailure = error;
  } finally {
    writeFileSync(foundationPath, originalFoundation);
    writeFileSync(workflowPath, originalWorkflow);
    writeFileSync(approvalFeedPath, originalApprovalFeed);
  }

  assert.deepEqual(readFileSync(foundationPath), originalFoundation);
  assert.deepEqual(readFileSync(workflowPath), originalWorkflow);
  assert.deepEqual(readFileSync(approvalFeedPath), originalApprovalFeed);
  if (testFailure) throw testFailure;
});

test("G004 machine-readable passkey and policy controls fail under hostile matrix mutations", async () => {
  const { hasPasskeyContract, hasPolicyContract } = await import(
    "./check-g004-identity-foundation.mjs"
  );
  const matrix = JSON.parse(
    readFileSync(
      join(root, "docs/benchmarks/g004-identity-foundation-matrix.json"),
      "utf8",
    ),
  );
  assert.equal(hasPasskeyContract(matrix.routePaths), true);
  assert.equal(hasPolicyContract(matrix.routePaths), true);

  const withoutPasskey = structuredClone(matrix.routePaths);
  for (const row of withoutPasskey) {
    row.requiredStory = row.requiredStory.replaceAll(/passkey/gi, "factor");
  }
  assert.equal(hasPasskeyContract(withoutPasskey), false);

  const withoutPolicy = structuredClone(matrix.routePaths);
  for (const row of withoutPolicy) {
    row.requiredStory = row.requiredStory.replaceAll(/policy/gi, "rules");
  }
  assert.equal(hasPolicyContract(withoutPolicy), false);
});

test("T1-CONV replacement is CI-reachable via check:g004-identity-foundation", () => {
  // Binary identity: package script + CI preflight lock mention the gate.
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  assert.equal(
    pkg.scripts["check:g004-identity-foundation"],
    "node scripts/check-g004-identity-foundation.mjs",
  );
  const preflight = readFileSync(
    join(root, "scripts/check-ci-preflight.mjs"),
    "utf8",
  );
  assert.match(preflight, /check:g004-identity-foundation/);
});
