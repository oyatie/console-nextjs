#!/usr/bin/env node
// Evaluate a workflow script's MODULE SCOPE with the runtime's injected globals stubbed.
//
// Why this exists: a workflow script is not a Node module. The runtime evaluates the body in a
// bare sandbox where `process` does not exist (`Date.now`/`Math.random` are likewise absent — they
// would break resume). A single `process.env.HOME` therefore does not degrade, it throws
// ReferenceError before agent 1 spawns, and the whole run dies in ~13ms. That cost two launches.
//
// It survived review because it was written as a FALLBACK — `A.repo || process.env.X || '/lit'` —
// and `||` short-circuits whenever `A.repo` is passed. The broken branch was never once evaluated.
// A default that cannot execute is not a default; it is a latent crash keyed to an unset arg.
//
// Usage:  node tools/lanes/wfcheck.mjs scripts/console/workflows/slice.js '{"task":"t","lane":"1"}'
// Exit 0 = module scope evaluated clean. Exit 1 = it threw (message printed).
//
// NOTE ON THE PROBE ITSELF: the first version wrapped the body in `(async()=>{})()` and try/caught
// the CALL. A throw inside an async function is a rejected promise, not a synchronous throw, so the
// catch never fired and this script printed OK for a file containing `process.env.HOME`. It is now
// verified the only way a probe may be trusted — proven RED on a known-bad input before its GREEN
// is believed. `--self-test` re-proves that on demand; run it if you ever touch this file.
import { readFileSync, writeFileSync, mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import vm from 'node:vm'

const PENDING = Symbol('pending')

/** A value that tolerates any property access, indexing, call, await or interpolation. */
function stub() {
  const target = function () { return stub() }
  return new Proxy(target, {
    get(_t, prop) {
      // `then: undefined` keeps `await` from treating it as a thenable and hanging.
      if (prop === 'then') return undefined
      // A workflow script interpolates agent results into prompt template literals, which coerces
      // to a primitive. Without these the probe throws "Cannot convert object to primitive value"
      // — its own defect, reported as if it were the script's.
      if (prop === Symbol.toPrimitive) return () => '<stub>'
      if (prop === 'toString' || prop === Symbol.toStringTag) return () => '<stub>'
      if (prop === Symbol.iterator) return function* () {}
      return stub()
    },
    apply: () => stub(),
  })
}

async function check(file, argsJson) {
  const src = readFileSync(file, 'utf8')
    .replace(/^export const meta/m, 'const meta') // `export` is illegal outside a module
    .replace(/^return .*/ms, '') // the trailing return is the runtime's form, not JS's
  const sandbox = {
    args: argsJson ? JSON.parse(argsJson) : {},
    // agent()/parallel()/pipeline() never resolve: reaching a pending await means module scope
    // completed, which is exactly what is under test. Nothing is spawned and nothing is billed.
    // These RESOLVE rather than hang. An earlier version returned never-settling promises, which
    // meant evaluation stopped at the script's first `await` and everything after it — every later
    // phase's template literals, where an undefined identifier hides just as well — went unchecked.
    // That is how a `${BASE_REF}` with no binding passed this probe: it lives after the first
    // await, so the crash would have fired only once the implementer had already finished.
    // Returning a permissive stub lets the whole script run; property access on it yields more
    // stubs rather than throwing, so only genuine ReferenceErrors surface.
    agent: async () => stub(),
    parallel: async (thunks) => (Array.isArray(thunks) ? thunks.map(() => stub()) : []),
    pipeline: async (items) => (Array.isArray(items) ? items.map(() => stub()) : []),
    phase: () => {},
    log: () => {},
    budget: { total: null, spent: () => 0, remaining: () => Infinity },
    console, JSON, Math, Array, Object, String, Number, Boolean, Error, Promise, setTimeout,
  }
  let run
  try {
    run = new vm.Script(`(async () => {\n${src}\n})()`).runInNewContext(sandbox)
  } catch (e) {
    return e // a genuine parse error surfaces synchronously
  }
  const settled = await Promise.race([
    run.then(() => null, (e) => e),
    new Promise((r) => setTimeout(() => r(PENDING), 2000)),
  ])
  return settled === PENDING ? null : settled
}

async function selfTest() {
  // Prove RED on a file whose ONLY defect is the banned global, then GREEN once it is removed.
  const dir = mkdtempSync(join(tmpdir(), 'wfcheck-'))
  const body = (home) => `export const meta = { name: 'p', description: 'probe' }\n` +
    `const HOME = ${home}\nlog(HOME)\nawait agent('x')\n`
  const bad = join(dir, 'bad.js'); writeFileSync(bad, body('process.env.HOME'))
  const good = join(dir, 'good.js'); writeFileSync(good, body("'/Users/jasonlee'"))
  const onBad = await check(bad, '{}')
  const onGood = await check(good, '{}')
  if (!onBad) { console.log('SELF-TEST FAILED: probe passed a file containing process.env.HOME'); return 1 }
  if (onGood) { console.log(`SELF-TEST FAILED: probe rejected a clean file — ${onGood.message}`); return 1 }
  console.log(`self-test OK — RED on known-bad (${onBad.message}), GREEN on clean`)
  return 0
}

const [file, argsJson] = process.argv.slice(2)
if (file === '--self-test') process.exit(await selfTest())
if (!file) { console.log('usage: wfcheck.mjs <workflow.js> [argsJson] | --self-test'); process.exit(2) }
const err = await check(file, argsJson)
if (err) { console.log(`RED ${err.constructor.name}: ${err.message}`); process.exit(1) }
console.log('OK  module scope evaluated with no ReferenceError')
