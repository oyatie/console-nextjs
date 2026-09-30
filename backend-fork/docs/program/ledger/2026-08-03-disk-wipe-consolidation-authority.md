# Disk-wipe consolidation candidate authority

## Scope and identity

- Protected base B: `435e251edfab12750850d5b1d411528b10a3ed8a`
- Signed content candidate C: `bbc0cd6f6de43be5d1202d61a02b556fd242b515`
- Authority tip T: the signed direct child that adds only this ledger entry
- Delivery vehicle: PR #562, `docs/session-handoff-2026-08-02` into `main`
- Integration branch: `main`; no local or remote `dev`/`develop` integration branch existed during the audit

This record authorizes exact-SHA review and CI of C/T. It is not merge evidence,
release evidence, deployment evidence, or permission to wipe the workstation.
Those claims require their own readback after the corresponding operation.

## Consolidation decision

The candidate preserves only independently inspected, post-pivot work that fits
the active Ontology/Foundry/Policy → Company/OrgUnit/Employee → HR/Payroll
boundary. It rejects unsafe or out-of-scope local work rather than preserving
branches as an informal backlog. The complete preserved and rejected inventory,
fresh-session order, operational holds, and external-custody requirements are in
[`../../handoffs/2026-08-03-disk-wipe-consolidation.md`](../../handoffs/2026-08-03-disk-wipe-consolidation.md).

The remote-ref audit began with 252 heads. It retained `main` and PR #562's head,
then deleted 250 exact refs: 167 merged-PR heads, 76 unassociated pre-pivot
leftovers, six closed-unmerged heads whose useful final net was already retained,
and one rejected post-pivot Cosskorea head. The point-in-time identities are in
[`../../handoffs/2026-08-03-remote-branch-deletion-manifest.tsv`](../../handoffs/2026-08-03-remote-branch-deletion-manifest.tsv).
Deleting a ref did not erase merged history; PR refs, tags, and reachable objects
remain the archaeology sources.

The user-provided scorecard is an external static opinion only. It did not run
the complete PostgreSQL suite or deploy the system. Its scores and recommendations
establish no repository authority and caused no speculative scope expansion or
subtractive rewrite. Only findings reproduced against current authoritative
sources affected this candidate.

## Evidence established before T

- C has a good signature from the pinned ED25519 authority, and the consolidation
  first-parent train preserves signed content commits. One older unsigned merge
  remains reachable as historical source ancestry; it is neither C nor T and was
  not rewritten or misrepresented as newly authenticated content.
- The complete CI proof-lock passed 51/51 contract tests. Two independent audits
  exercised 666 generated run-step, action, job-envelope, trigger, and local-action
  mutations with zero accepted bypasses.
- Foundation checks passed 134 assertions and 6/6 tests; executed-test inventory
  passed 22/22; local-CI mirror tests passed 12/12; security-workflow hardening
  passed 34/34; external-PR authority tests passed 17/17; and reasoning-lens
  validation passed 40/40 with six durable evidence blocks.
- Release `v0.3.1` and protected `main` at B passed the complete hosted workflow,
  including the serialized PostgreSQL reachability lane, before this candidate
  was sealed.
- Exactly three immutable Ultragoal inputs and one evolving execution-status JSON
  are tracked. New `.omx` runtime churn is ignored. Live secrets, signing identity,
  GitHub authentication, OCI access, and Kubernetes/Talos credentials are not
  copied into Git and must be escrowed or deliberately reissued before a wipe.

## Remaining merge boundary

PR #562 remains unmerged at this candidate stage. Before merging it:

1. Run the protected-main authority simulation and the complete local verifier at
   exact T.
2. Obtain two independent adversarial reviews of exact T and resolve every
   finding without silently changing the reviewed bytes.
3. Push T, require all hosted checks to succeed, and replace only the obsolete
   protected API-context name while adding the already-published Domain and
   `dev-up` contexts. Read back strict/app-bound protection after mutation.
4. Enable and satisfy one formal approval from someone other than the PR author,
   stale-review dismissal, last-push approval, and conversation resolution.
5. Squash-merge only after the exact head, review decision, conversations, and
   required contexts are re-read. Finish any generated release PR under the same
   C/T, review, and CI discipline.
6. On final `main`, record exact merge/release/run/protection identities, verify
   zero open PRs and `main` as the sole remote branch, then and only then treat
   repository work as safe to abandon locally.

Every capability, jurisdiction, Korea-control, production-exposure, infrastructure,
and legal conclusion remains unchanged and `HOLD`. The grandfathered OCI Ampere
A1 instance must not be destroyed, terminated, resized, or reprovisioned.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "approval",
    "release",
    "production"
  ],
  "selected_lenses": [
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Red Team": "Tested required proof jobs for bypasses and treated unsigned, stale, ignored, and externally supplied material as untrusted.",
    "Systems Thinking": "Traced branch topology, candidate ancestry, CI producers, protection consumers, review state, release automation, ignored state, and fresh-session continuity as one closeout system.",
    "Operability / Day-2": "Kept credentials outside Git, preserved an ordered restart entrypoint, and required exact post-operation readback before declaring the disk disposable.",
    "Blast-radius / cell-based": "Consolidated through one existing PR and one integration branch while leaving production, cluster, DNS, legal, and rejected-domain mutations on HOLD.",
    "Telemetry-first": "Bound decisions to exact commits, signatures, test counts, branch names, deletion identities, check contexts, and required future readbacks.",
    "Zero-trust / defense-in-depth": "Required protected-target authentication, two independent exact-SHA reviews, hosted checks, formal external approval, strict protection, and post-merge verification as separate safeguards."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Useful post-pivot work can be preserved in one signed C/T train while stale branches, unsafe domain work, runtime churn, and an unverified external opinion remain non-authoritative."
  ],
  "decisions_changed_or_rejected": [
    "Rejected preserving every local branch or ignored artifact merely because the workstation will be wiped.",
    "Rejected treating local green checks, an external scorecard, or a pushed head as sufficient merge or wipe authority."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
