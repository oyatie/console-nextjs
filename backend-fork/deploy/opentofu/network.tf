module "network" {
  source = "./contexts/oci-guest/primitives/network"

  compartment_ocid = var.compartment_ocid
  vcn_cidr         = var.vcn_cidr
  subnet_cidr      = var.subnet_cidr
  admin_cidr       = var.admin_cidr
  tags             = var.tags
}
