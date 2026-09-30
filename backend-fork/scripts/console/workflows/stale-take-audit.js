export const meta = {
  name: 'stale-take-audit',
  description: 'Find every file where a hand-rebuilt branch silently took the OLD side of a file the base branch had advanced — before CI finds them one push at a time',
  whenToUse: 'After reconstructing a branch by copying a tree rather than merging, or after any hand-written exclusion list decided which side of a file to keep. Run it BEFORE the first push: it found in one pass what cost five CI round-trips to discover one file at a time.',
  phases: [
    { title: 'Audit', detail: 'batched: does main have content this branch dropped?' },
    { title: 'Confirm', detail: 'adversarially re-check only the files claimed stale' },
  ],
}

// The defect class, stated once so every agent judges the same thing:
//
// PR #618 was rebuilt by taking a superseded branch's TREE for the files it owned, with a
// hand-written exclusion list for files main owned. That list was a LIST -- each entry reasoned
// about individually -- so it was complete only for the cases already thought of. Five CI pushes
// have each surfaced one more file where main had advanced and the copy silently reverted it:
// ci.yml (un-wired a test suite main runs), and the ci-preflight lock and its counters.
//
// A revert that reintroduces old content is INVISIBLE in the diff-to-main -- it looks like an
// ordinary change. Only comparing against what main HAS reveals it.

// args can arrive as a JSON STRING. Every other harness here guards for it and this one did not,
// so the first run died at line 16 having spawned zero agents.
let ARGS = args
if (typeof ARGS === 'string') {
  try { ARGS = JSON.parse(ARGS) } catch (e) {
    throw new Error(`stale-take-audit: args arrived as a string that is not valid JSON: ${e.message}`)
  }
}
ARGS = ARGS || {}

// Unknown options are rejected BEFORE required fields, so a typo is reported as the typo rather
// than as the missing field it happens to look like.
const KNOWN_ARGS = ['repo', 'main', 'files', 'batch']
{
  const unknown = Object.keys(ARGS).filter((k) => !KNOWN_ARGS.includes(k))
  if (unknown.length) {
    throw new Error(`stale-take-audit: unknown option(s) ${unknown.join(', ')}. Known: ${KNOWN_ARGS.join(', ')}.`)
  }
}

const FILES = ARGS.files
const REPO = ARGS.repo
const MAIN = ARGS.main
if (!Array.isArray(FILES) || !FILES.length) throw new Error('stale-take-audit: args.files must be a non-empty array')
if (!REPO || !MAIN) throw new Error('stale-take-audit: args.repo and args.main are required')

const BATCH = ARGS.batch || 6
const chunk = (xs, n) => xs.reduce((a, x, i) => (i % n ? a[a.length - 1].push(x) : a.push([x]), a), [])

const RULES = `
REPO: ${REPO}   BASE BRANCH: ${MAIN}   (read-only: do not edit, commit, or push anything)

For each file, run BOTH directions IN ${REPO} (never the inherited cwd) and read them:
    git -C ${REPO} diff HEAD ${MAIN} -- <file>     # '+' lines = what MAIN has that HEAD LACKS  <-- the danger
    git -C ${REPO} diff ${MAIN} HEAD -- <file>     # '+' lines = what HEAD adds

You are looking for ONE thing: content present in ${MAIN} and ABSENT from HEAD, where the absence is
a REVERSION rather than a deliberate removal. Signals that it is a reversion:
  - main's version is strictly larger and HEAD's matches an older shape
  - a locked list, ratchet, counter, registry or wiring entry that main added and HEAD does not have
  - a CI step, npm script, or test-suite registration that exists in main and not in HEAD
  - a comment in main referencing a commit or PR that HEAD's version predates

NOT a reversion, and must NOT be reported:
  - content HEAD deliberately removes as its stated deliverable (this branch removes employees DML
    from backend/app/src/hr.rs on purpose -- that is the whole point of the change)
  - main's prose being merely reworded
  - generated files whose content is derived (they are regenerated separately)

For every file you judge STALE you must quote the exact missing lines. A file you cannot decide is
UNCERTAIN, not stale -- a false 'stale' costs a wrong revert, which is worse than another CI round.
`

const SCHEMA = {
  type: 'object',
  required: ['results'],
  properties: {
    results: {
      type: 'array',
      items: {
        type: 'object',
        required: ['file', 'verdict', 'evidence'],
        properties: {
          file: { type: 'string' },
          verdict: { type: 'string', enum: ['STALE', 'CLEAN', 'UNCERTAIN'] },
          evidence: { type: 'string', description: 'the exact lines main has and HEAD lacks, or why it is clean' },
          missingFromHead: { type: 'string', description: 'verbatim content to graft back, if STALE' },
          wouldBreak: { type: 'string', description: 'which gate or behaviour breaks if this stays reverted' },
        },
      },
    },
  },
}

phase('Audit')
const audited = await parallel(chunk(FILES, BATCH).map((batch, i) => () =>
  agent(
    `Audit ${batch.length} file(s) for silently-reverted content.
${RULES}

FILES:
${batch.map((f) => `  ${f}`).join('\n')}

Return one result per file, in order. Do not skip any.`,
    // Cheap tier: every STALE claim from this pass is adversarially re-checked below, and a
    // CLEAN verdict that is wrong shows up as the next CI failure rather than as a bad merge.
    { schema: SCHEMA, label: `audit:${i}`, phase: 'Audit', model: 'sonnet' },
  )))

const all = (audited || []).filter(Boolean).flatMap((r) => r.results || [])
const dead = chunk(FILES, BATCH).length - (audited || []).filter(Boolean).length
if (dead) log(`!! ${dead} audit batch(es) died — those files are UNAUDITED, not clean`)

// Batch objects returning is not coverage. A live batch that omits or duplicates a requested
// path used to leave `dead === 0` and report "full coverage" while classifying the omitted file
// as audited. Every requested path must appear exactly once before coverage is claimed.
const resultCounts = new Map()
for (const r of all) {
  if (!r || typeof r.file !== 'string') continue
  resultCounts.set(r.file, (resultCounts.get(r.file) || 0) + 1)
}
const missingAuditFiles = FILES.filter((f) => !resultCounts.has(f))
const duplicatedAuditFiles = FILES.filter((f) => (resultCounts.get(f) || 0) > 1)
const coverageIncomplete = dead > 0 || missingAuditFiles.length > 0 || duplicatedAuditFiles.length > 0
if (missingAuditFiles.length) {
  log(`!! ${missingAuditFiles.length} requested file(s) have no audit result — UNAUDITED, not clean`)
  for (const f of missingAuditFiles) log(`   missing: ${f}`)
}
if (duplicatedAuditFiles.length) {
  log(`!! ${duplicatedAuditFiles.length} requested file(s) have duplicate audit results — coverage ambiguous`)
  for (const f of duplicatedAuditFiles) log(`   duplicated: ${f}`)
}

const suspect = all.filter((r) => r.verdict === 'STALE')
log(`audit: ${all.length} file(s) read, ${suspect.length} claimed STALE, ${all.filter((r) => r.verdict === 'UNCERTAIN').length} uncertain`)

if (!suspect.length) {
  return {
    headline: [
      `No stale takes found across ${all.length} files.`,
      coverageIncomplete ? `coverage INCOMPLETE — deadBatches=${dead}, missing=${missingAuditFiles.length}, duplicated=${duplicatedAuditFiles.length}` : 'full coverage',
    ],
    stale: [],
    uncertain: all.filter((r) => r.verdict === 'UNCERTAIN'),
    missingAuditFiles,
    duplicatedAuditFiles,
  }
}

// Only the accusations get a second opinion; confirming a CLEAN verdict costs more than it is worth.
phase('Confirm')
const confirmed = await parallel(suspect.map((s) => () =>
  agent(
    `Try to REFUTE this claim. Default to refuted=true when uncertain.

CLAIM: ${MAIN} contains content that HEAD dropped by reversion, in ${s.file}.
EVIDENCE OFFERED: ${s.evidence}

${RULES}

Refute it if: the content is absent from HEAD deliberately (it is the branch's stated deliverable),
or main's version is not actually newer, or the lines quoted do not exist as claimed. Run the diffs
yourself rather than trusting the quote.

Do NOT be handed a graft candidate from the first pass — a payload nobody re-read is a claim, not
evidence. If you uphold STALE (refuted=false), you MUST return missingFromHead as the exact graft
payload YOU reconstruct after re-reading the diffs. An upheld STALE without that payload is
unresolved, not confirmed. A mismatched or missing confirmation payload must not be substituted
from the first agent's text.`,
    {
      schema: {
        type: 'object',
        required: ['file', 'refuted', 'reasoning', 'missingFromHead'],
        properties: {
          file: { type: 'string' },
          refuted: { type: 'boolean' },
          reasoning: { type: 'string' },
          missingFromHead: {
            type: 'string',
            description: 'When refuted=false: the exact content to graft, attested by THIS pass. When refuted=true: empty string.',
          },
        },
      },
      label: `confirm:${s.file.split('/').pop()}`, phase: 'Confirm',
    },
  // `v && v.refuted` yields NULL for a dead agent, and null is neither `=== false` nor truthy -- so
  // the suspicion fell out of BOTH lists below and vanished without trace while the headline still
  // said "full coverage". `typeof === 'boolean'` is used rather than a null check because the schema
  // is a request to the model, not an enforcement: an agent that returns an object without `refuted`
  // must land in the same unresolved bucket as one that never returned at all.
  ).then((v) => ({
    ...s,
    refuted: v ? v.refuted : null,
    refutation: v ? v.reasoning : null,
    confirmedMissing: v && typeof v.missingFromHead === 'string' ? v.missingFromHead : null,
  }))))

const answered = (confirmed || []).filter((c) => c && typeof c.refuted === 'boolean')
// Upholding STALE without attesting the graft payload must not publish the first-pass quote.
const real = answered.filter((c) =>
  c.refuted === false
  && typeof c.confirmedMissing === 'string'
  && c.confirmedMissing.trim() !== '')
// A suspicion whose REFUTATION never ran is still a suspicion. This file's own rule two paragraphs
// up says a false 'stale' costs a wrong revert while a missed one costs another CI round -- but the
// missed one here is the worse half: the reversion this workflow exists to catch is a file whose
// un-wiring means the check that would have caught it does not run.
const unconfirmed = suspect.filter((s) => {
  const a = answered.find((c) => c.file === s.file)
  if (!a) return true
  if (a.refuted === false && !(typeof a.confirmedMissing === 'string' && a.confirmedMissing.trim())) return true
  return false
})
if (unconfirmed.length) {
  log(`!! ${unconfirmed.length} confirmation(s) never returned — those files are UNRESOLVED, not clean`)
  for (const u of unconfirmed) log(`   ${u.file}`)
}

return {
  headline: [
    `${all.length} files audited, ${suspect.length} suspected, ${real.length} CONFIRMED stale after adversarial re-check`,
    unconfirmed.length ? `${unconfirmed.length} suspected file(s) UNRESOLVED — their confirmation never returned` : null,
    coverageIncomplete || unconfirmed.length
      ? `coverage INCOMPLETE — deadBatches=${dead}, missing=${missingAuditFiles.length}, duplicated=${duplicatedAuditFiles.length}, unresolved=${unconfirmed.length}`
      : 'full coverage',
  ].filter(Boolean),
  // Publish ONLY the confirmer-attested graft. Never r.missingFromHead from the audit spread —
  // that is an unconfirmed claim even when the STALE verdict itself was upheld.
  stale: real.map((r) => ({ file: r.file, missingFromHead: r.confirmedMissing, wouldBreak: r.wouldBreak, evidence: r.evidence })),
  // Unresolved accusations keep the first-pass claim under a non-graft key so an operator sees
  // what was alleged without a ready-to-apply missingFromHead payload.
  unconfirmed: unconfirmed.map((u) => ({
    file: u.file,
    evidence: u.evidence,
    claimedMissingFromHead: u.missingFromHead,
    wouldBreak: u.wouldBreak,
  })),
  refuted: answered.filter((c) => c.refuted).map((c) => ({ file: c.file, why: c.refutation })),
  uncertain: all.filter((r) => r.verdict === 'UNCERTAIN').map((r) => ({ file: r.file, evidence: r.evidence })),
  missingAuditFiles,
  duplicatedAuditFiles,
}
