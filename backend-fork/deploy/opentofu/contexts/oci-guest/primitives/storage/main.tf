terraform {
  required_providers {
    oci = {
      source = "oracle/oci"
    }
  }
}

variable "compartment_ocid" {
  type        = string
  description = "Compartment the OCI guest buckets live in."
}

variable "tags" {
  type        = map(string)
  description = "Freeform tags applied to OCI resources."
}

data "oci_objectstorage_namespace" "ns" {
  compartment_id = var.compartment_ocid
}

# DB PITR backups (CNPG Barman) + evidence object store (S3-compatible API).
resource "oci_objectstorage_bucket" "db_backups" {
  compartment_id = var.compartment_ocid
  namespace      = data.oci_objectstorage_namespace.ns.namespace
  name           = "mnt-db-backups"
  access_type    = "NoPublicAccess"
  versioning     = "Enabled" # extra protection for backup objects
  freeform_tags  = var.tags
}

resource "oci_objectstorage_bucket" "evidence" {
  compartment_id = var.compartment_ocid
  namespace      = data.oci_objectstorage_namespace.ns.namespace
  name           = "mnt-evidence"
  access_type    = "NoPublicAccess"
  freeform_tags  = var.tags
}

# WORM replica of the evidence bucket. Evidence (repair photo/video, financial
# documents) is mirrored here and integrity-verified before financial operations
# proceed — data resiliency + tamper-evidence. Versioned; no public access.
resource "oci_objectstorage_bucket" "evidence_replica" {
  compartment_id = var.compartment_ocid
  namespace      = data.oci_objectstorage_namespace.ns.namespace
  name           = "mnt-evidence-replica"
  access_type    = "NoPublicAccess"
  versioning     = "Enabled"
  freeform_tags  = var.tags
}

output "object_namespace" {
  value = data.oci_objectstorage_namespace.ns.namespace
}

output "db_backups_bucket" {
  value = oci_objectstorage_bucket.db_backups.name
}

output "evidence_bucket" {
  value = oci_objectstorage_bucket.evidence.name
}

output "evidence_replica_bucket" {
  value = oci_objectstorage_bucket.evidence_replica.name
}
