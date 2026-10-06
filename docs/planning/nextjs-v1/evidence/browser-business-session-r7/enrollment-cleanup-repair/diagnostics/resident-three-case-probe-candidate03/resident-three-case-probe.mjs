// Artifact-only actual lifecycle probe, prepared for candidate source; no ceremonies/auth proof.
// Three cases exercise real close/EOF/kill paths. No SDK methods or provider results are shadowed.
import { createHash } from "node:crypto";
import { spawn, execFileSync } from "node:child_process";
import { createReadStream } from "node:fs";
import { writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { performance } from "node:perf_hooks";
import { setTimeout as delay } from "node:timers/promises";

const ROOT = "/Users/jasonlee/Developer/frontend/.worktrees/browser-business-session-r7";
const RUNNER = path.join(ROOT, "tools/test-resident-authenticator.mjs");
const OUTPUT = path.join(path.dirname(fileURLToPath(import.meta.url)), "resident-three-case-report01.json");
const MAX_FRAME = 65536;
const MAX_FRAMES = 128;
const CASES = [
  { id: "ready-done", mode: "done", expected_outcome: "completed", expected_exit: 0 },
  { id: "eof-after-ready", mode: "eof", expected_outcome: "aborted", expected_exit: 2 },
  { id: "owned-browser-stop-before-done", mode: "stop-done", expected_outcome: "completed", expected_exit: 0 },
];
const STARTUP_MS = 25000;
const CLEANUP_MS = 15000;
const FORCE_CLEANUP_MS = 6000;
const EXPECTED = {
  "tools/test-resident-authenticator.mjs": "b62d0d6eba89439d4194b79d4dab62ed8fd07614f93bf984bbd3ab261a71799e",
  "package-lock.json": "ad526fce2fe800ee7122f39029843a69fd9a335572de82a4a71d9c1a9714bc7b",
  "backend-fork/backend/app/tests/auth_rest/resident_authenticator.rs": "30345239d9e8321ea9394dd5b5f75d91b5ca4d1b1a98a27c7360951a82179c45",
  "node_modules/@playwright/test/package.json": "5587f932b8979b6889654b60e3c28aeb7ca0abbac00eec8c176c939a16149536",
  "node_modules/playwright-core/package.json": "f061c58427e47e843e26d201f0a57076c734e57c734ecbbd87d4a23b7a20db9b",
  "node_modules/playwright-core/lib/coreBundle.js": "549070af3acabb3efcc4f55bfe6210f9f7c2fcf633cf7eaa59bfe60719969171",
};
// Repeated catchable termination signals cancel work once; they never skip owned cleanup.
let interruptedSignal = null;
const cancellation = Promise.withResolvers();
function interrupt(signal) { interruptedSignal ??= signal; cancellation.resolve(); }
process.on("SIGINT", () => interrupt("SIGINT"));
process.on("SIGTERM", () => interrupt("SIGTERM"));
class Failure extends Error { constructor(reason) { super(reason); this.reason = reason; } }
function require(condition, reason) { if (!condition) throw new Failure(reason); }
function requireActive() { require(interruptedSignal === null, "probe-interrupted"); }
async function hash(file) {
  const digest = createHash("sha256");
  for await (const bytes of createReadStream(file, { highWaterMark: 1048576 })) digest.update(bytes);
  return digest.digest("hex");
}
async function sources() {
  const result = {};
  for (const [relative, expected] of Object.entries(EXPECTED)) {
    const actual = await hash(path.join(ROOT, relative));
    require(actual === expected, "source-drift");
    result[relative] = actual;
  }
  return result;
}
async function bounded(promise, deadline, reason, cleanup = false) {
  if (!cleanup) requireActive();
  const remaining = deadline - performance.now();
  require(remaining > 0, reason);
  let timer;
  try {
    const waiting = [promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Failure(reason)), remaining);
    })];
    if (!cleanup) waiting.push(cancellation.promise.then(() => { throw new Failure("probe-interrupted"); }));
    return await Promise.race(waiting);
  } finally { clearTimeout(timer); }
}
function signalClass(signal) {
  return signal === null || ["SIGTERM", "SIGKILL", "SIGINT", "SIGABRT", "SIGSEGV", "SIGBUS", "SIGILL"].includes(signal)
    ? signal : "other";
}
// ps output is used only transiently for positive ownership and immutable-start
// checks. Neither command output nor executable paths/PIDs enter the report.
function processStamp(pid) {
  try {
    const text = execFileSync("/bin/ps", ["-p", String(pid), "-o", "pid=", "-o", "ppid=", "-o", "pgid=", "-o", "stat=", "-o", "lstart=", "-o", "comm="],
      { encoding: "utf8", timeout: 1000, maxBuffer: 4096, env: { LC_ALL: "C" }, stdio: ["ignore", "pipe", "pipe"] });
    const match = text.match(/^\s*(\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s+([A-Za-z]{3}\s+[A-Za-z]{3}\s+\d+\s+\d{2}:\d{2}:\d{2}\s+\d{4})\s+([^\r\n]+)\s*$/);
    if (!match) return null;
    return { pid: Number(match[1]), parent: Number(match[2]), group: Number(match[3]), state: match[4], started: match[5], executable: match[6].trim() };
  } catch { return null; }
}
function gone(pid) {
  try { process.kill(pid, 0); return false; } catch (error) { return error.code === "ESRCH"; }
}
function sameProcess(before, now) {
  return now && now.pid === before.pid && now.group === before.group
    && now.started === before.started && now.executable === before.executable;
}

async function attempt(index, testCase) {
  requireActive();
  const began = performance.now();
  const stages = {};
  const elapsed = () => Math.round(performance.now() - began);
  let browserPid;
  let browserStamp;
  let protocolFailure;
  let spawnFailed = false;
  let stdoutEnded = false;
  let stdoutBytes = 0;
  let frames = 0;
  let sent = 0;
  let exited = false;
  let exitStatus;
  let closing = false;
  const queue = [];
  let changed = Promise.withResolvers();
  const notify = () => { const previous = changed; changed = Promise.withResolvers(); previous.resolve(); };
  const stderr = { bytes: 0, finished: false, read_failed: false, classes: [], diagnostics: [] };
  const custody = { browser_pid_observed: false, browser_owner_verified: false, browser_absence_observed: false,
    browser_signal_identity_verified: false, browser_signal_identity_unconfirmed: false };
  const interventions = { eof: false, fixture_term: false, fixture_kill: false, browser_term: false, browser_kill: false };
  const faultInjection = { stdin_eof_sent: false, browser_stop_sent: false, browser_stopped_observed: false };
  const environment = Object.fromEntries(["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "PLAYWRIGHT_BROWSERS_PATH"]
    .filter((key) => process.env[key] !== undefined).map((key) => [key, process.env[key]]));
  const child = spawn(process.execPath, [RUNNER], { cwd: ROOT, env: environment, detached: true, stdio: ["pipe", "pipe", "pipe"] });
  stages.spawn_ms = elapsed();
  const closed = Promise.withResolvers();
  child.on("error", () => { spawnFailed = true; protocolFailure ??= "fixture-spawn-failed"; notify(); });
  child.on("exit", (code, signal) => {
    exited = true; exitStatus = { code, signal: signalClass(signal) }; stages.exit_ms = elapsed(); notify();
  });
  child.on("close", () => { stages.stdio_close_ms = elapsed(); closed.resolve(); });
  // A pipe failure is observed, not printed or treated as a successful write.
  child.stdin.on("error", () => { if (!closing) protocolFailure ??= "stdin-write-failed"; notify(); });
  function keys(frame, expected) { return JSON.stringify(Object.keys(frame).sort()) === JSON.stringify(expected.sort()); }
  function frame(frame) {
    require(frame && typeof frame === "object" && !Array.isArray(frame), "frame-json-shape");
    require(frame.v === 1 && frame.seq === frames && frames < MAX_FRAMES, "frame-identity");
    frames += 1;
    if (frame.kind === "ready") {
      require(keys(frame, ["v", "seq", "kind", "origin", "secure_context", "browser_pid"]), "ready-fields");
      require(frame.origin === "https://auth.example.com" && frame.secure_context === true, "ready-security");
      require(Number.isSafeInteger(frame.browser_pid) && frame.browser_pid > 0, "ready-browser-identity");
      require(browserPid === undefined, "duplicate-ready");
      browserPid = frame.browser_pid; custody.browser_pid_observed = true;
      return { kind: "ready" };
    }
    if (frame.kind === "cleaned") {
      require(keys(frame, ["v", "seq", "kind", "outcome", "browser_pid", "browser_exited"]), "cleaned-fields");
      require(["completed", "aborted", "failed"].includes(frame.outcome), "cleaned-outcome-shape");
      return { kind: "cleaned", outcome: frame.outcome, browser_matches: frame.browser_pid === browserPid, browser_exited: frame.browser_exited === true };
    }
    if (frame.kind === "error") {
      require(keys(frame, ["v", "seq", "kind", "code"]) && frame.code === "fixture-failure", "error-frame-shape");
      return { kind: "error", code: "fixture-failure" };
    }
    throw new Failure("unsupported-frame");
  }
  const stdoutTask = (async () => {
    let buffer = Buffer.alloc(0);
    try {
      for await (const chunk of child.stdout) {
        stdoutBytes = Math.min(Number.MAX_SAFE_INTEGER, stdoutBytes + chunk.length);
        if (protocolFailure) continue; // Keep draining even after a rejected frame.
        try {
          require(buffer.length + chunk.length <= MAX_FRAME + 4, "stdout-buffer-limit");
          buffer = Buffer.concat([buffer, chunk]);
          while (buffer.length >= 4) {
            const length = buffer.readUInt32BE();
            require(length > 0 && length <= MAX_FRAME, "frame-size");
            if (buffer.length < length + 4) break;
            const parsed = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(buffer.subarray(4, length + 4)));
            buffer = buffer.subarray(length + 4);
            require(queue.length < MAX_FRAMES, "frame-queue-limit");
            queue.push(frame(parsed)); notify();
          }
        } catch (error) {
          protocolFailure = error instanceof Failure ? error.reason : "frame-json";
          buffer = Buffer.alloc(0); notify();
        }
      }
      if (buffer.length) protocolFailure ??= "truncated-frame";
      stdoutEnded = true;
    } catch { protocolFailure ??= "stdout-read-failed"; }
    finally { notify(); }
  })();
  const stderrTask = (async () => {
    let prefix = [];
    let length = 0;
    function classify() {
      if (!length) return;
      const text = Buffer.from(prefix).toString("utf8");
      let classification = length <= 512 && text === "resident authenticator fixture failed" ? "fixture-failure"
        : text.startsWith("FATAL ERROR:") ? "node-fatal"
        : text.startsWith("Error [ERR_MODULE_NOT_FOUND]") ? "module-not-found"
        : "unrecognized";
      if (length <= 512 && text.startsWith("RESIDENT_CLEANUP_DIAGNOSTIC ")) {
        try {
          const fields = JSON.parse(text.slice("RESIDENT_CLEANUP_DIAGNOSTIC ".length));
          const flags = ["child_exit_seen", "child_close_seen", "close_settled", "kill_settled", "child_exited"];
          require(keys(fields, ["stage", ...flags])
            && ["startup", "browser-identity", "browser-close", "browser-kill", "browser-exit", "ack"].includes(fields.stage)
            && flags.every((key) => typeof fields[key] === "boolean"), "diagnostic-shape");
          require(stderr.diagnostics.length < 4, "diagnostic-count");
          stderr.diagnostics.push({ stage: fields.stage, observed_ms: elapsed(),
            ...Object.fromEntries(flags.map((key) => [key, fields[key]])) });
          classification = "resident-cleanup-diagnostic";
        } catch { classification = "invalid-resident-diagnostic"; }
      }
      if (!stderr.classes.includes(classification)) stderr.classes.push(classification);
      prefix = []; length = 0;
    }
    try {
      for await (const chunk of child.stderr) {
        stderr.bytes = Math.min(Number.MAX_SAFE_INTEGER, stderr.bytes + chunk.length);
        for (const byte of chunk) {
          if (byte === 10) classify();
          else { length = Math.min(Number.MAX_SAFE_INTEGER, length + 1); if (prefix.length < 512) prefix.push(byte); }
        }
      }
      classify(); stderr.finished = true;
    } catch { stderr.read_failed = true; }
  })();
  async function read(deadline) {
    while (true) {
      requireActive();
      require(!spawnFailed && !protocolFailure, protocolFailure ?? "fixture-spawn-failed");
      if (queue.length) return queue.shift();
      require(!stdoutEnded, "stdout-ended-before-frame");
      await bounded(changed.promise, deadline, "frame-deadline");
    }
  }
  function observedBrowserSignal(signal) {
    const now = browserStamp ? processStamp(browserPid) : null;
    if (!browserStamp || !sameProcess(browserStamp, now) || now.group !== browserPid) {
      custody.browser_signal_identity_unconfirmed = true; return;
    }
    custody.browser_signal_identity_verified = true;
    try {
      process.kill(-browserPid, signal);
      interventions[signal === "SIGTERM" ? "browser_term" : "browser_kill"] = true;
    } catch { custody.browser_signal_identity_unconfirmed = true; }
  }
  async function stopOwnedBrowser(deadline) {
    requireActive();
    const before = processStamp(browserPid);
    require(sameProcess(browserStamp, before) && before.parent === child.pid && before.group === browserPid,
      "stop-browser-custody-unconfirmed");
    custody.browser_signal_identity_verified = true;
    try { process.kill(-browserPid, "SIGSTOP"); }
    catch { throw new Failure("stop-browser-signal-failed"); }
    faultInjection.browser_stop_sent = true; stages.browser_stop_ms = elapsed();
    while (performance.now() < deadline) {
      requireActive();
      const now = processStamp(browserPid);
      require(performance.now() < deadline, "stop-browser-observation-deadline");
      require(sameProcess(browserStamp, now) && now.parent === child.pid && now.group === browserPid,
        "stop-browser-identity-changed");
      if (now.state.includes("T")) {
        faultInjection.browser_stopped_observed = true; stages.browser_stopped_ms = elapsed(); return;
      }
      await delay(Math.min(20, Math.max(1, deadline - performance.now())));
    }
    throw new Failure("stop-browser-observation-deadline");
  }
  async function failureCleanup() {
    closing = true;
    const deadline = performance.now() + FORCE_CLEANUP_MS;
    if (!child.stdin.destroyed) { interventions.eof = true; child.stdin.end(); }
    const termAt = deadline - 4000;
    const killAt = deadline - 2000;
    let termed = false; let killed = false;
    while (performance.now() < deadline) {
      if (browserPid && gone(browserPid)) custody.browser_absence_observed = true;
      if ((exited || spawnFailed) && (!browserPid || custody.browser_absence_observed)) break;
      if (!termed && performance.now() >= termAt) {
        termed = true;
        if (!exited && !spawnFailed) { interventions.fixture_term = true; child.kill("SIGTERM"); }
        if (browserPid && !custody.browser_absence_observed) observedBrowserSignal("SIGTERM");
      }
      if (!killed && performance.now() >= killAt) {
        killed = true;
        if (!exited && !spawnFailed) { interventions.fixture_kill = true; child.kill("SIGKILL"); }
        if (browserPid && !custody.browser_absence_observed) observedBrowserSignal("SIGKILL");
      }
      await delay(Math.min(20, Math.max(1, deadline - performance.now())));
    }
    try { await bounded(closed.promise, deadline, "failure-cleanup-close-deadline", true); } catch {}
    try { await bounded(Promise.all([stdoutTask, stderrTask]), deadline, "failure-cleanup-drain-deadline", true); } catch {}
    stages.failure_cleanup_ms = elapsed();
  }
  let reason;
  let acknowledgement;
  try {
    const startupDeadline = began + STARTUP_MS;
    const ready = await read(startupDeadline);
    require(ready.kind === "ready", "ready-not-received"); stages.ready_ms = elapsed();
    browserStamp = processStamp(browserPid);
    require(browserStamp && browserStamp.pid === browserPid && browserStamp.parent === child.pid
      && browserStamp.group === browserPid && /chrome|chromium|headless_shell/i.test(browserStamp.executable), "browser-custody-unconfirmed");
    custody.browser_owner_verified = true; stages.browser_owner_ms = elapsed();
    require(performance.now() < startupDeadline, "startup-deadline");
    if (testCase.mode === "stop-done") await stopOwnedBrowser(startupDeadline);
    requireActive();
    const cleanupDeadline = performance.now() + CLEANUP_MS;
    if (testCase.mode === "eof") {
      closing = true;
      await bounded(new Promise((resolve, reject) => child.stdin.end(
        (error) => error ? reject(new Failure("eof-write-failed")) : resolve())), cleanupDeadline, "eof-write-deadline");
      faultInjection.stdin_eof_sent = true; stages.eof_ms = elapsed();
    } else {
      const body = Buffer.from(JSON.stringify({ v: 1, seq: sent++, kind: "done" }));
      require(body.length > 0 && body.length <= MAX_FRAME && sent <= MAX_FRAMES, "write-frame-bound");
      const header = Buffer.alloc(4); header.writeUInt32BE(body.length);
      await bounded(new Promise((resolve, reject) => child.stdin.write(Buffer.concat([header, body]),
        (error) => error ? reject(new Failure("done-write-failed")) : resolve())), cleanupDeadline, "done-write-deadline");
      stages.done_ms = elapsed();
    }
    acknowledgement = await read(cleanupDeadline); stages.ack_ms = elapsed();
    require(acknowledgement.kind === "cleaned" && acknowledgement.outcome === testCase.expected_outcome
      && acknowledgement.browser_matches && acknowledgement.browser_exited, "cleanup-ack-invalid");
    await bounded(closed.promise, cleanupDeadline, "natural-exit-deadline");
    await bounded(Promise.all([stdoutTask, stderrTask]), cleanupDeadline, "stdio-drain-deadline");
    require(exited && exitStatus.code === testCase.expected_exit && exitStatus.signal === null, "natural-exit-invalid");
    require(stdoutEnded && !protocolFailure && !queue.length, "stdout-drain-unconfirmed");
    require(stderr.finished && !stderr.read_failed, "stderr-unobserved");
    require(stderr.bytes === 0 && stderr.classes.length === 0, "stderr-diagnostic-observed");
    require(gone(browserPid), "browser-still-present");
    custody.browser_absence_observed = true; stages.browser_absent_ms = elapsed();
    requireActive();
  } catch (error) {
    reason = error instanceof Failure ? error.reason : "probe-error";
    await failureCleanup();
  }
  try { await sources(); } catch { reason ??= "source-drift"; }
  if (interruptedSignal) reason ??= "probe-interrupted";
  const forced = Object.entries(interventions).some(([key, value]) => key !== "eof" && value);
  if (forced) reason ??= "forced-intervention";
  if (!stdoutEnded || !stderr.finished || stderr.read_failed) reason ??= "drain-unconfirmed";
  if (browserPid && !custody.browser_absence_observed) reason ??= "browser-custody-unconfirmed";
  return { index, case: testCase.id, expected: { outcome: testCase.expected_outcome, exit_code: testCase.expected_exit }, status: reason ? "failed" : "passed", reason: reason ?? null, stages_ms: stages,
    frames_received: frames, frames_sent: sent, stdout_bytes: stdoutBytes,
    acknowledgement: acknowledgement ? { kind: acknowledgement.kind, outcome: acknowledgement.outcome ?? null,
      browser_matches: acknowledgement.browser_matches ?? false, browser_exited: acknowledgement.browser_exited ?? false } : null,
    exit: exitStatus ?? null, stderr, custody, fault_injection: faultInjection, interventions, forced };
}

const report = { schema: "resident-three-case/1",
  scope: "Actual ready/done, EOF-after-ready and owned real Chromium group SIGSTOP/done lifecycle; no credential ceremony, authentication or native acceptance proof",
  proof_limits: { failed_kill_retained_unref_deadline: "SOURCE_ONLY", historical_exit3_cause: "UNPROVEN",
    close_timeout_kill_path: "Source path plus verified real browser stop and actual cleanup ACK/exit; no SDK call interception",
    uncatchable_probe_termination: "SIGKILL/host failure cannot run in-process cleanup; no success claim" },
  state: "running", command: [process.execPath, fileURLToPath(import.meta.url)], fixture_command: [process.execPath, RUNNER], cwd: ROOT,
  node_version: process.versions.node, cases: CASES.map(({ id }) => id),
  deadlines_ms: { startup: STARTUP_MS, cleanup_ack_and_natural_exit: CLEANUP_MS, failure_cleanup: FORCE_CLEANUP_MS },
  counts: { expected: CASES.length, started: 0, passed: 0, failed: 0, unreached: CASES.length }, attempts: [] };
try {
  require(process.cwd() === ROOT && process.versions.node === "24.21.0", "runtime-prerequisite");
  report.source_hashes = await sources();
  report.node_binary_sha256 = await hash(process.execPath);
  report.probe_sha256 = await hash(fileURLToPath(import.meta.url));
  for (const [index, testCase] of CASES.entries()) {
    requireActive();
    await sources();
    requireActive();
    report.counts.started += 1; report.counts.unreached -= 1;
    const result = await attempt(index + 1, testCase); report.attempts.push(result); report.counts[result.status] += 1;
    if (result.status === "failed") break;
  }
  report.state = report.counts.failed ? "diagnostic_failure" : "bounded_diagnostic_pass";
} catch (error) {
  report.state = "prerequisite_failure";
  report.reason = error instanceof Failure ? error.reason : "probe-prerequisite-error";
}
report.unreached_cases = CASES.slice(report.counts.started).map(({ id }) => id);
report.interruption = interruptedSignal;
if (interruptedSignal) { report.state = "diagnostic_failure"; report.reason ??= "probe-interrupted"; }
await writeFile(OUTPUT, JSON.stringify(report, null, 2) + "\n", { flag: "wx", mode: 0o600 });
process.stdout.write(JSON.stringify({ state: report.state, counts: report.counts, report: OUTPUT }) + "\n");
process.exit(report.state === "bounded_diagnostic_pass" && interruptedSignal === null ? 0 : 1);
