# 2026-08-02 — the handoff that recorded its own corrections

A session handoff was written for an agent with no prior context. The useful part was not the
summary; it was that assembling one forced every carried-forward claim to be re-checked, and three
did not survive.

**The Kubernetes node name has two answers and nobody had noticed.** `deploy/OPS-RUNBOOK.md:42`
names the node `console-fsm-node`; `deploy/opentofu/contexts/oci-guest/primitives/compute/main.tf:100`
declares `display_name = "mnt-fsm-node"`. The live OCI *instance* display name is neither — it is
`control`, verified by `oci compute instance list -c <prod>`. Two files, two names, one live resource
that matches neither, and the contradiction had survived long enough to be recited into a prior
memory as settled fact. It is now recorded as UNVERIFIED rather than resolved by picking the more
plausible one.

**`kubectl` on this machine does not talk to production.** The only configured context is
`admin@oya-talos` — the laptop QEMU cluster, three nodes on `10.5.0.0/24`, Talos v1.13.7, shared with
another repo and not to be mutated. `kubectl get nodes` succeeds and prints a healthy cluster. Every
property of that output is true and none of it describes the thing anything is deployed to. A check
that answers confidently about the wrong system is worse than one that fails.

**Re-measured numbers beat remembered ones.** Third-party Buck2 targets pinned at `visibility = []`
were carried in prose as 1,575 of 1,632. Measured on `f2646890b`:

    grep -rc 'visibility = \[\]' third-party/     ->  1,045
    grep -rho 'visibility = \['   third-party/     ->  1,104

The conclusion those figures supported is unchanged — third-party visibility is genuinely enforced,
first-party is generated `PUBLIC` 678 times and refuses nothing, and `within_view` has zero uses. But
the figures themselves were wrong by a third, and they had been repeated because they sounded
measured. Every number in the handoff now ships with the command that produced it.

**And the fourth, found after the document was already written.** It claimed the domain lane's work
"never landed on any branch" — inferred from `git branch -r` and an empty `gh pr list`, both of which
were accurate and neither of which was the right question. The work was sitting **uncommitted in the
main checkout**: 8 modified files on `docs/ecosystem-plan-session`, a 7-rule expand with an
anchor-alias refactor. `git status` in the working directory would have said so in one command. Two
searches agreeing does not make them the right searches, and "no branch has it" is not "it does not
exist" when the repo's own rule against working in the main checkout is the thing being violated.

That checkout has already been wiped once this program, by a `git checkout … -- .` run as a supposed
no-op; uncommitted content destroyed that way is unrecoverable. The finding is therefore also a
near-miss: the handoff was about to instruct a fresh agent to redo work that already existed, in the
one directory where redoing it would have destroyed the original.

The shape common to all four: a claim that was true when first written, or never checked at all, or
checked by the wrong instrument, travelling forward under the authority of having been written down.
Restating is not verifying. The cheapest moment to catch it is the one where the claim is being
copied.

Every capability, evidence contract, jurisdiction binding, Korea control, review disposition, and
exposure state remains `HOLD`.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "review",
  "risk_class": "high",
  "risk_domains": [
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
    "Red Team": "Challenged inherited claims and reproduced how correct commands could answer the wrong question.",
    "Systems Thinking": "Separated repository, workstation, Kubernetes, and live OCI sources instead of treating them as one system.",
    "Operability / Day-2": "Converted uncertain state into explicit restart holds rather than an unsafe continuation instruction.",
    "Blast-radius / cell-based": "Kept every infrastructure and exposure mutation on HOLD while facts remained contradictory.",
    "Telemetry-first": "Replaced remembered counts and branch inference with commands, exact identities, and direct readback.",
    "Zero-trust / defense-in-depth": "Required each carried-forward assertion to be independently re-established at its authoritative boundary."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Four handoff claims failed when checked against the authoritative repository, worktree, cluster, or OCI boundary."
  ],
  "decisions_changed_or_rejected": [
    "Rejected branch absence, local kubectl success, and repeated prose as sufficient evidence of current state."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
