module "compute" {
  source = "./contexts/oci-guest/primitives/compute"

  compartment_ocid    = var.compartment_ocid
  availability_domain = var.availability_domain
  subnet_id           = module.network.public_subnet_id
  node_ocpus          = var.node_ocpus
  node_memory_gbs     = var.node_memory_gbs
  talos_image_ocid    = var.talos_image_ocid
  tags                = var.tags
}
