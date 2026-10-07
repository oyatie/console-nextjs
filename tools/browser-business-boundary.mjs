// Additive R7c real-service test proposal. This file is not production code.
// Root owns integration, execution, resources and all candidate mutations.
import assert from "node:assert/strict";
import { createHash, randomUUID } from "node:crypto";
import { readFile, readdir } from "node:fs/promises";
import { request as httpsRequest } from "node:https";
import { request as httpRequest } from "node:http";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { pathToFileURL } from "node:url";

const UUID = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const SESSION = /^__Host-console-session-([0-9a-f-]{36})$/;
const PREAUTH = /^__Host-console-preauth-([0-9a-f-]{36})$/;
const MAX_RESPONSE = 2 * 1024 * 1024;
const REQUEST_MS = 20_000;
const POLL_MS = 1000;
const NATIVE_START = "/api/v1/auth/browser-session/start";

export class BoundaryPrerequisiteError extends Error {
  constructor(prerequisite) {
    super(`unreached browser prerequisite: ${prerequisite}`);
    this.code = "R7_BROWSER_PREREQUISITE";
  }
}

export const boundaryRequirements = Object.freeze({
  scenarioIds: ["P01", "P04", "P05", "P06", "P07"],
  historyVariantOwner: "P03",
  publicContract: {
    start: ["ceremony_id", "challenge", "expires_at", "csrf_token"],
    finishBody: ["ceremony_id", "credential"],
    finishReply: ["context_id", "expires_at"],
    logoutBody: ["browser_context"],
    mutationHeader: "x-csrf-token",
  },
  requiredControlOps: ["effects", "fixture-history", "inspect-ip", "rate-status"],
  // Actual elapsed time, never a clock override or rate-table rewrite.
  maximumRateWaitMs: 70_000,
  maximumPreauthExpiryWaitMs: 70_000,
});

function requireHarness(h) {
  for (const name of ["begin", "pass", "assertNoProof", "checkTable", "checkpoint", "control", "startRuntime"]) {
    if (typeof h[name] !== "function") throw new BoundaryPrerequisiteError(name);
  }
  if (!(h.actors instanceof Map) || !(h.sessions instanceof Map)) throw new BoundaryPrerequisiteError("actual actors/sessions");
  if (!h.tls?.origin || !h.tls?.caPem || !h.sourceRoot || !h.stagedRuntime || !h.manifest) {
    throw new BoundaryPrerequisiteError("owned staged runtime and TLS configuration");
  }
  assert.equal(h.actors.get("a").context, h.actors.get("b").context,
    "the genuine actors must exercise the same browser cookie jar");
}

// Connect directly to the supported Node entry. socket.localAddress witnesses
// the source peer independently of all caller-controlled forwarding headers.
export async function directRequest(tls, pathname, { method = "GET", headers = {}, body, family = 4 } = {}) {
  const origin = new URL(tls.origin);
  assert.ok(["http:", "https:"].includes(origin.protocol));
  assert.equal(origin.hostname, "localhost");
  assert.equal(origin.pathname, "/");
  assert.ok(pathname.startsWith("/") && !pathname.startsWith("//"));
  const localAddress = family === 6 ? "::1" : "127.0.0.1";
  const bytes = body === undefined ? undefined : Buffer.from(typeof body === "string" ? body : JSON.stringify(body));
  const transport = origin.protocol === "https:" ? httpsRequest : httpRequest;
  const result = await new Promise((resolve, reject) => {
    let peer;
    const request = transport(new URL(pathname, origin), {
      family, localAddress, ca: tls.caPem, rejectUnauthorized: true, agent: false,
      method, headers: { ...headers, ...(bytes ? { "content-length": String(bytes.length) } : {}) },
    }, (response) => {
      const chunks = [];
      let received = 0;
      response.on("data", (chunk) => {
        received += chunk.length;
        if (received > MAX_RESPONSE) response.destroy(new Error("bounded real response exceeded"));
        else chunks.push(chunk);
      });
      response.once("error", reject);
      response.once("end", () => resolve({ status: response.statusCode, headers: response.headers,
        rawHeaders: response.rawHeaders, body: Buffer.concat(chunks), peer }));
    });
    request.on("socket", (socket) => {
      socket.once(origin.protocol === "https:" ? "secureConnect" : "connect", () => {
        peer = { localAddress: socket.localAddress, remoteAddress: socket.remoteAddress,
          encrypted: Boolean(socket.encrypted) };
      });
    });
    request.setTimeout(REQUEST_MS, () => request.destroy(new Error("real socket deadline")));
    request.once("error", reject);
    request.end(bytes);
  });
  assert.equal(result.peer?.localAddress, localAddress);
  if (origin.protocol === "https:") assert.equal(result.peer.encrypted, true);
  return result;
}

async function digest(h) {
  const result = await h.control({ op: "effects" });
  assert.deepEqual(Object.keys(result).sort(), ["digest"]);
  assert.match(result.digest, /^[0-9a-f]{64}$/);
  return result.digest;
}

function cookieHeader(cookies) { return cookies.map((cookie) => `${cookie.name}=${cookie.value}`).join("; "); }
function cookieSnapshot(cookies) { return cookies.toSorted((a, b) => a.name.localeCompare(b.name)); }
async function actualCookies(entry) { return entry.context.cookies(entry.origin); }
function sessionCookie(cookies, context) {
  assert.match(context, UUID);
  const found = cookies.filter((cookie) => cookie.name === `__Host-console-session-${context}`);
  assert.equal(found.length, 1);
  assert.match(found[0].value, /^bs1\.[A-Za-z0-9_-]{43}$/);
  return found[0];
}
function sessionCsrf(cookie, context) {
  const handle = Buffer.from(cookie.value.slice(4), "base64url");
  assert.equal(handle.length, 32);
  assert.equal(handle.toString("base64url"), cookie.value.slice(4));
  const uuid = Buffer.from(context.replaceAll("-", ""), "hex");
  assert.equal(uuid.length, 16);
  return createHash("sha256").update("console/session-csrf/v1\0").update(handle).update(uuid).digest("base64url");
}
function noSetCookie(response) {
  assert.ok(!response.rawHeaders.some((value, index) => index % 2 === 0 && value.toLowerCase() === "set-cookie"),
    "a denied request may not clear, replace or create a cookie");
}
function noPrivateFacts(h, body) {
  const text = typeof body === "string" ? body : body.toString("utf8");
  h.assertNoProof(text);
  for (const { facts } of h.actors.values()) {
    assert.ok(!text.includes(facts.account_name) && !text.includes(facts.company_name),
      "denied or unavailable output cannot contain a Company or Account fact");
    for (const item of facts.first_page.items) assert.ok(!text.includes(item.note));
  }
}
async function deniedMutation(h, pathname, options) {
  const before = await digest(h);
  const response = await directRequest(h.tls, pathname, options);
  assert.ok(response.status >= 400 && response.status < 500, "invalid public mutation must be denied");
  noSetCookie(response); noPrivateFacts(h, response.body);
  assert.equal(await digest(h), before, "invalid request may not reach a native business/auth effect");
  return response;
}

async function browserMutation(h, actor, pathname, { csrf, body, rawBody } = {}) {
  const entry = h.actors.get(actor);
  const result = await entry.page.evaluate(async ({ pathname, csrf, body, rawBody }) => {
    const response = await fetch(pathname, { method: "POST", credentials: "same-origin", cache: "no-store",
      headers: { ...(csrf ? { "x-csrf-token": csrf } : {}), ...(body === undefined && rawBody === undefined ? {} : { "content-type": "application/json" }) },
      ...(body === undefined && rawBody === undefined ? {} : { body: rawBody ?? JSON.stringify(body) }) });
    return { status: response.status, text: await response.text() };
  }, { pathname, csrf, body, rawBody });
  h.assertNoProof(result.text);
  return result;
}

async function startPublic(h, actor) {
  const result = await browserMutation(h, actor, h.paths.start);
  assert.equal(result.status, 200);
  const start = JSON.parse(result.text);
  assert.deepEqual(Object.keys(start).sort(), boundaryRequirements.publicContract.start.toSorted());
  assert.match(start.ceremony_id, UUID);
  assert.match(start.csrf_token, /^[A-Za-z0-9_-]{43}$/);
  const cookie = (await actualCookies(h.actors.get(actor))).find((c) => c.name === `__Host-console-preauth-${start.ceremony_id}`);
  assert.ok(cookie?.httpOnly && cookie.secure && cookie.sameSite === "Strict" && cookie.path === "/");
  assert.match(cookie.value, /^pa1\.[A-Za-z0-9_-]+$/);
  assert.ok(cookie.value.length <= 1024);
  return start;
}

async function genuineAssertion(h, actor, start) {
  const entry = h.actors.get(actor);
  assert.equal(Object.hasOwn(start.challenge, "mediation"), false);
  assert.deepEqual(start.challenge.publicKey.allowCredentials, []);
  assert.equal(start.challenge.publicKey.userVerification, "required");
  const credential = await entry.page.evaluate(async (challenge) => {
    const credential = await navigator.credentials.get({ ...challenge,
      publicKey: PublicKeyCredential.parseRequestOptionsFromJSON(challenge.publicKey),
      signal: AbortSignal.timeout(15_000) });
    return credential.toJSON();
  }, start.challenge);
  assert.equal(credential.id, entry.credentialId);
  return { ceremony_id: start.ceremony_id, credential };
}

async function finishPublic(h, actor, start, signed, { rawBody } = {}) {
  const entry = h.actors.get(actor);
  const before = await actualCookies(entry);
  const observed = entry.page.waitForResponse((response) => response.url() === `${entry.origin}${h.paths.finish}`
    && response.request().method() === "POST" && response.request().postDataJSON()?.ceremony_id === start.ceremony_id);
  void observed.catch(() => {}); // Own rejection if the actual browser mutation fails.
  const result = await browserMutation(h, actor, h.paths.finish, { csrf: start.csrf_token, body: signed, rawBody });
  const response = await observed;
  assert.equal(result.status, 200);
  const reply = JSON.parse(result.text);
  assert.deepEqual(Object.keys(reply).sort(), ["context_id", "expires_at"]);
  assert.equal(reply.context_id, start.ceremony_id);
  const cookies = await actualCookies(entry);
  const cookie = sessionCookie(cookies, reply.context_id);
  assert.ok(cookie.httpOnly && cookie.secure && cookie.sameSite === "Strict" && cookie.path === "/");
  const cookieExpires = h.verifyCookieDeadline(reply, cookie, response, await response.headersArray());
  for (const old of before.filter((c) => SESSION.test(c.name))) {
    assert.ok(cookies.some((c) => c.name === old.name && c.value === old.value && c.expires === old.expires),
      "finishing an admitted ceremony must preserve every older session cookie");
  }
  const data = Buffer.from(signed.credential.response.authenticatorData, "base64url");
  assert.ok(data.length >= 37);
  const session = { context: reply.context_id, token: cookie.value, cookieExpires, browserCookieExpires: cookie.expires,
    signedCounter: data.readUInt32BE(33) };
  await h.checkpoint(actor, session.context, session.token, "open", session.cookieExpires, session.signedCounter);
  return session;
}

async function closeCreated(h, actor, session) {
  const entry = h.actors.get(actor);
  const before = await actualCookies(entry);
  const cookie = sessionCookie(before, session.context);
  const result = await browserMutation(h, actor, h.paths.logout, {
    csrf: sessionCsrf(cookie, session.context), body: { browser_context: session.context },
  });
  assert.equal(result.status, 204); assert.equal(result.text, "");
  const after = await actualCookies(entry);
  assert.ok(!after.some((c) => c.name === cookie.name));
  for (const old of before.filter((c) => SESSION.test(c.name) && c.name !== cookie.name)) {
    assert.ok(after.some((c) => c.name === old.name && c.value === old.value && c.expires === old.expires));
  }
  await h.checkpoint(actor, session.context, session.token, "closed", session.cookieExpires, session.signedCounter);
}

async function rateStatus(h, device) {
  const result = await h.control({ op: "rate-status", device: device ?? null });
  assert.deepEqual(Object.keys(result).sort(), ["attempts", "now", "window_end", "window_start"]);
  assert.ok(Number.isSafeInteger(result.now) && Number.isSafeInteger(result.window_end) && Number.isSafeInteger(result.window_start));
  assert.equal(result.window_end - result.window_start, 60);
  assert.ok(result.window_end > result.now && result.now >= result.window_start);
  for (const key of ["ip:127.0.0.1", "ip:::1", "global", ...(device ? [`dev:${device}`] : [])]) {
    assert.ok(Number.isSafeInteger(result.attempts[key]) && result.attempts[key] >= 0);
  }
  return result;
}

async function waitStartBudget(h, requiredSlots, minimumRemainingSeconds = 0) {
  const deadline = Date.now() + boundaryRequirements.maximumRateWaitMs;
  while (true) {
    const result = await rateStatus(h);
    if (result.window_end - result.now > minimumRemainingSeconds
        && result.attempts["ip:127.0.0.1"] <= 10 - requiredSlots && result.attempts["ip:::1"] <= 10 - requiredSlots
        && result.attempts.global <= 100 - requiredSlots) return;
    assert.ok(Date.now() < deadline, "actual rate window did not become available within bounded wait");
    await delay(POLL_MS);
  }
}

export async function runtimeBoundaries(h) {
  const packager = await import(pathToFileURL(path.join(h.sourceRoot, "tools/production-runtime.mjs")).href)
    .catch(() => { throw new BoundaryPrerequisiteError("production runtime packager"); });
  if (typeof packager.verifyProductionRuntime !== "function") throw new BoundaryPrerequisiteError("runtime verifier");
  const verified = await packager.verifyProductionRuntime(h.stagedRuntime);
  assert.deepEqual(verified, h.manifest);
  const appPaths = JSON.parse(await readFile(path.join(h.stagedRuntime, ".next/server/app-paths-manifest.json"), "utf8"));
  assert.ok(!Object.keys(appPaths).some((route) => route.includes("(console)")));
  const pages = (await readdir(path.join(h.sourceRoot, "src/app/(console)"), { recursive: true }))
    .filter((file) => file.endsWith("/page.tsx")).map((file) => `/${file.slice(0, -"/page.tsx".length)}/`);
  assert.ok(pages.length > 0);
  for (const pathname of pages) {
    const response = await directRequest(h.tls, pathname);
    assert.equal(response.status, 404);
    noPrivateFacts(h, response.body);
    for (const marker of ["박지영 수석", "AP-3121", "시연용 화면"]) assert.ok(!response.body.toString("utf8").includes(marker));
  }
  // Each launch owns a separate port/process and closes it before returning.
  // These are actual unsupported/misconfigured serving paths, not source grep.
  for (const configuration of [
    { launcher: "next-start", overrides: {} },
    { launcher: "custom", overrides: { CONSOLE_BROWSER_INGRESS_KEY: "" } },
    { launcher: "custom", overrides: { CONSOLE_BROWSER_INGRESS_KEY: "bi1.invalid" } },
    { launcher: "custom", overrides: { CONSOLE_BROWSER_PREAUTH_KEY: "" } },
    { launcher: "custom", overrides: { CONSOLE_BROWSER_PREAUTH_KEY: "invalid" } },
  ]) {
    const before = await digest(h);
    const instance = await h.startRuntime(configuration);
    if (!instance?.close || !instance?.tls) throw new BoundaryPrerequisiteError("owned alternate launcher");
    try {
      const response = await directRequest(instance.tls, h.paths.start, { method: "POST", headers: { origin: instance.tls.origin } });
      assert.ok(response.status >= 400 && response.status < 600);
      noSetCookie(response); noPrivateFacts(h, response.body);
      assert.equal(await digest(h), before);
    } finally { await instance.close(); }
  }
}

export async function historyVariants(h) {
  const entry = h.actors.get("a"); const session = h.sessions.get("a");
  const original = entry.facts;
  const before = await digest(h);
  await entry.page.goto(`${entry.origin}/me/${session.context}/attendance/?page=3`, { waitUntil: "domcontentloaded" });
  await h.checkTable("a", { ...original.first_page, items: [] });
  const out = await entry.page.locator("body").innerText();
  assert.match(out, new RegExp(`총\\s*${original.first_page.total}\\s*건`));
  assert.ok(!out.includes("총 0건") && !out.includes("기록이 없습니다"));
  assert.ok(await entry.page.getByRole("table").locator("tbody tr").count() === 0);
  const reset = entry.page.waitForNavigation({ waitUntil: "domcontentloaded" });
  void reset.catch(() => {});
  await entry.page.getByRole("link", { name: "처음", exact: true }).click(); await reset;
  await h.checkTable("a", original.first_page);
  assert.equal(await digest(h), before);
  try {
    for (const mode of ["linked-empty", "unlinked"]) {
      const facts = await h.control({ op: "fixture-history", actor: "a", mode });
      assert.equal(facts.actor, "a"); assert.equal(facts.employee_linked, mode === "linked-empty");
      assert.equal(facts.first_page.total, 0); assert.deepEqual(facts.first_page.items, []);
      entry.facts = { ...original, ...facts };
      await entry.page.goto(`${entry.origin}/me/${session.context}/attendance/`, { waitUntil: "domcontentloaded" });
      await h.checkTable("a", facts.first_page);
      const text = await entry.page.locator("body").innerText();
      assert.ok(text.includes(facts.company_name) && text.includes(facts.account_name));
      assert.match(text, mode === "linked-empty" ? /근태 기록이 없습니다/ : /계정에 연결된 직원 정보가 없습니다/);
      assert.ok(await entry.page.getByRole("table").locator("tbody tr").count() === 0);
      h.assertNoProof(await entry.page.content());
      await h.checkpoint("a", session.context, session.token, "open", session.cookieExpires, session.signedCounter);
    }
  } finally {
    entry.facts = { ...original, ...await h.control({ op: "fixture-history", actor: "a", mode: "populated" }) };
  }
  await entry.page.goto(`${entry.origin}/me/${session.context}/attendance/`);
  await h.checkTable("a", original.first_page);
  await h.checkTable("b", h.actors.get("b").facts.first_page);
}

export async function focusByKeyboard(page, role, name) {
  const target = page.getByRole(role, { name, exact: true });
  assert.equal(await target.count(), 1);
  for (let attempt = 0; attempt < 60; attempt++) {
    if (await target.evaluate((element) => element === document.activeElement)) {
      const focus = await target.evaluate((element) => {
        const style = getComputedStyle(element);
        return (style.outlineStyle !== "none" && parseFloat(style.outlineWidth) > 0) || style.boxShadow !== "none";
      });
      assert.ok(focus, "keyboard focus must have a visible indicator");
      if (role === "button") {
        const outline = await target.evaluate((element) => {
          const style = getComputedStyle(element);
          return { style: style.outlineStyle, width: style.outlineWidth, color: style.outlineColor, offset: style.outlineOffset };
        });
        assert.equal(outline.style, "solid");
        assert.ok(parseFloat(outline.width) >= 3 && parseFloat(outline.offset) >= 3);
        assert.equal(outline.color, "rgb(14, 116, 144)", "button focus outline contrasts at least 3:1 with both actual light surfaces");
      }
      return;
    }
    await page.keyboard.press("Tab");
  }
  assert.fail("required control was not reachable by keyboard");
}

export async function responsiveRecovery(h) {
  const entry = h.actors.get("a"); const session = h.sessions.get("a");
  for (const width of [360, 390, 768, 1280, 1440]) {
    await entry.page.setViewportSize({ width, height: 900 });
    await entry.page.goto(`${entry.origin}/me/${session.context}/attendance/`);
    await h.checkTable("a", entry.facts.first_page);
    assert.equal(await entry.page.getByRole("heading", { level: 1, name: "나의 근태 기록", exact: true }).count(), 1);
    assert.equal(await entry.page.getByRole("region", { name: /근태.*기록/ }).count(), 1);
    assert.equal(await entry.page.getByRole("table", { name: /근태.*기록/ }).count(), 1);
    const fits = await entry.page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1);
    assert.ok(fits, "history scrolling must remain within its labeled region");
    await focusByKeyboard(entry.page, "button", "로그아웃");
    await focusByKeyboard(entry.page, "link", "다음");
    const navigated = entry.page.waitForNavigation({ waitUntil: "domcontentloaded" });
    void navigated.catch(() => {});
    await entry.page.keyboard.press("Enter"); await navigated;
    await h.checkTable("a", entry.facts.second_page);
  }
  try {
    await entry.cdp.send("Emulation.setScriptExecutionDisabled", { value: true });
    await entry.page.goto(`${entry.origin}/me/${session.context}/attendance/`, { waitUntil: "domcontentloaded" });
    const visible = await entry.page.locator("body").innerText();
    assert.ok(!visible.includes(entry.facts.company_name) && !visible.includes(entry.facts.account_name));
    assert.match(visible, /자바스크립트|JavaScript|스크립트/);
    assert.ok(await entry.page.getByRole("link", { name: /다시|새로|로그인/ }).count() > 0);
    for (const table of await entry.page.getByRole("table").all()) assert.equal(await table.isVisible(), false);
  } finally { await entry.cdp.send("Emulation.setScriptExecutionDisabled", { value: false }); }
  await entry.page.goto(`${entry.origin}/me/${session.context}/attendance/`);
  await h.checkTable("a", entry.facts.first_page);
}

export async function socketProvenance(h) {
  await waitStartBudget(h, 6, 20);
  const device = `r7-peer-${randomUUID()}`;
  const initial = await rateStatus(h, device);
  assert.equal(initial.attempts[`dev:${device}`], 0);
  const probes = [
    { family: 4, expected: "127.0.0.1", headers: {} },
    { family: 6, expected: "::1", headers: {} },
    { family: 4, expected: "127.0.0.1", headers: { "x-forwarded-for": "203.0.113.77",
      forwarded: "for=203.0.113.77;proto=http", "x-real-ip": "203.0.113.77",
      "x-console-browser-ingress": "bi1.caller-supplied",
      "x-console-node-peer": "::ffff:203.0.113.77", "x-console-node-secure": "0",
      "x-console-node-ingress": "caller-supplied-internal-proof" } },
    { family: 6, expected: "::1", headers: { "x-forwarded-for": ["203.0.113.77", "198.51.100.88"],
      "x-forwarded-host": "attacker.invalid", "x-forwarded-proto": "http",
      "x-console-browser-ingress": ["bi1.caller-first", "bi1.caller-second"],
      "x-console-node-peer": ["127.0.0.1", "::ffff:198.51.100.88"],
      "x-console-node-secure": ["1", "0"],
      "x-console-node-ingress": ["caller-first-internal-proof", "caller-second-internal-proof"] } },
  ];
  // Six further starts alternate the two actual peers while retaining one
  // actual device bucket. The eleventh trips only that shared device ceiling.
  const internalSpoofs = [
    { "x-console-node-peer": "not-an-ip", "x-console-node-secure": "not-a-boolean", "x-console-node-ingress": "" },
    { "x-console-node-peer": "::ffff:198.51.100.88", "x-console-node-secure": "TRUE", "x-console-node-ingress": "bi1.wrong-codec" },
    { "x-console-node-peer": "127.0.0.1:443", "x-console-node-secure": "1, 0", "x-console-node-ingress": "invalid.with.extra.parts" },
    { "x-console-node-peer": "[::1]:443", "x-console-node-secure": "false", "x-console-node-ingress": ["", "second-invalid-proof"] },
    { "x-console-node-peer": ["malformed", "::ffff:203.0.113.77"], "x-console-node-secure": ["false", "true"], "x-console-node-ingress": ["first", "second"] },
    { "x-console-node-peer": "203.0.113.77, 198.51.100.88", "x-console-node-secure": " 0 ", "x-console-node-ingress": "noncanonical-proof=" },
  ];
  for (let index = 0; index < internalSpoofs.length; index++) probes.push({
    family: index % 2 === 0 ? 4 : 6, expected: index % 2 === 0 ? "127.0.0.1" : "::1", headers: internalSpoofs[index],
  });
  for (const probe of probes) {
    const response = await directRequest(h.tls, h.paths.start, { method: "POST", family: probe.family,
      headers: { origin: h.tls.origin, "x-device-id": device, ...probe.headers } });
    assert.equal(response.status, 200);
    assert.equal(response.peer.localAddress, probe.expected);
    h.assertNoProof(response.body.toString("utf8"));
    const publicStart = JSON.parse(response.body.toString("utf8"));
    assert.match(publicStart.ceremony_id, UUID);
    const witness = await h.control({ op: "inspect-ip", ceremony: publicStart.ceremony_id });
    assert.equal(witness.ceremony, publicStart.ceremony_id);
    assert.deepEqual(witness.forwarded, [probe.expected]);
    assert.match(witness.native_peer, /^127\.0\.0\.1:[1-9][0-9]*$/);
    if (witness.native_path !== undefined) assert.equal(witness.native_path, NATIVE_START);
  }
  const admitted = await rateStatus(h, device);
  assert.equal(admitted.window_start, initial.window_start, "peer/device accounting must stay in one actual window");
  assert.equal(admitted.attempts["ip:127.0.0.1"], initial.attempts["ip:127.0.0.1"] + 5);
  assert.equal(admitted.attempts["ip:::1"], initial.attempts["ip:::1"] + 5);
  assert.equal(admitted.attempts.global, initial.attempts.global + 10);
  assert.equal(admitted.attempts[`dev:${device}`], 10);
  const denied = await directRequest(h.tls, h.paths.start, { method: "POST", family: 6,
    headers: { origin: h.tls.origin, "x-device-id": device } });
  assert.equal(denied.status, 429); noSetCookie(denied); noPrivateFacts(h, denied.body);
  const full = await rateStatus(h, device);
  assert.equal(full.window_start, initial.window_start);
  assert.equal(full.attempts[`dev:${device}`], 11);
  assert.ok(full.attempts["ip:127.0.0.1"] <= 10 && full.attempts["ip:::1"] <= 10,
    "a shared device ceiling must be distinguished from an IP ceiling");
  await mutationAndReadProvenance(h);
}

export async function publicBoundaries(h) {
  const a = h.actors.get("a"); const one = h.sessions.get("a"); const two = h.sessions.get("b");
  const cookies = await actualCookies(a); const header = cookieHeader(cookies);
  const csrf = sessionCsrf(sessionCookie(cookies, one.context), one.context);
  const base = { method: "POST", headers: { origin: a.origin, cookie: header, "content-type": "application/json" } };
  for (const origin of [undefined, "null", "https://attacker.invalid", `${a.origin}/`]) {
    const headers = { ...base.headers };
    if (origin === undefined) delete headers.origin; else headers.origin = origin;
    await deniedMutation(h, h.paths.start, { ...base, headers });
    await deniedMutation(h, h.paths.logout, { ...base, headers: { ...headers, "x-csrf-token": csrf }, body: { browser_context: one.context } });
  }
  for (const changed of [undefined, "x".repeat(43), sessionCsrf(sessionCookie(cookies, two.context), two.context)]) {
    const headers = { ...base.headers };
    if (changed !== undefined) headers["x-csrf-token"] = changed;
    await deniedMutation(h, h.paths.logout, { ...base, headers, body: { browser_context: one.context } });
  }
  for (const body of [{}, { browser_context: two.context }, { browser_context: one.context, account_id: one.context },
    { browser_context: one.context.toUpperCase() }, { browser_context: "invalid" }]) {
    await deniedMutation(h, h.paths.logout, { ...base, headers: { ...base.headers, "x-csrf-token": csrf }, body });
  }
  for (const pathname of [h.paths.start, h.paths.finish, h.paths.logout]) await deniedMutation(h, pathname, { method: "GET", headers: { cookie: header } });
  await deniedMutation(h, h.paths.start, { ...base, body: {} });
  await waitStartBudget(h, 1);
  const start = await startPublic(h, "a"); const signed = await genuineAssertion(h, "a", start);
  const withPreauth = cookieHeader(await actualCookies(a));
  const finishHeaders = { ...base.headers, cookie: withPreauth, "x-csrf-token": start.csrf_token };
  for (const headers of [
    { ...finishHeaders, "x-csrf-token": "x".repeat(43) },
    { ...finishHeaders, cookie: header },
    { ...finishHeaders, origin: "https://attacker.invalid" },
  ]) await deniedMutation(h, h.paths.finish, { method: "POST", headers, body: signed });
  await deniedMutation(h, h.paths.finish, { method: "POST", headers: finishHeaders,
    body: { ...signed, ceremony_id: two.context } });
  await deniedMutation(h, h.paths.finish, { method: "POST", headers: finishHeaders,
    body: { ...signed, browser_context: one.context } });
  const selectedPreauth = (await actualCookies(a)).find((c) => c.name === `__Host-console-preauth-${start.ceremony_id}`);
  await deniedMutation(h, h.paths.finish, { method: "POST", headers: { ...finishHeaders,
    cookie: `${withPreauth}; ${selectedPreauth.name}=${selectedPreauth.value}` }, body: signed });
  const encoded = JSON.stringify(signed);
  const oversized = encoded + " ".repeat(2 * 1024 * 1024 + 1 - Buffer.byteLength(encoded));
  const tooLarge = await deniedMutation(h, h.paths.finish, { method: "POST", headers: finishHeaders, body: oversized });
  assert.equal(tooLarge.status, 413);
  // The identical actual assertion still succeeds after denied adapter calls.
  const exactFinishBody = encoded + " ".repeat(2 * 1024 * 1024 - Buffer.byteLength(encoded));
  const created = await finishPublic(h, "a", start, signed, { rawBody: exactFinishBody });
  await closeCreated(h, "a", created);
  const selected = sessionCookie(await actualCookies(a), one.context);
  const pathname = `/me/${one.context}/attendance/`;
  const prefix = `${selected.name}=${selected.value}; unrelated=`;
  const exact = prefix + "x".repeat(8192 - Buffer.byteLength(prefix));
  const before = await digest(h);
  const admitted = await directRequest(h.tls, pathname, { headers: { cookie: exact } });
  assert.equal(admitted.status, 200); h.assertNoProof(admitted.body.toString("utf8"));
  assert.ok(admitted.body.toString("utf8").includes(a.facts.account_name));
  assert.equal(await digest(h), before);
  for (const raw of [exact + "x", `${header}; ${selected.name}=${selected.value}`,
    `${header}; __Host-console-session-invalid=${selected.value}`]) {
    const before = await digest(h);
    const denied = await directRequest(h.tls, pathname, { headers: { cookie: raw } });
    noSetCookie(denied); noPrivateFacts(h, denied.body);
    const deniedHtml = denied.body.toString("utf8");
    assert.ok(!deniedHtml.includes("페이지 번호 또는 주소가 올바르지 않습니다"), "malformed access cookies are distinct from invalid pagination");
    assert.ok(!deniedHtml.includes("처음 페이지로 이동"), "first-page navigation cannot repair malformed access cookies");
    assert.equal(await digest(h), before);
  }
  for (const query of ["page=0", "page=-1", "page=01", "page=1.0", "page=1e2", "page=", "page=%201", "page=9007199254740992", "page=1&page=2"]) {
    const before = await digest(h);
    const denied = await directRequest(h.tls, `${pathname}?${query}`, { headers: { cookie: header } });
    noSetCookie(denied); noPrivateFacts(h, denied.body);
    assert.match(denied.body.toString("utf8"), /페이지 번호 또는 주소가 올바르지 않습니다/);
    assert.ok(!denied.body.toString("utf8").includes("다시 로그인하세요"), "invalid pagination must not be presented as expired authority");
    assert.equal(await digest(h), before);
  }
  await a.page.goto(`${a.origin}${pathname}?page=01`);
  await a.page.getByRole("heading", { name: "입력 정보를 확인하세요", exact: true }).waitFor({ state: "visible" });
  noPrivateFacts(h, await a.page.content());
  await focusByKeyboard(a.page, "link", "처음 페이지로 이동");
  await a.page.keyboard.press("Enter");
  await h.checkTable("a", a.facts.first_page);
  for (const otherContext of [two.context, one.context.toUpperCase()]) {
    const response = await directRequest(h.tls, `/me/${otherContext}/attendance/`, { headers: { cookie: `${selected.name}=${selected.value}` } });
    noPrivateFacts(h, response.body);
  }
  await a.page.goto(`${a.origin}${pathname}`); await h.checkTable("a", a.facts.first_page);
}

async function expireActualPreauth(entry) {
  const deadline = Date.now() + boundaryRequirements.maximumPreauthExpiryWaitMs;
  while ((await actualCookies(entry)).some((cookie) => PREAUTH.test(cookie.name))) {
    assert.ok(Date.now() < deadline, "real admitted preauth cookies must expire within their original bounds");
    await delay(POLL_MS);
  }
}

async function concurrentAdmissionBoundaries(h) {
  if (typeof h.holdResponse !== "function") throw new BoundaryPrerequisiteError("real native response gates");
  const entry = h.actors.get("a"); const original = await actualCookies(entry);
  assert.equal(original.filter((cookie) => SESSION.test(cookie.name)).length, 2);
  await expireActualPreauth(entry); await waitStartBudget(h, 9, 20);
  const pending = [];
  for (let index = 0; index < 7; index++) pending.push(await startPublic(h, "a"));
  const heldStarts = [await h.holdResponse(h.native.start), await h.holdResponse(h.native.start)];
  const requests = [startPublic(h, "a"), startPublic(h, "b")];
  for (const request of requests) void request.catch(() => {});
  try {
    for (const gate of heldStarts) assert.equal((await gate.wait()).status, 200);
    assert.equal((await actualCookies(entry)).filter((cookie) => PREAUTH.test(cookie.name)).length, 7,
      "both real requests must enter before either actual response admits its cookie");
    while (heldStarts.length) {
      await heldStarts[0].release();
      heldStarts.shift(); // Only unreleased gates remain owned by cleanup.
    }
    pending.push(...await Promise.all(requests));
    const cookies = await actualCookies(entry);
    assert.equal(new Set(pending.map((start) => start.ceremony_id)).size, 9);
    assert.equal(cookies.filter((cookie) => PREAUTH.test(cookie.name)).length, 9,
      "request-observed admission permits this witnessed race; it is not an atomic browser-store quota");
    for (const start of pending) assert.ok(cookies.some((cookie) => cookie.name === `__Host-console-preauth-${start.ceremony_id}`));
    const before = await digest(h);
    const denied = await Promise.all([browserMutation(h, "a", h.paths.start), browserMutation(h, "b", h.paths.start)]);
    for (const response of denied) assert.equal(response.status, 429);
    assert.equal(await digest(h), before);
    assert.deepEqual(cookieSnapshot(await actualCookies(entry)), cookieSnapshot(cookies),
      "concurrently denied growth must preserve all nine independently admitted generations");
  } finally {
    for (const gate of heldStarts) await gate.release().catch(() => {});
    await Promise.allSettled(requests);
  }
  // No cookie deletion repairs the witnessed race. Let the original native
  // deadlines expire before beginning a separate session-cookie boundary.
  await expireActualPreauth(entry); await waitStartBudget(h, 8, 20);
  const starts = []; const created = []; const gates = []; const finishing = [];
  try {
    for (let index = 0; index < 8; index++) starts.push(await startPublic(h, "a"));
    for (let index = 0; index < 5; index++) {
      created.push({ actor: "a", session: await finishPublic(h, "a", starts[index], await genuineAssertion(h, "a", starts[index])) });
    }
    assert.equal((await actualCookies(entry)).filter((cookie) => SESSION.test(cookie.name)).length, 7);
    const signedA = await genuineAssertion(h, "a", starts[5]);
    const signedB = await genuineAssertion(h, "b", starts[6]);
    gates.push(await h.holdResponse(h.native.login, starts[5].ceremony_id), await h.holdResponse(h.native.login, starts[6].ceremony_id));
    finishing.push(finishPublic(h, "a", starts[5], signedA), finishPublic(h, "b", starts[6], signedB));
    for (const request of finishing) void request.catch(() => {});
    for (const gate of gates) {
      const witness = await gate.wait(); assert.equal(witness.status, 200);
      assert.equal(witness.committed.mapping_open, true); assert.equal(witness.committed.family_open, true);
    }
    assert.equal((await actualCookies(entry)).filter((cookie) => SESSION.test(cookie.name)).length, 7,
      "both native owners must commit while their actual response headers are still held");
    while (gates.length) {
      await gates[0].release();
      gates.shift(); // Only unreleased gates remain owned by cleanup.
    }
    const completed = await Promise.all(finishing);
    created.push({ actor: "a", session: completed[0] }, { actor: "b", session: completed[1] });
    const cookies = await actualCookies(entry);
    assert.equal(cookies.filter((cookie) => SESSION.test(cookie.name)).length, 9,
      "two witnessed requests observed seven session cookies; no stronger store quota is claimed");
    const remaining = await genuineAssertion(h, "a", starts[7]); const before = await digest(h);
    const denied = await Promise.all([
      browserMutation(h, "a", h.paths.finish, { csrf: starts[7].csrf_token, body: remaining }),
      browserMutation(h, "b", h.paths.start),
    ]);
    for (const response of denied) assert.equal(response.status, 429);
    assert.equal(await digest(h), before, "concurrent full admission cannot consume or create native work");
    assert.deepEqual(cookieSnapshot(await actualCookies(entry)), cookieSnapshot(cookies));
    for (const cookie of original.filter((cookie) => SESSION.test(cookie.name))) {
      assert.ok(cookies.some((current) => current.name === cookie.name && current.value === cookie.value && current.expires === cookie.expires));
    }
  } finally {
    for (const gate of gates) await gate.release().catch(() => {});
    const outcomes = await Promise.allSettled(finishing);
    for (let index = 0; index < outcomes.length; index++) {
      if (outcomes[index].status === "fulfilled" && !created.some((item) => item.session.context === outcomes[index].value.context)) {
        created.push({ actor: index === 0 ? "a" : "b", session: outcomes[index].value });
      }
    }
    for (const item of created.toReversed()) await closeCreated(h, item.actor, item.session);
  }
  assert.equal((await actualCookies(entry)).filter((cookie) => SESSION.test(cookie.name)).length, 2);
}

export async function admissionBoundaries(h) {
  const entry = h.actors.get("a"); const original = await actualCookies(entry);
  assert.equal(original.filter((c) => SESSION.test(c.name)).length, 2);
  const expiryDeadline = Date.now() + boundaryRequirements.maximumPreauthExpiryWaitMs;
  while ((await actualCookies(entry)).some((c) => PREAUTH.test(c.name))) {
    assert.ok(Date.now() < expiryDeadline, "preexisting real preauth cookies must expire before isolated boundary proof");
    await delay(POLL_MS);
  }
  await waitStartBudget(h, 8);
  const starts = []; const created = [];
  try {
    for (let index = 0; index < 8; index++) starts.push(await startPublic(h, "a"));
    let cookies = await actualCookies(entry);
    assert.equal(cookies.filter((c) => PREAUTH.test(c.name)).length, 8);
    const before = await digest(h);
    const denied = await browserMutation(h, "a", h.paths.start);
    assert.equal(denied.status, 429);
    assert.match(denied.text, /기존|로그아웃|완료|기한|만료/);
    assert.equal(await digest(h), before);
    assert.deepEqual(cookieSnapshot(await actualCookies(entry)), cookieSnapshot(cookies), "ninth preauth admission must not evict an existing cookie");
    // Finish at exactly eight preauth remains possible. Each signature is
    // generated only immediately before its execution, preserving real counters.
    for (const index of [0, 1, 2, 3, 4, 6]) {
      const signed = await genuineAssertion(h, "a", starts[index]);
      created.push(await finishPublic(h, "a", starts[index], signed));
    }
    cookies = await actualCookies(entry);
    assert.equal(cookies.filter((c) => SESSION.test(c.name)).length, 8);
    const pending = starts[5];
    const signed = await genuineAssertion(h, "a", pending);
    const beforeFull = await digest(h);
    const noFinish = await browserMutation(h, "a", h.paths.finish, { csrf: pending.csrf_token, body: signed });
    assert.equal(noFinish.status, 429);
    const noStart = await browserMutation(h, "a", h.paths.start);
    assert.equal(noStart.status, 429);
    assert.equal(await digest(h), beforeFull, "full admission cannot consume the actual pending assertion");
    assert.deepEqual(cookieSnapshot(await actualCookies(entry)), cookieSnapshot(cookies), "full admission may not clear any other generation");
    await closeCreated(h, "a", created.pop());
    assert.equal((await actualCookies(entry)).filter((c) => SESSION.test(c.name)).length, 7);
    // Retry the very same signed assertion; denied growth did not consume it.
    created.push(await finishPublic(h, "a", pending, signed));
    assert.equal((await actualCookies(entry)).filter((c) => SESSION.test(c.name)).length, 8);
    for (const cookie of original.filter((c) => SESSION.test(c.name))) {
      assert.ok((await actualCookies(entry)).some((c) => c.name === cookie.name && c.value === cookie.value && c.expires === cookie.expires));
    }
  } finally {
    // Authorized fixture cleanup closes only sessions created by this scenario.
    // Never clear the whole browser context to manufacture successful isolation.
    for (const session of created.toReversed()) await closeCreated(h, "a", session);
  }
  assert.equal((await actualCookies(entry)).filter((c) => SESSION.test(c.name)).length, 2);
  await entry.page.goto(`${entry.origin}/me/${h.sessions.get("a").context}/attendance/`);
  await h.checkTable("a", entry.facts.first_page);
  const b = h.actors.get("b"); await b.page.goto(`${b.origin}/me/${h.sessions.get("b").context}/attendance/`);
  await h.checkTable("b", b.facts.first_page);
  await concurrentAdmissionBoundaries(h);
  for (const actor of ["a", "b"]) {
    const current = h.actors.get(actor);
    await current.page.goto(`${current.origin}/me/${h.sessions.get(actor).context}/attendance/`);
    await h.checkTable(actor, current.facts.first_page);
    await h.checkpoint(actor, h.sessions.get(actor).context, h.sessions.get(actor).token, "open",
      h.sessions.get(actor).cookieExpires, h.sessions.get(actor).signedCounter);
  }
}

// Call after core P02 sign-ins and populated P03 checks, before core logouts.
// P03 stays owned by the composed harness: call historyVariants before pass(P03).
export async function runBoundaryScenarios(h) {
  requireHarness(h);
  for (const [id, run] of [["P01", runtimeBoundaries], ["P04", responsiveRecovery],
    ["P05", socketProvenance], ["P06", publicBoundaries], ["P07", admissionBoundaries]]) {
    h.begin(id); await run(h); h.pass(id);
  }
}

// Test-only additive helper, composed into boundary-r3 using its existing helpers.
// Session handles remain in Node custody; no browser cookie injection or storage.
async function mutationAndReadProvenance(h) {
  const entry = h.actors.get("a");
  const original = cookieSnapshot(await actualCookies(entry));
  const metadata = (family, route) => {
    const trace = randomUUID().replaceAll("-", "");
    const span = randomUUID().replaceAll("-", "").slice(0, 16);
    return { device: `r7-peer-${family}-${route}-${randomUUID()}`,
      agent: `r7-real-peer/${family}/${route}`, trace, span,
      traceparent: `00-${trace}-${span}-01` };
  };
  const headers = (meta) => ({ "x-device-id": meta.device,
    "user-agent": meta.agent, traceparent: meta.traceparent,
    // Caller forwarding claims must never become the native audit identity.
    "x-forwarded-for": "203.0.113.77", "x-real-ip": "203.0.113.77" });
  async function inspect(pathname, context, family, meta, status, audited) {
    const observed = await h.control({ op: "inspect-request", path: pathname, context });
    assert.deepEqual(Object.keys(observed).sort(), ["audit", "context", "device_id", "forwarded",
      "native_peer", "path", "status", "traceparent", "user_agent"]);
    assert.equal(observed.path, pathname); assert.equal(observed.context, context);
    assert.equal(observed.status, status);
    assert.match(observed.native_peer, /^127\.0\.0\.1:[1-9][0-9]*$/);
    assert.deepEqual(observed.forwarded, [family === 6 ? "::1" : "127.0.0.1"]);
    assert.equal(observed.device_id, meta.device); assert.equal(observed.user_agent, meta.agent);
    assert.equal(observed.traceparent, meta.traceparent);
    if (!audited) { assert.equal(observed.audit, null); return; }
    assert.deepEqual(observed.audit, { count: 1, ip: family === 6 ? "::1" : "127.0.0.1",
      device: meta.device, trace_id: meta.trace, span_id: meta.span,
      auth_method: "browser_session", user_agent: meta.agent });
  }
  for (const family of [4, 6]) {
    await waitStartBudget(h, 1, 20);
    // Start and sign with the real browser's existing resident credential.
    const start = await startPublic(h, "a");
    const signed = await genuineAssertion(h, "a", start);
    const beforeBrowser = cookieSnapshot(await actualCookies(entry));
    const loginMeta = metadata(family, "login");
    const login = await directRequest(h.tls, h.paths.finish, { family, method: "POST",
      headers: { ...headers(loginMeta), origin: entry.origin,
        cookie: cookieHeader(beforeBrowser), "content-type": "application/json",
        "x-csrf-token": start.csrf_token }, body: signed });
    assert.equal(login.status, 200); h.assertNoProof(login.body.toString("utf8"));
    const reply = JSON.parse(login.body.toString("utf8"));
    assert.deepEqual(Object.keys(reply).sort(), ["context_id", "expires_at"]);
    assert.equal(reply.context_id, start.ceremony_id);
    const name = `__Host-console-session-${reply.context_id}`;
    const setCookies = login.headers["set-cookie"] ?? [];
    assert.ok(Array.isArray(setCookies)); assert.equal(setCookies.length, 2);
    const sessionHeaders = setCookies.filter((value) => value.startsWith(`${name}=`));
    assert.equal(sessionHeaders.length, 1);
    const [pair, ...attributes] = sessionHeaders[0].split(";").map((value) => value.trim());
    const token = pair.slice(name.length + 1);
    assert.match(token, /^bs1\.[A-Za-z0-9_-]{43}$/);
    const flags = attributes.map((value) => value.toLowerCase());
    for (const flag of ["httponly", "secure", "samesite=strict", "path=/"]) assert.ok(flags.includes(flag));
    assert.ok(!flags.some((value) => value.startsWith("domain=")));
    const expiry = attributes.filter((value) => value.toLowerCase().startsWith("expires="));
    assert.equal(expiry.length, 1);
    const cookieExpires = Date.parse(expiry[0].slice("expires=".length)) / 1000;
    assert.equal(cookieExpires, Math.floor(Date.parse(reply.expires_at) / 1000));
    const removedPreauth = setCookies.filter((value) => value.startsWith(`__Host-console-preauth-${start.ceremony_id}=`));
    assert.equal(removedPreauth.length, 1);
    assert.match(removedPreauth[0], /(?:^|;)\s*Max-Age=0(?:;|$)/i);
    assert.deepEqual(cookieSnapshot(await actualCookies(entry)), beforeBrowser,
      "Node-only public response cookies cannot change the real browser jar");
    const bytes = Buffer.from(signed.credential.response.authenticatorData, "base64url");
    assert.ok(bytes.length >= 37); const signedCounter = bytes.readUInt32BE(33);
    await h.checkpoint("a", reply.context_id, token, "open", cookieExpires, signedCounter);
    await inspect(h.native.login, reply.context_id, family, loginMeta, 200, true);
    const cookie = { name, value: token };
    const pathname = `/me/${reply.context_id}/attendance/`;
    const readMeta = metadata(family, "history");
    const beforeRead = await digest(h);
    const read = await directRequest(h.tls, pathname, { family,
      headers: { ...headers(readMeta), cookie: cookieHeader([cookie]) } });
    assert.equal(read.status, 200); const html = read.body.toString("utf8"); h.assertNoProof(html);
    assert.ok(html.includes(entry.facts.account_name) && html.includes(entry.facts.company_name));
    for (const item of entry.facts.first_page.items) assert.ok(html.includes(item.note));
    assert.equal(await digest(h), beforeRead, "SSR read must not mutate auth or business custody");
    await inspect(h.native.history, reply.context_id, family, readMeta, 200, false);
    const logoutMeta = metadata(family, "logout");
    const logout = await directRequest(h.tls, h.paths.logout, { family, method: "POST",
      headers: { ...headers(logoutMeta), origin: entry.origin, cookie: cookieHeader([cookie]),
        "content-type": "application/json", "x-csrf-token": sessionCsrf(cookie, reply.context_id) },
      body: { browser_context: reply.context_id } });
    assert.equal(logout.status, 204); assert.equal(logout.body.length, 0);
    const cleared = logout.headers["set-cookie"] ?? [];
    assert.equal(cleared.length, 1); assert.ok(cleared[0].startsWith(`${name}=`));
    assert.match(cleared[0], /(?:^|;)\s*Max-Age=0(?:;|$)/i);
    await h.checkpoint("a", reply.context_id, token, "closed", cookieExpires, signedCounter);
    await inspect(h.native.logout, reply.context_id, family, logoutMeta, 204, true);
    const afterLogout = await digest(h);
    const denied = await directRequest(h.tls, pathname, { family, headers: { cookie: cookieHeader([cookie]) } });
    noSetCookie(denied); noPrivateFacts(h, denied.body);
    assert.equal(await digest(h), afterLogout);
    assert.deepEqual(cookieSnapshot(await actualCookies(entry)), beforeBrowser);
  }
  const remaining = await actualCookies(entry);
  for (const cookie of original.filter((value) => SESSION.test(value.name))) {
    assert.ok(remaining.some((value) => value.name === cookie.name && value.value === cookie.value && value.expires === cookie.expires));
  }
  for (const actor of ["a", "b"]) {
    const current = h.actors.get(actor);
    await current.page.goto(`${current.origin}/me/${h.sessions.get(actor).context}/attendance/`);
    await h.checkTable(actor, current.facts.first_page);
  }
}
