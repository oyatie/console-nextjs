# Wave-4 migration-slot ledger — APPEND-ONLY

**Owner:** the integrator. **Status:** live from 2026-07-25.
**Governs:** `backend/crates/platform/db/migrations/**` (a shared collision root).

This file is the only authority for which migration number a lane may use.
Entries are **appended, never edited or reordered**. A struck entry gets a new
appended row recording the strike; the original row stays.

---

## 1. The protocol

1. A lane that needs a migration **requests a slot** — lane id, subject, one
   line on what the migration does — and then waits.
2. The **integrator appends** a row here and assigns the number.
3. The lane names its file `manifests/migration/<NNNN>_<subject>.sql` inside its
   own worktree and hands it to the integrator as a manifest. Lanes do not write
   into `backend/crates/platform/db/migrations/` directly.
4. **A lane never picks its own number.** In particular a lane never runs `ls`
   on the migrations directory, because that directory shows only *this*
   worktree — see §2 for why that is not the same question.

Requesting is cheap. Colliding is not — though the detection story is better
than this document originally claimed.

> **Corrected 2026-07-28.** The original text said a duplicate number is "not
> detected by `cargo`, `sqlx`, or any CI gate in this repo". That is **wrong**:
> `backend/ci/gates/migration-safety/src/lib.rs` buckets every `NNNN_` filename
> prefix by parent directory and emits `ViolationKind::DuplicateMigrationVersion`
> when more than one file shares a prefix (covered by
> `backend/ci/gates/migration-safety/tests/gate_detects_violation.rs`). It is
> wired at `.github/workflows/ci.yml` as the "Migration-safety gate" step
> (`cargo run -p console-gate-migration-safety`) in the `backend` job. The gate
> predates this document.
>
> The real hazard is narrower, and still real: the gate only fires when **both
> files are in one tree**. Pull-request CI runs against the merge ref, so a
> second lane *would* fail — but nothing forces a re-run when the base advances.
> `main` is currently **unprotected** (`gh api .../branches/main/protection` →
> 404): no required status checks, no "require branches up to date", no merge
> queue. So a lane whose check went green before the other lane merged stays
> mergeable, and detection lands **after** the merge, when the `push: main` run
> of the same gate turns `main` red. Deployment is contained — `image-release.yml`
> requires a successful exact-SHA `main` CI run — but `main` itself breaks.
>
> Reserving numbers up front therefore remains correct; the reason is
> merge-ordering, not the absence of a detector.

## 2. Why `ls` is the wrong instrument — the proven failure mode

Surveyed 2026-07-25 across every ref (`git ls-tree` on all
`refs/remotes/origin/*` and `refs/heads/*`, not `ls` on one worktree).

**`0197` is a live duplicate right now.** Two different subjects hold it:

| File | Where it lives |
|---|---|
| `0197_notice_audience_and_category.sql` | merged; on the spine and on this branch (`7dca8757`, 2026-07-24) |
| `0197_customer_site_registry_foundation.sql` | unmerged, on three live branches: `codex/customer-site-registry-foundation-20260724` (`54f9d658`), `codex/registry-current-base-review-fixes` (`5aef5164`), `review/registry-current-base-53bf6c0` (`5b139418`) |

Whichever lands second silently shadows or duplicates the first. Both authors
would have seen `0196` as the high-water when they ran `ls`.

**It is not an isolated slip.** Distinct subjects sharing one number, at ref
tips, today:

| Slot | Subject A | Subject B |
|---|---|---|
| 0092 | `add_payroll_access_features` | `create_absence_exit_workflow` |
| 0093 | `add_group_domain_role_scopes` | `settlement_certification` |
| 0117 | `comms_email_account_claim_token_fencing` | `search_trgm_indexes` |
| 0122 | `audit_events_action_created_at_idx` | `comms_account_by_address` / `create_leave_requests` (three-way) |
| 0124 | `audit_events_action_created_at_idx` | `workflow_trigger_binding_subject_kind` |
| 0125 | `comms_account_by_address` | `workflow_definition_pending_revision` |
| 0126 | `workflow_trigger_binding_subject_kind_idx` | `workflow_trigger_binding_subject_kind` |
| 0127 | `audit_events_action_created_at_idx` | `workflow_definition_pending_revision` |
| 0128 | `comms_account_by_address` | `workflow_trigger_binding_subject_kind_idx` |
| 0170 | `financial_purchase_request_queue_index` | `harden_object_policy_attachment_and_blockers` |
| 0181 | `financial_purchase_request_queue_index` | `leave_api_create_employee` |
| 0182 | `create_equipment_3r` | `financial_purchase_request_queue_index` |
| 0185 | `create_equipment_3r` | `financial_purchase_request_queue_index` |

Read the 0181/0182/0185 rows together: the same two migrations were renumbered
past each other three times by lanes each running `ls` on their own worktree.
That is the failure this ledger exists to end.

## 3. Seed state — surveyed 2026-07-25

Command of record:

```
for r in $(git for-each-ref --format='%(refname)' refs/remotes/origin refs/heads); do
  git ls-tree --name-only "$r" backend/crates/platform/db/migrations/
done | sed 's|.*/||' | grep -E '^0[0-9]{3}_' | sort -u
```

| Fact | Value |
|---|---|
| Highest slot on this branch (`claude/w4-epoch-20260725` @ `4cabe239`) | **0202** |
| Highest slot across **all** refs surveyed | **0202** — no ref carries anything above it |
| Refs at the 0202 high-water | `origin/codex/operational-object-runtime-progress`, `origin/codex/pr488-final-integration-v2-20260725` |
| `0202` subject | `0202_notification_policies_and_object_agg.sql` (`fe1e8b91`) |
| `0201` | **SPENT 2026-07-25** — released by `0201_release_reserved_evidence_retention_slot.sql`, a documented no-op. The reservation was the defect (see §5). Evidence-retention takes a fresh number at merge when it is written. |
| First free slot | **0203** |

## 4. Assignments

Appended by the integrator only. `assigned` means the number is spent — even if
the lane is later dropped, the number is not recycled.

| # | Slot | Assigned to | Subject | State | Appended |
|---|---|---|---|---|---|
| 1 | 0201 | integrator | release the reserved gap (documented no-op; closes NonContiguousMigrationVersion for every lane) | spent | 2026-07-25 |
| 2 | 0203 | hf-leaveapi-revoke | REVOKE PUBLIC on leave_api.assert_employee_directory_manager + restore the deny-by-default tripwire | assigned (swapped in 2026-07-25, see §5) | 2026-07-25 |
| 3 | — | L-X1 | deal aggregate — the CRM trunk | number assigned at merge (§5) | 2026-07-25 |
| 4 | — | L-X2 | deal stage transitions + per-stage evidence enum | number assigned at merge (§5) | 2026-07-25 |
| 5 | — | L-X3 | activity discipline + Closed-Lost reason enum + auto-Lost settings | number assigned at merge (§5) | 2026-07-25 |
| 6 | — | L-X4 | deterministic round-robin owner assignment + bulk reassignment | number assigned at merge (§5) | 2026-07-25 |
| 7 | — | L-X5 | Won → contract `C-` + large-deal threshold object | number assigned at merge (§5) | 2026-07-25 |
| 8 | — | L-X7 | ontology projections (deal / listing / inquiry) | number assigned at merge (§5) | 2026-07-25 |
| 9 | — | L-X8 | lead PII: consent, retention, masking, audited sensitive view | number assigned at merge (§5) | 2026-07-25 |
| 10 | 0204 | L-A1 | ontology catalog additive-upgrade path — assigned AT MERGE per §5 (pre-assignment had said 0211; taking the next free number keeps the sequence contiguous) | landed | 2026-07-25 |
| 11 | ~~0212~~ | hf-audit-guard | extend protected_audit_writer_guard to the two builtin-catalog audit actions (console_rt can currently forge them) | pre-assignment VOIDED 2026-08-09 — takes its number at merge per §5, exactly as row 10 did | 2026-07-25 |
| 12 | 0212 | admission/backend-p1-p5 | repair the logistics POD evidence reference check | assigned AT MERGE per §5 | 2026-08-09 |

Next free slot after the seeded assignments: **0213**.

### Why row 11's pre-assignment was voided rather than honoured

A reviewer read row 11 as reserving 0212 and asked the P1–P5 lane to "allocate the
next serialized free slot" instead. That was measured before it was believed, and it
turns the tree red:

```
$ cargo run -q -p console-gate-migration-safety          # 0212 occupied
console-gate-migration-safety: PASSED                                    EXIT=0

$ git mv .../0212_repair_....sql .../0213_repair_....sql # the prescribed fix
console-gate-migration-safety: FAILED - 1 violation(s):
  [NonContiguousMigrationVersion] ...: missing migration version 0212 before 0213
                                                                          EXIT=1
```

`origin/main` tops out at 0211, so 0212 is not merely *a* free slot — it is the only
legal one, and any lane that skips it fails the gate. A reserved number and a
contiguity gate cannot both be satisfied by the lane that merges first; this is the
same defect §5 was written for, and the same one row 10 already resolved by
overriding a pre-assignment of 0211. Rows carrying a bare slot number are therefore
advisory. **§5 governs: the number is taken at merge, by whoever merges first.**

### Standing hazards for these nine

- The 0197 duplicate above is **still unmerged on three branches**. If any of
  them is admitted, its file must be renumbered to a slot requested here first.
- `codex/equipment-evidence-custody-20260724` tops out at 0184 and
  `codex/console-fanout-planner-hardening-20260724` at 0183; both are ~19 slots
  behind and will renumber on admission. Neither may self-assign.

## 5. Requesting a slot

Open a request to the integrator with exactly this shape:

```
slot request
  lane:    L-XN
  subject: <snake_case_subject>          # becomes the filename suffix
  does:    <one line>
  tables:  <new tables, or "none">       # every new table needs FORCE RLS
```

The integrator appends a row to §4 and replies with the number. There is no
self-service path.

> **Integrator deviation, recorded 2026-07-25.** Slot 0211 was assigned after the
> integrator first mis-allocated 0203 by surveying migration *files* across refs
> instead of reading §4 — the exact failure rule 1.4 forbids. The lane caught it
> and did not self-reassign. Its migration was also written directly into
> `backend/crates/platform/db/migrations/` rather than handed over as a §1.3
> manifest, because the integrator briefed it that way; accepted as-is this once
> since the integrator lands it immediately, and a manifest round-trip would add
> a copy step with drift risk and no safety gain. §1.3 still stands for lanes the
> integrator is not landing in the same pass.

## 5. Policy correction — assign at LAND time, not at charter time

**The pre-assignment model in §1 was wrong, and `console-gate-migration-safety` was
right.** The gate requires contiguous versions from 0001
(`NonContiguousMigrationVersion`, `backend/ci/gates/migration-safety/src/lib.rs:132`).
That is not ceremony: a hole on the main line is a real hazard, because any
database already migrated past it rejects a migration that later lands *into*
it — sqlx's out-of-order guard. Pre-assigning 0203-0210 to lanes that have not
written them manufactures exactly those holes, so the ledger and the gate
encoded contradictory policies and every reserved gap reproduced the conflict.

Corrected protocol, effective now:

1. A lane develops against a **placeholder** number and never depends on it —
   proven trivial in practice: renumbering is a `git mv` plus a grep, because
   nothing in a migration body or its tests references its own version.
2. The **integrator assigns the real number at merge time**, taking the next
   number above the highest currently on the branch, so the sequence stays
   contiguous by construction and lands in the order it is applied.
3. §4 remains append-only as the record of what was spent. A swap before either
   lane has written anything is not recycling and is permitted; a swap after a
   migration exists is not.

The 0203 ↔ 0211 swap above is the first application: `hf-leaveapi-revoke` had a
written, tested migration and needed to land immediately; `L-A1` had written
nothing, so it moves up. This keeps the sequence contiguous and avoids opening
eight holes for a security hotfix that is ready now.

**Correction, same day:** the paragraph above originally kept `0201` as a
standing reserved gap and called it an acceptable single violation. That was
inconsistent with the very reasoning that produced this section — if a hole is a
real out-of-order hazard, a *permanent* hole is a permanent hazard, and every
wave-4 lane inherits the red. The L-A1 lane pushed back on it. `0201` is now
spent by a documented no-op migration, the sequence is contiguous from 0001, and
reserved gaps are no longer a thing this ledger creates.

### 5.1 A dropped slot is backfilled or the tail renumbers

Raised by L-A1 after the 0203↔0211 swap left it holding the highest assigned
number. If any of 0204–0210 is **dropped rather than landed**, 0211 leaves a
permanent contiguity gap behind it — the same defect as the old 0201
reservation, produced by lane attrition instead of pre-assignment.

Rule: when an assigned slot is abandoned, the integrator either backfills it
(a documented no-op, as `0201_release_reserved_evidence_retention_slot.sql`
does) or renumbers the tail down before the train lands. A gap is never left
open, because the number can never be safely reclaimed afterwards — a database
already past it rejects anything that later tries to fill it.

This is the general form of §5: **holes are not free, whoever creates them.**
