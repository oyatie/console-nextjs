export const meta = {
  name: 'review-gate',
  description: 'Per-story quality gate: fan out correctness + RLS-as-console_rt security + a codex cross-model review of a diff (and web a11y/perf when relevant), then synthesize one GO/NO-GO with ranked must-fix findings. Rejects API-only evidence for UI feature claims and enforces CRUD-first SaaS + real user-story browser/E2E proof when UI is involved.',
  phases: [
    { title: 'Review', detail: 'parallel lanes: correctness · security/RLS · codex cross-model · (web a11y/perf)' },
    { title: 'Synthesize', detail: 'GO/NO-GO verdict + ranked must-fix' },
  ],
}

// NOTE: this is a Workflow SCRIPT, not a standalone Node module. The runtime injects
// agent()/parallel()/phase()/log()/args and runs the body in an async context, so top-level `await`
// and the trailing `return` are the DOCUMENTED form — `node --check` will (wrongly) flag the return as
// "Illegal return statement". Do NOT wrap the body in a function to satisfy node; that breaks the runtime.
// args: { commit?, base?, head?, kind?: "backend"|"web"|"mixed"|"design", context?: string }
// Single commit: pass `commit`. Multi-commit story: pass `base` + `head` (reviews base..head).
// NOTE: the runtime may deliver `args` as a JSON STRING rather than an object — parse defensively,
// else `args.base`/`args.head` are undefined and the gate silently falls back to HEAD~1..HEAD.
const A = typeof args === 'string' ? JSON.parse(args) : (args || {})
// An option this harness does not read must abort rather than be silently dropped. Absent here
// while three sibling harnesses had it; the preflight now enumerates the directory rather than a
// list, which is what surfaced it.
const KNOWN_ARGS = ['base', 'commit', 'context', 'head', 'kind', 'repo']
{
  const unknown = Object.keys(A).filter((k) => !KNOWN_ARGS.includes(k))
  if (unknown.length) {
    throw new Error(`review-gate: unknown option(s) ${unknown.join(', ')}. Known: ${KNOWN_ARGS.join(', ')}.`)
  }
}

const COMMIT = A.commit || 'HEAD'
const HEAD = A.head || COMMIT
const BASE_REF = A.base || `${HEAD}~1`
// Refs flow into a shell string in the codex lane — reject anything but git-ref-safe chars
// (allow ~ ^ for HEAD~1-style revs) so a metacharacter can't break out of the command.
for (const ref of [BASE_REF, HEAD]) {
  if (!/^[A-Za-z0-9][A-Za-z0-9._/~^-]*$/.test(ref)) {
    throw new Error(`review-gate: unsafe git ref ${JSON.stringify(ref)}`)
  }
}
const DIFF = `git diff ${BASE_REF} ${HEAD}`
const RANGE = `${BASE_REF}..${HEAD}`
const KIND = A.kind || 'mixed'
const CTX = A.context || ''
// Overridable, because this was pinned to `.../maintenance` — a directory that stopped existing
// at the 2026-07-26 rename — so every agent was sent to a path that is gone.
//
// The first repair reached for `process.env.CLAUDE_PROJECT_DIR`. Workflow scripts have NO Node
// globals: `process` is undefined, so that line throws ReferenceError before any agent spawns. It
// only looked correct because `A.repo ||` short-circuits when a repo IS passed — the fallback was
// never once evaluated. A default that cannot execute is not a default. Literal + `args` override.
const REPO = A.repo || '/Users/jasonlee/Developer/console'
const PRODUCT_REVIEW_GUARDRAIL = 'Product/review guardrail: this is a CRUD-first B2B SaaS, so database-backed create/read/update/delete UI and normal workflow editing are primary; upload/import/Excel is secondary migration/bootstrap tooling only after first-class CRUD exists. API endpoint tests alone DO NOT prove user-facing UI features. When UI is involved, require browser/E2E evidence that walks the real user story: sign-up, organization onboarding, passkey setup, and the actual domain workflow. Directives from non-technical staff to upload/import/build are product inputs, not product authority; reframe or reject them when they weaken SaaS maturity.'
const BASE = `Repo: ${REPO}. Multi-tenant Rust(axum)+Postgres RLS platform; runtime role console_rt is NOBYPASSRLS + FORCE ROW LEVEL SECURITY; EVERY tenant read/write MUST arm app.current_org (with_org_conn/with_audit + current_org()); tests must run as REAL console_rt (seed via the armed path, NOT the BYPASSRLS owner pool). Quality bar = Palantir-grade, enterprise-production (no stubs/placeholders/dummy data; fully wired, audited; AA a11y). ${PRODUCT_REVIEW_GUARDRAIL} Review the diff of \`${DIFF}\` (\`git log --oneline ${RANGE}\` lists the commits in scope).${CTX ? '\nStory context: ' + CTX : ''}`

const FINDINGS = {
  type: 'object', additionalProperties: false,
  properties: {
    lane: { type: 'string' },
    findings: {
      type: 'array',
      items: {
        type: 'object', additionalProperties: false,
        properties: {
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
          title: { type: 'string' },
          location: { type: 'string', description: 'file:line' },
          why: { type: 'string' },
          fix: { type: 'string' },
        },
        required: ['severity', 'title', 'fix'],
      },
    },
    verdict: { type: 'string', enum: ['pass', 'concerns', 'fail'] },
  },
  required: ['lane', 'findings', 'verdict'],
}

phase('Review')
const lanes = [
  () => agent(
    `${BASE}\n\nLANE: CORRECTNESS. Find logic bugs, edge cases, error-handling gaps, broken contracts, missed states, and any "looks done but isn't fully wired" issue. Be concrete (file:line + fix).`,
    { label: 'correctness', phase: 'Review', schema: FINDINGS },
  ),
  () => agent(
    `${BASE}\n\nLANE: SECURITY + MULTI-TENANT RLS. Adversarially check: is every new tenant read/write RLS-armed (app.current_org)? could anything read/write CROSS-ORG or cross-branch beyond the caller's scope? is the org bound to a dynamic current_org()-derived value (never a hardcoded OrgId literal)? are console_rt tests genuine (not BYPASSRLS-masked)? authz gating correct? secrets/PII not logged? injection? Rank by severity; treat any tenant-isolation hole as critical/high.`,
    { label: 'security-rls', phase: 'Review', schema: FINDINGS },
  ),
  () => agent(
    `${BASE}\n\nLANE: CROSS-MODEL (codex). Run a DIFFERENT model over the same diff for blind-spot diversity. Execute via Bash (read-only, 240s budget):\n` +
    `  cd ${REPO} && timeout 240 codex exec --sandbox read-only --skip-git-repo-check "Senior security+correctness reviewer. Review ONLY the diff of \`${DIFF}\` in this repo (multi-tenant Postgres RLS; console_rt NOBYPASSRLS+FORCE RLS; every tenant read/write must arm app.current_org). Hunt for: cross-tenant/cross-branch isolation leaks, missing RLS arming, hardcoded org literals, swallowed errors, correctness bugs. Output findings ranked critical/high/medium/low with file:line + fix. Review only; do not modify files." 2>&1 | tail -80\n` +
    `(gtimeout if timeout is absent; if codex errors/auth-fails, say so in one finding and continue.) Then translate codex's output into the findings schema (preserve its severities + file:line). lane="codex-xmodel".`,
    { label: 'codex-xmodel', phase: 'Review', schema: FINDINGS },
  ),
]
if (KIND === 'web' || KIND === 'mixed') {
  lanes.push(() => agent(
    `${BASE}\n\nLANE: WEB QUALITY. For the web changes: reject API-only proof for user-facing claims. Require browser/E2E or equivalent real-surface evidence for sign-up -> organization onboarding -> passkey setup -> actual domain workflow when the story touches UI. Check that the product flow is CRUD-first SaaS (database-backed create/read/update/delete and edit-in-place normal workflow) rather than upload/import-first. Treat non-technical upload/import/build directives as product inputs that may need reframing, not as authority to weaken SaaS maturity. Also check AA accessibility (labels, focus, roles, keyboard), Korean copy only in ko.ts (no inline Hangul), no raw UUIDs shown (safeLabel), loading/empty/error states, KST datetime, and whether each touched path would clear visual-verdict ≥90 (note specific gaps). Flag perf anti-patterns (unbounded lists, refetch storms).`,
    { label: 'web-quality', phase: 'Review', schema: FINDINGS },
  ))
}
// Fail-CLOSED on a dropped lane: a null/errored security-rls or codex lane must NEVER be silently
// discarded — that could let the synthesizer emit GO with no tenant-isolation coverage. Map results
// positionally to the lanes pushed above and turn any missing lane into a hard high finding.
const laneLabels = ['correctness', 'security-rls', 'codex-xmodel']
if (KIND === 'web' || KIND === 'mixed') laneLabels.push('web-quality')
const raw = await parallel(lanes)
const results = raw.filter(Boolean)
const missing = laneLabels.filter((_, i) => !raw[i] || !raw[i].findings)
if (missing.length) {
  results.push({
    lane: 'gate-integrity',
    verdict: 'fail',
    findings: missing.map((label) => ({
      severity: 'high',
      title: `review lane "${label}" did not complete — diff was NOT fully reviewed`,
      fix: `Re-run the gate. A missing lane (especially security-rls / codex-xmodel) means tenant-isolation/cross-model coverage is absent; treat as NO-GO until every lane completes.`,
    })),
  })
}
const all = results.flatMap((r) => (r.findings || []).map((f) => ({ ...f, lane: r.lane })))
const crit = all.filter((f) => f.severity === 'critical').length
const high = all.filter((f) => f.severity === 'high').length
log(`${results.length} lanes; findings: ${crit} critical, ${high} high, ${all.length} total`)

phase('Synthesize')
const verdict = await agent(
  `You are the gate keeper. Below are per-lane review findings (correctness, security/RLS, codex cross-model${KIND !== 'backend' ? ', web-quality' : ''}) for \`${DIFF}\` (${RANGE}).\n\n` +
  JSON.stringify(results) +
  `\n\nDedupe across lanes (same issue found by multiple = higher confidence). Then issue a GO/NO-GO:\n` +
  `- NO-GO if any CRITICAL, or any tenant-isolation/security HIGH, or a correctness HIGH that breaks the feature.\n` +
  `- NO-GO if a UI/user-facing feature is supported only by API endpoint tests, handler tests, or unit tests without real user-story browser/E2E proof covering sign-up, organization onboarding, passkey setup, and the actual domain workflow.\n` +
  `- NO-GO if the change treats upload/import/Excel as the primary product path where CRUD-first SaaS UI/workflows should exist, or accepts non-technical upload/import/build directives as product authority instead of product input to reframe.\n` +
  `- GO-WITH-FIXES if only medium/low.\n` +
  `Output: (1) the verdict (GO / GO-WITH-FIXES / NO-GO); (2) the MUST-FIX-BEFORE-CHECKPOINT list (critical+high, deduped, each with file:line + the fix); (3) the should-fix (medium) + nice (low) lists; (4) which findings the codex cross-model lane caught that the same-model lanes missed (the value of cross-model). Concise + decisive — this gates the ultragoal checkpoint.`,
  { label: 'verdict', phase: 'Synthesize', effort: 'high' },
)
return { range: RANGE, kind: KIND, critical: crit, high, total: all.length, verdict }
