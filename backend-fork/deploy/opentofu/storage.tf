module "storage" {
  source = "./contexts/oci-guest/primitives/storage"

  compartment_ocid = var.compartment_ocid
  tags             = var.tags
}
