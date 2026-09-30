# DARK on-prem HA Talos machineconfigs and CAPI/Metal3 templates

This directory stages ADR-0024 roadmap item #6 Talos machineconfig inputs and the
matching Cluster API/Metal3 provisioning template for the `on-prem` deployment
context. It is intentionally inert: nothing here is watched by Argo CD,
OpenTofu, or an apply script, and the renderers never call `talosctl
apply-config`, `talosctl bootstrap`, `kubectl apply`, or `clusterctl`.

## Topology encoded by the renderer

- exactly three control-plane nodes, giving etcd quorum;
- one or more dedicated worker nodes for application workloads;
- a control-plane VIP on the control-plane interface;
- Cilium-owned networking (`cluster.network.cni.name=none`) and kube-proxy
  disabled, matching `deploy/apps/cilium/values.yaml`;
- no single-node control-plane scheduling override in on-prem patches,
  generated machineconfigs, or CAPI templates;
- Cluster API roles represented as one `TalosControlPlane` backed by a
  control-plane `Metal3MachineTemplate`, plus one worker `MachineDeployment`
  backed by a worker `TalosConfigTemplate` and worker `Metal3MachineTemplate`.

## Files

| File | Purpose |
|---|---|
| `cluster.patch.yaml` | Cluster-wide on-prem patch: no default CNI, kube-proxy disabled. |
| `controlplane.patch.yaml` | Control-plane role labels only; deliberately does not enable scheduling on control planes. |
| `worker.patch.yaml` | Dedicated worker role labels. |
| `nodes.example.json` | Example node inventory with 3 CP + 2 workers, CAPI versions/image metadata, BMH names, BMC Secret names, boot MACs, install disks, and failure-domain labels. Copy/edit outside git or in a task-owned scratch path for real hardware. |
| `render-machineconfigs.py` | Local renderer around `talosctl gen config`; writes ignored custody artifacts under `deploy/talos/_out/on-prem` by default. |
| `render-capi-metal3.py` | Deterministic renderer for the CAPI/CABPT/CACPPT + CAPM3 manifest from the same inventory and patches. It does not contact a cluster. |
| `capi-metal3.example.yaml` | Rendered, reviewable example manifest from `nodes.example.json`; replace placeholders before applying to any management cluster. |

## Render and validate with placeholder inventory

```sh
python3 deploy/talos/on-prem/render-machineconfigs.py \
  --inventory deploy/talos/on-prem/nodes.example.json \
  --output-dir /tmp/console-talos-onprem \
  --validate
```

Expected output includes three control-plane machineconfigs and one file per
worker, for example:

```text
controlplane-console-cp-1.yaml
controlplane-console-cp-2.yaml
controlplane-console-cp-3.yaml
worker-console-worker-1.yaml
worker-console-worker-2.yaml
talosconfig
secrets.yaml
render-manifest.json
```

`secrets.yaml`, `talosconfig`, and the rendered machineconfigs are operator
custody artifacts. Do not commit them; keep them in an encrypted/off-host secret
custody path for any real cluster.

## Render the CAPI/Metal3 template from the same inventory

```sh
python3 deploy/talos/on-prem/render-capi-metal3.py \
  --inventory deploy/talos/on-prem/nodes.example.json \
  --output deploy/talos/on-prem/capi-metal3.example.yaml

# CI/review freshness check:
python3 deploy/talos/on-prem/render-capi-metal3.py \
  --inventory deploy/talos/on-prem/nodes.example.json \
  --output deploy/talos/on-prem/capi-metal3.example.yaml \
  --check
```

The rendered manifest is intentionally a template, not an activation command. It
assumes an on-prem management cluster has Cluster API, Metal3/CAPM3, the Sidero
Talos bootstrap provider (CABPT), and the Sidero Talos control-plane provider
(CACPPT) installed, for example with a site-approved equivalent of:

```sh
clusterctl init --bootstrap talos --control-plane talos --infrastructure metal3
```

`clusterctl` is not required for rendering this repo artifact, and the renderer
does not verify a live management cluster.

## How CAPI/Metal3 consumes the generated Talos data

The CAPI path keeps the same Talos patch boundaries as the local renderer:

1. `cluster.patch.yaml` disables the default CNI and kube-proxy for the on-prem
   Cilium/eBPF lane.
2. `render-capi-metal3.py` adds an inventory-generated strategic patch for the
   stable control-plane endpoint and API server SANs.
3. `controlplane.patch.yaml` or `worker.patch.yaml` adds role labels without
   enabling control-plane workload scheduling.

`capi-metal3.example.yaml` wires those patches into provider resources:

- `Cluster.spec.controlPlaneRef` points at `TalosControlPlane`.
- `TalosControlPlane.spec.controlPlaneConfig.controlplane` uses
  `generateType: controlplane`, the pinned Talos/Kubernetes versions, and the
  strategic patches above. CACPPT/CABPT generate the control-plane Talos
  bootstrap data and CAPI passes it to Metal3 as machine user data.
- The worker `MachineDeployment` points at `TalosConfigTemplate` with
  `generateType: worker` and the worker patches. CABPT generates worker Talos
  bootstrap data for each worker Machine.
- Control-plane and worker `Metal3MachineTemplate` objects select only
  `BareMetalHost` inventory with the matching
  `console.nousresearch.com/onprem-role` label, attach the Talos metal image
  URL/checksum from inventory, and reference role-specific `Metal3DataTemplate`
  network metadata.
- `BareMetalHost` objects are rendered from inventory with boot MAC, install disk
  hint, role/failure-domain labels, and BMC credential Secret names. The Secret
  values themselves must be created out of band and never committed.

## NetworkPolicy enforcement requirement

The Talos patch in `cluster.patch.yaml` deliberately sets
`cluster.network.cni.name=none` so a policy-capable CNI can own the on-prem data
plane. Production cannot claim namespace or egress isolation while running plain
Talos/flannel: flannel does not enforce Kubernetes NetworkPolicy, so rendered
resources such as `deploy/apps/console/base/networkpolicy.yaml` are inert
until Cilium, Calico, or Canal with Calico policy is actually running.

For the ADR-0024 on-prem path, `deploy/apps/cilium/` is the staged CNI contract.
If an activation ticket selects Calico or Canal instead, update the Talos/CNI docs
and promotion evidence before cutover. A clean machineconfig render,
`kubectl kustomize`, or `npm run check:production-hardening` proves desired-state
shape only; the production security gate also needs CNI readiness and deny/allow
connectivity evidence against the Maintenance NetworkPolicy set.

Use the local `render-machineconfigs.py` path when an operator wants explicit
secret-custody Talos YAMLs for manual inspection or an approved scratch cluster.
Use the CAPI/Metal3 path when the management cluster should own provisioning and
machine lifecycle. Both paths are reproducible from the same inventory and both
preserve the rule that on-prem control planes do not run ordinary workloads.

## Promotion guardrails

1. Replace `nodes.example.json` with real operator-approved inventory: control
   plane VIP, node names, interfaces, install disks, static addressing or DHCP
   assumptions, NTP, DNS, BMC addresses, BMC credential Secret names, boot MACs,
   Talos metal image URL/checksum, and CAPI provider versions.
2. Re-render local Talos configs with `--validate`; keep `render-manifest.json`
   as local evidence. The local renderer strips Talos' generated
   `HostnameConfig(auto=stable)` document only when a per-node static hostname
   patch is present, so `talosctl validate --mode metal` stays authoritative.
3. Re-render `capi-metal3.example.yaml` with `--check` clean before any review of
   the CAPI/Metal3 path.
4. Coordinate with the on-prem substrate lane (#375) before using the configs.
5. Apply only by explicit operator action to real nodes. A merge of this repo
   must remain a no-op for on-prem Talos.
