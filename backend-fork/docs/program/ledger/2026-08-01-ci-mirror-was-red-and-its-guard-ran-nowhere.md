# 2026-08-01 — the local CI mirror was red, and its guard ran nowhere

`npm run verify` exists so a defect costs a local minute rather than a 45-minute CI round trip.
On `main` it was red: `node --test scripts/verify.test.mjs` failed 6 of 6.

`ci.yml` renamed the job `support-domain-unit` to `domain-unit`; the mirror kept the old name. The
job-completeness check fails closed, so it aborted before reaching the step-level check — and five
run-steps had accumulated unclassified behind it: the executed-tests ratchet, doc citations,
undeclared imports, the request-body contract, and the mirror's own new step.

The reason none of it surfaced is the finding rather than the drift. `scripts/verify.test.mjs` is
the guard that keeps the mirror honest, and it ran in no workflow at all — only the similarly named
`scripts/console/verify-console-authority-train.test.mjs` did. A guard nobody executes is how a
rename outlives its rename, and how a fail-closed check ends up masking the four behind it.

It now runs in preflight.

**The first thing it caught was a step added while it was still unwired.** #556 wired
`verify-console-pr-authority-bootstrap.test.mjs` into `ci.yml` as `Console PR authority bootstrap
regression`, and nothing declared it — so this mirror's job-completeness check failed closed on it
the moment the two changes met. That is the drift this file exists to catch, caught on its first
opportunity rather than discovered a month later by a rename. The step is now declared, and removing
that declaration fails 3 of the guard's 6 tests by name.

Two honesty gaps stay named rather than fixed. The step called "Doc citations — every code citation
must resolve" resolves a declared subset of two files, not the repository; renaming the step is what
would make the promise true. And `scripts/verify.test.mjs` was one of 14 `.test.mjs` suites that
execute in no workflow — wiring this one does not wire the other 13, and `check-executed-tests.mjs`
cannot see them because it traces Rust targets only.

Every capability, evidence contract, jurisdiction binding, Korea control, review disposition, and
exposure state remains `HOLD`.
