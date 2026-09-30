# Primitive: on-prem/metal3-inventory

`metal3-inventory` is a provider-free OpenTofu primitive for the Maintenance
on-prem context. It validates and normalizes bare-metal host inventory for a
future Cluster API + cluster-api-provider-metal3 + Talos render/apply lane.

It intentionally declares no cloud IaaS provider and no `resource` blocks. The
primitive emits typed output objects that a later renderer can turn into
`Cluster`, `Metal3Cluster`, `Metal3MachineTemplate`, `BareMetalHost`, Talos
bootstrap, and Metal3 `networkData` Secret manifests.

## Boundary

In scope:

- host identity and Kubernetes-safe names;
- host role (`control-plane` or `worker`) and role selectors;
- BMC address and credentials Secret name only;
- static network addresses, gateways, DNS, interfaces, VLANs, MTU;
- install disks, root device hints, and data-disk purposes;
- Talos image/checksum/installer metadata, worker labels, kernel modules, and
  Cilium/Talos CAPI config-patch defaults;
- manifest-ready CAPI/CAPM3 object skeletons with `noCloudProvider = true`.

Out of scope:

- BMC passwords or secret material;
- OCI/AWS/GCP resources;
- Ironic, CAPM3, Talos, or Kubernetes provider side effects;
- generated Talos machine configs, kubeconfigs, or cluster secrets;
- live apply/cutover.

## Inputs

See `variables.tf` for the machine-readable contract. Important rules:

- `cluster.control_plane_endpoint.host` must be the future site VIP/load balancer.
- `nodes` must include at least one `control-plane` host.
- BMC passwords never live in tfvars; each host references a Kubernetes Secret by
  name through `bmc.credentials_secret_name`.
- `talos.image_checksum` must be replaced with the real Talos Image Factory
  checksum before activation.
- `talos.cni_name = "none"` and `talos.proxy_disabled = true` preserve the
  Oyatie/Talos CAPI pattern where Cilium supplies CNI and kube-proxy replacement.

## Outputs

- `capi_inventory`: aggregate object for downstream renderers.
- `capi_cluster`: Cluster API `Cluster` object.
- `metal3_cluster`: CAPM3 `Metal3Cluster` object with `noCloudProvider = true`.
- `metal3_machine_templates`: control-plane and worker template skeletons.
- `bare_metal_hosts`: `BareMetalHost` objects with BMC, image, and disk hints.
- `network_data_by_host`: per-host static network facts for `networkData` Secret
  rendering.
- `metal3_host_selectors`: selectors matching the generated host labels.
- `talos_config_patches`: role-level Talos JSON patches for the CAPI/Talos lane.
- `talos_node_metadata`: per-node Talos install/disk metadata.
- `bmc_credential_secret_names`: host-to-secret-name map for security review.
- `node_names_by_role`: host names grouped by role for HA/readiness checks.

## Validation

From the repo root:

```sh
tofu -chdir=deploy/opentofu/contexts/on-prem init -backend=false
tofu -chdir=deploy/opentofu/contexts/on-prem validate
```

No OCI credentials are required because this primitive and its wrapper have no
cloud provider configuration.
