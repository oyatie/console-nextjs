import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import { focusByKeyboard } from "./browser-business-boundary.mjs";

// Every arm holds an actual owner response before Next receives its headers.
// Cookies are only observed, never fabricated or manually expired here.
function watchAttempt(promise) {
  const state = { settled: false };
  const settled = promise.then(() => {
    state.settled = true; return { passed: true };
  }, () => {
    state.settled = true; return { passed: false };
  });
  return { promise, settled, state };
}
function ownCookie(cookies, context) {
  return cookies.find((cookie) => cookie.name === `__Host-console-session-${context}`);
}
export function assertReadCookies(before, after, observedAtSeconds) {
  assert.ok(Number.isFinite(observedAtSeconds) && observedAtSeconds > 0);
  const original = new Map(before.map((cookie) => [cookie.name, cookie]));
  const current = new Map(after.map((cookie) => [cookie.name, cookie]));
  assert.equal(original.size, before.length, "original cookie names must be unique");
  assert.equal(current.size, after.length, "observed cookie names must be unique");
  for (const cookie of after) {
    assert.ok(original.has(cookie.name), "reads must not add cookies");
    assert.deepEqual(cookie, original.get(cookie.name), "reads must not change surviving cookies");
  }
  for (const cookie of before) {
    if (current.has(cookie.name)) continue;
    assert.ok(Number.isFinite(cookie.expires) && cookie.expires > 0 && cookie.expires <= observedAtSeconds,
      "only a cookie with an actually elapsed recorded expiry may disappear");
  }
}
async function noCookieMutation(response) {
  assert.equal(response.request().redirectedFrom(), null, "cookie observation must include the original response");
  assert.ok(!(await response.headersArray()).some((header) => header.name.toLowerCase() === "set-cookie"),
    "read or rejected authentication response must not mutate browser cookies");
}
async function unchangedReadCookies(h, before) {
  const after = await h.cookieSnapshot();
  // This is the completed-observation time, not the browser's internal capture
  // instant. Actual responses independently reject every Set-Cookie header.
  const observedAtSeconds = Date.now() / 1000;
  assertReadCookies(before, after, observedAtSeconds);
}
async function stable(h, actor, session) {
  const entry = h.actors.get(actor);
  const before = await h.control({ op: "effects" });
  const cookies = await h.cookieSnapshot();
  const response = await entry.page.goto(`${entry.origin}/me/${session.context}/attendance/`);
  assert.equal(response.status(), 200);
  await noCookieMutation(response);
  assert.ok(response.headers()["cache-control"]?.includes("no-store"));
  await h.checkTable(actor, entry.facts.first_page);
  const reloaded = await entry.page.reload();
  assert.equal(reloaded.status(), 200); await noCookieMutation(reloaded);
  assert.ok(reloaded.headers()["cache-control"]?.includes("no-store"));
  await h.checkTable(actor, entry.facts.first_page);
  await unchangedReadCookies(h, cookies);
  const after = await h.control({ op: "effects" });
  assert.equal(after.digest, before.digest, "own reads must not rotate or issue authentication effects");
  await h.checkpoint(actor, session.context, session.token, "open", session.cookieExpires, session.signedCounter);
}
async function release(gate, action = "intact") { await gate.release(action); }
async function logoutClick(h, actor) {
  const page = h.actors.get(actor).page;
  const context = h.sessions.get(actor).context;
  const failures = [];
  const matches = (request) => {
    if (new URL(request.url()).pathname !== h.paths.logout || request.method() !== "POST") return false;
    return request.postDataJSON()?.browser_context === context;
  };
  const failed = (request) => {
    if (matches(request)) failures.push({ path: h.paths.logout, context,
      error: request.failure()?.errorText ?? "" });
  };
  page.on("requestfailed", failed);
  const observed = page.waitForResponse((response) => new URL(response.url()).pathname === h.paths.logout
    && matches(response.request()));
  const pending = watchAttempt(observed);
  pending.failures = failures;
  void pending.settled.then(() => page.off("requestfailed", failed));
  await page.getByRole("button", { name: "로그아웃", exact: true }).click();
  return pending;
}

function assertConservativeAbsent(text) {
  assert.match(text, /만료|접근.*(?:없|확인|불가)|로그인.*(?:다시|필요)|확인.*(?:할 수 없|못|필요)/,
    "lost access must have an explicit absent/expired/uncertain explanation");
  assert.ok(!/로그아웃.*(?:완료|성공)|정상.*로그아웃/.test(text),
    "cookie loss or expiry is not a confirmed logout receipt");
}

async function elapsedDeadline(h, deadlineSeconds) {
  assert.ok(Number.isFinite(deadlineSeconds) && deadlineSeconds > 0);
  const bound = Date.now() + 10000;
  while (Date.now() <= deadlineSeconds * 1000 + 200) {
    assert.ok(Date.now() < bound, "real short-family deadline exceeded the bounded proof budget");
    await h.control({ op: "rate-status", device: "" });
    await delay(Math.min(1000, Math.max(1, deadlineSeconds * 1000 + 201 - Date.now())));
  }
}

async function genuineCancellationAndInvalidResponse(h) {
  const a = h.actors.get("a");
  await a.page.goto(`${a.origin}${h.paths.login}`);
  const finishRequests = [];
  const observe = (request) => {
    if (new URL(request.url()).pathname === h.paths.finish && request.method() === "POST") finishRequests.push(request);
  };
  a.page.on("request", observe);
  let afterStartCookies;
  let afterActualStart;
  let asserted = 0;
  const assertedListener = (event) => { if (event.authenticatorId === a.authenticatorId) asserted += 1; };
  a.cdp.on("WebAuthn.credentialAsserted", assertedListener);
  try {
    try {
      await a.cdp.send("WebAuthn.setAutomaticPresenceSimulation", { authenticatorId: a.authenticatorId, enabled: false });
      await h.rateBudget();
      const start = a.page.waitForResponse((response) => new URL(response.url()).pathname === h.paths.start
        && response.request().method() === "POST");
      void start.catch(() => {}); // Own rejection if the actual click fails; the original observer remains awaited.
      await focusByKeyboard(a.page, "button", "패스키로 로그인");
      await a.page.getByRole("button", { name: "패스키로 로그인", exact: true }).click();
      const response = await start;
      assert.equal(response.status(), 200); h.assertNoProof(await response.text());
      afterActualStart = await h.control({ op: "effects" });
      afterStartCookies = await h.cookieSnapshot();
      // Invoke the actual pending product handler. No navigator stub, fabricated
      // AbortError, injected credentials or fake provider response is accepted.
      const cancel = a.page.getByRole("button", { name: /(?:로그인|인증).*취소|^취소$/ });
      assert.equal(await cancel.count(), 1);
      await focusByKeyboard(a.page, "button", "로그인 인증 취소");
      await cancel.click();
      const notice = a.page.getByRole("main").getByRole("alert")
        .filter({ hasText: /^로그인 인증을 취소했습니다\. 다시 시작할 수 있습니다\.$/ });
      await notice.waitFor({ state: "visible", timeout: 20000 });
      assert.equal(await notice.count(), 1);
      assert.match(await notice.innerText(), /취소/);
      assert.equal(await a.page.getByRole("button", { name: "패스키로 로그인", exact: true }).isEnabled(), true);
      assert.equal(await cancel.count(), 0, "terminal cancellation must remove the pending cancel control");
      assert.equal(finishRequests.length, 0, "cancelled actual credential wait must not submit an assertion");
      assert.equal((await h.control({ op: "effects" })).digest, afterActualStart.digest);
      assert.deepEqual(await h.cookieSnapshot(), afterStartCookies,
        "cancellation may not issue or clear another generation cookie");
    } finally {
      await a.cdp.send("WebAuthn.setAutomaticPresenceSimulation", { authenticatorId: a.authenticatorId, enabled: true });
    }
    // Re-enabled actual authenticator presence would resolve any uncancelled
    // lingering browser wait. Observe the actual CDP assertion event and network,
    // without replacing navigator.credentials or fabricating a rejection.
    await delay(1000);
    assert.equal(asserted, 0, "cancelled actual browser wait must remain aborted when presence returns");
    assert.equal(finishRequests.length, 0);
    assert.equal((await h.control({ op: "effects" })).digest, afterActualStart.digest);
    assert.deepEqual(await h.cookieSnapshot(), afterStartCookies);
  } finally {
    a.cdp.off("WebAuthn.credentialAsserted", assertedListener);
    a.page.off("request", observe);
  }

  await a.page.goto(`${a.origin}${h.paths.login}`);
  await h.rateBudget();
  const beforeInvalid = await h.cookieSnapshot();
  const malformed = await h.holdResponse(h.native.start);
  const attempted = watchAttempt(h.signIn("a"));
  const held = await malformed.wait();
  assert.equal(held.status, 200); assert.ok(held.ceremony, "invalid response fault must start from genuine owner bytes");
  const afterActualCeremony = await h.control({ op: "effects" });
  await release(malformed, "malformed-json");
  await h.pendingFailure("a");
  await attempted.settled;
  assert.equal(await a.page.getByRole("table").count(), 0);
  assert.equal((await h.control({ op: "effects" })).digest, afterActualCeremony.digest,
    "invalid genuine start bytes may not consume a proof or issue a family");
  assert.deepEqual(await h.cookieSnapshot(), beforeInvalid);
  h.assertNoProof(await a.page.content());
  await h.signIn("a"); await h.logout("a"); // Actual accessible recovery into a fresh ceremony.
}

async function currentAuthorityAndExpiry(h) {
  await h.signIn("b"); const survivor = { ...h.sessions.get("b") };
  for (const change of ["account-suspend", "company-suspend", "session-generation-advance", "subject-version-advance"]) {
    await h.signIn("a"); const old = { ...h.sessions.get("a") };
    const browserCookies = await h.cookieSnapshot();
    const changed = await h.control({ op: "fixture-authority", actor: "a", context: old.context, change });
    assert.equal(changed.actor, "a"); assert.equal(changed.context, old.context);
    assert.equal(changed.change, change); assert.equal(changed.applied, true);
    const afterActualFault = await h.control({ op: "effects" });
    await noCookieMutation(await h.absent("a", old.context));
    await noCookieMutation(await h.actors.get("a").page.reload());
    const explanation = await h.actors.get("a").page.locator("body").innerText();
    assert.match(explanation, /권한|계정|법인|접근|다시.*로그인|로그인.*(?:다시|필요)/,
      "current authority loss needs an actionable denied-access explanation");
    assert.equal(await h.actors.get("a").page.getByRole("table").count(), 0);
    await unchangedReadCookies(h, browserCookies);
    assert.equal((await h.control({ op: "effects" })).digest, afterActualFault.digest,
      "denied current-authority reads may not issue or repair authority");
    await stable(h, "b", survivor);
    if (change === "account-suspend" || change === "company-suspend") {
      const restore = change.replace("suspend", "restore");
      const restored = await h.control({ op: "fixture-authority", actor: "a", context: old.context, change: restore });
      assert.equal(restored.applied, true); assert.equal(restored.change, restore);
      await stable(h, "a", old); // Exact previously captured active/status state only.
      await h.logout("a");
    } else {
      // Forward generation changes remain in effect. Close the known original
      // family through its real native owner, then clear only its actual cookie.
      const closed = await h.control({ op: "native-revoke", actor: "a", context: old.context, session_token: old.token });
      assert.equal(closed.status, 204);
      const confirmed = await h.rawPublic("a", h.paths.logout, h.publicLogoutBody(old.context), h.sessionCsrf(old.token, old.context));
      assert.equal(confirmed.status, 204);
      await h.checkpoint("a", old.context, old.token, "closed", old.cookieExpires, old.signedCounter);
      assert.ok(!ownCookie(await h.cookieSnapshot(), old.context));
    }
    await stable(h, "b", survivor);
  }

  // Expiry follows a genuinely configured native family and actual browser
  // deadline. B's original long family survives the serving-runtime change.
  await h.rateBudget();
  await h.restartRuntime({ CONSOLE_BACKEND_ORIGIN: h.runtime.shortBackend });
  await h.signIn("a"); const expiring = { ...h.sessions.get("a") };
  // signIn proves the real header/body/jar deadline is live at this observation.
  // Its later checkpoint/table work may legitimately consume the six seconds.
  assert.ok(expiring.cookieExpires - expiring.cookieObservedAt <= 6 && expiring.cookieExpires > expiring.cookieObservedAt);
  await elapsedDeadline(h, Math.max(expiring.cookieExpires, expiring.browserCookieExpires));
  assert.ok(!ownCookie(await h.cookieSnapshot(), expiring.context));
  const expiredState = await h.control({ op: "effects" });
  await noCookieMutation(await h.absent("a", expiring.context));
  await noCookieMutation(await h.actors.get("a").page.reload());
  assertConservativeAbsent(await h.actors.get("a").page.locator("body").innerText());
  assert.equal((await h.control({ op: "effects" })).digest, expiredState.digest);
  await stable(h, "b", survivor);
  const closed = await h.control({ op: "native-revoke", actor: "a", context: expiring.context, session_token: expiring.token });
  assert.equal(closed.status, 204);
  await h.checkpoint("a", expiring.context, expiring.token, "closed", expiring.cookieExpires, expiring.signedCounter);
  await h.restartRuntime({ CONSOLE_BACKEND_ORIGIN: h.runtime.normalBackend });
  await stable(h, "b", survivor); await h.logout("b");
}

export async function runTemporalScenarios(h) {
  h.begin("P08");
  await genuineCancellationAndInvalidResponse(h);
  await h.signIn("a"); await h.signIn("b");
  const independentA = { ...h.sessions.get("a") }, independentB = { ...h.sessions.get("b") };
  assert.equal(h.actors.get("a").context, h.actors.get("b").context, "same browser cookie jar is required");
  assert.notEqual(independentA.context, independentB.context);
  await stable(h, "a", independentA); await stable(h, "b", independentB);
  await h.logout("a"); await h.absent("a", independentA.context);
  await stable(h, "b", independentB);
  const entry = h.actors.get("a");
  await entry.page.goto(`${entry.origin}${h.paths.login}`);
  assert.equal(new URL(entry.page.url()).pathname, h.paths.login);
  assert.equal(await entry.page.getByRole("table").count(), 0, "entry must not select another live cookie");
  await h.logout("b"); h.pass("P08");

  h.begin("P09");
  await h.rateBudget(2);
  const startA = await h.holdResponse(h.native.start);
  const attemptA = watchAttempt(h.signIn("a"));
  const heldStart = await startA.wait();
  assert.equal(heldStart.status, 200); assert.ok(heldStart.ceremony);
  assert.ok(!(await h.cookieSnapshot()).some((cookie) => cookie.name === `__Host-console-preauth-${heldStart.ceremony}`));
  const finishB = await h.holdResponse(h.native.login);
  const attemptB = watchAttempt(h.signIn("b"));
  const heldB = await finishB.wait();
  assert.equal(heldB.status, 200); assert.equal(heldB.committed.mapping_exists, true);
  const preauthB = (await h.cookieSnapshot()).find((cookie) => cookie.name === `__Host-console-preauth-${heldB.context}`);
  assert.ok(preauthB, "newer B preauth must still be pending while the older A start is delayed");
  assert.ok(!ownCookie(await h.cookieSnapshot(), heldB.context));
  const finishA = await h.holdResponse(h.native.login, heldStart.ceremony);
  await release(startA);
  const heldFinish = await finishA.wait();
  assert.equal(heldFinish.context, heldStart.ceremony);
  const delayedCookies = await h.cookieSnapshot();
  assert.ok(delayedCookies.some((cookie) => cookie.name === `__Host-console-preauth-${heldStart.ceremony}`));
  assert.deepEqual(delayedCookies.find((cookie) => cookie.name === preauthB.name), preauthB,
    "delivery of old A start must preserve the actual pending newer B preauth envelope");
  await release(finishB); await attemptB.promise;
  const laterB = { ...h.sessions.get("b") };
  assert.equal(laterB.context, heldB.context);
  const cookieB = ownCookie(await h.cookieSnapshot(), laterB.context);
  await stable(h, "b", laterB);
  await release(finishA); await attemptA.promise;
  assert.deepEqual(ownCookie(await h.cookieSnapshot(), laterB.context), cookieB);
  await stable(h, "b", laterB); await h.logout("a"); await h.logout("b"); h.pass("P09");

  h.begin("P10");
  for (const variant of ["intact", "navigation-before-headers", "client-timeout-before-headers"]) {
    await h.rateBudget(2);
    const gateA = await h.holdResponse(h.native.login);
    const attempt = watchAttempt(h.signIn("a"));
    const held = await gateA.wait();
    assert.equal(held.status, 200); assert.equal(held.committed.mapping_exists, true);
    assert.equal(held.committed.family_open, true); assert.equal(held.committed.ceremony_consumed, true);
    assert.ok(!ownCookie(await h.cookieSnapshot(), held.context), "held native response has not reached browser headers");
    await h.signIn("b"); const newB = { ...h.sessions.get("b") };
    const protectedCookie = ownCookie(await h.cookieSnapshot(), newB.context);
    if (variant === "navigation-before-headers") {
      await h.actors.get("a").page.goto(`${h.actors.get("a").origin}${h.paths.login}`);
    } else if (variant === "client-timeout-before-headers") {
      await h.pendingFailure("a");
    }
    await release(gateA);
    if (variant === "intact") await attempt.promise;
    else {
      await Promise.race([attempt.settled, delay(20000).then(() => ({ passed: false }))]);
      // A discarded old result may install its own cookie at header arrival;
      // observed presence/absence never permits it to overwrite B's generation.
      const old = ownCookie(await h.cookieSnapshot(), held.context);
      if (old) {
        assert.equal(old.name, `__Host-console-session-${held.context}`);
        const retired = await h.control({ op: "native-revoke", actor: "a", context: held.context, session_token: old.value });
        assert.equal(retired.status, 204);
        const csrf = h.sessionCsrf(old.value, held.context);
        const response = await h.rawPublic("a", h.paths.logout, h.publicLogoutBody(held.context), csrf);
        assert.equal(response.status, 204);
      }
    }
    assert.deepEqual(ownCookie(await h.cookieSnapshot(), newB.context), protectedCookie);
    await stable(h, "b", newB);
    if (variant === "intact") await h.logout("a");
    // Retain the observed 20-second/header race above, then finish the bounded
    // old observer lifecycle before another attempt reuses this same page.
    await attempt.settled;
    await h.logout("b");
  }
  // The required page-close variant runs last after P17 so closing the actual
  // authenticator-host page cannot force private-key copying for later cases.
  h.defer("P10");

  h.begin("P11");
  await h.rateBudget(2);
  await h.signIn("a"); const oldA = { ...h.sessions.get("a") };
  const oldLogout = await h.holdResponse(h.native.logout, oldA.context);
  const observedLogout = await logoutClick(h, "a");
  const closed = await oldLogout.wait();
  assert.equal(closed.status, 204); assert.equal(closed.committed.mapping_open, false);
  await h.signIn("b"); const newB = { ...h.sessions.get("b") };
  const protectedCookie = ownCookie(await h.cookieSnapshot(), newB.context);
  await stable(h, "b", newB);
  await release(oldLogout); const logoutResponse = await observedLogout.promise;
  assert.equal(logoutResponse.status(), 204);
  assert.ok(!ownCookie(await h.cookieSnapshot(), oldA.context));
  assert.deepEqual(ownCookie(await h.cookieSnapshot(), newB.context), protectedCookie);
  await stable(h, "b", newB); await h.logout("b"); h.pass("P11");

  h.begin("P12");
  await h.rateBudget(2);
  await h.signIn("b"); const survivor = { ...h.sessions.get("b") };
  const lost = await h.holdResponse(h.native.login);
  const attempted = watchAttempt(h.signIn("a"));
  const held = await lost.wait();
  assert.equal(held.status, 200); assert.equal(held.committed.mapping_exists, true);
  const request = h.actors.get("a").page.context().request;
  // Capture only the actual browser assertion and CSRF from its real request.
  // The native JWT and refresh token never enter this process.
  const finishRequest = h.actors.get("a").lastFinishRequest;
  assert.ok(finishRequest, "signIn must retain its observed public finish request for replay proof");
  const signedBody = finishRequest.postDataJSON();
  const csrf = finishRequest.headers()["x-csrf-token"];
  await release(lost, "drop");
  await h.pendingFailure("a");
  await attempted.settled;
  assert.ok(!ownCookie(await h.cookieSnapshot(), held.context));
  const beforeReplay = await h.control({ op: "effects" });
  const replayGate=await h.holdResponse(h.native.login,held.context);
  const replayPending=request.post(`${h.actors.get("a").origin}${h.paths.finish}`, {
    headers: { origin:h.actors.get("a").origin,"x-csrf-token": csrf }, data: signedBody,
  });
  void replayPending.catch(() => {}); // Own socket rejection while the real native gate waits; original remains awaited.
  const replayOwner=await replayGate.wait();
  assert.equal(replayOwner.status,401,"replay must reach actual consumed native assertion owner");
  assert.equal(replayOwner.committed.ceremony_consumed,true);
  await release(replayGate);
  const replay=await replayPending;
  assert.ok([401, 409, 410].includes(replay.status()));
  h.assertNoProof(await replay.text());
  assert.equal((await h.control({ op: "effects" })).digest, beforeReplay.digest);
  await h.absent("a", held.context); await stable(h, "b", survivor);
  await h.signIn("a"); assert.notEqual(h.sessions.get("a").context, held.context);
  await h.logout("a"); await h.logout("b"); h.pass("P12");

  h.begin("P13");
  await h.rateBudget(2);
  await h.restartRuntime({ CONSOLE_BACKEND_ORIGIN: h.runtime.shortBackend });
  const originalSender = h.runtime.instance;
  await h.signIn("a"); const short = { ...h.sessions.get("a") };
  assert.ok(short.cookieExpires - Date.now() / 1000 <= 6 && short.cookieExpires > Date.now() / 1000);
  const logoutGate = await h.holdResponse(h.native.logout, short.context);
  let heldLogout;
  let observedLoss;
  await h.withCapturedResponse("a", h.paths.logout, async (getResponse) => {
    const shortLogout = await logoutClick(h, "a");
    heldLogout = await logoutGate.wait(); assert.equal(heldLogout.status, 204);
    assert.equal(heldLogout.committed.mapping_open, false);
    await elapsedDeadline(h, Math.max(short.cookieExpires, short.browserCookieExpires));
    assert.ok(!ownCookie(await h.cookieSnapshot(), short.context), "genuine original cookie deadline elapsed");
    const uncertain = await h.pendingFailure("a");
    assert.match(uncertain, /확인.*(?:할 수 없|못|필요)|불확실|UNKNOWN|결과.*(?:알 수 없|확인)|재시도/,
      "lost logout result must remain explicitly uncertain before hard refresh");
    assert.ok(!/완료|성공/.test(uncertain));
    // This variant is a real unconfirmed-result/expired-access proof. An already
    // failed or cancelled initiating request cannot also prove late headers.
    // Capture original failure bytes before unchanged continuation; validate
    // their request binding before recovery navigation. An unread no-store
    // fetch body can defer Chromium/Playwright completion. The observer pause
    // supplies no untouched-latency proof.
    const lostOutcome = await shortLogout.settled;
    if (lostOutcome.passed) {
      const failureResponse = await shortLogout.promise;
      assert.equal(failureResponse.url(), `${h.actors.get("a").origin}${h.paths.logout}`);
      assert.equal(failureResponse.request().method(), "POST");
      assert.ok(failureResponse.status() >= 400, "lost result must not have received an exact logout204 receipt");
      const wire = await getResponse();
      assert.equal(wire.status, failureResponse.status());
      assert.equal(wire.postData, failureResponse.request().postData());
      assert.equal(JSON.parse(wire.postData).browser_context, short.context);
      h.assertNoProof(wire.body);
      observedLoss = { kind: "http-failure", status: failureResponse.status() };
    } else {
      assert.equal(shortLogout.failures.length, 1, "missing response needs exact observed request failure evidence");
      const failed = shortLogout.failures[0];
      assert.equal(failed.context, short.context);
      assert.match(failed.error, /ERR_ABORTED|ERR_FAILED|CANCEL|ABORT|CONNECTION|DISCONNECTED/i);
      observedLoss = { kind: "request-failure", error: failed.error };
    }
  });
  await h.absent("a", short.context);
  await h.actors.get("a").page.reload();
  const absentText=await h.actors.get("a").page.locator("body").innerText();
  assertConservativeAbsent(absentText);
  assert.equal(h.runtime.instance, originalSender);
  assert.equal(originalSender.child.exitCode, null); assert.equal(originalSender.child.signalCode, null);
  await h.signIn("b"); const replacement = { ...h.sessions.get("b") };
  const newCookie = ownCookie(await h.cookieSnapshot(), replacement.context);
  await release(logoutGate);
  assert.equal(h.runtime.instance, originalSender);
  assert.deepEqual(ownCookie(await h.cookieSnapshot(), replacement.context), newCookie);
  await stable(h, "b", replacement);
  await h.checkpoint("a", short.context, short.token, "closed", short.cookieExpires, short.signedCounter);
  await h.logout("b");
  h.recordCheck("P13", { id: "lost-result-expired-access", status: "passed", evidence: {
    observed: observedLoss, native_owner_status: heldLogout.status, actual_cookie_expired: true,
    unknown_before_refresh: true, conservative_absent_after_refresh: true,
    same_sender_alive: h.runtime.instance === originalSender, late_header_claim: false,
  } });
  await h.restartRuntime({ CONSOLE_BACKEND_ORIGIN: h.runtime.normalBackend });

  // Independently prove the actual near-expiry late-header race. Keep the
  // original requesting page alive; use another real same-cookie Document for
  // recovery/refresh, so navigation cannot cancel the measured old request.
  await h.rateBudget(2);
  await h.restartRuntime({ CONSOLE_BACKEND_ORIGIN: h.runtime.shortBackend });
  const intactSender = h.runtime.instance;
  await h.signIn("a"); const lateA = { ...h.sessions.get("a") };
  assert.ok(lateA.cookieExpires - Date.now() / 1000 <= 6 && lateA.cookieExpires > Date.now() / 1000);
  const intactGate = await h.holdResponse(h.native.logout, lateA.context);
  const originalLogout = await logoutClick(h, "a");
  const heldIntact = await intactGate.wait();
  assert.equal(heldIntact.status, 204); assert.equal(heldIntact.committed.mapping_open, false);
  await elapsedDeadline(h, Math.max(lateA.cookieExpires, lateA.browserCookieExpires));
  assert.ok(!ownCookie(await h.cookieSnapshot(), lateA.context));
  assert.equal(originalLogout.state.settled, false, "original request must still await its actual headers at expiry");
  const originalPage = h.actors.get("a").page;
  const recovery = await h.actors.get("a").context.newPage();
  try {
    const url = `${h.actors.get("a").origin}/me/${lateA.context}/attendance/`;
    for (const navigate of [() => recovery.goto(url), () => recovery.reload()]) {
      const result = await navigate(); h.assertNoProof(await result.text());
      const facts = h.actors.get("a").facts;
      const text = await recovery.locator("body").innerText();
      assert.ok(!text.includes(facts.account_name) && !text.includes(facts.company_name));
      assert.equal(await recovery.getByRole("table").count(), 0);
      assertConservativeAbsent(text);
    }
    assert.equal(originalPage.isClosed(), false);
    assert.equal(h.runtime.instance, intactSender);
    assert.equal(intactSender.child.exitCode, null); assert.equal(intactSender.child.signalCode, null);
    await h.signIn("b"); const liveB = { ...h.sessions.get("b") };
    const protectedB = ownCookie(await h.cookieSnapshot(), liveB.context);
    assert.ok(protectedB);
    assert.equal(originalLogout.state.settled, false,
      "B's actual cookie must exist before the original old logout headers arrive");
    const returned = originalPage.waitForURL((url) => url.origin === h.actors.get("a").origin
      && url.pathname === h.paths.login);
    const receipt = originalLogout.promise.then(h.assertNoContentResponse);
    void returned.catch(() => {}); void receipt.catch(() => {});
    await release(intactGate);
    const exactOriginal = await originalLogout.promise;
    await Promise.all([receipt, returned]);
    await originalPage.getByRole("heading", { name: "패스키 로그인", exact: true }).waitFor();
    assert.equal(new URL(exactOriginal.url()).pathname, h.paths.logout);
    assert.equal(exactOriginal.request().method(), "POST");
    assert.equal(exactOriginal.request().postDataJSON().browser_context, lateA.context);
    const cookieHeaders = (await exactOriginal.headersArray())
      .filter((header) => header.name.toLowerCase() === "set-cookie");
    assert.ok(cookieHeaders.some((header) => header.value.startsWith(`__Host-console-session-${lateA.context}=`)),
      "actual exact old204 headers must target only that expired generation");
    assert.ok(!cookieHeaders.some((header) => header.value.startsWith(`__Host-console-session-${liveB.context}=`)));
    assert.equal(h.runtime.instance, intactSender);
    assert.deepEqual(ownCookie(await h.cookieSnapshot(), liveB.context), protectedB);
    await stable(h, "b", liveB);
    await h.checkpoint("a", lateA.context, lateA.token, "closed", lateA.cookieExpires, lateA.signedCounter);
    await h.logout("b");
    h.recordCheck("P13", { id: "near-expiry-intact-late-headers", status: "passed", evidence: {
      native_owner_status: heldIntact.status, original_public_status: exactOriginal.status(),
      empty_body: true, actual_cookie_expired: true, recovery_document_refreshed: true,
      original_request_pending_until_b_cookie: true, exact_original_generation_headers: true,
      same_sender_alive: h.runtime.instance === intactSender, newer_b_cookie_and_reads_preserved: true,
    } });
  } finally { await recovery.close(); }
  await h.restartRuntime({ CONSOLE_BACKEND_ORIGIN: h.runtime.normalBackend });
  h.pass("P13");

  h.begin("P14");
  await currentAuthorityAndExpiry(h);
  const page = h.actors.get("a").page;
  await page.goto(`${h.actors.get("a").origin}${h.paths.login}`);
  const pendingStart = await h.startCeremony("a");
  const pendingAssertion = await h.assertion("a", pendingStart);
  const beforeRotation = await h.control({ op: "effects" });
  const beforeCookies = await h.cookieSnapshot();
  await h.restartRuntime({ CONSOLE_BROWSER_PREAUTH_KEY: randomBytes(32).toString("base64url") });
  const observedFinish = page.waitForResponse((response) => response.url() === `${h.actors.get("a").origin}${h.paths.finish}`
    && response.request().method() === "POST" && response.request().postDataJSON()?.ceremony_id === pendingStart.ceremony_id);
  void observedFinish.catch(() => {});
  const rejected = await h.rawPublic("a", h.paths.finish, h.publicFinishBody(pendingStart, pendingAssertion), pendingStart.csrf_token);
  await noCookieMutation(await observedFinish);
  assert.ok([400, 401, 403, 410].includes(rejected.status)); h.assertNoProof(rejected.body);
  assert.equal((await h.control({ op: "effects" })).digest, beforeRotation.digest);
  await unchangedReadCookies(h, beforeCookies);
  await h.signIn("a"); await h.signIn("b");
  const revoked = { ...h.sessions.get("a") }, live = { ...h.sessions.get("b") };
  const browserCookies = await h.cookieSnapshot();
  const owner = await h.control({ op: "native-revoke", actor: "a", context: revoked.context, session_token: revoked.token });
  assert.equal(owner.status, 204); await unchangedReadCookies(h, browserCookies);
  await noCookieMutation(await h.absent("a", revoked.context)); await stable(h, "b", live);
  await h.control({ op: "cleanup", actor: "a" }); await h.restartRuntime();
  await noCookieMutation(await h.absent("a", revoked.context)); await stable(h, "b", live);
  const confirmed = await h.rawPublic("a", h.paths.logout, h.publicLogoutBody(revoked.context), h.sessionCsrf(revoked.token, revoked.context));
  assert.equal(confirmed.status, 204);
  await h.logout("b"); h.pass("P14");
}

export async function runFinalCloseScenario(h) {
  h.resume("P10");
  await h.rateBudget(2);
  const originalPage = h.actors.get("a").page;
  const gate = await h.holdResponse(h.native.login);
  const attempt = watchAttempt(h.signIn("a"));
  const held = await gate.wait();
  assert.equal(held.status, 200); assert.equal(held.committed.mapping_exists, true);
  assert.equal(held.committed.family_open, true); assert.equal(held.committed.ceremony_consumed, true);
  assert.ok(!ownCookie(await h.cookieSnapshot(), held.context));
  await originalPage.close(); // Actual request-origin page closes before native headers arrive.
  assert.equal(originalPage.isClosed(), true);
  await attempt.settled;
  await h.signIn("b"); const survivor = { ...h.sessions.get("b") };
  const newerCookie = ownCookie(await h.cookieSnapshot(), survivor.context);
  await release(gate); // Intact actual owner response, no replay or fabricated status.
  assert.deepEqual(ownCookie(await h.cookieSnapshot(), survivor.context), newerCookie);
  await stable(h, "b", survivor);
  await h.logout("b");
  h.pass("P10");
}

export async function runUnavailableScenarios(h) {
  h.begin("P17");
  await h.signIn("a"); const session = { ...h.sessions.get("a") };
  const before = await h.control({ op: "effects" });
  await h.restartRuntime({ CONSOLE_BACKEND_ORIGIN: "http://127.0.0.1:1" });
  const page = h.actors.get("a").page;
  const retryPath = `/me/${session.context}/attendance/?page=2`;
  const response = await page.goto(`${h.actors.get("a").origin}${retryPath}`);
  const html = await response.text(); h.assertNoProof(html);
  assert.equal(await h.actors.get("a").page.getByRole("table").count(), 0);
  assert.ok(!html.includes(h.actors.get("a").facts.account_name));
  const unavailable = h.actors.get("a").page.getByRole("main").getByRole("alert");
  assert.equal(await unavailable.count(), 1, "unavailable source needs a visible honest failure");
  assert.equal(await unavailable.isVisible(), true);
  assert.equal(await page.getByRole("heading", { name: "기록을 불러올 수 없습니다", exact: true }).count(), 1);
  const retry = page.getByRole("link", { name: "다시 불러오기", exact: true });
  assert.equal(await retry.count(), 1); assert.equal(await retry.isVisible(), true);
  assert.equal(await retry.getAttribute("href"), retryPath, "retry preserves the authorized context and requested page");
  await retry.click();
  assert.equal(await unavailable.isVisible(), true, "retry during the real outage remains unavailable");
  assert.equal(await page.getByRole("table").count(), 0);
  assert.equal((await h.control({ op: "effects" })).digest, before.digest);
  await h.restartRuntime({ CONSOLE_BACKEND_ORIGIN: h.runtime.normalBackend });
  await retry.click(); await h.checkTable("a", h.actors.get("a").facts.second_page);
  await stable(h, "a", session);
  for (const action of ["malformed-json", "malformed-clock"]) {
    const gate = await h.holdResponse(h.native.history, session.context);
    const pending = watchAttempt(h.actors.get("a").page.reload());
    const held = await gate.wait(); assert.equal(held.status, 200);
    await release(gate, action); const rendered = await pending.promise;
    h.assertNoProof(await rendered.text());
    assert.equal(await h.actors.get("a").page.getByRole("table").count(), 0);
    const invalid = h.actors.get("a").page.getByRole("main").getByRole("alert");
    assert.equal(await invalid.count(), 1, "malformed owner bytes must fail visibly");
    assert.equal(await invalid.isVisible(), true);
    assert.equal(await page.getByRole("heading", { name: "기록을 불러올 수 없습니다", exact: true }).count(), 1);
    const repair = page.getByRole("link", { name: "다시 불러오기", exact: true });
    assert.equal(await repair.count(), 1); assert.equal(await repair.isVisible(), true);
    assert.equal(await repair.getAttribute("href"), `/me/${session.context}/attendance/`);
    await repair.click(); await h.checkTable("a", h.actors.get("a").facts.first_page);
    await stable(h, "a", session);
  }
  await h.logout("a"); h.pass("P17");
}
