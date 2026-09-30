# Talos deployment contexts

> **POST-PIVOT UNVERIFIED / HOLD:** This is retained bootstrap and substrate
> reference, not evidence of current nodes or authority to bootstrap, apply, or
> reset Talos. The repository currently authorizes zero production mutations.
> Start with the
> [disk-wipe consolidation handoff](../../docs/handoffs/2026-08-03-disk-wipe-consolidation.md).

This directory keeps Talos substrate inputs split by deployment context:

| Context | Purpose | Activation state |
|---|---|---|
| `oci-guest/` | Existing OCI Always Free single-node cluster. Its control-plane patch intentionally enables control-plane scheduling. | Manual/operator path already used by the OCI guest. |
| `on-prem/` | DARK ADR-0024 HA substrate: three control-plane nodes with etcd quorum plus N dedicated workers. | Staged only; merge does not apply, sync, bootstrap, or cut over anything. |

The root `controlplane.patch.yaml` is a deprecated empty guard file. Use the
context paths below so the OCI single-node scheduling override cannot leak into
on-prem HA machineconfigs or CAPI templates.

## OCI guest — Oracle Cloud Ampere A1 single node

Bring-your-own-image bootstrap of a one-node Talos cluster in **ap-chuncheon-1**,
sized for Always Free (`VM.Standard.A1.Flex`, 4 OCPU / 24 GB).

### 0. Account note (free-tier image import)

Importing a custom compute image requires a **Pay-As-You-Go** account. Upgrading
from a pure free account to PAYG stays **$0** as long as you only launch
Always-Free-eligible shapes. If you must stay on a non-upgraded account, use the
boot-volume workaround: launch a throwaway image, attach a second block volume,
`dd` the Talos raw image onto it, then boot the A1 from that volume.

The boot-volume workaround is the OCI **dd-flasher** path. It is only for the
OCI guest context, and it is **history for the live cluster** — the node at
`140.245.68.253` was flashed this way on 2026-06-22 and is irreplaceable, so
none of these steps may be re-run against it. What follows is the procedure for
standing a *new* OCI Talos node up from scratch, in a tenancy with allotment to
spare. The flasher helper is no longer an OpenTofu resource: it was a one-shot
bootstrap host, it consumed a second A1, and leaving it in the module meant any
future apply would try to create one.

1. Apply OpenTofu for the network/storage/bastion, then launch the Oracle Linux
   flasher helper **by hand** — a throwaway instance on any shape with spare
   allotment. Do not add it back to the compute module.
2. Attach the target boot volume to the flasher helper and identify it by OCI
   volume/device metadata. Never guess a Linux device name and never write to the
   flasher helper's own boot disk.
3. From the flasher helper, write the approved Talos raw disk artifact to that
   attached volume, for example:

   ```sh
   xz -dc talos-oracle-arm64.raw.xz | \
     sudo dd of=/dev/disk/by-id/<attached-oci-boot-volume> bs=16M conv=fsync status=progress
   sync
   ```

4. Detach the flashed volume, create/import the resulting bootable image or boot
   volume according to the activation ticket, set `talos_image_ocid`, and re-run
   OpenTofu so the single OCI Talos node is managed declaratively.

On-prem bare metal does not use this flasher path; Metal3/Ironic writes the
approved Talos metal image directly to selected `BareMetalHost` inventory.

### 1. Get the Talos arm64 image (Image Factory)

Pick a schematic at <https://factory.talos.dev> (vanilla is fine; add the
`siderolabs/qemu-guest-agent` extension if you want OCI agent integration), then
download the **oracle-arm64** disk for v1.13.4:

```sh
curl -LO https://factory.talos.dev/image/<schematic-id>/v1.13.4/oracle-arm64.qcow2
tar zcf talos-oracle-arm64.oci oracle-arm64.qcow2 image_metadata.json   # metadata in this dir
```

### 2. Import as an OCI custom image

```sh
# Upload talos-oracle-arm64.oci to an OCI bucket, then:
oci compute image import from-object \
  --compartment-id "$COMPARTMENT" \
  --namespace "$OS_NAMESPACE" --bucket-name "$BUCKET" \
  --name talos-oracle-arm64.oci \
  --display-name talos-1.13.4-arm64 \
  --launch-mode PARAVIRTUALIZED \
  --source-image-type QCOW2
```

### 3. Networking (VCN security list — ingress rules)

| Port | Source | Purpose |
|---|---|---|
| 50000/tcp | your admin IP | Talos API (`talosctl`) |
| 6443/tcp | your admin IP | Kubernetes API |
| 80,443/tcp | 0.0.0.0/0 | Traefik ingress (HTTP-01 + app traffic) |

### 4. Launch the instance

Create a `VM.Standard.A1.Flex` (4 OCPU / 24 GB) from the imported image with a
public IP. Note its public IP as `$NODE_IP`.

### 5. Generate config (with the OCI single-node patch) and apply

```sh
talosctl gen config maintenance "https://$NODE_IP:6443" \
  --config-patch-control-plane @deploy/talos/oci-guest/controlplane.patch.yaml \
  --additional-sans "$NODE_IP" --output-dir ./_talos

# Confirm the install disk first (OCI boot volume is usually /dev/sda):
talosctl -n "$NODE_IP" disks --insecure

talosctl apply-config --insecure -n "$NODE_IP" --file ./_talos/controlplane.yaml
```

### 6. Bootstrap + kubeconfig

```sh
export TALOSCONFIG=./_talos/talosconfig
talosctl config endpoint "$NODE_IP"
talosctl config node "$NODE_IP"
talosctl bootstrap                      # run ONCE
talosctl health                         # wait for the node to converge
talosctl kubeconfig ./_talos/kubeconfig
export KUBECONFIG=./_talos/kubeconfig
kubectl get nodes                       # Ready (control-plane, schedulable)
```

Keep `_talos/` (it holds the cluster PKI + secrets) out of git and backed up
securely. Continue with Argo CD bootstrap in [`../README.md`](../README.md).

Notes:

- The OCI patch enables control-plane scheduling (single node runs workloads),
  OCI NTP (`169.254.169.254`), and host DNS. Validate generated OCI configs with
  `talosctl validate --mode cloud --config <controlplane.yaml>`.
- Upgrades are API-driven and atomic (A/B): `talosctl upgrade --image
  factory.talos.dev/...:v<next>`; Kubernetes upgrades via `talosctl upgrade-k8s`.
- `image_metadata.json` in this directory carries the A1 launch options
  (UEFI_64, paravirtualized) the import needs.

## On-prem HA — DARK three-control-plane templates

The on-prem context is documented in [`on-prem/README.md`](on-prem/README.md).
It supports two reproducible render paths from the same node inventory. The
operator-custody path renders three distinct control-plane machineconfigs and
one worker machineconfig per worker inventory entry:

```sh
python3 deploy/talos/on-prem/render-machineconfigs.py \
  --inventory deploy/talos/on-prem/nodes.example.json \
  --output-dir /tmp/console-talos-onprem \
  --validate
```

The renderer is local-only and writes operator custody artifacts (`secrets.yaml`,
`talosconfig`, and machineconfigs) to an ignored or scratch output directory. It
does not apply configs, bootstrap nodes, sync Argo CD, or mutate OpenTofu state.
On-prem control-plane machineconfigs deliberately omit the single-node
control-plane scheduling override; application workloads must run on the
dedicated worker machineconfigs.

The CAPI/Metal3 path renders a management-cluster template instead of secret
custody machine YAMLs:

```sh
python3 deploy/talos/on-prem/render-capi-metal3.py \
  --inventory deploy/talos/on-prem/nodes.example.json \
  --output deploy/talos/on-prem/capi-metal3.example.yaml \
  --check
```

That template wires `Cluster -> TalosControlPlane -> Metal3MachineTemplate` for
the three control-plane nodes and `MachineDeployment -> TalosConfigTemplate ->
Metal3MachineTemplate` for workers. CABPT/CACPPT generate Talos bootstrap data
from the same strategic patches, and CAPM3/Metal3 provisions the inventory-backed
`BareMetalHost` objects after BMC credentials and real image URLs/checksums are
provided out of band.

On-prem also does not inherit the OCI public-path MTU workaround from
`oci-guest/controlplane.patch.yaml`; record real fabric MTU in inventory or site
patches only after hardware evidence requires it.
