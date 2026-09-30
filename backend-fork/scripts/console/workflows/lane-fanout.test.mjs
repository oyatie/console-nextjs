// Offline preflight for lane-fanout.js. Run it BEFORE every dispatch:
//
//     node scripts/console/workflows/lane-fanout.test.mjs
//
// It exists because two classes of defect are invisible to reading, and both shipped here:
//
//   1. SYNTAX. An unescaped backtick inside a prompt template literal ends the literal. It
//      silently truncated BASE_LOCK once; the run looked normal and the lock was half gone.
//      `node --check` cannot see this — it rejects the top-level await and tells you nothing.
//   2. WIRING. A rule can be written, documented, believed, and reachable by nothing:
//        - `LENSES = ARGS.lenses || [...]` meant the standing lenses NEVER ran, because every
//          invocation passed `lenses`. Oracle integrity, the most common rejection cause in this
//          program, had never once been reviewed for.
//        - `verifierOk` matched the literal string "none" in the verifier's prose, so a verifier
//          writing "four, all minor; none contradicts the verdict" failed the check. Convergence
//          was unreachable whenever the verifier ran, and the harness manufactured rebuild rounds.
//        - convergence keyed on `severity === 'blocker'`, so a run reported CONVERGED while both
//          reviewers held six separately PROVEN fail-opens, every one filed "major".
//
// The harness is driven with STUB agents, so this proves the ENFORCEMENT LOGIC, not any agent's
// judgement. It is offline, free, and takes under a second.

import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
// The RED baseline for any change to these dispatchers is THIS file run against the PRE-FIX
// sources (`git show <tip>:scripts/console/workflows/<f>.js` into a scratch dir, then point this at it).
// Without the override the preflight can only ever measure the tree it ships with, so "it would
// have gone red on the old code" is a claim nobody can re-run. CI passes nothing and gets HERE.
const SRCDIR = process.env.LANE_FANOUT_SRCDIR || HERE
const SRC = fs.readFileSync(path.join(SRCDIR, 'lane-fanout.js'), 'utf8')

// Compile exactly as the harness evaluates it: an async function body with these globals, so
// top-level await and return are legal. This is the only honest syntax check.
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor
let run
try {
  run = new AsyncFunction(
    'args', 'agent', 'parallel', 'pipeline', 'log', 'phase', 'budget', 'workflow',
    SRC.replace(/^export const meta = /m, 'const meta = '),
  )
} catch (e) {
  console.error('FAIL compile —', e.message)
  console.error('  An unescaped ` inside a prompt template literal is the usual cause.')
  process.exit(1)
}
console.log('PASS compile — body parses as the harness evaluates it')

let failures = 0 // eslint-disable-line prefer-const
const check = (name, ok, detail) => {
  // JSON.stringify(undefined) is undefined, not "undefined", so a FAILING assertion called without a
  // detail threw TypeError here and killed the whole preflight mid-run — losing every assertion after
  // it AND the failure count, so the harness reported nothing rather than a red. The reporter's own
  // failure path was the one path no assertion exercised. It is exercised at the bottom of this file.
  const shown = detail === undefined ? '(no detail)' : String(JSON.stringify(detail)).slice(0, 500)
  console.log(`${ok ? 'PASS' : 'FAIL'} ${name}${ok ? '' : ' :: ' + shown}`)
  if (!ok) failures++
}

// Sequential is fine for a logic test; parallel() only needs to run every thunk and collect.
const parallel = async (thunks) => {
  const out = []
  for (const t of thunks) { try { out.push(await t()) } catch { out.push(null) } }
  return out
}
const pipeline = async (items, ...stages) => {
  const out = []
  for (const [i, it] of items.entries()) {
    let v = it
    for (const s of stages) v = await s(v, it, i)
    out.push(v)
  }
  return out
}

const BUILD = (over = {}) => ({
  status: 'done', summary: 's', filesChanged: ['x.rs'], redBaseline: 'RED', verification: 'ok',
  contractBreaches: 'none', enforcementPlacement: 'n/a - adds no enforcement',
  // Default must name a real command that VERIFY re-runs. `commands: []` used to converge
  // vacuously against any verifier command — that is the fail-open under test below.
  peripheralsUpdated: 'n/a - nothing described this behaviour', followUps: '',
  commands: ['cargo test -p x'], ...over,
})
const FINDING = (over = {}) => ({
  severity: 'major', claim: 'c', failureScenario: 'f', location: 'l',
  provenByExecution: false, ownerLease: false, ...over,
})
const REVIEW = (over = {}) => ({ verdict: 'accept', findings: [], oracleWeakened: false, scopeCreep: false, ...over })
const VERIFY = (over = {}) => ({
  reproduced: true, actualResults: 'a', discrepancies: 'none', contradictsClaim: false,
  falseGreenRisk: 'none', commandsRun: ['cargo test -p x'], oracleIntact: true, headSha: 'cafe1234', ...over,
})

// Records every label the harness actually dispatched, which is how we prove a rule is REACHABLE
// rather than merely written.
const mkAgent = (opts = {}) => {
  const seen = []
  const fn = async (prompt, o = {}) => {
    const label = o.label || ''
    seen.push({ label, prompt })
    if (label.startsWith('build:')) return opts.build ? opts.build(seen) : BUILD()
    if (label.startsWith('review:')) return opts.review ? opts.review(prompt, seen) : REVIEW()
    if (label.startsWith('verify:')) return opts.verify ? opts.verify() : VERIFY()
    if (label === 'land') return opts.land ? opts.land() : { branch: 'integration/x', headSha: 'deadbee', landedLanes: ['a'], skippedLanes: [], verification: 'v', outstandingLeasedEdits: 'none', prCommand: 'gh pr create ...' }
    return 'REPORT'
  }
  fn.seen = seen
  return fn
}

const LANE = (over = {}) => ({ key: 'a', bead: 'b', wt: '/w', owned: 'x/**', brief: 't', accept: 'a', ...over })
const ARGS = (over = {}) => ({ tip: 'abc1234', lanes: [LANE()], maxRounds: 1, land: false, ...over })

const go = (args, agent = mkAgent(), logs = []) =>
  run(args, agent, parallel, pipeline, (m) => logs.push(m), () => {}, { total: null, spent: () => 0, remaining: () => Infinity }, async () => {})

const threw = async (args) => {
  try { await go(args); return null } catch (e) { return e.message }
}

// --- 1. An option the harness does not read must ABORT, never be silently dropped. ----------
{
  const m = await threw(ARGS({ prose_hardening: false }))
  check('unknown top-level arg aborts', !!m && /prose_hardening/.test(m), m)

  const m2 = await threw(ARGS({ lanes: [LANE({ scopes: ['x/'] })] }))
  check('unknown per-lane key aborts', !!m2 && /scopes/.test(m2), m2)

  const m3 = await threw(ARGS({ lens: ['typo'] }))
  check('a typo in `lenses` aborts rather than silently using defaults', !!m3 && /lens\b/.test(m3), m3)

  const ok = await go(ARGS())
  check('every documented arg is accepted', !!ok && Array.isArray(ok.headline), ok && Object.keys(ok))
}

// --- 2. Standing lenses must survive custom lenses (the dead-default defect). ---------------
{
  const agent = mkAgent()
  await go(ARGS({ lenses: ['CUSTOM ONE', 'CUSTOM TWO'] }), agent)
  const reviews = agent.seen.filter((s) => s.label.startsWith('review:'))
  const hasOracle = reviews.some((r) => /ORACLE INTEGRITY/.test(r.prompt))
  const hasPlacement = reviews.some((r) => /ENFORCEMENT PLACEMENT/.test(r.prompt))
  const hasDrift = reviews.some((r) => /PERIPHERAL DRIFT/.test(r.prompt))
  const hasCustom = reviews.some((r) => /CUSTOM ONE/.test(r.prompt))
  const hasMaintainability = reviews.some((r) => /COST OF CARRY/.test(r.prompt))
  // Sized against the standing set rather than pinned to a literal. The count was hard-coded to 5,
  // so ADDING a standing lens -- the intended way to strengthen review -- read as a regression and
  // blocked dispatch. An assertion that a legitimate improvement breaks is an assertion that will
  // eventually be "fixed" by deleting the improvement. Two lanes have already learned this shape:
  // a rule stated as a list of members instead of a relationship over the set.
  const standing = SRC.match(/const STANDING_LENSES = \[([\s\S]*?)\n\]/)
  const standingCount = standing ? (standing[1].match(/^\s{2}'/gm) || []).length : -1
  check('custom lenses ADD to standing lenses, never replace them',
    standingCount > 0 && reviews.length === standingCount + 2 &&
      hasOracle && hasPlacement && hasDrift && hasMaintainability && hasCustom,
    { count: reviews.length, standingCount, hasOracle, hasPlacement, hasDrift, hasMaintainability, hasCustom })
}

// --- 3. Convergence keys on PROOF, not on the severity label. -------------------------------
{
  const provenMajor = mkAgent({ review: () => REVIEW({ verdict: 'accept_with_findings', findings: [FINDING({ severity: 'major', provenByExecution: true })] }) })
  const r1 = await go(ARGS(), provenMajor)
  check('a PROVEN major blocks convergence', r1.lanes[0].converged === false, r1.headline)

  const arguedMajor = mkAgent({ review: () => REVIEW({ verdict: 'accept_with_findings', findings: [FINDING({ severity: 'major', provenByExecution: false })] }) })
  const r2 = await go(ARGS(), arguedMajor)
  check('an ARGUED major does not block convergence', r2.lanes[0].converged === true, r2.headline)

  const leasedBlocker = mkAgent({ review: () => REVIEW({ verdict: 'reject', findings: [FINDING({ severity: 'blocker', provenByExecution: true, ownerLease: true })] }) })
  const r3 = await go(ARGS(), leasedBlocker)
  check('an owner lease releases at ANY severity', r3.lanes[0].converged === true, r3.headline)

  const realBlocker = mkAgent({ review: () => REVIEW({ verdict: 'reject', findings: [FINDING({ severity: 'blocker' })] }) })
  const r4 = await go(ARGS(), realBlocker)
  check('a real blocker blocks', r4.lanes[0].converged === false, r4.headline)
}

// --- 4. The oracle may never be weakened, whatever the findings say. ------------------------
{
  const weakened = mkAgent({ review: () => REVIEW({ oracleWeakened: true }) })
  const r = await go(ARGS(), weakened)
  check('oracleWeakened blocks convergence with zero findings', r.lanes[0].converged === false, r.headline)
}

// --- 5. The verifier decides via a BOOLEAN, not a regex over its prose. ---------------------
{
  const conscientious = mkAgent({ verify: () => VERIFY({ discrepancies: 'four, all minor; none contradicts the verdict', contradictsClaim: false }) })
  const r = await go(ARGS(), conscientious)
  check('a wordy but non-contradicting verifier still converges', r.lanes[0].converged === true, r.headline)

  const contradicts = mkAgent({ verify: () => VERIFY({ contradictsClaim: true }) })
  const r2 = await go(ARGS(), contradicts)
  check('contradictsClaim=true blocks convergence', r2.lanes[0].converged === false, r2.headline)

  const notReproduced = mkAgent({ verify: () => VERIFY({ reproduced: false }) })
  const r3 = await go(ARGS(), notReproduced)
  check('reproduced=false blocks convergence', r3.lanes[0].converged === false, r3.headline)
}

// --- 6. Nothing green to falsify => no verifier. Rounds are the scarce resource. ------------
{
  const partial = mkAgent({ build: () => BUILD({ status: 'partial' }) })
  await go(ARGS(), partial)
  check('a non-done build skips the verifier', partial.seen.filter((s) => s.label.startsWith('verify:')).length === 0,
    partial.seen.map((s) => s.label))

  const done = mkAgent()
  await go(ARGS(), done)
  check('a done build runs the verifier', done.seen.filter((s) => s.label.startsWith('verify:')).length === 1,
    done.seen.map((s) => s.label))
}

// --- 7. Agent death is routine at this scale; retry once, then abandon. ---------------------
{
  let n = 0
  const flaky = mkAgent({ build: () => { n++; return n === 1 ? null : BUILD() } })
  const r = await go(ARGS(), flaky)
  check('a dead implementer is retried once and the round proceeds', n === 2 && r.lanes[0].status === 'done', { n, r: r.headline })

  const dead = mkAgent({ build: () => null })
  const r2 = await go(ARGS(), dead)
  check('two deaths abandon the lane without throwing', r2.lanes[0].converged === false, r2.headline)
}

// --- 8. Rejected rounds must feed the next build, or findings are collected and discarded. --
{
  const seenPrompts = []
  const agent = mkAgent({
    build: (seen) => { seenPrompts.push(seen[seen.length - 1].prompt); return BUILD() },
    review: () => REVIEW({ verdict: 'reject', findings: [FINDING({ severity: 'blocker', claim: 'THE-DISTINCTIVE-BLOCKER' })] }),
  })
  await go(ARGS({ maxRounds: 2 }), agent)
  check('round 2 receives round 1 blockers as feedback',
    seenPrompts.length === 2 && /THE-DISTINCTIVE-BLOCKER/.test(seenPrompts[1]), seenPrompts.length)
}

// --- 9. Reviewers diff the TIP, never HEAD~1 (which once diffed the base commit). -----------
{
  const agent = mkAgent()
  await go(ARGS(), agent)
  const reviews = agent.seen.filter((s) => s.label.startsWith('review:'))
  check('reviewers are told to diff the tip, and HEAD~1 appears only as a warning',
    reviews.every((r) => r.prompt.includes('abc1234')), reviews.length)
}

// --- 10. Landing is part of the pipeline. Its absence is what produced a +99 backlog. -------
{
  const agent = mkAgent()
  // Distinctive key and worktree: 'a' and '/w' occur in English prose, so they cannot tell a
  // populated prompt from a broken one.
  const r = await go(ARGS({ land: true, lanes: [LANE({ key: 'LANE-KEY-Z', wt: '/wt/lane-z' })] }), agent)
  check('a converged lane LANDS by default', agent.seen.some((s) => s.label === 'land') && !!r.landed, r.headline)

  // THE PROMPT MUST CONTAIN THE LANE, NOT undefined. Asserting only that landing was INVOKED is
  // what let this ship: the prompt read o.lane.key / o.lane.wt / o.fix off the flattened summary,
  // whose `lane` is a STRING, so the integration owner was told "undefined — worktree undefined"
  // with no files and no follow-ups. A phase that runs on garbage is not a phase that runs.
  const landPrompt = (agent.seen.find((s) => s.label === 'land') || {}).prompt || ''
  check('the landing prompt names the real lane key and its worktree',
    landPrompt.includes('LANE-KEY-Z') && landPrompt.includes('/wt/lane-z'), landPrompt.slice(0, 400))
  check('the landing prompt carries the lane\'s changed files', landPrompt.includes('x.rs'), landPrompt.slice(0, 400))
  check('the landing prompt interpolates no undefined field', !/undefined/.test(landPrompt),
    (landPrompt.split('\n').filter((x) => /undefined/.test(x)) || []).slice(0, 4))

  // LANDING MUST BE BOUND TO THE REVIEWED HEAD. Clean-worktree + recent-log is not a binding: a
  // commit added after the reviewers finished is clean and would be cherry-picked as reviewed.
  check('the landing prompt carries the head SHA captured at review time',
    landPrompt.includes('cafe1234'), landPrompt.slice(0, 400))
  check('and orders a refusal when the worktree no longer matches it',
    /rev-parse HEAD/.test(landPrompt) && /do NOT land|REFUSE/.test(landPrompt), landPrompt.slice(0, 400))

  const noConverge = mkAgent({ review: () => REVIEW({ verdict: 'reject', findings: [FINDING({ severity: 'blocker' })] }) })
  await go(ARGS({ land: true }), noConverge)
  check('nothing converged => nothing lands', !noConverge.seen.some((s) => s.label === 'land'),
    noConverge.seen.map((s) => s.label))

  const off = mkAgent()
  await go(ARGS({ land: false }), off)
  check('land:false is honoured (and warns)', !off.seen.some((s) => s.label === 'land'), off.seen.map((s) => s.label))
}

// --- 10b. A DEAD REVIEWER IS NOT AN ABSENT FINDING. -----------------------------------------
// Measured: session-limit kills took 4 of 7 agents from one run and 3 of 3 from another, and
// `.filter(Boolean)` made them vanish — a lane could converge on one surviving reviewer in silence.
{
  const standingDied = mkAgent({ review: (prompt) => (/ORACLE INTEGRITY/.test(prompt) ? null : REVIEW()) })
  const logs = []
  const r = await go(ARGS(), standingDied, logs)
  check('a dead STANDING lens blocks convergence', r.lanes[0].converged === false, r.headline)
  check('and says which one died', logs.some((m) => /CANNOT CONVERGE.*ORACLE INTEGRITY/.test(m)),
    logs.filter((m) => /CONVERGE|DIED/.test(m)))

  const customDied = mkAgent({ review: (prompt) => (/CUSTOM A/.test(prompt) ? null : REVIEW()) })
  const logs2 = []
  const r2 = await go(ARGS({ lenses: ['CUSTOM A', 'CUSTOM B'] }), customDied, logs2)
  check('a dead CUSTOM lens is tolerated (a rebuild round costs more than it saves)',
    r2.lanes[0].converged === true, r2.headline)
  check('but the death is still logged', logs2.some((m) => /DIED/.test(m)),
    logs2.filter((m) => /DIED|reviewers returned/.test(m)))
}

// --- 10d. A DEAD VERIFIER IS NOT A PASSED VERIFICATION. -------------------------------------
// The sibling of 10b, and worse: `!verify` read a died verifier as "no verifier was needed", so the
// lane converged and the log asserted "independently re-verified" having verified NOTHING. A
// standing lens is one voice among several; the verifier is the ONLY thing that reproduces a
// claimed green, so a dead one has no substitute and cannot be tolerated the way a custom lens is.
{
  const verifierDied = mkAgent({ verify: () => null })
  const logs = []
  const r = await go(ARGS(), verifierDied, logs)
  check('a dead VERIFIER blocks convergence', r.lanes[0].converged === false, r.headline)
  check('and never claims the lane was independently re-verified',
    !logs.some((m) => /independently re-verified/.test(m)), logs.filter((m) => /re-verified|VERIFIER/.test(m)))
  check('and says the verification never happened',
    logs.some((m) => /VERIFIER DIED/.test(m)), logs.filter((m) => /VERIFIER/.test(m)))

  // The deliberate exception must survive: a lane claiming nothing has nothing to falsify, so the
  // verifier is SKIPPED, and a skip is not a death.
  const partial = mkAgent({ build: () => BUILD({ status: 'partial' }) })
  const logs2 = []
  await go(ARGS(), partial, logs2)
  check('a build claiming nothing is skipped, not accused of a dead verifier',
    logs2.some((m) => /verifier skipped/.test(m)) && !logs2.some((m) => /VERIFIER DIED/.test(m)), logs2)
}

// --- 10c. Telemetry must be able to be WRONG, i.e. must observe reality. --------------------
// The old expression was ((LENSES.length + 1) * buildRounds) / buildRounds — algebraically a
// constant. It could not detect a dead agent no matter how many died.
{
  const oneDied = mkAgent({ review: (prompt) => (/BLAST RADIUS/.test(prompt) ? null : REVIEW()) })
  const r = await go(ARGS(), oneDied)
  const t = r.telemetry
  check('telemetry reports dispatched > returned when a checker dies',
    t.checkersDispatched > t.checkersReturned, { d: t.checkersDispatched, r: t.checkersReturned })
  check('telemetry names the dead checker', (t.deadCheckers || []).length === 1, t.deadCheckers)

  const allLived = mkAgent()
  const r2 = await go(ARGS(), allLived)
  check('telemetry reports equality when none die',
    r2.telemetry.checkersDispatched === r2.telemetry.checkersReturned,
    { d: r2.telemetry.checkersDispatched, r: r2.telemetry.checkersReturned })
}

// --- 10d. Review-round findings: three defects the reviewer caught that the preflight did not. ---
{
  // Two lanes in ONE worktree passed validation and were dispatched concurrently, each told by the
  // lock to build on whatever it found — so the second treats the first's half-finished edits as
  // its baseline. Silent by design.
  const m = await threw(ARGS({ lanes: [LANE({ key: 'a', wt: '/same' }), LANE({ key: 'b', wt: '/same' })] }))
  check('two lanes sharing a worktree abort', !!m && /both declare worktree/.test(m), m)

  const m2 = await threw(ARGS({ lanes: [LANE({ key: 'dup' }), LANE({ key: 'dup', wt: '/other' })] }))
  check('duplicate lane keys abort', !!m2 && /duplicate lane key/.test(m2), m2)

  // Documented and accepted must be the same set: the args comment advertised blockedTargets while
  // the allowlist rejected it, so a caller following the docs aborted.
  const ok = await go(ARGS({ lanes: [LANE({ blockedTargets: ['x'] })] }))
  check('a documented per-lane key is accepted', !!ok && Array.isArray(ok.headline), ok && Object.keys(ok || {}))

  // scopeCreep is a first-class verdict, not a hint. A reviewer setting it without ALSO restating it
  // as a blocking finding was ignored, so a lane could edit an unowned root and still converge.
  const creeper = mkAgent({ review: () => REVIEW({ verdict: 'accept', scopeCreep: true }) })
  const r = await go(ARGS(), creeper)
  check('scopeCreep alone blocks convergence', r.lanes[0].converged === false, r.headline)

  const clean = await go(ARGS(), mkAgent())
  check('and does not block when unset', clean.lanes[0].converged === true, clean.headline)
}

// --- 11. The required-field trio must stay required, or the clause is skimmable again. ------
{
  const req = (SRC.match(/required: \[[^\]]*'enforcementPlacement'[^\]]*\]/) || [''])[0]
  check('enforcementPlacement is a REQUIRED build field', /enforcementPlacement/.test(req), req.slice(0, 200))
  check('peripheralsUpdated is a REQUIRED build field', /peripheralsUpdated/.test(req), req.slice(0, 200))
  check('redBaseline is a REQUIRED build field', /redBaseline/.test(req), req.slice(0, 200))
  const rf = (SRC.match(/required: \[[^\]]*'provenByExecution'[^\]]*\]/) || [''])[0]
  check('provenByExecution and ownerLease are REQUIRED per finding',
    /provenByExecution/.test(rf) && /ownerLease/.test(rf), rf.slice(0, 200))
}

// --- 12. The lock must not be silently truncated by a nested backtick. ----------------------
{
  const lock = SRC.slice(SRC.indexOf('const BASE_LOCK = `') + 19, SRC.indexOf('\n`\n\nconst LOCK'))
  check('BASE_LOCK contains no nested backtick', (lock.match(/`/g) || []).length === 0, (lock.match(/`/g) || []).length)
  for (const clause of ['NEVER WEAKEN THE ORACLE', 'CONTRACT TESTS ARE PART OF THE CHANGE',
    'AN ENFORCEMENT MUST BE ABLE TO SEE ITS SUBJECT', 'PERIPHERALS ARE PART OF THE CHANGE',
    'THE THIRD SPELLING MEANS THE MECHANISM IS WRONG', 'NEVER pass --workflow-only']) {
    check(`lock clause survives: ${clause}`, lock.includes(clause))
  }
}

// --- 13. The caller that dispatches this harness must not bake in one machine's paths. ------
// program-tick.js chains into lane-fanout and used to send every selected lane to a hard-coded
// /Users/<name>/... worktree, ignoring both the workspace it was given and the worktree inventory
// it had just collected. On CI, on Linux, on any other machine, every implementer was pointed at a
// path that does not exist — and nothing here could see it, because the preflight only ever
// compiled lane-fanout.js. A dispatcher is part of the harness.
{
  const TICK = fs.readFileSync(path.join(SRCDIR, 'program-tick.js'), 'utf8')
  try {
    new AsyncFunction('args', 'agent', 'parallel', 'pipeline', 'log', 'phase', 'budget', 'workflow',
      TICK.replace(/^export const meta = /m, 'const meta = '))
    check('program-tick compiles as the harness evaluates it', true)
  } catch (e) {
    check('program-tick compiles as the harness evaluates it', false, e.message)
  }
  const homePaths = TICK.split('\n').filter((l) => /(\/Users\/|\/home\/)[A-Za-z0-9_.-]+\//.test(l))
  check('program-tick hard-codes no machine-specific worktree path', homePaths.length === 0, homePaths)
}

// The unknown-option guard was written for lane-fanout, repeated in backlog-audit, and skipped in
// program-tick. A rule present in two of three sibling harnesses is not a rule, it is a coincidence,
// so the preflight now asserts it across ALL of them rather than for each file someone remembers.
{
  // ENUMERATE THE DIRECTORY, DO NOT LIST IT. This was a hardcoded array of the harnesses someone
  // remembered, which is how program-tick went without the guard while its two siblings had it, and
  // how review-gate.js and slice.js sat in this directory referenced-but-never-compiled. A new
  // harness must be covered by existing here, not by being added to a list a future author edits.
  const dispatchers = fs.readdirSync(HERE)
    .filter((f) => f.endsWith('.js'))
    .map((f) => f.replace(/\.js$/, ''))
    .sort()
  check('every harness in the directory is swept, not a hardcoded subset',
    dispatchers.length >= 4 && dispatchers.includes('scout'), dispatchers)
  for (const name of dispatchers) {
    const src = fs.readFileSync(path.join(SRCDIR, `${name}.js`), 'utf8')
      .replace(/^export const meta = /m, 'const meta = ')
    const fn = new AsyncFunction('args', 'agent', 'parallel', 'pipeline', 'log', 'phase', 'budget', 'workflow', src)
    const stub = async () => ({})
    // Every required field is supplied; ONLY the bogus key should be able to fail this.
    const base = { tip: 'a'.repeat(40), lanes: [LANE()], candidateWt: '/w', candidateTip: 'b'.repeat(40),
      base: 'main', repo: '/r', ghRepo: 'o/n', maxLanes: 2 }
    let threw = null
    try {
      await fn({ ...base, thisOptionDoesNotExist: true }, stub, async (t) => Promise.all(t.map((f) => f())),
        async (i) => i, () => {}, () => {}, { total: null, spent: () => 0, remaining: () => Infinity }, stub)
    } catch (e) { threw = e.message }
    check(`${name} refuses an option it does not read`,
      !!threw && /unknown option/i.test(threw), threw)
  }
}

// A batch size of -1 produces no batches and 1.5 produces overlapping slices; both exit cleanly.
{
  const src = fs.readFileSync(path.join(SRCDIR, 'backlog-audit.js'), 'utf8')
    .replace(/^export const meta = /m, 'const meta = ')
  const fn = new AsyncFunction('args', 'agent', 'parallel', 'pipeline', 'log', 'phase', 'budget', 'workflow', src)
  for (const bad of [-1, 0, 1.5, 'eight']) {
    let threw = null
    try {
      await fn({ repo: '/r', ghRepo: 'o/n', issueBatch: bad }, async () => ({}),
        async (t) => Promise.all(t.map((f) => f())), async (i) => i, () => {}, () => {},
        { total: null, spent: () => 0, remaining: () => Infinity }, async () => ({}))
    } catch (e) { threw = e.message }
    check(`backlog-audit refuses issueBatch=${JSON.stringify(bad)}`,
      !!threw && /issueBatch must be a positive integer/.test(threw), threw)
  }
}

// The reconciler was handed `JSON.stringify(findings).slice(0, 24000)`. With ~32 read lanes the cap
// binds routinely, so the single writer filed beads for a prefix of the audit and reported success.
{
  const src = fs.readFileSync(path.join(SRCDIR, 'backlog-audit.js'), 'utf8')
  check('backlog-audit no longer truncates the findings payload blindly',
    !/JSON\.stringify\(findingsAll\)\.slice\(/.test(src))
  const body = src.match(/function renderFindings\(all\) \{[\s\S]*?\n\}/)
  check('backlog-audit exposes renderFindings to the preflight', !!body)
  if (body) {
    const renderFindings = new Function(`${body[0]}; return renderFindings`)()
    const many = Array.from({ length: 400 }, (_, i) => ({
      title: `finding ${i} ${'x'.repeat(200)}`, severity: i ? 'minor' : 'blocker',
      provenByExecution: i === 399, evidence: 'f.rs:1',
    }))
    const out = renderFindings(many)
    check('an over-budget findings set says how many it dropped', /DID NOT FIT/.test(out), out.slice(-200))
    check('the proven finding survives truncation regardless of its position',
      out.includes('finding 399'), 'the last-listed proven finding was cut')
    const few = [{ title: 'only one', severity: 'blocker', provenByExecution: true }]
    check('a set that fits carries no truncation notice', !/DID NOT FIT/.test(renderFindings(few)))
  }
}

// The reporter must survive its own failure path. Proven by capturing stdout rather than by reading
// it: a detail-less FAIL used to throw TypeError and abort the run, which is worse than a red because
// it looks like a crash in the harness instead of a defect in the code under test.
{
  const realLog = console.log
  const lines = []
  console.log = (l) => lines.push(l)
  let crashed = null
  const before = failures
  try { check('self-test: a failing assertion carries no detail', false) } catch (e) { crashed = e.message }
  console.log = realLog
  failures = before // this deliberate FAIL must not colour the real result
  check('a detail-less failure reports instead of crashing the preflight',
    crashed === null && lines.length === 1 && lines[0].startsWith('FAIL'), crashed || lines)
}

// A doc edit that fails CI on a stale generated checksum is the cheapest possible review round: the
// fix is one command, and the lane that made the edit could have run it. This assertion exists
// because a correct change was turned red by exactly that, twice.
{
  const need = ['REGENERATE, THEN ASK GIT', 'git status --porcelain', 'POSTFLIGHT', 'tools/buck/preflight.sh', 'tools/lanes/pgtest.sh']
  for (const fragment of need) {
    check(`the lock names a TOTAL generated-face check, not a list: ${fragment}`, SRC.includes(fragment))
  }
}

// --- 14. OVERLAPPING OWNED ROOTS: the duplicate-worktree collision, deferred to LAND. --------
// Separate worktrees mean two lanes cannot corrupt each other's files, so every reviewer and every
// verifier passes. The collision arrives on the integration branch afterwards, as two independent
// rewrites of the same files from the same base — the most expensive possible moment. Refuse it
// where the duplicate `wt` is refused: at dispatch.
{
  const nested = await threw(ARGS({ lanes: [
    LANE({ key: 'outer', wt: '/w1', owned: 'backend/crates/x/**' }),
    LANE({ key: 'inner', wt: '/w2', owned: 'backend/crates/x/sub/**' }),
  ] }))
  check('a lane owning a subtree of another lane aborts',
    !!nested && /overlapping owned root/i.test(nested) && /outer/.test(nested) && /inner/.test(nested), nested)

  const same = await threw(ARGS({ lanes: [
    LANE({ key: 'a1', wt: '/w1', owned: 'docs/x/**' }),
    LANE({ key: 'a2', wt: '/w2', owned: 'docs/x/**' }),
  ] }))
  check('two lanes owning the SAME root abort', !!same && /overlapping owned root/i.test(same), same)

  // An owned root is routinely a LIST. One colliding member is enough, and checking only the first
  // would be the same defect with a smaller blast radius.
  const multi = await threw(ARGS({ lanes: [
    LANE({ key: 'm1', wt: '/w1', owned: 'a/**, b/**' }),
    LANE({ key: 'm2', wt: '/w2', owned: 'c/**\nb/deep/**' }),
  ] }))
  check('one colliding path inside a multi-path owned root is enough', !!multi && /overlapping owned root/i.test(multi), multi)

  // A guard that examines nothing must FAIL, not pass: an owned root naming no path cannot be
  // compared, and it is also unusable as the reviewer's IN-SCOPE PATHS list.
  const prose = await threw(ARGS({ lanes: [LANE({ key: 'p1', owned: 'everything in the crate' })] }))
  check('an owned root with no path in it aborts rather than being silently unguarded',
    !!prose && /no path/i.test(prose), prose)

  // `./backend/crates/foo` and `backend/crates/foo` are the same root; dispatch must refuse.
  const dotted = await threw(ARGS({ lanes: [
    LANE({ key: 'bare', wt: '/w1', owned: 'backend/crates/foo/**' }),
    LANE({ key: 'dot', wt: '/w2', owned: './backend/crates/foo/**' }),
  ] }))
  check('./ and bare owned roots collide at dispatch',
    !!dotted && /overlapping owned root/i.test(dotted), dotted)

  const parentDots = await threw(ARGS({ lanes: [
    LANE({ key: 'up', wt: '/w1', owned: 'backend/crates/foo/../bar/**' }),
    LANE({ key: 'other', wt: '/w2', owned: 'docs/x/**' }),
  ] }))
  check('owned roots containing .. are refused',
    !!parentDots && /\.\./.test(parentDots), parentDots)

  // ...and it must not OVER-refuse, or it becomes a thing people work around.
  const sibling = await go(ARGS({ lanes: [
    LANE({ key: 'd1', wt: '/w1', owned: 'backend/crates/ab/**' }),
    LANE({ key: 'd2', wt: '/w2', owned: 'backend/crates/a/**' }),
  ] }))
  check('a shared string prefix that is not a PATH prefix is not an overlap',
    !!sibling && Array.isArray(sibling.headline), sibling && sibling.headline)

  const disjoint = await go(ARGS({ lanes: [
    LANE({ key: 'e1', wt: '/w1', owned: 'backend/crates/a/**' }),
    LANE({ key: 'e2', wt: '/w2', owned: 'docs/**' }),
  ] }))
  check('disjoint owned roots dispatch normally', !!disjoint && Array.isArray(disjoint.headline), disjoint && disjoint.headline)
}

// --- 15. A GREEN THE VERIFIER DID NOT ANSWER FOR IS NOT A GREEN. -----------------------------
// reproduced+contradictsClaim say the verifier ran something and agreed. They do not say WHAT it
// ran, whether any command selected zero tests and exited 0, or whether the suite still proves as
// much as it did. Oracle integrity is the most common rejection cause in this programme and a
// standing review lens, yet the schema let a verifier certify a green without ever answering it.
{
  const req = (SRC.match(/const VERIFY_SCHEMA = \{[\s\S]*?required: \[([^\]]*)\]/) || ['', ''])[1]
  for (const f of ['falseGreenRisk', 'commandsRun', 'oracleIntact']) {
    check(`${f} is a REQUIRED verifier field`, req.includes(f), req.slice(0, 300))
  }

  // The schema is a request, not an enforcement — the harness must refuse the green itself.
  const noCommands = mkAgent({ verify: () => VERIFY({ commandsRun: [] }) })
  check('a verifier that lists no command it RAN cannot certify a green',
    (await go(ARGS(), noCommands)).lanes[0].converged === false)

  const noRisk = mkAgent({ verify: () => VERIFY({ falseGreenRisk: '' }) })
  check('a verifier that leaves the false-green risk blank cannot certify a green',
    (await go(ARGS(), noRisk)).lanes[0].converged === false)

  const silent = mkAgent({ verify: () => VERIFY({ oracleIntact: undefined }) })
  check('a verifier that never answers oracle integrity cannot certify a green',
    (await go(ARGS(), silent)).lanes[0].converged === false)

  const weakened = mkAgent({ verify: () => VERIFY({ oracleIntact: false }) })
  check('a verifier that OBSERVED a weakened oracle blocks convergence',
    (await go(ARGS(), weakened)).lanes[0].converged === false)

  // A rejection with no actionable text manufactures a wasted round, so the next build must be told.
  const seenPrompts = []
  const partialAnswer = mkAgent({
    build: (seen) => { seenPrompts.push(seen[seen.length - 1].prompt); return BUILD() },
    verify: () => VERIFY({ commandsRun: [] }),
  })
  await go(ARGS({ maxRounds: 2 }), partialAnswer)
  check('and the next round is told the verification was unanswered, not merely "disagreed"',
    seenPrompts.length === 2
      && (/commandsRun/.test(seenPrompts[1])
        || /NEVER independently run/.test(seenPrompts[1])
        || /named no well-formed commands/.test(seenPrompts[1])),
    { n: seenPrompts.length, round2: (seenPrompts[1] || '').slice(0, 400) })

  // A field nobody is asked for is a field nobody fills in.
  const asked = mkAgent()
  await go(ARGS(), asked)
  const vp = (asked.seen.find((s) => s.label.startsWith('verify:')) || {}).prompt || ''
  check('the verifier is ASKED for the commands it ran and for an oracle verdict',
    /commandsRun/.test(vp) && /oracleIntact/.test(vp), vp.slice(0, 300))

  const full = await go(ARGS(), mkAgent())
  check('a fully answered verification still converges', full.lanes[0].converged === true, full.headline)
}

// --- 16. program-tick is the CALLER, so a collision must be refused where the set is BUILT. ---
// Refusing downstream in lane-fanout is necessary and late: by then the agents are already chosen.
// These assertions drive the REAL program-tick body, with a genuinely concurrent parallel(), and
// observe what it dispatches.
{
  const compileWorkflow = (name) => new AsyncFunction(
    'args', 'agent', 'parallel', 'pipeline', 'log', 'phase', 'budget', 'workflow',
    fs.readFileSync(path.join(SRCDIR, `${name}.js`), 'utf8').replace(/^export const meta = /m, 'const meta = '))
  const tick = compileWorkflow('program-tick')

  const WT = (over = {}) => ({ path: '/ws/x', head: 'abc', dirtyCount: 0, prunable: false, filesVsBase: [], commitsAheadOfCandidate: 0, ...over })
  const RAW = (over = {}) => ({ candidateFiles: ['f.rs'], worktrees: [], prs: [], beads: [], ...over })
  const JUDGED = (over = {}) => ({ startNow: [], holdBack: [], alreadyDone: [], coverageRisks: [], ...over })
  const PR = (n, over = {}) => ({ number: n, title: `pr${n}`, checkConclusion: 'FAILURE', reviewDecision: '', mergeable: 'MERGEABLE', mergeStateStatus: 'CLEAN', headSha: `sha${n}`, failingChecks: ['t'], isDraft: false, ...over })
  const SEL = (n) => Array.from({ length: n }, (_, i) => ({ key: `l${i + 1}`, bead: `b${i + 1}`, owned: `p${i + 1}/**`, brief: 'concrete', accept: 'a', briefConfidence: 'grounded' }))

  const runTick = async (over = {}, raw = RAW(), judged = JUDGED()) => {
    const inflight = new Set()
    const overlaps = []
    const dispatched = []
    const logs = []
    const workflows = []
    const agentFn = async (prompt, o = {}) => {
      const label = o.label || ''
      dispatched.push(label)
      if (label === 'collect') return raw
      if (label === 'judge') return judged
      // A PR disposition agent. `fix-then-merge` is WORK: it edits files in the candidate worktree.
      inflight.add(label)
      if (inflight.size > 1) overlaps.push([...inflight])
      await new Promise((r) => setTimeout(r, 5))
      inflight.delete(label)
      return { pr: 1, done: true, outcome: 'x' }
    }
    // REAL concurrency. A sequential stub would make serialisation indistinguishable from its
    // absence, which is how a test measures the fixture instead of the code.
    const par = async (thunks) => Promise.all(thunks.map((t) => t()))
    const plan = await tick(
      { candidateWt: '/ws/cand', candidateTip: 'c'.repeat(40), base: 'main', ...over },
      agentFn, par, async (i) => i, (m) => logs.push(m), () => {},
      { total: null, spent: () => 0, remaining: () => Infinity },
      async (name, a) => { workflows.push({ name, args: a }); return { headline: [] } },
    )
    return { plan, overlaps, dispatched, logs, workflows }
  }

  const twoFixes = await runTick({}, RAW({ prs: [PR(1), PR(2)] }))
  // Both halves matter: a guard that dispatched ZERO PR lanes would also report zero overlaps.
  check('two PR-fix lanes are both dispatched',
    twoFixes.dispatched.filter((l) => l.startsWith('pr:')).length === 2, twoFixes.dispatched)
  check('...and never run concurrently in the one candidate worktree they both edit',
    twoFixes.overlaps.length === 0, twoFixes.overlaps)

  const overCap = await runTick({ fanout: true, maxLanes: 4 },
    RAW({ worktrees: [1, 2, 3, 4, 5].map((i) => WT({ path: `/ws/l${i}` })) }), JUDGED({ startNow: SEL(5) }))
  check('more selected lanes than maxLanes refuses the fanout instead of ignoring the cap',
    overCap.workflows.length === 0 && !!overCap.plan.fanoutBlocked,
    { workflows: overCap.workflows.length, blocked: overCap.plan.fanoutBlocked })
  check('and says the cap is why', /maxLanes/.test(JSON.stringify(overCap.plan.fanoutBlocked || '')), overCap.plan.fanoutBlocked)

  const withinCap = await runTick({ fanout: true, maxLanes: 4 },
    RAW({ worktrees: [1, 2, 3, 4].map((i) => WT({ path: `/ws/l${i}` })) }), JUDGED({ startNow: SEL(4) }))
  check('a lane set within the cap still fans out',
    withinCap.workflows.length === 1 && (withinCap.workflows[0].args.lanes || []).length === 4,
    withinCap.workflows.map((w) => w.name))

  const wts = await runTick({}, RAW({ worktrees: [
    WT({ path: '/ws/cand' }),
    WT({ path: '/ws/idle' }),
    WT({ path: '/ws/unreadable', commitsAheadOfCandidate: -1 }),
    WT({ path: '/ws/capped', filesVsBase: [], filesVsBaseCount: 900 }),
  ] }))
  const safe = wts.plan.worktrees.safeToRemove
  check('the ACTIVE candidate worktree is never offered as safe to remove', !safe.includes('/ws/cand'), safe)
  check('an unreadable worktree is not "unused"', !safe.includes('/ws/unreadable'), safe)
  check('a worktree whose file list was capped is not "empty"', !safe.includes('/ws/capped'), safe)
  // ...and the list is not simply emptied, which would pass all three above and help nobody.
  check('a genuinely empty worktree is still removable', safe.includes('/ws/idle'), safe)
}

// --- 17. backlog-audit: evidence nobody landed is not evidence, and coverage must be real. ----
{
  const audit = new AsyncFunction(
    'args', 'agent', 'parallel', 'pipeline', 'log', 'phase', 'budget', 'workflow',
    fs.readFileSync(path.join(SRCDIR, 'backlog-audit.js'), 'utf8').replace(/^export const meta = /m, 'const meta = '))

  const CENSUS = (over = {}) => {
    const base = {
      openIssueNumbers: [1], openIssueCount: 1, issues: [], beads: [],
      crates: [{ name: 'identity' }, { name: 'policy' }],
      cargoTomlPaths: ['backend/crates/identity/Cargo.toml', 'backend/crates/policy/Cargo.toml'],
      mergedPrs: [], excludedRoots: [],
    }
    const merged = { ...base, ...over }
    // Keep cargoTomlPaths consistent with crates unless the caller overrides either explicitly.
    if (!('cargoTomlPaths' in over) && 'crates' in over) {
      merged.cargoTomlPaths = (merged.crates || []).map((c) => `backend/crates/${c.name}/Cargo.toml`)
    }
    return merged
  }
  const VERDICT = (over = {}) => ({
    number: 1, title: 't', verdict: 'CLOSE-FIXED',
    evidence: 'implemented in backend/crates/identity/src/lib.rs:12 by commit deadbeefcafe1234 — verified by reading it',
    ...over,
  })

  const runAudit = async (census, verdicts, over = {}) => {
    const dispatched = []
    const logs = []
    const agentFn = async (prompt, o = {}) => {
      const label = o.label || ''
      dispatched.push({ label, prompt })
      if (label === 'collect') return census
      // Independent disk oracle — must not reuse Collect's crates as its only source.
      if (label === 'crate-disk-census') {
        return { cargoTomlPaths: census.cargoTomlPaths || [] }
      }
      if (label.startsWith('triage:')) return { verdicts }
      if (label === 'reconcile') return { ok: true }
      return { domain: label, findings: [], coverage: 'read it all' }
    }
    let res = null
    let err = null
    try {
      res = await audit({ repo: '/r', ghRepo: 'o/n', ...over }, agentFn,
        async (t) => Promise.all(t.map((f) => f())), async (i) => i, (m) => logs.push(m), () => {},
        { total: null, spent: () => 0, remaining: () => Infinity }, async () => ({}))
    } catch (e) { err = e.message }
    return { res, err, dispatched, logs }
  }

  const unmerged = await runAudit(CENSUS(), [VERDICT({ reachableFromDefault: false })])
  check('a CLOSE-FIXED whose evidence is not on the default branch is WITHHELD',
    !!unmerged.res && (unmerged.res.withheld || []).includes(1), unmerged.res && unmerged.res.withheld)

  const unanswered = await runAudit(CENSUS(), [VERDICT()])
  check('a CLOSE-FIXED that never answered reachability is WITHHELD too — absence is a NO',
    !!unanswered.res && (unanswered.res.withheld || []).includes(1), unanswered.res && unanswered.res.withheld)

  const landedEv = await runAudit(CENSUS(), [VERDICT({ reachableFromDefault: true })])
  check('a CLOSE-FIXED reachable from the default branch is still closable',
    !!landedEv.res && !(landedEv.res.withheld || []).includes(1), landedEv.res && landedEv.res.withheld)

  const keep = await runAudit(CENSUS(), [VERDICT({ verdict: 'KEEP', evidence: 'this is still broken, here is the file and line that shows it' })])
  check('a KEEP verdict is not withheld — the rule is about CLOSING',
    !!keep.res && (keep.res.withheld || []).length === 0, keep.res && keep.res.withheld)

  check('triage is told to PROVE reachability by running merge-base --is-ancestor',
    landedEv.dispatched.some((d) => d.label.startsWith('triage:') && /merge-base --is-ancestor/.test(d.prompt)),
    landedEv.dispatched.map((d) => d.label))

  const extraCrate = await runAudit(CENSUS({ crates: [{ name: 'identity' }, { name: 'brand-new-crate' }] }),
    [VERDICT({ reachableFromDefault: true })])
  // Match the lane's OWN crate list, not the census echoed into every audit prompt — the echo made
  // this assertion pass against the unfixed source, which is a test measuring its own fixture.
  check('a discovered crate that no named domain claims is still audited',
    extraCrate.dispatched.some((d) => /^audit:/.test(d.label) && /CRATES:[^\n]*brand-new-crate/.test(d.prompt)),
    extraCrate.dispatched.map((d) => d.label))

  const allCovered = await runAudit(CENSUS({ crates: [{ name: 'identity' }] }), [VERDICT({ reachableFromDefault: true })])
  check('and no lane is invented when every discovered crate is claimed',
    !allCovered.dispatched.some((d) => d.label === 'audit:uncovered'), allCovered.dispatched.map((d) => d.label))

  const blind = await runAudit(CENSUS({ crates: [] }), [VERDICT({ reachableFromDefault: true })])
  check('a census with no crate inventory cannot claim coverage and must abort',
    !!blind.err && /crate inventory/i.test(blind.err), blind.err)

  // crate-disk-census is the independent oracle (workflow sandbox has no Node fs). Omitting a
  // path that find would have returned must abort — same fail-closed class as a partial crates list.
  // A co-emitted Collect.cargoTomlPaths is NOT enough: Collect can omit from both fields together.
  const omittedDisk = await runAudit(CENSUS({
    crates: [{ name: 'identity' }],
    cargoTomlPaths: ['backend/crates/identity/Cargo.toml', 'backend/crates/brand-new/Cargo.toml'],
  }), [VERDICT({ reachableFromDefault: true })])
  check('a census that omits a crate-disk-census crate aborts',
    !!omittedDisk.err && /omitted/i.test(omittedDisk.err), omittedDisk.err)
  check('coverage uses a dedicated crate-disk-census agent, not Collect alone',
    omittedDisk.dispatched.some((d) => d.label === 'crate-disk-census'))

  const emptyToml = await runAudit(CENSUS({
    crates: [{ name: 'identity' }],
    cargoTomlPaths: [],
  }), [VERDICT({ reachableFromDefault: true })])
  check('an empty crate-disk-census list cannot cross-check coverage and must abort',
    !!emptyToml.err && /cargoTomlPaths|crate-disk-census/i.test(emptyToml.err), emptyToml.err)

  // Hostile: Collect's crates list is internally consistent and would have matched a co-emitted
  // cargoTomlPaths — the old self-validation false green. The independent disk census still sees
  // the omitted crate and must abort.
  const coordinatedPartial = await runAudit(CENSUS({
    crates: [{ name: 'identity' }, { name: 'policy' }],
    cargoTomlPaths: [
      'backend/crates/identity/Cargo.toml',
      'backend/crates/policy/Cargo.toml',
      'backend/crates/brand-new/Cargo.toml',
    ],
  }), [VERDICT({ reachableFromDefault: true })])
  check('a Collect list that omits an on-disk crate aborts even when well-formed',
    !!coordinatedPartial.err && /omitted/i.test(coordinatedPartial.err)
      && /brand-new/.test(coordinatedPartial.err), coordinatedPartial.err)
}

// Six green tests over a self-built registry coexisted with a production root that wired none of it.
// The lock must name the distinction, because "all the tests pass" is exactly how it presented.
{
  for (const fragment of ['BUILDS ITS OWN SUBJECT', 'MECHANISM:', 'WIRING:', 'composition root']) {
    check(`the lock separates mechanism evidence from wiring evidence: ${fragment}`, SRC.includes(fragment))
  }
}

// Sprawl and CI-caught-it-first are the two costs the harness never charged for.
{
  for (const fragment of ['MAINTAINABILITY / COST OF CARRY', 'COMMENT BLOBBING', 'RUN WHAT CI RUNS']) {
    check(`the lock charges for cost of carry: ${fragment}`, SRC.includes(fragment))
  }
  // A lens that is defined but unreachable is the defect this file exists to catch.
  const standing = SRC.match(/const STANDING_LENSES = \[([\s\S]*?)\n\]/)
  check('the maintainability lens is a STANDING lens, not an opt-in',
    !!standing && standing[1].includes('MAINTAINABILITY'))
}

// scout.js was added to the unknown-option sweep and NOTHING ELSE, so the harness that decides what
// every other lane works on was the least tested one in the directory. Both of its judgement calls
// shipped defective and both were caught by a real run rather than here: the packing produced two
// lanes owning the same territory, and agent-authored paths reached the emitted plan unvalidated.
// These assertions extract the two pure functions and drive them with the EXACT strings that run
// produced, so neither can regress silently.
{
  const SCOUT = fs.readFileSync(path.join(HERE, 'scout.js'), 'utf8')

  const normSrc = SCOUT.match(/function normaliseRoot\(raw\) \{[\s\S]*?\n\}/)
  check('scout exposes normaliseRoot to the preflight', !!normSrc)
  if (normSrc) {
    const REPO = '/Users/x/wt'
    const normaliseRoot = new Function('REPO', 'MIN_ROOT_SEGMENTS', `${normSrc[0]}; return normaliseRoot`)(REPO, 2)
    const cases = [
      [`${REPO}/backend/crates/platform/audit-chain/src/`, 'backend/crates/platform/audit-chain/src/', 'absolute path made repo-relative'],
      ['<the', null, 'prose fragment rejected'],
      ['`git', null, 'shell fragment rejected'],
      ['backend/', null, 'the repository is not an owned root'],
      ['docs/', null, 'nor is a top-level directory'],
      ['backend/app/src/hr.rs', 'backend/app/src/', 'a file becomes its directory'],
      ['backend/app/src/', 'backend/app/src/', 'a real root survives unchanged'],
      ['./backend/crates/foo/', 'backend/crates/foo/', './ prefix collapses to the same root'],
      ['backend/crates/foo/', 'backend/crates/foo/', 'bare form matches the ./ form'],
      ['/', null, 'root rejected'],
      ['', null, 'empty rejected'],
      ['backend/**/*.rs', null, 'glob rejected'],
      ['backend/crates/foo/../bar/', null, '.. segments rejected'],
      ['/Users/x/wt-other/backend/crates/foo/', null, 'sibling worktree absolute rejected'],
      ['/tmp/evil/backend/crates/foo/src/lib.rs', null, 'absolute outside REPO rejected'],
      ['backend/Dockerfile', 'backend/Dockerfile', 'extensionless file preserved as exact path'],
      ['backend/crates/foo/BUCK', 'backend/crates/foo/BUCK', 'BUCK file preserved as exact path'],
      ['tools/ci/Makefile', 'tools/ci/Makefile', 'Makefile preserved as exact path'],
    ]
    for (const [input, want, why] of cases) {
      check(`scout root: ${why}`, normaliseRoot(input) === want, { input, got: normaliseRoot(input), want })
    }
    check('scout root: ./ and bare forms are identical after normalise',
      normaliseRoot('./backend/crates/foo/') === normaliseRoot('backend/crates/foo/'),
      { a: normaliseRoot('./backend/crates/foo/'), b: normaliseRoot('backend/crates/foo/') })
  }

  // The packing must not be able to emit two lanes sharing territory. The first version decided
  // membership one bead at a time against a set that was still moving, so absorbing a bead grew a
  // lane's roots until it overlapped a lane created earlier -- and a run that had completed all
  // fifteen of its agents threw at the final guard and discarded the lot.
  const ovSrc = SCOUT.match(/function overlaps\(a, b\) \{[\s\S]*?\n\}/)
  check('scout exposes overlaps to the preflight', !!ovSrc)
  if (ovSrc) {
    const overlaps = new Function(`${ovSrc[0]}; return overlaps`)()
    check('scout overlap: a prefix counts as shared territory',
      overlaps(['backend/app/'], ['backend/app/src/']))
    check('scout overlap: siblings do not',
      !overlaps(['backend/app/'], ['backend/crates/']))

    // The transitive case the incremental packer got wrong: A and C do not overlap, but B bridges
    // them, so all three belong in ONE lane. A packer that pairs A-B then creates C separately emits
    // two lanes that collide at land.
    const items = [
      { id: 'a', roots: ['backend/app/src/'] },
      { id: 'b', roots: ['backend/app/', 'backend/crates/x/'] },
      { id: 'c', roots: ['backend/crates/x/y/'] },
    ]
    const parent = items.map((_, i) => i)
    const find = (i) => (parent[i] === i ? i : (parent[i] = find(parent[i])))
    for (let i = 0; i < items.length; i++) {
      for (let j = i + 1; j < items.length; j++) {
        if (overlaps(items[i].roots, items[j].roots)) parent[find(i)] = find(j)
      }
    }
    check('scout packing: transitively-linked territory collapses to ONE group',
      new Set(items.map((_, i) => find(i))).size === 1,
      items.map((_, i) => find(i)))
  }

  // A guard that only exists in scout.js text is a guard nobody proved runs.
  check('scout still refuses to emit a plan whose lanes overlap',
    /own overlapping roots/.test(SCOUT))
  check('scout defers a bead whose paths are all unusable rather than inventing a root',
    /every reported path was unusable as an owned root/.test(SCOUT))
}

// TRIAL GATE. Bun proved its port method on three files before scaling to 64 agents; this harness
// always dispatched every lane cold, so a brief that is wrong the same way for every lane wastes
// the whole wave instead of one lane.
{
  const TWO = (over = {}) => ARGS({
    // Distinct worktrees AND distinct owned roots: the overlap guard is a sibling rule and this
    // fixture must not trip it while testing something else.
    lanes: [LANE({ key: 'a', wt: '/w1', owned: 'aa/**' }), LANE({ key: 'b', wt: '/w2', owned: 'bb/**' })],
    ...over,
  })

  // A failing trial must hold the fleet back, and must NOT dispatch lane b at all.
  {
    const agent = mkAgent({ review: () => REVIEW({ verdict: 'reject', findings: [FINDING({ severity: 'blocker' })] }) })
    const out = await go(TWO({ trial: 'a', maxRounds: 1 }), agent)
    const bDispatched = agent.seen.some((x) => /:b\b/.test(x.label))
    check('a failed trial holds the fleet back', !!out && /DID NOT CONVERGE/.test(out.headline[0]), out && out.headline)
    check('a failed trial dispatches NO other lane', !bDispatched,
      agent.seen.map((x) => x.label))
    check('the held-back lanes are named, not silently dropped',
      !!out && Array.isArray(out.heldBack) && out.heldBack.includes('b'), out && out.heldBack)
  }

  // A converged trial must go on to dispatch the rest.
  {
    const agent = mkAgent()
    const out = await go(TWO({ trial: 'a', maxRounds: 1 }), agent)
    const bDispatched = agent.seen.some((x) => /:b\b/.test(x.label))
    check('a converged trial dispatches the remaining lanes', bDispatched,
      agent.seen.map((x) => x.label))
  }

  // Naming a lane that does not exist is a typo, not a silent no-trial run.
  {
    const m = await threw(TWO({ trial: 'nope' }))
    check('trial naming an unknown lane aborts', !!m && /nope/.test(m), m)
  }
  {
    const m = await threw(ARGS({ trial: 'a' }))
    check('trial with a single lane aborts rather than serialising for nothing',
      !!m && /serialises for nothing/.test(m), m)
  }
}

// The tier rule must stay a RULE, not a preference someone reverses on a slow day.
{
  check('the lock states when a cheaper tier is allowed',
    SRC.includes('A CHEAPER TIER IS ALLOWED ONLY WHERE AN INDEPENDENT STRONGER PASS AUDITS THE RESULT'))
  const judgePhases = SRC.match(/label: `verify:[\s\S]{0,160}/g) || []
  check('the independent verifier does NOT run on a cheaper tier',
    judgePhases.every((frag) => !/model:/.test(frag)), judgePhases.length)
}

// KNOWN_ARGS IS A CLAIM ABOUT THE SOURCE, SO CHECK IT AGAINST THE SOURCE.
// The guard's whole purpose is "an option this harness does not read must abort". Two of these
// lists were hand-written and both were wrong in BOTH directions at once: slice.js omitted five
// options it genuinely reads (so the guard rejected every real invocation) while review-gate.js
// listed two it never reads (so the guard fell open on exactly what it exists to catch). A
// hand-maintained list of what the code reads is a second copy of the code.
{
  for (const name of fs.readdirSync(HERE).filter((f) => f.endsWith('.js')).map((f) => f.replace(/\.js$/, ''))) {
    const src = fs.readFileSync(path.join(HERE, `${name}.js`), 'utf8')
    const declared = src.match(/const KNOWN_ARGS = \[([^\]]*)\]/)
    if (!declared) { check(`${name} declares KNOWN_ARGS`, false); continue }
    const listed = new Set([...declared[1].matchAll(/'([^']+)'/g)].map((m) => m[1]))

    // Whichever accessor this harness uses for its parsed args.
    const holder = /const KNOWN_ARGS[\s\S]{0,400}?\b(ARGS|A)\b\s*\)/.exec(src)?.[1]
      || (src.includes('const ARGS') || src.includes('let ARGS') ? 'ARGS' : 'A')
    const read = new Set(
      [...src.matchAll(new RegExp(`\\b${holder}\\.([a-zA-Z_][a-zA-Z0-9_]*)`, 'g'))]
        .map((m) => m[1])
        .filter((k) => !['length', 'lanes'].includes(k) || k === 'lanes'),
    )
    // Object.keys(ARGS) inside the guard itself is not an option read.
    read.delete('keys')

    const unread = [...listed].filter((k) => !read.has(k))
    const undeclared = [...read].filter((k) => !listed.has(k))
    check(`${name}: KNOWN_ARGS lists nothing the harness never reads`, unread.length === 0, unread)
    check(`${name}: every option the harness reads is declared`, undeclared.length === 0, undeclared)
  }
}

// stale-take-audit.js had NO logic coverage here — only the generic KNOWN_ARGS sweep — and its
// Confirm phase failed open exactly the way the Audit phase does not. A dead agent yielded
// `refuted: null`, which is neither `=== false` nor truthy, so the suspicion fell out of BOTH result
// lists and the headline still printed "full coverage". This is the step that decides whether a
// reported reversion is REAL, and the reversion it exists to catch is a file whose un-wiring means
// the check that would have caught it does not run.
{
  const STA = fs.readFileSync(path.join(HERE, 'stale-take-audit.js'), 'utf8')
    .replace(/^export const meta = /m, 'const meta = ')
  const fn = new AsyncFunction('args', 'agent', 'parallel', 'pipeline', 'log', 'phase', 'budget', 'workflow', STA)

  const drive = async (confirmReturns) => {
    const agent = async (prompt, o = {}) => {
      if ((o.label || '').startsWith('audit:')) {
        return { results: [{ file: '.github/workflows/ci.yml', verdict: 'STALE', evidence: 'main has a step HEAD lacks', missingFromHead: 'the step', wouldBreak: 'the suite un-wires' }] }
      }
      return confirmReturns()
    }
    return fn({ repo: '/r', main: 'origin/main', files: ['.github/workflows/ci.yml'] },
      agent, async (t) => Promise.all(t.map((f) => f().catch(() => null))), async (i) => i,
      () => {}, () => {}, { total: null, spent: () => 0, remaining: () => Infinity }, async () => ({}))
  }

  const dead = await drive(() => null)
  check('a dead confirmation does not erase the suspicion',
    !!dead && Array.isArray(dead.unconfirmed) && dead.unconfirmed.length === 1,
    dead && { stale: dead.stale, refuted: dead.refuted, unconfirmed: dead.unconfirmed })
  check('a dead confirmation stops the report claiming full coverage',
    !!dead && !dead.headline.some((h) => /full coverage/.test(h)), dead && dead.headline)

  // A live agent that omits the field despite the schema must land in the same bucket: the schema is
  // a request to the model, not an enforcement.
  const fieldless = await drive(() => ({ file: '.github/workflows/ci.yml', reasoning: 'no verdict' }))
  check('a confirmation without a verdict is unresolved, not clean',
    !!fieldless && fieldless.unconfirmed.length === 1, fieldless && fieldless.unconfirmed)

  // Controls: the live paths must still work, or the fix is an over-block.
  // Upholding STALE also requires this pass to attest the graft payload — publishing the
  // first agent's missingFromHead unseen is how a wrong quote becomes the "confirmed" patch.
  const kept = await drive(() => ({ file: '.github/workflows/ci.yml', refuted: false, reasoning: 'real', missingFromHead: 'the step (attested)' }))
  check('a live confirmation that fails to refute still reports STALE',
    !!kept && kept.stale.length === 1 && kept.unconfirmed.length === 0
      && kept.stale[0].missingFromHead === 'the step (attested)',
    kept && { s: kept.stale, u: kept.unconfirmed })
  const noPayload = await drive(() => ({ file: '.github/workflows/ci.yml', refuted: false, reasoning: 'real but no graft' }))
  check('an upheld STALE without an attested graft payload is unresolved',
    !!noPayload && noPayload.stale.length === 0 && noPayload.unconfirmed.length === 1,
    noPayload && { s: noPayload.stale, u: noPayload.unconfirmed })
  // console-zd7: a confirmed-stale verdict that never attested content must not publish the
  // first agent's text under the graft-shaped key. Unconfirmed may retain the claim under a
  // distinctly-named field so operators see the accusation without a ready-to-apply payload.
  check('an upheld STALE without attestation does not publish first-pass text as missingFromHead',
    !!noPayload && noPayload.stale.length === 0
      && !JSON.stringify(noPayload.stale).includes('the step')
      && noPayload.unconfirmed.length === 1
      && !Object.prototype.hasOwnProperty.call(noPayload.unconfirmed[0], 'missingFromHead')
      && noPayload.unconfirmed[0].claimedMissingFromHead === 'the step',
    noPayload && { s: noPayload.stale, u: noPayload.unconfirmed })
  const blankPayload = await drive(() => ({ file: '.github/workflows/ci.yml', refuted: false, reasoning: 'real', missingFromHead: '   ' }))
  check('an upheld STALE with a blank graft payload is unresolved',
    !!blankPayload && blankPayload.stale.length === 0 && blankPayload.unconfirmed.length === 1,
    blankPayload && { s: blankPayload.stale, u: blankPayload.unconfirmed })
  check('a blank confirmer payload does not expose a graft-shaped missingFromHead either',
    !!blankPayload && blankPayload.unconfirmed.length === 1
      && !Object.prototype.hasOwnProperty.call(blankPayload.unconfirmed[0], 'missingFromHead')
      && blankPayload.unconfirmed[0].claimedMissingFromHead === 'the step',
    blankPayload && blankPayload.unconfirmed)
  // Confirmer attests a DIFFERENT payload: publish only that. Publishing the first-pass quote
  // when the confirmation returned something else is the "mismatched" fail-open.
  check('confirmed stale publishes the confirmer payload, never the first-pass quote',
    !!kept && kept.stale[0].missingFromHead === 'the step (attested)'
      && kept.stale[0].missingFromHead !== 'the step',
    kept && kept.stale[0])
  // Confirm must re-derive the graft from diffs. Handing the first-pass quote in the prompt
  // invites rubber-stamping an unread payload (claim, not evidence).
  {
    let confirmPrompt = null
    const agent = async (prompt, o = {}) => {
      if ((o.label || '').startsWith('audit:')) {
        return { results: [{ file: '.github/workflows/ci.yml', verdict: 'STALE', evidence: 'main has a step HEAD lacks', missingFromHead: 'FIRST_PASS_SECRET_GRAFT', wouldBreak: 'the suite un-wires' }] }
      }
      confirmPrompt = prompt
      return { file: '.github/workflows/ci.yml', refuted: true, reasoning: 'deliberate', missingFromHead: '' }
    }
    await fn({ repo: '/r', main: 'origin/main', files: ['.github/workflows/ci.yml'] },
      agent, async (t) => Promise.all(t.map((f) => f().catch(() => null))), async (i) => i,
      () => {}, () => {}, { total: null, spent: () => 0, remaining: () => Infinity }, async () => ({}))
    check('confirm prompt does not offer the first-pass graft payload for rubber-stamping',
      typeof confirmPrompt === 'string'
        && !/FIRST_PASS_SECRET_GRAFT/.test(confirmPrompt)
        && !/PAYLOAD OFFERED/.test(confirmPrompt)
        && /EVIDENCE OFFERED/.test(confirmPrompt),
      confirmPrompt && confirmPrompt.slice(0, 400))
  }
  // Oracle integrity: the pre-zd7 publish path (`missingFromHead: r.missingFromHead` from the
  // audit spread) would leak the first-pass quote under a confirmed-stale verdict. Mutating the
  // control back to that mapping must go red against the attestation pin.
  {
    const leaked = { file: 'ci.yml', missingFromHead: 'FIRST_PASS_WRONG', confirmedMissing: null, refuted: false }
    const oldPublish = [leaked].filter((c) => c.refuted === false)
      .map((r) => ({ file: r.file, missingFromHead: r.missingFromHead }))
    const newPublish = [leaked].filter((c) =>
      c.refuted === false
      && typeof c.confirmedMissing === 'string'
      && c.confirmedMissing.trim() !== '')
      .map((r) => ({ file: r.file, missingFromHead: r.confirmedMissing }))
    check('mutate→red: old confirmed-stale publish path leaks the first-pass graft',
      oldPublish.length === 1 && oldPublish[0].missingFromHead === 'FIRST_PASS_WRONG'
        && newPublish.length === 0,
      { oldPublish, newPublish })
  }
  const dropped = await drive(() => ({ file: '.github/workflows/ci.yml', refuted: true, reasoning: 'deliberate', missingFromHead: '' }))
  check('a live refutation still drops the suspicion',
    !!dropped && dropped.stale.length === 0 && dropped.refuted.length === 1 && dropped.unconfirmed.length === 0,
    dropped && { s: dropped.stale, r: dropped.refuted, u: dropped.unconfirmed })
  check('a fully-answered run still claims full coverage',
    !!dropped && dropped.headline.some((h) => /full coverage/.test(h)), dropped && dropped.headline)

  // Partial result lists must not report full coverage: five verdicts for six files is incomplete.
  const partialAudit = async () => {
    const agent = async (prompt, o = {}) => {
      if ((o.label || '').startsWith('audit:')) {
        return {
          results: [
            { file: 'a.yml', verdict: 'CLEAN', evidence: 'ok' },
            { file: 'b.yml', verdict: 'CLEAN', evidence: 'ok' },
            { file: 'c.yml', verdict: 'CLEAN', evidence: 'ok' },
            { file: 'd.yml', verdict: 'CLEAN', evidence: 'ok' },
            { file: 'e.yml', verdict: 'CLEAN', evidence: 'ok' },
            // f.yml omitted
          ],
        }
      }
      return null
    }
    return fn({ repo: '/r', main: 'origin/main', files: ['a.yml', 'b.yml', 'c.yml', 'd.yml', 'e.yml', 'f.yml'] },
      agent, async (t) => Promise.all(t.map((f) => f().catch(() => null))), async (i) => i,
      () => {}, () => {}, { total: null, spent: () => 0, remaining: () => Infinity }, async () => ({}))
  }
  const partial = await partialAudit()
  check('a live audit that omits a requested file does not claim full coverage',
    !!partial && !partial.headline.some((h) => /full coverage/.test(h))
      && Array.isArray(partial.missingAuditFiles) && partial.missingAuditFiles.includes('f.yml'),
    partial && { headline: partial.headline, missing: partial.missingAuditFiles })

  check('stale-take RULES pin diffs to args.repo via git -C',
    /git -C \$\{REPO\} diff HEAD/.test(STA) && /git -C \$\{REPO\} diff \$\{MAIN\} HEAD/.test(STA))
}

// A verifier that re-ran ONE of five claimed commands satisfied `commandsRun.length > 0`, and with
// reproduced=true the lane converged while four suites were never independently run. Some of the
// evidence re-run is a sample, not a verification.
{
  const build = (cmds) => () => BUILD({ commands: cmds })
  const CMDS = ['cargo test -p a', 'cargo test -p b', 'npm run check:x']

  const partial = mkAgent({ build: build(CMDS), verify: () => VERIFY({ commandsRun: [CMDS[0]], falseGreenRisk: 'none', oracleIntact: true }) })
  const r1 = await go(ARGS({ maxRounds: 1 }), partial)
  check('a verifier that re-ran only some claimed commands does not converge',
    !!r1 && r1.lanes[0].converged === false, r1 && r1.lanes[0].converged)

  const full = mkAgent({ build: build(CMDS), verify: () => VERIFY({ commandsRun: [...CMDS], falseGreenRisk: 'none', oracleIntact: true }) })
  const r2 = await go(ARGS({ maxRounds: 1 }), full)
  check('a verifier that re-ran every claimed command still converges',
    !!r2 && r2.lanes[0].converged === true, r2 && r2.lanes[0].converged)

  // Whitespace must not decide it, and repetition must not substitute for coverage.
  const spaced = mkAgent({ build: build(CMDS), verify: () => VERIFY({ commandsRun: CMDS.map((c) => `  ${c.replace(/ /g, '  ')} `), falseGreenRisk: 'none', oracleIntact: true }) })
  const r3 = await go(ARGS({ maxRounds: 1 }), spaced)
  check('command matching is not defeated by whitespace', !!r3 && r3.lanes[0].converged === true, r3 && r3.lanes[0].converged)

  const repeated = mkAgent({ build: build(CMDS), verify: () => VERIFY({ commandsRun: [CMDS[0], CMDS[0], CMDS[0]], falseGreenRisk: 'none', oracleIntact: true }) })
  const r4 = await go(ARGS({ maxRounds: 1 }), repeated)
  check('re-running one command three times is not three commands',
    !!r4 && r4.lanes[0].converged === false, r4 && r4.lanes[0].converged)

  // commandsRun: [""] must not converge — every entry has to be a non-empty string.
  const blankOnly = mkAgent({
    build: build(['cargo test -p a']),
    verify: () => VERIFY({ commandsRun: [''], falseGreenRisk: 'none', oracleIntact: true }),
  })
  const r5 = await go(ARGS({ maxRounds: 1 }), blankOnly)
  check('commandsRun of a single empty string does not converge',
    !!r5 && r5.lanes[0].converged === false, r5 && r5.lanes[0].converged)

  const blankAmong = mkAgent({
    build: build(['cargo test -p a']),
    verify: () => VERIFY({ commandsRun: ['cargo test -p a', ''], falseGreenRisk: 'none', oracleIntact: true }),
  })
  const r6 = await go(ARGS({ maxRounds: 1 }), blankAmong)
  check('a blank entry among commandsRun fails closed even if a real command is present',
    !!r6 && r6.lanes[0].converged === false, r6 && r6.lanes[0].converged)

  // Omit/invalid claimed commands must not converge: empty coverage against the verifier's own
  // commands is a vacuous pass, not independent verification of a done build.
  const omitted = mkAgent({
    build: () => BUILD({ commands: [] }),
    verify: () => VERIFY({ commandsRun: ['cargo test -p x'], falseGreenRisk: 'none', oracleIntact: true }),
  })
  const r7 = await go(ARGS({ maxRounds: 1 }), omitted)
  check('a done build that omits commands does not converge',
    !!r7 && r7.lanes[0].converged === false, r7 && r7.lanes[0].converged)

  const blanksOnly = mkAgent({
    build: () => BUILD({ commands: ['', '   '] }),
    verify: () => VERIFY({ commandsRun: ['cargo test -p x'], falseGreenRisk: 'none', oracleIntact: true }),
  })
  const r8 = await go(ARGS({ maxRounds: 1 }), blanksOnly)
  check('a done build whose commands are only blanks does not converge',
    !!r8 && r8.lanes[0].converged === false, r8 && r8.lanes[0].converged)

  const missingField = mkAgent({
    build: () => {
      const b = BUILD()
      delete b.commands
      return b
    },
    verify: () => VERIFY({ commandsRun: ['cargo test -p x'], falseGreenRisk: 'none', oracleIntact: true }),
  })
  const r9 = await go(ARGS({ maxRounds: 1 }), missingField)
  check('a done build that omits the commands field does not converge',
    !!r9 && r9.lanes[0].converged === false, r9 && r9.lanes[0].converged)
}

// scout deferred a bead whose paths were ALL unusable and merely LOGGED the partial case, emitting a
// lane authorised for the work but forbidden from part of it — a failure that arrives after dispatch.
{
  const SCOUT = fs.readFileSync(path.join(HERE, 'scout.js'), 'utf8')
  check('scout defers a bead when only SOME of its paths are unusable',
    /are unusable as owned roots, so any lane would be authorised/.test(SCOUT))
  // The partial branch must CONTINUE, not fall through to placeable.push.
  const partial = SCOUT.match(/if \(rejected\) \{[\s\S]*?\n  \}/)
  check('the partial-rejection branch stops the bead being placed',
    !!partial && /continue/.test(partial[0]), partial && partial[0].slice(0, 120))

  // Unverified dependency edges must be dropped, not kept via the stored fallback.
  check('scout drops edges with no verification verdict (fail closed)',
    /if \(!v\) \{ unverifiedEdges\.push\(e\); continue \}/.test(SCOUT)
      && /UNVERIFIED EDGES ARE NOT KEPT/.test(SCOUT))
  check('scout no longer advertises fanoutArgs as feed-straight-into lane-fanout',
    /fanoutPlan:/.test(SCOUT) && /status: 'incomplete'/.test(SCOUT) && !/Feed straight into lane-fanout/.test(SCOUT))
  check('scout fanoutPlan is explicitly incomplete for lane-fanout',
    /missing tip and per-lane wt\/brief\/accept/.test(SCOUT))
  check('scout measures depth by downstream dependents (reverse edges)',
    /corrected\.filter\(\(e\) => e\.to === id\)/.test(SCOUT))
  check('scout counts rejected paths before deduplicating roots',
    /const normalised = \(item\.paths/.test(SCOUT) && /rejected = normalised\.filter/.test(SCOUT))
  check('scout rejects absolute paths outside REPO with a segment boundary',
    /r\.startsWith\(`\$\{repo\}\/`\)/.test(SCOUT) && /else return null/.test(SCOUT))
}

// backlog-audit must not treat a partial crate census as complete coverage.
{
  const AUDIT = fs.readFileSync(path.join(HERE, 'backlog-audit.js'), 'utf8')
  check('backlog-audit does not import Node fs for the crate census', !/import\(['"]node:fs['"]\)/.test(AUDIT))
  check('backlog-audit measures crates via a dedicated crate-disk-census agent',
    /label: 'crate-disk-census'/.test(AUDIT) && /find backend\/crates -name Cargo\.toml/.test(AUDIT))
  check('backlog-audit does not treat Collect.cargoTomlPaths as the disk oracle',
    /required: \['openIssueNumbers', 'openIssueCount', 'issues', 'beads', 'crates'\]/.test(AUDIT)
      && !/required: \['openIssueNumbers', 'openIssueCount', 'issues', 'beads', 'crates', 'cargoTomlPaths'\]/.test(AUDIT)
      && /diskCensus\.cargoTomlPaths/.test(AUDIT))
  const omitSrc = AUDIT.match(/function cratesOmittedFromCensus\([\s\S]*?\n\}/)
  check('backlog-audit exposes cratesOmittedFromCensus to the preflight', !!omitSrc)
  if (omitSrc) {
    const cratesOmittedFromCensus = new Function(`${omitSrc[0]}; return cratesOmittedFromCensus`)()
    check('a census that omits an on-disk crate is incomplete',
      cratesOmittedFromCensus(['identity', 'brand-new'], ['identity']).join(',') === 'brand-new')
    check('a census parent prefix still covers nested crates',
      cratesOmittedFromCensus(['identity/domain', 'identity/rest'], ['identity']).length === 0)
  }
  const deriveSrc = AUDIT.match(/function crateNamesFromCargoTomlPaths\([\s\S]*?\n\}/)
  check('backlog-audit exposes crateNamesFromCargoTomlPaths to the preflight', !!deriveSrc)
  if (deriveSrc) {
    const crateNamesFromCargoTomlPaths = new Function(`${deriveSrc[0]}; return crateNamesFromCargoTomlPaths`)()
    check('cargoTomlPaths strip to crate names under backend/crates',
      crateNamesFromCargoTomlPaths(['backend/crates/identity/Cargo.toml', './backend/crates/policy/Cargo.toml']).join(',') === 'identity,policy')
  }
}

// Contract-drift: string literals must not invent HTTP methods; OpenAPI path keys may contain ':'.
{
  const driftPath = path.join(HERE, '..', '..', '..', 'scripts', 'check-platform-contract-drift.mjs')
  const driftSrc = fs.readFileSync(driftPath, 'utf8')
  check('contract-drift masks string literals before method discovery',
    /function maskStringLiterals/.test(driftSrc) && /maskStringLiterals\(methodExpression\)/.test(driftSrc))
  check('contract-drift OpenAPI path keys allow colons inside the path',
    driftSrc.includes('trimmedRight.match(/^ {2}(\\/.+):$/)')
      || driftSrc.includes('trimmedRight.match(/^ {2}(/.+):$/)')
      || /\\\/\.+\):\$/.test(driftSrc))
  check('contract-drift refuses repo-wide same-name fallback for undeclared PATH consts',
    /refusing repo-wide same-name fallback/.test(driftSrc))
  check('contract-drift discovers route sources after stripping comments/literals',
    /stripRustCommentsAndLiterals\(readFileSync\(file, "utf8"\)\)\.includes\(\s*"\.route\("\s*\)/.test(driftSrc)
      || /stripRustCommentsAndLiterals\(readFileSync\(file, "utf8"\)\)\.includes\("\.route\("\)/.test(driftSrc))

  const maskSrc = driftSrc.match(/function maskStringLiterals\([\s\S]*?\n\}/)
  const stripSrc = driftSrc.match(/function stripRustCommentsAndLiterals\([\s\S]*?\n\}/)
  if (stripSrc) {
    const stripRustCommentsAndLiterals = new Function(`${stripSrc[0]}; return stripRustCommentsAndLiterals`)()
    const docOnly = '//! example\n/// `.route("/api/x", get(h))` in docs only\nfn unused() {}\n'
    const after = stripRustCommentsAndLiterals(docOnly)
    check('doc-comment .route( does not survive strip discovery',
      !after.includes('.route('), after)
    const real = 'fn router() { axum::Router::new().route("/api/x", get(h)) }\n'
    check('real .route( survives strip discovery',
      stripRustCommentsAndLiterals(real).includes('.route('))
  }
  if (maskSrc) {
    const maskStringLiterals = new Function(`${maskSrc[0]}; return maskStringLiterals`)()
    const methodConstructor = /\b(get|put|post|delete|options|head|patch|trace)\s*\(/g
    const expr = 'get(handler).layer(/* "documentation says get() here" */)'
    // Simulate a string that would false-positive without masking:
    const withProse = 'get(handler_with_doc("documentation says get() here"))'
    const rawHits = [...withProse.matchAll(methodConstructor)].map((m) => m[1])
    const maskedHits = [...maskStringLiterals(withProse).matchAll(methodConstructor)].map((m) => m[1])
    check('prose get() inside a string is a raw false-positive before masking',
      rawHits.includes('get') && rawHits.length >= 2, rawHits)
    check('masking string literals drops the prose get() false-positive',
      maskedHits.length === 1 && maskedHits[0] === 'get', maskedHits)

    const openApiSrc = driftSrc.match(/function openApiApiOperations\([\s\S]*?\n\}/)
    check('openApiApiOperations is extractable', !!openApiSrc)
    if (openApiSrc) {
      const helpers = driftSrc.match(/function operationKey\([\s\S]*?\n\}\n\nfunction normalizePathParameters\([\s\S]*?\n\}/)
      const openApiApiOperations = new Function(
        `const httpMethodSet = new Set(['get','put','post','delete','options','head','patch','trace']);\n`
        + `${helpers ? helpers[0] : 'function operationKey(m,p){return m.toUpperCase()+\" \"+p} function normalizePathParameters(p){return p}'};\n`
        + `${openApiSrc[0]}; return openApiApiOperations`,
      )()
      const ops = openApiApiOperations([
        'paths:',
        '  /api/jobs:run:',
        '    post:',
        '      summary: run',
        '  /api/plain:',
        '    get:',
      ].join('\n'))
      check('OpenAPI path keys with a colon are accepted',
        ops.has('POST /api/jobs:run') && ops.has('GET /api/plain'), [...ops])
    }
  }
}

console.log(failures ? `\n${failures} FAILURE(S) — do not dispatch` : '\nALL PASS — safe to dispatch')
process.exit(failures ? 1 : 0)
