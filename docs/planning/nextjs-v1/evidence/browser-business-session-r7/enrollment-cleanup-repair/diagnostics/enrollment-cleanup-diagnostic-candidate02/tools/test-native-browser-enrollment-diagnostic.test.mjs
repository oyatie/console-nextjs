// Real runner/process fault checks only. No native API or enrollment acceptance.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createHash } from "node:crypto";
import { lstat, mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { superviseEnrollmentFault } from "./enrollment-fault-deadlines.mjs";

const env = Object.fromEntries(
  ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "PLAYWRIGHT_BROWSERS_PATH"]
    .filter((key) => process.env[key] !== undefined)
    .map((key) => [key, process.env[key]]),
);

const startupFaults = ["eof-before-origin", "partial-eof-before-origin", "frame-eof-before-origin"];
const enrollmentFaults = [...startupFaults, "signal-after-origin", "eof-after-ready", "oversized", "wrong-version", "wrong-sequence", "wrong-kind", "legacy-selection", "missing-selection", "malformed-selection", "weak-uv"];
// Isolated exact real producer. Invoke as node DRIVER CANDIDATE_ENTRY from the
// root-owned frozen source cwd. Entry must be a regular copied bundle beneath
// that cwd's .artifacts so the exact installed SDK resolves without a symlink.
// Only cleanup localization is claimed; no authentication/full-suite acceptance.
const ROOT_PINS = {"tools/test-native-browser-enrollment.mjs": "6cbb58a1b2eae9f01d0b4d0838afd7c5c421c496a532fde056a1f33e153027ce", "tools/production-runtime.mjs": "7e4d05bac69900295f8777c931999f2987acd36c947f281cbea7d1bbdf673367", "server.mjs": "1f0f42b227af875f580ea4d11ff3b8355eb24487b17f45d9de99e3324a62f90f", "tools/enrollment-fault-deadlines.mjs": "6f80266162d5413bcda245b17536b991518e95e22f5b1a7001067aaa4120fc06", "tools/test-native-browser-enrollment.test.mjs": "3204b466940c270897814f9100ee8f92fbddfdc937e7b71a06406dacac778ead", "package.json": "1b9a58f08e845d1eefa4f124c345f84a347875593a515f9c9eaaff43f40e77cc", "package-lock.json": "ad526fce2fe800ee7122f39029843a69fd9a335572de82a4a71d9c1a9714bc7b", ".node-version": "73fb1b615e2043a933be1c0895cde4358036acc28d785692509b822aa53c761f", ".next/build-inputs.json": "45001f336b372dd488fd27cce5d6d8e1119c42e9dada4025c766fd0d4558d7c7", "node_modules/@playwright/test/package.json": "5587f932b8979b6889654b60e3c28aeb7ca0abbac00eec8c176c939a16149536", "node_modules/playwright-core/lib/coreBundle.js": "549070af3acabb3efcc4f55bfe6210f9f7c2fcf633cf7eaa59bfe60719969171", "node_modules/playwright-core/types/protocol.d.ts": "0410ed82a9ac63269dc1471ed249b280e2d461249a3a331e71cdb54cdf10db33"};
const BUNDLE_PINS = {"tools/production-runtime.mjs": "7e4d05bac69900295f8777c931999f2987acd36c947f281cbea7d1bbdf673367", "server.mjs": "1f0f42b227af875f580ea4d11ff3b8355eb24487b17f45d9de99e3324a62f90f", "tools/enrollment-fault-deadlines.mjs": "6f80266162d5413bcda245b17536b991518e95e22f5b1a7001067aaa4120fc06", "tools/test-native-browser-enrollment.mjs": "bcd5fb0730bdc268d35dbd220fb8c44bbf35b3219ea1deffba95b3a8a9e6d3f6"};
const DIAGNOSTIC_PREFIX = "ENROLLMENT_CLEANUP_DIAGNOSTIC ";
const STAGES = {"main_stage": ["unstarted", "startup", "owners", "runtime-stage", "ack", "complete"], "browser_stage": ["unstarted", "absent", "close", "kill", "exit-check", "complete"], "next_stage": ["unstarted", "absent", "already-exited", "term", "kill", "complete"]};
const FLAGS = ["startup_settled", "owners_settled", "owners_all_fulfilled", "browser_branch_settled", "browser_close_settled", "browser_close_fulfilled", "browser_close_phase_met", "browser_kill_settled", "browser_kill_fulfilled", "browser_kill_phase_met", "browser_exit_check_passed", "browser_exit_event_seen", "browser_close_event_seen", "next_branch_settled", "next_term_phase_met", "next_kill_phase_met", "next_exit_event_seen", "next_close_event_seen", "runtime_remove_started", "runtime_remove_settled", "runtime_remove_fulfilled", "ack_started", "ack_settled", "ack_fulfilled", "browser_owner_seen", "next_owner_seen", "temporary_scope_seen", "browser_exit_observed", "next_exit_observed"];
async function pinnedFile(root, relative, expected) {
  const file = path.join(root, relative); const info = await lstat(file);
  assert.ok(info.isFile() && !info.isSymbolicLink(), "regular pinned diagnostic input required");
  assert.equal(createHash("sha256").update(await readFile(file)).digest("hex"), expected,
    "diagnostic input bytes changed");
}
async function exactEntry() {
  try {
    assert.equal(process.argv.length, 3);
    const root = path.resolve(); const entry = path.resolve(process.argv[2]);
    assert.ok(entry.startsWith(path.join(root, ".artifacts") + path.sep));
    assert.equal(path.basename(entry), "test-native-browser-enrollment.mjs");
    const bundle = path.dirname(path.dirname(entry));
    assert.equal(path.dirname(entry), path.join(bundle, "tools"));
    assert.equal(path.dirname(fileURLToPath(import.meta.url)), path.join(bundle, "tools"));
    for (const [relative, expected] of Object.entries(ROOT_PINS)) await pinnedFile(root, relative, expected);
    for (const [relative, expected] of Object.entries(BUNDLE_PINS)) await pinnedFile(bundle, relative, expected);
    return entry;
  } catch { throw new Error("approved diagnostic inputs are unavailable or changed"); }
}
function decodeDiagnostic(line) {
  if (!line.startsWith(DIAGNOSTIC_PREFIX)) return null;
  const value = JSON.parse(line.slice(DIAGNOSTIC_PREFIX.length));
  assert.deepEqual(Object.keys(value).sort(), ["version", "failure_trigger", ...Object.keys(STAGES), ...FLAGS].sort());
  assert.equal(value.version, 1);
  assert.ok(["cleanup-rejected", "hard-deadline"].includes(value.failure_trigger));
  for (const [name, allowed] of Object.entries(STAGES)) assert.ok(allowed.includes(value[name]));
  for (const name of FLAGS) assert.equal(typeof value[name], "boolean");
  return value;
}
const cases = [["test-native-browser-enrollment.mjs", "eof-after-ready"]];
for (const [runner, fault] of cases) {
  // 140s prerequisite + 35s fault supervision + 6s final cleanup, with margin.
  const name = runner === "test-browser-business-session.mjs" ? "business" : "enrollment";
  test(`${name} runner confirms cleanup after ${fault}`, { timeout: 190000 }, async () => {
    const entry = await exactEntry();
    const owned = await mkdtemp(path.join(tmpdir(), "enrollment-fault-"));
    const child = spawn(process.execPath, [entry], {
      env: { ...env, TMPDIR: owned }, stdio: ["pipe", "pipe", "pipe"],
    });
    // Drain without retaining error stacks, server output or proof bytes.
    let stderrBytes = 0;
    let lineBytes = []; let lineLength = 0; let decodeFailed = false;
    const diagnostics = []; let ownedTempEmpty = null; let phase = "startup";
    let finalCleanupTermAttempted = false; let finalCleanupKillAttempted = false;
    child.stderr.on("data", (bytes) => {
      stderrBytes += bytes.length;
      for (const byte of bytes) {
        if (byte === 10) {
          // Drain all bytes. Only a bounded transient line is examined; raw
          // stderr and unapproved values never enter the reported observation.
          const line = Buffer.from(lineBytes).toString("utf8");
          if (line.startsWith(DIAGNOSTIC_PREFIX)) {
            try {
              assert.ok(lineLength <= 2048 && diagnostics.length < 2);
              const value = decodeDiagnostic(line); if (value) diagnostics.push(value);
            } catch { decodeFailed = true; }
          }
          lineBytes = []; lineLength = 0;
        } else {
          lineLength += 1; if (lineBytes.length < 2048) lineBytes.push(byte);
        }
      }
    });
    const exited = once(child, "exit");
    const input = child.stdout[Symbol.asyncIterator]();
    let buffered = Buffer.alloc(0);
    let sequence = 0;
    async function read() {
      while (true) {
        if (buffered.length >= 4) {
          const length = buffered.readUInt32BE();
          assert.ok(length > 0 && length <= 65536);
          if (buffered.length >= length + 4) {
            const frame = JSON.parse(buffered.subarray(4, length + 4).toString("utf8"));
            buffered = buffered.subarray(length + 4);
            assert.equal(frame.v, 1);
            assert.equal(frame.seq, sequence++);
            assert.ok(sequence <= 128);
            return frame;
          }
        }
        const next = await input.next();
        assert.ok(!next.done, "runner ended before cleanup acknowledgement");
        assert.ok(buffered.length + next.value.length <= 65540);
        buffered = Buffer.concat([buffered, next.value]);
      }
    }
    function send(frame) {
      const body = Buffer.from(JSON.stringify(frame));
      const header = Buffer.alloc(4);
      header.writeUInt32BE(body.length);
      child.stdin.write(Buffer.concat([header, body]));
    }
    // Parent timeout must request the runner's actual cleanup owner. A forced
    // termination cannot count as confirmed cleanup or a passed fault check.
    const supervision = superviseEnrollmentFault(child);
    try {
      if (startupFaults.includes(fault)) {
        supervision.beginFault();
        if (fault === "partial-eof-before-origin") child.stdin.write(Buffer.from([0, 0]));
        if (fault === "frame-eof-before-origin") send({ v: 1, seq: 0, kind: "done" });
        child.stdin.end();
      } else {
        assert.equal((await read()).kind, "origin"); phase = "origin-received";
      }
      if (fault === "signal-after-origin") {
        supervision.beginFault();
        child.kill("SIGTERM");
      } else if (!startupFaults.includes(fault)) {
        assert.equal((await read()).kind, "ready"); phase = "ready-received";
        supervision.beginFault(); phase = "fault-input";
        if (fault === "eof-after-ready") child.stdin.end();
        else if (fault === "oversized") {
          const header = Buffer.alloc(4);
          header.writeUInt32BE(65537);
          child.stdin.write(header);
        } else if (["legacy-selection", "missing-selection", "malformed-selection", "weak-uv"].includes(fault)) {
          const selection = fault === "legacy-selection"
            ? { residentKey: "discouraged", requireResidentKey: false, userVerification: "required" }
            : { residentKey: "required", requireResidentKey: fault === "malformed-selection" ? "true" : true,
                userVerification: fault === "weak-uv" ? "preferred" : "required" };
          send({ v: 1, seq: 0, kind: "register", actor: "a", mode: "discoverable",
            ceremony: "11111111-1111-4111-8111-111111111111",
            options: { publicKey: { rp: { id: "localhost" },
              ...(fault === "missing-selection" ? {} : { authenticatorSelection: selection }) } } });
        } else send({ v: fault === "wrong-version" ? 2 : 1,
          seq: fault === "wrong-sequence" ? 1 : 0, kind: fault === "wrong-kind" ? "done" : "register", actor: "a", mode: "legacy",
          ceremony: "11111111-1111-4111-8111-111111111111",
          options: { publicKey: { rp: { id: "localhost" } } } });
      }
      let cleaned = await read();
      // A startup ready frame may already have entered the pipe before SIGTERM.
      if (cleaned.kind === "ready" && fault === "signal-after-origin") cleaned = await read();
      assert.equal(cleaned.kind, "cleaned"); phase = "cleanup-ack";
      assert.equal(cleaned.creation_attempts, 0, "invalid contract cannot reach credential creation");
      const aborted = startupFaults.includes(fault) || ["signal-after-origin", "eof-after-ready"].includes(fault);
      assert.equal(cleaned.outcome, aborted ? "aborted" : "failed");
      child.stdin.end();
      const [code, signal] = await exited;
      supervision.clear();
      assert.equal(code, aborted ? 2 : 1);
      assert.equal(signal, null);
      assert.ok(stderrBytes <= 1048576);
      assert.equal(supervision.forced, false, "fallback intervention cannot prove requested cleanup");
      ownedTempEmpty = (await readdir(owned)).length === 0;
      assert.equal(ownedTempEmpty, true, "runner must release its owned temporary resources before parent cleanup");
      assert.equal(decodeFailed, false, "only allowlisted diagnostic metadata is consumed");
      phase = "checked-natural-exit";
    } finally {
      supervision.clear();
      child.stdin.destroy();
      if (child.exitCode === null && child.signalCode === null) {
        finalCleanupTermAttempted = true;
        child.kill("SIGTERM");
        const forced = setTimeout(() => {
          finalCleanupKillAttempted = true;
          child.kill("SIGKILL");
        }, 6000);
        await exited;
        clearTimeout(forced);
      }
      if (ownedTempEmpty === null) {
        try { ownedTempEmpty = (await readdir(owned)).length === 0; } catch {}
      }
      console.log("ENROLLMENT_DIAGNOSTIC_PROBE_OBSERVATION " + JSON.stringify({
        schema: "isolated-real-enrollment-cleanup-probe/2", phase,
        approved_producer_sha256: BUNDLE_PINS["tools/test-native-browser-enrollment.mjs"],
        supervisor_forced: supervision.forced,
        final_cleanup_sigterm_attempted: finalCleanupTermAttempted,
        final_cleanup_sigkill_attempted: finalCleanupKillAttempted,
        owned_tmp_empty_before_parent_removal: ownedTempEmpty,
        diagnostic_decode_failed: decodeFailed, received_cleanup_diagnostics: diagnostics,
        scope: "one actual EOF-after-ready producer; no historical-failure resolution or full acceptance" }));
      await rm(owned, { recursive: true, force: true });
    }
  });
}
