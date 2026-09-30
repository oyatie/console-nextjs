# OpenTofu — OCI guest infrastructure for the FSM cluster

> **POST-PIVOT UNVERIFIED / HOLD:** This is retained infrastructure reference,
> not evidence of a live OCI topology or authority to plan/apply against an
> account. The repository currently authorizes zero production mutations. Start
> with the
> [disk-wipe consolidation handoff](../../docs/handoffs/2026-08-03-disk-wipe-consolidation.md).

Declarative OCI infra (ap-chuncheon-1): VCN + hardened security list + public
subnet, object-storage buckets (DB backups, evidence, and evidence replica), a
managed OCI Bastion, the Talos node, and an Oracle Linux helper used to flash
Talos.

The live OCI target remains first-class under the ADR-0339-style `oci-guest`
deployment context. The root `deploy/opentofu/*.tf` files are intentionally thin
compatibility wrappers; the reusable primitives live under
`contexts/oci-guest/primitives/`.

Authenticates from `~/.oci/config` (API-key auth).

## Layout

```text
deploy/opentofu/
  providers.tf, variables.tf, outputs.tf    # root OCI stack contract
  network.tf, storage.tf, compute.tf, bastion.tf
                                           # thin module-block wrappers
  moved.tf                                 # old root address -> context address shim
  contexts/oci-guest/primitives/
    network/                               # VCN, IGW, route table, security list, subnet
    storage/                               # Object Storage namespace + buckets
    compute/                               # the single Talos node (prevent_destroy)
    bastion/                               # managed OCI Bastion service
```

## Security hardening baked in

- **Control-plane APIs are not public.** Talos (`50000`) and Kubernetes (`6443`)
  ingress is restricted to `var.admin_cidr` + the VCN CIDR — never `0.0.0.0/0`.
  Only the app's `80/443` is world-open.
- **Managed OCI Bastion** for private-IP cluster access (port-forwarding
  sessions) instead of a public-IP jump host with open SSH.
- **No public SSH to the node** (Talos has no SSH). SSH `22` is admin-CIDR-only,
  for the helper.
- **ICMP type 3** allowed so PMTU discovery doesn't black-hole (the mTLS path).
- Buckets are `NoPublicAccess`; the backups and evidence-replica buckets have
  versioning on.

## Usage

```sh
cd deploy/opentofu
cp terraform.tfvars.example terraform.tfvars   # fill in compartment, admin_cidr, talos image
tofu init
tofu plan
tofu apply
```

`talos_image_ocid` is **required** — it has no default. Import the Talos image
and flash the node per [`../talos/README.md`](../talos/README.md) before the
first apply. An earlier version defaulted it to `""`, which made the node's
`count` evaluate to 0 whenever the tfvars file was missing, so a plan would
quietly omit the cluster's only instance instead of failing.

The node also carries `prevent_destroy = true`. The A1 is grandfathered at
4 OCPU / 24 GB and OCI has since cut the Always Free allotment, so recreating it
yields 2 vCPU / 12 GB. Talos and Kubernetes upgrade **in place** (`talosctl
upgrade`, `talosctl upgrade-k8s`); this module never replaces the node.

## Address-preserving state migration

This refactor moved the OCI resources into `module.network`, `module.storage`,
`module.compute`, and `module.bastion` while preserving the same resource types,
names, arguments, tags, bootstrap assumptions, and Talos lifecycle settings. The
root `moved.tf` file is the compatibility shim for existing state. A plan against
an already-imported state should show only `has moved to` address notices for the
resources below, not delete/recreate actions:

| Old address | New address |
| --- | --- |
| `oci_core_vcn.mnt` | `module.network.oci_core_vcn.mnt` |
| `oci_core_internet_gateway.mnt` | `module.network.oci_core_internet_gateway.mnt` |
| `oci_core_route_table.mnt` | `module.network.oci_core_route_table.mnt` |
| `oci_core_security_list.mnt` | `module.network.oci_core_security_list.mnt` |
| `oci_core_subnet.mnt_public` | `module.network.oci_core_subnet.mnt_public` |
| `oci_objectstorage_bucket.db_backups` | `module.storage.oci_objectstorage_bucket.db_backups` |
| `oci_objectstorage_bucket.evidence` | `module.storage.oci_objectstorage_bucket.evidence` |
| `oci_objectstorage_bucket.evidence_replica` | `module.storage.oci_objectstorage_bucket.evidence_replica` |
| `oci_core_instance.node[0]` | `module.compute.oci_core_instance.node[0]` |
| `oci_bastion_bastion.mnt` | `module.bastion.oci_bastion_bastion.mnt` |

If an older OpenTofu/Terraform binary cannot consume `moved` blocks, stop before
applying and use equivalent `tofu state mv` commands for the same old/new address
pairs. Do not apply a plan that proposes deleting/replacing the live OCI VCN,
subnet, buckets, bastion, or Talos node as part of this refactor.

## Adopting the already-live infra

The pilot infra was first stood up imperatively. To bring it under OpenTofu
without recreating it, import into the current module addresses:

```sh
tofu import module.network.oci_core_vcn.mnt              <vcn-ocid>
tofu import module.network.oci_core_internet_gateway.mnt <ig-ocid>
tofu import module.network.oci_core_route_table.mnt      <rt-ocid>
tofu import module.network.oci_core_security_list.mnt    <sl-ocid>
tofu import module.network.oci_core_subnet.mnt_public    <subnet-ocid>

tofu import module.storage.oci_objectstorage_bucket.db_backups      <namespace>/mnt-db-backups
tofu import module.storage.oci_objectstorage_bucket.evidence        <namespace>/mnt-evidence
tofu import module.storage.oci_objectstorage_bucket.evidence_replica <namespace>/mnt-evidence-replica

tofu import module.bastion.oci_bastion_bastion.mnt <bastion-ocid>
```

**The Talos node is deliberately not imported, and importing it is its own
change — not a step of this one.** IaC ownership of an irreplaceable resource is
mostly downside: the payoff is recreate-from-code, which is the one forbidden
operation, and `oci_core_instance` replaces on `shape`, `availability_domain`,
`subnet_id`, and `source_details.source_id`. The live node diverges from this
config on at least two of those (its `display_name` is `control`, and its boot
volume was `dd`-flashed rather than launched from `talos_image_ocid`), so a
first plan would propose a replacement that must never be applied and could not
succeed anyway against a spent allotment.

If ownership is wanted later, the order is: remote state with locking → the
`prevent_destroy` guard already in `primitives/compute` → reconcile the config
to the live resource → `import {}` with `-generate-config-out` → **the plan must
show zero changes** before anything is applied.

Then `tofu plan` — it should converge to the hardened security-list rules above
(the live list was opened more broadly during bootstrap; apply tightens it) and
must not remove OCI from supported infrastructure.
