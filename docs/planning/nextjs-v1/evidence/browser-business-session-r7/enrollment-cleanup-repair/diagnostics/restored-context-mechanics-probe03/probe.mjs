// Isolated browser mechanics. Fixture HTML/timer are not a business integration,
// authentication result, real BFF result, product acceptance or release proof.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { writeSync } from 'node:fs';
import { access, mkdtemp, readFile, rm } from 'node:fs/promises';
import { createServer } from 'node:https';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const ROOT = '/Users/jasonlee/Developer/frontend/.worktrees/browser-business-session-r7';
const LAYOUT_SHA = 'c7049779098c70c199715872fb7b3d4774c4cf52e2f21bafcaa0993f817931b7';
const SDK_SHA = '549070af3acabb3efcc4f55bfe6210f9f7c2fcf633cf7eaa59bfe60719969171';
const GUARD_SHA = 'a1c04481ce25603ade8e8fcd645ec27ff4730b446a4c54d81680b5c2fac76d97';
const PHASE_MS = 5000;
const EVALUATION_MS = 20000;
const FIXTURE_RESPONSE_MS = 15000;
const beganAt = performance.now();
const interrupted = new AbortController();
const events = [];
let checkpoint = 'starting';
let stage; let stageCreation; let stageCreationSettled = false;
let server; let browserServer; let browser;
let browserPid; let fixtureTimer; let launch;
let fixtureRequestSeen = false; let fixtureResponseEnded = false;
const cleanup = { browser_owner_returned: false, browser_close_settled: false,
  browser_kill_settled: false, browser_process_exit_seen: null,
  browser_owner_process_live_seen: null, browser_owned_group_confirmed: null,
  browser_group_absent: null, stage_creation_settled: null,
  server_closed: null, owned_stage_removed: null };
const report = { schema: 'browser-mechanics-probe/3',
  scope: 'Actual Chromium mechanics with fixture HTTPS HTML and a fixture15s reload response; no business authentication/BFF/provider acceptance',
  product_runtime_proof: false, fixture_response_ms: FIXTURE_RESPONSE_MS,
  evaluation_budget_ms: EVALUATION_MS, phase_budget_ms: PHASE_MS,
  trusted_persisted_restore_seen: false, fixture_reload_request_seen: false,
  exact_original_context_pinned: false, exact_original_document_handle_pinned: false,
  evaluation: null, original_document_handle_evaluation: null,
  fresh_fixture_response: null, mechanism: 'inconclusive', cleanup };
function elapsed() { return Math.min(60000, Math.max(0, Math.floor(performance.now() - beganAt))); }
function emit(value) { writeSync(1, JSON.stringify(value) + '\n'); }
function mark(name, fields = {}) {
  checkpoint = name;
  const event = { checkpoint: name, observed_elapsed_ms: elapsed(), ...fields };
  if (events.length < 64) events.push(event);
  emit({ schema: 'browser-mechanics-checkpoint/3', ...event });
}
async function bounded(promise, ms, label, respectInterrupt = true) {
  let timer; let onAbort;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(Object.assign(new Error(), { mechanicsDeadline: label })), ms);
      if (respectInterrupt) {
        onAbort = () => reject(Object.assign(new Error(), { mechanicsInterrupted: true }));
        interrupted.signal.addEventListener('abort', onAbort, { once: true });
        if (interrupted.signal.aborted) onAbort();
      }
    })]);
  } finally {
    clearTimeout(timer);
    if (onAbort) interrupted.signal.removeEventListener('abort', onAbort);
  }
}
function failureKind(error) {
  return error?.mechanicsInterrupted ? 'interrupted' : error?.mechanicsDeadline ? 'phase-deadline'
    : error?.name === 'AssertionError' ? 'assertion' : 'unclassified';
}
function groupAbsent() {
  if (!Number.isSafeInteger(browserPid) || browserPid <= 1) return null;
  try { process.kill(-browserPid, 0); return false; }
  catch (error) { return error.code === 'ESRCH' ? true : null; }
}
const hard = setTimeout(() => {
  emit({ ...report, status: 'hard-deadline-unconfirmed', last_checkpoint: checkpoint,
    observed_elapsed_ms: elapsed(), events: [...events] });
  process.exit(3); // Never infer browser/group/stage absence from this exit.
}, 60000);
process.once('SIGTERM', () => interrupted.abort());
process.once('SIGINT', () => interrupted.abort());
let exitCode = 1;
try {
  mark('source-pins');
  const [layout, sdk] = await bounded(Promise.all([
    readFile(path.join(ROOT, 'src/app/layout.public.tsx')),
    readFile(path.join(ROOT, 'node_modules/playwright-core/lib/coreBundle.js')),
  ]), PHASE_MS, 'source-pins');
  const sha = bytes => createHash('sha256').update(bytes).digest('hex');
  assert.equal(sha(layout), LAYOUT_SHA); assert.equal(sha(sdk), SDK_SHA);
  const guard = layout.toString('utf8').match(/__html: `([^`]+)`/);
  assert.ok(guard && guard[1].includes('location.reload()'));
  assert.equal(sha(guard[1]), GUARD_SHA);
  report.layout_sha256 = LAYOUT_SHA; report.sdk_sha256 = SDK_SHA;
  report.guard_sha256 = GUARD_SHA;
  mark('owned-stage-create');
  // Retain assignment even if the bounded wait loses. Pending creation is
  // explicitly UNKNOWN custody; !stage alone is never proof of no directory.
  stageCreation = mkdtemp(path.join(tmpdir(), 'r7-mechanics03-')).then(created => {
    stage = created; stageCreationSettled = true; return created;
  }, error => { stageCreationSettled = true; throw error; });
  await bounded(stageCreation, PHASE_MS, 'owned-stage');
  const certFile = path.join(stage, 'fixture-cert.pem');
  const keyFile = path.join(stage, 'fixture-key.pem');
  mark('fixture-certificate-create');
  execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes',
    '-keyout', keyFile, '-out', certFile, '-days', '1', '-subj', '/CN=localhost',
    '-addext', 'subjectAltName=DNS:localhost,IP:127.0.0.1'],
  { timeout: 3000, stdio: 'ignore' });
  const [cert, key] = await bounded(Promise.all([readFile(certFile), readFile(keyFile)]), PHASE_MS, 'fixture-certificate-read');
  const fixtureHeld = Promise.withResolvers();
  let originalLoads = 0;
  server = createServer({ cert, key }, (request, response) => {
    const isOriginal = request.url === '/original';
    if (isOriginal && ++originalLoads === 2) {
      fixtureRequestSeen = true; report.fixture_reload_request_seen = true;
      mark('fixture-reload-request'); fixtureHeld.resolve();
      fixtureTimer = setTimeout(() => {
        mark('fixture-response-timer-fired');
        if (response.destroyed) { mark('fixture-response-connection-lost'); return; }
        response.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'private, no-store' });
        response.end('<!doctype html><html><body><h1>Fixture unavailable</h1></body></html>');
        fixtureResponseEnded = true; mark('fixture-response-ended');
      }, FIXTURE_RESPONSE_MS);
      return;
    }
    response.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'private, no-store' });
    response.end(isOriginal
      ? `<!doctype html><html><body><script id="console-browser-restoration-guard">${guard[1]}</script><div id="console-browser-private-region"><h1>Fixture original</h1></div></body></html>`
      : '<!doctype html><html><body><h1>Fixture other</h1></body></html>');
  });
  mark('fixture-server-listen');
  const listening = once(server, 'listening'); server.listen(0, '127.0.0.1');
  await bounded(listening, PHASE_MS, 'fixture-listen');
  const origin = `https://localhost:${server.address().port}`;
  // This is a standalone process. Scope both SDK parent os.tmpdir() and child
  // temp creation before requiring/launching the browser owner.
  process.env.TMPDIR = stage;
  const { chromium } = createRequire(path.join(ROOT, 'package.json'))('@playwright/test');
  const cleanEnv = Object.fromEntries(['PATH', 'HOME', 'LANG', 'LC_ALL', 'PLAYWRIGHT_BROWSERS_PATH']
    .filter(name => process.env[name] !== undefined).map(name => [name, process.env[name]]));
  // Keep SDK/browser temporary files inside this owned disposable stage.
  cleanEnv.TMPDIR = stage;
  mark('browser-launch');
  launch = chromium.launchServer({ channel: 'chromium', env: cleanEnv,
    timeout: PHASE_MS, ignoreDefaultArgs: ['--disable-back-forward-cache'] })
    .then(owner => { browserServer = owner; browserPid = owner.process().pid;
      cleanup.browser_owner_returned = true; return owner; });
  await launch;
  const processOwner = browserServer.process();
  cleanup.browser_owner_process_live_seen = processOwner.exitCode === null && processOwner.signalCode === null;
  // SDK source is pinned and launches detached on this POSIX platform. Confirm
  // the actual captured owner is a live leader of its own OS process group.
  assert.notEqual(process.platform, 'win32');
  assert.ok(Number.isSafeInteger(browserPid) && browserPid > 1);
  const observedPgid = execFileSync('ps', ['-o', 'pgid=', '-p', String(browserPid)],
    { timeout: 1000, maxBuffer: 128, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
  cleanup.browser_owned_group_confirmed = /^\d+$/.test(observedPgid)
    && Number(observedPgid) === browserPid && groupAbsent() === false
    && cleanup.browser_owner_process_live_seen;
  assert.equal(cleanup.browser_owned_group_confirmed, true);
  mark('owned-live-browser-group-confirmed', { owned_live_group_confirmed: true });
  mark('browser-connect');
  browser = await bounded(chromium.connect(browserServer.wsEndpoint(), { timeout: PHASE_MS }), PHASE_MS, 'browser-connect');
  const context = await bounded(browser.newContext({ ignoreHTTPSErrors: true }), PHASE_MS, 'browser-context');
  const page = await bounded(context.newPage(), PHASE_MS, 'browser-page');
  const cdp = await bounded(context.newCDPSession(page), PHASE_MS, 'cdp-session');
  const frameId = (await bounded(cdp.send('Page.getFrameTree'), PHASE_MS, 'frame-tree')).frameTree.frame.id;
  let lastWorld; let originalWorld;
  cdp.on('Runtime.executionContextCreated', ({ context: created }) => {
    if (!created.auxData?.isDefault || created.auxData.frameId !== frameId) return;
    lastWorld = created.uniqueId;
    if (originalWorld) mark('main-world-created', { matches_original: created.uniqueId === originalWorld });
  });
  cdp.on('Runtime.executionContextsCleared', () => { if (originalWorld) mark('execution-contexts-cleared'); });
  cdp.on('Runtime.executionContextDestroyed', event => {
    if (originalWorld && event.executionContextUniqueId === originalWorld) mark('original-context-destroyed');
  });
  cdp.on('Page.frameNavigated', ({ frame }) => {
    if (fixtureRequestSeen && frame.id === frameId) mark('main-frame-navigated', { matches_fixture_original_path: frame.url === origin + '/original' });
  });
  await bounded(cdp.send('Runtime.enable'), PHASE_MS, 'runtime-enable');
  await bounded(cdp.send('Page.enable'), PHASE_MS, 'page-enable');
  const restoration = Promise.withResolvers();
  let original;
  page.on('console', message => {
    // Read the transient token only to bind this real trusted restoration to the
    // pinned original Document. Publish no token, URL, raw console or payload.
    const text = message.text();
    if (!text.startsWith('MECHANICS02_RESTORE ') || text.length > 512) return;
    try {
      const value = JSON.parse(text.slice('MECHANICS02_RESTORE '.length));
      if (!original || value.document !== original.document || value.trusted !== true || value.persisted !== true) return;
      report.trusted_persisted_restore_seen = true;
      mark('trusted-persisted-original-restore', { original_document_match: true }); restoration.resolve();
    } catch { /* Malformed observations cannot satisfy the bounded prerequisite. */ }
  });
  await bounded(page.addInitScript(() => {
    const state = { document: crypto.randomUUID(), restored: false, queued: false, started: false };
    globalThis.__mechanics02 = state;
    const latch = new Promise(() => {}); // This probe never releases any queued action.
    addEventListener('pageshow', event => {
      if (!event.isTrusted || !event.persisted) return;
      state.restored = true; state.queued = true;
      setTimeout(async () => { await latch; state.started = true; }, 0);
      console.debug('MECHANICS02_RESTORE ' + JSON.stringify({ document: state.document,
        trusted: event.isTrusted, persisted: event.persisted }));
    });
  }), PHASE_MS, 'fixture-observer-install');
  mark('original-navigation');
  await page.goto(origin + '/original', { waitUntil: 'domcontentloaded', timeout: PHASE_MS });
  original = await bounded(page.evaluate(() => ({ document: globalThis.__mechanics02.document, secure: isSecureContext })), PHASE_MS, 'original-identity');
  assert.equal(original.secure, true); assert.equal(typeof lastWorld, 'string');
  originalWorld = lastWorld;
  const pinned = await bounded(cdp.send('Runtime.evaluate', { uniqueContextId: originalWorld,
    expression: `globalThis.__mechanics02.document === ${JSON.stringify(original.document)}`, returnByValue: true, timeout: EVALUATION_MS }), PHASE_MS, 'original-pin');
  assert.equal(pinned.exceptionDetails, undefined); assert.equal(pinned.result.value, true);
  report.exact_original_context_pinned = true; mark('original-world-pinned');
  const originalDocumentHandle = await bounded(page.evaluateHandle(() => document), PHASE_MS, 'original-document-handle');
  assert.equal(await bounded(originalDocumentHandle.evaluate((doc, token) => doc === document
    && globalThis.__mechanics02.document === token, original.document), PHASE_MS, 'original-handle-pin'), true);
  report.exact_original_document_handle_pinned = true; mark('original-document-handle-pinned');
  await page.goto(origin + '/other', { waitUntil: 'domcontentloaded', timeout: PHASE_MS });
  const freshResponse = page.waitForResponse(response => response.url() === origin + '/original'
    && response.request().isNavigationRequest() && response.request().frame() === page.mainFrame(), { timeout: EVALUATION_MS });
  void freshResponse.catch(() => {});
  const freshObservation = freshResponse.then(async response => {
    mark('fresh-response-observed', { http_status: response.status() });
    const html = await bounded(response.text(), PHASE_MS, 'fresh-response-body');
    const observed = { http_status: response.status(), fixture_unavailable_heading: html.includes('<h1>Fixture unavailable</h1>') };
    mark('fresh-response-body-observed', observed); return observed;
  });
  void freshObservation.catch(() => {});
  mark('back-issued');
  const back = page.goBack({ waitUntil: 'commit', timeout: PHASE_MS });
  void back.catch(() => {});
  await bounded(Promise.all([fixtureHeld.promise, restoration.promise]), PHASE_MS, 'actual-restore-and-held-reload');
  mark('exact-original-evaluation-issued');
  const evaluationBegan = performance.now();
  const observationsDeadline = evaluationBegan + EVALUATION_MS;
  const remainingBudget = () => {
    const remaining = Math.floor(observationsDeadline - performance.now());
    if (remaining <= 0) throw Object.assign(new Error(), { mechanicsDeadline: 'shared-observation-deadline' });
    return remaining;
  };
  const evaluation = cdp.send('Runtime.evaluate', { uniqueContextId: originalWorld,
    returnByValue: true, timeout: EVALUATION_MS,
    expression: `(() => { const state = globalThis.__mechanics02; return { original: state.document === ${JSON.stringify(original.document)}, restored: state.restored, queued: state.queued, started: state.started }; })()` });
  void evaluation.catch(() => {});
  // Comparison only. This exact original-Document handle is pinned before
  // departure and is never substituted for the required unique-context result.
  // Both commands share one20s observation deadline; there is no extended wait.
  mark('exact-original-document-handle-evaluation-issued');
  const handleEvaluation = originalDocumentHandle.evaluate((doc, token) => {
    const state = globalThis.__mechanics02;
    return { original: doc === document && state.document === token, restored: state.restored,
      queued: state.queued, started: state.started };
  }, original.document);
  void handleEvaluation.catch(() => {});
  const handleObservation = bounded(handleEvaluation, remainingBudget(), 'original-document-handle-evaluation')
    .then(value => ({ outcome: 'returned-value', cdp_response_received: null, local_deadline_fired: false,
      observed_elapsed_ms: Math.max(0, Math.floor(performance.now() - evaluationBegan)),
      pending_matches_expected: value.original === true && value.restored === true
        && value.queued === true && value.started === false }), error => ({
      outcome: error?.mechanicsDeadline ? 'local-deadline' : error?.mechanicsInterrupted ? 'interrupted' : 'handle-owner-rejected',
      cdp_response_received: null, local_deadline_fired: Boolean(error?.mechanicsDeadline),
      observed_elapsed_ms: Math.max(0, Math.floor(performance.now() - evaluationBegan)), pending_matches_expected: null,
    }));
  try {
    const value = await bounded(evaluation, remainingBudget(), 'original-evaluation');
    report.evaluation = { outcome: value.exceptionDetails ? 'returned-exception' : 'returned-value',
      observed_elapsed_ms: Math.max(0, Math.floor(performance.now() - evaluationBegan)), local_deadline_fired: false,
      cdp_response_received: true, exception_returned: Boolean(value.exceptionDetails),
      pending_matches_expected: !value.exceptionDetails && value.result?.value?.original === true
        && value.result.value.restored === true && value.result.value.queued === true && value.result.value.started === false };
  } catch (error) {
    report.evaluation = { outcome: error?.mechanicsDeadline ? 'local-deadline' : error?.mechanicsInterrupted ? 'interrupted' : 'cdp-rejected',
      observed_elapsed_ms: Math.max(0, Math.floor(performance.now() - evaluationBegan)),
      local_deadline_fired: Boolean(error?.mechanicsDeadline), cdp_response_received: false, exception_returned: null,
      pending_matches_expected: null };
  }
  mark('exact-original-evaluation-settled', report.evaluation);
  report.original_document_handle_evaluation = await handleObservation;
  mark('exact-original-document-handle-evaluation-settled', report.original_document_handle_evaluation);
  report.fresh_fixture_response = await bounded(freshObservation, remainingBudget(), 'fresh-response-completion');
  await bounded(back.then(() => 'returned', () => 'rejected'), PHASE_MS, 'back-settlement');
  const rejectedAfterResponse = report.evaluation.outcome === 'cdp-rejected'
    && fixtureResponseEnded && report.fresh_fixture_response.fixture_unavailable_heading === true
    && events.some(event => event.checkpoint === 'main-frame-navigated' && event.matches_fixture_original_path === true)
    && events.findIndex(event => event.checkpoint === 'fixture-response-ended')
      < events.findIndex(event => event.checkpoint === 'exact-original-evaluation-settled');
  report.mechanism = rejectedAfterResponse ? 'fixture-reproduced'
    : report.evaluation.outcome === 'returned-value' && report.evaluation.pending_matches_expected === true
      ? 'fixture-not-reproduced' : 'inconclusive';
  mark('observations-complete', { mechanism: report.mechanism });
  exitCode = report.mechanism === 'inconclusive' ? 1 : 0;
} catch (error) {
  report.failure = { category: failureKind(error), checkpoint };
  mark('mechanics-failure', { category: report.failure.category });
} finally {
  mark('cleanup-start');
  clearTimeout(fixtureTimer);
  if (stageCreation) await bounded(stageCreation.catch(() => {}), 1000, 'stage-creation-settlement', false).catch(() => {});
  cleanup.stage_creation_settled = stageCreation ? stageCreationSettled : null;
  if (launch) await bounded(launch.catch(() => {}), 6000, 'launch-owner-settlement', false).catch(() => {});
  if (browserServer) {
    try { await bounded(browserServer.close(), 2000, 'browser-close', false); cleanup.browser_close_settled = true; }
    catch {
      try { await bounded(browserServer.kill(), 2000, 'browser-kill', false); cleanup.browser_kill_settled = true; } catch {}
    }
    const processOwner = browserServer.process();
    cleanup.browser_process_exit_seen = processOwner.exitCode !== null || processOwner.signalCode !== null;
    const groupDeadline = performance.now() + 1000;
    do { cleanup.browser_group_absent = groupAbsent();
      if (cleanup.browser_group_absent === true) break;
      await delay(10);
    } while (performance.now() < groupDeadline);
  }
  if (server) {
    server.closeAllConnections();
    try { await bounded(new Promise(resolve => server.close(resolve)), 1000, 'fixture-server-close', false); cleanup.server_closed = true; }
    catch { cleanup.server_closed = false; }
  }
  const browserCustodyConfirmed = !launch || cleanup.browser_owner_returned === true
    && cleanup.browser_owned_group_confirmed === true
    && (cleanup.browser_close_settled === true || cleanup.browser_kill_settled === true)
    && cleanup.browser_process_exit_seen === true && cleanup.browser_group_absent === true;
  // Do not erase a possibly live browser's profile/certificate custody.
  if (stage && browserCustodyConfirmed) {
    try { await bounded(rm(stage, { recursive: true, force: true }), 1000, 'owned-stage-remove', false);
      try { await access(stage); cleanup.owned_stage_removed = false; }
      catch (error) { cleanup.owned_stage_removed = error.code === 'ENOENT'; }
    } catch { cleanup.owned_stage_removed = false; }
  }
  const custodyConfirmed = browserCustodyConfirmed
    && (!stageCreation || cleanup.stage_creation_settled === true)
    && (!server || cleanup.server_closed === true) && (!stage || cleanup.owned_stage_removed === true);
  if (!custodyConfirmed) exitCode = 3;
  mark('cleanup-complete', { custody_confirmed: custodyConfirmed });
  if (custodyConfirmed) clearTimeout(hard);
  else hard.unref(); // Keep the60s bound if unresolved owners keep this process alive.
  emit({ ...report, status: custodyConfirmed ? 'terminal-observations' : 'terminal-cleanup-unconfirmed',
    last_checkpoint: checkpoint, observed_elapsed_ms: elapsed(), events });
  process.exitCode = exitCode;
}
