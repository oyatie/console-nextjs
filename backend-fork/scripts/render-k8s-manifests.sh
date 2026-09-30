#!/usr/bin/env bash
# Render and sanity-check the Kubernetes/Argo delivery inputs that GitOps applies.
# This is intentionally cluster-free: CI verifies the committed desired state before
# Argo CD reconciles it into the live cluster.
set -euo pipefail

OUT_DIR="${1:-${RUNNER_TEMP:-/tmp}/console-rendered-k8s}"
mkdir -p "${OUT_DIR}"

require() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "render-k8s: required tool '$1' not found" >&2
    exit 127
  fi
}

require kubectl

PROD_RENDER="${OUT_DIR}/console-prod.yaml"
ARGO_RENDER="${OUT_DIR}/argocd-apps.yaml"

kubectl kustomize deploy/apps/console/overlays/prod > "${PROD_RENDER}"
cat deploy/argocd/project.yaml deploy/argocd/root.yaml deploy/argocd/apps/*.yaml > "${ARGO_RENDER}"

# Production images must be immutable digest-pinned artifacts. The image-release
# workflow scans/signs the digest, then this overlay is the single Argo source.
if grep -R "newTag:" deploy/apps/console/overlays/prod >/dev/null; then
  echo "render-k8s: prod overlay must not use mutable newTag values" >&2
  exit 1
fi
if [[ "$(grep -c 'digest: sha256:' deploy/apps/console/overlays/prod/kustomization.yaml)" -lt 1 ]]; then
  echo "render-k8s: expected a console-app sha256 digest pin in prod overlay" >&2
  exit 1
fi

# Live production must track main. Past incidents came from Argo Applications left
# on a feature branch while CI/release/image workflows were green on main.
if grep -R "targetRevision: feat/" deploy/argocd >/dev/null; then
  echo "render-k8s: Argo targetRevision must not point at a feature branch" >&2
  grep -R "targetRevision: feat/" deploy/argocd >&2
  exit 1
fi
for app in deploy/argocd/root.yaml deploy/argocd/apps/console.yaml; do
  if ! grep -q "targetRevision: main" "${app}"; then
    echo "render-k8s: ${app} must target main" >&2
    exit 1
  fi
done

printf 'render-k8s: wrote %s and %s\n' "${PROD_RENDER}" "${ARGO_RENDER}"
wc -l "${PROD_RENDER}" "${ARGO_RENDER}"
