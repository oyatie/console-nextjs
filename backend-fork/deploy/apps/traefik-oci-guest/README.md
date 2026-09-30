# OCI guest Traefik hostPort overlay

This directory holds the live OCI guest Traefik Helm values consumed by
`deploy/argocd/apps/traefik.yaml`.

The overlay intentionally preserves the current single-node OCI ingress shape:

- reserved OCI public IP `140.245.68.253` is published through
  `providers.kubernetesIngress.ingressEndpoint.ip`;
- the chart renders Traefik as a `DaemonSet`;
- the Traefik `Service` stays disabled; and
- entrypoints bind node `hostPort` 80 and 443.

Keep this overlay separate from the DARK on-prem HA variant at
`deploy/apps/traefik-onprem/`. On-prem ingress must use a real
`Service type=LoadBalancer` plus the selected VIP provider and must not import
this OCI reserved-IP/hostPort coupling.

## Render check

```sh
helm template traefik traefik --repo https://traefik.github.io/charts \
  --version 40.3.0 --namespace traefik \
  -f deploy/apps/traefik-oci-guest/values.yaml
```
