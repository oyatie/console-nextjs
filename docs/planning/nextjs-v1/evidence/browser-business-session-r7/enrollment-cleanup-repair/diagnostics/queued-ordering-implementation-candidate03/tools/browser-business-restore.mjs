// Draft proposal only. Imported by the C2-owned real-service runner; no launch here.
import assert from "node:assert/strict";
import { createHash, randomUUID } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";

const LOGIN = "/login/";
const LOGOUT = "/api/browser-session/logout/";
const PROBE = "__r7RestoreObservation";
const PREFIX = "R7_RESTORE_OBSERVATION ";
const GUARD_ID = "console-browser-restoration-guard";
const PRIVATE_SELECTOR = "#console-browser-private-region";

class Unreached extends Error {}
function isPrerequisite(error) { return error instanceof Unreached || error.code === "R7_BROWSER_PREREQUISITE"; }
function sha(bytes) { return createHash("sha256").update(bytes).digest("hex"); }
function selectedCookie(cookies, session) {
  return cookies.find((cookie) => cookie.name === `__Host-console-session-${session.context}`);
}
function cookieState(cookies) {
  return cookies.map((cookie) => ({ ...cookie })).sort((a, b) => a.name.localeCompare(b.name));
}
async function until(predicate, timeout) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const value = await predicate();
    if (value) return value;
    await delay(10);
  }
  return null;
}

// The exact identity is root-selected. Before faulting, measure its nonempty
// literal from two independently authorized documents of the immutable runtime.
// Never select an arbitrary inline script or infer correctness from existence.
function findGuard(html, contract) {
  const matches = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script\s*>/gi)]
    .filter((match) => !/\bsrc\s*=/i.test(match[1])
      && /(?:^|\s)id\s*=\s*["']console-browser-restoration-guard["'](?:\s|$)/i.test(match[1])
      && match[2] === contract.literal && sha(match[2]) === contract.inlineScriptSha256);
  assert.equal(matches.length, 1, "pinned parser guard must occur exactly once");
  return matches[0];
}

async function measureGuard(h) {
  const measured = [];
  for (const name of ["a", "b"]) {
    const actor = h.actors.get(name); const session = h.sessions.get(name);
    if (!actor || !session) throw new Unreached("two live independently authenticated actors are required before restore faults");
    const response = await actor.page.goto(`${actor.origin}/me/${session.context}/attendance/`, { waitUntil: "domcontentloaded" });
    if (response.status() !== 200) throw new Unreached("two actual unfaulted authorized documents unavailable");
    const html = await response.text(); h.assertNoProof(html); assert.match(response.headers()["cache-control"], /no-store/);
    const script = actor.page.locator(`script#${GUARD_ID}:not([src])`);
    const region = actor.page.locator(PRIVATE_SELECTOR);
    assert.equal(await script.count(), 1, "authorized HTML must provide exactly one root-selected inline guard");
    assert.equal(await region.count(), 1, "authorized HTML must provide exactly one private region");
    const literal = await script.textContent();
    assert.ok(literal?.trim(), "authorized HTML must provide a nonempty root-selected parser guard");
    const contract = { privateRegionSelector: PRIVATE_SELECTOR, inlineScriptId: GUARD_ID,
      literal, inlineScriptSha256: sha(literal) };
    findGuard(html, contract);
    await h.checkTable(name, actor.facts.first_page);
    const beforePrivate = await actor.page.evaluate(({ id, selector }) => Boolean(document.getElementById(id)
      .compareDocumentPosition(document.querySelector(selector)) & Node.DOCUMENT_POSITION_FOLLOWING),
    { id: GUARD_ID, selector: PRIVATE_SELECTOR });
    assert.equal(beforePrivate, true, "actual parser guard must precede private content");
    measured.push(contract);
  }
  assert.equal(measured[0].literal, measured[1].literal, "guard literal must be request-independent across genuine A/B contexts");
  assert.equal(measured[0].inlineScriptSha256, measured[1].inlineScriptSha256);
  return measured[0];
}

async function installProbe(page, selector, facts, queueLogout, observeReact = false) {
  const events = [];
  const errors = [];
  const listener = (message) => {
    const text = message.text();
    if (!text.startsWith(PREFIX)) return;
    try {
      assert.ok(text.length < 2048, "bounded secret-free restore observation");
      const value = JSON.parse(text.slice(PREFIX.length));
      assert.deepEqual(Object.keys(value).sort(), ["document", "event", "hidden", "persisted",
        "present", "private_visible", "react_commits", "restored", "trusted"].sort());
      events.push(value);
    } catch { errors.push("invalid restore probe frame"); }
  };
  page.on("console", listener);
  await page.addInitScript(({ selector, names, queueLogout, observeReact, key, prefix }) => {
    const state = { document: crypto.randomUUID(), restored: false, reactCommits: 0, logoutQueued: false, logoutStarted: false, logoutReleased: false };
    Object.defineProperty(window, key, { value: state });
    // Only a real persisted pageshow queues this timer. The native response
    // witness below releases it once; cleanup never releases an unproved action.
    let releaseLogout;
    const logoutRelease = queueLogout ? new Promise((resolve) => { releaseLogout = resolve; }) : null;
    state.releaseLogout = (original) => {
      if (!queueLogout || state.document !== original || !state.restored
        || !state.logoutQueued || state.logoutStarted || state.logoutReleased) return null;
      const commits = state.reactCommits;
      state.logoutReleased = true;
      releaseLogout();
      return commits;
    };
    function report(event, source) {
      const region = document.querySelector(selector);
      const style = region ? getComputedStyle(region) : null;
      const hidden = !region || !region.getClientRects().length
        || style.display === "none" || style.visibility === "hidden" || style.visibility === "collapse";
      const visible = document.body?.innerText ?? "";
      console.debug(prefix + JSON.stringify({ document: state.document, event,
        trusted: source?.isTrusted ?? false, persisted: source?.persisted ?? false,
        restored: state.restored, present: Boolean(region), hidden,
        private_visible: names.some((name) => visible.includes(name)), react_commits: state.reactCommits }));
    }
    // Observe actual installed React commits. This hook does not render or alter
    // props/state, synthesize pageshow, or authorize any action.
    if (queueLogout || observeReact) {
      if (window.__REACT_DEVTOOLS_GLOBAL_HOOK__) throw new Error("unexpected pre-existing React probe");
      let renderer = 0;
      const renderers = new Map();
      window.__REACT_DEVTOOLS_GLOBAL_HOOK__ = {
        supportsFiber: true, renderers,
        inject(value) { renderers.set(++renderer, value); return renderer; },
        checkDCE() {}, onCommitFiberUnmount() {}, onPostCommitFiberRoot() {},
        onScheduleFiberRoot() {},
        onCommitFiberRoot() { state.reactCommits += 1; if (state.restored) report("react-commit"); },
      };
    }
    // Arm while the original Document is active, after its parser guard ran.
    // Native listener callbacks can checkpoint microtasks between listeners;
    // registration after the production listener permits a synchronous sample.
    state.armPagehide = (original) => {
      if (state.document !== original || state.pagehideArmed) return false;
      state.pagehideArmed = true;
      addEventListener("pagehide", (event) => report("pagehide", event));
      return true;
    };
    addEventListener("pageshow", (event) => {
      if (event.isTrusted && event.persisted) {
        state.restored = true;
        if (queueLogout && !state.logoutQueued) {
          state.logoutQueued = true;
          setTimeout(async () => {
            await logoutRelease;
            state.logoutStarted = true;
            const button = [...(document.querySelector(selector)?.querySelectorAll("button") ?? [])]
              .find((item) => item.textContent.trim() === "로그아웃");
            if (button) button.click(); // Actual mounted product handler; real owner response required below.
          }, 0);
        }
      }
      queueMicrotask(() => report("pageshow", event));
    });
    new MutationObserver(() => { if (state.restored) report("mutation"); })
      .observe(document, { subtree: true, childList: true, attributes: true });
  }, { selector, names: [facts.company_name, facts.account_name,
    ...facts.first_page.items.map((item) => item.note)], queueLogout, observeReact, key: PROBE, prefix: PREFIX });
  return { events, errors,
    async armPagehide(original) {
      assert.equal(await page.evaluate(({ key, original }) => window[key].armPagehide(original),
        { key: PROBE, original }), true, "arm departure observation once in the original active Document");
    },
    remove() { page.off("console", listener); } };
}

async function assertPrivateHidden(page, actor, h) {
  const text = await page.locator("body").innerText();
  assert.ok(!text.includes(actor.facts.company_name) && !text.includes(actor.facts.account_name));
  for (const row of actor.facts.first_page.items) assert.ok(!text.includes(row.note));
  const table = page.getByRole("table");
  for (let index = 0; index < await table.count(); index += 1) assert.equal(await table.nth(index).isVisible(), false);
  h.assertNoProof(await page.content());
}

async function assertRecovery(page, h, reason) {
  assert.match(await page.locator("body").innerText(), /확인|스크립트|자바스크립트|JavaScript|새로|다시/);
  const link = page.getByRole("link", { name: /다시.*(?:불러|시도)|새로고침|로그인/ });
  assert.equal(await link.count(), 1); assert.equal(await link.isVisible(), true);
  const href = new URL(await link.getAttribute("href"), page.url());
  assert.equal(href.origin, new URL(page.url()).origin);
  assert.ok(href.pathname === "/login/" || /^\/me\/[0-9a-f-]{36}\/attendance\/$/.test(href.pathname));
  return link;
}

async function assertBStillAuthorized(h) {
  const b=h.actors.get("b"); const session=h.sessions.get("b");
  const response=await b.page.goto(`${b.origin}/me/${session.context}/attendance/`);
  assert.equal(response.status(),200); await h.checkTable("b",b.facts.first_page);
  await b.page.reload(); await h.checkTable("b",b.facts.first_page);
  await h.checkpoint("b",session.context,session.token,"open",session.cookieExpires,session.signedCounter);
}

async function revokeWithoutCookieChange(h, actor, session) {
  const before = cookieState(await actor.context.cookies());
  assert.equal(selectedCookie(before, session)?.value, session.token);
  const result = await h.control({ op: "native-revoke", actor: "a", context: session.context,
    session_token: session.token });
  assert.equal(result.status, 204, "actual native owner must confirm exact-family closure");
  assert.deepEqual(cookieState(await actor.context.cookies()), before,
    "native revocation must not change browser cookies before the cache proof");
}

function assertRestored(events, original) {
  const restored = events.find((event) => event.event === "pageshow" && event.persisted && event.trusted
    && event.document === original);
  if (!restored) throw new Unreached("no actual trusted persisted:true pageshow for the original Document; no-store kept");
  assert.equal(restored.present, true); assert.equal(restored.hidden, true); assert.equal(restored.private_visible, false);
  const after = events.slice(events.indexOf(restored)).filter((event) => event.document === original);
  for (const event of after) { assert.equal(event.hidden, true); assert.equal(event.private_visible, false); }
  return restored;
}

async function backNavigation(page, start, h, original, probe, requireCache) {
  // A real cached Document's guard may supersede Back with its hard reload.
  // Only the known superseding navigation error is tolerated, with fresh-request
  // and private-denial evidence still mandatory at the caller.
  // Cached restoration commits without a new DOMContentLoaded event. Actual
  // original-Document pageshow and current-owner reload/denial remain required.
  const result = page.goBack({ waitUntil: "commit", timeout: h.phaseMs ?? 20000 })
    .then((response) => ({ response }), (error) => {
      if (!/ERR_ABORTED|interrupted by another navigation/i.test(error.message)) throw error;
      return { superseded: true };
    });
  void result.catch(() => {}); // Preserve the checked await while owning rejection during the restore observation.
  if (requireCache) {
    const restored = await until(() => probe.events.slice(start).find((event) => event.event === "pageshow"
      && event.trusted && event.persisted && event.document === original), h.phaseMs ?? 20000);
    if (!restored) {
      await result;
      throw new Unreached("Back did not restore the original cached Document; dispatched events are not accepted");
    }
  }
  return result;
}

async function ordinaryLogoutBack(h) {
  await h.signIn("a");
  const actor = h.actors.get("a"); const session = { ...h.sessions.get("a") };
  const page = await actor.context.newPage();
  const probe = await installProbe(page, h.guardContract.privateRegionSelector, actor.facts, false);
  try {
    await page.goto(`${actor.origin}/me/${session.context}/attendance/`);
    await page.getByRole("table").waitFor({ state: "visible", timeout: h.phaseMs ?? 20000 });
    const original = await page.evaluate((key) => window[key].document, PROBE);
    await probe.armPagehide(original);
    await page.goto(`${actor.origin}${LOGIN}`);
    await h.logout("a"); // Real visible product action, exact receipt, native effects and cookie removal.
    const restored = await page.goBack({ waitUntil: "domcontentloaded", timeout: h.phaseMs ?? 20000 })
      .then((value) => value, (error) => {
        if (!/ERR_ABORTED|interrupted by another navigation/i.test(error.message)) throw error;
        return null;
      });
    if (restored) h.assertNoProof(await restored.text());
    await assertPrivateHidden(page, actor, h);
    await page.reload({ waitUntil: "domcontentloaded" });
    await assertPrivateHidden(page, actor, h);
    await assertBStillAuthorized(h);
    assert.deepEqual(probe.errors, []);
    for (const event of probe.events.filter((event) => event.restored)) {
      assert.equal(event.hidden, true); assert.equal(event.private_visible, false);
    }
    return { ordinary_back: true, cached_restore_seen: probe.events.some((event) => event.persisted && event.trusted) };
  } finally { probe.remove(); await page.close(); }
}

async function cacheVariant(h, mode) {
  await h.signIn("a");
  const actor = h.actors.get("a"); const session = { ...h.sessions.get("a") };
  const url = `${actor.origin}/me/${session.context}/attendance/`;
  const page = await actor.context.newPage();
  const queued = mode === "queued-react-css";
  const probe = await installProbe(page, h.guardContract.privateRegionSelector, actor.facts, queued, mode === "delayed-bundles");
  const documents = []; const failedRequests = []; const heldScripts = [];
  const cacheFailures = [];
  let cacheObserver;
  // This diagnostic retains fixed browser enums only, never event IDs/URLs/details.
  const cacheReasonAllowlist = new Set([
    "BackForwardCacheDisabled", "RelatedActiveContentsExist", "Loading", "Timeout", "CacheLimit",
    "JavaScriptExecution", "RendererProcessKilled", "RendererProcessCrashed", "CacheFlushed",
    "TimeoutPuttingInCache", "BackForwardCacheDisabledByLowMemory", "BackForwardCacheDisabledByCommandLine",
    "NetworkRequestTimeout", "NetworkExceedsBufferLimit", "NavigationCancelledWhileRestoring",
    "CacheControlNoStore", "CacheControlNoStoreCookieModified", "CacheControlNoStoreHTTPOnlyCookieModified",
    "OutstandingNetworkRequestOthers", "OutstandingNetworkRequestFetch", "OutstandingNetworkRequestXHR",
    "InjectedJavascript", "InjectedStyleSheet", "KeepaliveRequest", "CacheLimitPrunedOnModerateMemoryPressure",
    "CacheLimitPrunedOnCriticalMemoryPressure", "ContentWebAuthenticationAPI", "Unknown",
  ]);
  const cacheFailureListener = ({ notRestoredExplanations }) => {
    for (const explanation of notRestoredExplanations ?? []) {
      if (cacheFailures.length >= 32) break;
      const type = ["SupportPending", "PageSupportNeeded", "Circumstantial"].includes(explanation.type)
        ? explanation.type : "unrecognized";
      const reason = cacheReasonAllowlist.has(explanation.reason) ? explanation.reason : "unrecognized";
      cacheFailures.push({ type, reason });
    }
  };
  const evidence = { mode, blocked_scripts: 0, blocked_styles: 0, transformed_documents: [] };
  let readGate; let back; let queuedResponse; let original; let consumedFault = false;
  let queuedOrderingObservation;
  let tearingDown = false;
  let readGateHeld = false;
  let readGateReleaseAttempted = false;
  let primaryError;
  async function releaseReadGate() {
    assert.ok(readGate && readGateHeld && !readGateReleaseAttempted, "release only an owned held native response once");
    readGateReleaseAttempted = true; // A lost receipt never permits a blind retry.
    const released = await readGate.release();
    assert.equal(released.gate, readGate.gate);
    assert.equal(released.released, true);
    readGate = undefined;
  }
  const requestListener = (request) => {
    if (request.isNavigationRequest() && request.frame() === page.mainFrame()) documents.push(request.url());
  };
  const failureListener = (request) => {
    if (request.isNavigationRequest() && request.frame() === page.mainFrame() && request.url() === url)
      failedRequests.push(request.failure()?.errorText ?? "");
  };
  page.on("request", requestListener); page.on("requestfailed", failureListener);
  const intercept = async (route) => {
    const request = route.request();
    if (new URL(request.url()).origin !== actor.origin) { await route.continue(); return; }
    if (request.resourceType() === "script" && ["blocked-bundles", "blocked-css-bundles"].includes(mode)) {
      evidence.blocked_scripts += 1; await route.abort("blockedbyclient"); return;
    }
    if (request.resourceType() === "stylesheet" && mode === "blocked-css-bundles") {
      evidence.blocked_styles += 1; await route.abort("blockedbyclient"); return;
    }
    if (request.resourceType() === "script" && mode === "delayed-bundles") {
      const response = await route.fetch({ timeout: h.phaseMs ?? 20000 }); // Real static bytes; no cookie/header-order claim.
      const body = await response.body();
      // A late real fetch must not create another hold after cleanup drained it.
      if (tearingDown) { await route.fulfill({ response, body }); return; }
      let release; const released = new Promise((resolve) => { release = resolve; });
      heldScripts.push({ release, digest: sha(body) });
      await released;
      await route.fulfill({ response, body });
      return;
    }
    if (request.isNavigationRequest() && request.url() === url && !consumedFault
      && ["missing-guard", "thrown-guard"].includes(mode)) {
      consumedFault = true;
      const response = await route.fetch({ timeout: h.phaseMs ?? 20000 }); const body = await response.body();
      assert.equal(response.status(), 200);
      const html = new TextDecoder("utf-8", { fatal: true }).decode(body);
      h.assertNoProof(html); assert.ok(html.includes(actor.facts.company_name) && html.includes(actor.facts.account_name));
      assert.match(response.headers()["cache-control"], /(?:^|,)\s*(?:private|no-store)(?:\s*[,;]|$)/);
      assert.match(response.headers()["cache-control"], /no-store/);
      const guard = findGuard(html, h.guardContract);
      const replacement = mode === "missing-guard" ? "" : `<script${guard[1]}>throw new Error('r7 guard fault');</script>`;
      const changed = html.slice(0, guard.index) + replacement + html.slice(guard.index + guard[0].length);
      const headers = { ...response.headers() };
      for (const name of ["content-length", "content-encoding", "etag", "content-md5", "digest"]) delete headers[name];
      evidence.transformed_documents.push({ upstream_status: response.status(),
        upstream_sha256: sha(body), delivered_sha256: sha(changed), pinned_guard_sha256: h.guardContract.inlineScriptSha256 });
      await route.fulfill({ response, headers, body: changed }); // Actual owner HTML with one explicit fault.
      return;
    }
    await route.continue();
  };
  const routed = ["blocked-bundles", "blocked-css-bundles", "delayed-bundles", "missing-guard", "thrown-guard"].includes(mode);
  try {
    if (queued) {
      cacheObserver = await actor.context.newCDPSession(page);
      cacheObserver.on("Page.backForwardCacheNotUsed", cacheFailureListener);
      await cacheObserver.send("Page.enable");
    }
    if (routed) await page.route("**/*", intercept);
    const response = await page.goto(url, { waitUntil: mode === "delayed-bundles" ? "commit" : "domcontentloaded" });
    assert.equal(response.status(), 200); const html = await response.text(); h.assertNoProof(html);
    assert.match(response.headers()["cache-control"], /no-store/);
    const region = page.locator(h.guardContract.privateRegionSelector);
    await region.waitFor({ state: "attached", timeout: h.phaseMs ?? 20000 });
    if (!["missing-guard", "thrown-guard"].includes(mode)) {
      const guard = findGuard(html, h.guardContract);
      const ordered = await page.evaluate(({ literal, selector }) => {
        const matches = [...document.querySelectorAll("script:not([src])")].filter((script) => script.textContent === literal);
        const region = document.querySelector(selector);
        return matches.length === 1 && Boolean(region)
          && Boolean(matches[0].compareDocumentPosition(region) & Node.DOCUMENT_POSITION_FOLLOWING);
      }, { literal: guard[2], selector: h.guardContract.privateRegionSelector });
      assert.equal(ordered, true, "pinned parser script must precede the real private DOM region");
    }
    assert.equal(await region.count(), 1);
    const inside = await region.textContent();
    assert.ok(inside.includes(actor.facts.company_name) && inside.includes(actor.facts.account_name));
    for (const item of actor.facts.first_page.items) assert.ok(inside.includes(item.note));
    original = await page.evaluate((key) => window[key].document, PROBE);
    const beforeHydration = ["blocked-bundles", "blocked-css-bundles", "delayed-bundles"].includes(mode);
    if (beforeHydration || ["missing-guard", "thrown-guard"].includes(mode)) {
      await assertPrivateHidden(page, actor, h);
      if (["missing-guard", "thrown-guard"].includes(mode)) await assertRecovery(page, h, mode);
    } else await page.getByRole("table").waitFor({ state: "visible", timeout: h.phaseMs ?? 20000 });
    if (queued) {
      const initial = await page.evaluate((key) => window[key].reactCommits, PROBE);
      assert.ok(initial > 0, "real installed React must have committed before queueing its actual logout handler");
      // An ordinary author stylesheet cannot undo the guarded restore latch.
      await page.addStyleTag({ content: `${h.guardContract.privateRegionSelector} { display:block; visibility:visible; }` });
    }
    if (mode === "delayed-bundles" && heldScripts.length === 0) {
      if (!await until(() => heldScripts.length > 0, h.phaseMs ?? 20000)) throw new Unreached("no real bundle response was delayed");
    }
    const departureStart = probe.events.length;
    await probe.armPagehide(original);
    await page.goto(`${actor.origin}${LOGIN}`, { waitUntil: mode === "delayed-bundles" ? "commit" : "domcontentloaded" });
    // Chromium may deliver cached-context console records only on restoration.
    // The trusted departure sample remains required after the real Back below.
    await revokeWithoutCookieChange(h, actor, session);
    if (["delayed-bundles", "queued-react-css"].includes(mode)) {
      if (typeof h.holdNextRead !== "function") throw new Unreached("actual native response gate adapter required");
      readGate = await h.holdNextRead({ actor: "a", context: session.context });
    }
    if (mode === "offline") await actor.context.setOffline(true);
    const eventStart = probe.events.length; const requestStart = documents.length;
    if (queued) {
      queuedResponse = page.waitForResponse((result) => new URL(result.url()).origin === actor.origin
        && new URL(result.url()).pathname === LOGOUT && result.request().method() === "POST"
        && result.request().postDataJSON()?.browser_context === session.context,
      { timeout: h.phaseMs ?? 20000 }).then(async (result) => {
        await h.assertNoContentResponse(result);
        return true;
      });
      void queuedResponse.catch(() => {}); // Preserve checked failure while owning the outstanding promise.
    }
    // Keep this promise owned: a gate must be released before awaiting the final reload.
    back = backNavigation(page, eventStart, h, original, probe, true);
    void back.catch(() => {}); // No unhandled rejection while the independently owned response gate waits.
    const seen = await until(() => probe.events.slice(eventStart).find((event) => event.event === "pageshow"
      && event.trusted && event.persisted && event.document === original), h.phaseMs ?? 20000);
    if (!seen) {
      assert.deepEqual(probe.errors, []);
      // Cleanup closes the owned page and settles Back without inventing a
      // response or releasing the private logout latch when no witness exists.
      throw new Unreached("actual BFCache restoration unavailable for this fault; no-store not weakened");
    }
    const restored = assertRestored(probe.events.slice(eventStart), original);
    // A later departure caused by the guard's reload cannot satisfy this check.
    const hiddenEvent = probe.events.slice(departureStart, probe.events.indexOf(restored))
      .find((event) => event.document === original && event.event === "pagehide"
        && event.trusted && !event.restored);
    if (!hiddenEvent) throw new Unreached("trusted original departure observation unavailable before restoration");
    assert.equal(hiddenEvent.persisted, true);
    assert.equal(hiddenEvent.hidden, true); assert.equal(hiddenEvent.private_visible, false);
    if (readGate) {
      const held = await readGate.wait();
      assert.equal(held.gate, readGate.gate);
      readGateHeld = true; // Preserve release ownership even if actual status is unexpected.
      if (queued) {
        assert.equal(held.context, session.context);
        assert.equal(held.status, 401);
        const pending = await page.evaluate(({ key, original }) => {
          const state = window[key];
          return { original: state?.document === original, restored: state?.restored,
            queued: state?.logoutQueued, started: state?.logoutStarted };
        }, { key: PROBE, original });
        // Record actual booleans at the verified release point before asserting.
        // This is not a claim about when the request first arrived at the server.
        const observedBoolean = (value) => typeof value === "boolean" ? value : null;
        queuedOrderingObservation = {
          stage: "verified-native-denial",
          native_gate_match: held.gate === readGate.gate,
          native_context_match: held.context === session.context,
          native_status_401: held.status === 401,
          pending: { original: observedBoolean(pending.original), restored: observedBoolean(pending.restored),
            queued: observedBoolean(pending.queued), started: observedBoolean(pending.started) },
          pending_matches_expected: pending.original === true && pending.restored === true
            && pending.queued === true && pending.started === false,
        };
        evidence.queued_ordering_observation = queuedOrderingObservation;
        assert.deepEqual(pending, { original: true, restored: true, queued: true, started: false },
          "actual queued logout must remain pending until the exact native reload denial is witnessed");
      }
      if (mode === "delayed-bundles") {
        for (const script of heldScripts) script.release();
        const committed=await until(() => probe.events.slice(eventStart).find((event) => event.document===original
          && event.event==="react-commit" && event.restored),h.phaseMs ?? 20000);
        if (!committed) throw new Unreached("released real bundle bytes did not execute React in the original restored Document");
        assert.equal(committed.hidden,true); assert.equal(committed.private_visible,false);
      }
      if (queued) {
        const commitsBeforeRelease = await page.evaluate(({ key, original }) => window[key]?.releaseLogout(original),
          { key: PROBE, original });
        assert.ok(Number.isSafeInteger(commitsBeforeRelease) && commitsBeforeRelease > 0,
          "release one queued logout only in the original restored Document after the actual native denial");
        const committed = await until(() => probe.events.slice(eventStart).find((event) => event.document === original
          && event.event === "react-commit" && event.restored && event.react_commits > commitsBeforeRelease), h.phaseMs ?? 20000);
        if (!committed) throw new Unreached("actual restored React update was not witnessed while native reload waited");
        assert.equal(committed.hidden, true); assert.equal(committed.private_visible, false);
        await queuedResponse;
        assert.ok(!selectedCookie(await actor.context.cookies(), session), "only the actual confirmed logout clears its cookie");
      }
      await assertPrivateHidden(page, actor, h);
      await releaseReadGate();
    }
    await back;
    assertRestored(probe.events.slice(eventStart), original);
    if (!["missing-guard", "thrown-guard"].includes(mode)) {
      if (mode === "offline") assert.ok(await until(() =>
        failedRequests.some((text) => /INTERNET_DISCONNECTED/.test(text)), h.phaseMs ?? 20000),
      "real offline failure of the exact main-frame reload required");
      else {
        const freshDocument = await until(async () => {
          try {
            const identity = await page.evaluate((key) => window[key]?.document, PROBE);
            return identity && identity !== original ? identity : null;
          } catch (error) {
            if (/Execution context was destroyed|Cannot find context/i.test(error.message)) return null;
            throw error;
          }
        }, h.phaseMs ?? 20000);
        assert.ok(freshDocument, "fresh native denial must replace the cached Document");
      }
      assert.equal(documents.slice(requestStart).filter((document) => document === url).length, 1,
        "persisted restore must initiate exactly one real hard reload");
    } else {
      const link = await assertRecovery(page, h, mode);
      await link.click();
    }
    await assertPrivateHidden(page, actor, h);
    if (mode === "offline") {
      await actor.context.setOffline(false);
      await page.goto(url); await assertPrivateHidden(page, actor, h);
    }
    if (["blocked-bundles", "blocked-css-bundles"].includes(mode)) assert.ok(evidence.blocked_scripts > 0);
    if (mode === "blocked-css-bundles") assert.ok(evidence.blocked_styles > 0);
    if (mode === "delayed-bundles") evidence.delayed_bundle_sha256 = heldScripts.map((script) => script.digest);
    evidence.trusted_original_restore = true;
    evidence.original_document = original;
    await assertBStillAuthorized(h);
    assert.deepEqual(probe.errors, []);
    return evidence;
  } catch (error) {
    primaryError = error;
    if (queued) error.cacheObservation = {
      queued_ordering_observation: queuedOrderingObservation ?? null,
      not_restored_reasons: cacheFailures,
      trusted_departures: probe.events.filter((event) => event.event === "pagehide" && event.trusted && event.document === original).length,
      trusted_original_restores: probe.events.filter((event) => event.event === "pageshow" && event.trusted && event.persisted && event.document === original).length,
    };
    throw error;
  } finally {
    tearingDown = true;
    const cleanupFailures = [];
    async function cleanup(step, operation) {
      try { await operation(); } catch { cleanupFailures.push(step); }
    }
    await cleanup("network-online", () => actor.context.setOffline(false));
    if (readGate) {
      if (!readGateHeld) cleanupFailures.push("native-gate-unheld");
      else if (readGateReleaseAttempted) cleanupFailures.push("native-gate-release-unconfirmed");
      else await cleanup("native-gate-release", releaseReadGate);
    }
    await cleanup("script-release", () => { for (const script of heldScripts) script.release(); });
    if (routed) await cleanup("routes-remove", () => page.unrouteAll({ behavior: "wait" }));
    await cleanup("probe-listeners", () => {
      cacheObserver?.off("Page.backForwardCacheNotUsed", cacheFailureListener);
      probe.remove(); page.off("request", requestListener); page.off("requestfailed", failureListener);
    });
    // Always attempt owned page closure, independently of earlier cleanup errors.
    // Closing also settles a pending Back when its native gate was never held.
    await cleanup("page-close", () => page.close());
    if (back) await cleanup("back-navigation", () => back);
    // Each fresh real sign-in is closed to avoid accidentally testing a ninth
    // session-cookie admission denial in an unrelated restore scenario.
    await cleanup("session-logout", async () => {
      if (selectedCookie(await actor.context.cookies(), session)) await h.logout("a");
    });
    if (cleanupFailures.length) {
      const error = primaryError ?? new Error("owned restore cleanup failed");
      error.cleanupObservation = { cleanup_failed_steps: cleanupFailures };
      if (!primaryError) throw error;
    }
  }
}

export async function runRestoreScenarios(h) {
  const reports = [];
  let prerequisites; let prerequisiteStatus = "unreached"; let prerequisiteReason;
  try {
    if (typeof h.control !== "function") throw new Unreached("actual native owner-control adapter absent");
    h.guardContract = await measureGuard(h); // Complete before installing any HTML/script/network fault.
    await h.logout("a"); // Close measured A generation before each variant creates its own.
    prerequisites = true;
  } catch (error) {
    prerequisiteStatus = isPrerequisite(error) ? "unreached" : "failed";
    prerequisiteReason = isPrerequisite(error) ? error.message : "actual unfaulted guard-measurement assertion failed";
  }
  for (const [id, variants] of [
    ["P15", [["ordinary-logout-back", ordinaryLogoutBack], ["native-revocation-real-bfcache", (input) => cacheVariant(input, "normal")]]],
    ["P16", ["blocked-bundles", "blocked-css-bundles", "delayed-bundles", "queued-react-css",
      "offline", "missing-guard", "thrown-guard"].map((mode) => [mode, (input) => cacheVariant(input, mode)])],
  ]) {
    h.begin(id);
    const checks = [];
    for (const [name, run] of variants) {
      if (!prerequisites) { checks.push({ id: name, status: prerequisiteStatus, reason: prerequisiteReason }); continue; }
      try { checks.push({ id: name, status: "passed", evidence: await run(h) }); }
      catch (error) {
        checks.push({ id: name, status: error.cleanupObservation ? "failed" : isPrerequisite(error) ? "unreached" : "failed",
          reason: isPrerequisite(error) ? error.message : "actual required restore assertion failed",
          ...(error.cacheObservation || error.cleanupObservation
            ? { diagnostic: { ...error.cacheObservation, ...error.cleanupObservation } } : {}) });
        // Continue only within this owned test runner. Failures remain failures.
      }
    }
    const status = checks.some((check) => check.status === "failed") ? "failed"
      : checks.every((check) => check.status === "passed") ? "passed" : "unreached";
    if (status === "passed") h.pass(id);
    reports.push({ id, status, ...(prerequisites ? { measured_guard_sha256: h.guardContract.inlineScriptSha256,
      measured_guard_documents: 2 } : {}), checks });
  }
  // Caller must attach all variant results, set failed/unreached explicitly and
  // clear its activeScenario. A running scenario is never implicit approval.
  return reports;
}
