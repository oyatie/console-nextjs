terraform {
  required_version = ">= 1.8"
  required_providers {
    oci = {
      source  = "oracle/oci"
      version = "~> 7.0"
    }
  }
}

# Authenticates from ~/.oci/config (API-key auth). Region is overridable so the
# same module can target another OCI region.
provider "oci" {
  region = var.region
}
