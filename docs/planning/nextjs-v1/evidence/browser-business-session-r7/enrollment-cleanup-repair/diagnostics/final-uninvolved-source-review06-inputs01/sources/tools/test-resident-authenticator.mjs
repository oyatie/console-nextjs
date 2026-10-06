// Test-only native crypto fixture: static HTTPS document, actual WebAuthn APIs.
// It never calls/mocks business APIs or injects credentials into Chromium.
// Conditional presentation is deliberately modal in this native fixture only;
// unchanged C2/product runners own actual conditional/production UI evidence.
import assert from "node:assert/strict";
import { writeSync } from "node:fs";
import { chromium } from "@playwright/test";

const ORIGIN = "https://auth.example.com";
const MAX_FRAME = 65_536;
const MAX_FRAMES = 128;
const PHASE_MS = 20_000;
const STARTUP_MS = 5_000;
const COMMAND_IDLE_MS = 60_000;
const cleanEnv = Object.fromEntries(
  ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "PLAYWRIGHT_BROWSERS_PATH"]
    .filter((key) => process.env[key] !== undefined).map((key) => [key, process.env[key]]),
);
const input = process.stdin[Symbol.asyncIterator]();
// Start one bounded read before launch/setup so early EOF reaches the owner
// immediately. Observe rejection without replacing the original awaited result.
function prefetchInput() {
  const pending = input.next();
  pending.catch(() => { if (!closing) interrupted(); });
  return pending;
}
let pendingInput = prefetchInput();
let buffered = Buffer.alloc(0);
let incoming = 0;
let outgoing = 0;
let browserServer;
let browser;
let context;
let page;
let cdp;
let authenticatorId;
let registration;
let closing = false;
let startup = Promise.resolve();
let finalization;
let watchdog;

async function readFrame() {
  assert.ok(incoming < MAX_FRAMES);
  const deadline = Date.now() + COMMAND_IDLE_MS;
  while (true) {
    if (buffered.length >= 4) {
      const length = buffered.readUInt32BE();
      assert.ok(length > 0 && length <= MAX_FRAME);
      if (buffered.length >= 4 + length) {
        const frame = JSON.parse(buffered.subarray(4, 4 + length).toString("utf8"));
        buffered = buffered.subarray(4 + length);
        assert.equal(frame.v, 1);
        assert.equal(frame.seq, incoming++);
        return frame;
      }
    }
    const remaining = deadline - Date.now();
    assert.ok(remaining > 0);
    let timer;
    const item = await Promise.race([
      pendingInput,
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("IPC deadline")), remaining); }),
    ]).finally(() => clearTimeout(timer));
    assert.ok(!item.done);
    pendingInput = prefetchInput();
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
  await new Promise((resolve, reject) => process.stdout.write(Buffer.concat([header, body]),
    (error) => error ? reject(error) : resolve()));
}

async function finalize(outcome, exitCode) {
  finalization ??= (async () => {
    closing = true;
    clearTimeout(watchdog);
    // Enclose the 5s launch owner plus 2s close, 2s kill and IPC headroom.
    // Rust allows 15s. Timeout remains unconfirmed cleanup, never success.
    let stage = "startup";
    let child;
    let childExitSeen = false;
    let childCloseSeen = false;
    let closeSettled = false;
    let killSettled = false;
    let cleanupConfirmed = false;
    // The fixture owns the 2s phase timer; it must release a losing timer too.
    async function waitForShutdown(operation) {
      let timer;
      try {
        return await Promise.race([operation,
          new Promise((resolve) => { timer = setTimeout(() => resolve(false), 2_000); })]);
      } finally { clearTimeout(timer); }
    }
    function diagnostic() {
      // Fixed failure metadata only. Never expose errors, IDs, paths or native payloads.
      const fields = { stage, child_exit_seen: childExitSeen, child_close_seen: childCloseSeen,
        close_settled: closeSettled, kill_settled: killSettled,
        child_exited: Boolean(child && (child.exitCode !== null || child.signalCode !== null)) };
      try { writeSync(2, `RESIDENT_CLEANUP_DIAGNOSTIC ${JSON.stringify(fields)}\n`); } catch {}
    }
    const deadline = setTimeout(() => { diagnostic(); process.exit(3); }, 12_000);
    try {
      // EOF/signal can race launch; await its returned resource owner first.
      await startup.catch(() => {});
      let browserPid = null;
      let browserExited = null;
      if (browserServer) {
        stage = "browser-identity";
        child = browserServer.process();
        child.once("exit", () => { childExitSeen = true; });
        child.once("close", () => { childCloseSeen = true; });
        browserPid = child.pid;
        assert.ok(Number.isSafeInteger(browserPid) && browserPid > 0);
        stage = "browser-close";
        const closed = await waitForShutdown(
          browserServer.close().then(() => { closeSettled = true; return true; },
            () => { closeSettled = true; return false; }),
        );
        if (!closed) {
          stage = "browser-kill";
          assert.ok(await waitForShutdown(
            browserServer.kill().then(() => { killSettled = true; return true; },
              () => { killSettled = true; return false; }),
          ));
        }
        stage = "browser-exit";
        browserExited = child.exitCode !== null || child.signalCode !== null;
        assert.equal(browserExited, true);
      }
      registration = undefined;
      buffered = Buffer.alloc(0);
      stage = "ack";
      await writeFrame({ kind: "cleaned", outcome, browser_pid: browserPid, browser_exited: browserExited });
      cleanupConfirmed = true;
      process.exitCode = exitCode;
    } catch {
      diagnostic();
      process.exitCode = 3; // No cleanup ACK or native-success substitute.
    } finally {
      // Failed shutdown still owns its hard bound; an idle failure may exit
      // naturally with code3, while pending SDK handles remain bounded by12s.
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
process.stdin.once("end", () => { if (!closing) interrupted(); });
watchdog = setTimeout(interrupted, 600_000);

async function secureDocument() {
  assert.equal(new URL(page.url()).origin, ORIGIN);
  assert.deepEqual(await page.evaluate(() => ({
    origin: location.origin, secure: isSecureContext,
    creation: typeof PublicKeyCredential.parseCreationOptionsFromJSON,
    request: typeof PublicKeyCredential.parseRequestOptionsFromJSON,
  })), { origin: ORIGIN, secure: true, creation: "function", request: "function" });
}

async function residentMetadata(credential) {
  // The CDP response includes a private key transiently. Never retain/log it;
  // destructure only public identity and resident metadata, as the C2 owner does.
  const { credentialId, rpId, isResidentCredential, signCount } = (await cdp.send("WebAuthn.getCredential", {
    authenticatorId, credentialId: Buffer.from(credential.id, "base64url").toString("base64"),
  })).credential;
  assert.ok(Buffer.from(credentialId, "base64").equals(Buffer.from(credential.id, "base64url")));
  assert.equal(rpId, "example.com");
  assert.equal(isResidentCredential, true);
  assert.ok(Number.isSafeInteger(signCount) && signCount >= 0 && signCount <= 0xffff_ffff);
  return signCount;
}

let outcome = "failed";
let exitCode = 1;
try {
  startup = chromium.launchServer({ env: cleanEnv, timeout: STARTUP_MS })
    .then((server) => { browserServer = server; });
  await startup;
  assert.ok(!closing);
  browser = await chromium.connect(browserServer.wsEndpoint(), { timeout: PHASE_MS });
  assert.ok(!closing);
  context = await browser.newContext({ serviceWorkers: "block", acceptDownloads: false });
  // Only this fixed fixture document is fulfilled. Every other URL is denied;
  // there is no native API interception and no HTTP/provider success simulation.
  await context.route("**/*", (route) => {
    if (route.request().url() === `${ORIGIN}/` && route.request().method() === "GET") {
      return route.fulfill({ status: 200, contentType: "text/html", body:
        "<!doctype html><html lang=en><meta charset=utf-8><title>Native authenticator fixture</title><body></body></html>" });
    }
    return route.abort("blockedbyclient");
  });
  page = await context.newPage();
  await page.goto(`${ORIGIN}/`, { timeout: PHASE_MS });
  await secureDocument();
  cdp = await context.newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  ({ authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: { protocol: "ctap2", transport: "internal", hasResidentKey: true,
      hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
  }));
  const browserPid = browserServer.process().pid;
  assert.ok(Number.isSafeInteger(browserPid) && browserPid > 0);
  await writeFrame({ kind: "ready", origin: ORIGIN, secure_context: true, browser_pid: browserPid });
  while (!closing) {
    const command = await readFrame();
    assert.ok(!closing);
    if (command.kind === "done") {
      assert.deepEqual(Object.keys(command).sort(), ["kind", "seq", "v"]);
      outcome = "completed";
      exitCode = 0;
      break;
    }
    assert.deepEqual(Object.keys(command).sort(), ["kind", "options", "origin", "seq", "v"]);
    assert.equal(command.origin, ORIGIN);
    await secureDocument();
    if (command.kind === "register") {
      assert.equal(registration, undefined);
      assert.equal(command.options.publicKey.rp.id, "example.com");
      const selection = command.options.publicKey.authenticatorSelection;
      assert.equal(selection?.residentKey, "required");
      assert.equal(selection?.requireResidentKey, true);
      assert.equal(selection?.userVerification, "required");
      const result = await page.evaluate(async (options) => {
        const abort = new AbortController();
        const timer = setTimeout(() => abort.abort(), 15_000);
        try {
          const credential = await navigator.credentials.create({ ...options,
            publicKey: PublicKeyCredential.parseCreationOptionsFromJSON(options.publicKey), signal: abort.signal });
          if (!(credential instanceof PublicKeyCredential)) throw new Error("No registration");
          return { credential: credential.toJSON(), resident: credential.getClientExtensionResults().credProps?.rk ?? null };
        } finally { clearTimeout(timer); }
      }, command.options);
      assert.ok(result.resident === null || result.resident === true);
      const counter = await residentMetadata(result.credential);
      registration = { id: result.credential.id, user: command.options.publicKey.user.id, counter };
      await writeFrame({ kind: "registered", credential: result.credential, resident: true });
    } else {
      assert.equal(command.kind, "authenticate");
      assert.ok(registration);
      assert.equal(command.options.publicKey.rpId, "example.com");
      assert.equal(command.options.publicKey.userVerification, "required");
      assert.deepEqual(command.options.publicKey.allowCredentials, []);
      const credential = await page.evaluate(async (options) => {
        const abort = new AbortController();
        const timer = setTimeout(() => abort.abort(), 15_000);
        try {
          // SoftPasskey never exercised UI mediation. This crypto fixture uses
          // modal presentation; publicKey and every authenticator field remain
          // unchanged. Conditional production UI retains its independent proof.
          const credential = await navigator.credentials.get({ ...options, mediation: "required",
            publicKey: PublicKeyCredential.parseRequestOptionsFromJSON(options.publicKey), signal: abort.signal });
          if (!(credential instanceof PublicKeyCredential)) throw new Error("No assertion");
          return credential.toJSON();
        } finally { clearTimeout(timer); }
      }, command.options);
      assert.equal(credential.id, registration.id);
      assert.equal(credential.response.userHandle, registration.user);
      const data = Buffer.from(credential.response.authenticatorData, "base64url");
      assert.ok(data.length >= 37);
      assert.equal(data[32] & 0x05, 0x05); // Actual signed UP and UV, never patched.
      const counter = data.readUInt32BE(33);
      assert.ok(counter > registration.counter);
      assert.equal(await residentMetadata(credential), counter);
      registration.counter = counter;
      await writeFrame({ kind: "authenticated", credential, resident: true });
    }
  }
} catch {
  if (!closing) {
    process.stderr.write("resident authenticator fixture failed\n");
    await writeFrame({ kind: "error", code: "fixture-failure" }).catch(() => {});
  }
} finally {
  await finalize(outcome, exitCode);
}
