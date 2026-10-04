// Native prerequisite only. Rust keeps OTPs, tokens, keys and the disposable DSN.
// No credential injection or changes to server-issued creation options.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { cp, mkdtemp, readdir, rm } from "node:fs/promises";
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
let incoming = 0;
let outgoing = 0;
async function readFrame(waitMs = PHASE_MS) {
  assert.ok(++incoming <= MAX_FRAMES);
  const deadline = Date.now() + waitMs;
  while (true) {
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
    const item = await Promise.race([
      input.next(),
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("IPC deadline")), remaining); }),
    ]).finally(() => clearTimeout(timer));
    assert.ok(!item.done);
    assert.ok(buffered.length + item.value.length <= MAX_FRAME + 4);
    buffered = Buffer.concat([buffered, item.value]);
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
async function stop() {
  stopPromise ??= (async () => {
    // An EOF can race an asynchronous launch. Wait for its returned owner before
    // acknowledging cleanup; closing prevents starting another resource.
    await startup.catch(() => {});
    // Playwright launches a separate browser process group. Close its owner,
    // rather than assume that terminating this Node process reaps Chromium.
    const results = await Promise.allSettled([
      (async () => {
        if (!browserServer) return;
        const closed = await Promise.race([
          browserServer.close().then(() => true, () => false),
          delay(2000).then(() => false),
        ]);
        if (!closed) {
          assert.ok(await Promise.race([
            browserServer.kill().then(() => true), delay(2000).then(() => false),
          ]));
        }
        const process = browserServer.process();
        assert.ok(process.exitCode !== null || process.signalCode !== null);
      })(),
      (async () => {
        if (!next || next.exitCode !== null || next.signalCode !== null) return;
        const exited = once(next, "exit");
        next.kill("SIGTERM");
        const closed = await Promise.race([exited.then(() => true), delay(2000).then(() => false)]);
        if (!closed) {
          next.kill("SIGKILL");
          assert.ok(await Promise.race([exited.then(() => true), delay(2000).then(() => false)]));
        }
      })(),
      reservation?.listening ? new Promise((resolve) => reservation.close(resolve)) : Promise.resolve(),
    ]);
    assert.ok(results.every((result) => result.status === "fulfilled"));
    if (stage) await rm(stage, { recursive: true, force: true });
  })();
  return stopPromise;
}
function finalize(outcome, exitCode) {
  finalization ??= (async () => {
    closing = true;
    const deadline = setTimeout(() => process.exit(3), 6000);
    try {
      await stop();
      await writeFrame({ kind: "cleaned", outcome, creation_attempts: creationAttempts });
      process.exitCode = exitCode;
    } catch {
      process.exitCode = 3; // Cleanup unconfirmed; never a native result.
    } finally {
      clearTimeout(deadline);
      process.stdin.destroy();
    }
  })();
  return finalization;
}
function interrupted() { void finalize("aborted", 2); }
process.once("SIGTERM", interrupted);
process.once("SIGINT", interrupted);
process.stdin.once("end", () => { if (!closing) interrupted(); });
const watchdog = setTimeout(interrupted, 140000);
let outcome = "failed";
let exitCode = 1;

try {
  const standalone = path.resolve(".next/standalone");
  // Next loads dotenv files at startup; no inherited source-tree dotenv file
  // belongs in this isolated service. Reject one instead of silently using it.
  const entries = await readdir(standalone, { recursive: true });
  assert.ok(!entries.some((entry) => path.basename(entry).startsWith(".env")));
  assert.ok(!closing);
  startup = (async () => {
    stage = await mkdtemp(path.join(tmpdir(), "console-native-browser-"));
    await cp(standalone, stage, { recursive: true });
    await cp(path.resolve(".next/static"), path.join(stage, ".next/static"), { recursive: true });
  })();
  await startup;
  // No public asset is needed for this browser-origin prerequisite.
  assert.ok(!closing);
  reservation = createServer();
  startup = once(reservation, "listening");
  reservation.listen(0, "127.0.0.1");
  await startup;
  assert.ok(!closing);
  const port = reservation.address().port;
  const origin = `http://localhost:${port}`;
  // Rust must configure the exact RP origin before building the native router.
  await writeFrame({ kind: "origin", origin });
  assert.ok(!closing);
  await new Promise((resolve) => reservation.close(resolve));
  assert.ok(!closing);
  next = spawn(process.execPath, [path.join(stage, "server.js")], {
    cwd: stage,
    env: { ...cleanEnv, NODE_ENV: "production", HOSTNAME: "127.0.0.1", PORT: String(port) },
    stdio: ["ignore", "pipe", "pipe"],
  });
  for (const stream of [next.stdout, next.stderr]) stream.on("data", (bytes) => {
    serverBytes += bytes.length;
    if (serverBytes > 1048576) void interrupted();
  });
  next.once("error", interrupted);
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
      await response.body?.cancel();
      if (ready) break;
    } catch { /* readiness deadline owns connection refusal */ }
    await delay(100);
  }
  assert.ok(ready);
  assert.ok(!closing);
  startup = chromium.launchServer({ env: cleanEnv }).then((server) => { browserServer = server; });
  await startup;
  assert.ok(!closing);
  browser = await chromium.connect(browserServer.wsEndpoint());
  assert.ok(!closing);
  await writeFrame({ kind: "ready", origin });

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
  outcome = "completed";
  exitCode = 0;
} catch {
  // No raw assertion, server output, private CDP credential or error stack.
  process.stderr.write("native browser enrollment fixture failed\n");
} finally {
  clearTimeout(watchdog);
  await finalize(outcome, exitCode);
}
