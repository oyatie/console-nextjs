# Thin context wrapper per ADR-0339: this file selects the on-prem primitive and
# passes site inventory. Resource bodies live under primitives/on-prem/*, not in
# the wrapper. This context intentionally declares no cloud IaaS provider.
module "metal3_inventory" {
  source = "./primitives/metal3-inventory"

  cluster       = var.cluster
  talos         = var.talos
  nodes         = var.nodes
  capm3         = var.capm3
  common_labels = var.common_labels
}
