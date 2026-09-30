#!/usr/bin/env bash
# Prove NetworkPolicy isolation at two layers:
#
#   Phase 1 (static render proof): kustomize-render the prod overlay and assert
#     the mox dark stack ships its NetworkPolicy objects, points the app at the
#     in-cluster mox webapi, exposes only internal ports, and renders no public
#     mail/admin/NodePort/LoadBalancer surface. This needs no live cluster.
#
#   Phase 2 (live enforcement preflight): read back the target cluster and prove
#     it has applied NetworkPolicies and a policy-capable CNI/enforcer before
#     operators claim network isolation. Rendering proves desired state is valid;
#     this proves the cluster can actually enforce it.
#
# This deliberately complements scripts/render-k8s-manifests.sh.
set -euo pipefail

KUBECTL="${KUBECTL:-kubectl}"
NAMESPACE="${CONSOLE_NETWORKPOLICY_NAMESPACE:-console}"
MODE="${CONSOLE_NETWORKPOLICY_PREFLIGHT:-warn}"
EXPECTED_ENFORCER="${CONSOLE_NETWORKPOLICY_EXPECTED_ENFORCER:-auto}"
CILIUM_NAMESPACE="${CONSOLE_NETWORKPOLICY_CILIUM_NAMESPACE:-kube-system}"

usage() {
  cat <<'USAGE'
Usage: scripts/check-networkpolicy-enforcement.sh

Phase 1 always runs a static kustomize-render proof of the prod overlay.
Phase 2 reads back live NetworkPolicy enforcement prerequisites.

Environment:
  CONSOLE_NETWORKPOLICY_PREFLIGHT=warn|require|off
      warn    (default): print a warning and exit 0 when no live cluster/CNI proof
               is available. Suitable for generic CI where manifests still render.
      require: fail when kubectl cannot reach the target cluster, the namespace or
               NetworkPolicies are absent, or no policy-capable CNI is detected.
      off:     skip the live readback entirely (the static render proof still runs).
  CONSOLE_NETWORKPOLICY_NAMESPACE=console
      Namespace that must contain applied NetworkPolicy objects. Defaults to the
      namespace the prod overlay renders (deploy/apps/console/base sets it).
  CONSOLE_NETWORKPOLICY_EXPECTED_ENFORCER=auto|cilium|calico|canal|antrea|kube-router|ovn-kubernetes
      Optional expected CNI/policy enforcer. "auto" accepts any recognized
      NetworkPolicy-capable enforcer and rejects plain flannel-only clusters.
  CONSOLE_NETWORKPOLICY_CILIUM_NAMESPACE=kube-system
      Namespace that must contain the Cilium DaemonSet/config when the expected
      enforcer is cilium.
  KUBECTL=kubectl
      kubectl binary to use.
USAGE
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

case "${MODE}" in
  warn|warning)
    MODE="warn"
    ;;
  require|required|fail|strict)
    MODE="require"
    ;;
  off|skip|disabled)
    MODE="off"
    ;;
  *)
    echo "networkpolicy-preflight: invalid CONSOLE_NETWORKPOLICY_PREFLIGHT=${MODE}; expected warn, require, or off" >&2
    exit 2
    ;;
esac

case "${EXPECTED_ENFORCER}" in
  auto|cilium|calico|canal|antrea|kube-router|ovn-kubernetes)
    ;;
  *)
    echo "networkpolicy-preflight: invalid CONSOLE_NETWORKPOLICY_EXPECTED_ENFORCER=${EXPECTED_ENFORCER}" >&2
    exit 2
    ;;
esac

# ---------------------------------------------------------------------------
# Phase 1: static render proof (kustomize build of the prod overlay).
# ---------------------------------------------------------------------------
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RENDER_CMD=()
if command -v kustomize >/dev/null 2>&1; then
  RENDER_CMD=(kustomize build)
elif command -v "${KUBECTL}" >/dev/null 2>&1; then
  # Same renderer scripts/render-k8s-manifests.sh uses; CI installs kubectl only.
  RENDER_CMD=("${KUBECTL}" kustomize)
fi

if [[ "${#RENDER_CMD[@]}" -eq 0 ]]; then
  # A missing renderer means the assertions below never ran: an unproven gate,
  # not a passing one. Only an interactive dev box is allowed to degrade.
  if [[ -n "${CI:-}" ]]; then
    echo "networkpolicy-static: FAIL: neither kustomize nor ${KUBECTL} found; the static NetworkPolicy proof did not run." >&2
    echo "networkpolicy-static: CI must install a kustomize renderer before this gate; a skipped proof is not a passing proof." >&2
    exit 1
  fi
  echo "networkpolicy-static: WARNING: no kustomize renderer found; skipping static render proof (local run; this FAILS in CI)." >&2
  echo "networkpolicy-static: install kustomize or kubectl for the static NetworkPolicy proof." >&2
else
  RENDER="$(mktemp)"
  trap 'rm -f "$RENDER"' EXIT
  failures=0

  "${RENDER_CMD[@]}" "$ROOT/deploy/apps/console/overlays/prod" > "$RENDER"

  require() {
    local needle="$1"
    local label="$2"
    if grep -Fq "$needle" "$RENDER"; then
      printf 'PASS  %s\n' "$label"
    else
      printf 'FAIL  %s (missing %q)\n' "$label" "$needle" >&2
      failures=$((failures + 1))
    fi
  }

  reject() {
    local needle="$1"
    local label="$2"
    if grep -Fq "$needle" "$RENDER"; then
      printf 'FAIL  %s (found forbidden %q)\n' "$label" "$needle" >&2
      failures=$((failures + 1))
    else
      printf 'PASS  %s\n' "$label"
    fi
  }

  require 'kind: StatefulSet' 'render includes workload objects'
  require 'name: console-mox' 'render includes console-mox resources'
  require 'name: allow-app-egress-mox' 'app/worker egress to mox is explicitly allowed'
  require 'name: allow-mox-ingress-internal' 'mox ingress is restricted to app/worker and monitoring'
  require 'name: default-deny-egress-mox' 'mox egress is default-denied'
  require 'name: allow-mox-egress-app-webhook' 'mox can reach the internal app webhook only'
  require 'CONSOLE_MAIL_MOX_BASE_URL: http://console-mox.console.svc:1080' 'app config points at the in-cluster mox webapi'
  require 'port: 1080' 'webapi port is present for internal service traffic'
  require 'port: 1143' 'IMAP port is present for internal service traffic'
  require 'port: 8010' 'metrics port is present for observability'
  reject 'type: NodePort' 'no NodePort services are rendered'
  reject 'type: LoadBalancer' 'no LoadBalancer services are rendered'
  reject 'containerPort: 25' 'public SMTP/MX port 25 is not enabled'
  reject 'containerPort: 587' 'public submission port 587 is not enabled'
  reject 'containerPort: 993' 'public IMAPS port 993 is not enabled'
  reject 'hostPort:' 'mox does not bind host ports'

  if (( failures > 0 )); then
    printf '\nStatic NetworkPolicy proof failed (%d checks). See FAIL lines above.\n' "$failures" >&2
    exit 1
  fi

  cat <<'MSG'

Static NetworkPolicy proof passed. To prove runtime enforcement on a cluster,
first confirm the CNI enforces NetworkPolicy (plain flannel does not), then run
operator-owned smoke pods with these labels:
  allowed:   app=console-app    -> http://console-mox.console.svc:1080/webapi/v0/
  denied:    app=console-deny   -> same URL must time out/fail
  webhook:   app=console-mox    -> http://console-app.console.svc:8080/readyz allowed
Keep public MX disabled until that live CNI smoke is recorded in the runbook.
MSG
fi

# ---------------------------------------------------------------------------
# Phase 2: live cluster NetworkPolicy enforcement preflight.
# ---------------------------------------------------------------------------
if [[ "${MODE}" == "off" ]]; then
  echo "networkpolicy-preflight: live readback skipped (CONSOLE_NETWORKPOLICY_PREFLIGHT=off)"
  exit 0
fi

finish_unavailable() {
  local message="$1"
  if [[ "${MODE}" == "require" ]]; then
    echo "networkpolicy-preflight: FAIL: ${message}" >&2
    exit 1
  fi
  echo "networkpolicy-preflight: WARNING: ${message}" >&2
  echo "networkpolicy-preflight: CI/render validation continues, but this is NOT live NetworkPolicy enforcement proof." >&2
  exit 0
}

require_output() {
  local description="$1"
  shift
  local output
  if ! output="$("$@" 2>/dev/null)"; then
    finish_unavailable "could not ${description}; check kubeconfig, RBAC, and target cluster context"
  fi
  printf '%s' "${output}"
}

if ! command -v "${KUBECTL}" >/dev/null 2>&1; then
  finish_unavailable "${KUBECTL} not found; cannot read cluster/CNI NetworkPolicy capability"
fi

context="$(require_output "read kubectl current-context" "${KUBECTL}" config current-context)"
if [[ -z "${context}" ]]; then
  finish_unavailable "kubectl has no current context"
fi

api_resources="$(require_output "read Kubernetes networking.k8s.io API resources" "${KUBECTL}" api-resources --api-group=networking.k8s.io --no-headers)"
if ! grep -Eq '(^|[[:space:]])networkpolicies([[:space:]]|$)' <<<"${api_resources}"; then
  finish_unavailable "cluster ${context} does not advertise networking.k8s.io NetworkPolicy resources"
fi

if ! "${KUBECTL}" get namespace "${NAMESPACE}" >/dev/null 2>&1; then
  finish_unavailable "namespace ${NAMESPACE} is not readable in cluster ${context}; applied NetworkPolicies cannot be confirmed"
fi

policies="$(require_output "read NetworkPolicies in namespace ${NAMESPACE}" "${KUBECTL}" -n "${NAMESPACE}" get networkpolicy -o name --ignore-not-found)"
if [[ -z "${policies}" ]]; then
  finish_unavailable "namespace ${NAMESPACE} has no applied NetworkPolicy objects in cluster ${context}"
fi

resources="$(require_output "list CNI controller/agent workloads" "${KUBECTL}" get pods,daemonsets,deployments -A -o jsonpath='{range .items[*]}{.kind} {.metadata.namespace}/{.metadata.name}{"\n"}{end}')"
crds="$(${KUBECTL} get crd -o name 2>/dev/null || true)"
snapshot="$(printf '%s\n%s\n' "${resources}" "${crds}" | tr '[:upper:]' '[:lower:]')"

declare -a enforcers=()

if grep -Eq '(^|[[:space:]/-])cilium([[:space:]/-]|$)|cilium\.io|ciliumnetworkpolic' <<<"${snapshot}"; then
  enforcers+=("cilium")
fi
if grep -Eq 'calico-node|tigera-operator|projectcalico\.org|calico\.org|/calico($|[[:space:]-])|(^|[[:space:]/-])canal([[:space:]/-]|$)' <<<"${snapshot}"; then
  enforcers+=("calico")
fi
if grep -Eq 'antrea-agent|antrea-controller|antrea\.io|antrea\.tanzu\.vmware\.com' <<<"${snapshot}"; then
  enforcers+=("antrea")
fi
if grep -Eq 'kube-router' <<<"${snapshot}"; then
  enforcers+=("kube-router")
fi
if grep -Eq 'ovn-kubernetes|ovnkube-node|openshift-ovn-kubernetes' <<<"${snapshot}"; then
  enforcers+=("ovn-kubernetes")
fi

flannel_detected=0
if grep -Eq 'kube-flannel|/flannel($|[[:space:]-])|flannel\.cni|(^|[[:space:]/-])canal([[:space:]/-]|$)' <<<"${snapshot}"; then
  flannel_detected=1
fi
flannel_note=""
if [[ "${flannel_detected}" -eq 1 ]]; then
  flannel_note=" (flannel present)"
fi

expected_found=0
if [[ "${EXPECTED_ENFORCER}" == "auto" ]]; then
  if [[ "${#enforcers[@]}" -eq 0 ]]; then
    if [[ "${flannel_detected}" -eq 1 ]]; then
      finish_unavailable "cluster ${context} appears to run flannel without a detected policy engine; plain flannel does not enforce NetworkPolicy"
    fi
    finish_unavailable "no recognized policy-capable CNI/enforcer found in cluster ${context}; checked Cilium, Calico/Canal, Antrea, kube-router, and OVN-Kubernetes"
  fi
else
  for enforcer in "${enforcers[@]}"; do
    if [[ "${EXPECTED_ENFORCER}" == "${enforcer}" ]]; then
      expected_found=1
    fi
    if [[ "${EXPECTED_ENFORCER}" == "canal" && "${enforcer}" == "calico" && "${flannel_detected}" -eq 1 ]]; then
      expected_found=1
    fi
  done
  if [[ "${expected_found}" -ne 1 ]]; then
    finish_unavailable "expected ${EXPECTED_ENFORCER} NetworkPolicy enforcer was not detected in cluster ${context}; detected: ${enforcers[*]:-none}${flannel_note}"
  fi
fi

cilium_ready_note=""
if [[ "${EXPECTED_ENFORCER}" == "cilium" ]]; then
  cilium_status="$("${KUBECTL}" -n "${CILIUM_NAMESPACE}" get daemonset cilium -o jsonpath='{.status.desiredNumberScheduled} {.status.numberReady} {.status.updatedNumberScheduled}' 2>/dev/null || true)"
  if [[ -z "${cilium_status}" ]]; then
    finish_unavailable "expected cilium, but daemonset ${CILIUM_NAMESPACE}/cilium is not readable; Cilium live status cannot be proven"
  fi

  read -r cilium_desired cilium_ready cilium_updated <<<"${cilium_status}"
  cilium_desired="${cilium_desired:-0}"
  cilium_ready="${cilium_ready:-0}"
  cilium_updated="${cilium_updated:-0}"
  if [[ ! "${cilium_desired}" =~ ^[0-9]+$ || ! "${cilium_ready}" =~ ^[0-9]+$ || ! "${cilium_updated}" =~ ^[0-9]+$ ]]; then
    finish_unavailable "could not parse Cilium DaemonSet readiness in ${CILIUM_NAMESPACE}: '${cilium_status}'"
  fi
  if (( cilium_desired < 1 || cilium_ready < cilium_desired || cilium_updated < cilium_desired )); then
    finish_unavailable "Cilium DaemonSet is not fully ready in ${CILIUM_NAMESPACE}: desired=${cilium_desired} ready=${cilium_ready} updated=${cilium_updated}"
  fi

  cilium_config="$("${KUBECTL}" -n "${CILIUM_NAMESPACE}" get configmap cilium-config -o jsonpath='{.data.enable-policy} {.data.enable-k8s-networkpolicy}' 2>/dev/null || true)"
  if [[ -z "${cilium_config}" ]]; then
    finish_unavailable "expected cilium, but configmap ${CILIUM_NAMESPACE}/cilium-config is not readable; Cilium NetworkPolicy settings cannot be proven"
  fi
  read -r cilium_enable_policy cilium_enable_k8s_np <<<"${cilium_config}"
  if [[ "${cilium_enable_policy}" == "never" || "${cilium_enable_k8s_np}" == "false" ]]; then
    finish_unavailable "Cilium is live but Kubernetes NetworkPolicy enforcement appears disabled: enable-policy=${cilium_enable_policy:-unknown} enable-k8s-networkpolicy=${cilium_enable_k8s_np:-unknown}"
  fi
  cilium_ready_note=" cilium_daemonset=${cilium_ready}/${cilium_desired} cilium_enable_policy=${cilium_enable_policy:-unknown} cilium_k8s_networkpolicy=${cilium_enable_k8s_np:-unknown}"
fi

policy_count="$(grep -c '^' <<<"${policies}")"
echo "networkpolicy-preflight: PASS"
echo "networkpolicy-preflight: context=${context} namespace=${NAMESPACE} policies=${policy_count} detected_enforcers=${enforcers[*]} expected=${EXPECTED_ENFORCER}${cilium_ready_note}"
echo "networkpolicy-preflight: live cluster has applied NetworkPolicies and a recognized policy-capable CNI/enforcer."
