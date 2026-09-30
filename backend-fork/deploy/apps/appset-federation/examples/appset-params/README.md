# Non-secret ApplicationSet render fixtures

These files are parameter fixtures for manual ApplicationSet template review.
They are not Kubernetes Secret manifests and intentionally contain no cluster
credentials.

- `one-primary.yaml` documents the degenerate one-cluster case. A selected
  `primary` cluster must generate a root Application sourcing
  `deploy/argocd/apps` at `targetRevision: main`.
- `primary-and-warm-standby.yaml` documents the HA case. The selected primary
  cluster sources `deploy/argocd/apps`; the warm-standby cluster sources
  `deploy/apps/appset-federation/app-groups/warm-standby`.
