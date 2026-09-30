output "capi_inventory" {
  description = "Manifest-ready inventory bundle for CAPI + CAPM3 + Talos bootstrap consumers."
  value       = module.metal3_inventory.capi_inventory
}

output "bare_metal_hosts" {
  description = "BareMetalHost manifest objects keyed by host name."
  value       = module.metal3_inventory.bare_metal_hosts
}

output "metal3_host_selectors" {
  description = "Label selectors for Metal3MachineTemplate control-plane and worker templates."
  value       = module.metal3_inventory.metal3_host_selectors
}

output "network_data_by_host" {
  description = "Sanitized static-address network facts to render into Metal3 networkData secrets."
  value       = module.metal3_inventory.network_data_by_host
}

output "talos_config_patches" {
  description = "Role-level Talos JSON-patch fragments adapted from the OCI/Oyatie CAPI patterns."
  value       = module.metal3_inventory.talos_config_patches
}

output "talos_node_metadata" {
  description = "Per-host Talos install metadata: disk, wipe flag, installer/image overrides, labels, and schematic IDs."
  value       = module.metal3_inventory.talos_node_metadata
}
