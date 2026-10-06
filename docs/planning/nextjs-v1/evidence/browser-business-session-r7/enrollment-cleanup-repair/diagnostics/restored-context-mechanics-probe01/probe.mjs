// Isolated browser mechanics only: no Accounts, business API or provider result.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
const require = createRequire('/Users/jasonlee/Developer/frontend/.worktrees/browser-business-session-r7/package.json');
const { chromium } = require('@playwright/test');
let browserServer; let browser; let releaseHeld; let heldSeen;
const held = new Promise(resolve => { heldSeen = resolve; });
let loads = 0;
const server = createServer((req,res) => {
  if (req.url === '/original' && ++loads > 1) {
    releaseHeld = () => res.writeHead(401, { 'content-type':'text/html', 'cache-control':'no-store' }).end('<p>Fresh response</p>');
    heldSeen(); return;
  }
  res.writeHead(200, {'content-type':'text/html','cache-control':'no-store'});
  res.end(req.url === '/original' ? '<p>Original document</p>' : '<p>Other document</p>');
});
const report = { scope:'Browser mechanics only; not authentication or product acceptance', persisted_restore:false };
let terminal = 1;
const hard = setTimeout(() => process.exit(3), 30000);
try {
  server.listen(0, '127.0.0.1'); await once(server,'listening');
  const origin = `http://127.0.0.1:${server.address().port}`;
  browserServer = await chromium.launchServer({channel:'chromium',ignoreDefaultArgs:['--disable-back-forward-cache'],timeout:5000});
  browser = await chromium.connect(browserServer.wsEndpoint());
  const context = await browser.newContext(); const page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  const frame = (await cdp.send('Page.getFrameTree')).frameTree.frame.id;
  let world;
  cdp.on('Runtime.executionContextCreated', ({context}) => {
    if (context.auxData?.isDefault && context.auxData.frameId === frame) world = context.uniqueId;
  });
  await cdp.send('Runtime.enable');
  let restored;
  const restoredSeen = new Promise(resolve => { restored = resolve; });
  page.on('console', message => { if (message.text()==='MECHANICS_REAL_RESTORE') restored(); });
  await page.addInitScript(() => {
    const state = {document:crypto.randomUUID(),restored:false,queued:false,started:false};
    let release; const latch = new Promise(resolve => { release = resolve; });
    state.release = original => { if (state.document !== original || !state.restored || !state.queued || state.started) return false;release();return true; };
    globalThis.__mechanics = state;
    addEventListener('pageshow', event => {
      if (!event.isTrusted || !event.persisted) return;
      state.restored=true;state.queued=true;
      setTimeout(async()=>{ await latch; state.started=true; console.debug('MECHANICS_REAL_START'); },0);
      console.debug('MECHANICS_REAL_RESTORE'); location.reload();
    });
  });
  await page.goto(origin+'/original');
  const original = await page.evaluate(()=>globalThis.__mechanics.document);
  assert.equal(typeof world,'string'); const originalWorld = world;
  await page.goto(origin+'/other');
  const back = page.goBack({waitUntil:'commit'}).catch(error => {
    if (!/ERR_ABORTED|interrupted by another navigation/.test(error.message)) throw error;
  });
  await Promise.all([held,restoredSeen]); report.persisted_restore=true;
  const ordinary = page.evaluate(()=>({document:globalThis.__mechanics?.document,restored:globalThis.__mechanics?.restored}));
  void ordinary.catch(()=>{});
  const observed = await Promise.race([ordinary.then(()=>({kind:'returned'}),error=>({kind:'rejected',context_destroyed:/Execution context was destroyed|Cannot find context/.test(error.message)})),delay(1000).then(()=>({kind:'pending'}))]);
  report.page_evaluate = observed;
  const pending = await cdp.send('Runtime.evaluate',{uniqueContextId:originalWorld,expression:'JSON.stringify({document:globalThis.__mechanics?.document,restored:globalThis.__mechanics?.restored,queued:globalThis.__mechanics?.queued,started:globalThis.__mechanics?.started})',returnByValue:true});
  assert.equal(pending.exceptionDetails,undefined);
  const state = JSON.parse(pending.result.value);
  assert.deepEqual(state,{document:original,restored:true,queued:true,started:false});report.exact_original_world_pending=true;
  const released = await cdp.send('Runtime.evaluate',{uniqueContextId:originalWorld,expression:`globalThis.__mechanics.release(${JSON.stringify(original)})`,returnByValue:true});
  assert.equal(released.exceptionDetails,undefined);assert.equal(released.result.value,true);report.exact_original_world_release=true;
  let started;
  const startedSeen = new Promise(resolve=>{started=resolve;});
  page.on('console',message=>{if(message.text()==='MECHANICS_REAL_START')started();});
  // Also read actual state: console delivery can precede listener attachment.
  const after = await cdp.send('Runtime.evaluate',{uniqueContextId:originalWorld,expression:'globalThis.__mechanics.started',returnByValue:true});
  if (!after.result.value) await Promise.race([startedSeen,delay(1000).then(()=>{throw new Error('start not observed');})]);
  report.real_timer_started=true;releaseHeld();releaseHeld=undefined;await back;
  await ordinary.catch(()=>{});terminal=0;
} catch(error) {
  report.failure = {name:['Error','AssertionError','TimeoutError'].includes(error.name)?error.name:'unrecognized',context_destroyed:/Execution context was destroyed|Cannot find context/.test(error.message)};
} finally {
  releaseHeld?.();
  try {await browserServer?.close();report.sdk_cleanup_settled=true;} catch {terminal=1;report.sdk_cleanup_settled=false;}
  server.closeAllConnections();await new Promise(resolve=>server.close(resolve));clearTimeout(hard);
  console.log(JSON.stringify(report));process.exitCode=terminal;
}
