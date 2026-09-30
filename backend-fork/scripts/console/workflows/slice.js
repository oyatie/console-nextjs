export const meta = {
  name: 'slice',
  description: 'Build ONE vertical slice through the full pipeline: explore, design, RED tests first, implement, cover, doubt, simplify, security, CI integration, then two adversarial reviewers who see the DIFF ONLY. Reusable for any slice — parameterised by task, exploration areas, and owned paths.',
  whenToUse: 'Any bounded implementation slice where correctness matters more than speed: a new crate, a gate, a domain type, a migration. Not for trivial edits (a 2-line change does not need ten phases).',
  phases: [
    { title: 'Explore', detail: 'parallel readers — read the code, do not guess' },
    { title: 'Design', detail: 'competing designs, then judged and synthesised' },
    { title: 'Red', detail: 'failing tests FIRST, each observed failing for the right reason' },
    { title: 'Implement', detail: 'make them green without rewriting them' },
    { title: 'Cover', detail: 'the rest of the tests, measured not estimated' },
    { title: 'Doubt', detail: 'hunt what is wrong and repair it' },
    { title: 'Simplify', detail: 'smaller without weaker — never delete a check' },
    { title: 'Security', detail: 'attack it as a hostile tenant' },
    { title: 'Integrate', detail: 'prove every new test actually executes in CI' },
    { title: 'Prove', detail: 'two adversarial reviewers — diff only, no author reasoning' },
  ],
}

// Workflow SCRIPT, not a Node module: the runtime injects agent()/parallel()/phase()/log()/args
// and runs the body in an async context. Top-level await and the trailing return are the
// documented form — `node --check` will wrongly flag the return. Do not wrap in a function.
//
// args: {
//   task:     string   — what to build. Required.
//   context?: string   — domain facts the agents cannot infer.
//   explore?: [{label, prompt}]  — exploration areas. Defaults to one generalist reader.
//   owns?:    string   — paths the implementer owns. A COHERENT SLICE, not a file list (see below).
//   crate?:   string   — crate boundary for the work queue. Bun grouped ~16k errors BY CRATE,
//                        never by file, explicitly to prevent task fragmentation.
//   designs?: number   — competing designs before the judge. Default 2.
//   lane?:    "1".."5" — REQUIRED for anything that builds. Routes implement/prove into
//                        ~/Developer/console-lanes/lane-N, which has its own backend/target.
//                        Omitting it builds in the main checkout and contends on the build lock.
//   repo?:    string
// }
// NO NODE GLOBALS. The script body runs in a bare sandbox: `process` is not defined, so a
// `process.env.HOME` fallback is not a fallback — it is an immediate ReferenceError that kills the
// run before agent 1 starts. (`Date.now`/`Math.random` are likewise banned, as they would break
// resume.) Paths are therefore literals or `args`. Verified by the failure this line replaced.
const HOME = '/Users/jasonlee'
const A = typeof args === 'string' ? JSON.parse(args) : (args || {})
// The unknown-option guard runs BEFORE the required-field check on purpose: with the order
// reversed, a caller who typos an option gets "`task` is required" and goes looking for the wrong
// thing. Report the typo you can see, not the consequence of it.
const KNOWN_ARGS = ['base', 'context', 'crate', 'designs', 'explore', 'lane', 'owns', 'repo', 'task']
{
  const unknown = Object.keys(A).filter((k) => !KNOWN_ARGS.includes(k))
  if (unknown.length) {
    throw new Error(`slice: unknown option(s) ${unknown.join(', ')}. Known: ${KNOWN_ARGS.join(', ')}.`)
  }
}
if (!A.task) throw new Error('slice: `task` is required')
const REPO = A.repo || `${HOME}/Developer/console`

// MEASURED 2026-07-28: a workflow that ran its implementer with cwd = the MAIN checkout turned a
// single-crate `cargo check` into 47 MINUTES — the log reads `Blocking waiting for file lock on
// build directory`, because the implementer and the caller contended on one `backend/target`. CI
// for the same change is 20 minutes. The bottleneck was never CI; it was a shared target dir.
//
// Anything that BUILDS therefore runs in a lane worktree, which has its own `target/`. Read-only
// exploration may use the main checkout because reads take no build lock.
const LANE = A.lane ? `${HOME}/Developer/console-lanes/lane-${A.lane}` : null
const BUILD_CWD = LANE || REPO
if (!LANE) log('WARNING: no `lane` arg — implement/prove will build in the MAIN checkout and may contend on the build lock. Pass lane: "1".."5".')
// What a reviewer diffs against. The implementer now COMMITS, so `git diff` alone would show an
// empty tree and a reviewer would report "no changes" as a pass.
const BASE_REF = A.base || 'origin/main'
const N_DESIGNS = Math.min(Math.max(A.designs ?? 2, 1), 3)
const CRATE = A.crate || ''
const OWNS = A.owns || ''

// ── Discipline shared by every agent ────────────────────────────────────────
// Each line below was earned by a measured failure, not adopted on principle.
const DISCIPLINE = `
## Non-negotiable discipline

- **Verify by EXECUTION.** Run it; quote real output. Cite \`file:line\` of CODE, never a header
  comment — a plan premise died because a migration's header described the problem it had already
  fixed. Reasoning that feels airtight has been wrong repeatedly here.
- **A probe must be proven RED on a known-bad input before its GREEN is trusted.** Six verification
  probes were defective in one session; in one case the models were right and the *grader* was wrong.
  A probe with no demonstrated failure mode is not evidence.
- **Reproduce the original failure, not the artifact you touched.** A repair once fixed 1 of 3
  commands and reported green because it tested only its own file.
- **\`git stash\` and \`git reset\` are BANNED.** Commit or abandon. Atomic, per-file commits.
  (Bun had to amend their workflow to forbid these after agents used them to escape trouble.)
- **Write findings to disk as you go.** Agents have gone idle without ever returning a report; do not
  rely on your final message surviving.
- **\`grep\` is unreliable in this shell** (exits 1 on files it matches). Use \`awk\` for anything
  load-bearing.
- **Escalate rather than settle.** If the correct fix lies outside your slice, STOP and report it.
  Do not implement the second-best fix — an implementer once knowingly shipped a worse design because
  a briefing file-list omitted the file the real fix needed. The briefing was the defect.
`

const CTX = `
Repo: ${REPO}
${A.context ? A.context + '\n' : ''}${CRATE ? `Crate boundary: \`${CRATE}\`. Group work BY CRATE, never by file — \`cargo check -p ${CRATE}\` is the work queue.\n` : ''}${OWNS ? `Slice you own: ${OWNS}\n` : ''}
## Task
${A.task}
${DISCIPLINE}`

// Appended to EVERY phase, exploration included.
//
// The first version excluded exploration on the reasoning that "reads take no build lock". That
// premise was refuted by observation within the hour: an explorer asked to confirm a call signature
// ran `cargo test -p console-ontology-rest --test publish_auto_create_action_as_runtime_role`
// against the MAIN checkout's Cargo.toml and held `backend/target/debug/.cargo-lock` for over seven
// minutes, blocking `npm run verify` behind `Blocking waiting for file lock on build directory`.
// Verifying a signature by executing it is exactly the discipline demanded elsewhere in this file,
// so the fix is to give exploration a lane too — not to tell it to stop running things.
const WORKDIR = `
## WORKING DIRECTORY — READ THIS BEFORE RUNNING ANYTHING

**Your inherited cwd is NOT your lane.** You start in whatever directory the calling session
happened to be in when you were spawned, and that value drifts constantly as the caller works.
Assume it is wrong.

**Run this FIRST — before \`git status\`, before \`pwd\`, before any orientation command at all:**
\`\`\`bash
cd ${BUILD_CWD}
source ${REPO}/scripts/console/lane-env.sh    # RUSTC_WRAPPER=sccache, 50G ceiling
\`\`\`

This is not a formality. An implementer once oriented itself with \`git status\` in its inherited
cwd, found a DIFFERENT lane's branch and commits there, concluded that its own brief was stale,
and asked whether it should ignore the instruction and work in the wrong tree. It was right about
what it saw and wrong about what it meant. Had it proceeded, its work would have landed in another
agent's open pull-request branch and been swept into that agent's next commit.

If what you find after \`cd\` contradicts your brief, that is a real conflict worth escalating. If
you find a contradiction BEFORE \`cd\`, you are simply in the wrong directory.
sccache's cache is user-global (\`~/Library/Caches/Mozilla.sccache\`), so lanes — and other repos on
this machine — reuse each other's compiled artifacts. Without it every lane recompiles the whole
dependency graph from cold: measured \`Cache hits: 0%\` across 4,084 commands, because nothing had
ever sourced this. Confirm with \`sccache --show-stats\` after your build; hits should climb.

${LANE
  ? `This is an isolated lane worktree with its own \`backend/target\`, so your build cannot contend with
another agent's. Building in the main checkout instead is what turned a single-crate \`cargo check\`
into 47 minutes.`
  : `NOTE: no lane was assigned, so this IS the main checkout and you may contend on the build lock
with concurrent agents. Report the contention if a build stalls rather than waiting it out.`}
`

phase('Explore')

const EXPLORE_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['area', 'findings', 'exact_api', 'gotchas'],
  properties: {
    area: { type: 'string' },
    findings: { type: 'string', description: 'What EXISTS, with file:line. Concrete, not summary.' },
    exact_api: { type: 'string', description: 'Exact signatures/paths to call, copied verbatim from source.' },
    gotchas: { type: 'array', items: { type: 'string' }, description: 'What will silently break a naive implementation.' },
  },
}

const areas = A.explore?.length ? A.explore : [{
  label: 'survey',
  prompt: 'Survey what already exists for this task. The most valuable finding is that some of it is already built — assume nothing is greenfield until you have checked.',
}]

const facts = (await parallel(areas.map((a) => () =>
  agent(`${CTX}${WORKDIR}\n## YOUR EXPLORATION AREA\n${a.prompt}\n\nRead the code. Return exact APIs a caller must use — an implementer will build directly from your answer, so a vague finding becomes their wrong guess.`,
    { label: `explore:${a.label}`, phase: 'Explore', schema: EXPLORE_SCHEMA })
))).filter(Boolean)

log(`${facts.length}/${areas.length} exploration lanes returned`)

phase('Design')

const DESIGN_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['approach', 'file_layout', 'verification', 'risks'],
  properties: {
    approach: { type: 'string' },
    file_layout: { type: 'string', description: 'Exact paths and what each contains.' },
    verification: { type: 'string', description: 'How this is PROVEN to work, including what would make it fail.' },
    risks: { type: 'array', items: { type: 'string' } },
  },
}

// Competing designs, not one. Bun could skip this because a .zig reference removed
// design entirely; where no reference exists, one design is one unexamined guess.
const BIASES = [
  'BIAS: minimal surface. Fewest files, least abstraction, no framework. An interface with one implementation is a defect here.',
  'BIAS: durability. Optimise for surviving change underneath you. What breaks when the substrate moves, and what makes this immutable?',
  'BIAS: falsifiability. Optimise for the failure being LOUD and specific. How does this fail for the right reason, and how would a silent pass be detected?',
]

const designs = (await parallel(
  Array.from({ length: N_DESIGNS }, (_, i) => () =>
    agent(`${CTX}\n\nVerified exploration findings:\n${JSON.stringify(facts, null, 2)}\n\n## Design it\n${BIASES[i % BIASES.length]}`,
      { label: `design:${i + 1}`, phase: 'Design', schema: DESIGN_SCHEMA }))
)).filter(Boolean)

const spec = designs.length === 1 ? JSON.stringify(designs[0], null, 2) : await agent(
  `${CTX}\n\nCompeting designs:\n${JSON.stringify(designs, null, 2)}\n\n## JUDGE and SYNTHESISE

Pick the better spine and graft the best ideas from the others. State plainly which you chose and
what you took from the losers — a synthesis that silently drops a rival's best idea is a worse
outcome than either design alone.

Judge in this order:
1. **Is it verifiable, and does it fail LOUDLY?** A design that cannot demonstrate its own failure is
   disqualified regardless of elegance.
2. **Is it simple?** Fewest files, least abstraction.
3. **Does it survive the substrate changing?**

Return a FINAL SPECIFICATION concrete enough to implement with no further design decisions: exact
paths, exact APIs from the exploration findings, and the exact proof of correctness.`,
  { label: 'judge', phase: 'Design' })

// ── The build pipeline ──────────────────────────────────────────────────────
// Red tests BEFORE implementation, then defect-hunting, simplification, security and CI wiring as
// SEPARATE passes before the final adversarial verification. Each stage is its own agent because a
// single agent asked to implement AND simplify AND security-review its own work grades its own
// homework — the same reason the reviewers never see the implementer's narrative.
//
// Order is deliberate and is not arbitrary taste: defects are fixed before simplification (you
// cannot safely simplify code that is wrong), simplification precedes security review (so the
// review reads what actually ships, not a draft), and CI wiring precedes verification (so the
// verifier can confirm the gate really executes rather than that it merely exists).
const COMMIT_RULE = `
**COMMIT as you go** on the branch already checked out in your lane. Atomic commits, one coherent
step each. Stage the paths you own BY NAME — never \`git add -A\` or \`git commit -a\`, which is how
one agent's work ends up inside another agent's commit. \`git stash\` and \`git reset\` stay banned.

An earlier version of this workflow told implementers to leave work UNCOMMITTED because "the caller
owns landing". That contradicted the stash/reset ban, an implementer followed the nearer rule, and a
concurrent \`git reset --hard\` in the same tree destroyed a finished, passing deliverable. The
caller owns landing; it does not own keeping your work alive, and neither does the filesystem.`

phase('Red')

// TDD, and the reason it is a separate phase with its own gate: a test written after the code tends
// to assert what the code does. Written first, it asserts what the code SHOULD do. The gate is that
// the tests must be OBSERVED failing — a red test nobody watched fail is just an unproven claim.
const red = await agent(`${CTX}${WORKDIR}

Invoke the \`test-driven-development\` skill and follow it.

## SPECIFICATION — already judged. Do NOT implement it yet.
${spec}

Exploration findings you may rely on:
${JSON.stringify(facts, null, 2)}

## Your job: write the FAILING tests, and nothing else
Write the tests that will prove this specification correct, BEFORE any implementation exists. Then
RUN them and paste the real failure output.

Hard rules:
1. **Do not write implementation code.** If a test cannot even compile without a function that does
   not exist yet, add the smallest possible signature that returns \`unimplemented!()\` or its
   equivalent — never a working body. That stub is the thing the next phase replaces.
2. **Every test must be OBSERVED failing, and for the RIGHT REASON.** Quote the actual output. A
   test that fails because a helper is missing, a fixture is wrong, or the file does not compile is
   NOT a red test — it is a broken test that happens to be red. Distinguish these explicitly.
3. **A test that passes before the implementation exists is a defect in the test.** Say so and fix
   it. This is the single most valuable thing this phase produces.
4. Assert BEHAVIOUR, not implementation shape. A test that pins internal structure blocks the
   simplification phase for no safety gain.
5. Include the negative and refusal cases now, not later — those are the ones that get quietly
   dropped when written after the fact.
${COMMIT_RULE}

Report: each test, the failure you observed, and whether that failure is the right one.`,
  { label: 'red', phase: 'Red' })

phase('Implement')

const impl = await agent(`${CTX}${WORKDIR}

## SPECIFICATION — already judged. Implement it; do not redesign.
${spec}

## THE RED TESTS ARE ALREADY WRITTEN AND OBSERVED FAILING
${red}

## Your job
Make those tests pass. Do not rewrite them to fit your implementation — if a test is genuinely
wrong, say so explicitly and explain why rather than quietly editing it green. Changing a test to
match the code you wrote inverts the entire point of writing it first.

Implement it, and ${COMMIT_RULE.trim().slice(2)}

This instruction used to read "leave changes UNCOMMITTED, the caller owns landing", which
contradicted the \`git stash\`/\`git reset\` ban three paragraphs above it. An implementer followed
the nearer rule, and a concurrent agent's \`git reset --hard\` in the same tree destroyed the
finished deliverable — a working mechanism, already passing, gone. Two rules pointing opposite ways
is a defect in the process, not in the agent that picked one.

Never \`git commit -a\` or \`git add -A\`: stage the paths you own by name. A blanket add is how one
agent's work ends up inside another agent's commit.
${OWNS ? `You own: ${OWNS}. Touching anything else is a scope violation; escalate instead.\n` : ''}
Requirements:
1. It must COMPILE${CRATE ? ` — run \`cargo check -p ${CRATE}\` and iterate until clean` : ''}.
2. It must be PROVEN, by execution, with real output quoted.
3. If you add a Rust crate, a valid \`Cargo.toml\` lands in the SAME change — an unmatched workspace
   glob breaks the build for every lane. Never hand-edit a generated BUCK file.
4. **Report what you could NOT verify, plainly.** An unproven claim stated as fact is worse than an
   admitted gap, because it survives review.`,
  { label: 'implement', phase: 'Implement' })

phase('Cover')

const cover = await agent(`${CTX}${WORKDIR}

The specification is implemented and the red tests are green. Read the diff yourself:
\`git diff ${BASE_REF}...HEAD\`.

## Your job: close the coverage gap the red tests did not reach
The red phase wrote the tests that prove the SPEC. That is necessarily narrower than the code that
now exists. Find what ships untested and test it.

1. **Measure, do not estimate.** Use whatever coverage tooling this repo has; if none is wired, walk
   every branch of the new code by hand and say that is what you did. "Looks well covered" is not a
   measurement.
2. Prioritise by consequence, not by line count: error paths, refusal paths, boundary values, and
   anything touching authorization, tenancy or money. An untested happy path is a risk; an untested
   refusal path is a vulnerability.
3. **Every test you add must be proven RED first** — break the code, watch it fail, restore with a
   \`cp\` backup (never \`git checkout\`, which would discard uncommitted work). A test added at this
   stage is at the highest risk of asserting what the code does rather than what it should do.
4. State the residual gap plainly. Some things genuinely cannot be tested at this layer; naming them
   is worth more than a fabricated test that pretends otherwise.
${COMMIT_RULE}`,
  { label: 'cover', phase: 'Cover' })

phase('Doubt')

// Defect-hunting BEFORE simplification: simplifying wrong code produces elegant wrong code, and the
// elegance makes the wrongness harder to see.
const doubt = await agent(`${CTX}${WORKDIR}

Invoke the \`doubt-driven-development\` skill, and draw on \`code-review-and-quality\` and
\`debugging-and-error-recovery\` as needed. Follow them.

An implementation was just committed on this lane's branch. Read it yourself:
\`git diff ${BASE_REF}...HEAD\`. You are NOT given the implementer's account — their reasoning would
prime you to accept it.

## Your job: find what is WRONG, then fix it
You may edit, unlike the final reviewers. This is the repair pass.

1. **Doubt every claim the code makes about itself.** Comments, docstrings and commit messages are
   the least reliable artefacts in this repository — it has recorded THREE separate incidents of a
   comment describing a problem it had already fixed, one of them yesterday. Where a comment and the
   code disagree, the code is the fact and the comment is the defect.
2. **Hunt the failure modes the tests do not express.** This suite has TWICE been unable to
   distinguish a correct implementation from a wrong one: a resolver that produced a byte-identical
   graph while writing false history, and an as-of read silently becoming a head read. Both were
   caught by reading the diff and asking what ELSE would produce this green.
3. Check concurrency, partial failure, and rollback: what happens if this is interrupted midway?
4. Fix what you find, with a test that would have caught it. Report anything you judge out of scope
   as an explicit escalation rather than leaving it unsaid.
${COMMIT_RULE}`,
  { label: 'doubt', phase: 'Doubt' })

phase('Simplify')

const simplify = await agent(`${CTX}${WORKDIR}

Invoke the \`code-simplification\` skill and the \`ponytail\` skill. Follow them.

Read the diff yourself: \`git diff ${BASE_REF}...HEAD\`.

## Your job: make it smaller without making it weaker
The tests are green and the known defects are fixed. Now remove what does not earn its place.

1. **Behaviour must not change.** Run the full test set before and after; both must be identical.
   If a simplification requires a test to change, it is not a simplification — stop and report it.
2. Delete speculative generality: an abstraction with one implementation, a parameter every caller
   passes the same value for, a config knob nothing configures, a branch nothing reaches.
3. Prefer the stdlib and what this repo already has over anything new. Reuse beats invention.
4. **Do NOT simplify away:** input validation at trust boundaries, error handling that prevents data
   loss, authorization checks, tenancy scoping, or any assertion. Removing a check is not
   simplification, it is scope reduction wearing a disguise — and this repo's reviewers explicitly
   check for deleted assertions.
5. If the diff is already tight, say so and change nothing. A pass that invents work to look busy is
   worse than a pass that reports the code is clean.
${COMMIT_RULE}`,
  { label: 'simplify', phase: 'Simplify' })

phase('Security')

// After simplification deliberately: the review must read what actually ships.
const security = await agent(`${CTX}${WORKDIR}

Invoke the \`security-and-hardening\` skill (and \`claude-security\` if relevant). Follow them.

Read the diff yourself: \`git diff ${BASE_REF}...HEAD\`. You may edit to fix what you find.

## Your job: attack this change
This is a multi-tenant system with row-level security, policy-based authorization and an audit
chain. Assume an attacker holds valid credentials for ONE tenant and wants another tenant's data.

1. **Tenancy first.** Is every new query armed with the org scope? Is any read reachable that
   bypasses RLS? A superuser or BYPASSRLS read in a test makes the isolation assertion vacuous —
   check that the tests assert as the genuine non-superuser runtime role.
2. **Authorization, not authentication.** Who can call this? Is the check on the right side of the
   trust boundary? Does a deny-by-default path stay deny-by-default under every input, including
   empty, null and absent?
3. **Injection and trust boundaries.** Any string interpolated into SQL, any client-supplied
   identifier used as a key, any input reaching a \`SECURITY DEFINER\` function.
4. **Failure modes.** Does an error leak internal state? Does a partial failure leave a half-applied
   write? Does a raw database error escape as a 500 where a mapped 4xx belongs?
5. Fix what you find. For anything you cannot fix in scope, ESCALATE it explicitly with its concrete
   attack path — a named vulnerability is worth more than a silent one.
${COMMIT_RULE}`,
  { label: 'security', phase: 'Security' })

phase('Integrate')

// CI wiring as its own phase because "a test that cannot execute in CI is not a deliverable" has
// been violated repeatedly here — most recently a Buck target that existed, was correct, and that no
// workflow ever referenced, so the only proof of a core mechanism had never once run.
const integrate = await agent(`${CTX}${WORKDIR}

Invoke the \`ci-cd-and-automation\` skill and follow it.

Read the diff yourself: \`git diff ${BASE_REF}...HEAD\`.

## Your job: make every new test actually EXECUTE in CI
A test that cannot run in CI is not a deliverable. This repository has shipped the failure twice: a
Buck target that existed and was correct but that no workflow referenced — so the only committed
proof of a core mechanism had never executed — and a required check whose display name promised
something its steps did not run.

1. Trace each new test from the file to the workflow step that runs it. Name the chain explicitly:
   generator entry, build target, wrapper, workflow step. A missing link anywhere means it never runs.
2. Generated files are GENERATED. Never hand-edit one; change the generator and regenerate.
3. **Prove it, do not assume it.** Run the local equivalent of the CI job. If a job cannot run
   locally, say exactly which link you could not verify and why.
4. If a new gate should become a required check, say so and explain the sequencing — a required
   context that has never reported blocks every merge, so it must report green at least once first.
${COMMIT_RULE}`,
  { label: 'integrate', phase: 'Integrate' })

phase('Prove')

const VERDICT_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['verdict', 'proven_failable', 'weakening', 'residual'],
  properties: {
    verdict: { type: 'string', enum: ['PASS', 'FAIL'] },
    proven_failable: { type: 'string', description: 'Concrete inputs you RAN that make it fail, with real output. Could not make it fail = FAIL.' },
    weakening: { type: 'string', description: 'Was any assertion deleted, loosened, skipped or made conditional? Diff it yourself.' },
    residual: { type: 'array', items: { type: 'string' }, description: 'Still vacuous or unverified after this change.' },
  },
}

// Bun's reviewers received the DIFF ONLY and never the implementer's reasoning — the author's
// narrative primes a reviewer to accept. Passing the implementer's report here (as I did in an
// earlier workflow) quietly defeats the mechanism, so these agents are told to read the diff
// themselves and are given no summary of it.
const REVIEW_BASE = `${CTX}${WORKDIR}

Invoke the \`verification-before-completion\` skill and follow it.

**READ-ONLY. No edits, no git mutations.** Every earlier phase could edit; you cannot. You are the
last thing between this change and the branch, and your job is to disbelieve it.

An implementation was just COMMITTED on this lane's branch. **Read the diff yourself** —
\`git diff ${BASE_REF}...HEAD\`, plus \`git log --oneline ${BASE_REF}..HEAD\` and
\`git status --short\` to confirm nothing was left dangling. You are deliberately NOT given the
implementer's account of what they did: their reasoning would prime you to accept it, and Bun's
reviewers saw the diff alone for exactly this reason.

**If you build or test, do it in THIS lane and nowhere else.** Never run a build in another agent's
lane — two writers in one worktree share a \`target/\` and a build lock, and the results are not
merely slow but WRONG. A verification run made while another agent was building in the same tree
reported a test binary as 0-passed/3-failed; the identical command on an uncontended tree reported
3-passed/0-failed minutes later. A contended run is not evidence, in either direction.

Assume the change is WRONG and look hard before conceding anything. Bun's reviewers also rejected
solutions that needed a paragraph of justification to defend — a workaround that must be explained
is a defect.`

const verdicts = (await parallel([
  () => agent(`${REVIEW_BASE}

## YOUR LENS: vacuity and correctness
1. Can you make it FAIL? Construct concrete failing inputs and RUN them. If you cannot make it fail,
   that is a FAIL verdict — a check that cannot fail is not a check.
2. Would a trivial STUB satisfy this? Try to write one. Success here is the most valuable finding
   available.
3. Was any existing assertion deleted, loosened or made conditional?`,
    { label: 'prove:vacuity', phase: 'Prove', schema: VERDICT_SCHEMA }),

  () => agent(`${REVIEW_BASE}

## YOUR LENS: discipline and honesty — deliberately different from the other reviewer
1. Does it actually compile and run? Run it. Do not accept that it does.
2. Are the project's testing constraints honoured — non-superuser runtime role, disposable Postgres,
   \`--test-threads=1\`, no hand-edited generated files?
3. **Is anything claimed that was not verified?** Compare what the code proves against what any
   comment or docstring asserts. Overstatement that survives review becomes tomorrow's false premise.
4. Did it stay inside its slice? Check \`git status\` for files outside ${OWNS || 'the declared scope'}.`,
    { label: 'prove:discipline', phase: 'Prove', schema: VERDICT_SCHEMA }),
])).filter(Boolean)

const passed = verdicts.length === 2 && verdicts.every((v) => v.verdict === 'PASS')

// "Edit the process, not the outputs." When a lane produces bad work, Bun fixed the PROMPT and
// reran rather than hand-patching the diff. Emit that rerun brief instead of a bare failure, so the
// next attempt corrects the instruction rather than the symptom.
const rerun = passed ? null : {
  guidance: 'Do NOT hand-patch the diff. Fix the brief and rerun this workflow — edit the process, not the outputs.',
  failures: verdicts.filter((v) => v.verdict !== 'PASS').flatMap((v) => [v.proven_failable, v.weakening]).filter(Boolean),
  suggested_context_additions: verdicts.flatMap((v) => v.residual || []),
}

return {
  specification: spec,
  red,
  implementation: impl,
  cover,
  doubt,
  simplify,
  security,
  integrate,
  verdicts,
  passed,
  rerun,
}
