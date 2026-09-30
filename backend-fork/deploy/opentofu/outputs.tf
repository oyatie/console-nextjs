output "vcn_id" { value = module.network.vcn_id }
output "subnet_id" { value = module.network.public_subnet_id }
output "security_list_id" { value = module.network.security_list_id }
output "bastion_ocid" { value = module.bastion.bastion_ocid }
output "object_namespace" { value = module.storage.object_namespace }

output "db_backups_bucket" { value = module.storage.db_backups_bucket }
output "evidence_bucket" { value = module.storage.evidence_bucket }

output "node_public_ip" {
  value       = module.compute.node_public_ip
  description = "Talos node public IP (Traefik 80/443). Reserve it for stable DNS."
}
