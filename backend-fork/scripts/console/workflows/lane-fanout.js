export const meta = {
  name: 'lane-fanout',
  description: 'Reusable hardened lane fan-out: RED-baseline build, cross-lane defect ledger, adversarial diff-only review, independent re-verification, converge-or-escalate',
  whenToUse: 'Any multi-lane implementation phase with path-disjoint owned roots. Parameterise with args; do not fork this file per phase.',
  phases: [
    { title: 'Build', detail: 'one implementer per disjoint lane, failing test first' },
    { title: 'Review', detail: '2 diff-only adversarial reviewers + 1 independent re-runner per lane' },
  ],
}

// ---------------------------------------------------------------------------
// args = {
//   tip:        "<sha>"            // what each lane started from; ALL diffs are taken against this
//   lanes:      [{ key, bead, wt, owned, brief, accept, blockedTargets? }]
//               Lanes must be pairwise disjoint in BOTH `wt` and `owned`, and both are refused at
//               dispatch: a shared worktree collides while they build, a shared owned root collides
//               at LAND. `owned` must name paths, not describe them.
//   lockExtra?: "<string>"         // phase-specific additions to the lock contract
//   maxRounds?: 3
//   lenses?:    ["...", "..."]     // review lenses; defaults below
// }
//
// This file exists because the same harness was re-derived four times by hand, and every defect
// found in it (HEAD~1 diffing the base commit, reviewer findings discarded with no feedback edge,
// a stale owned-root after a mid-run scope ruling, "empty diff = auto reject") had to be fixed in
// each copy separately. Bun's rule applies to the pipeline as much as to the code it produces:
// fix the process that generates the work, not each instance of the work.
// ---------------------------------------------------------------------------

// args may arrive as a real object OR as a JSON-encoded string depending on how the caller passed
// it. A reusable harness should tolerate both rather than die on the caller's serialisation choice.
let ARGS = args
if (typeof ARGS === 'string') {
  try {
    ARGS = JSON.parse(ARGS)
  } catch (e) {
    throw new Error(`lane-fanout: args arrived as a string that is not valid JSON: ${e.message}`)
  }
}
ARGS = ARGS || {}

// AN OPTION THIS HARNESS DOES NOT READ MUST ABORT, NEVER BE SILENTLY DROPPED.
// Borrowed from a sibling runner where exactly this defect cost six lanes and ~2.3M tokens: a
// top-level option was accepted, ignored, and the run looked normal. The arg blobs passed here are
// kilobytes of hand-written JSON, so a single typo -- `lens` for `lenses`, `maxRound` for
// `maxRounds` -- silently changes what runs while every log line still looks right. Fail loudly at
// dispatch instead, where it costs seconds.
const KNOWN_ARGS = ['tip', 'lanes', 'maxRounds', 'lockExtra', 'lenses', 'land', 'integrationBranch', 'trial']
// `blockedTargets` was documented in the args comment above but omitted here, so a caller
// following the documented interface aborted with "unknown option(s)". Documented and accepted
// must be the same set — a doc that describes an input the code rejects is the same defect class
// as a doc that claims a property the code lost.
const KNOWN_LANE_KEYS = ['key', 'bead', 'wt', 'owned', 'brief', 'accept', 'reviewOnly', 'priorResult', 'blockedTargets']
{
  const unknown = Object.keys(ARGS).filter((k) => !KNOWN_ARGS.includes(k))
  for (const l of ARGS.lanes || []) {
    for (const k of Object.keys(l || {})) {
      if (!KNOWN_LANE_KEYS.includes(k)) unknown.push(`lanes[${(l && l.key) || '?'}].${k}`)
    }
  }
  if (unknown.length) {
    throw new Error(
      `lane-fanout: unknown option(s) ${unknown.join(', ')} — this harness would ignore them silently. ` +
      `Known top-level: ${KNOWN_ARGS.join(', ')}. Known per-lane: ${KNOWN_LANE_KEYS.join(', ')}.`,
    )
  }
}

const TIP = ARGS.tip
const LANES = ARGS.lanes || []
const MAX_ROUNDS = ARGS.maxRounds || 3

if (!TIP) throw new Error('lane-fanout: args.tip is required (the SHA every lane diffs against)')
if (!Array.isArray(LANES) || !LANES.length) throw new Error('lane-fanout: args.lanes must be a non-empty array')
for (const l of LANES) {
  for (const f of ['key', 'wt', 'owned', 'brief', 'accept']) {
    if (!l || !l[f]) throw new Error(`lane-fanout: lane ${l && l.key ? l.key : '<unnamed>'} is missing required field "${f}"`)
  }
}

// TWO LANES MAY NEVER SHARE A WORKTREE. Presence checks alone let two lanes carry the same `wt`
// through validation, and `parallel(LANES.map(...))` then dispatches both concurrently into one root
// — each told to edit and commit there. That is the two-writers-one-worktree failure this programme
// has already paid for twice: once as symptoms that looked like filesystem corruption (files edited
// at unclaimed timestamps, a deleted file reappearing, a bound silently changing), and once as 28
// duplicate beads when a second writer was resurrected into live shared state. The lock contract
// tells each implementer to build on whatever the worktree contains, which makes the collision
// silent by design — the second lane treats the first lane's half-finished edits as its own baseline.
// Refuse at dispatch, where it costs nothing.
{
  const seen = new Map()
  for (const l of LANES) {
    const prior = seen.get(l.wt)
    if (prior) {
      throw new Error(
        `lane-fanout: lanes "${prior}" and "${l.key}" both declare worktree ${l.wt}. Concurrent lanes ` +
        'must have disjoint worktrees — two implementers in one root overwrite each other silently, ' +
        'because the lock tells each to build on what it finds. Give them separate worktrees, or run ' +
        'them in separate invocations.',
      )
    }
    seen.set(l.wt, l.key)
  }
  const keys = new Set()
  for (const l of LANES) {
    if (keys.has(l.key)) throw new Error(`lane-fanout: duplicate lane key "${l.key}" — labels and telemetry would merge two lanes into one.`)
    keys.add(l.key)
  }

  // OVERLAPPING OWNED ROOTS ARE THE SAME COLLISION, DEFERRED TO LAND. Separate worktrees mean two
  // lanes cannot corrupt each other's files while they build, so this one survives every reviewer
  // and every verifier: each diff is correct in isolation. It fires at LAND time, on the
  // integration branch, after the whole run has been paid for — two independent rewrites of the
  // same files from the same base, handed to a lander with no authority to judge either. Refuse it
  // where the duplicate worktree is refused, for the cost of a string compare.
  //
  // An owned root is a list of path patterns, sometimes with prose around it. Reduce each entry to
  // the fixed path prefix before its first wildcard segment; two lanes collide when one prefix is
  // the other, or is a PATH prefix of it (segment-aligned, so crates/ab does not collide with
  // crates/a).
  // Canonicalise before compare: `./backend/crates/foo` and `backend/crates/foo` must collide.
  // Collapse `.` segments; reject `..` rather than silently rewriting ownership.
  const canonicalizeOwnedPrefix = (t) => {
    const segs = []
    for (const seg of String(t).split('/')) {
      if (seg.includes('*')) break
      if (!seg || seg === '.') continue
      if (seg === '..') {
        throw new Error(
          `lane-fanout: owned path contains '..' (${JSON.stringify(t)}) — refuse rather than collapse ` +
          'ownership across directories.',
        )
      }
      segs.push(seg)
    }
    return segs.join('/').replace(/\/+$/, '')
  }
  const ownedPrefixes = (owned) => String(owned)
    .split(/[\s,]+/)
    .filter(Boolean)
    // Path-shaped tokens only. Comparing every word would refuse two lanes for sharing "the".
    .filter((t) => t.includes('/') || t.includes('*') || /\.\w+$/.test(t))
    .map(canonicalizeOwnedPrefix)
  const pathOverlap = (a, b) => a === b || a === '' || b === '' || a.startsWith(`${b}/`) || b.startsWith(`${a}/`)
  const roots = LANES.map((l) => ({ key: l.key, owned: l.owned, prefixes: ownedPrefixes(l.owned) }))
  for (const r of roots) {
    // A guard that examines zero subjects must FAIL. An owned root naming no path cannot be
    // compared with anything, and it is also useless as the reviewer's IN-SCOPE PATHS list.
    if (!r.prefixes.length) {
      throw new Error(
        `lane-fanout: lane "${r.key}" declares an owned root with no path in it (${JSON.stringify(r.owned)}). ` +
        'The overlap check would examine nothing and pass, and the reviewer would be handed prose as ' +
        'its scope list. Name the paths.',
      )
    }
  }
  for (let i = 0; i < roots.length; i++) {
    for (let j = i + 1; j < roots.length; j++) {
      const hit = roots[i].prefixes
        .flatMap((a) => roots[j].prefixes.map((b) => [a, b]))
        .find(([a, b]) => pathOverlap(a, b))
      if (hit) {
        throw new Error(
          `lane-fanout: lanes "${roots[i].key}" and "${roots[j].key}" declare OVERLAPPING owned roots ` +
          `(${hit[0] || '<repo root>'} vs ${hit[1] || '<repo root>'}). Separate worktrees keep them from ` +
          'corrupting each other while they build, so nothing before LAND can see this — the collision ' +
          'arrives on the integration branch after every reviewer has passed, as two independent ' +
          'rewrites of the same files from the same base. Narrow one lane, or run them in sequence.',
        )
      }
    }
  }
}

const BASE_LOCK = `
=== LOCK CONTRACT (binding; violation = rejected work) ===
GIT — these caused a real multi-agent collision in the Bun rewrite and again in this program:
  NO stash / stash pop / reset (any mode) / checkout <branch> / rebase / merge / clean
  NO push, NO force-push, NO git worktree add|remove, NO branch creation.
  PERMITTED: git status, git diff, git log, 'git add <explicit path>', 'git commit' of those paths.
  Commit ONLY inside your owned root. If the worktree already contains work, BUILD ON IT — you may
  not reset or revert it. Fix forward.
BUILD:
  Run from the WORKTREE ROOT. Never 'cd backend'. Never a bare 'cargo test' or --workspace.
  Scope every invocation: cargo test --locked --manifest-path backend/Cargo.toml -p <pkg> ...
  PostgreSQL-backed targets: tools/ci/cargo_needs_postgres.sh --only <name> --num-threads=1
  *** NEVER pass --workflow-only. Dark targets carry in_workflow_postgres_job=false, so it selects
      ZERO targets and exits 0. That is a FALSE GREEN, not a pass. Always --only. ***
  sqlx::query! is COMPILE-TIME checked against a live schema; SQLX_OFFLINE=true compiles with no
  database (offline cache committed at backend/.sqlx/). A hand-made createdb will NOT work — it
  lacks the role topology and fails at compile time with 'role "anonymous" does not exist'.
CHANGE DISCIPLINE:
  Minimal and mechanical. Do not refactor, tidy or rename anything near the fix. Improving while
  fixing is what previously broke a lane into 99 errors across 8 crates it never opened.
  If you need a paragraph-long comment to justify a workaround, the code is wrong — fix the code.
NEVER WEAKEN THE ORACLE:
  No deleted tests, no #[ignore], no relaxed or loosened assertions, and above all NEVER make a
  test pass by conforming it to the defect. Multiple lanes in this program were rejected for
  exactly that, and in each case the "green" test was hiding a live production outage.
AN ENFORCEMENT MUST BE ABLE TO SEE ITS SUBJECT
RUN WHAT CI RUNS, BEFORE YOU REPORT DONE:
  A defect that CI catches and the lane did not is a HARNESS failure, not a CI success. The lane had the
  same tree, the same commands and more context; CI just had a checklist. The asymmetry is that accept
  criteria are prose and CI is commands, so the lane satisfies a sentence while CI executes a gate.
  Before status=done, find the commands CI will actually run over the files you touched -- read
  .github/workflows/ci.yml rather than guessing -- and RUN THEM. At minimum, for the paths in your diff:
  the formatter, the linter with the repo's own flags, the unit target, and any repo gate whose name
  matches your area. Put the exact command lines and their exit codes in verification.commands.
  If a gate cannot run locally, say WHICH and WHY there, rather than omitting it and reporting green.
  A lane that reports done without having run the gates has reported an intention, not a result.
  TESTS NAMED *_as_runtime_role.rs NEED A DATABASE, and a plain cargo test does NOT give them one --
  it fails ALL of them in about 0.01s, including tests that were passing, which reads like your
  change broke everything when nothing ran at all. The invocation takes the repo root as its FIRST
  argument and is easy to get wrong three times in a row:
      tools/lanes/pgtest.sh "$PWD" cargo test -p <crate> --test <name>
  Without the leading repo root it tries to lstat a file named cargo and exits having run nothing.
  A rule that cannot be followed without tribal knowledge is a rule that gets skipped, so the exact
  line is written here rather than left in an ADR.

A TEST THAT BUILDS ITS OWN SUBJECT MEASURES THE STUB, NOT THE DEPLOYMENT:
  The sharper form of the rule above, and the one that actually shipped. A suite proved a dispatch
  derivation TOTAL over every target in the contract -- six tests, all green, none of them naming a
  target so none of them able to go stale. At the same moment the production composition root
  registered ZERO of those thirteen targets. Both facts were true, because the tests constructed
  their own registry from stub ports and then measured the thing they had just built.
  The claim was about the DEPLOYED registry. The control could see a subject; it could not see THAT
  subject. So when your evidence is a test, say plainly which of these it is:
    - MECHANISM: the thing derives / fails closed / refuses the wrong payload. Stubs are correct
      here, and a stub is the only way to test a fourteenth target that does not exist yet.
    - WIRING: the composition root actually installs it. This one must drive the REAL constructor
      -- "super::the_production_fn(...)" -- and must fail when a registration line is deleted.
  A condition phrased as "X does not require hand-written Y per action" is a WIRING claim. Mechanism
  evidence does not close it, however total the mechanism test is. If your redBaseline was produced
  by deleting a line from a test fixture rather than from the composition root, you have proved the
  fixture.:
  If your change adds or modifies a gate, check, census, guard or invariant, answer TWO questions
  in writing BEFORE you build it, and put the answers in enforcementPlacement:
    (1) WHERE does it run in the sequence, and does its subject EXIST yet at that point?
    (2) What is the FINEST distinction its data source can express?
  Both have already shipped as no-ops in this program. A canonical-writer census was placed in a
  reconcile script that runs BEFORE migrations, so it matched zero tables and its REVOKE loop
  iterated nothing in every automated path — and the lane recorded "succeeds on a bare cluster" as a
  feature, which is exactly how the no-op hid. Separately, a database-capability control was
  specified to enforce per-CRATE ownership, but every crate connects as the same role (console_rt),
  so the finest distinction available to it was per-ROLE and the crate boundary was never drawn.
  A check that never runs and a check that runs blind both exit 0. Neither shows up in a test count.
  THEREFORE: "examined zero subjects" MUST be a FAILURE, never a pass. And never claim a control
  covers a distinction its data source cannot express — say what it actually enforces, and name the
  residual gap in followUps.
PERIPHERALS ARE PART OF THE CHANGE, NOT A FOLLOW-UP:
  A change is not done when the code compiles. Before you report done, find everything that
  DESCRIBES the behaviour you changed and bring it with you:
    - the module doc (//! and ///) of every file you touched, especially any comment that
      ENUMERATES something you just made total, or claims a property you just changed;
    - registries, rosters, baselines and ratchets that name what you added or removed;
    - the bead / issue text, if the change makes its description wrong;
    - any doc under docs/** that states the thing you changed as fact.
  Docs here rot in ONE direction: they describe holds already lifted and problems already fixed, so
  a stale doc reads as a live constraint and someone re-solves a solved problem. A module doc that
  says "the three ways X can happen are each pinned separately" after you found a fourth is not an
  inaccuracy, it is a false claim about a control.
  SCOPE RULE, same as everywhere else: update the peripherals you OWN; for a leased one, report the
  exact edit in followUps. Never leave a doc contradicting the code you just shipped, and never
  silently widen scope to fix a doc you were not given.

  MECHANICAL, AND NOT OPTIONAL — some peripherals are GENERATED, and prose about keeping things
  current does not update a checksum. A change that is otherwise entirely correct fails CI because a
  file some script writes is now stale.
  DO NOT work from a list of generators. The first version of this clause named exactly one (the
  documentation manifest) and the very next lane was failed by a different one (the first-party BUCK
  faces). A list of the faces you have been burned by is not a rule, it is a record of your own
  history -- and this lock says two paragraphs down that the third spelling means the mechanism is
  wrong. The mechanism is: REGENERATE, THEN ASK GIT.
      run every generator the repo exposes for the areas your diff touches, then:
      git status --porcelain          # ANY output = your commit is incomplete
  git is the oracle because it cannot be fooled by a face you did not think of. Two entry points
  worth knowing, neither of which is the whole set: tools/buck/preflight.sh (what CI's preflight job
  actually runs, covering every generated Buck face) and
  node scripts/console/generate-documentation-manifest.mjs --write.
  This is a POSTFLIGHT check: run it after your last edit, not before. It is here rather than left to
  the reviewer because it is decidable by a command, and a lane that can run the command has no
  business spending a review round on it.
THE THIRD SPELLING MEANS THE MECHANISM IS WRONG, NOT THE LIST:
  If you are fixing the SAME class of bug for the third time in a different spelling, stop patching
  and replace the mechanism. Measured: a gate hand-lexed Rust and was defeated by '} // end tests',
  then 'use path::{A, B}', then a char literal, a block comment and a raw string; its hand-written
  cfg rule was defeated by not(all(test)) and then by any(test, X). Each fix was correct and each
  left a sibling live, because the set of spellings is open-ended. In both cases a total primitive
  already existed (a real parser; has_table_privilege, which answers ownership, recursive
  membership, column grants and superuser in one call) and replacing the enumeration DELETED more
  code than it added. Before the third patch, ask: what already answers this question totally?
  Say so in followUps if the total answer needs a dependency or a leased file -- a precise request
  is a complete result.
TEST THE CONTROL BY EXECUTING IT, NOT BY READING IT:
  A contains()/substring assertion over a gate's own source text is not evidence the gate works. A
  reviewer inverted a census to 'IF leaked IS NOT NULL AND false THEN', killing it entirely, and all
  16 tests stayed green. Mutate the control itself and prove each mutation goes RED.
ROOT CAUSE, NOT SYMPTOM:
  Before editing, grep every caller of the function you are about to touch. One guard in the shared
  function beats a guard in each caller, and patching only the reported path leaves siblings broken.
CONTRACT TESTS ARE PART OF THE CHANGE:
  Before you edit behaviour, find every test that ENCODES the behaviour you are changing —
  including integration suites in other crates (backend/app/tests/** is the usual one). Changing a
  contract necessarily breaks the tests asserting the old contract; that is the change, not a
  regression. If such a test is OUTSIDE your owned root, STOP and report it in followUps with the
  exact file and assertions BEFORE you build. Do not silently break it, and do not abandon the
  work. This has been mis-scoped four times in this program: a lane authorised to fix a defect but
  forbidden the crate holding it, authorised to create a crate but forbidden the workspace
  manifest, and twice authorised to change a contract but forbidden the test that encodes it.
`

const LOCK = BASE_LOCK + (ARGS.lockExtra || '')

const BUILD_SCHEMA = {
  type: 'object',
  required: ['status', 'summary', 'filesChanged', 'redBaseline', 'verification', 'contractBreaches', 'enforcementPlacement', 'peripheralsUpdated'],
  properties: {
    status: { type: 'string', enum: ['done', 'partial', 'blocked'] },
    summary: { type: 'string' },
    filesChanged: { type: 'array', items: { type: 'string' } },
    redBaseline: { type: 'string', description: 'the failing test written FIRST and its exact failure output, before implementation' },
    verification: { type: 'string', description: 'EXACT commands run and EXACT pass/fail counts; an independent agent will re-run these' },
    commands: { type: 'array', items: { type: 'string' }, description: 'the verbatim commands an independent verifier should re-run' },
    contractBreaches: { type: 'string' },
    // Required, with an explicit n/a escape, so that OMITTING the answer is impossible rather than
    // merely discouraged. A prose clause in the lock can be skimmed; a schema field cannot.
    enforcementPlacement: {
      type: 'string',
      description:
        'If this change adds or modifies any gate/check/census/guard: WHERE in the sequence it runs and whether its subject exists at that point, and the FINEST distinction its data source can express. State how "examined zero subjects" fails. If the change adds no enforcement, write exactly: n/a - adds no enforcement.',
    },
    // Required for the same reason as enforcementPlacement: a lock clause can be skimmed, a schema
    // field cannot. Doc drift is invisible in a test count, which is exactly why it accumulates.
    peripheralsUpdated: {
      type: 'string',
      description:
        'Every doc/comment/registry/bead that DESCRIBED the behaviour you changed: what you updated (you own it), and what you are reporting instead (leased). Include module docs whose claims your change invalidates. If nothing described this behaviour, write exactly: n/a - nothing described this behaviour, and say how you checked.',
    },
    followUps: { type: 'string' },
  },
}

const REVIEW_SCHEMA = {
  type: 'object',
  required: ['verdict', 'findings'],
  properties: {
    verdict: { type: 'string', enum: ['reject', 'accept_with_findings', 'accept'] },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        required: ['severity', 'claim', 'failureScenario', 'location', 'provenByExecution', 'ownerLease'],
        properties: {
          severity: { type: 'string', enum: ['blocker', 'major', 'minor'] },
          claim: { type: 'string' },
          failureScenario: { type: 'string' },
          location: { type: 'string' },
          // Severity alone is the wrong convergence signal, measured: a round converged on
          // blockers=0 while both reviewers returned accept_with_findings carrying SIX distinct
          // fail-opens they had each PROVEN BY RUNNING -- a census blind to the table owner, a
          // partial-roster shrink that passed, a contains() wiring check defeated by a '#', a
          // silent Docker-absent skip that certified an unexecuted census as green. Every one was
          // labelled "major" and every one was waved through. What separates those from prose
          // is not severity, it is whether the reviewer OBSERVED the failure.
          provenByExecution: {
            type: 'boolean',
            description: 'TRUE only if YOU ran a command and OBSERVED the failure -- you have the output. Reasoning from source, however sound, is FALSE. Be strict: this field decides whether the lane rebuilds.',
          },
          // The lease carve-out is a rule about WHOSE work it is, so it must survive the severity
          // it was filed under; a lease item labelled blocker would otherwise make every
          // test-adding lane permanently unconvergeable.
          ownerLease: {
            type: 'boolean',
            description: 'TRUE if this is a companion edit the INTEGRATION OWNER must land (a leased path), not a defect in the lane. These never block convergence at any severity.',
          },
        },
      },
    },
    oracleWeakened: { type: 'boolean' },
    scopeCreep: { type: 'boolean' },
  },
}

// An independent re-runner. Lanes self-report their own green; nobody checked that in the first
// four runs, and the integration owner had to re-run everything by hand afterwards. This makes the
// check part of the pipeline.
const VERIFY_SCHEMA = {
  type: 'object',
  // A GREEN MUST BE ANSWERED FOR, NOT MERELY ASSERTED. reproduced + contradictsClaim say the
  // verifier ran something and agreed with it. They do not say WHAT it ran, whether any command
  // selected zero tests and still exited 0, or whether the suite still proves as much as it did.
  // Oracle integrity is the most common rejection cause in this programme and a STANDING review
  // lens, yet this schema let a verifier certify a green without ever answering it — and a field
  // that is optional is a field that gets skipped. Requiring the command list is the same
  // discipline as `commands` on the build side: a claim nobody can re-run is a claim.
  required: ['reproduced', 'actualResults', 'discrepancies', 'contradictsClaim', 'headSha', 'falseGreenRisk', 'commandsRun', 'oracleIntact'],
  properties: {
    // WHAT was verified, not just whether. Landing cherry-picks a worktree AFTER review finishes,
    // and "clean working tree" is not a binding: a commit added after the reviewers finished is
    // clean too, and would land as if reviewed. This is the immutable head the review round
    // actually applies to, captured by the one agent that is read-and-run only.
    headSha: { type: 'string', description: 'output of `git rev-parse HEAD` in the worktree, verbatim. This is the head the review round binds to; landing refuses anything else.' },
    reproduced: { type: 'boolean', description: 'true only if YOU ran the commands and saw the claimed results' },
    actualResults: { type: 'string', description: 'the exact output you observed, not what was claimed' },
    discrepancies: { type: 'string', description: 'any difference between claimed and observed, or "none"' },
    // The VERIFIER decides whether its findings matter -- not a regex over its prose. A previous
    // version required discrepancies to be the literal string "none", so a conscientious verifier
    // writing "four, all minor; none contradicts the verdict" failed the check and the lane was
    // sent back for another build round. Convergence was effectively unreachable whenever the
    // verifier ran: every lane with one reported converged=false while every review-only lane
    // reported true. That defect manufactured rebuild rounds, which are the dominant cost here.
    contradictsClaim: { type: 'boolean', description: 'TRUE only if what you observed CONTRADICTS the claimed result - a count that differs, a command that failed, a false green. Cosmetic differences (line numbers, timings, wording) are FALSE.' },
    falseGreenRisk: { type: 'string', description: 'did any command select ZERO tests and still exit 0? Answer for every command you ran; "none" is a valid answer, silence is not.' },
    commandsRun: {
      type: 'array',
      items: { type: 'string' },
      description: 'the commands you ACTUALLY ran, verbatim, in the order you ran them. Not the commands you were given — the ones you executed. An empty list means you verified nothing.',
    },
    oracleIntact: {
      type: 'boolean',
      description: 'TRUE only if the suite still proves as much as it did: no test deleted, no #[ignore] added, no assertion loosened, and no test made to pass by conforming it to the defect. FALSE if you observed any of those OR could not tell.',
    },
  },
}

// STANDING lenses run on EVERY lane, every round. They are not defaults.
//
// They used to be defaults -- `ARGS.lenses || [...]` -- and every single invocation of this harness
// passed `lenses`, so the fallback never once evaluated. Oracle integrity is the most common
// rejection cause in this program and it had never been reviewed for; it was only ever caught
// incidentally by a custom lens that happened to look. A default that is always overridden is not a
// default, it is dead code that reads as coverage.
// MODEL TIER: FRONTIER WHERE THE DECISION IS, CHEAP WHERE THE WORK IS.
//
// Every agent in this directory ran at the session model, which is the expensive one. Cursor's
// swarm measured the same worker fleet at $9,373 with a frontier model and $411 with a cheap one
// behind a frontier planner -- 23x -- on the argument that "few moments in a large task genuinely
// require frontier intelligence, such as the original decomposition, the design decisions, and
// certain trade-offs." This programme has now lost two runs to account limits, one of them 117
// agents deep, so this is not a theoretical saving.
//
// The line we draw is NOT "cheap for reads, expensive for writes". It is:
//
//   A CHEAPER TIER IS ALLOWED ONLY WHERE AN INDEPENDENT STRONGER PASS AUDITS THE RESULT.
//
// That is the same defence-in-depth argument as decorrelated review lenses: a weaker first pass is
// fine when a stronger one adjudicates it, and is NOT fine when its output is the final word.
// So: batched fan-out that feeds a challenge/reconcile phase may run cheap; anything whose verdict
// is terminal -- the adversarial reviewers, the independent verifier, the single writer, and any
// re-derivation of a dependency edge, where a reversed answer silently reschedules everything --
// stays on the inherited model.
// NOT APPLIED TO THIS HARNESS'S OWN AGENTS, deliberately, and recorded because the first version of
// this block defined a WORKER constant and used it NOWHERE -- a rule written, documented, believed,
// and reachable by nothing, committed in the very change that added the rule. That is the defect the
// preflight beside this file exists to catch, and it caught it.
//
// A build agent's output DOES qualify under the rule above: adversarial reviewers and an independent
// verifier audit it. What stops it is arithmetic, not principle. This harness converges by ROUNDS,
// and a round costs a full build plus every reviewer plus the verifier, so a tier that needs even one
// extra round costs more than it saves. Unlike Cursor's workers, which execute a planner's explicit
// instructions, a lane here is handed an open-ended brief. Applying it needs a measurement nobody has
// made: rounds-to-converge per tier on the same briefs. Until then, the harnesses tiered are the ones
// whose fan-out is a mechanical read feeding a stronger pass, where a wrong answer costs one re-read.

// TRIAL ONE LANE BEFORE COMMITTING THE FLEET.
//
// Bun's Zig-to-Rust port ran ONE implementer plus TWO adversarial reviewers over THREE files, and
// only then scaled to 64 agents across 4 worktrees. This harness has always dispatched every lane
// cold, and it has cost real runs: a wave where the brief was wrong in the same way for all four
// lanes is four wasted lanes, not one.
//
// `trial` names a lane key that must CONVERGE before the rest are dispatched. It is not a
// smoke test -- it is the same full build/review/verify cycle, so what it proves is that the LOCK,
// the accept criteria and the review lenses actually work against this tree, which is the part a
// wave gets wrong identically across every lane.
//
// Deliberately opt-in. For two independent lanes the serialisation costs more than it saves; the
// value appears when the lanes share a brief shape, which is exactly when they fail together.

// Judges, verifiers and writers deliberately omit `model` so they inherit the session's.

const STANDING_LENSES = [
  'MAINTAINABILITY / COST OF CARRY — every other lens asks whether the change is CORRECT. This one asks what it costs to keep, and its default verdict is DELETE. (a) SPRAWL: does this add a doc, a crate, a script, a config, a workflow step or a process that duplicates one that exists? A second file describing the same fact is a future contradiction, not documentation. Name the existing thing it should have extended. (b) COMMENT BLOBBING: is there a paragraph of prose where a name would do? A comment that restates the code is noise that goes stale independently; a comment earning its place explains WHY, names a measurement, or records a rejected alternative. Twenty lines of comment over five lines of code is a defect in this repository, not thoroughness. (c) IDIOM: would a competent Rust/JS reader of this repo write it this way, or is it this author\'s private dialect? Hand-rolled parsing where a library exists, and enumerations where the language has a total construct, are the two that recur here. (d) AUTOMATION: is a human being asked to remember something a command could decide? If the accept criteria contain a step a script could run, that step WILL be skipped eventually. (e) UNDOCUMENTED-BUT-SHOULD-BE: the inverse of sprawl. A non-obvious constraint, a measured number, or a deliberate asymmetry that exists only in the author\'s head is undocumented, and the next lane will \'simplify\' it away. Report the NET line count of prose and config this change adds. A change that adds more explanation than behaviour needs a reason.',
  'CORRECTNESS + ORACLE INTEGRITY — does the change address the root cause, and does the suite still prove as much as before? Hunt for tests conformed to defects and assertions that would pass even if the behaviour were broken. Pick the load-bearing assertion, break the code it guards, and say whether it actually goes RED.',
  'PERIPHERAL DRIFT — read the diff, then go looking for what it made WRONG somewhere else. Does any module doc, /// comment, registry, roster, baseline, docs/** page or bead text still describe the behaviour as it was before this change? Pay closest attention to comments that ENUMERATE ("the three ways X can happen", "these are the cases") next to code this change made total or extended — those are false claims about a control, not stale prose. Verify the build agent\'s peripheralsUpdated field against the actual tree rather than trusting it, and check the reverse direction too: a doc updated to describe something the code does NOT do is worse than a stale one. Leased peripherals correctly reported in followUps are ownerLease=true, not defects.',
  'ENFORCEMENT PLACEMENT — for every gate/check/census/guard this change touches, ignore whether its LOGIC is right and ask only whether it can SEE its subject. (a) Where does it run in the sequence, and does its subject exist yet at that point? (b) What is the finest distinction its data source can express, and does the change claim a finer one? (c) Is the rule TOTAL over its domain, or is it an enumeration of spellings that a reviewer can always add one more to? If the change closes named cases rather than making the class unrepresentable, name the total primitive it should have used instead. (d) Does "examined zero subjects" fail, or pass? (d) Is it tested by EXECUTING it, or by a contains() over its own source text — mutate the control and check the tests go RED. Both failure modes have shipped here: a census that ran before migrations existed, and a per-crate rule enforced by a data source that only distinguishes roles. Verify the answers in enforcementPlacement rather than trusting them.',
]

const LENSES = [
  ...STANDING_LENSES,
  ...(ARGS.lenses || [
    'BLAST RADIUS — does any public wire contract, stored format, or authorization outcome change shape? Who can now do what they could not before, or vice versa? Consider generated clients, rows already written under the old format, and callers in other crates.',
  ]),
]

// --- telemetry -------------------------------------------------------------
// Measured across six runs of this harness: ~130 agents, ~12M tokens, ~5.7h wall-clock, at a ratio
// of 3.67 checkers per build. Wall-clock divided by builds is 7-13 min per ROUND, and reviews run
// in parallel, so a round costs roughly one build plus one review wave.
//
// That makes ROUNDS the scarce resource, not tokens. And an audit of why rounds were spent found
// most early ones went to defects in the HARNESS and the BRIEF, not in the lane's code: reviewers
// diffing HEAD~1, findings discarded with no feedback edge, empty-diff auto-reject, reviewers not
// told which paths were owner-leased, a lane authorised to create a crate but forbidden the
// workspace manifest. Each of those cost a full build cycle per affected lane.
//
// The lever follows directly: another reviewer is cheap, another BUILD ROUND is expensive. So
// classify what each round was actually spent on, and let the numbers say whether the next
// improvement belongs in the brief or in the code.
const TELEMETRY = { rounds: [], startedAt: null, checkers: [] }

// What holds a lane open. Severity is the reviewer's opinion; provenByExecution is a fact about
// whether they watched it fail. A proven fail-open holds the lane regardless of the label it was
// filed under, and an owner lease releases it regardless -- because a lease is a statement about
// whose work it is, not about how bad it is.
function isBlocking(f) {
  if (f.ownerLease === true) return false
  if (f.severity === 'blocker') return true
  return f.severity === 'major' && f.provenByExecution === true
}

function classifyRejection(blockers, weakened, verifierOk, status, checkerDied) {
  // Coarse, deliberately: the point is to see the SHAPE of wasted rounds, not to be precise.
  const text = blockers.map((b) => `${b.claim} ${b.location}`).join(' ').toLowerCase()
  // A round lost to a dead checker is an INFRASTRUCTURE cost, not a defect in the lane's claim.
  // Filing it as unreproducible-claim would tell the human to fix a brief that was never wrong.
  if (checkerDied) return 'checker-died'
  if (!verifierOk) return 'unreproducible-claim'
  if (weakened) return 'oracle-weakened'
  if (/outside the authorised|in-scope path|scope list|not in scope/.test(text)) return 'scope-brief-defect'
  if (/executes nowhere|ratchet|baseline|ci\.yml|workflow step|lease/.test(text)) return 'owner-lease'
  if (status && status !== 'done') return 'incomplete'
  if (blockers.length) return 'code-defect'
  return 'unclassified'
}

// --- cross-lane defect ledger: Bun's "fix the generator" -------------------
const DEFECT_LEDGER = []

function recordDefects(laneKey, blockers, weakened) {
  for (const b of blockers) {
    const claim = (b.claim || '').trim()
    if (!claim) continue
    const key = claim.slice(0, 80).toLowerCase()
    if (DEFECT_LEDGER.some((d) => d.key === key)) continue
    DEFECT_LEDGER.push({ key, laneKey, claim, location: b.location || '' })
  }
  if (weakened && !DEFECT_LEDGER.some((d) => d.key === '__oracle__')) {
    DEFECT_LEDGER.push({
      key: '__oracle__',
      laneKey,
      claim: 'A reviewer judged that a change made the suite prove LESS than before (oracleWeakened=true).',
      location: '',
    })
  }
}

function ledgerBlock(selfKey) {
  const others = DEFECT_LEDGER.filter((d) => d.laneKey !== selfKey)
  if (!others.length) return ''
  return `
=== DEFECT CLASSES ALREADY CAUGHT IN OTHER LANES THIS RUN ===
Found by adversarial review of a DIFFERENT lane. They are CLASSES, not incidents — the same mistake
is available to you. Read them as things not to do.
${others.map((d, i) => `${i + 1}. (lane ${d.laneKey}) ${d.claim}${d.location ? `\n   seen at: ${d.location}` : ''}`).join('\n')}
`
}

function reviewPrompt(l, lens) {
  return `You are an ADVERSARIAL CODE REVIEWER. Your ONLY job is to find bugs and reasons this change
does not work. You are NOT told the author's reasoning and must not go looking for it.
Assume the code is WRONG until the diff proves otherwise.

  cd ${l.wt}
  git --no-pager diff ${TIP}
  git status --porcelain

*** Diff against ${TIP} exactly as above. Do NOT use 'git diff HEAD~1' — that shows an unrelated
    earlier commit's own contents rather than the change under review. If what you are reading has
    nothing to do with this lane's subject, you have the wrong diff; re-run the command above. ***

An EMPTY diff is NOT automatically a rejection. Some work is a proof that no change was needed.
Judge whether the reported evidence supports that claim; reject only if it does not.

YOUR REVIEW LENS: ${lens}

Every finding needs a CONCRETE failure scenario: specific inputs or state producing wrong
behaviour. A finding without one is a label, not a finding, and is worthless.

BLOCKER-level specifically:
 - any test made to pass by conforming it to a defect instead of fixing the defect
 - any deleted test, added #[ignore], or loosened assertion
 - an expectation matching observed output rather than the intended contract, especially anything
   environment-dependent (collation, locale, timezone, hash or iteration order)
 - a security or authorization decision that FAILS OPEN on error, absence, staleness or
   unavailability where the contract requires fail-closed
 - a claimed invariant with no test, or a test that would still pass if the invariant were broken
 - a backward-compatibility shim with no test proving it reads the legacy form
 - edits outside the IN-SCOPE PATHS below

*** NOT A BLOCKER — INTEGRATION-OWNER LEASES ***
Some files are deliberately withheld from every lane and are applied by the integration owner:
  docs/program/executed-tests-baseline.json, .github/workflows/**, tools/ci/postgres-cargo-map.json,
  backend/openapi/**, lockfiles, backend/crates/platform/db/migrations/**
A change that ADDS TESTS will therefore, by construction, leave the executed-tests ratchet red and
its new binaries unwired until the owner lands the companion edit. That is EXPECTED and is NOT a
defect in the lane's work. Report it with ownerLease=true and the exact companion edit required —
never as grounds to reject. Rejecting on it makes any test-adding lane permanently unconvergeable,
which would penalise exactly the changes that add the most coverage.
Do still reject if the lane EDITED one of those paths itself.

*** TWO FIELDS DECIDE WHETHER THE LANE REBUILDS — SET THEM DELIBERATELY ***
provenByExecution: true ONLY if you ran a command and OBSERVED the failure, and you have the output
  to show. A finding you reasoned out from the source is false here no matter how confident you are.
  A "major" you PROVED forces another build round; a "major" you argued does not. This exists
  because a previous round converged clean while both reviewers were holding six separately proven
  fail-opens, each filed as an unproven-looking "major" and each waved through.
ownerLease: true for companion edits belonging to the integration owner, per the section above.
  These never block at any severity, so a lease item is never a reason to withhold convergence.
If you proved a fail-open by running it, say so with provenByExecution=true even when you are
otherwise willing to accept the change. Your verdict and this field are independent.

=== IN-SCOPE PATHS (authorised) ===
${l.owned}
This is SCOPE, not rationale. It says which files were legitimately in play and nothing about
whether the author got them right. Edits inside are not scope creep by location alone but can
still be wrong. Edits outside are a contract breach — report as blockers.

Set oracleWeakened=true if the suite now proves LESS than before. Default to rejecting when unsure.`
}

function verifyPrompt(l, fix) {
  const cmds = (fix.commands && fix.commands.length ? fix.commands : []).map((c) => `  ${c}`).join('\n')
  return `You are an INDEPENDENT VERIFIER. You do not review code and you do not read the author's
reasoning. Your single job is to RE-RUN what was claimed and report what ACTUALLY happens.

  cd ${l.wt}
  git rev-parse HEAD          <- report this verbatim as headSha, FIRST, before anything else

That SHA is what this whole review round binds to: the landing step will refuse to cherry-pick this
worktree if its head has moved since you read it. Report what you actually saw, never a guess.

CLAIMED VERIFICATION:
${fix.verification}

${cmds ? `COMMANDS TO RE-RUN VERBATIM:\n${cmds}` : 'The author listed no explicit commands. Derive them from the claimed verification above and say so.'}

RULES:
 - RUN the commands. Do not reason about whether they would pass. Reproduced=true only if you
   personally observed the result.
 - Report the EXACT output you saw, including counts, not the counts that were claimed.
 - *** FALSE-GREEN CHECK: if any command reports running ZERO tests and still exits 0, that is a
     false green — say so loudly in falseGreenRisk. In this repository --workflow-only selects zero
     dark targets and exits 0, which is exactly this trap. ***
 - If a command fails for an ENVIRONMENTAL reason (missing dependency, database not provisioned,
   Docker down), say so explicitly rather than reporting the change as broken.
 - Do NOT edit any file. You are read-and-run only.

*** THREE FIELDS DECIDE WHETHER THIS GREEN COUNTS. A green you cannot answer for is not a green. ***
 commandsRun:     every command you ACTUALLY executed, verbatim and in order. Not the list you were
                  handed — the one you ran. An empty list means nothing was verified, and the lane
                  rebuilds.
 falseGreenRisk:  answer it for every command, not just the suspicious one. "none" is an answer.
 oracleIntact:    read the diff for deleted tests, added #[ignore], loosened assertions, and any
                  expectation rewritten to match observed output instead of the intended contract.
                  TRUE only if the suite still proves as much as before. If you could not tell,
                  that is FALSE — this is the most common rejection cause in this programme and
                  silence on it used to be free.

Report any difference between what was claimed and what you observed, however small.`
}

function buildPrompt(l, fb) {
  // A rejection MUST arrive with something actionable. A previous run rejected a lane on verifier
  // disagreement alone, rendered an empty "BLOCKERS:" list, and the lane reported: "the rejection
  // came with no blocker text, so I re-derived the weakest point of my own diff". It recovered by
  // luck. When there are no blockers, state the actual cause and fall back to lower-severity
  // findings so the lane always has a concrete starting point.
  let reasonLines = []
  if (fb) {
    if (fb.weakened) reasonLines.push('A reviewer set oracleWeakened=true: the suite now proves LESS than before. Restoring the oracle is the highest priority.')
    if (fb.verifierSaid) reasonLines.push(`The INDEPENDENT VERIFIER re-ran your commands and disagreed with your claimed result: ${fb.verifierSaid}`)
    if (fb.notDone) reasonLines.push(`You reported status="${fb.notDone}" rather than "done". If that is genuinely blocked, say so precisely in followUps; if it is finishable, finish it.`)
    if (!fb.blockers.length && !reasonLines.length) reasonLines.push('No blocker was recorded, which means the rejection came from a failed convergence check rather than a named finding. Re-derive the weakest point of your own diff and address it.')
  }

  const blockerText = fb && fb.blockers.length
    ? `BLOCKERS (must all be resolved):\n${fb.blockers.map((b, i) => `${i + 1}. [${b.severity}] ${b.claim}\n   where: ${b.location}\n   fails when: ${b.failureScenario}`).join('\n')}`
    : (fb && fb.lesser && fb.lesser.length
      ? `NO BLOCKERS were raised. Lower-severity findings, treat as the actionable list:\n${fb.lesser.map((b, i) => `${i + 1}. [${b.severity}] ${b.claim}\n   where: ${b.location}\n   fails when: ${b.failureScenario}`).join('\n')}`
      : 'NO named findings were recorded this round.')

  const feedback = fb
    ? `
=== ROUND ${fb.round}: THIS LANE WAS REJECTED ===
Your earlier change is already in the worktree. Amend it; do not revert wholesale.

WHY IT WAS REJECTED:
${reasonLines.map((r) => ` - ${r}`).join('\n')}

${blockerText}

Fix the CAUSE, not the symptom named. If two findings share a root, fix the root once.
Reviewers see ONLY your diff, never your reasoning — if a finding looks like a misunderstanding,
that is itself a signal the change is not self-evident. Make the code clearer rather than arguing.
If a finding names a path you may not touch, say so in followUps rather than editing it.
`
    : ''

  return `You are the IMPLEMENTER for lane "${l.key}"${l.bead ? ` (beads issue ${l.bead})` : ''}.
${feedback}${ledgerBlock(l.key)}
WORKTREE (yours alone, based on ${TIP}):
  ${l.wt}
Work ONLY there. Do not touch any other worktree or the primary checkout.

YOUR OWNED ROOT (the only place you may write):
  ${l.owned}

${l.brief}

ACCEPTANCE: ${l.accept}

METHOD — RED BASELINE FIRST, not optional:
 1. WRITE THE FAILING TEST FIRST. Encode the required behaviour as a test and RUN it. Capture the
    exact failure output — that is your redBaseline. A lane that implements first and tests after
    cannot demonstrate its test would have caught anything.
 2. Implement the smallest change that makes it pass.
 3. Re-run and report EXACT counts. Populate 'commands' with the verbatim commands you ran — an
    independent verifier will re-run them and compare against what you claim.
 4. MUTATION-CHECK your own new assertions: break the thing you just fixed, confirm the suite goes
    RED, then restore and confirm GREEN. An assertion that still passes when the behaviour is
    broken proves nothing. Report this in verification.
 5. Commit only your owned paths. Do not push.

Cold builds take several minutes; that is expected, not a hang.
If you cannot finish, report "partial" or "blocked" with the exact failure. An honest blocked
result is far more useful than a weakened test.

${LOCK}`
}

async function runLane(l) {
  let fb = null
  let last = { lane: l, fix: null, reviews: [], verify: null, blockers: [], rounds: 0, converged: false }

  // A lane whose implementer already finished gets REVIEW ONLY. Re-running it would duplicate
  // committed work and risk a second writer in a worktree, and its diff still needs review.
  if (l.reviewOnly) {
    const checks = await parallel(LENSES.map((lens) => () =>
      agent(reviewPrompt(l, lens), { label: `review:${l.key}`, phase: 'Review', schema: REVIEW_SCHEMA })))
    // Same death accounting as the main path: a review-only lane that lost a standing lens has not
    // been reviewed for the thing that lens exists to catch, and must not report converged.
    const deadStanding = checks
      .slice(0, STANDING_LENSES.length)
      .map((r, i) => (r ? null : STANDING_LENSES[i].split(' —')[0]))
      .filter(Boolean)
    const reviews = checks.filter(Boolean)
    const blockers = reviews.flatMap((v) => (v.findings || []).filter(isBlocking))
    const weakened = reviews.some((v) => v.oracleWeakened)
    // scopeCreep blocks for the same reason oracleWeakened does: the review schema carries a
    // dedicated flag, and the reviewer prompt defines an edit outside `l.owned` as a contract
    // breach. A reviewer that sets the flag without ALSO restating it as a blocking finding was
    // being ignored, so a lane could edit an unowned root, converge, and be handed to the lander.
    // The flag is the reviewer's verdict; requiring them to say it twice is how it gets lost.
    const creep = reviews.some((v) => v.scopeCreep)
    recordDefects(l.key, blockers, weakened)
    TELEMETRY.checkers.push({ dispatched: LENSES.length, returned: reviews.length, deadLenses: deadStanding })
    const converged = blockers.length === 0 && !weakened && !creep && deadStanding.length === 0
    log(`${l.key}: review-only -> ${reviews.length}/${LENSES.length} reviewers, ${blockers.length} blocker(s)${weakened ? ', ORACLE WEAKENED' : ''}${deadStanding.length ? `, STANDING LENS DIED: ${deadStanding.join(', ')}` : ''}`)
    return { lane: l, fix: l.priorResult || null, reviews, verify: null, blockers, rounds: 0, converged }
  }

  for (let round = 1; round <= MAX_ROUNDS; round++) {
    const sfx = round > 1 ? `#${round}` : ''

    // Agent death is routine at this scale; retry once before giving up on the round.
    let fix = await agent(buildPrompt(l, fb), { label: `build:${l.key}${sfx}`, phase: 'Build', schema: BUILD_SCHEMA })
    if (!fix) {
      log(`${l.key}: implementer died in round ${round}, retrying once`)
      fix = await agent(buildPrompt(l, fb), { label: `build:${l.key}${sfx}r`, phase: 'Build', schema: BUILD_SCHEMA })
    }
    if (!fix) {
      log(`${l.key}: implementer died twice in round ${round} — abandoning lane`)
      last.rounds = round
      break
    }

    // Two adversarial readers plus, when there is something to falsify, one independent re-runner.
    //
    // The verifier exists to test a CLAIMED green by re-running the lane's own commands. A lane
    // reporting "partial" or "blocked" is claiming nothing, so there is nothing to falsify and the
    // round cannot converge regardless. Running it anyway re-executes full test suites for no
    // decision value: in one measured phase, two consecutive rounds converged zero lanes, so six
    // verifiers re-ran suites whose result could not have changed any outcome. Reviews still run
    // on every round -- a rejected round's findings are exactly what the next build needs.
    const claimsGreen = fix.status === 'done'
    const checks = await parallel([
      ...LENSES.map((lens) => () => agent(reviewPrompt(l, lens), { label: `review:${l.key}${sfx}`, phase: 'Review', schema: REVIEW_SCHEMA })),
      ...(claimsGreen ? [() => agent(verifyPrompt(l, fix), { label: `verify:${l.key}${sfx}`, phase: 'Review', schema: VERIFY_SCHEMA })] : []),
    ])
    // A DEAD REVIEWER IS NOT AN ABSENT FINDING. parallel() resolves a died agent to null, and
    // .filter(Boolean) used to make it disappear -- so a round could converge on one surviving
    // reviewer while three others died, and the log said nothing. That is a false-green path, and
    // it fired repeatedly in one measured session: session-limit kills took 4 of 7 agents from one
    // run, 3 of 3 from another, and an adjacent workflow's synthesis step silently ran on 2 of 3
    // inputs. Convergence must know how many eyes actually reported.
    //
    // Index order from parallel() matches the dispatch order, so a null identifies WHICH lens died.
    // STANDING lenses are non-negotiable: if one of them did not report, the lane has not been
    // reviewed for oracle integrity, enforcement placement or peripheral drift, and it may not
    // converge no matter what the survivors said. A custom lens dying is logged and tolerated,
    // because forcing a whole rebuild round over it costs more than it saves.
    const reviewSlots = checks.slice(0, LENSES.length)
    const deadLenses = reviewSlots
      .map((r, i) => (r ? null : LENSES[i].split(' —')[0].split(' ').slice(0, 3).join(' ')))
      .filter(Boolean)
    const deadStanding = reviewSlots
      .slice(0, STANDING_LENSES.length)
      .map((r, i) => (r ? null : STANDING_LENSES[i].split(' —')[0]))
      .filter(Boolean)
    const reviews = reviewSlots.filter(Boolean)
    const verify = claimsGreen ? (checks[LENSES.length] || null) : null
    if (!claimsGreen) log(`${l.key}: round ${round} reported "${fix.status}" — verifier skipped, nothing green to falsify`)
    if (deadLenses.length) {
      log(`${l.key}: round ${round} — ${reviews.length}/${LENSES.length} reviewers returned; DIED: ${deadLenses.join(', ')}`)
    }
    if (deadStanding.length) {
      log(`${l.key}: round ${round} CANNOT CONVERGE — standing lens(es) never reported: ${deadStanding.join(', ')}`)
    }

    const blockers = reviews.flatMap((v) => (v.findings || []).filter(isBlocking))
    const weakened = reviews.some((v) => v.oracleWeakened)
    // scopeCreep blocks for the same reason oracleWeakened does: the review schema carries a
    // dedicated flag, and the reviewer prompt defines an edit outside `l.owned` as a contract
    // breach. A reviewer that sets the flag without ALSO restating it as a blocking finding was
    // being ignored, so a lane could edit an unowned root, converge, and be handed to the lander.
    // The flag is the reviewer's verdict; requiring them to say it twice is how it gets lost.
    const creep = reviews.some((v) => v.scopeCreep)
    recordDefects(l.key, blockers, weakened)
    TELEMETRY.checkers.push({ dispatched: LENSES.length + (claimsGreen ? 1 : 0), returned: reviews.length + (verify ? 1 : 0), deadLenses })

    // A lane is not green because it says so. The verifier must have reproduced it.
    // The verifier judges whether its own findings contradict the claim; contradictsClaim is a
    // required field, so there is no prose to parse and nothing to fall back to.
    //
    // A DEAD VERIFIER IS NOT A PASSED VERIFICATION. This used to read `!verify || (...)`, so a
    // verifier killed by a session limit made verifierOk TRUE and the lane converged while the log
    // said "independently re-verified" -- the exact false-green class this harness exists to
    // remove, and worse than the rest because the log asserts the opposite of what happened.
    // A dead CUSTOM lens is tolerated because four other eyes still read the diff; the verifier has
    // no such substitute. It is the ONLY thing that re-runs a claimed green, so its death leaves
    // the claim untested and the lane must rebuild.
    // The deliberate exception stands: when the build claims nothing (`claimsGreen` false) no
    // verifier is dispatched, there is nothing to falsify, and its absence is not a death.
    //
    // AND A GREEN NOBODY ANSWERED FOR IS NOT A GREEN. The schema now REQUIRES the command list, the
    // false-green answer and an oracle-integrity verdict, but a schema is a request to an agent and
    // this is the enforcement: a verifier that reproduced the result while leaving any of the three
    // unanswered has told us it agreed without saying what it ran or whether the suite still proves
    // anything. Absence is a NO here, never a yes.
    const verifierDied = claimsGreen && !verify
    // COUNT THE COMMANDS AGAINST WHAT WAS CLAIMED, not against zero. `length > 0` let a verifier
    // that re-ran ONE of the implementer's five commands satisfy the check, and with reproduced=true
    // and contradictsClaim=false the lane converged while four suites were never independently run.
    // "Some of it was re-run" is not independent verification of a green; it is a sample of one.
    const claimedCommands = Array.isArray(fix.commands) ? fix.commands.filter((c) => typeof c === 'string' && c.trim()) : []
    // Every commandsRun entry must be a non-empty string. Filtering blanks used to turn
    // `commandsRun: [""]` (or `["cargo test", ""]`) into a quieter shape and let a hollow list
    // look like "no commands named" rather than "the verifier answered with nothing". Fail closed:
    // any blank or non-string entry means the verifier did not answer for what it ran.
    const rawRan = verify && Array.isArray(verify.commandsRun) ? verify.commandsRun : []
    const commandsRunWellFormed = rawRan.length > 0
      && rawRan.every((c) => typeof c === 'string' && c.trim() !== '')
    const ranCommands = commandsRunWellFormed ? rawRan.map((c) => c.trim()) : []
    // Compared as a SET over normalised text rather than by count, because a verifier that ran the
    // same command five times would satisfy a count and prove nothing. Coverage is against EVERY
    // claimed command, not merely "ran at least one".
    const norm = (c) => c.replace(/\s+/g, ' ').trim()
    const ranSet = new Set(ranCommands.map(norm))
    const unrun = claimedCommands.filter((c) => !ranSet.has(norm(c)))
    // A done implementer that omits `commands` or returns only blanks used to make
    // `unrun.length === 0` vacuously true: the verifier could name any command of its own and the
    // lane converged without independently re-running anything the build claimed. Status=done
    // requires a nonempty claimed-command set before coverage can be judged.
    const verifierAnswered = !!verify
      && claimedCommands.length > 0
      && commandsRunWellFormed
      && ranCommands.length > 0
      && unrun.length === 0
      && typeof verify.falseGreenRisk === 'string' && verify.falseGreenRisk.trim() !== ''
      && verify.oracleIntact === true
    const verifierOk = !claimsGreen
      || (!!verify && verify.reproduced === true && verify.contradictsClaim === false && verifierAnswered)
    const verifierSaid = verifierOk
      ? null
      : (verifierDied
        ? 'the INDEPENDENT VERIFIER died before reporting, so nothing re-ran your claimed green. Re-run your own commands and report the exact output.'
        : (claimedCommands.length === 0
          ? `the build claimed status=done but named no well-formed commands (${JSON.stringify(fix.commands)}); a green with nothing claimed cannot be independently verified. List the commands you ran.`
          : (unrun.length
          ? `the verifier re-ran ${ranCommands.length} command(s) but the build claimed ${claimedCommands.length}; these were NEVER independently run: ${unrun.join(' | ')}. A green re-run of part of the evidence is a sample, not a verification.`
          : (verify && verify.reproduced === true && verify.contradictsClaim === false
          ? `the INDEPENDENT VERIFIER reproduced your result but could not answer for it: commandsRun=${JSON.stringify(verify.commandsRun)}, falseGreenRisk=${JSON.stringify(verify.falseGreenRisk)}, oracleIntact=${verify.oracleIntact}. Either it ran nothing it could name, or it judged the oracle no longer intact. Make the commands reproducible and show that the suite still proves what it did before.`
          : `reproduced=${verify && verify.reproduced}; contradictsClaim=${verify && verify.contradictsClaim}; discrepancies=${verify && verify.discrepancies}; falseGreenRisk=${verify && verify.falseGreenRisk}`))))
    if (verifierDied) log(`${l.key}: round ${round} CANNOT CONVERGE — VERIFIER DIED; the claimed green was never re-run`)

    last = { lane: l, fix, reviews, verify, blockers, rounds: round, converged: false }

    if (fix.status === 'done' && blockers.length === 0 && !weakened && !creep && verifierOk && deadStanding.length === 0) {
      last.converged = true
      log(`${l.key}: CONVERGED round ${round} (independently re-verified by ${reviews.length}/${LENSES.length} reviewers)`)
      break
    }

    const cause = classifyRejection(blockers, weakened, verifierOk, fix.status, verifierDied || deadStanding.length > 0)
    TELEMETRY.rounds.push({ lane: l.key, round, cause, blockers: blockers.length, weakened, verifierOk })
    log(`${l.key}: round ${round} -> status=${fix.status}, ${blockers.length} blocker(s)${weakened ? ', ORACLE WEAKENED' : ''}${verifierOk ? '' : ', VERIFIER DISAGREED'} [cause: ${cause}]`)
    if (round === MAX_ROUNDS) {
      log(`${l.key}: ESCALATION — hit MAX_ROUNDS=${MAX_ROUNDS} unconverged; ${blockers.length} blocker(s) survive`)
      break
    }
    // Carry lower-severity findings and the status so the next round always has something concrete
    // even when no blocker was raised.
    const lesser = reviews.flatMap((v) => (v.findings || []).filter((f) => !isBlocking(f))).slice(0, 6)
    fb = {
      round: round + 1,
      blockers,
      lesser,
      weakened,
      verifierSaid,
      notDone: fix.status === 'done' ? null : fix.status,
    }
  }
  return last
}

phase('Build')

let results
if (ARGS.trial) {
  const first = LANES.find((l) => l.key === ARGS.trial)
  if (!first) {
    throw new Error(`lane-fanout: trial names lane "${ARGS.trial}", which is not in lanes: ${LANES.map((l) => l.key).join(', ')}`)
  }
  if (LANES.length < 2) throw new Error('lane-fanout: trial with fewer than two lanes serialises for nothing')
  log(`TRIAL: running lane "${first.key}" alone before committing the other ${LANES.length - 1}`)
  const trial = await runLane(first)
  const converged = trial && trial.converged
  if (!converged) {
    // The whole point: a brief that is wrong is usually wrong the SAME way for every lane, so
    // dispatching the rest would multiply one defect by N rather than discover N defects.
    log(`TRIAL FAILED — not dispatching the remaining ${LANES.length - 1} lane(s).`)
    log('The lock, the accept criteria or the review lenses did not hold against this tree. Fix the')
    log('brief, not the lane, then re-run: the other lanes would have failed the same way.')
    return {
      headline: [
        `TRIAL LANE "${first.key}" DID NOT CONVERGE — fleet not dispatched`,
        `${LANES.length - 1} lane(s) held back deliberately, not dropped`,
      ],
      trial,
      heldBack: LANES.filter((l) => l.key !== first.key).map((l) => l.key),
    }
  }
  log(`TRIAL CONVERGED — dispatching the remaining ${LANES.length - 1} lane(s)`)
  const rest = await parallel(LANES.filter((l) => l.key !== first.key).map((l) => () => runLane(l)))
  results = [trial, ...rest]
} else {
  results = await parallel(LANES.map((l) => () => runLane(l)))
}

const out = results.filter(Boolean).map((r) => ({
  lane: r.lane.key,
  bead: r.lane.bead || null,
  converged: r.converged,
  rounds: r.rounds,
  status: r.fix ? r.fix.status : 'agent_failed',
  summary: r.fix ? r.fix.summary : null,
  redBaseline: r.fix ? r.fix.redBaseline : null,
  filesChanged: r.fix ? r.fix.filesChanged : [],
  claimedVerification: r.fix ? r.fix.verification : null,
  independentlyReproduced: r.verify ? r.verify.reproduced : null,
  verifierObserved: r.verify ? r.verify.actualResults : null,
  verifierDiscrepancies: r.verify ? r.verify.discrepancies : null,
  falseGreenRisk: r.verify ? r.verify.falseGreenRisk : null,
  contractBreaches: r.fix ? r.fix.contractBreaches : null,
  followUps: r.fix ? r.fix.followUps : null,
  verdicts: (r.reviews || []).map((v) => v.verdict),
  oracleWeakened: (r.reviews || []).some((v) => v.oracleWeakened),
  remainingBlockers: r.blockers || [],
}))

const conv = out.filter((o) => o.converged).map((o) => o.lane)
const stuck = out.filter((o) => !o.converged).map((o) => `${o.lane}(${o.rounds}r,${o.remainingBlockers.length}b)`)
log(`CONVERGED: ${conv.join(', ') || 'none'} | UNCONVERGED: ${stuck.join(', ') || 'none'}`)
if (DEFECT_LEDGER.length) log(`defect classes seen this run: ${DEFECT_LEDGER.length}`)

// --- terminal phase: LAND ---------------------------------------------------
// This phase exists because the harness did not have one, and its absence was a process defect
// rather than an oversight. A lane used to converge and stop, leaving its commits on a detached
// HEAD in a worktree nobody landed. Accumulation was therefore guaranteed by construction: measured
// 2026-08-08, ZERO open PRs against a main that twelve-plus worktrees sat above -- one at +99, one
// at +95, one at +61 -- which is hours of correct work nobody could review.
//
// The fix is NOT a consolidation phase. Consolidation-as-an-institution is the bureaucracy you get
// from refusing to fix the thing that generates the debt. The fix is that landing is part of the
// pipeline, because the cost curve is superlinear: one lane's diff onto an integration branch is a
// minutes-long rebase and STAYS minutes; twelve lanes at once is not twelve times worse, because
// conflicts multiply and the authority train rebinds on every fix.
//
// So: neither N PRs nor one heroic merge. ONE long-lived integration branch that every converged
// lane lands on immediately, kept continuously mergeable, PR'd on a cadence the human chooses.
//
// ONE WRITER. A single agent lands every lane in sequence. Landing is the one place where parallel
// agents would share a write target, and two writers on one branch is the failure that has already
// cost this program a round.
const LAND = ARGS.land !== false && conv.length > 0
const INTEGRATION_BRANCH = ARGS.integrationBranch || 'integration/lane-fanout'
let landed = null
if (LAND) {
  phase('Land')
  // Built from the ORIGINAL result objects, never from `out`. `out` flattens `lane` to a STRING
  // (`lane: r.lane.key`), so the prompt's `o.lane.key` / `o.lane.wt` / `o.fix` were all undefined
  // and the integration owner was handed "undefined — worktree undefined", no files, no follow-ups.
  // The landing phase ran and told the owner nothing; a phase that runs on garbage has not run.
  const landable = results.filter(Boolean).filter((r) => r.converged)
  landed = await agent(
    `You are the integration owner. ${landable.length} lane(s) converged and must be LANDED NOW, in one sequence, by you alone.

INTEGRATION BRANCH: ${INTEGRATION_BRANCH}   BASE: ${TIP}

LANES TO LAND, in this order:
${landable.map((r, i) => `${i + 1}. ${r.lane.key} — worktree ${r.lane.wt}
   REVIEWED HEAD: ${(r.verify && r.verify.headSha) || 'NOT CAPTURED — refuse this lane and report it'}
   files: ${(r.fix && r.fix.filesChanged ? r.fix.filesChanged : []).join(', ') || '(see the worktree)'}
   leased edits it reported instead of making: ${(r.fix && r.fix.followUps) || 'none'}`).join('\n')}

WHY THIS RUNS AT ALL: work that converges but does not land accumulates, and the cost of landing it
grows faster than linearly. Landing one lane now is cheap; landing twelve later is not.

BEFORE YOU TOUCH ANYTHING:
  For each lane's worktree run 'git status --porcelain', 'git rev-parse HEAD' and 'git log --oneline -3'.
  A worktree with uncommitted changes has a LIVE WRITER -- do not land it, report it as skipped and
  say so. A finished workflow id is NOT evidence that nothing is writing; the working tree is.

  *** LAND ONLY THE REVIEWED HEAD. If 'git rev-parse HEAD' does not equal the REVIEWED HEAD listed
      for that lane above, do NOT land it: skip it and report both SHAs. A clean worktree is NOT a
      binding -- a commit added after the reviewers finished is clean too, and cherry-picking it
      would land unreviewed code under a reviewed lane's name. The same applies to a lane whose
      reviewed head was NOT CAPTURED: refuse it. Never resolve a mismatch by re-reviewing it
      yourself; you are the lander, and a lane whose head moved goes back through the harness. ***

HOW TO LAND:
  Create or fast-forward ${INTEGRATION_BRANCH} from ${TIP} in the PRIMARY repo. For each lane in
  order, apply that lane's commits (cherry-pick its range, or format-patch/am). After EACH lane:
  build and test the packages that lane touched, and stop at the first failure rather than piling
  the next lane on top of a broken tree.
  PERMITTED: branch create, checkout of the integration branch, cherry-pick, am, add, commit, and
  read-only git everywhere.
  FORBIDDEN: push, force-push, PR creation, reset --hard, clean, stash, rebase of anything already
  landed, and ANY write inside a lane worktree. Pushing and PR-opening are the human's call; your
  job is to make the branch exist, be correct, and be continuously mergeable.
  NEVER use a merge that would create an unsigned two-parent head -- that breaks the authority train
  and the error will blame the signature.

CONFLICTS: if a lane conflicts, resolve ONLY if the resolution is mechanical and obvious; otherwise
stop, leave the branch at the last good lane, and report the conflicting hunks exactly. A wrong
conflict resolution is far more expensive than a skipped lane.

REPORT: the branch and its head SHA, which lanes landed and which were skipped and why, the exact
verification you ran after each lane with its real output, every leased edit still outstanding across
all lanes (deduplicated -- these are what the human must apply before the PR is green), and the
exact 'gh pr create' command you did NOT run.`,
    { label: 'land', phase: 'Land', schema: {
      type: 'object',
      required: ['branch', 'headSha', 'landedLanes', 'skippedLanes', 'verification', 'outstandingLeasedEdits'],
      properties: {
        branch: { type: 'string' },
        headSha: { type: 'string' },
        landedLanes: { type: 'array', items: { type: 'string' } },
        skippedLanes: { type: 'array', items: { type: 'string' }, description: 'lane key + the reason, e.g. "p4-tai: live writer, 4 files dirty"' },
        verification: { type: 'string', description: 'exact commands and real output, per lane' },
        conflicts: { type: 'string' },
        outstandingLeasedEdits: { type: 'string', description: 'deduplicated across lanes; what the human must apply before a PR can be green' },
        prCommand: { type: 'string', description: 'the gh pr create command, NOT run' },
      },
    } },
  )
  if (landed) {
    log(`LANDED on ${landed.branch} @ ${landed.headSha}: ${(landed.landedLanes || []).join(', ') || 'none'}`)
    if ((landed.skippedLanes || []).length) log(`NOT LANDED: ${landed.skippedLanes.join(' | ')}`)
  } else {
    log('LAND phase returned nothing — converged work is still sitting in worktrees; land it by hand')
  }
} else if (conv.length === 0) {
  log('nothing converged, so nothing to land')
} else {
  log(`land disabled by args; ${conv.length} converged lane(s) left in their worktrees — this is how the +99 backlog happened`)
}

// Where did the rounds go? Rounds are the scarce resource, so this is the number that should drive
// the next improvement. A run dominated by scope-brief-defect means fix the BRIEF; by code-defect
// means the lanes are genuinely hard; by owner-lease means the lease boundary is drawn wrong.
const byCause = {}
for (const r of TELEMETRY.rounds) byCause[r.cause] = (byCause[r.cause] || 0) + 1
const wasted = (byCause['scope-brief-defect'] || 0) + (byCause['owner-lease'] || 0)
const buildRounds = out.reduce((n, o) => n + (o.rounds || 0), 0)

// SERIAL DEPTH is what wall-clock actually tracks — measured across this harness's runs, per-step
// latency sits at 9-14 min and total time follows depth almost exactly while ignoring width:
// 12 agents at depth 6 took 74 min; 36 agents at depth 6 took 63 min. Three times the width, no
// slower. So adding reviewers is nearly free and adding a serial step is not.
// This harness is already flat WITHIN a round -- build, then one parallel block of reviewers plus
// the verifier -- so depth is 2 per round and nothing here can be unstacked further. All remaining
// depth is ROUNDS, which makes brief quality the only real lever on wall-clock.
const maxRounds = out.reduce((n, o) => Math.max(n, o.rounds || 0), 0)
const depth = maxRounds * 2
log(`telemetry: ${buildRounds} build round(s); serial depth ~${depth} (${maxRounds} round(s) x 2: build then one parallel check block)`)
log(`telemetry: rejection causes ${JSON.stringify(byCause)}`)
if (wasted) log(`telemetry: ${wasted}/${TELEMETRY.rounds.length} rejected round(s) were BRIEF or LEASE defects, not code — fix those in the brief, not the lane`)

// Headline FIRST and compact, because the result file gets truncated on long runs and the journal
// stores content-hash labels rather than the lane keys passed in — so a truncated tail leaves
// verdicts unattributable to lanes. One line per lane, before anything verbose.
const headline = out.map((o) =>
  `${o.lane}: ${o.converged ? 'CONVERGED' : 'UNCONVERGED'} r${o.rounds} ${o.status} blockers=${(o.remainingBlockers || []).length} weakened=${o.oracleWeakened} verified=${o.independentlyReproduced}`)
log(`HEADLINE | ${headline.join(' | ')}`)

return {
  headline,
  lanes: out,
  // The landing result must be RETURNED, not merely logged. Caught by the offline preflight on its
  // first run: the Land phase executed and its branch and head SHA went nowhere, so the caller could
  // not tell where the work went -- a phase that runs and reports nothing is barely better than the
  // missing phase it replaced.
  landed,
  defectClasses: DEFECT_LEDGER.map((d) => ({ lane: d.laneKey, claim: d.claim })),
  telemetry: {
    buildRounds,
    serialDepth: depth,
    // Report what ACTUALLY reported, not what was dispatched. The previous expression was
    // ((LENSES.length + 1) * buildRounds) / buildRounds -- algebraically just LENSES.length + 1,
    // a constant that could never observe a dead agent. Telemetry that cannot be wrong is not
    // telemetry, and this harness lost checkers to session limits in most of its measured runs.
    checkersDispatched: TELEMETRY.checkers.reduce((n, c) => n + c.dispatched, 0),
    checkersReturned: TELEMETRY.checkers.reduce((n, c) => n + c.returned, 0),
    checkersPerBuild: TELEMETRY.checkers.length
      ? +(TELEMETRY.checkers.reduce((n, c) => n + c.returned, 0) / TELEMETRY.checkers.length).toFixed(2)
      : 0,
    deadCheckers: TELEMETRY.checkers.flatMap((c) => c.deadLenses),
    rejectionCauses: byCause,
    briefOrLeaseRounds: wasted,
    rounds: TELEMETRY.rounds,
  },
}
