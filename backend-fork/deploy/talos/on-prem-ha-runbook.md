# DARK on-prem Talos HA control-plane runbook

This runbook stages the ADR-0024 / issue #376 operator plan for a future
bare-metal `on-prem` Talos cluster. It is a documentation artifact only: the
current live `oci-guest` cluster remains the single-node OCI Ampere A1 cluster,
and no merge of this file should apply machine configs, change Argo CD sync
paths, or activate real hardware.

The demonstrable target for this lane is narrow and explicit: etcd must survive
loss of one control-plane node while a three-member control plane keeps
Kubernetes API/etcd quorum available. The real drill is DARK until on-prem
hardware or an approved scratch multi-node Talos cluster exists.

## Intended topology

| Role | Count | Scheduling expectation | Notes |
|---|---:|---|---|
| Control plane / etcd | 3 | Unschedulable for ordinary workloads | Provides odd-member etcd quorum and Kubernetes control-plane redundancy. Do not set `cluster.allowSchedulingOnControlPlanes: true` for `on-prem`. |
| Workers | N, minimum 2 for ingress app HA | Schedulable | Run `console-app`, `console-worker`, Traefik, storage, CNPG, and other workload pods subject to the relevant anti-affinity lanes. |

Quorum math for the first on-prem target:

- Three voting etcd members require two available members for quorum.
- Losing one control-plane node leaves two voters, so etcd should remain healthy
  and the API server should continue serving through the control-plane VIP or
  load-balanced endpoint.
- Losing two control-plane nodes leaves one voter, so quorum is lost. That is
  outside the issue #376 acceptance target and becomes a recovery exercise, not
  normal failover.
- Do not add an even-numbered fourth voting control-plane member as a steady
  state. If more capacity is needed, add workers first; keep etcd voters at an
  odd count unless a later design deliberately changes the topology.

## DARK staging and safety rules

- The `on-prem` HA path must be additive beside the `oci-guest` path. Preserve
  the OCI single-node scheduling patch for the Always Free cluster.
- `cluster.allowSchedulingOnControlPlanes: true` belongs only to the
  single-node `oci-guest` machine config patch. It must not appear in the
  future `on-prem` control-plane template.
- No Talos machine secrets, generated `_talos/` output, kubeconfig, disks,
  certs, or node-specific private inventory may be committed.
- The Kubernetes endpoint for HA should be a stable control-plane VIP or load
  balancer address, not one node's IP. The VIP ownership belongs to the
  MetalLB/kube-vip/substrate lanes and must be recorded before activation.
- This runbook is not a live-apply instruction until issue #375/on-prem
  substrate, real node inventory, founder/operator activation, and a successful
  render/validation lane exist.

## Preflight checklist for the future real-node drill

Record these facts in the activation ticket before touching hardware:

1. Three named control-plane nodes with stable management addresses, stable
   install disks, time sync source, MTU/fabric assumptions, and the Talos API
   reachable from the operator network.
2. At least two dedicated worker nodes, plus any storage-node requirements from
   the Longhorn/CNPG lane.
3. A control-plane endpoint such as `https://ON_PREM_CONTROL_PLANE_VIP:6443`
   with DNS/SANs captured before `talosctl gen config` runs.
4. The exact Talos version, Kubernetes version, and generated machine config
   patch files used for both control-plane and worker roles.
5. A secure out-of-band copy of the Talos secrets/talosconfig and at least one
   fresh etcd snapshot location outside the cluster.
6. Confirmation that workload anti-affinity and storage HA expectations are
   coordinated with the CNPG/storage lane before application cutover.

## Generate and apply configs for the HA shape

Example command shape only; replace every placeholder with the approved
activation ticket values. Keep generated output out of git. The canonical repo
inputs live under `deploy/talos/on-prem/`: `nodes.example.json` captures the
inventory shape, `cluster.patch.yaml` captures the shared CNI/kube-proxy patch,
and `controlplane.patch.yaml` / `worker.patch.yaml` capture role-specific Talos
patches.

```sh
mkdir -p ./_talos-onprem
talosctl gen secrets --output-file ./_talos-onprem/secrets.yaml

export TALOSCONFIG=./_talos-onprem/talosconfig
export CONTROL_PLANE_ENDPOINT="https://ON_PREM_CONTROL_PLANE_VIP:6443"

talosctl gen config maintenance "$CONTROL_PLANE_ENDPOINT" \
  --with-secrets ./_talos-onprem/secrets.yaml \
  --additional-sans ON_PREM_CONTROL_PLANE_VIP \
  --config-patch @deploy/talos/on-prem/cluster.patch.yaml \
  --config-patch-control-plane @deploy/talos/on-prem/controlplane.patch.yaml \
  --config-patch-worker @deploy/talos/on-prem/worker.patch.yaml \
  --output-dir ./_talos-onprem
```

For the CAPI/Metal3-managed path, render
`deploy/talos/on-prem/capi-metal3.example.yaml` from the same inventory with
`deploy/talos/on-prem/render-capi-metal3.py`. That manifest represents the three
control-plane nodes as a `TalosControlPlane` and the workers as a
`MachineDeployment` using `TalosConfigTemplate`; Metal3MachineTemplates select the
inventory-backed `BareMetalHost` objects by role label. The management cluster,
not this repo, then owns provisioning and machine lifecycle after operator
approval.

Apply control-plane config to the three approved nodes, then workers. Bootstrap
etcd once, from exactly one initial control-plane node, after the first
control-plane node has accepted config and is reachable.

```sh
for node in CP1_IP CP2_IP CP3_IP; do
  talosctl -n "$node" disks --insecure
  talosctl apply-config --insecure -n "$node" \
    --file ./_talos-onprem/controlplane.yaml
done

talosctl --talosconfig ./_talos-onprem/talosconfig \
  --nodes CP1_IP --endpoints CP1_IP bootstrap

for node in WORKER1_IP WORKER2_IP; do
  talosctl -n "$node" disks --insecure
  talosctl apply-config --insecure -n "$node" \
    --file ./_talos-onprem/worker.yaml
done
```

After bootstrap, configure the talosconfig endpoints to include the stable VIP
and, when useful during recovery, all control-plane node addresses.

```sh
talosctl --talosconfig ./_talos-onprem/talosconfig config endpoint \
  ON_PREM_CONTROL_PLANE_VIP CP1_IP CP2_IP CP3_IP
talosctl --talosconfig ./_talos-onprem/talosconfig config node CP1_IP
```

## Steady-state health checks

Use these checks before and after every membership operation:

```sh
talosctl --talosconfig ./_talos-onprem/talosconfig health

talosctl --talosconfig ./_talos-onprem/talosconfig \
  --nodes CP1_IP,CP2_IP,CP3_IP etcd members

talosctl --talosconfig ./_talos-onprem/talosconfig \
  --nodes CP1_IP,CP2_IP,CP3_IP etcd status

kubectl --kubeconfig ./_talos-onprem/kubeconfig get nodes -o wide
kubectl --kubeconfig ./_talos-onprem/kubeconfig get pods -A -o wide
```

Expected steady state:

- three etcd members are listed and healthy;
- all three control-plane nodes are `Ready` and normally tainted/unschedulable
  for ordinary app workloads;
- workers are `Ready` and carry ordinary workloads;
- no on-prem machine config relies on `allowSchedulingOnControlPlanes: true`;
- the control-plane endpoint remains reachable through the VIP/load balancer.

## Member add procedure

Use this when intentionally adding or re-adding a control-plane node to a
healthy cluster.

1. Confirm the cluster currently has quorum with `talosctl etcd status` from all
   reachable control-plane nodes.
2. If the target node reuses hardware from an old member, remove the old etcd
   member first or wipe/reset the node so it cannot start with stale etcd data.
3. Generate or select the approved `controlplane.yaml` for the same cluster
   secrets and endpoint. Do not generate a new independent cluster.
4. Apply the control-plane config to the new node with `talosctl apply-config`.
   Talos should join it to the existing etcd cluster as the node converges.
5. Watch `talosctl health`, `talosctl etcd members`, and
   `talosctl etcd status` until the new member is healthy.
6. If adding a temporary replacement would create an even voter count, finish the
   old-member removal promptly so the steady state returns to three voters.

## Member remove procedure

Prefer graceful removal while the node is reachable and quorum is healthy.

```sh
talosctl --talosconfig ./_talos-onprem/talosconfig \
  --nodes CP_TO_REMOVE_IP etcd leave
```

Then verify from the remaining control-plane nodes:

```sh
talosctl --talosconfig ./_talos-onprem/talosconfig \
  --nodes CP1_IP,CP2_IP etcd members

talosctl --talosconfig ./_talos-onprem/talosconfig \
  --nodes CP1_IP,CP2_IP etcd status
```

If the node is dead, unreachable, or cannot call `etcd leave`, first capture the
failed member ID from `talosctl etcd members`, then remove that broken member
from a healthy remaining control-plane node:

```sh
talosctl --talosconfig ./_talos-onprem/talosconfig \
  --nodes HEALTHY_CP_IP etcd remove-member FAILED_MEMBER_ID
```

`remove-member` is the break-glass path; Talos' CLI help says to prefer
`etcd leave` whenever the node can still participate.

## Control-plane replacement procedure

Use this for a failed CP node when the other two CP nodes still have quorum.

1. Freeze unrelated changes. Do not perform Kubernetes upgrades, storage
   failovers, or application cutovers during membership repair.
2. Confirm the two surviving members are healthy:
   `talosctl --nodes SURVIVOR1_IP,SURVIVOR2_IP etcd status`.
3. Take an etcd snapshot from a survivor and store it outside the cluster:
   `talosctl --nodes SURVIVOR1_IP etcd snapshot ./evidence/etcd-pre-replace.db`.
4. Remove the failed member gracefully with `etcd leave` if it is reachable;
   otherwise use `etcd remove-member FAILED_MEMBER_ID` from a survivor.
5. Provision or repair the replacement host. Confirm the install disk, wipe/reset
   stale disks when reusing hardware, and apply the approved `controlplane.yaml`.
6. Wait for the replacement to appear in `etcd members` and become healthy in
   `etcd status`.
7. Re-run the steady-state health checks and record node names, member IDs,
   snapshot path, command output, and any deviations in the drill evidence.

## Recovery procedure when quorum is lost

Two failed control-plane nodes in a three-member cluster is a quorum-loss event.
This is not the normal HA target; it requires operator recovery from backup or a
Talos/etcd disaster-recovery plan approved for the exact site.

Minimum safe response:

1. Stop all automation and preserve evidence. Do not repeatedly reboot or reset
   nodes before identifying which etcd data directory is the best survivor.
2. Identify the freshest surviving etcd member or the latest verified snapshot.
3. Keep Talos secrets and cluster identity consistent. Do not generate a new
   cluster unless the incident commander explicitly declares a rebuild.
4. Restore/recover etcd from the selected snapshot or surviving member according
   to the Talos version's disaster-recovery procedure.
5. Re-add replacement control-plane nodes one at a time until three healthy
   voters exist again.
6. Only after control-plane quorum is healthy should workers, storage, CNPG, and
   ingress failback be evaluated.

A future real-node drill should exercise the one-node-loss path, not destructive
quorum-loss recovery, unless a separate approved disaster-recovery rehearsal is
scheduled.

## Planned one-control-plane-loss drill

Run this only on real on-prem hardware or an approved scratch multi-node Talos
cluster. Capture command output and timestamps in the activation evidence.

1. Prove steady state with the health checks above: three healthy etcd members,
   API reachable through the VIP, workers schedulable, app/storage pods placed on
   workers according to their own anti-affinity rules.
2. Select a non-leader control-plane node when possible. If the selected node is
   leader, record it and expect leadership transfer/election noise.
3. Simulate loss by powering off, disconnecting, or otherwise isolating exactly
   one control-plane node. Do not remove its etcd member for the ordinary
   one-node-loss drill.
4. Confirm the remaining two etcd members retain quorum:
   `talosctl --nodes SURVIVOR1_IP,SURVIVOR2_IP etcd status`.
5. Confirm the API server still serves through the stable endpoint:
   `kubectl --kubeconfig ./_talos-onprem/kubeconfig get --raw='/readyz?verbose'`
   and `kubectl get nodes -o wide`.
6. Confirm ordinary workloads stay on workers and no on-prem workload scheduling
   depends on the control-plane nodes being schedulable.
7. Restore the lost node, then watch it rejoin and return to healthy etcd status.
8. Record the PASS/FAIL verdict. PASS means etcd and the Kubernetes API remained
   available with one control-plane node down; any need to restore from backup is
   a FAIL for issue #376's HA target.

## Evidence to attach when the drill becomes real

- Talos and Kubernetes versions.
- Sanitized machine-config patch names and checksums, not secret-bearing config.
- `etcd members` and `etcd status` before loss, during one-node loss, and after
  restoration.
- Kubernetes `/readyz` output and `kubectl get nodes -o wide` before/during/after.
- Confirmation that ordinary workloads schedule on workers, not control planes.
- Incident timeline, recovery actions, and explicit PASS/FAIL against the target:
  "etcd survives loss of one control-plane node."
