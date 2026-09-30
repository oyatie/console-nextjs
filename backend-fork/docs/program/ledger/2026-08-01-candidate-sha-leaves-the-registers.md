# The candidate SHA leaves the documents that were validated against it

**Date:** 2026-08-01 · **Status:** built, not merged · **Train:** contract half of an expand/contract pair

This is the first entry written as its own file. That is the point of it: `docs/program/ledger/` is a
flat directory of one-file-per-entry `.md` documents, so two lanes recording two decisions no longer
write the same bytes of the same file. The gate learned the shape in the expand half; this entry
exercises it.

## What changed

Both authority registers stored the exact candidate SHA in every candidate-evidence leaf, in
`provenance.exact_current_candidate_sha`, and in a top-level `candidate` block. All of those are
deleted. The validator, the merge-train check and the `pull_request_target` bootstrap verifier now
take the candidate from **outside** the documents: `.github/workflows/ci.yml` derives the C/T/M train
from Git parentage before it opens either file, and the bootstrap verifier reads C as T's single
parent rather than out of T's capability registry.

`scripts/console/rebind-candidate.mjs`, `scripts/console/rebind-candidate.test.mjs` and
`scripts/console/rebind-authority-train.mjs` are deleted — there is nothing left to rebind.
`.gitattributes` is deleted with them: it existed only to collapse the register diffs that the rebind
produced, and a routine authority commit now changes zero lines in either register.

`source_revision` is **kept**: it is read by `plan-fanout.mjs` through
`validateSourceRevisionForAnchor`, which requires the SHA to exist, to be an ancestor of the epoch
anchor, and to be an ancestor of whatever the named ref resolves to. It was pinned at
`fix/backend-dead-helper@…`, a branch that resolves nowhere, so the ref half of that check was
passing vacuously. It now reads `origin/main@508520bcb` — the tip of `origin/main` and the base of
this train. An earlier draft of this entry pointed it at `main@5330914c2` and claimed that named a
commit on main; it did not name what the reader would assume, because the local `main` branch was
25 commits behind `origin/main`. That claim is retracted here.

The non-HOLD review receipt's own `candidate_sha` is **untouched**. That one originates in a
separately signed receipt, so comparing it against the candidate is a genuine cross-document check —
replay protection — not a value compared against a copy of itself.

## Why the comparison carried no information

`validate-console-truth-ledger.mjs` asserted that every stored copy equalled `candidate.sha`. The
copies were written from `candidate.sha` by the rebind tool, and `candidate.sha` was written from the
same Git fact CI derives independently. The assertion could not fail unless the rebind tool had a
bug, which is a test of the tool, not of the world. The bootstrap verifier's version was worse: it
*located* C by reading `T:docs/program/console-capability-registry.json` → `candidate.sha`, then
required that value to equal T's parent — a comparison a signed commit could not lose, whose only
lasting effect was to force a rewrite of the register on every rebase.

## The accepted loss, stated plainly

**A wholesale revert of either register now validates clean.** The old check refused it: reverting a
register restored an older candidate SHA, which then disagreed with the current one. The new
validator has nothing to disagree with.

This is accepted rather than mitigated, and the honest bound is:

- **Every capability row and every jurisdiction control is HOLD**, and the validator still forces
  that. A reverted register cannot promote anything; it can only restore a different set of HOLD
  prose.
- **Buck targets and route facts are read from C**, not from the register, and fail closed. A
  reverted register that claims a route or a target the candidate does not have goes red.
- **A promoted capability binds `registry_canonical_sha256` and `jurisdiction_canonical_sha256`** in
  a separately signed receipt, so the first row to leave HOLD re-ties both documents to a signature.
- **C..T is unchanged in what it forbids**: T is signed by the pinned SSH authority, is C's direct
  single-parent child, and may modify nothing outside the authority allow-list. This is the one real
  binding left, and it is a strong one — the register that gets validated is the tree exactly one
  commit after C.

**What is NOT a mitigation, and was claimed as one.** An earlier draft of this entry and of the
comment above the check named "the bijection below, which fails the moment the register's
control/trace set stops matching the registry's bindings" as what replaces the deleted binding. It
does not. That bijection compares the jurisdiction register against the capability registry, and
both are read out of the same T, so two documents that are stale *together* satisfy it exactly as
well as two that are current. It catches disagreement between the two documents, which is a real
property and a different one. The claim is withdrawn; the loss above stands unmitigated. (The
bijection's `controls.has(binding.control_id)` guard was also untested — deleting it produced an
unhandled `TypeError`, not a red test — and now has one.)

And the counterweight the old check never had: **it never caught in-place falsification.** Editing
`freshness.status` in place, leaving the SHA alone, passed the old validator and passes the new one
identically. The property that was lost is exactly "an older revision of an all-HOLD register", and
nothing more.

Re-adding the stored field would not restore the check — the validator no longer reads it — and it
would reinstate the shared-file mutex this change exists to remove. `validate-console-truth-ledger.test.mjs`
asserts that a re-added copy is inert, so the trade cannot be quietly undone by a data edit.

## What was explicitly preserved, not dropped — and at equal strength

Three `?.candidate_sha` comparisons were doubling as payload checks; the round-4 refutation in
`docs/ideas/process-minimum-viable-provenance.md` was correct about that. What that refutation, and
an earlier draft of this entry, both got wrong is *how much* they were carrying:
`control.candidate_evidence?.candidate_sha !== candidate.sha` refuses an EMPTY `{}` as surely as it
refuses a wrong SHA, because `undefined !== sha`. Replacing it with `object(value, label)` alone is
therefore weaker than what it replaces, not equal to it — an empty payload would have validated
clean, and this entry would have been wrong to say the lost property is "an older revision of an
all-HOLD register, and nothing more".

Each site now carries `object(...)` **plus** the fields that make the payload evidence: the
jurisdiction control's `candidate_evidence` requires a valid `status` and a non-empty `reason`,
which is exactly what the capability rows already required. The suite asserts that `{}`, a missing
`reason` and an invalid `status` all fail closed, in both documents. The refutation's other
argument, that the leaves catch a *partial* rebind, dissolved: there is no rebind step left to fail
halfway.

## What the gate gained

`C..T` must modify **at least one** authority document rather than all three, and may **add** regular
mode-100644 files under `docs/program/ledger/` — status `A`, at that prefix, and nowhere else. Added
files anywhere outside the prefix, near-miss siblings of the directory name, non-regular modes, and
deletions are all still refused, in both the train check and the bootstrap verifier, with tests built
on real signed Git fixtures.

The prefix admits a **flat directory of lowercase `.md` files**. A bare `startsWith` is a string
test, not a path test: it accepted `docs/program/ledger/../../evil`, a path naming a location
outside the prefix entirely, and it accepted a nested subtree and an added `.mjs`. Refusing any `/`
in the remainder removes traversal and nesting together. The `.md` constraint is deliberate: this is
the only prefix at which the authority tip may add a file at all, so without it the same allowance
admits executable content under `docs/`, added by the one commit otherwise forbidden to touch a
product path.

Three scripts gate that one diff, and they did not read it the same way. The validator passed
`--find-renames --find-copies-harder`; the train check and the bootstrap verifier passed
`--no-renames`. A new ledger entry ≥50% similar to a file already in the tree is then status `C`
with two paths to one reader and `A` with one path to the others — measured at `C084` versus `A` —
so the same commit was refused by one gate and accepted by the two that decide the merge. The
prefix rule and the diff flags now live in `scripts/console/authority-ledger-path.mjs` and are
imported by all three, with a fixture that builds the near-copy and asserts all three agree.
