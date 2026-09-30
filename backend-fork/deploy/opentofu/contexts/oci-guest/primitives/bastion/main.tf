terraform {
  required_providers {
    oci = {
      source = "oracle/oci"
    }
  }
}

variable "compartment_ocid" {
  type        = string
  description = "Compartment the managed OCI Bastion lives in."
}

variable "target_subnet_id" {
  type        = string
  description = "Subnet the managed OCI Bastion can target."
}

variable "admin_cidr" {
  type        = string
  description = "Admin source CIDR allowed to create Bastion sessions."
}

variable "tags" {
  type        = map(string)
  description = "Freeform tags applied to OCI resources."
}

# Managed OCI Bastion — the hardened way to reach the node's PRIVATE IP
# (Talos 50000 / k8s 6443) from an admin workstation without a public-IP jump
# host. Create a port-forwarding session against the node's private IP, e.g.:
#   oci bastion session create-port-forwarding --bastion-id <id> \
#     --target-private-ip 10.0.0.x --target-port 6443 \
#     --ssh-public-key-file ~/.ssh/id.pub --session-ttl-in-seconds 10800
resource "oci_bastion_bastion" "mnt" {
  bastion_type                 = "standard"
  compartment_id               = var.compartment_ocid
  target_subnet_id             = var.target_subnet_id
  name                         = "mnt-bsvc"
  client_cidr_block_allow_list = [var.admin_cidr]
  freeform_tags                = var.tags
}

output "bastion_ocid" {
  value = oci_bastion_bastion.mnt.id
}
