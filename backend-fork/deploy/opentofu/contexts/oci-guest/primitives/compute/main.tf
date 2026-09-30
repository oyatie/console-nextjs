terraform {
  required_providers {
    oci = {
      source = "oracle/oci"
    }
  }
}

variable "compartment_ocid" {
  type        = string
  description = "Compartment the OCI guest compute resources live in."
}

variable "availability_domain" {
  type        = string
  description = "Availability domain for the Talos node."
}

variable "subnet_id" {
  type        = string
  description = "Public subnet ID for the Talos node."
}

variable "node_ocpus" {
  type        = number
  description = "OCPUs for the Ampere A1 node."
}

variable "node_memory_gbs" {
  type        = number
  description = "Memory in GiB for the Ampere A1 node."
}

variable "talos_image_ocid" {
  type        = string
  description = "OCID of the bootable Talos arm64 image. Empty disables the Talos node."
}

variable "tags" {
  type        = map(string)
  description = "Freeform tags applied to OCI resources."
}

locals {
  shape = "VM.Standard.A1.Flex"
}

# The Talos control-plane node — the only instance this module may ever create.
# It consumes the ENTIRE free-tier A1 allotment (deploy/OPS-RUNBOOK.md), and the
# allotment has since been cut: a destroy-and-recreate returns 2 vCPU / 12 GB,
# not the 4 / 24 this node holds. That loss is not recoverable by re-applying,
# so the node is guarded below and upgraded in place via talosctl, never here.
resource "oci_core_instance" "node" {
  count               = var.talos_image_ocid != "" ? 1 : 0
  compartment_id      = var.compartment_ocid
  availability_domain = var.availability_domain
  display_name        = "mnt-fsm-node"
  shape               = local.shape

  shape_config {
    ocpus         = var.node_ocpus
    memory_in_gbs = var.node_memory_gbs
  }

  source_details {
    source_type = "image"
    source_id   = var.talos_image_ocid
  }

  create_vnic_details {
    subnet_id        = var.subnet_id
    assign_public_ip = true
  }

  freeform_tags = var.tags

  lifecycle {
    # Talos has no SSH and is configured out of band via talosctl; ignore any
    # metadata/image drift so tofu doesn't try to rebuild the configured node.
    ignore_changes = [source_details, metadata]

    # The node is irreplaceable (see above). This also catches the count going
    # 1 -> 0, which is how an unset talos_image_ocid would otherwise read as
    # "destroy it" rather than as the missing input it is.
    prevent_destroy = true
  }
}

output "node_public_ip" {
  value = try(oci_core_instance.node[0].public_ip, null)
}

output "node_instance_id" {
  value = try(oci_core_instance.node[0].id, null)
}
