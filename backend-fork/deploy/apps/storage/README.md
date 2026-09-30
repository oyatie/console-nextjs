# Dark on-prem replicated storage stage

This directory stages the ADR-0024 on-prem replicated block-storage app without
changing the live OCI guest. The selected first backend for issue #379 is
**Longhorn**, exposed through the console-owned StorageClass name
`console-pg-hot`.

It is intentionally under `deploy/apps/storage/`, not `deploy/argocd/apps/`, and
`application.yaml` has no `syncPolicy.automated` block. A merge to `main` is
therefore a no-op for the current Argo app-of-apps root, because
`deploy/argocd/root.yaml` only watches `deploy/argocd/apps/`.

## What is staged

- `project.yaml` creates an isolated Argo CD project for the dark on-prem storage
  app without broadening the live `maintenance` AppProject.
- `application.yaml` defines a manual-sync Argo CD Application with three
  sources: the upstream Longhorn Helm chart, this repo's `values.yaml`, and this
  repo's on-prem manifests under `manifests/`.
- `values.yaml` disables the vendor-named chart StorageClass so workloads use the
  stable maintenance contract instead of `longhorn`.
- `manifests/namespace.yaml` declares `longhorn-system` with privileged Pod
  Security labels required by Longhorn's host-mounted engine/CSI workloads.
- `manifests/storageclass-console-pg-hot.yaml` declares the on-prem default
  replicated block StorageClass:
  - `provisioner: driver.longhorn.io`
  - `numberOfReplicas: "3"`
  - `fsType: ext4`
  - `allowVolumeExpansion: true`
  - `reclaimPolicy: Retain`
  - `volumeBindingMode: WaitForFirstConsumer`
  - `storageclass.kubernetes.io/is-default-class: "true"`

The canonical-name pattern is adapted from oyatie ADR-0161, but normalized to the
maintenance repo and bound to Longhorn for this first DARK lane. Rook-Ceph remains
a future scale-up path, not the backend staged here.

## Activation prerequisites

Do not sync this app into the current `oci-guest` single-node cluster. Before any
manual on-prem activation, verify and record:

1. At least three eligible Kubernetes worker/storage nodes or failure domains.
2. Durable Longhorn data disks on each storage node, mounted at or intentionally
   mapped to `/var/lib/longhorn`.
3. Talos node images/config include the Longhorn host dependencies for the chosen
   Longhorn release, including iSCSI tooling for RWO block volumes and the
   util-linux/mount tooling Longhorn uses for host volume operations. If RWX/NFS
   features are enabled later, verify NFS client support separately.
4. Nodes intended for automatic default-disk creation are labeled according to
   Longhorn's `create-default-disk` workflow before the first sync, because
   `values.yaml` sets `createDefaultDiskLabeledNodes: true`.
5. The on-prem CNPG overlay explicitly uses `spec.storage.storageClass:
   console-pg-hot`; do not patch `deploy/apps/console/base/database.yaml` away
   from the current single-node OCI posture.
6. Issue #371's production-hardening gate rewrite and founder/operator cutover
   approval are complete before wiring this app into live Argo CD.

## Sizing assumptions

- The staged StorageClass uses three Longhorn replicas for PostgreSQL data.
- Raw disk consumption is approximately three times requested PVC size, plus
  snapshots/backups/replica rebuild headroom.
- `console-db` currently requests `5Gi`; keep substantially more than `15Gi` usable
  raw Longhorn capacity available before even a small rehearsal.
- Production sizing must account for WAL/base backup retention, failover drills,
  replica rebuild traffic, node maintenance windows, and the future CNPG
  `instances: 3` on-prem overlay.

## Manual activation flow

1. Render locally first:

   ```sh
   kubectl kustomize deploy/apps/storage
   kubectl kustomize deploy/apps/storage/manifests
   helm template longhorn longhorn --repo https://charts.longhorn.io \
     --version 1.12.0 --namespace longhorn-system \
     -f deploy/apps/storage/values.yaml
   ```

2. On the approved on-prem cluster only, stage Argo visibility without syncing the
   chart yet:

   ```sh
   kubectl apply -k deploy/apps/storage
   ```

3. Review the generated Argo CD Application and confirm it still has no automated
   sync policy:

   ```sh
   kubectl get application longhorn-storage-onprem -n argocd -o yaml
   ```

4. Manually sync after node prerequisites are verified:

   ```sh
   argocd app sync longhorn-storage-onprem
   ```

5. Only after Longhorn is healthy should the separate on-prem CNPG overlay be
   synced with `storageClass: console-pg-hot` and `instances: 3`.

## Verification

After a manual sync on the on-prem cluster:

```sh
kubectl -n longhorn-system get pods
kubectl get storageclass console-pg-hot -o yaml
kubectl get storageclass local-path -o yaml  # OCI only; verify it remains the current guest default there
```

Confirm `console-pg-hot` has `driver.longhorn.io`, three replicas, `Retain`,
`WaitForFirstConsumer`, and the default-class annotation in the on-prem context.
Then create a disposable PVC that explicitly sets `storageClassName: console-pg-hot`,
wait for it to bind with a test pod, delete the test workload, and retain the PVC
until the Longhorn UI/CRs show the expected healthy replicas.

A real production cutover must also include a CNPG primary pod/node kill drill and
capture automatic promotion plus data-continuity evidence before any DNS or user
traffic is moved.

## Rollback

If only Argo visibility was staged and the app was never synced, delete the dark
Application/Project objects created by `kubectl apply -k deploy/apps/storage`.
That does not touch the live OCI app-of-apps root.

If Longhorn was synced:

1. Stop new consumers by reverting or pausing the on-prem CNPG overlay first.
2. Remove the default marker from `console-pg-hot` before returning another class to
   default status:

   ```sh
   kubectl annotate storageclass console-pg-hot \
     storageclass.kubernetes.io/is-default-class- --overwrite
   ```

3. Do not prune/delete Longhorn while any PVC/PV still points at
   `driver.longhorn.io`; migrate or intentionally delete those volumes first.
4. For the OCI guest, keep using `deploy/infra/local-path/` and
   `deploy/argocd/apps/local-path.yaml`; this DARK storage stage does not modify
   either file.
