#!/usr/bin/env bash
# Live denied-traffic smoke for deploy/apps/console/base/networkpolicy.yaml.
#
# The smoke creates temporary pods in the target namespace:
# - an app=console-app HTTP target on port 8080, which the checked-in ingress policy
#   allows from same-namespace pods;
# - an unlabeled control client, which should reach that target; and
# - an app=console-app policy client, selected by default-deny-egress-app-tier, which
#   should resolve DNS, reach outbound HTTPS, optionally reach CNPG Postgres on
#   TCP/5432 when the database Service exists, and be denied when it tries to
#   reach the non-allowed same-namespace HTTP target on TCP/8080.
#
# A pass is live packet evidence that the target cluster enforces the app-tier
# egress deny. It is intentionally separate from manifest rendering and refuses to
# run on clusters that do not pass the policy-capable CNI preflight.
set -euo pipefail

KUBECTL="${KUBECTL:-kubectl}"
NAMESPACE="${CONSOLE_NETWORKPOLICY_NAMESPACE:-maintenance}"
EXPECTED_ENFORCER="${CONSOLE_NETWORKPOLICY_EXPECTED_ENFORCER:-auto}"
CLIENT_IMAGE="${CONSOLE_NETWORKPOLICY_SMOKE_CLIENT_IMAGE:-curlimages/curl:8.11.1}"
TARGET_IMAGE="${CONSOLE_NETWORKPOLICY_SMOKE_TARGET_IMAGE:-nginxinc/nginx-unprivileged:1.27-alpine}"
POSTGRES_CLIENT_IMAGE="${CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES_CLIENT_IMAGE:-postgres:18-alpine}"
RUN_ID="${CONSOLE_NETWORKPOLICY_SMOKE_RUN_ID:-$(date +%s)-$$}"
NAME_PREFIX="${CONSOLE_NETWORKPOLICY_SMOKE_NAME:-console-netpol-smoke}"
WAIT_TIMEOUT="${CONSOLE_NETWORKPOLICY_SMOKE_WAIT_TIMEOUT:-90s}"
CURL_CONNECT_TIMEOUT="${CONSOLE_NETWORKPOLICY_SMOKE_CONNECT_TIMEOUT_SECONDS:-3}"
CURL_MAX_TIME="${CONSOLE_NETWORKPOLICY_SMOKE_MAX_TIME_SECONDS:-8}"
KEEP_RESOURCES="${CONSOLE_NETWORKPOLICY_SMOKE_KEEP:-0}"
DNS_NAME="${CONSOLE_NETWORKPOLICY_SMOKE_DNS_NAME:-kubernetes.default.svc.cluster.local}"
HTTPS_URL="${CONSOLE_NETWORKPOLICY_SMOKE_HTTPS_URL:-https://example.com/}"
POSTGRES_CHECK="${CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES:-auto}"
POSTGRES_SERVICE="${CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES_SERVICE:-console-db-rw}"
POSTGRES_PORT="${CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES_PORT:-5432}"
LABEL_KEY="console.nousresearch.com/networkpolicy-smoke"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
  cat <<'USAGE'
Usage: scripts/smoke-networkpolicy-deny.sh

Runs a live denied-traffic smoke test for deploy/apps/console/base/networkpolicy.yaml.
This requires a real Kubernetes cluster where NetworkPolicy is enforced by a
policy-capable CNI such as Cilium, Calico, Canal (flannel dataplane + Calico
policy), Antrea, kube-router, or OVN-Kubernetes. Plain flannel is expected to
fail the preflight and is not valid evidence.

What the smoke proves:
  1. an unlabeled same-namespace control pod can reach a temporary app=console-app
     HTTP target on TCP/8080, proving the target Service and ingress allow path;
  2. a temporary app=console-app pod can use kube-dns and outbound HTTPS on TCP/443,
     matching allow-app-egress-dns and allow-app-egress-https;
  3. when the CloudNativePG console-db Service exists, an app=console-app Postgres client
     can reach console-db-rw:5432, matching allow-app-egress-postgres plus
     allow-postgres-from-app;
  4. that same app=console-app pod cannot reach the HTTP target on TCP/8080,
     proving default-deny-egress-app-tier denies non-allowed app-tier egress.

Environment:
  KUBECTL=kubectl
      kubectl binary to use.
  CONSOLE_NETWORKPOLICY_NAMESPACE=maintenance
      Namespace containing the applied Maintenance NetworkPolicies.
  CONSOLE_NETWORKPOLICY_EXPECTED_ENFORCER=auto|cilium|calico|canal|antrea|kube-router|ovn-kubernetes
      Expected live policy enforcer passed to check-networkpolicy-enforcement.sh.
  CONSOLE_NETWORKPOLICY_SMOKE_CLIENT_IMAGE=curlimages/curl:8.11.1
      Client image used for control and app-policy pods. Override to an internal
      registry mirror when the target cluster blocks public pulls.
  CONSOLE_NETWORKPOLICY_SMOKE_TARGET_IMAGE=nginxinc/nginx-unprivileged:1.27-alpine
      Non-root HTTP target image. Override to an approved internal mirror when needed.
  CONSOLE_NETWORKPOLICY_SMOKE_DNS_NAME=kubernetes.default.svc.cluster.local
      Name resolved from the app-labelled client to prove DNS egress is allowed.
  CONSOLE_NETWORKPOLICY_SMOKE_HTTPS_URL=https://example.com/
      HTTPS URL fetched from the app-labelled client to prove outbound TCP/443 is
      allowed. Override to an approved reachable HTTPS endpoint if egress is
      restricted to a site proxy or private mirror.
  CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES=auto|required|skip
      auto (default) runs the Postgres TCP check when the Service exists and skips
      it otherwise; required fails when the Service is missing; skip disables it.
  CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES_SERVICE=console-db-rw
      CloudNativePG read/write Service used for the intra-namespace Postgres check.
  CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES_CLIENT_IMAGE=postgres:18-alpine
      Client image used for pg_isready; override to an internal registry mirror.
  CONSOLE_NETWORKPOLICY_SMOKE_NAME=console-netpol-smoke
      DNS-label-safe resource name prefix. The script appends a unique run id.
  CONSOLE_NETWORKPOLICY_SMOKE_RUN_ID=<dns-label-suffix>
      Optional lowercase run suffix for reproducibility; default is timestamp-pid.
  CONSOLE_NETWORKPOLICY_SMOKE_KEEP=1
      Keep temporary pods/service for debugging instead of cleaning them up.

Pass/fail interpretation:
  PASS means the selected cluster/context has applied the expected policies, a
  policy-capable CNI was detected, the control connection succeeded, and the
  app-labelled client could use the allowed DNS/HTTPS/Postgres paths while the
  denied TCP/8080 connection failed as expected.
  FAIL before pod creation usually means wrong context, namespace/RBAC, missing
  policies, or plain/non-policy CNI. FAIL after the control connection usually
  means the CNI did not enforce the expected allow/deny set, the configured DNS /
  HTTPS / Postgres probe is not reachable, or the checked-in policy selectors no
  longer match the smoke assumptions.
USAGE
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

fail() {
  echo "networkpolicy-deny-smoke: FAIL: $*" >&2
  exit 1
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    fail "required tool '$1' not found"
  fi
}

validate_dns_label() {
  local label_name="$1"
  local label_value="$2"
  if [[ ! "${label_value}" =~ ^[a-z0-9]([-a-z0-9]*[a-z0-9])?$ ]]; then
    fail "${label_name} must be a lowercase DNS label fragment, got '${label_value}'"
  fi
}

validate_positive_integer() {
  local name="$1"
  local value="$2"
  if [[ ! "${value}" =~ ^[0-9]+$ ]]; then
    fail "${name} must be a positive integer, got '${value}'"
  fi
  if (( value < 1 )); then
    fail "${name} must be a positive integer, got '${value}'"
  fi
}

require_command "${KUBECTL}"
validate_dns_label "CONSOLE_NETWORKPOLICY_SMOKE_NAME" "${NAME_PREFIX}"
validate_dns_label "CONSOLE_NETWORKPOLICY_SMOKE_RUN_ID" "${RUN_ID}"
validate_positive_integer "CONSOLE_NETWORKPOLICY_SMOKE_CONNECT_TIMEOUT_SECONDS" "${CURL_CONNECT_TIMEOUT}"
validate_positive_integer "CONSOLE_NETWORKPOLICY_SMOKE_MAX_TIME_SECONDS" "${CURL_MAX_TIME}"
validate_positive_integer "CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES_PORT" "${POSTGRES_PORT}"

case "${POSTGRES_CHECK}" in
  auto|required|skip)
    ;;
  *)
    fail "invalid CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES=${POSTGRES_CHECK}; expected auto, required, or skip"
    ;;
esac

BASE_NAME="${NAME_PREFIX}-${RUN_ID}"
if (( ${#BASE_NAME} > 54 )); then
  fail "resource base name '${BASE_NAME}' is too long; keep prefix+run id <= 54 chars"
fi

TARGET_POD="${BASE_NAME}-target"
CONTROL_POD="${BASE_NAME}-control"
APP_CLIENT_POD="${BASE_NAME}-app"
POSTGRES_CLIENT_POD="${BASE_NAME}-pg"
TARGET_SERVICE="${BASE_NAME}-target"
TARGET_URL="http://${TARGET_SERVICE}.${NAMESPACE}.svc.cluster.local:8080/"
MANIFEST="$(mktemp "${TMPDIR:-/tmp}/console-networkpolicy-smoke.XXXXXX.yaml")"
CREATED=0

cleanup() {
  local ec=$?
  if [[ "${CREATED}" -eq 1 && "${KEEP_RESOURCES}" != "1" ]]; then
    "${KUBECTL}" -n "${NAMESPACE}" delete pod,service \
      -l "${LABEL_KEY}=${RUN_ID}" \
      --ignore-not-found=true --wait=false >/dev/null 2>&1 || true
  elif [[ "${CREATED}" -eq 1 ]]; then
    echo "networkpolicy-deny-smoke: kept temporary resources with selector ${LABEL_KEY}=${RUN_ID}" >&2
  fi
  rm -f "${MANIFEST}"
  exit "${ec}"
}
trap cleanup EXIT

case "${EXPECTED_ENFORCER}" in
  auto|cilium|calico|canal|antrea|kube-router|ovn-kubernetes)
    ;;
  *)
    fail "invalid CONSOLE_NETWORKPOLICY_EXPECTED_ENFORCER=${EXPECTED_ENFORCER}"
    ;;
esac

context="$(${KUBECTL} config current-context 2>/dev/null || true)"
if [[ -z "${context}" ]]; then
  fail "kubectl has no current context"
fi

echo "networkpolicy-deny-smoke: context=${context} namespace=${NAMESPACE} expected_enforcer=${EXPECTED_ENFORCER}"

echo "networkpolicy-deny-smoke: running policy-capable CNI/readback preflight"
KUBECTL="${KUBECTL}" \
CONSOLE_NETWORKPOLICY_NAMESPACE="${NAMESPACE}" \
CONSOLE_NETWORKPOLICY_PREFLIGHT=require \
CONSOLE_NETWORKPOLICY_EXPECTED_ENFORCER="${EXPECTED_ENFORCER}" \
  "${SCRIPT_DIR}/check-networkpolicy-enforcement.sh"

required_policies=(
  default-deny-ingress
  allow-web-tier-from-traefik
  allow-postgres-from-app
  default-deny-egress-app-tier
  allow-app-egress-dns
  allow-app-egress-postgres
  allow-app-egress-https
)
for policy in "${required_policies[@]}"; do
  if ! "${KUBECTL}" -n "${NAMESPACE}" get networkpolicy "${policy}" >/dev/null 2>&1; then
    fail "missing required NetworkPolicy ${NAMESPACE}/${policy}; apply deploy/apps/console/base/networkpolicy.yaml before running the smoke"
  fi
done

POSTGRES_AVAILABLE=0
if [[ "${POSTGRES_CHECK}" != "skip" ]]; then
  if "${KUBECTL}" -n "${NAMESPACE}" get service "${POSTGRES_SERVICE}" >/dev/null 2>&1; then
    POSTGRES_AVAILABLE=1
  elif [[ "${POSTGRES_CHECK}" == "required" ]]; then
    fail "missing Postgres Service ${NAMESPACE}/${POSTGRES_SERVICE}; set CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES=skip only when the database is intentionally absent"
  else
    echo "networkpolicy-deny-smoke: skipping Postgres path; Service ${NAMESPACE}/${POSTGRES_SERVICE} not found (set CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES=required to fail closed)"
  fi
fi

cat >"${MANIFEST}" <<YAML
apiVersion: v1
kind: Pod
metadata:
  name: ${TARGET_POD}
  namespace: ${NAMESPACE}
  labels:
    app: console-app
    ${LABEL_KEY}: ${RUN_ID}
spec:
  restartPolicy: Never
  automountServiceAccountToken: false
  securityContext:
    runAsNonRoot: true
    seccompProfile:
      type: RuntimeDefault
  containers:
    - name: http
      image: ${TARGET_IMAGE}
      imagePullPolicy: IfNotPresent
      ports:
        - name: http
          containerPort: 8080
          protocol: TCP
      securityContext:
        runAsUser: 101
        runAsGroup: 101
        allowPrivilegeEscalation: false
        capabilities:
          drop: ["ALL"]
---
apiVersion: v1
kind: Service
metadata:
  name: ${TARGET_SERVICE}
  namespace: ${NAMESPACE}
  labels:
    ${LABEL_KEY}: ${RUN_ID}
spec:
  selector:
    app: console-app
    ${LABEL_KEY}: ${RUN_ID}
  ports:
    - name: http
      port: 8080
      targetPort: http
      protocol: TCP
---
apiVersion: v1
kind: Pod
metadata:
  name: ${CONTROL_POD}
  namespace: ${NAMESPACE}
  labels:
    ${LABEL_KEY}: ${RUN_ID}
spec:
  restartPolicy: Never
  automountServiceAccountToken: false
  securityContext:
    runAsNonRoot: true
    seccompProfile:
      type: RuntimeDefault
  containers:
    - name: curl
      image: ${CLIENT_IMAGE}
      imagePullPolicy: IfNotPresent
      command: ["sh", "-c", "sleep 3600"]
      securityContext:
        runAsUser: 1000
        runAsGroup: 1000
        allowPrivilegeEscalation: false
        capabilities:
          drop: ["ALL"]
---
apiVersion: v1
kind: Pod
metadata:
  name: ${APP_CLIENT_POD}
  namespace: ${NAMESPACE}
  labels:
    app: console-app
    ${LABEL_KEY}: ${RUN_ID}
spec:
  restartPolicy: Never
  automountServiceAccountToken: false
  securityContext:
    runAsNonRoot: true
    seccompProfile:
      type: RuntimeDefault
  containers:
    - name: curl
      image: ${CLIENT_IMAGE}
      imagePullPolicy: IfNotPresent
      command: ["sh", "-c", "sleep 3600"]
      securityContext:
        runAsUser: 1000
        runAsGroup: 1000
        allowPrivilegeEscalation: false
        capabilities:
          drop: ["ALL"]
YAML

if [[ "${POSTGRES_AVAILABLE}" -eq 1 ]]; then
  cat >>"${MANIFEST}" <<YAML
---
apiVersion: v1
kind: Pod
metadata:
  name: ${POSTGRES_CLIENT_POD}
  namespace: ${NAMESPACE}
  labels:
    app: console-app
    ${LABEL_KEY}: ${RUN_ID}
spec:
  restartPolicy: Never
  automountServiceAccountToken: false
  securityContext:
    runAsNonRoot: true
    seccompProfile:
      type: RuntimeDefault
  containers:
    - name: pg
      image: ${POSTGRES_CLIENT_IMAGE}
      imagePullPolicy: IfNotPresent
      command: ["sh", "-c", "sleep 3600"]
      securityContext:
        runAsUser: 999
        runAsGroup: 999
        allowPrivilegeEscalation: false
        capabilities:
          drop: ["ALL"]
YAML
fi

"${KUBECTL}" apply -f "${MANIFEST}"
CREATED=1

"${KUBECTL}" -n "${NAMESPACE}" wait --for=condition=Ready "pod/${TARGET_POD}" --timeout="${WAIT_TIMEOUT}"
"${KUBECTL}" -n "${NAMESPACE}" wait --for=condition=Ready "pod/${CONTROL_POD}" --timeout="${WAIT_TIMEOUT}"
"${KUBECTL}" -n "${NAMESPACE}" wait --for=condition=Ready "pod/${APP_CLIENT_POD}" --timeout="${WAIT_TIMEOUT}"
if [[ "${POSTGRES_AVAILABLE}" -eq 1 ]]; then
  "${KUBECTL}" -n "${NAMESPACE}" wait --for=condition=Ready "pod/${POSTGRES_CLIENT_POD}" --timeout="${WAIT_TIMEOUT}"
fi

run_curl() {
  local pod="$1"
  local url="${2:-${TARGET_URL}}"
  "${KUBECTL}" -n "${NAMESPACE}" exec "${pod}" -- \
    curl -fsS --connect-timeout "${CURL_CONNECT_TIMEOUT}" --max-time "${CURL_MAX_TIME}" "${url}"
}

run_dns_lookup() {
  "${KUBECTL}" -n "${NAMESPACE}" exec "${APP_CLIENT_POD}" -- \
    sh -c '
      name="$1"
      if command -v nslookup >/dev/null 2>&1; then
        nslookup "$name"
      elif command -v getent >/dev/null 2>&1; then
        getent hosts "$name"
      else
        echo "client image lacks nslookup/getent; override CONSOLE_NETWORKPOLICY_SMOKE_CLIENT_IMAGE" >&2
        exit 127
      fi
    ' sh "${DNS_NAME}"
}

echo "networkpolicy-deny-smoke: checking allowed control path ${CONTROL_POD} -> ${TARGET_SERVICE}:8080"
if ! control_output="$(run_curl "${CONTROL_POD}" "${TARGET_URL}" 2>&1)"; then
  fail "control pod could not reach ${TARGET_URL}; this does not prove deny behavior. Output: ${control_output}"
fi
echo "networkpolicy-deny-smoke: control path PASS (${#control_output} bytes from target)"

echo "networkpolicy-deny-smoke: checking allowed DNS path ${APP_CLIENT_POD} (app=console-app) -> ${DNS_NAME}"
if ! dns_output="$(run_dns_lookup 2>&1)"; then
  fail "app-labelled pod could not resolve ${DNS_NAME}; allow-app-egress-dns is not proven. Output: ${dns_output}"
fi
echo "networkpolicy-deny-smoke: DNS path PASS (${DNS_NAME})"

echo "networkpolicy-deny-smoke: checking allowed outbound HTTPS path ${APP_CLIENT_POD} (app=console-app) -> ${HTTPS_URL}"
if ! https_output="$(run_curl "${APP_CLIENT_POD}" "${HTTPS_URL}" 2>&1)"; then
  fail "app-labelled pod could not reach ${HTTPS_URL}; allow-app-egress-https is not proven. Output: ${https_output}"
fi
echo "networkpolicy-deny-smoke: HTTPS path PASS (${#https_output} bytes from target)"

if [[ "${POSTGRES_AVAILABLE}" -eq 1 ]]; then
  POSTGRES_HOST="${POSTGRES_SERVICE}.${NAMESPACE}.svc.cluster.local"
  echo "networkpolicy-deny-smoke: checking allowed Postgres path ${POSTGRES_CLIENT_POD} (app=console-app) -> ${POSTGRES_HOST}:${POSTGRES_PORT}"
  if ! postgres_output="$("${KUBECTL}" -n "${NAMESPACE}" exec "${POSTGRES_CLIENT_POD}" -- pg_isready -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -t "${CURL_CONNECT_TIMEOUT}" 2>&1)"; then
    fail "app-labelled Postgres client could not reach ${POSTGRES_HOST}:${POSTGRES_PORT}; allow-app-egress-postgres/allow-postgres-from-app are not proven. Output: ${postgres_output}"
  fi
  echo "networkpolicy-deny-smoke: Postgres path PASS (${postgres_output})"
fi

echo "networkpolicy-deny-smoke: checking denied app-tier path ${APP_CLIENT_POD} (app=console-app) -> ${TARGET_SERVICE}:8080"
set +e
denied_output="$(run_curl "${APP_CLIENT_POD}" "${TARGET_URL}" 2>&1)"
denied_ec=$?
set -e
if [[ "${denied_ec}" -eq 0 ]]; then
  fail "app-labelled pod unexpectedly reached ${TARGET_URL}; default-deny-egress-app-tier is not enforcing the expected denied path. Output: ${denied_output}"
fi

echo "networkpolicy-deny-smoke: denied path PASS (curl exit ${denied_ec} as expected)"
echo "networkpolicy-deny-smoke: denied output: ${denied_output}"
echo "networkpolicy-deny-smoke: PASS - NetworkPolicy enforcement allowed DNS/HTTPS/Postgres paths and denied app-tier TCP/8080 egress while the control path succeeded"
