# Self-hosted S3 object store (DARK)

This directory stages ADR-0024 roadmap item #3 / GitHub issue #370 for an
on-prem, self-hosted S3-compatible endpoint. The app and Barman already speak the
S3 protocol; this lane stages the object-store substrate and documents how an
operator selects the `on-prem` endpoint context without changing application S3
client code.

References:

- ADR-0024 — Self-Host-First, Cloud-Portable Multi-Substrate + High Availability
  (`origin/main:docs/decisions/ADR-0024-bare-metal-portability-and-ha.md`).
- GitHub issue #370 — self-hosted S3 object storage + config-selectable endpoint.
- Checksum/backup evidence: `ops/dr/prerequisite-checklists/t_81d5480c-20260709T212739Z.md`.

The staged implementation uses the same SeaweedFS family and S3 port as the dev
reference in `ops/compose.yml`:

- image: `chrislusf/seaweedfs:4.32`
- command shape: `server -s3 -ip.bind=0.0.0.0 -dir=/data -s3.port=8333`
- in-cluster S3 endpoint:
  `http://console-object-store-s3.console-object-store.svc.cluster.local:8333`
- addressing mode for consumers: path-style (`CONSOLE_S3_FORCE_PATH_STYLE=true`)

## Deployment-context matrix

| Context | Argo/app path | Evidence/app endpoint | CNPG Barman endpoint | Checksum behavior | Credential source |
|---|---|---|---|---|---|
| `oci-guest` / current prod | `deploy/apps/console/overlays/prod` today; `deploy/apps/console/overlays/oci-guest` is the explicit ADR-0024 alias | OCI Object Storage S3-compatible endpoint in `ap-chuncheon-1` | OCI Object Storage `s3://mnt-db-backups/` through `oci-objectstore-creds` | Keep `AWS_REQUEST_CHECKSUM_CALCULATION=when_required` and `AWS_RESPONSE_CHECKSUM_VALIDATION=when_required`; this is an OCI compatibility workaround | OCI Vault recovery bundle projected to `console-secrets` and `oci-objectstore-creds` |
| `on-prem` / DARK activation context | `deploy/apps/console/overlays/on-prem` only after the on-prem substrate is approved | `http://console-object-store-s3.console-object-store.svc.cluster.local:8333`, region `us-east-1`, path-style | same in-cluster S3 Service via `console-cnpg-objectstore-creds` | Do not inherit the OCI checksum workaround; SeaweedFS validation passed with default boto3/Barman checksum behavior | OpenBao/External Secrets for production activation; one-time Kubernetes secrets only for a rehearsal |

Selecting the endpoint context is a deployment decision, not an app-code change:

- Keep OCI guest deployments on OCI Object Storage by continuing to sync the live
  `maintenance` Application from `deploy/apps/console/overlays/prod` (or the
  explicit `overlays/oci-guest` alias once the app path is migrated).
- Select self-hosted S3 only by deploying the on-prem context
  `deploy/apps/console/overlays/on-prem` in an approved on-prem or rehearsal
  cluster. Do not point the current OCI guest at the in-cluster self-hosted
  endpoint unless a separate rollback/bridge ticket explicitly says so.
- Do not edit `backend/crates/platform/storage/**` or other S3 client code for
  this activation path. The app already builds `S3StorageConfig` from env.

## DARK boundary

These manifests are intentionally under `deploy/apps/object-store/`, not
`deploy/argocd/apps/`. The live app-of-apps root only watches
`deploy/argocd/apps/`, and `application.yaml` has no `syncPolicy.automated`
block. Merging this directory is therefore a reviewable no-op for production
traffic and does not alter the current OCI Object Storage endpoint.

Top-level `kubectl kustomize deploy/apps/object-store` renders only the dark
`AppProject` and manual-sync Argo `Application`. The workload stack itself is
reviewed with `kubectl kustomize deploy/apps/object-store/manifests` and is only
created after an operator explicitly applies/syncs the dark app.

## Files

- `project.yaml` — isolated Argo CD AppProject for the dark object-store stack.
- `application.yaml` — manual-sync Argo CD Application pointing at
  `deploy/apps/object-store/manifests`.
- `manifests/namespace.yaml` — restricted `console-object-store` namespace.
- `manifests/services.yaml` — headless StatefulSet service plus the consumer S3
  ClusterIP service `console-object-store-s3` on port `8333`.
- `manifests/statefulset.yaml` — single-pod SeaweedFS S3 StatefulSet with a
  `50Gi` PVC that uses the activation context's default replicated StorageClass,
  plus credential references to `console-object-store-credentials`.
- `manifests/networkpolicy.yaml` — allows S3 ingress only from the `maintenance`
  and `console-object-store` namespaces.

## Activation prerequisites

1. Founder/operator approval for an on-prem or rehearsal cluster. Do not sync
   this app into the current single-node `oci-guest` cluster.
2. The DARK replicated-storage app, or an equivalent operator-approved storage
   substrate, is activated first and provides the default replicated StorageClass
   for the SeaweedFS PVC. If object storage gets a dedicated class, add that via
   a later activation overlay instead of editing this dark base.
3. OpenBao/External Secrets is the production secret source for `on-prem`. Manual
   `kubectl create secret` commands below are acceptable only for a rehearsal and
   must be replaced by the selected secret manager before production data moves.
4. NetworkPolicy/equivalent egress from the `maintenance` namespace to
   `console-object-store-s3.console-object-store.svc.cluster.local:8333` is
   allowed before app/worker/CNPG consumers are switched.
5. Required buckets exist before consumers move: `mnt-evidence`,
   `mnt-evidence-replica` if the current app config remains unchanged, and
   `mnt-db-backups` for CNPG/Barman. Production evidence durability still needs a
   second physical site or equivalent independent failure domain; creating both
   evidence buckets on this single StatefulSet is only rehearsal/local-integrity
   evidence, not ADR-0024 multi-site WORM durability.
6. A signed S3 smoke, bucket put/get/delete check, WORM/object-lock validation,
   and CNPG Barman backup/restore drill are captured before any production
   endpoint cutover. The prior SeaweedFS evidence lives in
   `docs/evidence/t1.4-seaweedfs-worm.md` and
   `ops/dr/prerequisite-checklists/t_81d5480c-20260709T212739Z.md`; rerun against
   the activated cluster instead of copying those results blindly.

## Secret setup

No credentials are committed. Use the selected secret manager for production;
the following names and keys are the Kubernetes consumer contract.

SeaweedFS S3 server credentials in the object-store namespace:

```sh
kubectl create namespace console-object-store
kubectl -n console-object-store create secret generic console-object-store-credentials \
  --from-literal=AWS_ACCESS_KEY_ID='<object-store-access-key>' \
  --from-literal=AWS_SECRET_ACCESS_KEY='<object-store-secret-key>'
```

CNPG/Barman credentials in the maintenance namespace. The on-prem overlay expects
the same key names as the live OCI Barman secret, but under the on-prem secret
name `console-cnpg-objectstore-creds`:

```sh
kubectl -n console create secret generic console-cnpg-objectstore-creds \
  --from-literal=ACCESS_KEY_ID='<object-store-access-key>' \
  --from-literal=ACCESS_SECRET_KEY='<object-store-secret-key>'
```

App/worker evidence credentials continue to come from `console-secrets` via
`envFrom`. For on-prem, project the approved self-hosted S3 credentials into
these existing keys; do not rename them in app manifests:

```text
CONSOLE_S3_ENDPOINT_URL=http://console-object-store-s3.console-object-store.svc.cluster.local:8333
CONSOLE_S3_REGION=us-east-1
CONSOLE_S3_FORCE_PATH_STYLE=true
CONSOLE_S3_ACCESS_KEY_ID=<approved self-hosted S3 access key, from secret>
CONSOLE_S3_SECRET_ACCESS_KEY=<approved self-hosted S3 secret key, from secret>
CONSOLE_S3_PRIMARY_BUCKET=mnt-evidence
CONSOLE_S3_REPLICA_BUCKET=mnt-evidence-replica
```

SeaweedFS uses `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` as fallback S3
credentials when no S3 config file is present. Rehearsals may use one credential
pair for all consumers; production should prefer least-privilege, bucket-scoped
users if the selected SeaweedFS configuration supports them.

## Manual activation flow

Render first, then apply/sync only in the approved context:

```sh
kubectl kustomize deploy/apps/object-store
kubectl kustomize deploy/apps/object-store/manifests
kubectl apply -k deploy/apps/object-store
kubectl get application console-object-store-dark -n argocd -o yaml
argocd app sync console-object-store-dark
```

Confirm that the Application still has no `syncPolicy.automated` before syncing.
After sync, verify the pod and S3 path-style endpoint with signed requests:

```sh
kubectl -n console-object-store rollout status statefulset/console-object-store
kubectl -n console-object-store get svc console-object-store-s3 -o yaml
kubectl -n console-object-store port-forward svc/console-object-store-s3 8333:8333
```

From a separate shell while the port-forward is active:

```sh
AWS_ACCESS_KEY_ID='<object-store-access-key>' \
AWS_SECRET_ACCESS_KEY='<object-store-secret-key>' \
aws --endpoint-url http://127.0.0.1:8333 s3api list-buckets
```

Create the required buckets before moving consumers:

```sh
for bucket in mnt-evidence mnt-evidence-replica mnt-db-backups; do
  AWS_ACCESS_KEY_ID='<object-store-access-key>' \
  AWS_SECRET_ACCESS_KEY='<object-store-secret-key>' \
  aws --endpoint-url http://127.0.0.1:8333 s3api create-bucket --bucket "$bucket"
done
```

Run a signed object smoke for each bucket. Use non-production test keys and clean
them up afterward:

```sh
printf 'object-store-smoke %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > /tmp/console-s3-smoke.txt
for bucket in mnt-evidence mnt-evidence-replica mnt-db-backups; do
  AWS_ACCESS_KEY_ID='<object-store-access-key>' \
  AWS_SECRET_ACCESS_KEY='<object-store-secret-key>' \
  aws --endpoint-url http://127.0.0.1:8333 s3 cp /tmp/console-s3-smoke.txt "s3://$bucket/smoke/console-s3-smoke.txt"
  AWS_ACCESS_KEY_ID='<object-store-access-key>' \
  AWS_SECRET_ACCESS_KEY='<object-store-secret-key>' \
  aws --endpoint-url http://127.0.0.1:8333 s3 cp "s3://$bucket/smoke/console-s3-smoke.txt" -
  AWS_ACCESS_KEY_ID='<object-store-access-key>' \
  AWS_SECRET_ACCESS_KEY='<object-store-secret-key>' \
  aws --endpoint-url http://127.0.0.1:8333 s3 rm "s3://$bucket/smoke/console-s3-smoke.txt"
done
```

The Kubernetes readiness/liveness probes use TCP checks because auth-enabled
SeaweedFS returns HTTP 403 to unsigned S3 root requests. A real readiness signoff
must include signed S3 API calls, bucket creation, object put/get/delete behavior,
WORM/object-lock validation, and a CNPG/Barman backup/restore drill before any
consumer endpoint is moved.

## Selecting the on-prem endpoint context

Render the explicit contexts and compare the endpoints before applying anything:

```sh
kubectl kustomize deploy/apps/console/overlays/oci-guest > /tmp/console-oci-guest.yaml
kubectl kustomize deploy/apps/console/overlays/on-prem > /tmp/console-on-prem.yaml

grep -n 'CONSOLE_S3_ENDPOINT_URL\|endpointURL\|AWS_REQUEST_CHECKSUM_CALCULATION\|AWS_RESPONSE_CHECKSUM_VALIDATION' /tmp/console-oci-guest.yaml
grep -n 'CONSOLE_S3_ENDPOINT_URL\|endpointURL\|AWS_REQUEST_CHECKSUM_CALCULATION\|AWS_RESPONSE_CHECKSUM_VALIDATION' /tmp/console-on-prem.yaml
```

Expected result:

- `oci-guest` shows the OCI Object Storage endpoint, region `ap-chuncheon-1`, and
  the OCI-only checksum env workaround.
- `on-prem` shows the in-cluster self-hosted endpoint, region `us-east-1`, the
  `console-cnpg-objectstore-creds` secret name, and no `AWS_*_CHECKSUM_*` env on the
  rendered CNPG `Cluster`.

Only after the object store, buckets, secrets, network path, WORM check, and
Barman drill are proven should an activation lane point the maintenance Argo
Application at `deploy/apps/console/overlays/on-prem` (or sync that overlay
in the approved on-prem cluster). Until then, the live `maintenance` Application
remains on `deploy/apps/console/overlays/prod`, so OCI guest deployments keep
using OCI Object Storage.

## Post-cutover verification

After the on-prem context is synced, verify the three consumer classes.

1. App/worker evidence configuration:

   ```sh
   kubectl -n console get configmap console-config -o jsonpath='{.data.CONSOLE_S3_ENDPOINT_URL}{"\n"}{.data.CONSOLE_S3_REGION}{"\n"}{.data.CONSOLE_S3_FORCE_PATH_STYLE}{"\n"}{.data.CONSOLE_S3_PRIMARY_BUCKET}{"\n"}{.data.CONSOLE_S3_REPLICA_BUCKET}{"\n"}'
   kubectl -n console get pods -l app=console-app -o wide
   kubectl -n console get pods -l app=console-worker -o wide
   ```

   Expected endpoint: `http://console-object-store-s3.console-object-store.svc.cluster.local:8333`.
   Expected buckets: `mnt-evidence` and `mnt-evidence-replica`. Use the app's
   normal evidence workflow or a controlled non-production evidence smoke to
   confirm primary/replica object writes and reads.

2. CNPG/Barman configuration:

   ```sh
   kubectl -n console get objectstore.barmancloud.cnpg.io console-backups -o yaml
   kubectl -n console get cluster.postgresql.cnpg.io console-db -o yaml
   kubectl -n console get scheduledbackup.postgresql.cnpg.io console-db-daily -o yaml
   ```

   Expected endpoint: the in-cluster self-hosted S3 Service. Expected credential
   secret: `console-cnpg-objectstore-creds`. Expected checksum posture: no inherited
   `AWS_REQUEST_CHECKSUM_CALCULATION=when_required` or
   `AWS_RESPONSE_CHECKSUM_VALIDATION=when_required` in the on-prem CNPG Cluster.

3. Backup/restore and object readability:

   - trigger or wait for a Barman backup into `s3://mnt-db-backups/`;
   - verify WAL archive objects appear;
   - restore into a scratch/recovery namespace or rehearsal cluster;
   - boot recovered Postgres and run a read probe against seeded rows;
   - range-read at least `backup.info`, a base tarball, and a WAL object with
     signed S3 requests.

   The previous local SeaweedFS drill proved this flow with default boto3/Barman
   checksum behavior. Production activation still needs fresh evidence from the
   selected cluster and storage substrate.

## Rollback

Rollback is context-dependent. Preserve data first, then move endpoints back.

### If only Argo visibility was staged

If `kubectl apply -k deploy/apps/object-store` was run but the dark Application
was never synced, delete the dark Application/Project objects. This does not
touch the live OCI app-of-apps root or any maintenance consumer endpoint:

```sh
kubectl -n argocd delete application console-object-store-dark --ignore-not-found
kubectl -n argocd delete appproject console-object-store-dark --ignore-not-found
```

### If SeaweedFS was synced but consumers did not move

Stop before deleting storage. Confirm no app/CNPG consumers use the self-hosted
endpoint, then preserve or export any rehearsal data:

```sh
kubectl -n console get configmap console-config -o jsonpath='{.data.CONSOLE_S3_ENDPOINT_URL}{"\n"}'
kubectl -n console get objectstore.barmancloud.cnpg.io console-backups -o jsonpath='{.spec.configuration.endpointURL}{"\n"}'
argocd app terminate-op console-object-store-dark || true
argocd app delete console-object-store-dark --cascade=false --yes
```

Do not prune the `console-object-store` PVC until the operator confirms that no legal
evidence, backup, WORM, or validation data exists only on that volume.

### If consumers were moved to `on-prem`

Use a maintenance window. Stop or drain writes if there is any chance new
evidence or Barman objects were written to self-hosted S3 and must be copied back
to OCI. Then roll the console Application back to the previous context:

1. Restore the maintenance Argo Application source path to
   `deploy/apps/console/overlays/prod` (or `overlays/oci-guest` if that is the
   explicit live path at rollback time), or sync the last known-good commit.
2. Restore the OCI-backed `console-secrets` S3 keys and `oci-objectstore-creds` from
   OCI Vault. Do not reuse self-hosted S3 credentials against OCI.
3. Sync the console Application and restart workloads if the cluster does not
   automatically roll pods after the ConfigMap/Secret change:

   ```sh
   argocd app sync maintenance
   kubectl -n console patch rollout.argoproj.io/console-app --type merge \
     -p "{\"spec\":{\"restartAt\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\"}}"
   kubectl -n console rollout restart deployment/console-worker
   ```

4. Verify rollback state:

   ```sh
   kubectl -n console get configmap console-config -o jsonpath='{.data.CONSOLE_S3_ENDPOINT_URL}{"\n"}{.data.CONSOLE_S3_REGION}{"\n"}'
   kubectl -n console get objectstore.barmancloud.cnpg.io console-backups -o jsonpath='{.spec.configuration.endpointURL}{"\n"}'
   kubectl kustomize deploy/apps/console/overlays/oci-guest | grep -n 'AWS_REQUEST_CHECKSUM_CALCULATION\|AWS_RESPONSE_CHECKSUM_VALIDATION'
   ```

   Expected rollback endpoint: the OCI Object Storage S3-compatible endpoint in
   `ap-chuncheon-1`. Expected checksum posture: the OCI-only Barman checksum
   workaround is present again.

5. After OCI app and CNPG verification pass, decide whether to keep the
   self-hosted S3 StatefulSet parked for forensics, export/migrate its objects,
   or delete it. Never delete the PVC before the backup/evidence owner signs off.

## Operational notes and known differences

- OCI Object Storage is a managed regional object store with OCI Customer Secret
  Keys and the Chuncheon S3-compatible endpoint. SeaweedFS is a self-hosted
  S3-compatible service; credentials, bucket lifecycle, versioning/object lock,
  backups, upgrades, and capacity alarms become operator responsibilities.
- The live OCI context needs `AWS_*_CHECKSUM_*=when_required` for Barman because
  OCI rejects boto3's default flexible-checksum chunked upload behavior. The
  self-hosted SeaweedFS drill accepted default checksum behavior and completed
  Barman backup/WAL/restore, so the on-prem overlay removes the OCI workaround.
  Re-test if a newer accepted decision changes the implementation or a newer
  SeaweedFS image is selected.
- The self-hosted endpoint uses `us-east-1` as a SigV4 region label by convention;
  it is not a cloud-region durability claim.
- S3-compatible implementations may differ from OCI in bucket creation semantics,
  lifecycle-policy names, multipart defaults, object-lock/versioning setup,
  error codes, and admin/tenant identity models. Validate the exact selected
  implementation with signed API calls instead of assuming OCI behavior.
- This DARK manifest is a single StatefulSet/PVC. It is suitable for review and
  rehearsal, but it is not by itself ADR-0024 HA or multi-site evidence
  durability. Production needs replicated block storage plus a separate-site
  WORM/evidence replica plan.
- TCP probes prove only socket readiness. They do not prove credentials, bucket
  permissions, object integrity, object-lock retention, Barman PITR, or app
  evidence behavior.
