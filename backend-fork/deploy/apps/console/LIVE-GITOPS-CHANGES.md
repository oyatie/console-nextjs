# Live GitOps changes

> **POST-PIVOT UNVERIFIED / HOLD:** The topology and sync claims below are a
> historical change ledger, not proof that Argo CD currently reconciles this
> repository or that merging mutates a live environment. The repository
> currently authorizes zero production mutations. Start with the
> [disk-wipe consolidation handoff](../../../docs/handoffs/2026-08-03-disk-wipe-consolidation.md).

ArgoCD syncs `deploy/apps/console/overlays/prod` from `main` with `targetRevision: main`.
A change to any live input therefore takes effect **the instant it merges** — there is no
separate deploy step to catch it, and no environment between the merge and production.

`scripts/check-command-database-wiring.test.mjs` enforces two different things about those
paths. It refuses the DARK governed-command-database topology **by name**, and it refuses
any change at all that is not declared **here**. The second is the backstop for a topology
nobody has named yet.

## The rule

Changing any of these paths requires an entry below naming each changed path:

- `deploy/argocd/apps/console.yaml`
- `deploy/apps/console/base`
- `deploy/apps/console/overlays/prod`
- `deploy/apps/secrets-management/wiring`

The gate reads the **diff** of this file against `origin/main`, not its contents. A path
named in an earlier entry does not buy silence for a later change — each change declares
itself. That is the whole cost: one entry, in the same commit as the change.

This exists because the check used to be an unconditional byte-identity assertion against
`origin/main`, which no branch carrying a change could ever satisfy. A 90-day retention
policy was withdrawn rather than landed for that reason on 2026-07-31. A control with no
exception route does not hold the line; it gets deleted by whoever needs the next change
badly enough.

---

## 2026-07-31 — a finite backup retention window

**Changed:** `deploy/apps/console/base/database.yaml`

The `console-backups` ObjectStore declared no `retentionPolicy`, so barman-cloud never
pruned base backups or WALs and point-in-time recovery reached back to the first backup
forever. Now `35d`.

That window is the erasure horizon: for as long as the archive covers a person's lifetime
in the database, deleting their row does not make them unreconstructable. Korean law sets
no duration for it — the only backup provision in Korean privacy law, 개인정보의 안전성
확보조치 기준 제11조, requires a backup-and-recovery *plan* above a subject-count threshold
and states no period. The statutory retention floors (근로기준법 제42조, 국세기본법
제85조의3제2항) attach to records the live database holds, not to the archive. ADR-0037
carries the full table and the citations.

So the number comes from the operational question instead — how long corruption can go
undiscovered — and payroll's monthly cycle sets it. One cycle plus slack.

**Safe to set now, expensive later:** verified on 2026-07-31 that no CNPG cluster in the
tenancy declares a backup, the `barmancloud` ObjectStore CRD is not installed, and the
`console` namespace does not exist. There are no backups for this policy to prune. Setting
a retention policy after a production archive exists deletes history.

The number is the owner's and counsel's to change. ADR-0015 constrains recovery *speed*
(RPO ≤ 5 min, RTO ≤ 1 h) and says nothing about window *length*; neither is affected by
this change.
