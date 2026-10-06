// Real runner/process fault checks only. No native API or enrollment acceptance.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { superviseEnrollmentFault } from "./enrollment-fault-deadlines.mjs";

const STARTUP_DIAGNOSTIC_PREFIX = "STARTUP_DIAGNOSTIC ";
function parseStartupDiagnostic(line) {
  try {
    if (line.length > 512 || !line.startsWith(STARTUP_DIAGNOSTIC_PREFIX)) return;
    const value = JSON.parse(line.slice(STARTUP_DIAGNOSTIC_PREFIX.length));
    if (Object.keys(value).sort().join(",") !== "error_name,next_exit_code,next_signal,phase,server_bytes") return;
    if (!["runtime-staging", "origin-reservation", "reservation-close", "next-spawn", "next-readiness", "chromium-launch", "chromium-connect", "ready-write", "native-input"].includes(value.phase)) return;
    if (!["Error", "AssertionError", "TimeoutError", "AbortError", "SystemError", "other"].includes(value.error_name)) return;
    if (value.next_exit_code !== null && value.next_exit_code !== "other" && !(Number.isInteger(value.next_exit_code) && value.next_exit_code >= 0 && value.next_exit_code <= 255)) return;
    if (![null, "SIGTERM", "SIGKILL", "other"].includes(value.next_signal)) return;
    if (!(Number.isInteger(value.server_bytes) && value.server_bytes >= 0 && value.server_bytes <= 1048577)) return;
    return value;
  } catch { /* Malformed or untrusted stderr never becomes diagnostic output. */ }
}
// One runnable trust-boundary check; named fault-case accounting stays unchanged.
const diagnosticSample = { phase: "next-readiness", error_name: "AssertionError", next_exit_code: null, next_signal: null, server_bytes: 0 };
const diagnosticOther = { ...diagnosticSample, next_exit_code: "other", next_signal: "other" };
assert.deepEqual([
  diagnosticSample,
  diagnosticOther,
  { ...diagnosticSample, token: "untrusted-sentinel" },
  { ...diagnosticSample, phase: "untrusted-sentinel" },
  { ...diagnosticSample, next_exit_code: -1 },
  { ...diagnosticSample, server_bytes: 1048578 },
  { ...diagnosticSample, next_signal: "SIGSEGV" },
].map((value) => parseStartupDiagnostic(STARTUP_DIAGNOSTIC_PREFIX + JSON.stringify(value))), [diagnosticSample, diagnosticOther, undefined, undefined, undefined, undefined, undefined]);

const env = Object.fromEntries(
  ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "PLAYWRIGHT_BROWSERS_PATH"]
    .filter((key) => process.env[key] !== undefined)
    .map((key) => [key, process.env[key]]),
);

const startupFaults = ["eof-before-origin", "partial-eof-before-origin", "frame-eof-before-origin"];
const enrollmentFaults = [...startupFaults, "signal-after-origin", "eof-after-ready", "oversized", "wrong-version", "wrong-sequence", "wrong-kind", "legacy-selection", "missing-selection", "malformed-selection", "weak-uv"];
const cases = [
  ...enrollmentFaults.map((fault) => ["test-native-browser-enrollment.mjs", fault]),
  ...startupFaults.map((fault) => ["test-browser-business-session.mjs", fault]),
];
for (const [runner, fault] of cases) {
  // 140s prerequisite + 35s fault supervision + 6s final cleanup, with margin.
  const name = runner === "test-browser-business-session.mjs" ? "business" : "enrollment";
  test(`${name} runner confirms cleanup after ${fault}`, { timeout: 190000 }, async () => {
    const owned = await mkdtemp(path.join(tmpdir(), "enrollment-fault-"));
    const child = spawn(process.execPath, [fileURLToPath(new URL(`./${runner}`, import.meta.url))], {
      env: { ...env, TMPDIR: owned }, stdio: ["pipe", "pipe", "pipe"],
    });
    // Drain without retaining error stacks, server output or proof bytes.
    let stderrBytes = 0;
    let diagnostic; let diagnosticLine = ""; let discardDiagnosticLine = false;
    let faultChecksCompleted = false; let cleanupCompleted = false; let finished = false; let diagnosticReported = false;
    function reportStartupDiagnostic() {
      if (!finished || (faultChecksCompleted && cleanupCompleted) || !diagnostic || diagnosticReported) return;
      diagnosticReported = true;
      try { process.stdout.write(STARTUP_DIAGNOSTIC_PREFIX + JSON.stringify({ runner: name, fault, ...diagnostic }) + "\n"); }
      catch { /* Diagnostic output cannot replace the original test failure. */ }
    }
    child.stderr.on("data", (bytes) => {
      stderrBytes += bytes.length;
      if (diagnostic) return;
      for (const character of bytes.toString("utf8")) {
        if (character === "\n") {
          if (!discardDiagnosticLine) diagnostic ??= parseStartupDiagnostic(diagnosticLine);
          diagnosticLine = ""; discardDiagnosticLine = false;
          if (diagnostic) { reportStartupDiagnostic(); break; }
        } else if (!discardDiagnosticLine) {
          if (diagnosticLine.length + character.length > 512) { diagnosticLine = ""; discardDiagnosticLine = true; }
          else {
            diagnosticLine += character;
            if (diagnosticLine.length <= STARTUP_DIAGNOSTIC_PREFIX.length && !STARTUP_DIAGNOSTIC_PREFIX.startsWith(diagnosticLine)) {
              diagnosticLine = ""; discardDiagnosticLine = true;
            }
          }
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
        assert.equal((await read()).kind, "origin");
      }
      if (fault === "signal-after-origin") {
        supervision.beginFault();
        child.kill("SIGTERM");
      } else if (!startupFaults.includes(fault)) {
        assert.equal((await read()).kind, "ready");
        supervision.beginFault();
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
      assert.equal(cleaned.kind, "cleaned");
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
      assert.deepEqual(await readdir(owned), [], "runner must release its owned temporary resources before parent cleanup");
      faultChecksCompleted = true;
    } finally {
      try {
        supervision.clear();
        child.stdin.destroy();
        if (child.exitCode === null && child.signalCode === null) {
          child.kill("SIGTERM");
          const forced = setTimeout(() => child.kill("SIGKILL"), 6000);
          await exited;
          clearTimeout(forced);
        }
        await rm(owned, { recursive: true, force: true });
        cleanupCompleted = true;
      } finally {
        finished = true;
        reportStartupDiagnostic();
      }
    }
  });
}
