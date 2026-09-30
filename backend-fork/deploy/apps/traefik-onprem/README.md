# Dark on-prem Traefik LoadBalancer variant

This directory stages the ADR-0024 on-prem Traefik ingress shape without changing
the live OCI ingress path.

It is intentionally under `deploy/apps/traefik-onprem/`, not
`deploy/argocd/apps/`, and `application.yaml` has no `syncPolicy.automated` block.
A merge to `main` is therefore a no-op for the current Argo app-of-apps root,
because `deploy/argocd/root.yaml` only watches `deploy/argocd/apps/`.

## What is staged

- `project.yaml` creates an isolated Argo CD project for the dark on-prem Traefik
  app without broadening the live `maintenance` AppProject.
- `application.yaml` defines a manual-sync Argo CD Application for the upstream
  Traefik chart plus this repo's `values.yaml`.
- `values.yaml` keeps the chart as a multi-replica `Deployment`, enables a real
  `Service` of `type: LoadBalancer`, requests the MetalLB
  `console-onprem-ingress` pool, enables `publishedService`, and spreads two
  replicas across distinct nodes with pod anti-affinity/topology spread.

The current OCI app under `deploy/argocd/apps/traefik.yaml` remains the live path
and consumes `deploy/apps/traefik-oci-guest/values.yaml`: it keeps the hostPort
DaemonSet shape, disabled Service, and explicit `140.245.68.253` ingress
endpoint until a separate operator-approved cutover.

## Activation prerequisites

1. Stage the selected VIP provider from `deploy/apps/vip-ingress/` and replace its
   documentation-only `10.0.0.240/32` pool placeholder with the real reserved
   on-prem VIP or pool. Do **not** reuse the OCI reserved address.
2. Apply this dark app only when intentionally adding Argo visibility:
   `kubectl apply -k deploy/apps/traefik-onprem`.
3. Do not sync this app side-by-side with the live OCI `traefik` Application in
   the same cluster. Cutover must first disable or replace the current
   `deploy/argocd/apps/traefik.yaml` ownership path.
4. Before production wiring, record the real VLAN/subnet, external DNS target,
   node drain/kill failover result, and rollback path.

## Render checks

```sh
kubectl kustomize deploy/apps/traefik-onprem
helm template traefik traefik --repo https://traefik.github.io/charts \
  --version 40.3.0 --namespace traefik \
  -f deploy/apps/traefik-onprem/values.yaml
```
