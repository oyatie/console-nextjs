terraform {
  required_providers {
    oci = {
      source = "oracle/oci"
    }
  }
}

variable "compartment_ocid" {
  type        = string
  description = "Compartment the OCI guest network lives in."
}

variable "vcn_cidr" {
  type        = string
  description = "CIDR block for the OCI guest VCN."
}

variable "subnet_cidr" {
  type        = string
  description = "CIDR block for the public subnet."
}

variable "admin_cidr" {
  type        = string
  description = "Admin source CIDR allowed to reach Talos, k8s, and helper SSH."
}

variable "tags" {
  type        = map(string)
  description = "Freeform tags applied to OCI resources."
}

resource "oci_core_vcn" "mnt" {
  compartment_id = var.compartment_ocid
  cidr_blocks    = [var.vcn_cidr]
  display_name   = "mnt-vcn"
  dns_label      = "mntvcn"
  freeform_tags  = var.tags
}

resource "oci_core_internet_gateway" "mnt" {
  compartment_id = var.compartment_ocid
  vcn_id         = oci_core_vcn.mnt.id
  enabled        = true
  display_name   = "mnt-ig"
  freeform_tags  = var.tags
}

resource "oci_core_route_table" "mnt" {
  compartment_id = var.compartment_ocid
  vcn_id         = oci_core_vcn.mnt.id
  display_name   = "mnt-rt"

  route_rules {
    destination       = "0.0.0.0/0"
    destination_type  = "CIDR_BLOCK"
    network_entity_id = oci_core_internet_gateway.mnt.id
  }

  freeform_tags = var.tags
}

# Hardened ingress: the Talos (50000) and Kubernetes (6443) control-plane APIs
# are reachable ONLY from the admin CIDR and from inside the VCN (the managed
# bastion / cluster components) — never the public internet. Only the app's
# 80/443 is world-open. ICMP type 3 keeps PMTU discovery working.
resource "oci_core_security_list" "mnt" {
  compartment_id = var.compartment_ocid
  vcn_id         = oci_core_vcn.mnt.id
  display_name   = "mnt-sl"
  freeform_tags  = var.tags

  egress_security_rules {
    protocol    = "all"
    destination = "0.0.0.0/0"
    description = "all egress"
  }

  # Public HTTP/HTTPS (Traefik ingress + ACME HTTP-01).
  dynamic "ingress_security_rules" {
    for_each = { http = 80, https = 443 }

    content {
      protocol    = "6" # TCP
      source      = "0.0.0.0/0"
      description = "public ${ingress_security_rules.key}"

      tcp_options {
        min = ingress_security_rules.value
        max = ingress_security_rules.value
      }
    }
  }

  # Control-plane APIs (Talos 50000-50001, k8s 6443) from admin + intra-VCN only.
  dynamic "ingress_security_rules" {
    for_each = {
      talos_admin = { src = var.admin_cidr, min = 50000, max = 50001, d = "Talos API (admin)" }
      talos_vcn   = { src = var.vcn_cidr, min = 50000, max = 50001, d = "Talos API (intra-VCN)" }
      k8s_admin   = { src = var.admin_cidr, min = 6443, max = 6443, d = "k8s API (admin)" }
      k8s_vcn     = { src = var.vcn_cidr, min = 6443, max = 6443, d = "k8s API (intra-VCN)" }
      ssh_admin   = { src = var.admin_cidr, min = 22, max = 22, d = "SSH to bastion (admin)" }
    }

    content {
      protocol    = "6"
      source      = ingress_security_rules.value.src
      description = ingress_security_rules.value.d

      tcp_options {
        min = ingress_security_rules.value.min
        max = ingress_security_rules.value.max
      }
    }
  }

  # ICMP fragmentation-needed so path-MTU discovery does not black-hole.
  ingress_security_rules {
    protocol    = "1" # ICMP
    source      = "0.0.0.0/0"
    description = "PMTUD (fragmentation-needed)"

    icmp_options {
      type = 3
      code = 4
    }
  }
}

resource "oci_core_subnet" "mnt_public" {
  compartment_id    = var.compartment_ocid
  vcn_id            = oci_core_vcn.mnt.id
  cidr_block        = var.subnet_cidr
  display_name      = "mnt-subnet"
  dns_label         = "mntsub"
  route_table_id    = oci_core_route_table.mnt.id
  security_list_ids = [oci_core_security_list.mnt.id]
  freeform_tags     = var.tags
}

output "vcn_id" {
  value = oci_core_vcn.mnt.id
}

output "public_subnet_id" {
  value = oci_core_subnet.mnt_public.id
}

output "security_list_id" {
  value = oci_core_security_list.mnt.id
}

output "route_table_id" {
  value = oci_core_route_table.mnt.id
}

output "internet_gateway_id" {
  value = oci_core_internet_gateway.mnt.id
}
