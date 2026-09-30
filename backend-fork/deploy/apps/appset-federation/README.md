# Dark ApplicationSet federation activation guide

This directory is the DARK staging area for ADR-0024 / issue #380 multi-cluster
Argo CD federation. It is intentionally under `deploy/apps/appset-federation/`,
not under `deploy/argocd/apps/`; the current live root (`deploy/argocd/root.yaml`)
only watches `deploy/argocd/apps/`, so merging these files must not activate
federation by itself.

The production flow remains the hand-applied single-cluster root until a founder
or operator explicitly promotes this package. Do not apply these manifests to a
live Argo CD control plane until the prerequisites below are satisfied.

## What this package is expected to stage

- `applicationset.yaml` — an Argo CD `ApplicationSet` using the clusters
  generator to create one app-of-apps root per registered cluster.
- `app-groups/warm-standby/` — standby-safe child Applications rendered only for
  clusters labeled as warm standby.
- `examples/cluster-secrets/` — non-secret registration examples and label
  contract notes. The examples must never include kubeconfigs, bearer tokens,
  client certificates, or encoded cluster Secret data.

Primary clusters continue to source the existing app-of-apps path:

- `repoURL: https://github.com/jason931225/console.git`
- `targetRevision: main`
- `path: deploy/argocd/apps`

Warm-standby clusters source the DARK standby app group path:

- `path: deploy/apps/appset-federation/app-groups/warm-standby`

That split keeps the one-primary/single-cluster case equivalent to today's Argo
root while preventing a standby cluster from accidentally running the live
write-active Maintenance overlay before the storage/traffic failover lanes are
ready.

## Cluster registration prerequisites

Register clusters with Argo CD only after all of these are true:

1. The target Argo CD control plane has the ApplicationSet controller/CRDs
   installed. The current Argo CD install path already needs the ApplicationSet
   CRD server-side apply path documented in `deploy/OPS-RUNBOOK.md`.
2. The cluster to register has an operator-approved name and site/residency
   assignment. Use stable slugs; do not encode personal names or private data in
   labels.
3. Cluster credentials are sourced from External Secrets + OpenBao, or from the
   repo's current approved secret-management path in `deploy/SECRETS.md` and
   `deploy/OPS-RUNBOOK.md`. Secret material may be reconciled into the cluster,
   but it must never be committed to this repository.
4. The cluster Secret is created in the `argocd` namespace with the label contract
   below. The `argocd.argoproj.io/secret-type=cluster` label is mandatory for
   Argo CD's cluster generator.
5. The standby data plane is explicitly known: backup/restore, replication, DNS
   or VIP ownership, and traffic hold status are documented before a standby is
   labeled promotion-ready.

## Required cluster Secret labels

Every cluster selected by the federation must have these labels on its Argo CD
cluster Secret:

| Label | Required value / example | Purpose |
| --- | --- | --- |
| `argocd.argoproj.io/secret-type` | `cluster` | Makes the Secret an Argo CD cluster registration. |
| `console.io/federation` | `enabled` | Opts into this ApplicationSet. Remove or change this label to hold a cluster out. |
| `console.io/environment` | `prod` | Keeps production federation separate from dev/test registrations. |
| `console.io/site` | `oci-iad`, `onprem-kr-a`, `onprem-kr-b` | Stable site/cell slug used in generated app labels and runbooks. |
| `console.io/dr-role` | `primary` or `warm-standby` | Chooses the primary app-of-apps path or the standby app group. |
| `console.io/residency` | `kr`, `us-east`, `eu` | Prevents accidental cross-residency failover. |

Recommended optional labels:

| Label | Example | Purpose |
| --- | --- | --- |
| `console.io/traffic` | `active` or `held` | Traffic must be `active` only for the site primary. |
| `console.io/standby-mode` | `warm` or `none` | Documents whether standby controllers/workloads should be ready. |
| `console.io/storage-profile` | `local-path`, `replicated`, `restored-replica` | Helps failover reviewers verify the data plane. |
| `console.io/registration-source` | `external-secrets-openbao` | Proves the Secret is reconciled from an approved source. |

Validity rules:

1. During activation, each `console.io/site` may have exactly one
   `console.io/dr-role=primary` cluster.
2. A site may have zero or more `warm-standby` clusters, but standby clusters must
   not receive production traffic before the failover runbook promotes them.
3. A failover pair should share `console.io/residency` unless the tenant or
   site residency policy explicitly permits the move. Residency conflicts fail
   closed: do not promote and do not move traffic.
4. Cluster credential data is not documentation. Only labels, Secret names,
   ExternalSecret templates, and remote-reference paths may be committed.

## Non-secret label example

This example is intentionally metadata-only. It demonstrates the labels the
cluster generator selects; it is not an Argo CD cluster credential and is not
applyable as a real cluster registration.

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: console-onprem-kr-a
  namespace: argocd
  labels:
    argocd.argoproj.io/secret-type: cluster
    console.io/federation: enabled
    console.io/environment: prod
    console.io/site: onprem-kr-a
    console.io/dr-role: primary
    console.io/residency: kr
    console.io/traffic: active
    console.io/standby-mode: none
    console.io/storage-profile: replicated
    console.io/registration-source: external-secrets-openbao
# Real Argo CD cluster Secrets also contain name/server/config data. That data
# must come from External-Secrets/OpenBao or another approved secret path and must
# never be committed.
```

See `examples/cluster-secrets/README.md` for a fuller non-secret ExternalSecret
shape.

## Activation prerequisites

Do not wire this DARK package into the live Argo CD flow until all prerequisites
are recorded in the activation ticket/runbook:

1. The manifest implementation and validation cards are complete, including a
   render check for one primary and for primary + warm-standby registrations.
2. `npm run check:production-hardening` still passes, and
   `deploy/argocd/root.yaml` plus `deploy/argocd/apps/console.yaml` still
   track `targetRevision: main`.
3. No real cluster credentials, kubeconfigs, bearer tokens, certificate keys, or
   base64-encoded Secret payloads are present in the git diff.
4. At least one primary cluster registration exists for the site being activated.
   A standby registration may exist, but it must be labeled
   `console.io/traffic=held` until the failover runbook promotes it.
5. The current hand-applied `root` Application and the generated primary root are
   not both allowed to own the same child Applications in the same Argo CD control
   plane. The activation plan must choose one owner and avoid duplicate ownership.
6. Backup/restore or replication evidence exists for the database and object
   storage path the standby will rely on. If the broader ADR-0024 failover lane
   owns this evidence, activation waits for that lane's approval.
7. DNS/VIP routing ownership is known. Moving traffic is outside this DARK
   package and must be coordinated with the VIP/ingress and DR runbooks.

## Activation steps

1. Re-read the current issue/card and confirm the activation approval is in scope.
   This README is not approval by itself.
2. Register or reconcile the cluster Secrets through External-Secrets/OpenBao or
   the approved secret-management path. Verify only labels and Secret names are
   committed.
3. Render the package locally:

   ```sh
   kubectl kustomize deploy/apps/appset-federation
   ```

4. In a scratch or staging Argo CD control plane, verify the generated primary
   root points at `deploy/argocd/apps` with `targetRevision: main`, and the
   generated standby root points at
   `deploy/apps/appset-federation/app-groups/warm-standby`.
5. Pause or remove the current hand-applied `root` Application only as specified
   by the operator-approved cutover plan. Do not let both root owners reconcile
   the same live child Applications.
6. Apply this DARK package deliberately:

   ```sh
   kubectl apply -k deploy/apps/appset-federation
   ```

7. Sync the generated primary root first and verify the child Applications match
   today's live behavior. Keep generated standby roots held until their standby
   app group and data-plane evidence are approved.
8. Record activation evidence: selected clusters, labels, generated root names,
   Argo health/sync states, database/storage readiness, traffic state, and the
   rollback command chosen for this cutover.

## Rollback / deactivation

Choose the narrowest safe rollback for the activation stage reached:

- **Before applying the ApplicationSet:** remove or correct the cluster labels;
  no live Argo resources should have changed.
- **After applying the ApplicationSet but before traffic moves:** delete or
  suspend the ApplicationSet, then re-apply the previous hand-owned root if it
  was paused. Confirm generated Applications are gone or orphaned intentionally
  before reenabling the old root.
- **After a primary cutover:** restore the previous root owner and cluster labels
  only if the data plane has not accepted divergent writes. If writes may have
  diverged, use the CNPG/object-storage recovery runbooks rather than relabeling
  blindly.
- **After DNS/VIP traffic moves:** move traffic back only after the old primary is
  healthy and has the authoritative data state. Record TTLs, VIP holder, and
  health-check evidence.

A deactivated cluster can be held out of federation by removing
`console.io/federation=enabled` or setting `console.io/traffic=held`, then
reconciling the ApplicationSet and verifying no generated root remains for that
cluster.

## Primary-to-warm-standby promotion

The detailed DR procedure lives in `ops/dr/multicluster-failover-runbook.md`.
The short form is:

1. Declare an incident or planned failover and freeze writes on the current
   primary unless the storage layer proves single-writer promotion safety.
2. Verify the candidate standby shares an allowed `console.io/residency`, has
   fresh approved secrets, and has current backup/replication evidence.
3. Promote or restore the database/storage layer first; do not move Argo labels
   while the standby is still data-stale.
4. Relabel the old primary to `console.io/dr-role=warm-standby` or hold it out
   with `console.io/federation!=enabled`; relabel exactly one standby to
   `console.io/dr-role=primary` and `console.io/traffic=active`.
5. Reconcile the ApplicationSet and verify the generated root for the new primary
   sources `deploy/argocd/apps` at `targetRevision: main`.
6. Move DNS/VIP/GeoDNS traffic only after app, database, ingress, and audit checks
   pass.

Coordinate this promotion with the ADR-0024 roadmap item #6 owner only when that
owner explicitly also owns broader failover orchestration for the current release.
Roadmap item #9 owns context-aware CI hardening, not promotion authority, so the
promotion remains founder/operator gated unless failover orchestration is assigned
explicitly.
