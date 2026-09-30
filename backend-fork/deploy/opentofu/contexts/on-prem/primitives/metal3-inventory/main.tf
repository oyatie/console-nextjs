locals {
  base_labels = merge(
    {
      "app.kubernetes.io/part-of"     = "maintenance"
      "maintenance.io/context"        = "on-prem"
      "cluster.x-k8s.io/cluster-name" = var.cluster.name
    },
    var.common_labels,
    var.cluster.labels,
  )

  node_names_by_role = {
    control_plane = [for name, node in var.nodes : name if node.role == "control-plane"]
    workers       = [for name, node in var.nodes : name if node.role == "worker"]
  }

  role_labels = {
    control_plane = "control-plane"
    worker        = "worker"
  }

  root_device_hints = {
    for name, node in var.nodes : name => (
      try(node.disks.root_device_hints, null) == null ? null : {
        for hint_key, hint_value in {
          deviceName       = try(node.disks.root_device_hints.device_name, null)
          wwn              = try(node.disks.root_device_hints.wwn, null)
          serialNumber     = try(node.disks.root_device_hints.serial_number, null)
          hctl             = try(node.disks.root_device_hints.hctl, null)
          minSizeGigabytes = try(node.disks.root_device_hints.min_size_gigabytes, null)
          rotational       = try(node.disks.root_device_hints.rotational, null)
        } : hint_key => hint_value if hint_value != null
      }
    )
  }

  node_interfaces = {
    for name, node in var.nodes : name => (
      length(node.network.interfaces) > 0 ? [
        for iface in node.network.interfaces : {
          name        = iface.name
          mac_address = iface.mac_address
          vlan_id     = try(iface.vlan_id, null)
          mtu         = try(iface.mtu, null)
        }
        ] : [
        {
          name        = "boot"
          mac_address = node.boot_mac_address
          vlan_id     = try(node.network.vlan_id, null)
          mtu         = null
        }
      ]
    )
  }

  network_data_by_host = {
    for name, node in var.nodes : name => {
      hostname   = coalesce(try(node.network.hostname, null), name)
      interfaces = local.node_interfaces[name]
      ipv4 = {
        address       = node.network.ip_address
        prefix_length = node.network.prefix_length
        gateway       = node.network.gateway
        dns_servers   = node.network.dns_servers
      }
    }
  }

  talos_node_metadata = {
    for name, node in var.nodes : name => {
      role            = node.role
      install_disk    = coalesce(try(node.talos.install_disk, null), node.disks.install_disk)
      wipe            = coalesce(try(node.talos.wipe, null), var.talos.wipe)
      installer_image = coalesce(try(node.talos.installer_image, null), var.talos.installer_image)
      image_url       = coalesce(try(node.talos.image_url, null), var.talos.image_url)
      image_checksum  = var.talos.image_checksum
      image_format    = var.talos.image_format
      schematic_id    = try(node.talos.schematic_id, null)
      node_labels     = try(node.talos.node_labels, {})
      data_disks      = node.disks.data_disks
    }
  }

  metal3_host_selectors = {
    control_plane = {
      matchLabels = merge(local.base_labels, {
        "maintenance.io/node-role" = local.role_labels.control_plane
      })
    }
    worker = {
      matchLabels = merge(local.base_labels, {
        "maintenance.io/node-role" = local.role_labels.worker
      })
    }
  }

  bare_metal_hosts = {
    for name, node in var.nodes : name => {
      apiVersion = var.capm3.bare_metal_host_api_version
      kind       = "BareMetalHost"
      metadata = {
        name      = name
        namespace = var.cluster.namespace
        labels = merge(local.base_labels, {
          "maintenance.io/node-role" = node.role
          "maintenance.io/host"      = name
        }, node.labels)
        annotations = node.annotations
      }
      spec = merge(
        {
          online         = node.online
          bootMACAddress = node.boot_mac_address
          bootMode       = node.boot_mode
          bmc = {
            address                        = node.bmc.address
            credentialsName                = node.bmc.credentials_secret_name
            disableCertificateVerification = node.bmc.disable_certificate_verification
          }
          image = {
            url      = local.talos_node_metadata[name].image_url
            checksum = local.talos_node_metadata[name].image_checksum
            format   = local.talos_node_metadata[name].image_format
          }
        },
        local.root_device_hints[name] == null ? {} : { rootDeviceHints = local.root_device_hints[name] },
      )
    }
  }

  metal3_cluster = {
    apiVersion = var.capm3.metal3_api_version
    kind       = "Metal3Cluster"
    metadata = {
      name      = var.cluster.name
      namespace = var.cluster.namespace
      labels    = local.base_labels
    }
    spec = {
      controlPlaneEndpoint = {
        host = var.cluster.control_plane_endpoint.host
        port = var.cluster.control_plane_endpoint.port
      }
      noCloudProvider = true
    }
  }

  capi_cluster = {
    apiVersion = var.capm3.cluster_api_version
    kind       = "Cluster"
    metadata = {
      name      = var.cluster.name
      namespace = var.cluster.namespace
      labels    = local.base_labels
    }
    spec = {
      clusterNetwork = {
        pods     = { cidrBlocks = var.cluster.pod_cidr_blocks }
        services = { cidrBlocks = var.cluster.service_cidr_blocks }
      }
      controlPlaneRef = {
        apiVersion = var.capm3.talos_control_plane_api_version
        kind       = "TalosControlPlane"
        name       = "${var.cluster.name}-control-plane"
      }
      infrastructureRef = {
        apiVersion = var.capm3.metal3_api_version
        kind       = "Metal3Cluster"
        name       = var.cluster.name
      }
    }
  }

  metal3_machine_templates = {
    control_plane = {
      apiVersion = var.capm3.metal3_api_version
      kind       = "Metal3MachineTemplate"
      metadata = {
        name      = "${var.cluster.name}-control-plane"
        namespace = var.cluster.namespace
        labels    = local.metal3_host_selectors.control_plane.matchLabels
      }
      spec = {
        template = {
          spec = {
            hostSelector = local.metal3_host_selectors.control_plane
            image = {
              url      = var.talos.image_url
              checksum = var.talos.image_checksum
              format   = var.talos.image_format
            }
          }
        }
      }
    }
    worker = {
      apiVersion = var.capm3.metal3_api_version
      kind       = "Metal3MachineTemplate"
      metadata = {
        name      = "${var.cluster.name}-worker"
        namespace = var.cluster.namespace
        labels    = local.metal3_host_selectors.worker.matchLabels
      }
      spec = {
        template = {
          spec = {
            hostSelector = local.metal3_host_selectors.worker
            image = {
              url      = var.talos.image_url
              checksum = var.talos.image_checksum
              format   = var.talos.image_format
            }
          }
        }
      }
    }
  }

  talos_config_patches = {
    control_plane = [
      { op = "add", path = "/cluster/network/cni", value = { name = var.talos.cni_name } },
      { op = "add", path = "/cluster/proxy", value = { disabled = var.talos.proxy_disabled } },
      { op = "add", path = "/machine/install/image", value = var.talos.installer_image },
      { op = "add", path = "/machine/install/disk", value = var.talos.control_plane_install_disk },
      { op = "add", path = "/machine/install/wipe", value = var.talos.wipe },
      { op = "add", path = "/cluster/apiServer/certSANs", value = distinct(concat([var.cluster.control_plane_endpoint.host], var.talos.api_server_cert_sans)) },
    ]
    worker = [
      { op = "add", path = "/cluster/network/cni", value = { name = var.talos.cni_name } },
      { op = "add", path = "/cluster/proxy", value = { disabled = var.talos.proxy_disabled } },
      { op = "add", path = "/machine/install/image", value = var.talos.installer_image },
      { op = "add", path = "/machine/install/disk", value = var.talos.worker_install_disk },
      { op = "add", path = "/machine/install/wipe", value = var.talos.wipe },
      { op = "add", path = "/machine/nodeLabels", value = var.talos.worker_node_labels },
      { op = "add", path = "/machine/kernel", value = { modules = [for module in var.talos.worker_kernel_modules : { name = module }] } },
    ]
  }

  bmc_credential_secret_names = {
    for name, node in var.nodes : name => node.bmc.credentials_secret_name
  }

  capi_inventory = {
    cluster                  = local.capi_cluster
    metal3Cluster            = local.metal3_cluster
    metal3MachineTemplates   = local.metal3_machine_templates
    bareMetalHosts           = local.bare_metal_hosts
    networkDataByHost        = local.network_data_by_host
    metal3HostSelectors      = local.metal3_host_selectors
    talosConfigPatches       = local.talos_config_patches
    talosNodeMetadata        = local.talos_node_metadata
    bmcCredentialSecretNames = local.bmc_credential_secret_names
    nodeNamesByRole          = local.node_names_by_role
  }
}
