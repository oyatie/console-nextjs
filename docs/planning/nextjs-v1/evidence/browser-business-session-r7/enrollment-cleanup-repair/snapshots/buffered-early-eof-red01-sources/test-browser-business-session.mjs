// Native prerequisite only. Rust keeps OTPs, tokens, keys and the disposable DSN.
// No credential injection or changes to server-issued creation options.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { chromium } from "@playwright/test";
import { browserFailureLocation } from "./browser-business-failure.mjs";

const MAX_FRAME = 65536;
const MAX_FRAMES = 2048; // Product-only bounded control/rate-window census.
const PHASE_MS = 90000; // Product-only phase bound; C2 remains 20 seconds.
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
async function readFrameUnsafe(waitMs = PHASE_MS) {
  const expectedSequence = incoming++; assert.ok(incoming <= MAX_FRAMES);
  const deadline = Date.now() + waitMs;
  while (true) {
    if (buffered.length >= 4) {
      const length = buffered.readUInt32BE();
      assert.ok(length > 0 && length <= MAX_FRAME);
      if (buffered.length >= 4 + length) {
        const frame = JSON.parse(buffered.subarray(4, 4 + length).toString("utf8"));
        buffered = buffered.subarray(4 + length);
        assert.equal(frame.v, 1);
        assert.equal(frame.seq, expectedSequence);
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
async function writeFrameUnsafe(fields) {
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
async function stop() {
  stopPromise ??= (async () => {
    // An EOF can race an asynchronous launch. Wait for its returned owner before
    // acknowledging cleanup; closing prevents starting another resource.
    runtimeStagingAbort.abort();
    await startup.catch(() => {});
    await Promise.allSettled([...productPendingLaunches]);
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
      Promise.all([...productInstances].map((instance) => instance.close())),
      Promise.all([...productReservations].map((lease) => lease.listening
        ? new Promise((resolve) => lease.close(resolve)) : Promise.resolve())),
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

import { createHash, randomBytes, randomUUID } from "node:crypto";
import { writeFile, lstat } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { request as httpsRequest } from "node:https";
import { request as httpRequest } from "node:http";
clearTimeout(watchdog); // C2 source watchdog remains unchanged in its own runner.
const productWatchdog = setTimeout(interrupted, 1800000);
async function readFrame(waitMs) {
  try { return await readFrameUnsafe(waitMs); } catch { throw new InfrastructureError("IPC read failed"); }
}
async function writeFrame(fields) {
  try { return await writeFrameUnsafe(fields); } catch { throw new InfrastructureError("IPC write failed"); }
}

// Test-only adapters. All network bytes come from the staged immutable runtime
// or the actual private native service; control IPC carries no JWT/refresh/DSN.
let productRuntimeConfig;
let productRuntimeManifest;
let productSharedContext;
const productInstances = new Set();
const productPendingLaunches = new Set();
const productReservations = new Set();
const productRuntimeSecrets = new Set();
const PRODUCT_NATIVE = Object.freeze({
  start: "/api/v1/auth/browser-session/start", login: "/api/v1/auth/browser-session/login",
  logout: "/api/v1/auth/browser-session/logout", history: "/api/v1/hr/browser-session/attendance-records/me",
});

let productIpcChain = Promise.resolve();
function productExchange(fields, validate) {
  const exchange = productIpcChain.then(async () => {
    await writeFrame(fields); const response = await readFrame();
    try { validate(response); } catch { throw new InfrastructureError("invalid IPC response"); }
    return response;
  });
  productIpcChain=exchange.then(()=>undefined,()=>undefined);
  return exchange;
}

async function control(fields) {
  const id = randomUUID();
  const response = await productExchange({ kind: "control", id, ...fields },(response)=>{
    assert.deepEqual(Object.keys(response).sort(), ["id", "kind", "op", "result", "seq", "v"]);
    assert.equal(response.kind, "controlled"); assert.equal(response.id, id); assert.equal(response.op, fields.op);
  });
  assertNoProof(response.result);
  return response.result;
}

async function rateBudget(starts = 1, device = "") {
  assert.ok(Number.isSafeInteger(starts) && starts > 0 && starts <= 10);
  const observations = [];
  const deadline = Date.now() + 65000;
  while (Date.now() < deadline) {
    const state = await control({ op: "rate-status", device });
    observations.push({ now: state.now, window_end: state.window_end, attempts: state.attempts });
    assert.ok(Number.isSafeInteger(state.now) && state.window_end - state.window_start === 60);
    const used = Math.max(state.attempts["ip:127.0.0.1"] ?? 0, state.attempts["ip:::1"] ?? 0);
    if (used + starts <= 10 && (state.attempts.global ?? 0) + starts <= 100) return observations;
    // Real fixed-window wait, with bounded IPC observations; no bucket reset.
    await delay(Math.min(1000, Math.max(10, (state.window_end - state.now) * 1000 + 20)));
  }
  throw new InfrastructureError();
}

async function stopInstance(instance) {
  instance.stopPromise ??= stopInstanceOwned(instance);
  return instance.stopPromise;
}

async function stopInstanceOwned(instance) {
  if (instance.child.exitCode === null && instance.child.signalCode === null) {
    const exited = once(instance.child, "exit"); instance.child.kill("SIGTERM");
    if (!await Promise.race([exited.then(() => true), delay(2000).then(() => false)])) {
      instance.child.kill("SIGKILL");
      assert.ok(await Promise.race([exited.then(() => true), delay(2000).then(() => false)]));
    }
  }
  productInstances.delete(instance);
}

function rememberRuntimeSecrets(environment) {
  for (const name of ["CONSOLE_BROWSER_INGRESS_KEY", "CONSOLE_BROWSER_PREAUTH_KEY"]) {
    const value = environment[name];
    if (typeof value !== "string" || value.length === 0) continue;
    const bytes = Buffer.from(value, "base64url");
    for (const encoding of [value, bytes.toString("base64url"), bytes.toString("base64"), bytes.toString("hex")]) {
      if (encoding.length >= 32) productRuntimeSecrets.add(encoding);
    }
  }
}

function startRuntime(options = {}) {
  if (!productRuntimeConfig || closing) return Promise.reject(new InfrastructureError("runtime launch is closed"));
  const launch = startRuntimeOwned(options);
  productPendingLaunches.add(launch);
  void launch.finally(() => productPendingLaunches.delete(launch)).catch(() => {});
  return launch;
}

async function startRuntimeOwned({ launcher = "custom", overrides = {}, main = false } = {}) {
  let lease;
  try {
  if (!productRuntimeConfig || closing) throw new InfrastructureError("runtime launch is closed");
  let port = productRuntimeConfig.port;
  if (!main) {
    lease = createServer(); productReservations.add(lease);
    const listening = once(lease, "listening");
    lease.listen(0, "::"); await listening; port = lease.address().port;
    await new Promise((resolve) => lease.close(resolve));
  }
  const origin = `${launcher === "next-start" ? "http" : "https"}://localhost:${port}`;
  const cwd = path.join(stage, "runtime");
  const baseEnv = { ...cleanEnv, NODE_ENV: "production", HOSTNAME: "::", PORT: String(port),
    CONSOLE_BACKEND_ORIGIN: productRuntimeConfig.backend,
    CONSOLE_PUBLIC_ORIGIN: origin, CONSOLE_BROWSER_INGRESS_KEY: productRuntimeConfig.ingress,
    CONSOLE_BROWSER_PREAUTH_KEY: productRuntimeConfig.preauth,
    CONSOLE_TLS_CERT_FILE: productRuntimeConfig.certFile,
    CONSOLE_TLS_KEY_FILE: productRuntimeConfig.keyFile,
    CONSOLE_BROWSER_ALLOW_LOOPBACK_HTTP: "true", ...overrides };
  for (const key of Object.keys(baseEnv)) if (baseEnv[key] === undefined) delete baseEnv[key];
  rememberRuntimeSecrets(baseEnv);
  const entry = launcher === "custom" ? [path.join(cwd, "server.mjs")]
    : launcher === "next-start" ? [path.join(cwd, "node_modules/next/dist/bin/next"), "start", "-H", "::", "-p", String(port)]
    : (() => { throw new InfrastructureError(); })();
  // Closing is checked after the asynchronous reservation and immediately
  // before spawn, so cleanup cannot miss a newly created process owner.
  if (closing) throw new InfrastructureError("runtime launch is closed");
  const instance = { child: spawn(process.execPath, entry, { cwd, env: baseEnv, stdio: ["ignore", "pipe", "pipe"] }),
    origin, launcher, tls: { origin, caPem: productRuntimeConfig.caPem }, close() { return stopInstance(instance); } };
  productInstances.add(instance);
  if (main) { next = instance.child; productRuntimeConfig.instance = instance; }
  for (const stream of [instance.child.stdout, instance.child.stderr]) stream.on("data", (bytes) => {
    serverBytes += bytes.length; if (serverBytes > 1048576) void interrupted();
  });
  instance.child.once("error", () => { instance.startupError = true; if (main) interrupted(); });
  await runtimeReady(instance);
  return instance;
  } finally {
    if (lease) {
      if (lease.listening) await new Promise((resolve) => lease.close(resolve));
      productReservations.delete(lease);
    }
  }
}

async function runtimeReady(instance) {
  const deadline = Date.now() + PHASE_MS;
  while (Date.now() < deadline) {
    if (closing || instance.startupError || instance.child.exitCode !== null || instance.child.signalCode !== null) throw new InfrastructureError();
    try {
      const status = await new Promise((resolve, reject) => {
        const transport = instance.origin.startsWith("https:") ? httpsRequest : httpRequest;
        const request = transport(`${instance.origin}/_not-found/`, { ca: productRuntimeConfig.caPem, family: 4 }, (response) => {
          response.resume(); resolve(response.statusCode);
        });
        request.setTimeout(1000, () => request.destroy()); request.once("error", reject); request.end();
      });
      if (status === 404) return;
    } catch { /* Only the real owned listener can satisfy readiness. */ }
    await delay(100);
  }
  throw new InfrastructureError();
}

async function preflightScenarioModules(sourceRoot) {
  const required = [
    ["boundary", "browser-business-boundary.mjs", ["historyVariants", "runBoundaryScenarios"]],
    ["temporal", "browser-business-temporal.mjs", ["runTemporalScenarios", "runUnavailableScenarios", "runFinalCloseScenario"]],
    ["restore", "browser-business-restore.mjs", ["runRestoreScenarios"]],
  ];
  const modules = {};
  try {
    for (const [name, file, exports] of required) {
      const module = await import(pathToFileURL(path.join(sourceRoot, "tools", file)).href);
      if (!exports.every((symbol) => typeof module[symbol] === "function")) throw new Error("missing scenario export");
      modules[name] = module;
    }
  } catch { throw new InfrastructureError("scenario modules unavailable"); }
  return modules;
}

async function restartRuntime(overrides = {}) {
  const before = productRuntimeConfig.instance;
  if (before) await before.close();
  if (overrides.CONSOLE_BACKEND_ORIGIN) productRuntimeConfig.backend = overrides.CONSOLE_BACKEND_ORIGIN;
  if (overrides.CONSOLE_BROWSER_PREAUTH_KEY) productRuntimeConfig.preauth = overrides.CONSOLE_BROWSER_PREAUTH_KEY;
  startup = startRuntime({ main: true, overrides });
  const instance = await startup; await runtimeReady(instance); return instance;
}

async function holdResponse(path, binding = null) {
  const gate = randomUUID();
  await control({ op: "fault-arm", gate, path, binding });
  return { gate, wait() { return control({ op: "fault-wait", gate }); },
    release(action = "intact") { return control({ op: "fault-release", gate, action }); } };
}

async function rawPublic(actor, pathname, body, csrf) {
  const entry = actors.get(actor);
  return entry.page.evaluate(async ({ pathname, body, csrf }) => {
    const response = await fetch(pathname, { method: "POST", credentials: "same-origin", cache: "no-store",
      headers: { ...(body === undefined ? {} : { "content-type": "application/json" }), ...(csrf ? { "x-csrf-token": csrf } : {}) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
    return { status: response.status, body: await response.text() };
  }, { pathname, body, csrf });
}

async function startCeremony(actor) {
  await rateBudget();
  const result = await rawPublic(actor, PRODUCT_PATHS.start, undefined);
  assert.equal(result.status, 200); assertNoProof(result.body);
  const value = JSON.parse(result.body);
  assert.deepEqual(Object.keys(value).sort(), ["ceremony_id", "challenge", "csrf_token", "expires_at"]);
  return value;
}

async function assertion(actor, start) {
  const entry = actors.get(actor);
  const credential = await entry.page.evaluate(async (challenge) => {
    const result = await navigator.credentials.get({ ...challenge,
      publicKey: PublicKeyCredential.parseRequestOptionsFromJSON(challenge.publicKey),
      signal: AbortSignal.timeout(15000) });
    return result.toJSON();
  }, start.challenge);
  assert.equal(credential.id, entry.credentialId); return credential;
}

function sessionCsrf(token, context) {
  return createHash("sha256").update("console/session-csrf/v1").update(Buffer.from([0]))
    .update(Buffer.from(token.slice(4), "base64url")).update(Buffer.from(context.replaceAll("-", ""), "hex")).digest("base64url");
}

async function absent(actor, context) {
  const entry = actors.get(actor);
  const response = await entry.page.goto(`${entry.origin}/me/${context}/attendance/`, { timeout: PHASE_MS });
  const text = await response.text(); assertNoProof(text);
  assert.ok(!text.includes(entry.facts.account_name) && !text.includes(entry.facts.company_name));
  assert.equal(await entry.page.getByRole("table").count(), 0);
  return response;
}

async function cookieSnapshot() {
  return (await productSharedContext.cookies()).filter((cookie) => cookie.name.startsWith("__Host-console-"))
    .map(({ name, value, expires, httpOnly, secure, sameSite, path }) => ({ name, value, expires, httpOnly, secure, sameSite, path }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

async function pendingFailure(actor) {
  const page = actors.get(actor).page;
  // Next's shadow-DOM route announcer has its own alert outside main.
  const notice = page.getByRole("main").getByRole("alert");
  await notice.waitFor({ state: "visible", timeout: PHASE_MS });
  assert.equal(await notice.count(), 1);
  const text = await notice.innerText();
  assert.ok(text.trim().length > 0);
  assert.ok(!/완료|성공/.test(text)); assertNoProof(text);
  return text;
}

function proposalHarness() {
  return { actors, sessions, paths: PRODUCT_PATHS, native: PRODUCT_NATIVE, begin, pass, resume, defer, assertNoProof,
    checkTable, signIn, logout, withCapturedResponse, assertNoContentResponse, checkpoint, verifyCookieDeadline, control, startRuntime, restartRuntime, runtimeReady, rateBudget,
    rawPublic, startCeremony, assertion, sessionCsrf, absent, cookieSnapshot, pendingFailure,
    holdResponse, holdNextRead: ({ context }) => holdResponse(PRODUCT_NATIVE.history, context),
    tls: { origin: productRuntimeConfig.origin, caPem: productRuntimeConfig.caPem },
    sourceRoot: process.cwd(), stagedRuntime: path.join(stage, "runtime"), runtimeDigest, manifest: productRuntimeManifest,
    runtime: productRuntimeConfig,
    publicFinishBody: (start, credential) => ({ ceremony_id: start.ceremony_id, credential }),
    publicLogoutBody: (context) => ({ browser_context: context }),
    recordCheck(id, check) {
      const item=scenarios.find((item)=>item.id===id);
      assert.ok(item); assert.equal(item.status,"running");
      assert.deepEqual(Object.keys(check).sort(),["evidence","id","status"]);
      assert.equal(check.status,"passed"); assert.match(check.id,/^[a-z0-9-]{1,64}$/);
      const serialized=JSON.stringify(check); assert.ok(Buffer.byteLength(serialized)<=4096);
      assertNoProof(serialized);
      item.checks ??= [];
      assert.ok(item.checks.length<16 && !item.checks.some((prior)=>prior.id===check.id));
      item.checks.push(JSON.parse(serialized));
    },
    scenarioResult(id, result) {
      const item = scenarios.find((item) => item.id === id);
      item.checks = result.checks ?? [];
      if (result.measured_guard_sha256) item.measured_guard_sha256=result.measured_guard_sha256;
      if (result.measured_guard_documents) item.measured_guard_documents=result.measured_guard_documents;
      if (result.complete || result.status === "passed") {
        item.status = "passed"; delete item.reason;
      } else {
        item.status = result.status === "failed" ? "failed" : "unreached";
        item.reason = result.reason ?? "required restore proof not reached";
        failureClass ??= item.status === "failed" ? "behavior" : "infrastructure";
      }
      activeScenario = undefined;
    },
  };
}

// Full P01–P17 source proposal; no execution/admission credit.
// This body is combined with the exact protected C2 IPC/resource-owner prefix.
const PRODUCT_PATHS = Object.freeze({
  login: "/login/", start: "/api/browser-session/start/",
  finish: "/api/browser-session/login/", logout: "/api/browser-session/logout/",
});
const UUID = "[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}";
const ATTENDANCE = new RegExp(`^/me/(${UUID})/attendance/$`);
const scenarios = [
  ["P01", "runtime/exclusion/launcher/config negatives"],
  ["P02", "two genuine product sign-ins and native family logout"],
  ["P03", "real own-history count, rows, pagination and reload"],
  ["P04", "responsive keyboard and inaccessible-script recovery"],
  ["P05", "two actual source peers and ingress spoofing negatives"],
  ["P06", "Origin/CSRF/context/duplicate-cookie/body boundaries"],
  ["P07", "eight-preauth/eight-session admission boundaries"],
  ["P08", "independent tabs, stable reads and no context fallback"],
  ["P09", "reversed actual start responses"],
  ["P10", "reversed actual finish/header-arrival responses"],
  ["P11", "reversed logout versus a new independent login"],
  ["P12", "lost committed login result and consumed-assertion handling"],
  ["P13", "lost logout across genuine short-family expiry"],
  ["P14", "current authority, key replacement and cleanup restart"],
  ["P15", "logout Back and witnessed real BFCache restore"],
  ["P16", "actual restore before hydration with CSS/React/offline faults"],
  ["P17", "real unavailable/malformed service and failure cleanup"],
].map(([id, name]) => ({ id, name, status: "unreached",
  reason: "not started" }));
const sessions = new Map();
const actors = new Map();
let activeScenario;
let failureClass;
let runtimeDigest;
let captureFailure; // Fatal observer faults cannot be accepted as intentional lost responses.
class InfrastructureError extends Error {
  constructor(message = "product infrastructure unavailable") { super(message); this.code = "R7_BROWSER_PREREQUISITE"; }
}
function begin(id) {
  const item = scenarios.find((item) => item.id === id);
  assert.equal(item.status, "unreached");
  item.status = "running"; delete item.reason;
  activeScenario = id;
}
function pass(id) {
  if (captureFailure) throw captureFailure;
  const item = scenarios.find((item) => item.id === id);
  assert.equal(item.status, "running");
  item.status = "passed";
  activeScenario = undefined;
}
function resume(id) {
  const item = scenarios.find((item) => item.id === id);
  assert.equal(item.status, "running"); activeScenario = id;
}
function defer(id) {
  const item = scenarios.find((item) => item.id === id);
  assert.equal(item.status, "running"); assert.equal(activeScenario, id);
  activeScenario = undefined;
}
function assertNoProof(value) {
  const text = typeof value === "string" ? value : JSON.stringify(value);
  assert.ok(!/(?:bs1|pa1|bi1)\.[A-Za-z0-9_-]+/.test(text));
  assert.ok(!/"(?:access_token|refresh_token|session_token)"\s*:/.test(text));
  assert.ok(!/[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}/.test(text));
  for (const secret of productRuntimeSecrets) assert.ok(!text.includes(secret), "browser output contains runtime key material");
}
async function checkpoint(actor, context, sessionToken, phase, cookieExpires, signedCounter) {
  await productExchange({ kind: "checkpoint", actor, context, session_token: sessionToken, phase,
    cookie_expires: cookieExpires, signed_counter: signedCounter }, (result) => {
    assert.deepEqual(Object.keys(result).sort(), ["actor", "context", "kind", "phase", "seq", "v"]);
    assert.equal(result.kind, "checked"); assert.equal(result.actor, actor);
    assert.equal(result.context, context); assert.equal(result.phase, phase);
  });
}
function kst(tuple) {
  assert.equal(tuple.length, 9);
  assert.ok(tuple.every(Number.isSafeInteger));
  const [year, ordinal, hour, minute, second, nano, oh, om, os] = tuple;
  assert.ok(year >= 1 && year <= 9999 && ordinal >= 1 && ordinal <= 366);
  assert.ok(hour >= 0 && hour <= 23 && minute >= 0 && minute <= 59 && second >= 0 && second <= 59);
  assert.ok(nano >= 0 && nano < 1e9 && Math.abs(oh) <= 23 && Math.abs(om) <= 59 && Math.abs(os) <= 59);
  // Fixture dates are modern. This independently computes the native tuple's instant.
  const ms = Date.UTC(year, 0, ordinal, hour, minute, second, Math.floor(nano / 1e6))
    - (oh * 3600 + om * 60 + os) * 1000 + 9 * 3600 * 1000;
  return new Date(ms).toISOString().slice(0, 19).replace("T", " ");
}
async function checkTable(actor, expected) {
  const { page, facts } = actors.get(actor);
  const table = page.getByRole("table");
  await table.waitFor({ state: "visible", timeout: PHASE_MS });
  const body = await page.locator("body").innerText();
  assert.ok(body.includes(facts.company_name)); assert.ok(body.includes(facts.account_name));
  assert.ok(new RegExp(`총\\s*${expected.total}\\s*건`).test(body));
  const other = actors.get(actor === "a" ? "b" : "a").facts;
  assert.ok(!body.includes(other.company_name) && !body.includes(other.account_name));
  const rows = table.locator("tbody tr");
  assert.equal(await rows.count(), expected.items.length);
  const pageNumber = Number(new URL(page.url()).searchParams.get("page") ?? "1");
  const first = expected.items.length ? (pageNumber - 1) * 25 + 1 : 0;
  const last = expected.items.length ? first + expected.items.length - 1 : 0;
  assert.ok(new RegExp(`${first}\\s*[-–~]\\s*${last}\\s*건`).test(body), "real native page range must be shown");
  assert.match(body, /한국\s*표준시|KST/); assert.match(body, /초\s*단위|소수\s*초.*(?:생략|표시하지)/);
  assert.match(body, /비고|메모/); assert.match(body, /(?:본인|직원).*(?:기록|입력)|(?:기록|입력).*(?:본인|직원)/,
    "note/source meaning must explain who recorded the history");
  assert.match(body, /(?:급여|자료\s*연결)[^\n]*(?:계산|산정)[^\n]*승인[^\n]*(?:지급|입금|정산)[^\n]*(?:아님|아니|의미하지|뜻하지|보장하지|확인할\s*수\s*없)/,
    "history must distinguish material linkage from payroll calculation, approval and payment");
  const kinds = { CLOCK_IN: "출근", OUT_FOR_WORK: "외근", BUSINESS_TRIP: "출장", RETURNED: "복귀", CLOCK_OUT: "퇴근" };
  const states = { CLOCKED_IN: /근무\s*중|출근\s*상태/, OUT_FOR_WORK: /외근\s*중|외근\s*상태/,
    BUSINESS_TRIP: /출장\s*중|출장\s*상태/, OFF_DUTY: /퇴근\s*상태|근무\s*종료/ };
  for (let index = 0; index < expected.items.length; index += 1) {
    const item = expected.items[index];
    assert.equal(typeof item.note, "string");
    assert.ok(item.note.length > 0);
    const visible = await rows.nth(index).innerText();
    assert.ok(visible.includes(item.note)); assert.ok(visible.includes(kst(item.occurred_at)));
    assert.ok(Object.hasOwn(kinds, item.kind) && Object.hasOwn(states, item.state_after));
    assert.ok(visible.includes(kinds[item.kind])); assert.match(visible, states[item.state_after]);
    assert.ok(["LINKED", "UNLINKED"].includes(item.payroll_link_status));
    assert.equal(item.payroll_material_ref_id !== null, item.payroll_link_status === "LINKED");
    assert.match(visible, item.payroll_link_status === "LINKED" ? /자료\s*연결됨|연결된\s*자료|자료\s*있음/ : /자료\s*미연결|연결된\s*자료\s*없음/,
      "each row must describe the actual material-reference linkage");
  }
  assertNoProof(await page.content());
  const storage = await page.evaluate(async () => ({
    local: Object.keys(localStorage), session: Object.keys(sessionStorage),
    indexed: (await indexedDB.databases()).map((database) => database.name),
    cache: await caches.keys(), service_workers: (await navigator.serviceWorker.getRegistrations()).length,
  }));
  assertNoProof(storage);
  assert.deepEqual(storage, { local: [], session: [], indexed: [], cache: [], service_workers: 0 });
}
function verifyCookieDeadline(reply, cookie, response, headers) {
  assert.equal(reply.context_id, cookie.name.slice("__Host-console-session-".length));
  const sessionHeaders = headers.filter((header) => header.name.toLowerCase() === "set-cookie"
    && header.value.startsWith(`${cookie.name}=`));
  assert.equal(sessionHeaders.length, 1);
  const expiry = sessionHeaders[0].value.split(";").map((part) => part.trim())
    .filter((part) => part.toLowerCase().startsWith("expires="));
  assert.equal(expiry.length, 1);
  const wireDate = expiry[0].slice("expires=".length);
  const cookieExpires = Date.parse(wireDate) / 1000;
  assert.ok(Number.isSafeInteger(cookieExpires));
  assert.equal(new Date(cookieExpires * 1000).toUTCString(), wireDate);
  assert.equal(cookieExpires, Math.floor(Date.parse(reply.expires_at) / 1000));
  assert.ok(cookieExpires > Date.now() / 1000);
  // Chromium may adjust expiry using the server Date and local receipt clock.
  // Preserve the observed jar deadline separately; never round it into custody.
  assert.ok(Number.isFinite(cookie.expires) && cookie.expires > Date.now() / 1000);
  const dates = headers.filter((header) => header.name.toLowerCase() === "date");
  assert.equal(dates.length, 1);
  const serverSeconds = Date.parse(dates[0].value) / 1000;
  assert.ok(Number.isSafeInteger(serverSeconds));
  assert.equal(new Date(serverSeconds * 1000).toUTCString(), dates[0].value);
  const requestMilliseconds = response.request().timing().startTime;
  const observedMilliseconds = Date.now();
  assert.ok(Number.isFinite(requestMilliseconds) && requestMilliseconds > 0 && requestMilliseconds <= observedMilliseconds);
  const adjustment = cookie.expires - cookieExpires;
  assert.ok(adjustment === 0 || (adjustment >= requestMilliseconds / 1000 - serverSeconds
    && adjustment <= observedMilliseconds / 1000 - serverSeconds), "browser deadline must be explained by actual Date/request/receipt clocks");
  return cookieExpires;
}
async function withCapturedResponse(actor, pathname, operation) {
  if (captureFailure) throw captureFailure;
  const entry = actors.get(actor);
  // Navigation can drop no-store body handles; unread fetch bodies can also
  // defer Chromium completion. Capture real bytes while paused, then continue
  // without overrides. This observer pause supplies no untouched-latency proof.
  const captured = Promise.withResolvers();
  void captured.promise.catch(() => {});
  const captures = [];
  let captureCount = 0;
  let closingCapture = false;
  let setupSettled = false;
  const bounded = async (promise, milliseconds = 2000) => {
    let timer;
    try {
      return await Promise.race([promise, new Promise((_, reject) => {
        timer = setTimeout(() => reject(new InfrastructureError("response observer command deadline")), milliseconds);
      })]);
    } finally { clearTimeout(timer); }
  };
  const observe = (event) => {
    const task = (async () => {
      try {
        captureCount += 1;
        if (closingCapture) return; // Release late pauses, without starting another body command.
        assert.equal(captureCount, 1);
        assert.equal(event.request.url, `${entry.origin}${pathname}`);
        assert.equal(event.request.method, "POST");
        assert.ok(!event.responseErrorReason);
        const wire = await entry.cdp.send("Fetch.getResponseBody", { requestId: event.requestId });
        assert.ok(Buffer.byteLength(wire.body) <= MAX_FRAME * 2);
        const bytes = Buffer.from(wire.body, wire.base64Encoded ? "base64" : "utf8");
        assert.ok(bytes.length <= MAX_FRAME);
        const body = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
        // Every captured body is checked, including a later browser abort.
        assertNoProof(body);
        return { body, postData: event.request.postData, status: event.responseStatusCode };
      } finally {
        await entry.cdp.send("Fetch.continueResponse", { requestId: event.requestId });
      }
    })();
    captures.push(task);
    void task.then(captured.resolve, captured.reject);
  };
  entry.cdp.on("Fetch.requestPaused", observe);
  const captureDeadline = setTimeout(() => {
    // Missing responses are expected in fault scenarios; keep the healthy
    // authenticator alive. Cleanup detaches only actually stuck commands.
    captured.reject(new InfrastructureError("actual response capture deadline"));
  }, PHASE_MS);
  let result;
  let requiredCapture = false;
  let primaryError;
  try {
    const setup = entry.cdp.send("Fetch.enable", { patterns: [{
      urlPattern: `${entry.origin}${pathname}`, requestStage: "Response",
    }] }).finally(() => { setupSettled = true; });
    await bounded(setup, PHASE_MS);
    result = await operation(() => { requiredCapture = true; return captured.promise; });
  } catch (error) { primaryError = error; }
  finally {
    closingCapture = true;
    clearTimeout(captureDeadline);
    let drained = false;
    let disabled = false;
    try {
      // A setup timeout cannot race a later enable success and look cleaned.
      assert.ok(setupSettled);
      await bounded(Promise.allSettled(captures));
      drained = true; // No getResponseBody can start after closingCapture.
      if (entry.page.isClosed()) {
        // P10 witnesses closing this page before the held response's headers.
        // The closed target owns teardown; it must have had no body commands.
        assert.equal(captures.length, 0);
        disabled = true;
      } else {
        await bounded(entry.cdp.send("Fetch.disable"));
        disabled = true;
      }
      // Keep the handler through disable so late pauses are released unchanged
      // and counted. Check every task, including those after first resolution.
      const results = await bounded(Promise.allSettled(captures));
      assert.ok(results.every((result) => result.status === "fulfilled"));
      assert.ok(captureCount <= 1);
      if (!primaryError && requiredCapture) assert.equal(captureCount, 1);
    } catch (error) {
      captureFailure ??= new InfrastructureError("response observer cleanup failed");
      captureFailure.cause = error;
      if (!drained || !disabled) {
        // Pending setup/body/disable needs positive teardown, never a competing
        // disable during getResponseBody. This is fatal to the whole matrix.
        try {
          await bounded(entry.cdp.detach());
          await bounded(Promise.allSettled(captures));
        } catch (detachError) { captureFailure.detachCause = detachError; }
      }
      if (primaryError) primaryError.observerFailure = captureFailure;
    } finally { entry.cdp.off("Fetch.requestPaused", observe); }
  }
  if (primaryError) throw primaryError;
  if (captureFailure) throw captureFailure;
  return result;
}

async function signIn(actor) {
  if (captureFailure) throw captureFailure;
  await rateBudget();
  const entry = actors.get(actor);
  const response = await entry.page.goto(`${entry.origin}${PRODUCT_PATHS.login}`, { timeout: PHASE_MS });
  assert.equal(response.status(), 200);
  assertNoProof(await response.text());
  const started = entry.page.waitForResponse((r) => new URL(r.url()).pathname === PRODUCT_PATHS.start
    && r.request().method() === "POST", { timeout: PHASE_MS });
  const finished = entry.page.waitForResponse((r) => new URL(r.url()).pathname === PRODUCT_PATHS.finish
    && r.request().method() === "POST", { timeout: PHASE_MS });
  const navigated = entry.page.waitForURL((url) => ATTENDANCE.test(url.pathname), { timeout: PHASE_MS });
  // Own every wait before the first asynchronous observer setup.
  for (const pending of [started, finished, navigated]) void pending.catch(() => {});
  let startBody;
  let finish;
  let finishReply;
  await withCapturedResponse(actor, PRODUCT_PATHS.finish, async (getResponse) => {
    await entry.page.getByRole("button", { name: "패스키로 로그인", exact: true }).click();
    const start = await started; assert.equal(start.status(), 200);
    startBody = await start.json(); assertNoProof(startBody);
    assert.match(startBody.ceremony_id, new RegExp(`^${UUID}$`));
    finish = await finished; assert.equal(finish.status(), 200);
    const wire = await getResponse();
    assert.equal(wire.status, finish.status());
    assert.equal(wire.postData, finish.request().postData());
    assert.equal(JSON.parse(wire.postData).ceremony_id, startBody.ceremony_id);
    assertNoProof(wire.body);
    finishReply = JSON.parse(wire.body);
    assert.deepEqual(Object.keys(finishReply).sort(), ["context_id", "expires_at"]);
    assert.equal(finishReply.context_id, startBody.ceremony_id);
  });
  const request = finish.request().postDataJSON();
  assert.equal(request.ceremony_id, startBody.ceremony_id);
  assert.equal(request.credential.id, entry.credentialId);
  await navigated;
  const context = new URL(entry.page.url()).pathname.match(ATTENDANCE)[1];
  assert.equal(context, startBody.ceremony_id);
  const cookie = (await entry.context.cookies()).find((c) => c.name === `__Host-console-session-${context}`);
  assert.ok(cookie && cookie.httpOnly && cookie.secure && cookie.path === "/" && cookie.sameSite === "Strict");
  assert.match(cookie.value, /^bs1\.[A-Za-z0-9_-]{43}$/);
  assert.equal(Buffer.from(cookie.value.slice(4), "base64url").length, 32);
  const finishHeaders = await finish.headersArray();
  const cookieObservedAt = Date.now() / 1000;
  const cookieExpires = verifyCookieDeadline(finishReply, cookie, finish, finishHeaders);
  const cookieHeaders = finishHeaders.filter((header) => header.name.toLowerCase() === "set-cookie");
  const sessionHeader = cookieHeaders.find((header) => header.value.startsWith(`__Host-console-session-${context}=`));
  assert.ok(sessionHeader && !/;\s*Domain=/i.test(sessionHeader.value));
  const signedData = Buffer.from(request.credential.response.authenticatorData, "base64url");
  assert.ok(signedData.length >= 37 && signedData.length <= MAX_FRAME);
  const signedCounter = signedData.readUInt32BE(33);
  sessions.set(actor, { context, token: cookie.value, cookieExpires, cookieObservedAt, browserCookieExpires: cookie.expires, signedCounter });
  await checkpoint(actor, context, cookie.value, "open", cookieExpires, signedCounter);
  await checkTable(actor, entry.facts.first_page);
}
async function assertNoContentResponse(response) {
  // HTTP 204 ends at its headers. Playwright's finished() never resolves for
  // post-header RequestFailed; actual framing, navigation and native closure
  // are checked independently rather than draining a nonexistent body.
  assert.equal(response.status(), 204);
  const headers = await response.allHeaders();
  assert.equal(headers["content-length"], undefined);
  assert.equal(headers["transfer-encoding"], undefined);
  assert.equal(headers["content-type"], undefined);
}
async function logout(actor) {
  const entry = actors.get(actor); const session = sessions.get(actor);
  const receipt = entry.page.waitForResponse((r) => new URL(r.url()).origin === entry.origin
    && new URL(r.url()).pathname === PRODUCT_PATHS.logout && r.request().method() === "POST"
    && r.request().postDataJSON()?.browser_context === session.context, { timeout: PHASE_MS })
    .then(assertNoContentResponse);
  const navigated = entry.page.waitForURL((url) => url.origin === entry.origin
    && url.pathname === PRODUCT_PATHS.login, { timeout: PHASE_MS });
  // Own both observers before the actual click; their original results remain required.
  void receipt.catch(() => {}); void navigated.catch(() => {});
  await entry.page.getByRole("button", { name: "로그아웃", exact: true }).click();
  await Promise.all([receipt, navigated]);
  await entry.page.getByRole("heading", { name: "패스키 로그인", exact: true }).waitFor({ timeout: PHASE_MS });
  assert.ok(!(await entry.context.cookies()).some((c) => c.name === `__Host-console-session-${session.context}`));
  await checkpoint(actor, session.context, session.token, "closed", session.cookieExpires, session.signedCounter);
  const denied = await entry.page.goto(`${entry.origin}/me/${session.context}/attendance/`, { timeout: PHASE_MS });
  const html = await denied.text(); assertNoProof(html);
  assert.ok(!html.includes(entry.facts.account_name) && !html.includes(entry.facts.company_name));
  const table = entry.page.getByRole("table");
  assert.ok(await table.count() === 0 || !await table.isVisible());
  // Do not infer a logout receipt from subsequent cookie absence: the receipt above is required.
}
let outcome = "failed";
let exitCode = 1;
try {
  const sourceRoot = process.cwd();
  const scenarioModules = await preflightScenarioModules(sourceRoot);
  const packagerPath = path.join(sourceRoot, "tools/production-runtime.mjs");
  let packager;
  try { packager = await import(pathToFileURL(packagerPath).href); } catch { throw new InfrastructureError(); }
  if (typeof packager.stageProductionRuntime !== "function" || typeof packager.verifyProductionRuntime !== "function") throw new InfrastructureError();
  startup = (async () => {
    stage = await mkdtemp(path.join(tmpdir(), "console-product-browser-"));
    const destination = path.join(stage, "runtime");
    const packaged = await packager.stageProductionRuntime({ sourceRoot, destination, signal: runtimeStagingAbort.signal });
    const verified = await packager.verifyProductionRuntime(destination);
    assert.equal(verified.version, 1); assert.deepEqual(verified, packaged);
    productRuntimeManifest = verified;
    runtimeDigest = createHash("sha256").update(JSON.stringify(verified)).digest("hex");
    if (!(await lstat(path.join(destination, "server.mjs"))).isFile()) throw new InfrastructureError();
    for (const forbidden of ["src", "backend-fork", "docs", "fixtures", ".env", ".env.local"]) {
      try { await lstat(path.join(destination, forbidden)); throw new InfrastructureError(); }
      catch (error) { if (error.code !== "ENOENT") throw error; }
    }
  })();
  await startup; assert.ok(!closing);
  reservation = createServer(); startup = once(reservation, "listening");
  reservation.listen(0, "127.0.0.1"); await startup; assert.ok(!closing);
  const port = reservation.address().port; const origin = `https://localhost:${port}`;
  await writeFrame({ kind: "origin", origin, runtime_digest: runtimeDigest });
  const configure = await readFrame();
  assert.deepEqual(Object.keys(configure).sort(), ["backend_origin", "ingress_key", "kind", "seq", "short_backend_origin", "tls_cert_pem", "tls_key_pem", "v"]);
  assert.equal(configure.kind, "configure");
  assert.match(configure.ingress_key, /^[A-Za-z0-9_-]{43}$/);
  const backend = new URL(configure.backend_origin);
  assert.equal(backend.protocol, "http:"); assert.equal(backend.hostname, "127.0.0.1");
  assert.equal(backend.pathname, "/"); assert.equal(backend.search + backend.hash + backend.username + backend.password, "");
  const certFile = path.join(stage, "fixture-cert.pem"); const keyFile = path.join(stage, "fixture-key.pem");
  await writeFile(certFile, configure.tls_cert_pem, { mode: 0o600 });
  await writeFile(keyFile, configure.tls_key_pem, { mode: 0o600 });
  await new Promise((resolve) => reservation.close(resolve)); assert.ok(!closing);
  productRuntimeConfig = { port, origin, backend: backend.origin, normalBackend: backend.origin,
    shortBackend: configure.short_backend_origin, ingress: configure.ingress_key,
    preauth: randomBytes(32).toString("base64url"), certFile, keyFile, caPem: configure.tls_cert_pem };
  configure.tls_key_pem = ""; configure.ingress_key = "";
  const initialRuntime = await startRuntime({ main: true }); await runtimeReady(initialRuntime);
  startup = chromium.launchServer({ channel: "chromium", env: cleanEnv,
    ignoreDefaultArgs: ["--disable-back-forward-cache"] }).then((server) => { browserServer = server; });
  await startup; assert.ok(!closing); browser = await chromium.connect(browserServer.wsEndpoint());
  await writeFrame({ kind: "ready", origin, runtime_digest: runtimeDigest });
  for (const actor of ["a", "b"]) {
    const command = await readFrame(60000);
    assert.deepEqual(Object.keys(command).sort(), ["actor", "ceremony", "kind", "mode", "options", "seq", "v"]);
    assert.match(command.ceremony, new RegExp(`^${UUID}$`));
    assert.equal(command.kind, "register"); assert.equal(command.actor, actor); assert.equal(command.mode, "discoverable");
    const selection = command.options.publicKey.authenticatorSelection;
    assert.equal(selection.residentKey, "required"); assert.equal(selection.requireResidentKey, true);
    assert.equal(selection.userVerification, "required"); assert.equal(command.options.publicKey.rp.id, "localhost");
    productSharedContext ??= await browser.newContext({ ignoreHTTPSErrors: true });
    const context = productSharedContext;
    const page = await context.newPage();
    const prerequisite = await page.goto(`${origin}/_not-found/`, { timeout: PHASE_MS });
    assert.equal(prerequisite.status(), 404);
    assert.equal(new URL(page.url()).origin, origin);
    assert.equal(await page.evaluate(() => isSecureContext), true);
    const cdp = await context.newCDPSession(page); await cdp.send("WebAuthn.enable");
    const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
      options: { protocol: "ctap2", transport: "internal", hasResidentKey: true,
        hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
    });
    creationAttempts += 1;
    const result = await page.evaluate(async (options) => {
      const credential = await navigator.credentials.create({ ...options,
        publicKey: PublicKeyCredential.parseCreationOptionsFromJSON(options.publicKey),
        signal: AbortSignal.timeout(15000) });
      return { credential: credential.toJSON(), resident_key: credential.getClientExtensionResults().credProps?.rk ?? null };
    }, command.options);
    assert.ok(result.resident_key === null || typeof result.resident_key === "boolean");
    await writeFrame({ kind: "registered", actor, ceremony: command.ceremony, ...result });
    const login = await readFrame(60000);
    assert.deepEqual(Object.keys(login).sort(), ["actor", "ceremony", "kind", "options", "seq", "v"]);
    assert.match(login.ceremony, new RegExp(`^${UUID}$`));
    assert.equal(login.kind, "login"); assert.equal(login.actor, actor);
    assert.equal(Object.hasOwn(login.options, "mediation"), false);
    assert.equal(login.options.publicKey.rpId, "localhost"); assert.deepEqual(login.options.publicKey.allowCredentials, []);
    assert.equal(login.options.publicKey.userVerification, "required");
    const { credentialId, rpId, isResidentCredential } = (await cdp.send("WebAuthn.getCredential", {
      authenticatorId, credentialId: Buffer.from(result.credential.id, "base64url").toString("base64"),
    })).credential;
    assert.ok(Buffer.from(credentialId, "base64").equals(Buffer.from(result.credential.id, "base64url")));
    assert.equal(rpId, "localhost"); assert.equal(isResidentCredential, true);
    const attempted = await page.evaluate(async (options) => {
      const credential = await navigator.credentials.get({ ...options,
        publicKey: PublicKeyCredential.parseRequestOptionsFromJSON(options.publicKey), signal: AbortSignal.timeout(15000) });
      return { outcome: "assertion", credential: credential.toJSON() };
    }, login.options);
    const environment = await page.evaluate(() => ({ secure_context: isSecureContext,
      document_has_focus: document.hasFocus(), visibility_state: document.visibilityState,
      conditional_mediation_available: null }));
    await writeFrame({ kind: "login-result", actor, ceremony: login.ceremony, ...attempted,
      authenticator_is_resident: isResidentCredential, browser_environment: environment });
    const facts = await readFrame(60000); assert.equal(facts.kind, "fixture"); assert.equal(facts.actor, actor);
    const entry = { context, page, cdp, authenticatorId, origin, credentialId: result.credential.id, facts };
    page.on("request", (request) => {
      if (new URL(request.url()).pathname === PRODUCT_PATHS.finish && request.method() === "POST") entry.lastFinishRequest = request;
    });
    actors.set(actor, entry);
  }
  begin("P02"); await signIn("a"); await signIn("b");
  assert.notEqual(sessions.get("a").context, sessions.get("b").context);
  begin("P03");
  const a = actors.get("a");
  const nextPage = a.page.waitForURL((url) => url.pathname === `/me/${sessions.get("a").context}/attendance/`
    && url.searchParams.get("page") === "2", { timeout: PHASE_MS });
  void nextPage.catch(() => {}); // A missing/disabled link is still a checked behavioral failure.
  await a.page.getByRole("link", { name: "다음", exact: true }).click(); await nextPage;
  await checkTable("a", a.facts.second_page);
  const reloaded = await a.page.reload({ timeout: PHASE_MS }); assertNoProof(await reloaded.text());
  await checkTable("a", a.facts.second_page);
  await checkpoint("a", sessions.get("a").context, sessions.get("a").token, "open",
    sessions.get("a").cookieExpires, sessions.get("a").signedCounter);
  const firstPage = a.page.waitForURL((url) => url.pathname === `/me/${sessions.get("a").context}/attendance/`
    && [null, "1"].includes(url.searchParams.get("page")), { timeout: PHASE_MS });
  void firstPage.catch(() => {});
  await a.page.getByRole("link", { name: "처음", exact: true }).click(); await firstPage;
  await checkTable("a", a.facts.first_page); await checkTable("b", actors.get("b").facts.first_page);
  const harness = proposalHarness();
  const { boundary, temporal, restore } = scenarioModules;
  await boundary.historyVariants(harness); pass("P03");
  await boundary.runBoundaryScenarios(harness);
  activeScenario = "P02";
  await logout("a"); await checkTable("b", actors.get("b").facts.first_page); await logout("b");
  pass("P02");
  await temporal.runTemporalScenarios(harness);
  await signIn("a"); await signIn("b");
  const restoreReports = await restore.runRestoreScenarios(harness);
  for (const report of restoreReports) harness.scenarioResult(report.id, report);
  if (sessions.get("b")) await logout("b");
  await temporal.runUnavailableScenarios(harness);
  await temporal.runFinalCloseScenario(harness);
} catch (error) {
  failureClass = error instanceof InfrastructureError || error.code === "R7_BROWSER_PREREQUISITE" || !activeScenario ? "infrastructure" : "behavior";
  if (activeScenario) {
    const item = scenarios.find((item) => item.id === activeScenario);
    item.status = failureClass === "infrastructure" ? "unreached" : "failed"; item.reason = failureClass === "infrastructure" ? "required real-service prerequisite unavailable" : "actual assertion or required journey failed";
    const location = browserFailureLocation(error);
    if (location) item.failure_location = location;
  }
  process.stderr.write(`product browser proposal ${failureClass} failure\n`);
} finally {
  for (const item of scenarios) if (item.status === "running") {
    item.status = "unreached"; item.reason = "scenario prerequisites did not complete";
  }
  const counts = Object.fromEntries(["passed", "failed", "unreached", "skipped"].map((key) => [key, scenarios.filter((item) => item.status === key).length]));
  try {
    await productExchange({ kind: "report", discovered: scenarios.length,
      executed: counts.passed + counts.failed, counts, failure_class: failureClass ?? null, scenarios }, (done) => {
      assert.deepEqual(Object.keys(done).sort(), ["kind", "seq", "v"]); assert.equal(done.kind, "done");
    });
  } catch { failureClass ??= "infrastructure"; /* Owned cleanup still runs; no completed transport is claimed. */ }
  // Required unauthored/partial scenarios deliberately prevent this draft from passing.
  if (counts.passed === scenarios.length && !failureClass) { outcome = "completed"; exitCode = 0; }
  clearTimeout(watchdog); clearTimeout(productWatchdog);
  await finalize(outcome, exitCode);
}
