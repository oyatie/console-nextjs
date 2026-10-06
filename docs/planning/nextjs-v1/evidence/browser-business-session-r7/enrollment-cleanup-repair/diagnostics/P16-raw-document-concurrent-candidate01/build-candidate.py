from pathlib import Path
import difflib,hashlib,json
base=Path('/Users/jasonlee/Developer/frontend/.artifacts/acceptance/browser-business-session/r7-native-implementation/enrollment-deadline-repair/real-p16-rejection-diagnostic-candidate01/tools/browser-business-restore.mjs')
expected='63a0da2617d01fcd0fb98f6e3aeabf806e7a597a5f3924c924f9537dae8d3c40'
out=Path(__file__).parent
before=base.read_bytes()
assert hashlib.sha256(before).hexdigest()==expected,'frozen base mismatch'
s=before.decode()
root_before=Path('/Users/jasonlee/Developer/frontend/.worktrees/browser-business-session-r7/tools/browser-business-restore.mjs').read_bytes()
assert hashlib.sha256(root_before).hexdigest()=='a23c66dc14171cc07ab6d10d6835d07d4aaa105fae90e99ee9920015cf5214b6','tracked source drift'
def replace_once(a,b):
    global s
    assert s.count(a)==1,(a[:100],s.count(a))
    s=s.replace(a,b,1)
replace_once('  let originalWorldObservation;','''  let originalWorldObservation;
  let originalDocumentObjectId;
  const originalDocumentObjectGroup = `r7-original-document-${randomUUID()}`;
  let originalDocumentObjectGroupMayExist = false;
  let originalDocumentPinObservation; let originalDocumentOwnerObservation;
  let originalDocumentOwnerAtRequiredOutcome;''')
replace_once('    let cdpRejection;\n    function observe(outcome, valueKind = null) {','''    let cdpRejection;
    let timer; let documentDiagnosis;
    function observe(outcome, valueKind = null) {''')
replace_once('        originalWorldObservation = { purpose, category: outcome,','        const observation = { purpose, category: outcome,')
replace_once('currentMainWorld === originalMainWorld : null };\n      } catch { /* Missing diagnostics cannot replace the actual outcome. */ }','''currentMainWorld === originalMainWorld : null };
        if (purpose === "document-pin") originalDocumentPinObservation = observation;
        else originalWorldObservation = observation;
        if (purpose === "pending" && originalDocumentOwnerObservation) {
          originalDocumentOwnerAtRequiredOutcome = { ...originalDocumentOwnerObservation,
            ...(originalDocumentOwnerObservation.cdp_rejection
              ? { cdp_rejection: { ...originalDocumentOwnerObservation.cdp_rejection } } : {}),
            ...(originalDocumentOwnerObservation.pending
              ? { pending: { ...originalDocumentOwnerObservation.pending } } : {}) };
        }
      } catch { /* Missing diagnostics cannot replace the actual outcome. */ }''')
replace_once('      const evaluation = cacheObserver.send("Runtime.evaluate", {\n        uniqueContextId: originalMainWorld, returnByValue: true, timeout: h.phaseMs ?? 20000,','''      if (purpose === "document-pin") originalDocumentObjectGroupMayExist = true;
      const evaluation = cacheObserver.send("Runtime.evaluate", {
        uniqueContextId: originalMainWorld, returnByValue: purpose !== "document-pin", timeout: h.phaseMs ?? 20000,
        ...(purpose === "document-pin" ? { objectGroup: originalDocumentObjectGroup } : {}),''')
replace_once('''      let timer;
      const result = await Promise.race([evaluation, new Promise((_, reject) => {
        timer = setTimeout(() => {
          deadlineFired = true;
          reject(new Error("original-world observation deadline"));
        }, h.phaseMs ?? 20000);
      })]).finally(() => clearTimeout(timer));''','''      const deadline = new Promise((_, reject) => {
        timer = setTimeout(() => {
          deadlineFired = true;
          reject(new Error("original-world observation deadline"));
        }, h.phaseMs ?? 20000);
      });
      // Read-only diagnostic starts after the mandatory exact-context call.
      // Both share this existing deadline; neither can release the logout.
      if (purpose === "pending") documentDiagnosis = observeOriginalDocumentPending(
        deadline, startedAt, configuredBudgetMs, () => deadlineFired);
      const result = await Promise.race([evaluation, deadline]);''')
replace_once('''      category = "result-access";
      const value = result.result.value;''','''      category = "result-access";
      if (purpose === "document-pin") {
        assert.equal(result.result?.type, "object");
        assert.equal(result.result?.subtype, "node");
        assert.ok(typeof result.result?.objectId === "string" && result.result.objectId.length > 0);
        originalDocumentObjectId = result.result.objectId;
        observe("returned-original-document", "node");
        return true;
      }
      const value = result.result.value;''')
replace_once('''      throw error; // Diagnostics never replace the actual first failure.
    }
  }
  async function assertOriginalPrivateHidden() {''','''      throw error; // Diagnostics never replace the actual first failure.
    } finally {
      // Capture the mandatory outcome before waiting for diagnostic settlement.
      // A stuck diagnostic receives the same original deadline, never a new one.
      try { if (documentDiagnosis) await documentDiagnosis; }
      catch { originalDocumentOwnerObservation = { category: "diagnostic-internal-failure" }; }
      finally { clearTimeout(timer); }
    }
  }
  async function observeOriginalDocumentPending(deadline, startedAt, budget, deadlineHasFired) {
    let category = "document-unpinned";
    let cdpRejection; let responseReceived = false; let returnedException = null;
    let exceptionClass = null; let pending = null; let matches = null; let sendRejected = false;
    let callIssued = false;
    try {
      originalDocumentOwnerObservation = { category: "awaiting-result", raw_document_call_issued: false };
      if (typeof originalDocumentObjectId !== "string") return;
      category = "cdp-send";
      callIssued = true;
      originalDocumentOwnerObservation = { category: "awaiting-result", raw_document_call_issued: true };
      const evaluation = cacheObserver.send("Runtime.callFunctionOn", {
        objectId: originalDocumentObjectId, returnByValue: true,
        functionDeclaration: `function(original, key) {
          const document = this; const window = document.defaultView;
          const state = window?.[key];
          if (document.nodeType !== 9 || !window || window.document !== document
            || !state || state.document !== original) throw new Error('original Document unavailable');
          return { original: state.document === original, restored: state.restored,
            queued: state.logoutQueued, started: state.logoutStarted };
        }`,
        arguments: [{ value: original }, { value: PROBE }],
      });
      void evaluation.catch((error) => {
        sendRejected = true; cdpRejection = classifyCdpRejection(error, "Runtime.callFunctionOn");
      });
      const result = await Promise.race([evaluation, deadline]);
      responseReceived = true;
      returnedException = Boolean(result?.exceptionDetails);
      if (returnedException) {
        const name = result.exceptionDetails.exception?.className;
        exceptionClass = ["Error", "EvalError", "RangeError", "ReferenceError", "SyntaxError", "TypeError", "URIError"]
          .includes(name) ? name : "other";
        category = "returned-exception";
        return;
      }
      const value = result?.result?.value;
      if (!value || typeof value !== "object" || Array.isArray(value)
        || Object.keys(value).sort().join(",") !== "original,queued,restored,started"
        || Object.values(value).some((item) => typeof item !== "boolean")) {
        category = "invalid-snapshot"; return;
      }
      pending = { original: value.original, restored: value.restored, queued: value.queued, started: value.started };
      matches = value.original && value.restored && value.queued && !value.started;
      category = "returned-pending-snapshot";
    } catch {
      category = deadlineHasFired() ? "shared-local-deadline" : sendRejected ? "cdp-rejected" : category;
    } finally {
      // Only enums, booleans and bounded relative clocks leave this diagnostic.
      // Document handles, tokens, bodies and exception messages remain transient.
      try {
        const elapsed = Math.max(0, Math.floor(performance.now() - startedAt));
        originalDocumentOwnerObservation = { category,
          configured_budget_ms: Number.isSafeInteger(budget) && budget > 0 && budget <= 600000 ? budget : null,
          observed_elapsed_ms: Math.min(elapsed, 600000), elapsed_capped: elapsed > 600000,
          local_deadline_fired: deadlineHasFired(), raw_document_call_issued: callIssued,
          cdp_response_received: responseReceived, cdp_rejection: cdpRejection ?? null,
          exception_returned: returnedException, exception_class: exceptionClass,
          pending, pending_matches_expected: matches };
      } catch { /* Missing sidecar metadata cannot change the required result. */ }
    }
  }
  async function assertOriginalPrivateHidden() {''')
replace_once('''      assert.equal(await evaluateOriginal("pin", "state.document"), original);
    }''','''      assert.equal(await evaluateOriginal("pin", "state.document"), original);
      // Capture a raw native Document object while its exact world is active.
      // Pin failure is diagnostic only; the required pending call still executes.
      try { await evaluateOriginal("document-pin", "document"); }
      catch { /* Its bounded pin observation preserves this diagnostic failure. */ }
    }''')
replace_once('''      original_world_observation: originalWorldObservation ?? null,
      // Copy at the actual first failure; late callbacks/cleanup cannot rewrite it.''','''      original_world_observation: originalWorldObservation ?? null,
      original_document_pin_observation: originalDocumentPinObservation ?? null,
      original_document_owner_at_required_outcome: originalDocumentOwnerAtRequiredOutcome ?? null,
      original_document_owner_observation: originalDocumentOwnerObservation ?? null,
      // Copy at failure reporting; late callbacks/cleanup cannot rewrite it.''')
replace_once('''    evidence.original_document = original;
    if (queued) queuedCheckpoint = "independent-actor";''','''    evidence.original_document = original;
    if (queued) {
      evidence.original_document_pin_observation = originalDocumentPinObservation ?? null;
      evidence.original_document_owner_at_required_outcome = originalDocumentOwnerAtRequiredOutcome ?? null;
      evidence.original_document_owner_observation = originalDocumentOwnerObservation ?? null;
      queuedCheckpoint = "independent-actor";
    }''')
replace_once('''    if (routed) await cleanup("routes-remove", () => page.unrouteAll({ behavior: "wait" }));
    await cleanup("probe-listeners", () => {''','''    if (routed) await cleanup("routes-remove", () => page.unrouteAll({ behavior: "wait" }));
    if (originalDocumentObjectGroupMayExist) await cleanup("original-document-object-group", async () => {
      await cacheObserver.send("Runtime.releaseObjectGroup", { objectGroup: originalDocumentObjectGroup });
      originalDocumentObjectId = undefined; originalDocumentObjectGroupMayExist = false;
    });
    await cleanup("probe-listeners", () => {''')
root=out/'tools'; root.mkdir(exist_ok=True)
after=s.encode(); (root/'browser-business-restore.mjs').write_bytes(after)
(out/'classifier-timeline-before.mjs').write_bytes(before)
(out/'root-before.mjs').write_bytes(root_before)
for name,left in [('sidecar.diff',before),('combined.diff',root_before)]:
    patch=''.join(difflib.unified_diff(left.decode().splitlines(True),s.splitlines(True),fromfile='a/tools/browser-business-restore.mjs',tofile='b/tools/browser-business-restore.mjs'))
    (out/name).write_text(patch)
manifest={'schema':'P16-raw-document-concurrent-candidate/1','status':'UNEXECUTED_SOURCE_CANDIDATE','frozen_classifier_timeline_path':str(base),'frozen_classifier_timeline_sha256':expected,'tracked_before_sha256':hashlib.sha256(root_before).hexdigest(),'after_sha256':hashlib.sha256(after).hexdigest(),'after_bytes':len(after),'sidecar_diff_sha256':hashlib.sha256((out/'sidecar.diff').read_bytes()).hexdigest(),'combined_diff_sha256':hashlib.sha256((out/'combined.diff').read_bytes()).hexdigest(),'tests_executed':0,'tracked_source_writes':0}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps(manifest))
