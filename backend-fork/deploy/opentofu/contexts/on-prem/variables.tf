variable "cluster" {
  description = "On-prem Cluster API cluster contract. The control-plane endpoint must be a site VIP/load-balancer, not a single node address."
  type        = any
}

variable "talos" {
  description = "Talos install image and config-patch defaults shared by the on-prem Metal3 inventory."
  type        = any
}

variable "nodes" {
  description = "Bare-metal node inventory keyed by the Kubernetes/BareMetalHost-safe host name. BMC credentials are secret names only; do not put passwords in tfvars."
  type        = any
}

variable "capm3" {
  description = "Cluster API / Metal3 / Talos provider API-version overrides for future provider upgrades."
  type        = any
  default     = {}
}

variable "common_labels" {
  description = "Labels applied to generated manifest-ready objects."
  type        = map(string)
  default = {
    "app.kubernetes.io/part-of" = "maintenance"
    "maintenance.io/context"    = "on-prem"
  }
}
