#!/usr/bin/env bash
# Manual deploy helper: take a built commit all the way to a verified prod rollout.
#
#   scripts/deploy.sh [--digest-bump-only|--bump-only] [<git-sha>]
#
# Given a git sha (default: HEAD), this:
#   1. Finds + watches that sha's "Image Release" run until it completes.
#   2. Reads the freshly built @sha256 console-app digest from the run's
#      digest artifacts (the same artifacts the auto-bump job consumes).
#   3. Bumps deploy/apps/console/overlays/prod/kustomization.yaml via
#      scripts/bump-prod-digests.sh and commits the one-shot authorization
#      consumption plus any digest change to the current branch.
#   4. Triggers an Argo refresh, verifies the Argo Application synced the
#      desired git revision, waits for the console-app Rollout and console-worker,
#      and verifies their pod image digests match the built artifacts.
#   5. Verifies the public endpoints return HTTP 200.
#
# One-shot by design: a successful or partially consumed authorization must not
# be replayed. Start a new governed authorization lineage instead of rerunning.
# Each step echoes what it is doing. Default deploy mode requires: gh (authenticated), git,
# curl, kubectl with the target cluster kubeconfig, and the argo-rollouts kubectl
# plugin. Rollout verification is fail-closed: missing/unreachable cluster
# tooling, Argo refresh/sync failures, rollout failures, or image-digest
# mismatches abort before endpoint checks or the final deployed-and-verified
# success message. Use --digest-bump-only when you intentionally want to update
# only the desired prod image digests; that mode does not claim deployment or
# rollout verification.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

usage() {
  cat >&2 <<'USAGE'
usage: scripts/deploy.sh [--digest-bump-only|--bump-only] [<git-sha>]

Default mode bumps the desired prod digests and then fail-closed verifies the
Argo Application revision, rollout health, pod image digests, and public
endpoints before printing a deployed-and-verified success message.

--digest-bump-only / --bump-only updates the desired prod digests only. It is an
explicit opt-in escape hatch for hosts without production cluster access and does
not claim deployment, rollout, or endpoint verification.
USAGE
}

MODE="deploy"
case "${1:-}" in
  --digest-bump-only|--bump-only)
    MODE="digest-bump-only"
    shift
    ;;
  -h|--help)
    usage
    exit 0
    ;;
  -*)
    echo "deploy: unknown option '$1'" >&2
    usage
    exit 2
    ;;
esac

if [[ $# -gt 1 ]]; then
  usage
  exit 2
fi

SHA="${1:-$(git rev-parse HEAD)}"
SHORT_SHA="${SHA:0:7}"
WORKFLOW="image-release.yml"
APP_NAME="console"             # ArgoCD Application name
NAMESPACE="console"            # cluster namespace for the rollouts
ROLLOUTS=(console-app)
WORKER_DEPLOYMENT="console-worker"
ENDPOINTS=(https://console.knllogistic.com https://knllogistic.com)
ARGO_NS="argocd"

log() { echo "deploy: $*" >&2; }
fail() { echo "deploy: $*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

require() {
  if ! have "$1"; then
    fail "required tool '$1' not found on PATH."
  fi
}

kubectl_required() {
  local desc="$1"
  shift
  local output ec
  set +e
  output="$(kubectl "$@" 2>&1)"
  ec=$?
  set -e
  if [[ ${ec} -ne 0 ]]; then
    echo "deploy: ${desc} failed; aborting without rollout verification." >&2
    if [[ -n "${output}" ]]; then
      echo "${output}" >&2
    fi
    exit 1
  fi
  printf '%s' "${output}"
}

kubectl_jsonpath() {
  local desc="$1"
  local namespace="$2"
  local resource="$3"
  local path="$4"
  kubectl_required "${desc}" -n "${namespace}" get "${resource}" -o "jsonpath=${path}"
}

verify_argo_revision() {
  local expected_revision="$1"
  local revision sync_status health_status

  log "  waiting for Argo Application/${APP_NAME} to report Synced at ${expected_revision:0:7}"
  for _ in $(seq 1 60); do
    revision="$(kubectl_jsonpath "read Argo Application/${APP_NAME} revision" "${ARGO_NS}" "application/${APP_NAME}" '{.status.sync.revision}')"
    sync_status="$(kubectl_jsonpath "read Argo Application/${APP_NAME} sync status" "${ARGO_NS}" "application/${APP_NAME}" '{.status.sync.status}')"
    health_status="$(kubectl_jsonpath "read Argo Application/${APP_NAME} health status" "${ARGO_NS}" "application/${APP_NAME}" '{.status.health.status}')"
    if [[ "${revision}" == "${expected_revision}" && "${sync_status}" == "Synced" ]]; then
      log "  Argo Application/${APP_NAME} is Synced at ${revision:0:7} (health=${health_status:-unknown})"
      return 0
    fi
    log "  Argo revision=${revision:-unknown} sync=${sync_status:-unknown} health=${health_status:-unknown}; waiting..."
    sleep 10
  done

  fail "Argo Application/${APP_NAME} did not report Synced at ${expected_revision}; deployment is not verified."
}

verify_argo_pre_refresh_revision() {
  local authorized_revision="$1"
  local desired_revision="$2"
  local revision

  revision="$(kubectl_jsonpath "read Argo Application/${APP_NAME} pre-refresh revision" "${ARGO_NS}" "application/${APP_NAME}" '{.status.sync.revision}')"
  if [[ "${revision}" != "${authorized_revision}" && "${revision}" != "${desired_revision}" ]]; then
    fail "Argo Application/${APP_NAME} pre-refresh revision is '${revision:-unknown}', expected exact authorized ${authorized_revision} or exact desired ${desired_revision}."
  fi
  log "  Argo pre-refresh revision is exact known revision ${revision}"
}

verify_workload_template_image() {
  local resource_kind="$1"
  local name="$2"
  local container="$3"
  local digest="$4"
  local image

  image="$(kubectl_jsonpath "read ${resource_kind}/${name} ${container} image" "${NAMESPACE}" "${resource_kind}/${name}" "{.spec.template.spec.containers[?(@.name==\"${container}\")].image}")"
  if [[ -z "${image}" ]]; then
    fail "${resource_kind}/${name} has no ${container} container image; deployment is not verified."
  fi
  if [[ "${image}" != *"@${digest}" ]]; then
    fail "${resource_kind}/${name} template image is '${image}', expected @${digest}; deployment is not verified."
  fi
  log "  ${resource_kind}/${name} template image -> ${image}"
}

verify_pod_images() {
  local app_label="$1"
  local container="$2"
  local digest="$3"
  local rows pod phase ready image_id image seen=0

  rows="$(kubectl_required "list pods for app=${app_label}" -n "${NAMESPACE}" get pods -l "app=${app_label}" -o "jsonpath={range .items[*]}{.metadata.name}{\"\\t\"}{.status.phase}{\"\\t\"}{range .status.containerStatuses[?(@.name==\"${container}\")]}{.ready}{\"\\t\"}{.imageID}{\"\\t\"}{.image}{end}{\"\\n\"}{end}")"
  if [[ -z "${rows}" ]]; then
    fail "no pods found for app=${app_label}; deployment is not verified."
  fi

  while IFS=$'\t' read -r pod phase ready image_id image; do
    if [[ -z "${pod}" ]]; then
      continue
    fi
    if [[ "${phase}" != "Running" || "${ready}" != "true" ]]; then
      fail "pod ${pod} is phase=${phase:-unknown} ready=${ready:-unknown}; deployment is not verified."
    fi
    if [[ "${image_id}" != *"${digest}"* && "${image}" != *"@${digest}" ]]; then
      fail "pod ${pod} ${container} imageID='${image_id:-unknown}' image='${image:-unknown}', expected ${digest}; deployment is not verified."
    fi
    seen=$((seen + 1))
  done <<< "${rows}"

  if [[ ${seen} -eq 0 ]]; then
    fail "no ${container} container statuses found for app=${app_label}; deployment is not verified."
  fi
  log "  app=${app_label} pods (${seen}) are running ${container}@${digest}"
}

require gh
require git
require python3
if [[ "${MODE}" == "deploy" ]]; then
  require curl
  require kubectl
fi

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
python3 "${REPO_ROOT}/scripts/check-production-promotion-authority.py" initial \
  --expected-sha "${SHA}" \
  --expected-ref "refs/heads/${BRANCH}" \
  --require-local-branch

if [[ "${MODE}" == "deploy" ]]; then
  kubectl_required "reach the target Kubernetes cluster" version >/dev/null
  kubectl_required "verify the argo-rollouts kubectl plugin" argo rollouts version >/dev/null
  kubectl_required "read Argo Application/${APP_NAME}" -n "${ARGO_NS}" get "application/${APP_NAME}" >/dev/null
  verify_argo_pre_refresh_revision "${SHA}" "${SHA}"
fi

# ---------------------------------------------------------------------------
# 1. Find + watch the Image Release run for this sha.
# ---------------------------------------------------------------------------
log "[1/5] locating the '${WORKFLOW}' run for ${SHORT_SHA}"
RUN_ID=""
for _ in $(seq 1 30); do
  RUN_ID="$(gh run list --workflow "${WORKFLOW}" --commit "${SHA}" \
    --json databaseId --jq '.[0].databaseId' 2>/dev/null || true)"
  if [[ -n "${RUN_ID}" && "${RUN_ID}" != "null" ]]; then
    break
  fi
  log "  no run yet for ${SHORT_SHA}; waiting for it to register..."
  sleep 10
done

if [[ -z "${RUN_ID}" || "${RUN_ID}" == "null" ]]; then
  echo "deploy: no '${WORKFLOW}' run found for ${SHA}. Push the commit and let CI build it first." >&2
  exit 1
fi

log "  found run ${RUN_ID}; watching until it completes"
# `gh run watch --exit-status` returns non-zero if the run failed.
if ! gh run watch "${RUN_ID}" --exit-status >/dev/null 2>&1; then
  echo "deploy: image-release run ${RUN_ID} did not succeed; aborting (nothing vulnerable is deployed)." >&2
  echo "deploy: inspect it with: gh run view ${RUN_ID} --log-failed" >&2
  exit 1
fi
log "  run ${RUN_ID} succeeded"

# ---------------------------------------------------------------------------
# 2. Read the console-app digest from the run's digest artifact.
# ---------------------------------------------------------------------------
log "[2/5] reading built digests from run ${RUN_ID} artifacts"
ART_DIR="$(mktemp -d)"
trap 'rm -rf "${ART_DIR}"' EXIT

gh run download "${RUN_ID}" --name digest-console-app --dir "${ART_DIR}/app"

APP_DIGEST="$(cat "${ART_DIR}/app/digest-console-app.txt")"
log "  console-app digest: ${APP_DIGEST}"

# ---------------------------------------------------------------------------
# 3. Bump the overlay + consume the one-shot authorization in a new commit.
# ---------------------------------------------------------------------------
OVERLAY="deploy/apps/console/overlays/prod/kustomization.yaml"
log "[3/5] bumping ${OVERLAY}"
bash "${REPO_ROOT}/scripts/bump-prod-digests.sh" "${APP_DIGEST}"
python3 "${REPO_ROOT}/scripts/check-production-promotion-authority.py" reset \
  --expected-sha "${SHA}"

log "  committing one-shot authorization consumption and optional digest bump to ${BRANCH}"
git add "${OVERLAY}" docs/release/PR-473-PRODUCTION-PROMOTION.authorization.json
git commit -m "deploy(prod): auto-bump console-app @${SHORT_SHA}"
python3 "${REPO_ROOT}/scripts/check-production-promotion-authority.py" pre-push \
  --expected-sha "${SHA}"
git push origin "HEAD:${BRANCH}"

DEPLOY_REVISION="$(git rev-parse HEAD)"
log "  desired prod git revision: ${DEPLOY_REVISION}"
python3 "${REPO_ROOT}/scripts/check-production-promotion-authority.py" remote \
  --expected-sha "${DEPLOY_REVISION}"

if [[ "${MODE}" == "digest-bump-only" ]]; then
  log "done: ${SHORT_SHA} desired prod digests updated only (console-app=${APP_DIGEST}); deployment, rollout, pod-image, and endpoint verification were NOT run."
  exit 0
fi

# ---------------------------------------------------------------------------
# 4. Nudge Argo to pick up the committed digests + wait for the rollouts.
#    The Application already has syncPolicy.automated, so a hard refresh just
#    shortens the reconcile latency; the Argo Rollouts blue/green canary gates a
#    bad release regardless of how the sync was triggered.
# ---------------------------------------------------------------------------
log "[4/5] refreshing Argo, verifying revision, and waiting for in-cluster workloads"
verify_argo_pre_refresh_revision "${SHA}" "${DEPLOY_REVISION}"
python3 "${REPO_ROOT}/scripts/check-production-promotion-authority.py" remote \
  --expected-sha "${DEPLOY_REVISION}"

log "  requesting an Argo hard refresh on Application/${APP_NAME}"
kubectl_required "request an Argo hard refresh on Application/${APP_NAME}" -n "${ARGO_NS}" annotate "application/${APP_NAME}" \
  "argocd.argoproj.io/refresh=hard" --overwrite >/dev/null

verify_argo_revision "${DEPLOY_REVISION}"

for rollout in "${ROLLOUTS[@]}"; do
  case "${rollout}" in
    console-app) rollout_digest="${APP_DIGEST}" ;;
    *) fail "no expected digest configured for rollout ${rollout}" ;;
  esac
  log "  waiting for rollout ${rollout} to be Healthy"
  kubectl_required "wait for rollout ${rollout} to reach Healthy" argo rollouts status "${rollout}" -n "${NAMESPACE}" --timeout 600s >/dev/null
  log "  rollout ${rollout} is Healthy"
  verify_workload_template_image "rollout" "${rollout}" "${rollout}" "${rollout_digest}"
  verify_pod_images "${rollout}" "${rollout}" "${rollout_digest}"
done

log "  waiting for deployment ${WORKER_DEPLOYMENT} to roll out"
kubectl_required "wait for deployment/${WORKER_DEPLOYMENT} to roll out" -n "${NAMESPACE}" rollout status "deployment/${WORKER_DEPLOYMENT}" --timeout 600s >/dev/null
verify_workload_template_image "deployment" "${WORKER_DEPLOYMENT}" "${WORKER_DEPLOYMENT}" "${APP_DIGEST}"
verify_pod_images "${WORKER_DEPLOYMENT}" "${WORKER_DEPLOYMENT}" "${APP_DIGEST}"

# ---------------------------------------------------------------------------
# 5. Verify the public endpoints return HTTP 200.
# ---------------------------------------------------------------------------
log "[5/5] verifying public endpoints return HTTP 200"
for url in "${ENDPOINTS[@]}"; do
  code=""
  for _ in $(seq 1 12); do
    code="$(curl -fsS -o /dev/null -w '%{http_code}' --max-time 10 "${url}" 2>/dev/null || true)"
    if [[ "${code}" == "200" ]]; then
      break
    fi
    log "  ${url} -> ${code:-no-response}; retrying..."
    sleep 5
  done
  if [[ "${code}" != "200" ]]; then
    echo "deploy: ${url} did not return 200 (last: ${code:-no-response})." >&2
    exit 1
  fi
  log "  ${url} -> 200"
done

python3 "${REPO_ROOT}/scripts/check-production-promotion-authority.py" remote \
  --expected-sha "${DEPLOY_REVISION}"
verify_argo_revision "${DEPLOY_REVISION}"
log "done: ${SHORT_SHA} deployed and verified (revision=${DEPLOY_REVISION:0:7}, console-app=${APP_DIGEST})"
