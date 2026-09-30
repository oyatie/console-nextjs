export const meta = {
  name: 'scout',
  description: 'Read-only: census the backlog, verify its dependency edges by re-deriving them, compute the critical path in-script, and emit an incomplete fan-out plan (beads + owned roots) whose lanes provably cannot collide',
  whenToUse: 'Before a fan-out, when you need to know WHAT to parallelise and in WHICH order. Produces bead/root proposals for lane-fanout; callers must still supply tip/wt/brief/accept. Never edits code, never opens a PR.',
  phases: [
    { title: 'Census', detail: 'one agent gathers raw tracker + repo state' },
    { title: 'Verify', detail: 'fan out: is each ready item real, and is each dependency edge real?' },
    { title: 'Plan', detail: 'critical path computed in-script, lanes proved disjoint' },
  ],
}

// ---------------------------------------------------------------------------
// args = { repo, maxLanes?=4, integrationBranch?, focus? }
//
// WHY THIS IS NOT PART OF lane-fanout: that harness answers "is this change correct".
// This one answers "what should we work on, in what order, and what can run at once".
// Different subject, different output, and -- decisively -- this one must be able to
// run when there is NO candidate tip and NO worktree, which lane-fanout cannot.
//
// WHY THE ORDERING IS COMPUTED HERE AND NOT BY AN AGENT: a critical path is a
// deterministic function of a graph. Asking a model to "figure out the order" makes
// the answer unreproducible and unauditable, and this programme has already paid for
// a dependency graph that was silently REVERSED -- `bd dep add A B` and
// `bd dep A --blocks B` are inverse forms, the wrong one inverts the whole graph, and
// `bd dep cycles` stays green either way. So: agents report OBSERVATIONS, the script
// computes CONCLUSIONS.
// ---------------------------------------------------------------------------

let ARGS = args
if (typeof ARGS === 'string') {
  try { ARGS = JSON.parse(ARGS) } catch (e) {
    throw new Error(`scout: args arrived as a string that is not valid JSON: ${e.message}`)
  }
}
ARGS = ARGS || {}

const KNOWN_ARGS = ['repo', 'maxLanes', 'integrationBranch', 'focus', 'batch']
{
  const unknown = Object.keys(ARGS).filter((k) => !KNOWN_ARGS.includes(k))
  if (unknown.length) {
    throw new Error(`scout: unknown option(s) ${unknown.join(', ')}. Known: ${KNOWN_ARGS.join(', ')}.`)
  }
}

const REPO = ARGS.repo
const MAX_LANES = ARGS.maxLanes || 4
const FOCUS = ARGS.focus || ''
// FAN-OUT MUST NOT SCALE WITH BACKLOG SIZE. The first run dispatched one agent per edge and one per
// ready bead: 57 + 74 = 131 agents against a 92-bead tracker, and 117 of them died when the account
// limit was reached mid-run -- so the run cost a full quota and returned a plan with zero lanes.
// Width is also nearly free in wall-clock terms while DEPTH is what costs (12 agents at depth 6 took
// 74 minutes here; 36 at the same depth took 63), so a swarm buys nothing and risks everything.
// Work is batched instead: agent count is bounded by ceil(n / BATCH) and the depth is unchanged.
const BATCH = ARGS.batch === undefined ? 8 : ARGS.batch
if (!Number.isInteger(BATCH) || BATCH < 1) {
  throw new Error(`scout: batch must be a positive integer; got ${JSON.stringify(ARGS.batch)}`)
}
const chunk = (xs, n) => xs.reduce((a, x, i) => (i % n ? a[a.length - 1].push(x) : a.push([x]), a), [])
if (!REPO) throw new Error('scout: args.repo is required')
if (!Number.isInteger(MAX_LANES) || MAX_LANES < 1) {
  throw new Error(`scout: maxLanes must be a positive integer; got ${JSON.stringify(ARGS.maxLanes)}`)
}

const RULES = [
  '=== READ-ONLY. ABSOLUTELY. ===',
  'You may not edit a file, create or close a bead, comment on an issue, commit, push, or open a PR.',
  'You are deciding what OTHER lanes will do. A scout that changes the ground it is surveying makes',
  'every measurement after it wrong.',
  '',
  '=== REPORT OBSERVATIONS, NOT CONCLUSIONS ===',
  'Do not rank, order, or decide what is on the critical path. That is computed from what you report.',
  'Your job is to make each observation TRUE; the ordering is arithmetic over the set of them.',
  '',
  '=== THE FAILURES THIS PHASE EXISTS TO CATCH ===',
  '1. A DEPENDENCY EDGE POINTING THE WRONG WAY. `bd dep add A B` means "A depends on B";',
  '   `bd dep A --blocks B` is the INVERSE. Using the wrong one reverses the graph and',
  '   `bd dep cycles` still reports clean. Never trust the stored direction: re-derive it from the',
  '   WORK. Read both beads and answer in plain language which one cannot start until the other is',
  '   done, then say whether the stored edge agrees. A disagreement is a finding, not a footnote.',
  '2. A BEAD THAT IS ALREADY DONE. The tracker lags the tree. Before calling anything ready, check',
  '   whether the code it asks for already exists on the default branch.',
  '3. PARENT-CHILD READ AS BLOCKING. `bd blocked` counts parent-child edges as blocking, which makes',
  '   epics look like hard dependencies and hides work that is actually startable. Say which kind',
  '   each edge is.',
  '4. A BEAD WHOSE OWNED ROOT DOES NOT CONTAIN WHAT IT NEEDS. Authorising a goal while forbidding the',
  '   file it structurally needs has cost this programme four rounds. Name every path the work must',
  '   touch, including the tests that encode the OLD behaviour a change would have to update, and',
  '   including generated peripherals (the documentation manifest pins doc blob OIDs, so any docs/**',
  '   edit drags it in).',
].join('\n')

// --- Census ----------------------------------------------------------------
phase('Census')

const CENSUS_SCHEMA = {
  type: 'object',
  required: ['readyBeads', 'blockedBeads', 'edges', 'openPrCount', 'integrationBranches'],
  properties: {
    readyBeads: {
      type: 'array',
      description: 'beads with no unmet dependency, whatever the tracker says about status',
      items: {
        type: 'object',
        required: ['id', 'title', 'priority', 'paths'],
        properties: {
          id: { type: 'string' },
          title: { type: 'string' },
          priority: { type: 'number', description: '0 = highest' },
          paths: { type: 'array', items: { type: 'string' }, description: 'every path the work must touch — the owned root is derived from this, so an omission blocks a lane later' },
          alreadyDone: { type: 'boolean', description: 'TRUE if the tree already satisfies it — with evidence' },
          evidence: { type: 'string' },
        },
      },
    },
    blockedBeads: {
      type: 'array',
      items: {
        type: 'object',
        required: ['id', 'title', 'blockedBy'],
        properties: {
          id: { type: 'string' },
          title: { type: 'string' },
          blockedBy: { type: 'array', items: { type: 'string' } },
        },
      },
    },
    edges: {
      type: 'array',
      description: 'every dependency edge, as STORED. Direction is verified later, not here.',
      items: {
        type: 'object',
        required: ['from', 'to', 'kind'],
        properties: {
          from: { type: 'string', description: 'the dependent — cannot start until `to` is done' },
          to: { type: 'string' },
          kind: { type: 'string', enum: ['blocks', 'parent-child', 'related', 'unknown'] },
        },
      },
    },
    openPrCount: { type: 'number' },
    integrationBranches: { type: 'array', items: { type: 'string' } },
    trackerDrift: { type: 'string', description: 'where beads and GitHub issues disagree' },
  },
}

const census = await agent(
  `Census the backlog and the repository state. REPO: ${REPO}
${FOCUS ? `FOCUS: ${FOCUS}\n` : ''}
${RULES}

Gather, and report exactly what you observed:

1. BEADS. Every open bead: id, title, priority, and the dependency edges it participates in.
   Use \`bd list\`, \`bd show\`, \`bd dep\` and \`bd blocked\`. Report edges AS STORED — do not correct
   them here, a later phase re-derives direction independently and comparing the two is the point.
   For each edge say which KIND it is; parent-child and blocks are counted the same by \`bd blocked\`
   and they are not the same thing.

2. FOR EACH BEAD THAT LOOKS READY, the PATHS the work must touch. Be exhaustive and concrete:
   source files, the tests that encode the behaviour being changed, migrations, generated artifacts.
   This list becomes a lane's owned root, and a lane whose root omits a file it structurally needs
   cannot finish. Over-report rather than under-report, but do not list a whole crate when one
   module is meant.

3. ALREADY DONE? For each ready bead, check whether the default branch already satisfies it. The
   tracker lags the tree here. Say so with file:line evidence, and do NOT close anything.

4. REPO STATE. Count open PRs. List branches whose name suggests they are integration branches.
   Report how many worktrees exist. This measures whether work is piling up unlanded.

Report only what you ran and saw.`,
  { schema: CENSUS_SCHEMA, label: 'census' },
)

if (!census) throw new Error('scout: the census agent died — nothing can be planned from a null census')

const ready = (census.readyBeads || []).filter((b) => b && b.id)
const edges = (census.edges || []).filter((e) => e && e.from && e.to)
if (!ready.length) {
  log('scout: the census found NO ready beads. That is a legitimate state (everything blocked or done),')
  log('but it is also what a census that failed to read the tracker looks like. Check its coverage note.')
}
log(`census: ${ready.length} ready, ${(census.blockedBeads || []).length} blocked, ${edges.length} edges, ${census.openPrCount} open PRs`)

// --- Verify ----------------------------------------------------------------
// Two independent questions, fanned out together because neither needs the other's answer.
phase('Verify')

const EDGE_VERDICT = {
  type: 'object',
  required: ['verdicts'],
  properties: { verdicts: { type: 'array', items: { type: 'object',
    required: ['from', 'to', 'storedDirectionIsCorrect', 'reasoning'],
    properties: {
    from: { type: 'string' },
    to: { type: 'string' },
    storedDirectionIsCorrect: { type: 'boolean' },
    reasoning: { type: 'string', description: 'which work cannot start until which other work is done, in plain language, derived from reading BOTH beads' },
    isRealDependency: { type: 'boolean', description: 'FALSE if they merely touch nearby code — that is a scheduling hint, not a dependency' },
  } } } },
}

const READY_VERDICT = {
  type: 'object',
  required: ['verdicts'],
  properties: { verdicts: { type: 'array', items: { type: 'object',
    required: ['id', 'isReady', 'paths', 'reasoning'],
    properties: {
    id: { type: 'string' },
    isReady: { type: 'boolean' },
    alreadyDone: { type: 'boolean' },
    paths: { type: 'array', items: { type: 'string' } },
    sizeHint: { type: 'string', enum: ['small', 'medium', 'large', 'unknown'] },
    reasoning: { type: 'string' },
  } } } },
}

const [edgeVerdicts, readyVerdicts] = await Promise.all([
  parallel(chunk(edges, BATCH).map((batch) => () =>
    agent(
      `Verify ${batch.length} dependency edge(s) by re-deriving each from the work itself. REPO: ${REPO}

STORED EDGES:
${batch.map((e) => `  ${e.from} depends on ${e.to}   (kind: ${e.kind})`).join('\n')}

Return one verdict per edge, in the same order. Do not merge or skip any.

${RULES}

Read BOTH beads and the code each concerns. Then answer, in plain language, which one genuinely
cannot start until the other is finished — deriving it from the work, NOT from the stored edge.
Only then compare your answer to the stored direction.

This exists because the two \`bd dep\` forms are inverses, the wrong one reverses the graph, and
\`bd dep cycles\` stays green either way. A reversed edge schedules the whole plan backwards.

Also decide whether this is a real dependency at all. Two beads touching nearby code is a
scheduling hint — it belongs in the same lane — not a dependency.`,
      { schema: EDGE_VERDICT, label: `edges:${batch[0].from}+${batch.length - 1}`, phase: 'Verify' },
    ))),
  parallel(chunk(ready, BATCH).map((batch) => () =>
    agent(
      `Verify ${batch.length} backlog item(s) are genuinely ready, and enumerate everything each must touch.
REPO: ${REPO}

BEADS:
${batch.map((b) => `  ${b.id} — ${b.title}\n    census said it touches: ${(b.paths || []).join(', ') || '(nothing listed — itself suspicious)'}`).join('\n')}

Return one verdict per bead, in the same order. Do not merge or skip any.

${RULES}

1. Is it ALREADY SATISFIED by the default branch? Check before anything else, with file:line
   evidence. The tracker lags the tree.
2. Does it have an unmet dependency the census missed?
3. Enumerate EVERY path the work must touch. Include: the source, the tests that encode the
   behaviour being changed (a behaviour change forces the tests asserting the old behaviour), any
   migration, and any generated peripheral. If the work touches docs/**, the documentation manifest
   pins doc blob OIDs and must be regenerated, so it belongs in the list.
   An owned root that omits a file the work structurally needs cannot be finished by the lane that
   gets it, and that failure costs a full round.
4. Size it: small / medium / large.`,
      { schema: READY_VERDICT, label: `ready:${batch[0].id}+${batch.length - 1}`, phase: 'Verify' },
    ))),
])

// Each agent now returns { verdicts: [...] } for a batch, so flatten. A batch that DIED contributes
// nothing rather than a null that later code would read as a verdict.
const liveEdges = (edgeVerdicts || []).filter(Boolean).flatMap((r) => r.verdicts || [])
const liveReady = (readyVerdicts || []).filter(Boolean).flatMap((r) => r.verdicts || [])

const deadEdges = edges.length - liveEdges.length
const deadReady = ready.length - liveReady.length
if (deadEdges || deadReady) {
  log(`!! ${deadEdges} edge check(s) and ${deadReady} readiness check(s) DIED. Their subjects are UNVERIFIED`)
  log('   and are excluded from the plan below rather than silently assumed good.')
}

const reversed = liveEdges.filter((v) => v.storedDirectionIsCorrect === false)
const notReal = liveEdges.filter((v) => v.isRealDependency === false)
if (reversed.length) {
  log(`!! ${reversed.length} STORED EDGE(S) POINT THE WRONG WAY — the tracker's graph is inverted for:`)
  for (const r of reversed) log(`   ${r.from} -> ${r.to}: ${r.reasoning.slice(0, 160)}`)
  log('   The plan below uses the RE-DERIVED direction. Fix the tracker separately.')
}

// --- Plan ------------------------------------------------------------------
// Arithmetic, not judgement.
phase('Plan')

// Corrected graph: flip what verification says is backwards, drop what is not a real dependency.
// UNVERIFIED EDGES ARE NOT KEPT. A dead edge-verification batch used to fall through to
// `else corrected.push(stored)`, so the plan ordered work from precisely the edges nobody
// re-derived — including ones that point the wrong way. Fail closed: no verdict, no edge.
const corrected = []
const unverifiedEdges = []
for (const e of edges) {
  const v = liveEdges.find((x) => x.from === e.from && x.to === e.to)
  if (!v) { unverifiedEdges.push(e); continue }
  if (v.isRealDependency === false) continue
  if (v.storedDirectionIsCorrect === false) corrected.push({ from: e.to, to: e.from })
  else corrected.push({ from: e.from, to: e.to })
}
if (unverifiedEdges.length) {
  log(`!! ${unverifiedEdges.length} stored edge(s) have no verification verdict — dropped from the plan (fail closed)`)
  for (const e of unverifiedEdges.slice(0, 12)) log(`   unverified: ${e.from} -> ${e.to}`)
}

const startable = liveReady
  .filter((v) => v.isReady === true && v.alreadyDone !== true)
  .map((v) => {
    const b = ready.find((r) => r.id === v.id) || {}
    return { id: v.id, title: b.title || v.id, priority: b.priority ?? 2, paths: v.paths || b.paths || [], sizeHint: v.sizeHint || 'unknown' }
  })

const doneAlready = liveReady.filter((v) => v.alreadyDone === true).map((v) => v.id)
if (doneAlready.length) log(`already satisfied by the tree, do NOT dispatch: ${doneAlready.join(', ')}`)

// Longest-path depth over the corrected graph. Depth is what actually costs wall-clock: this
// programme measured 12 agents at depth 6 taking 74 minutes and 36 agents at the same depth taking
// 63 -- latency tracks DEPTH, and is nearly flat in WIDTH. So the critical path is the schedule.
// Edges are dependent `from` → prerequisite `to`. A ready bead has no outgoing deps, so measuring
// prerequisites always yields depth 0. Measure what starting the bead UNLOCKS: reverse edges.
function depthOf(id, seen = new Set()) {
  if (seen.has(id)) return 0 // a cycle cannot lengthen the path; it is reported separately
  seen.add(id)
  const downstream = corrected.filter((e) => e.to === id).map((e) => e.from)
  return downstream.length ? 1 + Math.max(...downstream.map((d) => depthOf(d, new Set(seen)))) : 0
}

const withDepth = startable.map((b) => ({ ...b, depth: depthOf(b.id) }))
const maxDepth = withDepth.reduce((m, b) => Math.max(m, b.depth), 0)
const criticalPath = withDepth.filter((b) => b.depth === maxDepth).map((b) => b.id)

// Two lanes may never share a worktree, and may never own overlapping roots: they cannot corrupt
// each other's files, but they collide at LAND time, after every reviewer has passed.
function overlaps(a, b) {
  return a.some((x) => b.some((y) => x.startsWith(y) || y.startsWith(x)))
}

const ordered = [...withDepth].sort((a, b) => b.depth - a.depth || a.priority - b.priority)
const deferred = []
// PATHS ARE AN AGENT'S PROSE UNTIL THIS FUNCTION SAYS OTHERWISE.
// The header of this file says agents report observations and the script computes conclusions. That
// was applied to the ORDERING and not to the PATHS, and a measured run showed all three ways it fails:
//   - absolute paths: 64 of one lane's 64 roots came back as /Users/<name>/<worktree>/backend/...
//   - prose in the array: one entry was "<the remaining files from `git grep -lE ...`", which split
//     on whitespace into fake roots like "<the", "remaining", "files"
//   - over-broad roots: "backend/" and "docs/" are not owned roots, they are the repository, and with
//     union-find ONE of them transitively swallows every bead into a single lane. 35 startable beads
//     collapsed into 2 lanes, one owning the whole tree. The partition was correct; the input was not.
// A root that does not survive normalisation is not silently dropped -- the bead is DEFERRED and says
// why, because a lane dispatched with a root it cannot use fails after the work is done, not before.
const MIN_ROOT_SEGMENTS = 2
function normaliseRoot(raw) {
  if (typeof raw !== 'string') return null
  let r = raw.trim()
  if (!r || /\s/.test(r) || /[<>`*?]/.test(r)) return null       // prose, globs, placeholders
  // Absolute paths must sit under REPO with a path-segment boundary. A prefix test alone accepts
  // `/workspace/console-lane/...` when REPO is `/workspace/console` (sibling worktree leak).
  if (r.startsWith('/')) {
    const repo = String(REPO).replace(/\/+$/, '')
    if (r === repo || r.startsWith(`${repo}/`)) r = r.slice(repo.length)
    else return null
  }
  r = r.replace(/^\/+/, '')
  // A trailing file becomes its directory before segment collapse, so both spellings of a
  // directory and a file inside it reduce to the same root. Extensionless files (Dockerfile,
  // BUCK, Makefile, LICENSE, …) must NOT be mistaken for directories: inventing
  // `backend/Dockerfile/` authorises a nonexistent subtree. Preserve the exact file path.
  let fileRoot = false
  if (r && !r.endsWith('/')) {
    const base = r.split('/').pop() || ''
    if (/\.[A-Za-z0-9]+$/.test(base)) r = r.slice(0, -base.length)
    else if (base) fileRoot = true
  }
  // Canonicalise `./backend/crates/foo/` and `backend/crates/foo/` to the same root. Without
  // this, overlaps treats them as disjoint and the packer can put one directory in two lanes.
  const parts = r.split('/').filter((seg) => seg && seg !== '.')
  if (parts.includes('..')) return null
  r = parts.join('/')
  if (!r) return null
  if (!fileRoot && !r.endsWith('/')) r += '/'
  if (r === '/') return null
  if (r.split('/').filter(Boolean).length < MIN_ROOT_SEGMENTS) return null  // "backend/" is the repo
  return r
}

const placeable = []
for (const item of ordered) {
  // Count failed normalisations BEFORE dedupe. Two valid files under one directory share a root;
  // subtracting after Set would misclassify them as rejected and defer ordinary multi-file work.
  const normalised = (item.paths || []).map(normaliseRoot)
  const rejected = normalised.filter((r) => !r).length
  const roots = [...new Set(normalised.filter(Boolean))]
  if (!roots.length) {
    deferred.push({ ...item, why: (item.paths || []).length
      ? `every reported path was unusable as an owned root (${(item.paths || []).length} rejected: absolute, prose, or repo-wide)`
      : 'no paths reported — an owned root cannot be derived' })
    continue
  }
  // A PARTIAL OWNED ROOT IS WORSE THAN NO LANE. The guard above defers a bead whose paths are ALL
  // unusable; this one was merely logging the partial case and emitting the lane anyway, so a lane
  // could be authorised for work while forbidden from touching a file that work structurally needs.
  // That failure arrives AFTER dispatch -- the lane builds, then blocks or fails review -- and it has
  // cost this programme four rounds in exactly that shape. Same defect as the all-unusable case, one
  // spelling over: rejecting SOME is not a smaller version of rejecting ALL, it is the same problem
  // with the evidence hidden in a log line.
  if (rejected) {
    const bad = (item.paths || []).filter((raw) => !normaliseRoot(raw))
    deferred.push({ ...item, why: `${rejected} of ${(item.paths || []).length} reported path(s) are unusable as owned roots, so any lane would be authorised for the work while forbidden from part of it: ${bad.join(', ')}` })
    log(`${item.id}: DEFERRED — ${rejected} unusable path(s): ${bad.join(', ')}`)
    continue
  }
  placeable.push({ ...item, roots })
}

// GROUP BY TERRITORY BEFORE CREATING LANES, transitively.
// The first version grew a lane's roots as it absorbed a bead, and a grown lane can overlap a lane
// created EARLIER -- so a run that had already done all its verification threw at the final guard and
// discarded 15 agents of work. It also merged a bead into only the FIRST lane it clashed with, while
// leaving the second clash unaddressed. Both are the same mistake: deciding membership one item at a
// time against a set that is still moving.
// Union-find settles the whole partition first, so two lanes CANNOT share territory by construction
// and the guard below becomes an assertion that can never fire rather than a way to lose a run.
const parent = placeable.map((_, i) => i)
const find = (i) => (parent[i] === i ? i : (parent[i] = find(parent[i])))
for (let i = 0; i < placeable.length; i++) {
  for (let j = i + 1; j < placeable.length; j++) {
    if (overlaps(placeable[i].roots, placeable[j].roots)) parent[find(i)] = find(j)
  }
}
const groups = new Map()
placeable.forEach((item, i) => {
  const root = find(i)
  if (!groups.has(root)) groups.set(root, [])
  groups.get(root).push(item)
})

// Deepest group first: depth is what costs wall-clock, so the longest chain must start earliest.
const sorted = [...groups.values()].sort((a, b) =>
  Math.max(...b.map((x) => x.depth)) - Math.max(...a.map((x) => x.depth)) ||
  Math.min(...a.map((x) => x.priority)) - Math.min(...b.map((x) => x.priority)))

const lanes = []
for (const group of sorted) {
  if (lanes.length >= MAX_LANES) {
    for (const item of group) deferred.push({ ...item, why: 'maxLanes reached' })
    continue
  }
  const beads = group.map((x) => x.id)
  const roots = [...new Set(group.flatMap((x) => x.roots))]
  lanes.push({ key: beads[0].replace(/[^a-z0-9]/gi, '').slice(-6) || `l${lanes.length}`, beads, roots, depth: Math.max(...group.map((x) => x.depth)) })
}

// The guard that makes the output trustworthy: never emit a plan whose lanes would be refused.
for (let i = 0; i < lanes.length; i++) {
  for (let j = i + 1; j < lanes.length; j++) {
    if (overlaps(lanes[i].roots, lanes[j].roots)) {
      throw new Error(`scout: emitted lanes ${lanes[i].key} and ${lanes[j].key} own overlapping roots — this plan would be refused at dispatch, which means the packing above is wrong`)
    }
  }
}

log(`plan: ${lanes.length} lane(s), critical path depth ${maxDepth}, ${deferred.length} deferred`)
for (const l of lanes) log(`  ${l.key}: ${l.beads.join(' ')} @ ${l.roots.join(' ')}`)

// ONE integration branch, ONE PR. N lanes each opening a PR is how a queue of unreviewable
// branches accumulates; this programme reached twelve-plus worktrees above main with zero open PRs,
// then the opposite. The lane fan-out lands every converged lane onto ONE branch, and that branch
// is what becomes a pull request -- once.
const integration = ARGS.integrationBranch || 'integration/scouted'

return {
  headline: [
    `${startable.length} startable, ${lanes.length} lanes proposed, critical path depth ${maxDepth}`,
    reversed.length ? `${reversed.length} STORED DEPENDENCY EDGE(S) ARE REVERSED — tracker needs fixing` : 'every verified edge points the right way',
    notReal.length ? `${notReal.length} stored edge(s) are not real dependencies` : null,
    doneAlready.length ? `${doneAlready.length} bead(s) already satisfied by the tree` : null,
    deadEdges || deadReady ? `${deadEdges + deadReady} check(s) died — those subjects are UNVERIFIED` : null,
  ].filter(Boolean),
  criticalPath,
  reversedEdges: reversed,
  alreadyDone: doneAlready,
  deferred,
  unverifiedEdges,
  // INCOMPLETE for lane-fanout. A prior revision advertised this as `fanoutArgs` that could be
  // "fed straight into lane-fanout", but every nonempty plan aborted: lane-fanout requires tip and
  // per-lane wt/brief/accept, which a read-only scout must not invent. Briefs especially: writing
  // them here would decide the implementation without reading the code. Callers must supply the
  // missing fields before dispatch.
  fanoutPlan: {
    status: 'incomplete',
    reason: 'missing tip and per-lane wt/brief/accept — scout only proposes beads and owned roots',
    integrationBranch: integration,
    land: true,
    lanes: lanes.map((l) => ({ key: l.key, bead: l.beads.join(' '), owned: l.roots.join(' ') })),
  },
}
