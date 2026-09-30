# OpenTofu on-prem context: Metal3 inventory

This is the provider-neutral on-prem deployment context for future bare-metal
Maintenance clusters. It is intentionally additive beside the live OCI guest
OpenTofu stack: validating this context must not load the OCI provider, require
`~/.oci/config`, or declare cloud IaaS resources.

The first primitive models the site inventory that downstream Cluster API with
cluster-api-provider-metal3 (CAPM3), BareMetalHost, and Talos bootstrap lanes need
before any hardware activation.

## ADR-0339 layout

```text
deploy/opentofu/contexts/on-prem/
  main.tf, variables.tf, outputs.tf       # thin context wrapper: module blocks only
  inventory.tfvars.example                # sanitized example inventory
  primitives/metal3-inventory/            # reusable on-prem primitive
    versions.tf, variables.tf, main.tf, outputs.tf, README.md
```

The wrapper only selects `primitives/metal3-inventory`. Resource bodies and the
input/output contract live in the primitive. Today the primitive declares no
`resource` blocks and no cloud provider; it emits manifest-ready data structures
for the CAPI/CAPM3/Talos render lane. A later HA lane can add sibling primitives
(for example `capi-render`, `ironic-bootstrap`, or `talos-control-plane`) without
turning this wrapper into a hand-written stack.

## Inputs

The wrapper forwards these values into the primitive:

- `cluster`: CAPI cluster name, namespace, Kubernetes version, pod/service CIDRs,
  labels, and a stable control-plane endpoint. For on-prem this endpoint must be
  a VIP/load-balancer address, not one node IP.
- `talos`: shared Talos installer/image/checksum/default install-disk metadata,
  CNI/proxy settings, API server SANs, worker labels, and worker kernel modules.
- `nodes`: map keyed by the intended host/BareMetalHost name. Each node records:
  - `role`: `control-plane` or `worker`;
  - `boot_mac_address` and `boot_mode`;
  - BMC Redfish/IPMI address plus a Kubernetes Secret name for credentials;
  - static network address, gateway, DNS, interface, VLAN, and MTU facts;
  - install disk, CAPM3 root device hints, and optional data disks;
  - optional Talos node labels, schematic ID, installer/image/disk overrides.
- `capm3`: API-version knobs for CAPI, CAPM3, BareMetalHost, and Talos provider
  upgrades.
- `common_labels`: labels applied to generated manifest-ready objects.

Never put BMC passwords, Talos machine secrets, `talosconfig`, kubeconfig, or
rendered machine config in this context. Store BMC passwords in SOPS/OpenBao/K8s
Secrets and refer to the Secret names here.

## Outputs

- `capi_inventory`: one bundle containing Cluster, Metal3Cluster,
  Metal3MachineTemplate, BareMetalHost, host network, and Talos metadata.
- `bare_metal_hosts`: BareMetalHost manifest objects keyed by host name.
- `metal3_host_selectors`: role selectors for CAPM3 machine templates.
- `network_data_by_host`: sanitized static network facts for networkData Secret
  rendering.
- `talos_config_patches`: role-level Talos JSON-patch fragments adapted from the
  Oyatie CAPI/Talos pattern (`cni:none`, `proxy.disabled`, installer image,
  worker kernel modules, worker labels, and API server SANs).
- `talos_node_metadata`: per-host disk/wipe/image/schematic/label metadata.

## Validate

```sh
tofu -chdir=deploy/opentofu/contexts/on-prem init -backend=false
tofu -chdir=deploy/opentofu/contexts/on-prem validate
```

Validation is intentionally offline and provider-free. To inspect the example
inventory without touching hardware, copy `inventory.tfvars.example` to a private
`*.tfvars` file and run a refresh-free plan; do not apply until real nodes,
BMC Secrets, Talos image checksums, and an operator activation ticket exist.

```sh
cp deploy/opentofu/contexts/on-prem/inventory.tfvars.example /tmp/mnt-onprem.tfvars
tofu -chdir=deploy/opentofu/contexts/on-prem plan -refresh=false -var-file=/tmp/mnt-onprem.tfvars
```

The plan should contain outputs only; no cloud resources should be created.
