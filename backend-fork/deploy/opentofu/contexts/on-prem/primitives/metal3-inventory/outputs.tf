output "capi_inventory" {
  description = "CAPI/CAPM3/Talos inventory bundle for a downstream manifest renderer."
  value       = local.capi_inventory
}

output "capi_cluster" {
  description = "Cluster API Cluster manifest object."
  value       = local.capi_cluster
}

output "metal3_cluster" {
  description = "CAPM3 Metal3Cluster manifest object with noCloudProvider=true."
  value       = local.metal3_cluster
}

output "metal3_machine_templates" {
  description = "Control-plane and worker Metal3MachineTemplate manifest objects."
  value       = local.metal3_machine_templates
}

output "bare_metal_hosts" {
  description = "BareMetalHost manifest objects keyed by host name."
  value       = local.bare_metal_hosts
}

output "network_data_by_host" {
  description = "Static-address host network facts for networkData Secret rendering."
  value       = local.network_data_by_host
}

output "metal3_host_selectors" {
  description = "Role-based host selectors for Metal3MachineTemplate specs."
  value       = local.metal3_host_selectors
}

output "talos_config_patches" {
  description = "Role-level Talos config patches adapted from the Oyatie/Talos CAPI substrate pattern."
  value       = local.talos_config_patches
}

output "talos_node_metadata" {
  description = "Per-node Talos install metadata and disk facts."
  value       = local.talos_node_metadata
}

output "bmc_credential_secret_names" {
  description = "Map of host name to Kubernetes Secret name containing BMC credentials."
  value       = local.bmc_credential_secret_names
}

output "node_names_by_role" {
  description = "Host names grouped into control-plane and worker roles."
  value       = local.node_names_by_role
}
