from pathlib import Path
import hashlib,difflib,json
root=Path('/Users/jasonlee/Developer/frontend/.worktrees/browser-business-session-r7')
out=Path(__file__).parent
before=(root/'tools/browser-business-restore.mjs').read_bytes()
assert hashlib.sha256(before).hexdigest()=='33bbfcc9e60abdc5f6eb6e9d8c800667bac46ced34cbbde6154744ed7da61a7b'
s=before.decode()
def replace_once(a,b):
    global s
    assert s.count(a)==1,(a[:100],s.count(a))
    s=s.replace(a,b,1)
replace_once('import { types } from "node:util";\n','')
a=s.index('// Compare complete fixed SDK/protocol literals');b=s.index('class Unreached',a);s=s[:a]+s[b:]
replace_once('''  const errors = [];
  const listener = (message) => {''','''  const errors = [];
  const inputObservations = [];
  const bindingName = `__r7NativeRestore${randomUUID().replaceAll("-", "")}`;
  const inputNonce = randomUUID();
  let inputWaiter; let inputAttempts = 0; let completedInputs = 0; let closed = false;
  if (queueLogout) await page.exposeBinding(bindingName, (source, value) => {
    if (closed) return;
    try {
      assert.ok(inputWaiter, "one original input reply must be pending");
      assert.equal(source.page, page); assert.equal(source.frame, page.mainFrame());
      assert.ok(value && typeof value === "object" && !Array.isArray(value));
      const extra = inputWaiter.kind === "snapshot" ? ["pending", "snapshot"] : ["commits"];
      assert.deepEqual(Object.keys(value).sort(), ["nonce", "sequence", "kind", "document", "trusted", "persisted", ...extra].sort());
      assert.equal(value.nonce, inputNonce); assert.equal(value.document, inputWaiter.original);
      assert.equal(value.sequence, inputWaiter.sequence); assert.equal(value.kind, inputWaiter.kind);
      assert.equal(value.trusted, true); assert.equal(value.persisted, true);
      if (value.kind === "snapshot") {
        assert.deepEqual(Object.keys(value.pending).sort(), ["original", "restored", "queued", "started"].sort());
        assert.ok(Object.values(value.pending).every((item) => typeof item === "boolean"));
        assert.deepEqual(Object.keys(value.snapshot).sort(), ["text", "html", "visibleTables"].sort());
        assert.equal(typeof value.snapshot.text, "string"); assert.equal(typeof value.snapshot.html, "string");
        assert.ok(value.snapshot.text.length + value.snapshot.html.length <= 2 * 1024 * 1024);
        assert.ok(Array.isArray(value.snapshot.visibleTables) && value.snapshot.visibleTables.length <= 1024
          && value.snapshot.visibleTables.every((item) => typeof item === "boolean"));
      } else assert.ok(Number.isSafeInteger(value.commits) && value.commits >= 0);
      const waiter = inputWaiter; inputWaiter = undefined;
      waiter.resolve(value); // Transient snapshot only; never appended to logs/evidence/events.
    } catch {
      if (errors.length < 32) errors.push("invalid original native input reply");
      const waiter = inputWaiter; inputWaiter = undefined;
      waiter?.reject(new Error("invalid original native input reply"));
    }
  });
  async function originalInput(kind, original, input, budget = 20000) {
    assert.ok(queueLogout && !closed && !inputWaiter);
    assert.equal(kind, inputAttempts === 0 ? "snapshot" : "release");
    assert.ok(inputAttempts < 2);
    if (kind === "release") assert.equal(completedInputs, 1, "snapshot must complete before the sole release input");
    const sequence = ++inputAttempts;
    const startedAt = performance.now();
    let timer; let deadlineFired = false; let stopped = false;
    let sent = false; let received = false; let category = "native-input-failure";
    const reply = new Promise((resolve, reject) => { inputWaiter = { kind, original, sequence, resolve, reject }; });
    const key = kind === "snapshot" ? "F13" : "F14";
    const windowsVirtualKeyCode = kind === "snapshot" ? 124 : 125;
    const sending = (async () => {
      await input.send("Input.dispatchKeyEvent", { type: "rawKeyDown", key, code: key, windowsVirtualKeyCode, autoRepeat: false });
      if (stopped) return;
      await input.send("Input.dispatchKeyEvent", { type: "keyUp", key, code: key, windowsVirtualKeyCode });
      sent = true;
    })();
    const operation = Promise.all([sending, reply.then((value) => { received = true; return value; })])
      .then(([, value]) => value);
    void operation.catch(() => {}); // Own any late send/reply rejection after this one bounded attempt.
    try {
      const value = await Promise.race([operation, new Promise((_, reject) => {
        timer = setTimeout(() => { deadlineFired = true; reject(new Error("original native input deadline")); }, budget);
      })]);
      category = "reply-received"; completedInputs += 1;
      return value;
    } finally {
      stopped = true; clearTimeout(timer); inputWaiter = undefined;
      const elapsed = Math.max(0, Math.floor(performance.now() - startedAt));
      inputObservations.push({ kind, sequence, category: deadlineFired ? "local-deadline" : category,
        configured_budget_ms: Number.isSafeInteger(budget) && budget > 0 && budget <= 600000 ? budget : null,
        observed_elapsed_ms: Math.min(elapsed, 600000), elapsed_capped: elapsed > 600000,
        native_input_completed: sent, valid_original_reply_received: received, local_deadline_fired: deadlineFired });
    }
  }
  const listener = (message) => {''')
replace_once('''  await page.addInitScript(({ selector, names, queueLogout, observeReact, key, prefix }) => {''','''  await page.addInitScript(({ selector, names, queueLogout, observeReact, key, prefix, bindingName, inputNonce }) => {''')
replace_once('''    Object.defineProperty(window, key, { value: state });
    // Only a real persisted''','''    Object.defineProperty(window, key, { value: state });
    const originalDocument = document; const originalToken = state.document;
    const reply = queueLogout ? window[bindingName] : null;
    if (queueLogout && typeof reply !== "function") throw new Error("native input reply binding absent");
    let trustedRestore = false; let inputSequence = 0;
    // Only a real persisted''')
replace_once('''    state.releaseLogout = (original) => {
      if (!queueLogout || state.document !== original || !state.restored''','''    const releaseQueuedLogout = () => {
      if (!queueLogout || state.document !== originalToken || !state.restored''')
replace_once('''      return commits;
    };
    function report(event, source) {''','''      return commits;
    };
    if (queueLogout) addEventListener("keydown", (event) => {
      if (!event.isTrusted || event.repeat || !["F13", "F14"].includes(event.code)
        || !trustedRestore || !state.restored || !state.logoutQueued || state.logoutStarted
        || state.document !== originalToken || document !== originalDocument
        || originalDocument.defaultView?.document !== originalDocument) return;
      const response = (kind, sequence, details) => {
        // SDK binding transport receives data directly; it never logs this snapshot.
        // Retiring-context delivery of the callback result is not awaited.
        void reply({ nonce: inputNonce, sequence, kind, document: originalToken,
          trusted: event.isTrusted, persisted: trustedRestore, ...details }).catch(() => {});
      };
      if (event.code === "F13" && inputSequence === 0) {
        inputSequence = 1;
        response("snapshot", 1, {
          pending: { original: state.document === originalToken, restored: state.restored,
            queued: state.logoutQueued, started: state.logoutStarted },
          snapshot: { text: originalDocument.body.innerText, html: originalDocument.documentElement.outerHTML,
            visibleTables: [...originalDocument.querySelectorAll('table,[role="table"]')].map((table) => {
              const style = getComputedStyle(table);
              return Boolean(table.getClientRects().length) && style.display !== "none"
                && style.visibility !== "hidden" && style.visibility !== "collapse";
            }) },
        });
      } else if (event.code === "F14" && inputSequence === 1 && !state.logoutReleased) {
        const commits = releaseQueuedLogout();
        if (commits === null) return;
        inputSequence = 2; response("release", 2, { commits });
      }
    });
    function report(event, source) {''')
replace_once('''      if (event.isTrusted && event.persisted) {
        state.restored = true;''','''      if (event.isTrusted && event.persisted) {
        trustedRestore = true; state.restored = true;''')
replace_once('''    ...facts.first_page.items.map((item) => item.note)], queueLogout, observeReact, key: PROBE, prefix: PREFIX });
  return { events, errors,''','''    ...facts.first_page.items.map((item) => item.note)], queueLogout, observeReact, key: PROBE, prefix: PREFIX,
    bindingName, inputNonce });
  return { events, errors, inputObservations, originalInput,''')
replace_once('''    remove() { page.off("console", listener); } };''','''    remove() {
      closed = true; inputWaiter?.reject(new Error("original native input receiver closed")); inputWaiter = undefined;
      page.off("console", listener);
    } };
''')
a=s.index('  let mainFrameId; let currentMainWorld;');b=s.index('  const queuedPhaseTimeline',a);s=s[:a]+'  let mainFrameId;\n'+s[b:]
a=s.index('  const originalWorldLifecycle');b=s.index('  // This diagnostic retains fixed browser enums only',a)
s=s[:a]+'''  function assertOriginalPrivateHidden(snapshot) {
    assertPrivateTextHidden(snapshot.text, actor);
    for (const visible of snapshot.visibleTables) assert.equal(visible, false);
    h.assertNoProof(snapshot.html); // Transient bytes only, never diagnostic output.
  }
'''+s[b:]
for line in [
'      cacheObserver.on("Runtime.executionContextCreated", contextListener);\n',
'      cacheObserver.on("Runtime.executionContextDestroyed", contextDestroyedListener);\n',
'      cacheObserver.on("Runtime.executionContextsCleared", contextsClearedListener);\n',
'      cacheObserver.on("close", observerClosedListener);\n',
'      await cacheObserver.send("Runtime.enable");\n',
'      cacheObserver?.off("Runtime.executionContextCreated", contextListener);\n',
'      cacheObserver?.off("Runtime.executionContextDestroyed", contextDestroyedListener);\n',
'      cacheObserver?.off("Runtime.executionContextsCleared", contextsClearedListener);\n',
'      cacheObserver?.off("close", observerClosedListener);\n']:
    replace_once(line,'')
a=s.index('    if (queued) {\n      queuedCheckpoint = "original-world-pin";');b=s.index('    if (queued) queuedCheckpoint = "initial-hydration";',a);s=s[:a]+s[b:]
replace_once('''        const pending = await evaluateOriginal("pending", `({ original: state.document === original,
          restored: state.restored, queued: state.logoutQueued, started: state.logoutStarted })`);''','''        const observed = await probe.originalInput("snapshot", original, cacheObserver, h.phaseMs ?? 20000);
        const pending = observed.pending;''')
replace_once('''        await assertOriginalPrivateHidden();''','''        assertOriginalPrivateHidden(observed.snapshot);
        observed.snapshot = undefined; // Release the transient private source bytes before the action.''')
replace_once('''        const commitsBeforeRelease = await evaluateOriginal("release", "state.releaseLogout(original)");''','''        const released = await probe.originalInput("release", original, cacheObserver, h.phaseMs ?? 20000);
        const commitsBeforeRelease = released.commits;''')
replace_once('''      evidence.original_document_pin_observation = originalDocumentPinObservation ?? null;
      evidence.original_document_owner_at_required_outcome = originalDocumentOwnerAtRequiredOutcome ?? null;
      evidence.original_document_owner_observation = originalDocumentOwnerObservation ?? null;''','''      evidence.original_input_observations = probe.inputObservations.map((item) => ({ ...item }));''')
replace_once('''      original_world_observation: originalWorldObservation ?? null,
      original_document_pin_observation: originalDocumentPinObservation ?? null,
      original_document_owner_at_required_outcome: originalDocumentOwnerAtRequiredOutcome ?? null,
      original_document_owner_observation: originalDocumentOwnerObservation ?? null,''','''      original_input_observations: probe.inputObservations.map((item) => ({ ...item })),''')
replace_once('''    // This exclusive page/CDP session owns the raw Document handle and any
    // outstanding read-only sidecar; confirmed closure retires both.''','''    // This exclusive page/CDP session owns the binding and any outstanding
    // native input/result delivery; confirmed closure retires them.''')
assert 'evaluateOriginal' not in s and 'originalDocumentObjectId' not in s and 'state.releaseLogout' not in s
(out/'tools').mkdir();after=s.encode();(out/'tools/browser-business-restore.mjs').write_bytes(after)
(out/'before.mjs').write_bytes(before)
patch=''.join(difflib.unified_diff(before.decode().splitlines(True),s.splitlines(True),fromfile='a/tools/browser-business-restore.mjs',tofile='b/tools/browser-business-restore.mjs'));(out/'patch.diff').write_text(patch)
manifest={'schema':'P16-native-input-owner-candidate/1','status':'UNEXECUTED_SOURCE_CANDIDATE','before_sha256':hashlib.sha256(before).hexdigest(),'after_sha256':hashlib.sha256(after).hexdigest(),'after_bytes':len(after),'patch_sha256':hashlib.sha256(patch.encode()).hexdigest(),'sdk_core_bundle_sha256':hashlib.sha256((root/'node_modules/playwright-core/lib/coreBundle.js').read_bytes()).hexdigest(),'sdk_protocol_sha256':hashlib.sha256((root/'node_modules/playwright-core/types/protocol.d.ts').read_bytes()).hexdigest(),'tests_executed':0,'tracked_source_writes':0,'workflow':'Existing native401 exactgate/context witness -> F13 nativekeydown/up -> exactpage/mainframe/nonce/seq/originaltoken/trustedpersisted binding snapshot -> existingpending/private assertions -> sole F14 nativekeydown/up -> originalclosure release -> existingactualReactcommit/logout204/cookie/privacy/nativegate/singlehardreload/independentB proofs. No postrestore Runtime.evaluate/callFunctionOn or rawsnapshotlogs; no timeoutwidening/fallback/retry.'}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');print(json.dumps(manifest))
