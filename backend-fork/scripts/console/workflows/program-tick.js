export const meta = {
  name: 'program-tick',
  description: 'Survey the program before doing any work: collect live state mechanically, classify it deterministically in-script, and use agents only where judgement is genuinely required — then dispose of finished PRs and hand the chosen lanes to lane-fanout',
  whenToUse: 'At the start of any working session or phase, INSTEAD of hand-picking lanes. Answers "what should be worked on right now, and what is already sitting somewhere unfinished" before a single implementer is spawned.',
  phases: [
    { title: 'Collect', detail: 'one mechanical agent: raw git/gh/bd output, verbatim, no interpretation' },
    { title: 'Judge', detail: 'agents only for what cannot be computed: is the work still needed, is the brief real' },
    { title: 'Disposition', detail: 'act on finished PRs by their CURRENT state; never poll' },
  ],
}

// ---------------------------------------------------------------------------
// DESIGN NOTE — why the split is where it is.
//
// Workflow scripts have no filesystem and no shell: only agent(), parallel(), phase(), log(). So
// running `git`/`gh`/`bd` MUST go through an agent. What must NOT go through an agent is the
// reasoning over that output. Set differences, graph reachability, path-collision detection and
// lane selection are total functions of the collected facts — computing them in an LLM adds a
// hallucination surface to arithmetic that cannot be wrong in JS.
//
//   agent  -> collection (forced: no fs access) and judgement (genuinely interpretive)
//   script -> every classification that is a function of the collected facts
//
// args = {
//   candidateWt, candidateTip, base, authority?, maxLanes?=4, fanout?=false, workspace?
//
// maxLanes is a HARD CAP, not advice to the judge: a selection larger than it refuses the whole
// fan-out rather than dispatching past it or silently truncating.
//
// workspace defaults to the directory candidateWt itself lives in. Lane worktrees are RESOLVED
// against the worktree inventory collected below, never constructed from a literal — see the
// fanout chain at the bottom of this file.
// }
// ---------------------------------------------------------------------------

let ARGS = args
if (typeof ARGS === 'string') {
  try { ARGS = JSON.parse(ARGS) } catch (e) { throw new Error(`program-tick: args is not valid JSON: ${e.message}`) }
}
ARGS = ARGS || {}

// An option this workflow does not read must abort rather than be silently dropped. This guard was
// written for lane-fanout, repeated in backlog-audit, and never applied here — and a rule living in
// two of three sibling files is the signal that it belongs to the shape, not to the file. In a
// sibling runner the same defect (an option accepted, ignored, and the run looking entirely normal)
// cost six lanes. `fanout` and `workspace` are read at the bottom of this file; both are listed.
const KNOWN_ARGS = ['candidateWt', 'candidateTip', 'base', 'authority', 'maxLanes', 'fanout', 'workspace']
{
  const unknown = Object.keys(ARGS).filter((k) => !KNOWN_ARGS.includes(k))
  if (unknown.length) {
    throw new Error(`program-tick: unknown option(s) ${unknown.join(', ')}. Known: ${KNOWN_ARGS.join(', ')}.`)
  }
}

const CAND_WT = ARGS.candidateWt
const CAND_TIP = ARGS.candidateTip
const BASE = ARGS.base
const MAX_LANES = ARGS.maxLanes || 4
const AUTHORITY = ARGS.authority || ''

for (const [k, v] of [['candidateWt', CAND_WT], ['candidateTip', CAND_TIP], ['base', BASE]]) {
  if (!v) throw new Error(`program-tick: args.${k} is required`)
}

const READ_ONLY = `
=== READ-ONLY. NON-NEGOTIABLE. ===
Run ONLY: git status/log/diff/rev-parse/ls-files/worktree list, gh pr list/view/checks, bd list/show/ready/blocked/dep.
NEVER: stash, reset, checkout <branch>, rebase, merge, clean, push, worktree add|remove, bd close/update,
gh pr merge/close/edit. Do not edit a single file. You are reading, not deciding and not acting.
DO NOT POLL: read each PR's current check conclusion ONCE. Never sleep, never wait for a run.
`

// --- COLLECT: raw facts only. No judgement, no classification, no opinion. -------------------
const RAW_SCHEMA = {
  type: 'object',
  required: ['candidateFiles', 'worktrees', 'prs', 'beads'],
  properties: {
    candidateFiles: { type: 'array', items: { type: 'string' }, description: `verbatim output of: git -C ${CAND_WT} diff --name-only ${BASE}..${CAND_TIP}` },
    worktrees: {
      type: 'array',
      items: {
        type: 'object',
        required: ['path', 'head', 'dirtyCount', 'prunable', 'filesVsBase', 'filesVsBaseCount', 'commitsAheadOfCandidate'],
        properties: {
          path: { type: 'string' },
          head: { type: 'string' },
          branch: { type: 'string' },
          dirtyCount: { type: 'number', description: 'lines of git status --porcelain' },
          prunable: { type: 'boolean', description: 'worktree list --porcelain marked it prunable' },
          filesVsBase: { type: 'array', items: { type: 'string' }, description: `git -C <wt> diff --name-only ${BASE} — empty array if none or unreadable` },
          // Without the count, an empty filesVsBase cannot be told apart from a capped one, and
          // "we did not look" reads exactly like "there is nothing there".
          filesVsBaseCount: { type: 'number', description: 'the @@N value: how many files differ from base IN TOTAL, reported even when filesVsBase was capped or unreadable; -1 if the command failed' },
          commitsAheadOfCandidate: { type: 'number', description: `git -C <wt> rev-list --count ${CAND_TIP}..HEAD, or -1 if unreadable` },
        },
      },
    },
    prs: {
      type: 'array',
      items: {
        type: 'object',
        required: ['number', 'title', 'checkConclusion', 'reviewDecision', 'mergeable', 'headSha'],
        properties: {
          number: { type: 'number' },
          title: { type: 'string' },
          checkConclusion: { type: 'string', description: 'SUCCESS / FAILURE / PENDING / NONE — the rollup, read once' },
          reviewDecision: { type: 'string' },
          mergeable: { type: 'string' },
          mergeStateStatus: { type: 'string' },
          headSha: { type: 'string' },
          failingChecks: { type: 'array', items: { type: 'string' } },
          isDraft: { type: 'boolean' },
        },
      },
    },
    beads: {
      type: 'array',
      items: {
        type: 'object',
        required: ['id', 'title', 'status', 'blockedByOpen'],
        properties: {
          id: { type: 'string' },
          title: { type: 'string' },
          status: { type: 'string' },
          priority: { type: 'number' },
          blockedByOpen: { type: 'array', items: { type: 'string' }, description: 'ids of OPEN blockers only, from bd show; empty array if none' },
          blocksCount: { type: 'number', description: 'how many issues this one blocks — 0 if none' },
        },
      },
    },
  },
}

phase('Collect')

const raw = await agent(`Collect raw program state. You are a COLLECTOR: report exactly what the commands
print. Do not classify, do not interpret, do not decide anything, do not summarise. Another stage
does all of that; your only failure mode that matters is reporting something you did not observe.

  cd ${CAND_WT}

1. CANDIDATE FILE SET
   git --no-pager diff --name-only ${BASE}..${CAND_TIP}

2. WORKTREES — there may be a HUNDRED of them. Do NOT run four separate commands per worktree:
   that is hundreds of shell round-trips and it is the slowest thing in this survey by far. Run ONE
   batched loop and parse its output:

     git worktree list --porcelain | awk '/^worktree /{print $2}' | while read -r w; do
       printf '@@WT\\t%s\\t%s\\t%s\\t%s\\n' \\
         "$w" \\
         "$(git -C "$w" rev-parse --short HEAD 2>/dev/null || echo MISSING)" \\
         "$(git -C "$w" status --porcelain 2>/dev/null | wc -l | tr -d ' ')" \\
         "$(git -C "$w" rev-list --count ${CAND_TIP}..HEAD 2>/dev/null || echo -1)"
       n=$(git -C "$w" --no-pager diff --name-only ${BASE} 2>/dev/null | wc -l | tr -d ' ')
       printf '@@N\\t%s\\n' "$n"
       [ "$n" -le 60 ] && git -C "$w" --no-pager diff --name-only ${BASE} 2>/dev/null | sed 's/^/@@F\\t/'
     done

   One pass, one round-trip per worktree instead of four. Measured on this repository: 89 worktrees
   in ~5 seconds, against many minutes for the per-worktree form.

   The 60-file cap is deliberate. A worktree differing from base by thousands of files is an old
   branch, not a lane: its file list is worthless to the classification and would swamp your
   output (unfiltered, this repository emits 25,000+ lines). Report its @@N count with an empty
   filesVsBase and let the count speak. A LANE worktree is small by construction.

   ALWAYS report @@N as filesVsBaseCount, capped or not. An empty filesVsBase with no count cannot
   be told apart from a worktree that genuinely holds nothing, and the classification below would
   then offer a capped worktree for removal.

   A worktree whose HEAD prints MISSING has no gitdir: set prunable true, filesVsBase [],
   filesVsBaseCount -1 and commitsAheadOfCandidate -1. Do NOT skip it and do NOT guess its contents.

3. PULL REQUESTS
     gh pr list --state open --json number,title,mergeable,reviewDecision,isDraft,headRefOid
   For each, ONCE:
     gh pr view <n> --json statusCheckRollup,mergeStateStatus
   Reduce statusCheckRollup to one word — SUCCESS / FAILURE / PENDING / NONE — and list the names
   of any failing checks. If there are zero open PRs, return an empty array. That is a complete
   answer, not a failure.

4. BEADS
     bd list --status=open --json   (fall back to plain output if --json is unsupported)
     bd blocked
   For each open bead record its id, title, status, priority, the ids of its OPEN blockers only,
   and how many issues it blocks.

Report numbers you actually saw. If a command fails, report the failure in that record rather than
inventing a plausible value.
${READ_ONLY}`, { label: 'collect', phase: 'Collect', schema: RAW_SCHEMA })

// --- CLASSIFY: pure functions of the collected facts. No agent involved. ----------------------
const candSet = new Set(raw.candidateFiles || [])
const wts = raw.worktrees || []

const classifyWorktree = (w) => {
  if (w.prunable) return 'prunable'
  const files = w.filesVsBase || []
  const ahead = typeof w.commitsAheadOfCandidate === 'number' ? w.commitsAheadOfCandidate : -1
  // UNREADABLE IS NOT UNUSED, and it used to be sorted as `empty` — i.e. offered for removal. Two
  // ways the inventory fails to describe a worktree, and both land here: `git rev-list` failed and
  // the collector reported -1, and the deliberate 60-file cap, which reports the COUNT and an empty
  // file list. In both cases the evidence that the worktree holds nothing is exactly what is
  // missing, and removal is the one irreversible action in this plan.
  const count = typeof w.filesVsBaseCount === 'number' ? w.filesVsBaseCount : files.length
  if (ahead < 0 || count > files.length) return 'unreadable'
  const unmerged = files.filter((f) => !candSet.has(f))
  // At risk if it carries commits the candidate lacks, or contributes files the candidate lacks,
  // or has uncommitted edits. Anything else contributes nothing that is not already captured.
  if (ahead > 0 || unmerged.length > 0) return 'atRisk'
  if ((w.dirtyCount || 0) > 0) return 'dirty'
  if (files.length === 0) return 'empty'
  return 'integrated'
}

const buckets = { integrated: [], atRisk: [], prunable: [], dirty: [], empty: [], unreadable: [] }
for (const w of wts) buckets[classifyWorktree(w)].push(w)

// Duplicate work: the same non-candidate file contributed by more than one worktree.
const contributors = new Map()
for (const w of wts) {
  for (const f of (w.filesVsBase || [])) {
    if (candSet.has(f)) continue
    if (!contributors.has(f)) contributors.set(f, [])
    contributors.get(f).push(w.path)
  }
}
const duplicated = [...contributors.entries()].filter(([, ps]) => ps.length > 1)
  .map(([file, paths]) => ({ file, paths }))

// Bead readiness is a graph fact, not a judgement call.
const beads = raw.beads || []
const openIds = new Set(beads.filter((b) => b.status === 'open' || b.status === 'in_progress').map((b) => b.id))
const unblocked = beads.filter((b) => b.status === 'open' && !(b.blockedByOpen || []).some((d) => openIds.has(d)))
const blocked = beads.filter((b) => b.status === 'open' && (b.blockedByOpen || []).some((d) => openIds.has(d)))
// Rank by what unblocks the most, then by priority.
const ranked = [...unblocked].sort((a, b) => (b.blocksCount || 0) - (a.blocksCount || 0) || (a.priority ?? 9) - (b.priority ?? 9))

// PR disposition is a decision table over the rollup, not an opinion.
const prAction = (p) => {
  if (p.isDraft) return 'report-only'
  if (p.checkConclusion === 'PENDING') return 'in-flight'
  if (p.checkConclusion === 'FAILURE') return 'fix-then-merge'
  if (p.mergeStateStatus === 'BEHIND') return 'rebase-then-merge'
  if (p.checkConclusion === 'SUCCESS' && p.reviewDecision !== 'APPROVED') return 'needs-review'
  if (p.checkConclusion === 'SUCCESS' && p.mergeable === 'MERGEABLE') return 'merge'
  return 'report-only'
}
const prPlan = (raw.prs || []).map((p) => ({
  pr: p.number, title: p.title, action: prAction(p),
  checkConclusion: p.checkConclusion, reviewDecision: p.reviewDecision,
  failingChecks: p.failingChecks || [],
}))

log(`collected: ${wts.length} worktrees, ${(raw.prs || []).length} open PRs, ${beads.length} open beads`)
log(`worktrees -> integrated ${buckets.integrated.length}, atRisk ${buckets.atRisk.length}, prunable ${buckets.prunable.length}, dirty ${buckets.dirty.length}, empty ${buckets.empty.length}, unreadable ${buckets.unreadable.length}`)
log(`beads -> ${unblocked.length} unblocked, ${blocked.length} blocked${duplicated.length ? `; ${duplicated.length} file(s) contributed by more than one worktree` : ''}`)

// --- JUDGE: only what cannot be computed. -----------------------------------------------------
const JUDGE_SCHEMA = {
  type: 'object',
  required: ['startNow', 'holdBack', 'alreadyDone', 'coverageRisks'],
  properties: {
    startNow: {
      type: 'array',
      items: {
        type: 'object',
        required: ['key', 'bead', 'owned', 'brief', 'accept', 'briefConfidence'],
        properties: {
          key: { type: 'string' }, bead: { type: 'string' }, owned: { type: 'string' },
          brief: { type: 'string' }, accept: { type: 'string' },
          briefConfidence: { type: 'string', enum: ['grounded', 'thin'] },
          structurallyNeedsLeasedFile: { type: 'string', description: 'a leased path the deliverable cannot avoid (workspace manifest for a new crate, CI wiring for a new test binary), or "none"' },
        },
      },
    },
    holdBack: { type: 'array', items: { type: 'object', required: ['bead', 'why'], properties: { bead: { type: 'string' }, why: { type: 'string' } } } },
    alreadyDone: { type: 'array', items: { type: 'object', required: ['bead', 'proof'], properties: { bead: { type: 'string' }, proof: { type: 'string' } } } },
    coverageRisks: { type: 'array', items: { type: 'string' } },
  },
}

phase('Judge')

const judged = await agent(`Decide what to actually work on. The mechanical classification is DONE and is not
yours to redo — trust the numbers below and spend your effort only on what arithmetic cannot answer.

ALREADY COMPUTED (do not recompute):
  unblocked beads, ranked by how much they unblock:
${ranked.map((b) => `    ${b.id} [P${b.priority ?? '?'}] blocks:${b.blocksCount ?? 0} — ${b.title}`).join('\n') || '    (none)'}
  blocked beads:
${blocked.map((b) => `    ${b.id} — waiting on ${(b.blockedByOpen || []).join(', ')}`).join('\n') || '    (none)'}
  worktrees: integrated ${buckets.integrated.length}, atRisk ${buckets.atRisk.length}, prunable ${buckets.prunable.length}, dirty ${buckets.dirty.length}, unreadable ${buckets.unreadable.length}
${buckets.atRisk.length ? `  AT RISK (hold work nowhere else — never propose removing these):\n${buckets.atRisk.map((w) => `    ${w.path} @ ${w.head} (+${w.commitsAheadOfCandidate} commits)`).join('\n')}` : ''}
${duplicated.length ? `  DUPLICATED across worktrees:\n${duplicated.slice(0, 10).map((d) => `    ${d.file} <- ${d.paths.join(', ')}`).join('\n')}` : ''}

  cd ${CAND_WT}
${AUTHORITY ? `  Authority for scope and non-goals: ${AUTHORITY}` : ''}

YOUR JOB — three judgements, each needing evidence a computation cannot supply:

1. IS THE WORK STILL NEEDED? For each unblocked bead, CHECK THE CODE before calling it runnable:
   grep for the artefact it would create, read the gate it would satisfy, run the check it would
   fix. A bead tracking work that already landed is a recurring failure here — list it under
   alreadyDone WITH THE PROOF, not under startNow.

2. WHAT SHOULD ACTUALLY START, at most ${MAX_LANES} — a hard cap: returning more refuses the whole
   fan-out and nothing starts. Choose for PATH DISJOINTNESS FIRST: two lanes sharing a writable
   path must never both start however ready they look, and the fan-out refuses them anyway.
   Prefer lanes that unblock the most. For each, write a brief citing CONCRETE files, line numbers
   and the governing authority clause — a brief that restates the bead title is useless, mark it briefConfidence
   "thin". Name any leased file the deliverable structurally cannot avoid (a new crate needs the
   workspace manifest; a new test binary needs CI wiring) under structurallyNeedsLeasedFile, and
   instruct the lane to REPORT it rather than stall or fake it. Omitting that is how a lane gets
   authorised to do something while forbidden the only way to do it.

3. COVERAGE RISKS. Anything landed or about to land WITHOUT a test that can fail, or a claimed
   invariant with no oracle. This program has repeatedly shipped guards that could not detect their
   own violation: a non-exhaustive matches!, a route check reading a hand-maintained list, a $ref
   validator that truncated its input, a preflight test that never called preflight. Hunt that shape.

Be honest when little is runnable. A thin frontier stated plainly beats lanes invented to look busy.
${READ_ONLY}`, { label: 'judge', phase: 'Judge', schema: JUDGE_SCHEMA })

const thin = (judged.startNow || []).filter((l) => l.briefConfidence === 'thin').map((l) => l.key)
if (thin.length) log(`briefs too thin to run unreviewed: ${thin.join(', ')}`)
if ((judged.alreadyDone || []).length) log(`beads tracking work that already landed: ${judged.alreadyDone.length}`)

// --- DISPOSITION: act on PRs by current state. ------------------------------------------------
phase('Disposition')

const actionable = prPlan.filter((p) => ['merge', 'fix-then-merge', 'rebase-then-merge'].includes(p.action))

// EVERY DISPOSITION RUNS IN THE SAME CANDIDATE WORKTREE, AND fix-then-merge IS WORK. Two of
// them through parallel() is two writers in one root: exactly the collision lane-fanout refuses at
// dispatch for lanes, arriving here by a different door because PR dispositions are not lanes.
// program-tick is the CALLER that builds this set, so refusing downstream would be late — the
// agents are already chosen. Serialised, not partitioned by action: any of these agents may touch
// the tree, and a rule that has to guess which ones is a rule with a next spelling. Depth costs
// wall-clock and a tick disposes of a handful of PRs, which is the cheap side of the trade.
const disposePr = (p) => agent(`Dispose of PR #${p.pr} ("${p.title}") in ${CAND_WT}.

DECIDED ACTION: ${p.action}
Current rollup: ${p.checkConclusion}; review: ${p.reviewDecision}${p.failingChecks.length ? `; failing: ${p.failingChecks.join(', ')}` : ''}

 - merge: verify ONCE that every required context is green, review is satisfied, and the head is
   still the reviewed head. Then squash merge. If ANY required context is not green, or the head
   moved since review, STOP and report — do not merge.
 - fix-then-merge: this is WORK, not a watchlist item. Diagnose the failing checks and fix the
   CAUSE. Do not re-run hoping for a different answer. Do not disable, skip or weaken the failing
   check. If the fix lies outside this PR's scope, report exactly what is needed and stop.
 - rebase-then-merge: you may NOT rebase or force-push. Report precisely what the owner must do.

*** You may not relax branch protection, bypass a required check, skip a test, or merge anything
whose signed authority train is not intact. If the right action needs any of that, report it. ***
Do not poll: act on the state above, once.`,
  { label: `pr:${p.pr}`, phase: 'Disposition', schema: { type: 'object', required: ['pr', 'done', 'outcome'], properties: { pr: { type: 'number' }, done: { type: 'boolean' }, outcome: { type: 'string' }, blockedBy: { type: 'string' } } } })

const prResults = []
for (const p of actionable) prResults.push(await disposePr(p))

if (!actionable.length) log(`no PR needed action this tick (${prPlan.length} open)`)

const plan = {
  summary: `${ranked.length} unblocked, ${(judged.startNow || []).length} to start, ${buckets.atRisk.length} worktrees at risk, ${prPlan.length} open PRs`,
  startNow: judged.startNow || [],
  holdBack: judged.holdBack || [],
  alreadyDone: judged.alreadyDone || [],
  coverageRisks: judged.coverageRisks || [],
  worktrees: {
    counts: Object.fromEntries(Object.entries(buckets).map(([k, v]) => [k, v.length])),
    // NEVER the worktree this tick is operating IN. It classifies as integrated by construction —
    // its diff against base IS the candidate file set — so it sat at the head of a list titled
    // "safe to remove", one copy-paste away from deleting the checkout doing the work.
    safeToRemove: buckets.integrated.concat(buckets.empty)
      .map((w) => w.path).filter((p) => String(p).replace(/\/+$/, '') !== CAND_WT.replace(/\/+$/, '')),
    mustNotRemove: buckets.atRisk.map((w) => ({ path: w.path, head: w.head, aheadBy: w.commitsAheadOfCandidate })),
    // Not removable and not at risk either: we simply cannot see what they hold. Reported so the
    // gap is visible rather than resolved by a silent default in either direction.
    unreadable: buckets.unreadable.map((w) => w.path),
    prunableRegistrations: buckets.prunable.map((w) => w.path),
    duplicated,
  },
  prPlan,
  prResults,
}

// A lane worktree must be one this tick OBSERVED, never one it composed. This used to send every
// selected lane to a literal absolute path under one developer's home directory: a path that exists
// on exactly one machine, that nothing in this workflow creates, and that on CI, on Linux, or in any
// other checkout pointed every implementer at nothing at all. (The preflight now forbids the
// spelling as well as the instance.) Workflow scripts have no filesystem, so the
// only evidence a path exists is the inventory the collector just read — resolve against that, and
// refuse to spawn anything if a lane has no single unambiguous worktree.
const WORKSPACE = (ARGS.workspace || CAND_WT.replace(/\/+$/, '').replace(/\/[^/]*$/, '')).replace(/\/+$/, '')
const resolveLaneWt = (key) => {
  const hits = wts.filter((w) => !w.prunable && (w.path === `${WORKSPACE}/${key}` || w.path.endsWith(`-${key}`)))
  return { hits: hits.map((w) => w.path) }
}

// Chaining is opt-in and refuses to spawn implementers off a brief nobody has read.
if (ARGS.fanout && (judged.startNow || []).length && !thin.length) {
  const resolved = judged.startNow.map((l) => ({ lane: l, ...resolveLaneWt(l.key) }))
  const unusable = resolved.filter((r) => r.hits.length !== 1).map((r) => ({
    key: r.lane.key,
    why: r.hits.length ? `ambiguous: ${r.hits.join(', ')}` : `no worktree under ${WORKSPACE} matches lane "${r.lane.key}"`,
  }))
  // maxLanes lived ONLY in the judge's prompt, and a prompt is a request. The judge is free to
  // return more, and everything it returned was then fanned out — the cap was read, printed, and
  // enforced by nobody. Enforced HERE because here is the last point before implementers exist.
  const overCap = judged.startNow.length > MAX_LANES
    ? [{ key: '(all)', why: `${judged.startNow.length} lanes selected but maxLanes=${MAX_LANES}. Raise maxLanes deliberately, or narrow the selection — do not dispatch past a cap the caller set.` }]
    : []
  const blocked = [...overCap, ...unusable]
  if (blocked.length) {
    // Refuse the WHOLE fanout, not the offending lanes only: chaining the remaining subset
    // would silently drop selected work, which is the same defect in a different spelling.
    plan.fanoutBlocked = blocked
    log(`FANOUT REFUSED — no implementer dispatched. ${plan.fanoutBlocked.map((b) => `${b.key}: ${b.why}`).join(' | ')}`)
    if (unusable.length) log(`create the missing worktree(s) from ${CAND_TIP} under ${WORKSPACE}, or pass args.workspace, then re-run with fanout`)
  } else {
    log(`chaining into lane-fanout with ${resolved.length} lane(s): ${resolved.map((r) => r.hits[0]).join(', ')}`)
    plan.fanoutResult = await workflow('lane-fanout', {
      tip: CAND_TIP,
      lanes: resolved.map(({ lane: l, hits }) => ({
        key: l.key, bead: l.bead, owned: l.owned, brief: l.brief, accept: l.accept, wt: hits[0],
      })),
    })
  }
} else if (ARGS.fanout && thin.length) {
  log('fanout requested but some briefs are thin — returning the plan for enrichment rather than spawning implementers')
}

return plan
