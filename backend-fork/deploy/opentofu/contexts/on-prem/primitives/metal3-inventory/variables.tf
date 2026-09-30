variable "cluster" {
  description = "Cluster API cluster contract for the on-prem site."
  type = object({
    name               = string
    namespace          = optional(string, "default")
    kubernetes_version = string
    control_plane_endpoint = object({
      host = string
      port = optional(number, 6443)
    })
    pod_cidr_blocks     = optional(list(string), ["10.244.0.0/16"])
    service_cidr_blocks = optional(list(string), ["10.96.0.0/12"])
    labels              = optional(map(string), {})
  })

  validation {
    condition     = can(regex("^[a-z0-9]([-a-z0-9]*[a-z0-9])?$", var.cluster.name))
    error_message = "cluster.name must be a Kubernetes-safe DNS label."
  }

  validation {
    condition     = trimspace(var.cluster.control_plane_endpoint.host) != ""
    error_message = "cluster.control_plane_endpoint.host must be a stable VIP/load-balancer endpoint."
  }
}

variable "talos" {
  description = "Talos install and bootstrap metadata shared by the inventory."
  type = object({
    installer_image            = string
    image_url                  = string
    image_checksum             = string
    image_format               = optional(string, "raw")
    control_plane_install_disk = optional(string, "/dev/sda")
    worker_install_disk        = optional(string, "/dev/sda")
    wipe                       = optional(bool, false)
    cni_name                   = optional(string, "none")
    proxy_disabled             = optional(bool, true)
    worker_node_labels         = optional(map(string), {})
    worker_kernel_modules      = optional(list(string), ["vhost_net", "vhost_vsock"])
    api_server_cert_sans       = optional(list(string), [])
  })

  validation {
    condition     = trimspace(var.talos.installer_image) != "" && trimspace(var.talos.image_url) != "" && trimspace(var.talos.image_checksum) != ""
    error_message = "talos.installer_image, talos.image_url, and talos.image_checksum are required."
  }
}

variable "nodes" {
  description = "Bare-metal node inventory keyed by Kubernetes/BareMetalHost-safe host name."
  type = map(object({
    role             = string
    boot_mac_address = string
    boot_mode        = optional(string, "UEFI")
    online           = optional(bool, true)
    bmc = object({
      address                          = string
      credentials_secret_name          = string
      disable_certificate_verification = optional(bool, false)
    })
    network = object({
      hostname      = optional(string)
      ip_address    = string
      prefix_length = number
      gateway       = string
      dns_servers   = optional(list(string), [])
      vlan_id       = optional(number)
      interfaces = optional(list(object({
        name        = string
        mac_address = string
        vlan_id     = optional(number)
        mtu         = optional(number)
      })), [])
    })
    disks = object({
      install_disk = string
      root_device_hints = optional(object({
        device_name        = optional(string)
        wwn                = optional(string)
        serial_number      = optional(string)
        hctl               = optional(string)
        min_size_gigabytes = optional(number)
        rotational         = optional(bool)
      }), null)
      data_disks = optional(list(object({
        name               = string
        device             = string
        purpose            = string
        min_size_gigabytes = optional(number)
      })), [])
    })
    talos = optional(object({
      installer_image = optional(string)
      image_url       = optional(string)
      install_disk    = optional(string)
      wipe            = optional(bool)
      schematic_id    = optional(string)
      node_labels     = optional(map(string), {})
    }), {})
    labels      = optional(map(string), {})
    annotations = optional(map(string), {})
  }))

  validation {
    condition     = alltrue([for name, _ in var.nodes : can(regex("^[a-z0-9]([-a-z0-9]*[a-z0-9])?$", name))])
    error_message = "Every node key must be a Kubernetes-safe DNS label."
  }

  validation {
    condition     = alltrue([for _, node in var.nodes : contains(["control-plane", "worker"], node.role)])
    error_message = "Each node.role must be either control-plane or worker."
  }

  validation {
    condition     = length([for _, node in var.nodes : node if node.role == "control-plane"]) > 0
    error_message = "At least one control-plane node is required."
  }

  validation {
    condition     = alltrue([for _, node in var.nodes : can(regex("^([0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}$", node.boot_mac_address))])
    error_message = "Each boot_mac_address must be a colon-separated MAC address."
  }
}

variable "capm3" {
  description = "Provider API-version knobs for CAPI/CAPM3/Talos upgrades."
  type = object({
    cluster_api_version             = optional(string, "cluster.x-k8s.io/v1beta1")
    metal3_api_version              = optional(string, "infrastructure.cluster.x-k8s.io/v1beta1")
    bare_metal_host_api_version     = optional(string, "metal3.io/v1alpha1")
    talos_control_plane_api_version = optional(string, "controlplane.cluster.x-k8s.io/v1alpha3")
    talos_bootstrap_api_version     = optional(string, "bootstrap.cluster.x-k8s.io/v1alpha3")
  })
  default = {}
}

variable "common_labels" {
  description = "Labels applied to generated manifest-ready objects."
  type        = map(string)
  default     = {}
}
