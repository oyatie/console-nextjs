module "bastion" {
  source = "./contexts/oci-guest/primitives/bastion"

  compartment_ocid = var.compartment_ocid
  target_subnet_id = module.network.public_subnet_id
  admin_cidr       = var.admin_cidr
  tags             = var.tags
}
