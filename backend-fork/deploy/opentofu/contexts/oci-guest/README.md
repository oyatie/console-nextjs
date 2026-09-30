# OCI guest deployment context

This context keeps the current OCI target as a first-class supported deployment
shape while moving the resource primitives out of the root service wrapper.

The root `deploy/opentofu` stack remains the compatibility entrypoint operators
run today. Its wrapper files contain module blocks only, and `moved.tf` maps the
old root resource addresses to the primitive module addresses so existing state
can migrate without replacing live OCI resources.

## Primitives

- `primitives/network`: VCN, internet gateway, route table, hardened security
  list, and public subnet.
- `primitives/storage`: Object Storage namespace lookup and private buckets for
  DB backups, evidence, and evidence replica.
- `primitives/compute`: the single Talos control-plane node, governed by
  `talos_image_ocid` and guarded by `prevent_destroy`. It is the only instance
  this context may create — it consumes the entire free-tier A1 allotment.
- `primitives/bastion`: managed OCI Bastion service for private-IP access.

Do not remove OCI support when adding on-prem or other contexts; add sibling
contexts/wrappers instead and preserve this import/state migration contract.
