// Native prerequisite only. Rust keeps OTPs, tokens, keys and the disposable DSN.
// No credential injection or changes to server-issued creation options.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { writeSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { chromium } from "@playwright/test";

const MAX_FRAME = 65536;
const MAX_FRAMES = 128;
const PHASE_MS = 20000;
// This probe needs only a genuine production Next origin. Its native not-found
// document needs no sales backend; a storefront failure cannot be called ready.
const DOCUMENT_PATH = "/_not-found/";
const cleanEnv = Object.fromEntries(
  ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "PLAYWRIGHT_BROWSERS_PATH"]
    .filter((key) => process.env[key] !== undefined)
    .map((key) => [key, process.env[key]]),
);
const input = process.stdin[Symbol.asyncIterator]();
let buffered = Buffer.alloc(0);
let inputEnded = false;
let inputChanged = Promise.withResolvers();
let incoming = 0;
let outgoing = 0;
async function readFrame(waitMs = PHASE_MS) {
  assert.ok(++incoming <= MAX_FRAMES);
  const deadline = Date.now() + waitMs;
  while (true) {
    assert.ok(!inputEnded && !closing);
    if (buffered.length >= 4) {
      const length = buffered.readUInt32BE();
      assert.ok(length > 0 && length <= MAX_FRAME);
      if (buffered.length >= 4 + length) {
        const frame = JSON.parse(buffered.subarray(4, 4 + length).toString("utf8"));
        buffered = buffered.subarray(4 + length);
        assert.equal(frame.v, 1);
        assert.equal(frame.seq, incoming - 1);
        return frame;
      }
    }
    const remaining = deadline - Date.now();
    assert.ok(remaining > 0);
    let timer;
    await Promise.race([
      inputChanged.promise,
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("IPC deadline")), remaining); }),
    ]).finally(() => clearTimeout(timer));
  }
}
async function writeFrame(fields) {
  assert.ok(outgoing < MAX_FRAMES);
  const body = Buffer.from(JSON.stringify({ v: 1, seq: outgoing++, ...fields }));
  assert.ok(body.length > 0 && body.length <= MAX_FRAME);
  const header = Buffer.alloc(4);
  header.writeUInt32BE(body.length);
  await new Promise((resolve, reject) => process.stdout.write(
    Buffer.concat([header, body]), (error) => error ? reject(error) : resolve(),
  ));
}

let stage;
let reservation;
let next;
let browserServer;
let browser;
let serverBytes = 0;
let stopPromise;
let closing = false;
let finalization;
let startup = Promise.resolve();
let creationAttempts = 0;
const runtimeStagingAbort = new AbortController();
const mainObservation = {
  main_phase: "unstarted", main_error_type: "none", next_readiness_status: "unstarted",
  next_probe_result: "unobserved", next_probe_error_type: "none",
  next_readiness_budget_elapsed: false, next_startup_error_seen: false,
  next_startup_exit_seen: false, next_startup_close_seen: false,
  ready_write_started: false, ready_write_settled: false, ready_write_fulfilled: false,
};
function observedErrorType(error) {
  // Map only fixed names; never retain or emit error text or arbitrary names.
  try {
    if (error?.name === "AssertionError") return "assertion";
    if (error?.name === "TypeError") return "type-error";
    if (error?.name === "TimeoutError") return "timeout";
    if (error?.name === "AbortError") return "abort";
    if (error?.name === "Error") return "error";
  } catch {}
  return "other";
}
function mainFailureDiagnostic(error) {
  try {
    mainObservation.main_error_type = observedErrorType(error);
    const fields = { version: 1, ...mainObservation,
      next_owner_seen: Boolean(next),
      next_exit_observed: Boolean(next && (next.exitCode !== null || next.signalCode !== null)) };
    writeSync(2, `ENROLLMENT_MAIN_FAILURE_DIAGNOSTIC ${JSON.stringify(fields)}\n`);
  } catch {}
}
const cleanupObservation = {
  main_stage: "unstarted", browser_stage: "unstarted", next_stage: "unstarted",
  browser_owner_status: "unstarted", next_owner_status: "unstarted",
  reservation_owner_status: "unstarted", runtime_owner_status: "unstarted", ack_owner_status: "unstarted",
  first_observed_rejected_owner: "none", first_observed_rejected_stage: "none", selected_failure_owner: "none",
  startup_settled: false, owners_settled: false, owners_all_fulfilled: false,
  browser_branch_settled: false, browser_close_settled: false, browser_close_fulfilled: false,
  browser_close_phase_met: false, browser_kill_settled: false, browser_kill_fulfilled: false,
  browser_kill_phase_met: false, browser_close_check_passed: false, browser_exit_check_passed: false,
  browser_shared_cleanup_completed: false, browser_exit_event_seen: false, browser_close_event_seen: false,
  next_branch_settled: false, next_term_phase_met: false, next_kill_phase_met: false,
  next_exit_event_seen: false, next_close_event_seen: false,
  runtime_safe_to_remove: false, runtime_remove_started: false, runtime_remove_settled: false,
  runtime_remove_fulfilled: false, ack_started: false, ack_settled: false, ack_fulfilled: false,
};
let cleanupStartedAt;
function observedCleanupRejection(owner, ownerStage) {
  // This is the first observer callback, not a claim of causal ordering between
  // concurrent owners. The selected failure separately follows stop's result order.
  if (cleanupObservation.first_observed_rejected_owner === "none") {
    cleanupObservation.first_observed_rejected_owner = owner;
    cleanupObservation.first_observed_rejected_stage = ownerStage;
  }
}
function cleanupDiagnostic(failureTrigger) {
  // Failure-only fixed metadata. Getter/logging failure cannot replace cleanup's
  // original result. No error, path, ID, credential or native payload is emitted.
  try {
    const child = browserServer?.process();
    const elapsed = Math.max(0, Math.floor(performance.now() - cleanupStartedAt));
    const fields = { version: 2, failure_trigger: failureTrigger, ...cleanupObservation, ...mainObservation,
      browser_owner_seen: Boolean(browserServer), next_owner_seen: Boolean(next),
      temporary_scope_seen: Boolean(stage),
      browser_exit_observed: Boolean(child && (child.exitCode !== null || child.signalCode !== null)),
      next_exit_observed: Boolean(next && (next.exitCode !== null || next.signalCode !== null)),
      cleanup_elapsed_ms: Math.min(elapsed, 600000), cleanup_elapsed_capped: elapsed > 600000,
      cleanup_budget_ms: 6000, cleanup_budget_elapsed: elapsed >= 6000 };
    writeSync(2, `ENROLLMENT_CLEANUP_DIAGNOSTIC ${JSON.stringify(fields)}\n`);
  } catch {}
}
async function stop() {
  stopPromise ??= (async () => {
    // An EOF can race an asynchronous launch. Wait for its returned owner before
    // acknowledging cleanup; closing prevents starting another resource.
    cleanupObservation.main_stage = "startup";
    runtimeStagingAbort.abort();
    await startup.catch(() => {});
    cleanupObservation.startup_settled = true;
    // Playwright launches a separate browser process group. Close its owner,
    // rather than assume that terminating this Node process reaps Chromium.
    cleanupObservation.main_stage = "owners";
    cleanupObservation.browser_owner_status = "pending";
    cleanupObservation.next_owner_status = "pending";
    cleanupObservation.reservation_owner_status = "pending";
    const results = await Promise.allSettled([
      (async () => {
        if (!browserServer) { cleanupObservation.browser_stage = "absent"; return; }
        cleanupObservation.browser_stage = "identity";
        const child = browserServer.process();
        let childCloseSeen = false;
        child.once("exit", () => { cleanupObservation.browser_exit_event_seen = true; });
        child.once("close", () => { childCloseSeen = true; cleanupObservation.browser_close_event_seen = true; });
        cleanupObservation.browser_stage = "close";
        const closed = await Promise.race([
          browserServer.close().then(() => {
            cleanupObservation.browser_close_settled = true;
            cleanupObservation.browser_close_fulfilled = true; return true;
          }, () => { cleanupObservation.browser_close_settled = true; return false; }),
          delay(2000).then(() => false),
        ]);
        cleanupObservation.browser_close_phase_met = closed;
        if (!closed) {
          cleanupObservation.browser_stage = "kill";
          const killing = browserServer.kill().then(() => {
            cleanupObservation.browser_kill_settled = true;
            cleanupObservation.browser_kill_fulfilled = true; return true;
          }, (error) => { cleanupObservation.browser_kill_settled = true; throw error; });
          const killed = await Promise.race([killing, delay(2000).then(() => false)]);
          cleanupObservation.browser_kill_phase_met = killed;
          if (!killed) {
            // close/kill share the SDK's process and artifact-cleanup owner.
            // Its pending cleanup retains finalize's original 6s hard bound.
            cleanupObservation.browser_stage = "close-check";
            cleanupObservation.browser_close_check_passed = childCloseSeen;
            assert.equal(childCloseSeen, true);
            cleanupObservation.browser_stage = "exit-check";
            cleanupObservation.browser_exit_check_passed = child.exitCode !== null || child.signalCode !== null;
            assert.ok(child.exitCode !== null || child.signalCode !== null);
            cleanupObservation.browser_stage = "sdk-cleanup";
            assert.equal(await killing, true);
            cleanupObservation.browser_shared_cleanup_completed = true;
          }
        }
        cleanupObservation.browser_stage = "exit-check";
        cleanupObservation.browser_exit_check_passed = child.exitCode !== null || child.signalCode !== null;
        assert.ok(child.exitCode !== null || child.signalCode !== null);
        cleanupObservation.browser_stage = "complete";
      })().then((value) => {
        cleanupObservation.browser_branch_settled = true;
        cleanupObservation.browser_owner_status = "fulfilled"; return value;
      }, (error) => {
        cleanupObservation.browser_branch_settled = true;
        cleanupObservation.browser_owner_status = "rejected";
        observedCleanupRejection("browser", cleanupObservation.browser_stage); throw error;
      }),
      (async () => {
        if (!next) { cleanupObservation.next_stage = "absent"; return; }
        if (next.exitCode !== null || next.signalCode !== null) {
          cleanupObservation.next_stage = "already-exited"; return;
        }
        next.once("close", () => { cleanupObservation.next_close_event_seen = true; });
        const exited = once(next, "exit").then((value) => {
          cleanupObservation.next_exit_event_seen = true; return value;
        });
        cleanupObservation.next_stage = "term";
        next.kill("SIGTERM");
        const closed = await Promise.race([exited.then(() => true), delay(2000).then(() => false)]);
        cleanupObservation.next_term_phase_met = closed;
        if (!closed) {
          cleanupObservation.next_stage = "kill";
          next.kill("SIGKILL");
          const killed = await Promise.race([exited.then(() => true), delay(2000).then(() => false)]);
          cleanupObservation.next_kill_phase_met = killed;
          assert.ok(killed);
        }
        cleanupObservation.next_stage = "complete";
      })().then((value) => {
        cleanupObservation.next_branch_settled = true;
        cleanupObservation.next_owner_status = "fulfilled"; return value;
      }, (error) => {
        cleanupObservation.next_branch_settled = true;
        cleanupObservation.next_owner_status = "rejected";
        observedCleanupRejection("next", cleanupObservation.next_stage); throw error;
      }),
      (reservation?.listening ? new Promise((resolve) => reservation.close(resolve)) : Promise.resolve()).then((value) => {
        cleanupObservation.reservation_owner_status = "fulfilled"; return value;
      }, (error) => {
        cleanupObservation.reservation_owner_status = "rejected";
        observedCleanupRejection("reservation", "reservation"); throw error;
      }),
    ]);
    cleanupObservation.owners_settled = true;
    cleanupObservation.owners_all_fulfilled = results.every((result) => result.status === "fulfilled");
    // Runtime files depend on confirmed process-owner cleanup. Reservation
    // failure does not skip independent file removal or replace its first error.
    cleanupObservation.runtime_safe_to_remove = Boolean(stage && results[0].status === "fulfilled" && results[1].status === "fulfilled");
    if (stage && results[0].status === "fulfilled" && results[1].status === "fulfilled") {
      cleanupObservation.main_stage = "runtime-stage";
      cleanupObservation.runtime_owner_status = "pending"; cleanupObservation.runtime_remove_started = true;
      const [removed] = await Promise.allSettled([rm(stage, { recursive: true, force: true }).then((value) => {
        cleanupObservation.runtime_remove_settled = true; cleanupObservation.runtime_remove_fulfilled = true;
        cleanupObservation.runtime_owner_status = "fulfilled"; return value;
      }, (error) => {
        cleanupObservation.runtime_remove_settled = true; cleanupObservation.runtime_owner_status = "rejected";
        observedCleanupRejection("runtime", "runtime-stage"); throw error;
      })]);
      results.push(removed);
    } else cleanupObservation.runtime_owner_status = "skipped";
    const failed = results.find((result) => result.status === "rejected");
    if (failed) {
      cleanupObservation.selected_failure_owner = ["browser", "next", "reservation", "runtime"][results.indexOf(failed)];
      throw failed.reason;
    }
  })();
  return stopPromise;
}
function finalize(outcome, exitCode) {
  finalization ??= (async () => {
    closing = true;
    let cleanupConfirmed = false;
    const deadlineAt = performance.now() + 6000;
    cleanupStartedAt = deadlineAt - 6000;
    const deadline = setTimeout(() => { cleanupDiagnostic("hard-deadline"); process.exit(3); }, 6000);
    try {
      await stop();
      cleanupObservation.main_stage = "pre-ack-budget";
      assert.ok(performance.now() < deadlineAt);
      cleanupObservation.main_stage = "ack";
      cleanupObservation.ack_owner_status = "pending"; cleanupObservation.ack_started = true;
      await writeFrame({ kind: "cleaned", outcome, creation_attempts: creationAttempts }).then((value) => {
        cleanupObservation.ack_settled = true; cleanupObservation.ack_fulfilled = true;
        cleanupObservation.ack_owner_status = "fulfilled"; return value;
      }, (error) => {
        cleanupObservation.ack_settled = true; cleanupObservation.ack_owner_status = "rejected";
        observedCleanupRejection("ack", "ack"); throw error;
      });
      cleanupObservation.main_stage = "post-ack-budget";
      assert.ok(performance.now() < deadlineAt);
      cleanupConfirmed = true;
      cleanupObservation.main_stage = "complete";
      process.exitCode = exitCode;
    } catch {
      cleanupDiagnostic("cleanup-rejected");
      process.exitCode = 3; // Cleanup unconfirmed; never a native result.
    } finally {
      // Failed cleanup still owns the original hard bound if handles remain.
      if (cleanupConfirmed) clearTimeout(deadline);
      else deadline.unref();
      process.stdin.destroy();
    }
  })();
  return finalization;
}
function interrupted() { void finalize("aborted", 2); }
process.once("SIGTERM", interrupted);
process.once("SIGINT", interrupted);
// Own the input from startup so EOF also cancels staging. Frame decoding and
// sequence validation stay in readFrame; early data only fills its bounded buffer.
void (async () => {
  try {
    for await (const chunk of input) {
      assert.ok(buffered.length + chunk.length <= MAX_FRAME + 4);
      buffered = Buffer.concat([buffered, chunk]);
      const changed = inputChanged;
      inputChanged = Promise.withResolvers();
      changed.resolve();
    }
  } catch {
    // Malformed/failed input retains failed/1; EOF retains aborted/2.
    void finalize("failed", 1);
  } finally {
    inputEnded = true;
    inputChanged.resolve();
    if (!closing) interrupted();
  }
})();
const watchdog = setTimeout(interrupted, 140000);
let outcome = "failed";
let exitCode = 1;

try {
  mainObservation.main_phase = "staging";
  const sourceRoot = path.resolve();
  const { stageProductionRuntime, verifyProductionRuntime } = await import("./production-runtime.mjs");
  assert.ok(!closing);
  startup = (async () => {
    stage = await mkdtemp(path.join(tmpdir(), "console-native-browser-"));
    const destination = path.join(stage, "runtime");
    await stageProductionRuntime({ sourceRoot, destination, signal: runtimeStagingAbort.signal });
    assert.equal((await verifyProductionRuntime(destination)).version, 1);
  })();
  await startup;
  assert.ok(!closing);
  mainObservation.main_phase = "reservation";
  reservation = createServer();
  startup = once(reservation, "listening");
  reservation.listen(0, "127.0.0.1");
  await startup;
  assert.ok(!closing);
  const port = reservation.address().port;
  const origin = `http://localhost:${port}`;
  // Rust must configure the exact RP origin before building the native router.
  mainObservation.main_phase = "origin";
  await writeFrame({ kind: "origin", origin });
  assert.ok(!closing);
  await new Promise((resolve) => reservation.close(resolve));
  assert.ok(!closing);
  mainObservation.main_phase = "next-readiness";
  mainObservation.next_readiness_status = "waiting";
  next = spawn(process.execPath, [path.join(stage, "runtime/server.mjs")], {
    cwd: path.join(stage, "runtime"),
    env: { ...cleanEnv, NODE_ENV: "production", HOSTNAME: "127.0.0.1", PORT: String(port),
      CONSOLE_PUBLIC_ORIGIN: origin, CONSOLE_BROWSER_ALLOW_LOOPBACK_HTTP: "true" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  for (const stream of [next.stdout, next.stderr]) stream.on("data", (bytes) => {
    serverBytes += bytes.length;
    if (serverBytes > 1048576) void interrupted();
  });
  next.once("error", () => { mainObservation.next_startup_error_seen = true; interrupted(); });
  next.once("exit", () => { mainObservation.next_startup_exit_seen = true; });
  next.once("close", () => { mainObservation.next_startup_close_seen = true; });
  let ready = false;
  const deadline = Date.now() + PHASE_MS;
  while (Date.now() < deadline) {
    assert.ok(!closing);
    assert.equal(next.exitCode, null);
    try {
      const response = await fetch(`${origin}${DOCUMENT_PATH}`, {
        redirect: "error", signal: AbortSignal.timeout(1000),
      });
      ready = response.status === 404 && response.headers.get("content-type")?.startsWith("text/html;");
      mainObservation.next_probe_result = ready ? "404-html" : "other-response";
      mainObservation.next_probe_error_type = "none";
      await response.body?.cancel();
      if (ready) break;
    } catch (error) {
      mainObservation.next_probe_result = "request-error";
      mainObservation.next_probe_error_type = observedErrorType(error);
      /* readiness deadline owns connection refusal */
    }
    await delay(100);
  }
  mainObservation.next_readiness_budget_elapsed = Date.now() >= deadline;
  mainObservation.next_readiness_status = ready ? "ready" : "not-ready";
  assert.ok(ready);
  assert.ok(!closing);
  mainObservation.main_phase = "browser-launch";
  startup = chromium.launchServer({ env: cleanEnv }).then((server) => { browserServer = server; });
  await startup;
  assert.ok(!closing);
  mainObservation.main_phase = "browser-connect";
  browser = await chromium.connect(browserServer.wsEndpoint());
  assert.ok(!closing);
  mainObservation.main_phase = "ready";
  mainObservation.ready_write_started = true;
  await writeFrame({ kind: "ready", origin }).then((value) => {
    mainObservation.ready_write_settled = true; mainObservation.ready_write_fulfilled = true; return value;
  }, (error) => { mainObservation.ready_write_settled = true; throw error; });
  mainObservation.main_phase = "validation";

  for (const actor of ["a", "b"]) {
    const command = await readFrame(60000);
    assert.ok(!closing);
    assert.deepEqual(Object.keys(command).sort(), ["actor", "ceremony", "kind", "mode", "options", "seq", "v"]);
    assert.equal(command.kind, "register");
    assert.equal(command.actor, actor);
    assert.match(command.ceremony, /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
    assert.equal(command.options.publicKey.rp.id, "localhost");
    assert.ok(["legacy", "discoverable"].includes(command.mode));
    if (command.mode === "discoverable") {
      // An older backend ignores the opt-in field. Fail BEFORE creating a key.
      const selection = command.options.publicKey.authenticatorSelection;
      assert.equal(selection?.residentKey, "required");
      assert.equal(selection?.requireResidentKey, true);
      assert.equal(selection?.userVerification, "required");
    }
    const context = await browser.newContext();
    const page = await context.newPage();
    const response = await page.goto(`${origin}${DOCUMENT_PATH}`, { timeout: PHASE_MS });
    assert.equal(response.status(), 404);
    assert.equal(new URL(page.url()).origin, origin);
    const cdp = await context.newCDPSession(page);
    await cdp.send("WebAuthn.enable");
    const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
      options: { protocol: "ctap2", transport: "internal", hasResidentKey: true,
        hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
    });
    creationAttempts += 1;
    const result = await page.evaluate(async (options) => {
      const abort = new AbortController();
      const timer = setTimeout(() => abort.abort(), 15000);
      try {
        const credential = await navigator.credentials.create({
          ...options,
          publicKey: PublicKeyCredential.parseCreationOptionsFromJSON(options.publicKey),
          signal: abort.signal,
        });
        return { credential: credential.toJSON(),
          resident_key: credential.getClientExtensionResults().credProps?.rk ?? null };
      } finally { clearTimeout(timer); }
    }, command.options);
    assert.ok(result.resident_key === null || typeof result.resident_key === "boolean");
    await writeFrame({ kind: "registered", actor, ceremony: command.ceremony, ...result });
    const login = await readFrame(60000);
    assert.deepEqual(Object.keys(login).sort(), ["actor", "ceremony", "kind", "options", "seq", "v"]);
    assert.equal(login.kind, "login");
    assert.equal(login.actor, actor);
    assert.match(login.ceremony, /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
    if (command.mode === "discoverable") assert.equal(Object.hasOwn(login.options, "mediation"), false);
    else assert.equal(login.options.mediation, "conditional");
    assert.equal(login.options.publicKey.rpId, "localhost");
    assert.deepEqual(login.options.publicKey.allowCredentials, []);
    assert.equal(login.options.publicKey.userVerification, "required");
    // Observe only the genuine, natively accepted test credential. CDP returns
    // its private key transiently; destructure only metadata, never log/retain it.
    const { credentialId, rpId, isResidentCredential } = (await cdp.send("WebAuthn.getCredential", {
      authenticatorId, credentialId: Buffer.from(result.credential.id, "base64url").toString("base64"),
    })).credential;
    assert.ok(Buffer.from(credentialId, "base64").equals(Buffer.from(result.credential.id, "base64url")));
    assert.equal(rpId, "localhost");
    assert.equal(typeof isResidentCredential, "boolean");
    const environment = await page.evaluate(async () => ({
      secure_context: window.isSecureContext,
      document_has_focus: document.hasFocus(),
      visibility_state: document.visibilityState,
      conditional_mediation_available: typeof PublicKeyCredential.isConditionalMediationAvailable === "function"
        ? await PublicKeyCredential.isConditionalMediationAvailable() : null,
    }));
    const attempted = await page.evaluate(async (options) => {
      // Codec/API failures are infrastructure failures, not browser outcomes.
      const publicKey = PublicKeyCredential.parseRequestOptionsFromJSON(options.publicKey);
      const abort = new AbortController();
      let deadline = false;
      const timer = setTimeout(() => { deadline = true; abort.abort(); }, 15000);
      let credential;
      try {
        credential = await navigator.credentials.get({ ...options, publicKey, signal: abort.signal });
      } catch (error) {
        return { outcome: deadline && error.name === "AbortError" ? "deadline-aborted" :
          ["NotAllowedError", "AbortError", "SecurityError", "NotSupportedError", "InvalidStateError"].includes(error.name) ? error.name : "other-rejection" };
      } finally { clearTimeout(timer); }
      return credential ? { outcome: "assertion", credential: credential.toJSON() } : { outcome: "null" };
    }, login.options);
    await writeFrame({ kind: "login-result", actor, ceremony: login.ceremony, ...attempted,
      authenticator_is_resident: isResidentCredential, browser_environment: environment });
    await context.close();
  }
  const done = await readFrame(60000);
  assert.deepEqual(done, { v: 1, seq: 4, kind: "done" });
  mainObservation.main_phase = "completed";
  outcome = "completed";
  exitCode = 0;
} catch (error) {
  mainFailureDiagnostic(error);
  // No raw assertion, server output, private CDP credential or error stack.
  process.stderr.write("native browser enrollment fixture failed\n");
} finally {
  clearTimeout(watchdog);
  await finalize(outcome, exitCode);
}
