#!/usr/bin/env python3
"""Render DARK CAPI/Metal3 manifests for the on-prem Talos inventory.

The output is a reviewable Kubernetes manifest only. It does not contact a
cluster, does not create BMC credentials, and does not run talosctl. The Talos
bootstrap/control-plane providers consume the same strategic patches used by the
local machineconfig renderer, while Metal3 consumes BareMetalHost inventory and
Metal3MachineTemplates.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from urllib.parse import urlparse

SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_INVENTORY = SCRIPT_DIR / "nodes.example.json"
DEFAULT_CLUSTER_PATCH = SCRIPT_DIR / "cluster.patch.yaml"
DEFAULT_CONTROL_PLANE_PATCH = SCRIPT_DIR / "controlplane.patch.yaml"
DEFAULT_WORKER_PATCH = SCRIPT_DIR / "worker.patch.yaml"
DEFAULT_OUTPUT = SCRIPT_DIR / "capi-metal3.example.yaml"
SAFE_DNS_LABEL = re.compile(r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$")
MAC_ADDRESS = re.compile(r"^[0-9a-fA-F]{2}(:[0-9a-fA-F]{2}){5}$")


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(2)


def load_json(path: Path) -> dict:
    try:
        with path.open("r", encoding="utf-8") as handle:
            data = json.load(handle)
    except json.JSONDecodeError as exc:
        fail(f"{path} is not valid JSON: {exc}")
    except OSError as exc:
        fail(f"cannot read {path}: {exc}")
    if not isinstance(data, dict):
        fail("inventory root must be a JSON object")
    return data


def require_str(data: dict, key: str, *, context: str = "inventory") -> str:
    value = data.get(key)
    if not isinstance(value, str) or not value.strip():
        fail(f"{context} field {key!r} must be a non-empty string")
    return value.strip()


def optional_str(data: dict, key: str, default: str) -> str:
    value = data.get(key, default)
    if not isinstance(value, str) or not value.strip():
        fail(f"inventory field {key!r} must be a non-empty string when set")
    return value.strip()


def require_list(data: dict, key: str) -> list[dict]:
    value = data.get(key)
    if not isinstance(value, list) or not value:
        fail(f"inventory field {key!r} must be a non-empty list")
    for index, item in enumerate(value):
        if not isinstance(item, dict):
            fail(f"{key}[{index}] must be an object")
    return value


def list_of_strings(value: object, field: str, default: list[str] | None = None) -> list[str]:
    if value is None:
        return list(default or [])
    if not isinstance(value, list):
        fail(f"{field} must be a list of strings")
    result: list[str] = []
    for index, item in enumerate(value):
        if not isinstance(item, str) or not item.strip():
            fail(f"{field}[{index}] must be a non-empty string")
        result.append(item.strip())
    return result


def as_bool(value: object, default: bool, field: str) -> bool:
    if value is None:
        return default
    if not isinstance(value, bool):
        fail(f"{field} must be true or false")
    return value


def as_int(value: object, default: int, field: str) -> int:
    if value is None:
        return default
    if not isinstance(value, int) or value < 0:
        fail(f"{field} must be a non-negative integer")
    return value


def yaml_quote(value: object) -> str:
    return json.dumps(str(value), ensure_ascii=False)


def yaml_bool(value: bool) -> str:
    return "true" if value else "false"


def yaml_list(values: list[str], indent: int) -> list[str]:
    prefix = " " * indent
    return [f"{prefix}- {yaml_quote(value)}" for value in values]


def read_patch(path: Path) -> str:
    try:
        text = path.read_text(encoding="utf-8").rstrip() + "\n"
    except OSError as exc:
        fail(f"cannot read patch {path}: {exc}")
    if "allowSchedulingOnControlPlanes: true" in text:
        fail(f"on-prem patch must not enable control-plane scheduling: {path}")
    return text


def validate_name(name: str, field: str) -> str:
    if not SAFE_DNS_LABEL.match(name):
        fail(f"{field} {name!r} must be a Kubernetes DNS label")
    return name


def dedupe(values: list[str]) -> list[str]:
    seen: set[str] = set()
    output: list[str] = []
    for value in values:
        if value not in seen:
            output.append(value)
            seen.add(value)
    return output


def endpoint_parts(endpoint: str) -> tuple[str, int]:
    parsed = urlparse(endpoint)
    if parsed.scheme != "https" or not parsed.hostname:
        fail("control_plane_endpoint must be an https://host[:port] URL")
    return parsed.hostname, parsed.port or 6443


def additional_sans(inventory: dict) -> list[str]:
    endpoint = require_str(inventory, "control_plane_endpoint")
    host, _ = endpoint_parts(endpoint)
    sans = list_of_strings(inventory.get("additional_sans"), "additional_sans")
    sans.append(host)
    vip = inventory.get("control_plane_vip")
    if isinstance(vip, str) and vip.strip():
        sans.append(vip.strip())
    return dedupe(sans)


def generated_cluster_patch(inventory: dict) -> str:
    endpoint = require_str(inventory, "control_plane_endpoint")
    lines = [
        "# Generated from nodes.example.json for CAPI/CABPT; keeps the HA endpoint and cert SANs explicit.",
        "cluster:",
        "  controlPlane:",
        f"    endpoint: {yaml_quote(endpoint)}",
        "  apiServer:",
        "    certSANs:",
    ]
    lines.extend(yaml_list(additional_sans(inventory), 6))
    return "\n".join(lines) + "\n"


def patch_list_block(patches: list[tuple[str, str]], indent: int) -> list[str]:
    spaces = " " * indent
    content_spaces = " " * (indent + 4)
    lines = [f"{spaces}strategicPatches:"]
    for name, text in patches:
        lines.append(f"{spaces}  - |")
        lines.append(f"{content_spaces}# {name}")
        for line in text.rstrip().splitlines():
            lines.append(f"{content_spaces}{line}" if line else content_spaces.rstrip())
    return lines


def get_capi(inventory: dict) -> dict:
    capi = inventory.get("capi", {})
    if not isinstance(capi, dict):
        fail("inventory field 'capi' must be an object when present")
    return capi


def get_image(capi: dict) -> dict:
    image = capi.get("image")
    if not isinstance(image, dict):
        fail("capi.image is required for Metal3MachineTemplate rendering")
    for key in ["url", "checksum", "checksum_type", "format"]:
        require_str(image, key, context="capi.image")
    return image


def node_role_labels(cluster_name: str, role: str) -> dict[str, str]:
    return {
        "cluster.x-k8s.io/cluster-name": cluster_name,
        "console.nousresearch.com/substrate": "on-prem",
        "console.nousresearch.com/onprem-role": role,
    }


def render_labels(labels: dict[str, str], indent: int) -> list[str]:
    lines: list[str] = []
    prefix = " " * indent
    for key in sorted(labels):
        lines.append(f"{prefix}{key}: {yaml_quote(labels[key])}")
    return lines


def node_install_disk(node: dict, defaults: dict) -> str:
    value = node.get("install_disk", defaults.get("install_disk", "/dev/sda"))
    if not isinstance(value, str) or not value.strip():
        fail(f"install_disk for node {node.get('name', '<unnamed>')} must be a non-empty string")
    return value.strip()


def render_baremetal_host(node: dict, *, role: str, namespace: str, cluster_name: str, defaults: dict) -> str:
    name = validate_name(require_str(node, "name", context=f"{role} node"), f"{role} node name")
    mac = require_str(node, "boot_mac_address", context=f"{name}")
    if not MAC_ADDRESS.match(mac):
        fail(f"boot_mac_address for {name} must look like 52:54:00:12:34:56")
    labels = node_role_labels(cluster_name, role)
    failure_domain = node.get("failure_domain")
    if isinstance(failure_domain, str) and failure_domain.strip():
        labels["topology.kubernetes.io/zone"] = failure_domain.strip()
    boot_mode = optional_str(node, "boot_mode", "UEFI")
    bmc_address = require_str(node, "bmc_address", context=name)
    credentials = require_str(node, "bmc_credentials_name", context=name)
    disk = node_install_disk(node, defaults)

    lines = [
        "# Create the referenced BMC credential Secret out-of-band; do not commit BMC usernames/passwords.",
        "apiVersion: metal3.io/v1alpha1",
        "kind: BareMetalHost",
        "metadata:",
        f"  name: {name}",
        f"  namespace: {namespace}",
        "  labels:",
        *render_labels(labels, 4),
        "spec:",
        "  online: true",
        f"  bootMACAddress: {yaml_quote(mac.lower())}",
        f"  bootMode: {yaml_quote(boot_mode)}",
        "  bmc:",
        f"    address: {yaml_quote(bmc_address)}",
        f"    credentialsName: {yaml_quote(credentials)}",
        "  rootDeviceHints:",
        f"    deviceName: {yaml_quote(disk)}",
    ]
    return "\n".join(lines) + "\n"


def render_network_data_template(name: str, *, namespace: str, cluster_name: str, defaults: dict) -> str:
    interface = defaults.get("interface", "eth0")
    if not isinstance(interface, str) or not interface.strip():
        fail("node_defaults.interface must be a non-empty string when set")
    interface = interface.strip()
    nameservers = list_of_strings(defaults.get("nameservers"), "node_defaults.nameservers", default=[])
    lines = [
        "apiVersion: infrastructure.cluster.x-k8s.io/v1beta1",
        "kind: Metal3DataTemplate",
        "metadata:",
        f"  name: {name}",
        f"  namespace: {namespace}",
        "spec:",
        f"  clusterName: {cluster_name}",
        "  metaData:",
        "    objectNames:",
        "      - key: machine_name",
        "        object: machine",
        "      - key: metal3machine_name",
        "        object: metal3machine",
        "      - key: baremetalhost_name",
        "        object: baremetalhost",
        "  networkData:",
        "    links:",
        "      ethernets:",
        "        - type: phy",
        f"          id: {interface}",
        "          macAddress:",
        f"            fromHostInterface: {interface}",
        "    networks:",
        "      ipv4DHCP:",
        "        - id: provisioning",
        f"          link: {interface}",
    ]
    if nameservers:
        lines.append("    services:")
        lines.append("      dns:")
        lines.extend(yaml_list(nameservers, 8))
    return "\n".join(lines) + "\n"


def render_machine_template(name: str, *, namespace: str, role: str, cluster_name: str, image: dict, data_template: str, capi: dict) -> str:
    node_reuse = as_bool(capi.get("node_reuse"), False, "capi.node_reuse")
    automated_cleaning = optional_str(capi, "automated_cleaning_mode", "metadata")
    labels = node_role_labels(cluster_name, role)
    lines = [
        "apiVersion: infrastructure.cluster.x-k8s.io/v1beta1",
        "kind: Metal3MachineTemplate",
        "metadata:",
        f"  name: {name}",
        f"  namespace: {namespace}",
        "spec:",
        f"  nodeReuse: {yaml_bool(node_reuse)}",
        "  template:",
        "    spec:",
        f"      automatedCleaningMode: {automated_cleaning}",
        "      hostSelector:",
        "        matchLabels:",
        *render_labels(labels, 10),
        "      image:",
        f"        url: {yaml_quote(image['url'])}",
        f"        checksum: {yaml_quote(image['checksum'])}",
        f"        checksumType: {yaml_quote(image['checksum_type'])}",
        f"        format: {yaml_quote(image['format'])}",
        "      dataTemplate:",
        f"        name: {data_template}",
    ]
    return "\n".join(lines) + "\n"


def render_cluster(*, namespace: str, cluster_name: str, capi: dict, endpoint_host: str, endpoint_port: int) -> str:
    pod_cidrs = list_of_strings(capi.get("pod_cidrs"), "capi.pod_cidrs", default=["10.244.0.0/16"])
    service_cidrs = list_of_strings(capi.get("service_cidrs"), "capi.service_cidrs", default=["10.96.0.0/12"])
    service_domain = optional_str(capi, "service_domain", "cluster.local")
    control_plane_name = f"{cluster_name}-controlplane"
    lines = [
        "apiVersion: cluster.x-k8s.io/v1beta1",
        "kind: Cluster",
        "metadata:",
        f"  name: {cluster_name}",
        f"  namespace: {namespace}",
        "spec:",
        "  clusterNetwork:",
        "    pods:",
        "      cidrBlocks:",
        *yaml_list(pod_cidrs, 8),
        "    services:",
        "      cidrBlocks:",
        *yaml_list(service_cidrs, 8),
        f"    serviceDomain: {yaml_quote(service_domain)}",
        "  infrastructureRef:",
        "    apiVersion: infrastructure.cluster.x-k8s.io/v1beta1",
        "    kind: Metal3Cluster",
        f"    name: {cluster_name}",
        "  controlPlaneRef:",
        "    apiVersion: controlplane.cluster.x-k8s.io/v1alpha3",
        "    kind: TalosControlPlane",
        f"    name: {control_plane_name}",
        "---",
        "apiVersion: infrastructure.cluster.x-k8s.io/v1beta1",
        "kind: Metal3Cluster",
        "metadata:",
        f"  name: {cluster_name}",
        f"  namespace: {namespace}",
        "spec:",
        "  controlPlaneEndpoint:",
        f"    host: {yaml_quote(endpoint_host)}",
        f"    port: {endpoint_port}",
        "  cloudProviderEnabled: false",
    ]
    return "\n".join(lines) + "\n"


def render_talos_control_plane(*, namespace: str, cluster_name: str, replicas: int, capi: dict, patches: list[tuple[str, str]]) -> str:
    k8s_version = require_str(capi, "kubernetes_version", context="capi")
    talos_version = require_str(capi, "talos_version", context="capi")
    lines = [
        "apiVersion: controlplane.cluster.x-k8s.io/v1alpha3",
        "kind: TalosControlPlane",
        "metadata:",
        f"  name: {cluster_name}-controlplane",
        f"  namespace: {namespace}",
        "spec:",
        f"  replicas: {replicas}",
        f"  version: {yaml_quote(k8s_version)}",
        "  infrastructureTemplate:",
        "    apiVersion: infrastructure.cluster.x-k8s.io/v1beta1",
        "    kind: Metal3MachineTemplate",
        f"    name: {cluster_name}-controlplane",
        "  controlPlaneConfig:",
        "    controlplane:",
        "      generateType: controlplane",
        f"      talosVersion: {yaml_quote(talos_version)}",
        "      hostname:",
        "        source: InfrastructureName",
        *patch_list_block(patches, 6),
        "  rolloutStrategy:",
        "    type: RollingUpdate",
        "    rollingUpdate:",
        "      maxSurge: 1",
    ]
    return "\n".join(lines) + "\n"


def render_worker_bootstrap(*, namespace: str, cluster_name: str, capi: dict, patches: list[tuple[str, str]]) -> str:
    talos_version = require_str(capi, "talos_version", context="capi")
    lines = [
        "apiVersion: bootstrap.cluster.x-k8s.io/v1alpha3",
        "kind: TalosConfigTemplate",
        "metadata:",
        f"  name: {cluster_name}-workers",
        f"  namespace: {namespace}",
        "spec:",
        "  template:",
        "    spec:",
        "      generateType: worker",
        f"      talosVersion: {yaml_quote(talos_version)}",
        "      hostname:",
        "        source: InfrastructureName",
        *patch_list_block(patches, 6),
    ]
    return "\n".join(lines) + "\n"


def render_machine_deployment(*, namespace: str, cluster_name: str, replicas: int, capi: dict) -> str:
    k8s_version = require_str(capi, "kubernetes_version", context="capi")
    drain_seconds = as_int(capi.get("node_drain_timeout_seconds"), 0, "capi.node_drain_timeout_seconds")
    lines = [
        "apiVersion: cluster.x-k8s.io/v1beta1",
        "kind: MachineDeployment",
        "metadata:",
        f"  name: {cluster_name}-workers",
        f"  namespace: {namespace}",
        "  labels:",
        f"    cluster.x-k8s.io/cluster-name: {cluster_name}",
        "    nodepool: workers",
        "spec:",
        f"  clusterName: {cluster_name}",
        f"  replicas: {replicas}",
        "  selector:",
        "    matchLabels:",
        f"      cluster.x-k8s.io/cluster-name: {cluster_name}",
        "      nodepool: workers",
        "  template:",
        "    metadata:",
        "      labels:",
        f"        cluster.x-k8s.io/cluster-name: {cluster_name}",
        "        nodepool: workers",
        "    spec:",
        f"      clusterName: {cluster_name}",
        f"      version: {yaml_quote(k8s_version)}",
    ]
    if drain_seconds:
        lines.extend([
            f"      nodeDrainTimeout: {yaml_quote(str(drain_seconds) + 's')}",
        ])
    lines.extend([
        "      bootstrap:",
        "        configRef:",
        "          apiVersion: bootstrap.cluster.x-k8s.io/v1alpha3",
        "          kind: TalosConfigTemplate",
        f"          name: {cluster_name}-workers",
        "      infrastructureRef:",
        "        apiVersion: infrastructure.cluster.x-k8s.io/v1beta1",
        "        kind: Metal3MachineTemplate",
        f"        name: {cluster_name}-workers",
    ])
    return "\n".join(lines) + "\n"


def render_manifest(inventory: dict, *, cluster_patch_path: Path, control_plane_patch_path: Path, worker_patch_path: Path) -> str:
    cluster_name = validate_name(require_str(inventory, "cluster_name"), "cluster_name")
    endpoint = require_str(inventory, "control_plane_endpoint")
    endpoint_host, endpoint_port = endpoint_parts(endpoint)
    defaults = inventory.get("node_defaults", {})
    if not isinstance(defaults, dict):
        fail("node_defaults must be an object when present")
    control_planes = require_list(inventory, "control_planes")
    workers = require_list(inventory, "workers")
    if len(control_planes) != 3:
        fail(f"on-prem CAPI/Metal3 control plane requires exactly 3 nodes; got {len(control_planes)}")
    capi = get_capi(inventory)
    namespace = validate_name(optional_str(capi, "namespace", cluster_name), "capi.namespace")
    image = get_image(capi)
    cluster_patch = read_patch(cluster_patch_path)
    cp_patch = read_patch(control_plane_patch_path)
    worker_patch = read_patch(worker_patch_path)
    dynamic_patch = generated_cluster_patch(inventory)
    cp_patches = [
        (cluster_patch_path.name, cluster_patch),
        ("inventory-generated-endpoint-and-sans.patch.yaml", dynamic_patch),
        (control_plane_patch_path.name, cp_patch),
    ]
    worker_patches = [
        (cluster_patch_path.name, cluster_patch),
        ("inventory-generated-endpoint-and-sans.patch.yaml", dynamic_patch),
        (worker_patch_path.name, worker_patch),
    ]

    docs = [
        "# Generated by deploy/talos/on-prem/render-capi-metal3.py from nodes.example.json.",
        "# DARK template only: review/apply in an on-prem management cluster after installing CAPI, CABPT/CACPPT, and CAPM3.",
        "# Do not commit BMC credential Secret data or generated Talos secrets/talosconfig.",
        "apiVersion: v1",
        "kind: Namespace",
        "metadata:",
        f"  name: {namespace}",
        "---",
        render_cluster(namespace=namespace, cluster_name=cluster_name, capi=capi, endpoint_host=endpoint_host, endpoint_port=endpoint_port).rstrip(),
        "---",
        render_talos_control_plane(namespace=namespace, cluster_name=cluster_name, replicas=len(control_planes), capi=capi, patches=cp_patches).rstrip(),
        "---",
        render_machine_template(f"{cluster_name}-controlplane", namespace=namespace, role="control-plane", cluster_name=cluster_name, image=image, data_template=f"{cluster_name}-controlplane-template", capi=capi).rstrip(),
        "---",
        render_network_data_template(f"{cluster_name}-controlplane-template", namespace=namespace, cluster_name=cluster_name, defaults=defaults).rstrip(),
        "---",
        render_machine_deployment(namespace=namespace, cluster_name=cluster_name, replicas=len(workers), capi=capi).rstrip(),
        "---",
        render_worker_bootstrap(namespace=namespace, cluster_name=cluster_name, capi=capi, patches=worker_patches).rstrip(),
        "---",
        render_machine_template(f"{cluster_name}-workers", namespace=namespace, role="worker", cluster_name=cluster_name, image=image, data_template=f"{cluster_name}-workers-template", capi=capi).rstrip(),
        "---",
        render_network_data_template(f"{cluster_name}-workers-template", namespace=namespace, cluster_name=cluster_name, defaults=defaults).rstrip(),
    ]

    for node in control_planes:
        docs.extend(["---", render_baremetal_host(node, role="control-plane", namespace=namespace, cluster_name=cluster_name, defaults=defaults).rstrip()])
    for node in workers:
        docs.extend(["---", render_baremetal_host(node, role="worker", namespace=namespace, cluster_name=cluster_name, defaults=defaults).rstrip()])

    output = "\n".join(docs).rstrip() + "\n"
    if "allowSchedulingOnControlPlanes: true" in output:
        fail("rendered CAPI/Metal3 manifest unexpectedly enables control-plane scheduling")
    return output


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inventory", type=Path, default=DEFAULT_INVENTORY)
    parser.add_argument("--cluster-patch", type=Path, default=DEFAULT_CLUSTER_PATCH)
    parser.add_argument("--control-plane-patch", type=Path, default=DEFAULT_CONTROL_PLANE_PATCH)
    parser.add_argument("--worker-patch", type=Path, default=DEFAULT_WORKER_PATCH)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT, help="write rendered manifest here; use '-' for stdout")
    parser.add_argument("--check", action="store_true", help="fail if --output exists and differs from a fresh render")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    inventory = load_json(args.inventory)
    manifest = render_manifest(
        inventory,
        cluster_patch_path=args.cluster_patch,
        control_plane_patch_path=args.control_plane_patch,
        worker_patch_path=args.worker_patch,
    )
    if str(args.output) == "-":
        sys.stdout.write(manifest)
        return 0
    if args.check:
        try:
            current = args.output.read_text(encoding="utf-8")
        except OSError as exc:
            fail(f"cannot read {args.output} for --check: {exc}")
        if current != manifest:
            fail(f"{args.output} is stale; rerun render-capi-metal3.py without --check")
        print(f"{args.output} is up to date")
        return 0
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(manifest, encoding="utf-8")
    print(f"Rendered CAPI/Metal3 Talos template to {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
