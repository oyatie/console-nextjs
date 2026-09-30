# OPS Runbook — bare-metal/on-prem Talos HA cluster (DARK)

This is the operator runbook for the future ADR-0024 `on-prem-ha` substrate. It
is parallel to the live OCI guest runbook in [`OPS-RUNBOOK.md`](OPS-RUNBOOK.md),
not a replacement for it. The live `oci-guest` cluster stays the single-node
Oracle Ampere A1 target until a founder/operator activation ticket explicitly
cuts over a real on-prem cluster.

Use this file when the target is a bare-metal Talos HA cluster. Use
`OPS-RUNBOOK.md` for the current OCI node, OCI Vault, OCI Bastion, OCI object
storage, the A1 free-tier limits, the `dd` boot-volume workaround, and the OCI
MTU 9000-to-1500 workaround. Do not apply OCI-only warnings such as "never run a
second A1" to bare metal; the on-prem target requires multiple nodes.

## 0. Choose the right path first

| Question | `oci-guest` answer | `on-prem` answer |
|---|---|---|
| Where is the cluster? | OCI ap-chuncheon-1, one Ampere A1 VM | Operator-owned bare metal, colo, or lab hardware |
| Bootstrap shape | Talos image/import or `dd` to OCI boot volume | Talos install media/PXE/IPMI plus Cluster API/Metal3 templates |
| Control plane | One schedulable control-plane node | Three dedicated control-plane/etcd members, not general workers |
| Secrets root | OCI Vault plus manually created Kubernetes secrets | OpenBao HA Raft plus External Secrets Operator |
| Storage | OCI Object Storage for backups/evidence and local-path PVCs | Replicated block storage (`console-pg-hot`, Longhorn first) plus self-hosted S3 |
| Ingress | Reserved OCI IP `140.245.68.253` and hostPort Traefik | On-prem VIP/LoadBalancer such as MetalLB L2 plus multi-replica Traefik |
| Time/MTU | OCI link-local NTP and VNIC MTU workaround | Site NTP/chrony/GPS and the real fabric MTU |

Production-hardening properties for this context are deliberately different from
`oci-guest`: OpenBao/External Secrets is the acceptable secret store, the object
store endpoint must be the selected self-hosted S3-compatible service with
retention/WORM/replication recorded, CNPG must run at least three instances on
replicated storage, and automatic failover is not claimed until etcd/API, CNPG,
VIP/ingress, and restore drills have been captured on the real substrate.

Stop if you cannot answer which column you are operating. A mixed procedure is a
failed preflight.

## 1. Prerequisites before touching hardware

Record these facts in the activation ticket and keep secret values out of git,
chat, and issue comments.

1. Approval and scope:
   - ADR-0024 is the governing portability/HA direction.
   - The activation ticket names the target site/cell, DNS names, residency
     constraints, and rollback target.
   - The current OCI guest production path is either explicitly out of scope or
     the rollback target. Do not delete or mutate OCI manifests as part of this
     runbook.
2. Node inventory:
   - Three named control-plane nodes with stable management/IPMI/BMC access,
     stable Talos API addresses, and dedicated install disks.
   - At least two workers for app/ingress, and at least three eligible
     worker/storage nodes or failure domains before claiming replicated storage
     or CNPG HA.
   - CPU architecture for every node. ADR-0024 expects multi-arch images; do not
     assume OCI arm64-only images are sufficient for x86_64 hardware.
3. Network plan:
   - A stable Kubernetes API endpoint: VIP, load balancer, or DNS name such as
     `https://ON_PREM_CONTROL_PLANE_VIP:6443`.
   - Ingress VIP/pool reserved outside DHCP/router ownership. Never reuse the OCI
     public IP `140.245.68.253`.
   - VLAN/subnet/gateway, intended node interfaces, NetworkPolicy/CNI choice, and
     whether MetalLB L2 is valid or BGP is required.
4. Time and MTU:
   - Site NTP sources are reachable from every node. Use local chrony/GPS/PTP or
     approved upstream NTP; do not copy OCI's `169.254.169.254` NTP setting.
   - The fabric MTU is known end-to-end for node, storage, ingress, and operator
     paths. Do not copy the OCI VNIC 9000-to-1500 workaround unless the site
     proves the same blackhole; use the real site MTU.
5. Secrets and identity:
   - OpenBao operator owners, unseal-key custodians, root-token break-glass owner,
     and audit-log destination are named.
   - External Secrets Operator integration is planned before app secrets are
     considered production-managed.
   - Talos secrets, talosconfig, kubeconfig, OpenBao unseal shares, and service
     credentials have an out-of-band encrypted backup location.
6. Storage and data:
   - Longhorn `console-pg-hot` or the approved replicated StorageClass has enough raw
     capacity for three replicas, snapshots, rebuild headroom, and CNPG `instances: 3`.
   - SeaweedFS is recorded as the accepted self-hosted S3 reference; an engine change cites a newer accepted decision and fresh security/readiness evidence.
     Evidence/WORM replication to a second physical site is designed before live
     evidence data moves.
   - A restore source and rollback target exist for any production data import or
     cutover.
7. Tooling on the operator workstation:
   - `talosctl`, `kubectl`, `clusterctl`, `argocd`, `helm`, `kustomize` or
     `kubectl kustomize`, and repository checkout at the intended branch.
   - Access to BMC/IPMI/iDRAC/Redfish or remote hands for power-cycle and media
     attach operations.

## 2. Day-0 bootstrap flow

### 2.1 Prepare Talos media and machine config

1. Select the Talos and Kubernetes versions in the activation ticket. Keep them
   aligned with the current repo docs unless the ticket deliberately upgrades.
2. Use Talos Image Factory or a pinned local factory build to produce install
   media for each hardware architecture. Record the factory schematic ID, Talos
   version, image digest/checksum, and which extension set is used.
3. Boot each node from ISO, PXE, iPXE, or virtual media. Confirm the intended
   install disk before applying config:

   ```sh
   talosctl -n <node-ip> disks --insecure
   ```

4. Generate one cluster identity. Do not generate separate secrets per node:

   ```sh
   mkdir -p ./_talos-onprem
   talosctl gen secrets --output-file ./_talos-onprem/secrets.yaml

   export CONTROL_PLANE_ENDPOINT="https://ON_PREM_CONTROL_PLANE_VIP:6443"
   talosctl gen config maintenance "$CONTROL_PLANE_ENDPOINT" \
     --with-secrets ./_talos-onprem/secrets.yaml \
     --additional-sans ON_PREM_CONTROL_PLANE_VIP \
     --config-patch @deploy/talos/on-prem/cluster.patch.yaml \
     --config-patch-control-plane @deploy/talos/on-prem/controlplane.patch.yaml \
     --config-patch-worker @deploy/talos/on-prem/worker.patch.yaml \
     --output-dir ./_talos-onprem
   ```

   Prefer the checked-in renderer for repeatable per-node output:

   ```sh
   python3 deploy/talos/on-prem/render-machineconfigs.py \
     --inventory deploy/talos/on-prem/nodes.example.json \
     --output-dir ./_talos-onprem \
     --validate
   ```

   Keep `_talos-onprem/` out of git; it contains Talos secrets, talosconfig, and
   machineconfigs.

5. Confirm the generated on-prem control-plane config does not enable ordinary
   workload scheduling on control planes. `cluster.allowSchedulingOnControlPlanes:
   true` belongs to the single-node OCI guest only.
6. Apply configs to three control-plane nodes and at least two workers, then
   bootstrap exactly once:

   ```sh
   for node in CP1_IP CP2_IP CP3_IP; do
     talosctl apply-config --insecure -n "$node" \
       --file ./_talos-onprem/controlplane.yaml
   done

   talosctl --talosconfig ./_talos-onprem/talosconfig \
     --nodes CP1_IP --endpoints CP1_IP bootstrap

   for node in WORKER1_IP WORKER2_IP; do
     talosctl apply-config --insecure -n "$node" \
       --file ./_talos-onprem/worker.yaml
   done
   ```

7. Fetch kubeconfig and set Talos endpoints to the stable VIP plus useful
   break-glass node addresses:

   ```sh
   talosctl --talosconfig ./_talos-onprem/talosconfig kubeconfig ./_talos-onprem/kubeconfig
   talosctl --talosconfig ./_talos-onprem/talosconfig config endpoint \
     ON_PREM_CONTROL_PLANE_VIP CP1_IP CP2_IP CP3_IP
   talosctl --talosconfig ./_talos-onprem/talosconfig config node CP1_IP
   export KUBECONFIG=./_talos-onprem/kubeconfig
   ```

For detailed etcd member add/remove/replacement steps, use
[`talos/on-prem-ha-runbook.md`](talos/on-prem-ha-runbook.md).

### 2.2 Cluster API / Metal3 ownership flow

ADR-0024 expects Cluster API with `cluster-api-provider-metal3` for the durable
bare-metal lifecycle. The repo now stages a DARK CAPI/Metal3 template at
`deploy/talos/on-prem/capi-metal3.example.yaml`; manual Talos bootstrap remains a
rehearsal/bring-up path, not proof that lifecycle automation is production-ready.

For the CAPI ownership path, use this sequence:

1. Start from an approved management cluster and initialize the exact providers
   named by the CAPI lane. The command shape is:

   ```sh
   clusterctl init --bootstrap talos --control-plane talos --infrastructure metal3
   clusterctl get providers
   ```

   Do not let `clusterctl` silently choose kubeadm bootstrap/control-plane
   providers for a Talos cluster unless the activation ticket explicitly says the
   provider stack has changed.

2. Replace `deploy/talos/on-prem/nodes.example.json` with the approved inventory
   values in an operator scratch path: BMC endpoint, BMC credential Secret name,
   boot MAC, root device hints, architecture/image URL/checksum, site labels, and
   whether each host is control-plane, worker, or storage-capable.
3. Render and review the CAPI/Metal3 template:

   ```sh
   python3 deploy/talos/on-prem/render-capi-metal3.py \
     --inventory <approved-inventory.json> \
     --output /tmp/console-onprem-capi-metal3.yaml
   ```

   The rendered desired state must show one `TalosControlPlane` with three
   replicas, a worker `MachineDeployment`, role-scoped `Metal3MachineTemplate`
   resources, and inventory-backed `BareMetalHost` resources before the cluster is
   declared ready.
4. Apply the approved rendered template only after BMC credential Secrets and the
   Talos metal image URL/checksum exist in the management cluster.
5. Wait for CAPI to reconcile infrastructure, bootstrap data, Talos nodes,
   kubeconfig, and control-plane readiness. Use CAPI status, Talos health, and
   Kubernetes node readiness together; one green surface alone is not enough.
6. If the cluster was manually bootstrapped first, record whether the CAPI lane
   adopts it or rebuilds it. Do not let CAPI and hand-run `talosctl apply-config`
   fight over the same nodes.
7. After ownership is established, routine node add/replace operations should go
   through CAPI/Metal3, with the Talos member procedures used as break-glass
   recovery guidance.

### 2.3 Hand the cluster to GitOps without activating OCI-only paths

1. Install Argo CD on the on-prem cluster using the repo-approved Argo version and
   server-side apply where CRDs exceed annotation limits.
2. Apply only dark/on-prem apps until the activation ticket authorizes production
   wiring. Current dark app paths include:
   - `deploy/apps/cilium/` for the on-prem CNI stage;
   - `deploy/apps/storage/` for Longhorn and `console-pg-hot`;
   - `deploy/apps/vip-ingress/` for MetalLB L2 VIP provider;
   - `deploy/apps/traefik-onprem/` for LoadBalancer Traefik;
   - `deploy/apps/console/overlays/on-prem/` for CNPG `instances: 3` and
     replicated storage.
3. Keep the live OCI app-of-apps root and `deploy/argocd/apps/traefik.yaml` intact
   until a separate cutover deliberately replaces them in the target cluster.

## 3. OpenBao and External Secrets operator handling

The on-prem path should not depend on OCI Vault. OpenBao is the secret root for
ADR-0024 on-prem, with External Secrets Operator reconciling Kubernetes secrets.

### Day-0 initialization

1. Deploy OpenBao only after the control plane, storage, and network paths needed
   for its HA Raft state are healthy. Prefer a dedicated namespace such as
   `openbao` and a storage class with replication.
2. Initialize once, capture unseal shares and the initial root token through the
   approved operator ceremony, and immediately store them in the out-of-band
   encrypted escrow. Do not paste them into tickets or chat:

   ```sh
   bao operator init -key-shares=5 -key-threshold=3
   bao operator unseal
   bao status
   ```

3. Split custody. At least three named custodians should hold unseal shares; no
   single laptop or chat log may be the only recovery path.
4. Enable audit logging before storing application secrets. The audit sink must be
   outside the failing node or PVC when possible.
5. Create policies and auth methods for External Secrets Operator, Argo CD, and
   application namespaces. Store app secrets under a path that encodes site and
   environment, for example `kv/maintenance/on-prem/prod/...`.
6. Configure External Secrets Operator to read from OpenBao and project only the
   required keys into Kubernetes namespaces. Rotate away any manually created
   bootstrap secrets once ESO reconciliation is healthy.

### Day-1 operation

- Check `bao status` during every cluster health sweep. A sealed OpenBao means new
  secret projections and rotations can stall even if running pods keep serving.
- Before planned node maintenance, confirm OpenBao Raft has quorum and a recent
  snapshot exists.
- Rotate root and break-glass tokens after initialization. Routine operators
  should use scoped policies, not the initial root token.
- If OpenBao is sealed after a reboot, use the custodian ceremony. Do not lower
  the threshold to make an incident easier unless the incident commander records
  why that is safer than waiting for custodians.

## 4. Replicated storage and data services

### Block storage for Postgres

1. Stage Longhorn from `deploy/apps/storage/` on the on-prem cluster only after the
   node prerequisites in that README are met.
2. Confirm `console-pg-hot` is a replicated default StorageClass in the on-prem
   context and that the OCI guest still uses its own local-path path:

   ```sh
   kubectl -n longhorn-system get pods -o wide
   kubectl get storageclass console-pg-hot -o yaml
   kubectl get storageclass local-path -o yaml
   ```

3. Create a disposable PVC with `storageClassName: console-pg-hot`, bind it to a test
   pod, delete the test workload, and verify Longhorn reports the expected replica
   count before putting CNPG on the class.
4. Sync the on-prem maintenance overlay only after storage is healthy. It expects
   CNPG `instances: 3`, synchronous replication, anti-affinity, and
   `storageClass: console-pg-hot`.
5. Before traffic moves, perform and record a CNPG primary pod/node kill drill.
   Success means a standby promotes automatically and data remains continuous.

### Self-hosted S3/object storage

ADR-0024 replaces managed object storage dependency with self-hosted S3-compatible
storage for the on-prem substrate. The app already consumes S3 via endpoint
configuration, so the operator task is to provide a reliable endpoint and verify
clients against it.

1. Select the accepted SeaweedFS reference in the activation ticket. Do not point
   on-prem production evidence at OCI Object Storage unless this is an explicit
   temporary rollback/bridge. A different engine requires a newer accepted
   decision and the same signed-call, retention, security, and recovery evidence.
2. Configure CNPG Barman and evidence storage endpoints with the on-prem S3 URL,
   credentials from OpenBao/ESO, bucket names, TLS CA material, and retention.
3. Re-test checksum and path-style behavior. ADR-0024 warns not to copy the
   OCI-specific `AWS_*_CHECKSUM_*=when_required` workaround blindly to the
   on-prem endpoint.
4. For evidence/WORM posture, replicate to a second physical site before claiming
   durable data-sovereign retention.

## 5. Network, time, and ingress validation

Run these checks before declaring the substrate ready for application traffic:

```sh
kubectl config current-context
kubectl get nodes -o wide
kubectl -n kube-system get pods -o wide

talosctl --talosconfig ./_talos-onprem/talosconfig health
talosctl --talosconfig ./_talos-onprem/talosconfig \
  --nodes CP1_IP,CP2_IP,CP3_IP etcd status

# Time: compare every node to the chosen site source.
talosctl --talosconfig ./_talos-onprem/talosconfig \
  --nodes CP1_IP,CP2_IP,CP3_IP,WORKER1_IP,WORKER2_IP time

# MTU: use the site's actual value; adjust payload for IP/ICMP headers.
ping -M do -s <payload-size> <peer-node-or-vip>
```

NetworkPolicy validation is a security gate, not just a render check. Production
namespace isolation requires a policy-capable CNI such as the staged Cilium app in
`deploy/apps/cilium/`, or an explicitly selected Calico/Canal equivalent. Plain
Talos/flannel leaves the Maintenance NetworkPolicy manifests in
`deploy/apps/console/base/networkpolicy.yaml` inert. `kubectl kustomize`,
`scripts/render-k8s-manifests.sh`, and `npm run check:production-hardening` are
still required desired-state checks, but they do not prove packet enforcement;
capture CNI readiness plus `npm run smoke:k8s:networkpolicy-deny` output (or an
equivalent recorded deny/allow pod-connectivity transcript) before moving
production traffic. The smoke is safe to repeat: it creates temporary
same-namespace pods/services, checks an allowed control path, proves the
`app=console-app` client can use DNS, outbound HTTPS, and Postgres when the
`console-db-rw` Service exists, expects that same app-tier client's TCP/8080 egress
to be denied by `default-deny-egress-app-tier`, and cleans up unless
`CONSOLE_NETWORKPOLICY_SMOKE_KEEP=1` is set for debugging.

For ingress, follow `deploy/apps/vip-ingress/README.md` and
`deploy/apps/traefik-onprem/README.md`:

1. Render MetalLB and Traefik on-prem manifests.
2. Replace documentation-only VIP placeholders with the reserved site VIP/pool.
3. Verify the Traefik Service is `LoadBalancer`, has no hostPort, has no
   `140.245.68.253` reference, and receives the expected VIP.
4. Run a VIP failover drill from the same L2 network: identify the current VIP
   holder, kill/drain/isolate it according to the site drill, verify the VIP moves
   to another worker, and perform repeated HTTPS health checks through the VIP.
5. Record old holder, new holder, outage duration, command output, DNS TTL, and
   rollback target before changing production DNS or upstream routing.

## 6. Day-1 operations

### Daily or per-shift health sweep

```sh
kubectl get nodes -o wide
kubectl get pods -A -o wide
kubectl get application -n argocd
kubectl -n longhorn-system get pods -o wide
kubectl -n console get cluster,pod,pvc -o wide
kubectl -n metallb-system get pods -o wide
bao status
```

Expected state:

- three control-plane/etcd members are healthy and API traffic reaches the stable
  endpoint;
- ordinary app, ingress, storage, and database workloads run on workers, not
  control-plane nodes;
- Longhorn or the approved storage backend is healthy with expected replicas;
- CNPG has three instances and synchronous replication according to the on-prem
  overlay;
- OpenBao is unsealed and ESO is reconciling secrets;
- Argo CD apps are Healthy/Synced, except intentionally dark apps waiting for
  manual activation.

### Node maintenance or replacement

- Prefer CAPI/Metal3 operations once CAPI owns the cluster. Use
  `clusterctl describe cluster`, Machine/MachineDeployment status, and provider
  events to verify reconciliation.
- For control-plane membership repair, use `talos/on-prem-ha-runbook.md` and keep
  quorum math explicit: three voters tolerate one loss; two losses are a disaster
  recovery event.
- Before draining a worker, confirm app, Traefik, Longhorn, and CNPG replicas can
  remain available on other workers. Do not drain multiple storage workers at the
  same time unless the storage owner signs off.
- After replacement, rerun Talos health, etcd status, node labels, Longhorn
  replica health, CNPG health, and ingress VIP checks.

### Upgrades

1. Upgrade Talos control-plane nodes one at a time, then workers, keeping quorum
   and workload budgets healthy between nodes.
2. Upgrade Kubernetes through the Talos-supported path and verify API readiness,
   node readiness, and CNI health before continuing.
3. Upgrade OpenBao, Cilium, MetalLB, Longhorn, CNPG, and Traefik through Argo CD
   with rendered diff review and rollback notes. Do not upgrade more than one
   substrate layer during a failover drill.
4. After every upgrade, run the validation checklist in section 7.

### Backup, restore, and rollback

- Keep recent etcd snapshots outside the cluster.
- Keep OpenBao Raft snapshots and unseal custody current.
- Keep CNPG WAL/base backups pointed at the approved on-prem S3 endpoint and run
  restore drills before production data enters the cluster.
- Keep evidence/object-storage replication to the second site healthy before
  claiming WORM/retention readiness.
- For failed on-prem activation, move DNS/upstream routing back to the previous
  target first. If the previous target is OCI guest, follow `OPS-RUNBOOK.md` and
  the preserved OCI Traefik/reserved-IP path.

## 7. Validation checklist

Run local render/documentation checks from the repository root before handing this
runbook to another operator:

```sh
kubectl kustomize deploy/apps/storage
kubectl kustomize deploy/apps/storage/manifests
kubectl kustomize deploy/apps/vip-ingress
kubectl kustomize deploy/apps/vip-ingress/manifests
kubectl kustomize deploy/apps/traefik-onprem
kubectl kustomize deploy/apps/console/overlays/on-prem
CONSOLE_NETWORKPOLICY_PREFLIGHT=require \
  CONSOLE_NETWORKPOLICY_EXPECTED_ENFORCER=cilium \
  npm run check:k8s:networkpolicy
CONSOLE_NETWORKPOLICY_EXPECTED_ENFORCER=cilium \
  CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES=auto \
  npm run smoke:k8s:networkpolicy-deny
npm run check:production-hardening
```

For a real on-prem activation, attach evidence for:

1. Talos media checksums, generated machine-config patch names/checksums, and proof
   no secret-bearing `_talos-onprem/` output was committed.
2. Three healthy etcd members before, during, and after one control-plane node
   loss.
3. CAPI/Metal3 ownership status or an explicit explanation that the run is a
   manual rehearsal pending the CAPI lane.
4. Site NTP/time output and MTU probe results for node/storage/ingress paths.
5. `scripts/check-networkpolicy-enforcement.sh` required-mode output showing the
   target context, applied `maintenance` NetworkPolicies, and Cilium (or an
   explicitly approved policy-capable equivalent) as the detected enforcer; add
   `scripts/smoke-networkpolicy-deny.sh` output showing the control, DNS,
   HTTPS, and Postgres-if-present paths passed and the `app=console-app` TCP/8080
   path was denied before claiming isolation.
6. OpenBao initialized, unsealed, audited, backed up, and ESO projections healthy.
7. Longhorn or approved storage backend healthy with replicated `console-pg-hot` PVCs.
8. CNPG `instances: 3`, primary failover drill, and restore-drill result.
9. MetalLB/Traefik VIP failover result and DNS/rollback evidence.
10. OCI guest path still documented and not accidentally modified or deleted.

## 8. Failure and escalation notes

- Wrong context: if `kubectl config current-context` or Argo CD points at the OCI
  guest while running on-prem steps, stop immediately. Do not rely on namespace or
  app names to catch this for you.
- Talos API unreachable: verify management network, control-plane VIP, node BMC
  power state, and whether the endpoint should be a node IP during early bring-up.
  Use the stable VIP only after the API endpoint is actually serving there.
- Time drift: stop certificate, etcd, and OpenBao work until NTP is healthy.
  Certificate and Raft symptoms caused by clock skew are not application bugs.
- MTU blackhole: test node-to-node, operator-to-API, storage, and ingress paths.
  Apply a site-specific MTU fix in Talos/network configuration rather than copying
  the OCI VNIC workaround blindly.
- OpenBao sealed or quorum lost: pause rotations and new app rollouts. Convene
  unseal custodians or restore according to the approved OpenBao recovery plan;
  do not recreate the secret root ad hoc.
- Longhorn degraded or rebuilding: do not perform CNPG failover drills, node
  drains, or storage-heavy migrations until replica health is restored.
- CNPG failover fails: keep traffic on the previous target, preserve logs and WAL
  state, and open an incident. Do not promote the on-prem cluster.
- VIP does not move: leave DNS/upstream routing on the previous target, capture
  MetalLB speaker logs, worker labels/interfaces, ARP/NDP evidence, and Traefik
  endpoint state.
- CAPI reconciliation stuck: do not bypass by repeatedly hand-applying Talos
  config unless incident command declares CAPI abandoned for this run. Record the
  owning Machine/BareMetalHost conditions and provider logs.
- Two control-plane nodes lost: this is quorum-loss disaster recovery, not normal
  HA. Preserve evidence, identify the freshest etcd data/snapshot, and follow a
  version-specific Talos/etcd recovery plan approved for the site.
