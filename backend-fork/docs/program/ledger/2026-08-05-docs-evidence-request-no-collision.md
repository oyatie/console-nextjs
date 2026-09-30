# Ledger — docs evidence work_order request_no uniqueness (2026-08-05)

## Identity

- Slice: main PG red repair after #577 cargo lane
- Failed run: https://github.com/jason931225/console/actions/runs/31015221797
- Target: `docs-rest-evidence-rest-rls-surfaces-as-runtime-role-pg`
- Test: `storage_attestation_controls_original_verification_and_derivative_meaning`

## Decision

Seed `work_orders.request_no` with `EVD-{work_order_id}` instead of
`20260724-{:03}` from `uuid % 1000`. The 3-digit space collides under
unique `(org_id, request_no)` when multiple tests share one disposable DB.

## Non-goals

- No production schema change
- No weakening of the RLS suite

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "other"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Pragmatism",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Log shows Pg 23505 on work_orders_org_request_no_key for request_no 20260724-643, not an RLS assertion failure.",
    "Essentialism / YAGNI": "One seed uniqueness change in the fixture; no harness or CI graph change.",
    "Pragmatism": "Restores main Required CI without demoting disposable PostgreSQL reachability.",
    "Red Team": "No authz policy change; fixture id collision only.",
    "Operability / Day-2": "CI red only; no deploy path.",
    "Blast-radius / cell-based": "docs/rest test fixture only.",
    "Zero-trust / defense-in-depth": "Does not weaken RLS suite coverage."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Main CI run 31015221797 PG job failed solely on duplicate request_no seed collision."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
