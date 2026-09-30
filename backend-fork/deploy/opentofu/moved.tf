# Address-preserving state migration for the ADR-0339-style oci-guest context.
# These moved blocks let existing imported state follow the resources into the
# module primitives without delete/recreate churn.
moved {
  from = oci_core_vcn.mnt
  to   = module.network.oci_core_vcn.mnt
}

moved {
  from = oci_core_internet_gateway.mnt
  to   = module.network.oci_core_internet_gateway.mnt
}

moved {
  from = oci_core_route_table.mnt
  to   = module.network.oci_core_route_table.mnt
}

moved {
  from = oci_core_security_list.mnt
  to   = module.network.oci_core_security_list.mnt
}

moved {
  from = oci_core_subnet.mnt_public
  to   = module.network.oci_core_subnet.mnt_public
}

moved {
  from = oci_objectstorage_bucket.db_backups
  to   = module.storage.oci_objectstorage_bucket.db_backups
}

moved {
  from = oci_objectstorage_bucket.evidence
  to   = module.storage.oci_objectstorage_bucket.evidence
}

moved {
  from = oci_objectstorage_bucket.evidence_replica
  to   = module.storage.oci_objectstorage_bucket.evidence_replica
}

moved {
  from = oci_core_instance.node[0]
  to   = module.compute.oci_core_instance.node[0]
}

moved {
  from = oci_bastion_bastion.mnt
  to   = module.bastion.oci_bastion_bastion.mnt
}
