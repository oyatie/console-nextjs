# Dark on-prem VIP ingress activation

This directory stages the ADR-0024 roadmap item #6 ingress VIP provider for the
`on-prem` context. The selected provider is **MetalLB in layer-2 mode**. It is a
DARK path: it is intentionally under `deploy/apps/vip-ingress/`, not
`deploy/argocd/apps/`, and `application.yaml` has no `syncPolicy.automated`
block. A merge to `main` is therefore a no-op for the current Argo app-of-apps
root, because `deploy/argocd/root.yaml` only watches `deploy/argocd/apps/`.

The live OCI guest ingress remains a separate path. Do not copy the OCI reserved
IP `140.245.68.253`, the hostPort Traefik `DaemonSet`, or the disabled-Service
shape into this on-prem path. The OCI shape is preserved by
`deploy/apps/traefik-oci-guest/` and the live `deploy/argocd/apps/traefik.yaml`
Application until a separate operator-approved cutover replaces it.

## What is staged

- `project.yaml` creates an isolated Argo CD project for the dark VIP provider
  app without broadening the live `maintenance` AppProject.
- `application.yaml` defines a manual-sync Argo CD Application with three
  sources: the upstream MetalLB Helm chart, this repo's `values.yaml`, and this
  repo's provider config under `manifests/`.
- `values.yaml` keeps chart-owned CRDs/RBAC enabled, disables BGP/FRR/frr-k8s for
  the initial L2 path, and avoids Prometheus/CNI assumptions while the substrate
  is still dark.
- `manifests/namespace.yaml` declares the `metallb-system` namespace and required
  privileged Pod Security labels for the speaker on Talos nodes.
- `manifests/metallb-l2-config.yaml` declares the placeholder
  `console-onprem-ingress` `IPAddressPool` plus the `console-onprem-l2`
  `L2Advertisement`.

The sibling oyatie infra sources available locally provide Talos/Cilium/control-
plane VIP assumptions but no ready MetalLB/kube-vip Service LoadBalancer app, so
this stage lifts only the provider-neutral on-prem/Talos constraints and uses the
upstream MetalLB chart for provider resources.

## Activation prerequisites

Complete and record these before any manual sync:

1. Select the on-prem Kubernetes and Argo CD context. Confirm you are not pointed
   at the current OCI guest cluster:
   ```sh
   kubectl config current-context
   kubectl get nodes -o wide
   argocd context
   ```
2. Confirm at least two schedulable worker nodes share the L2 segment that will
   own the ingress VIP. Label intended VIP-capable workers if the site needs an
   explicit selector, for example:
   ```sh
   kubectl label node <worker-a> console.nousresearch.com/ingress-vip-node=true
   kubectl label node <worker-b> console.nousresearch.com/ingress-vip-node=true
   ```
3. Reserve the ingress VIP or VIP pool outside DHCP and router-managed ranges.
   Record the VLAN/subnet, gateway, DNS name, and owner. Do **not** use the OCI
   reserved address `140.245.68.253`.
4. Replace the documentation-only `10.0.0.240/32` address in
   `manifests/metallb-l2-config.yaml` with the reserved on-prem VIP or pool.
   Add `interfaces` and/or `nodeSelectors` to `console-onprem-l2` if the
   fabric should advertise only from specific NICs or workers.
5. Confirm the on-prem Traefik variant in `deploy/apps/traefik-onprem/` is the
   intended consumer. It uses `Service type=LoadBalancer` and explicitly requests
   the `console-onprem-ingress` pool with
   `metallb.universe.tf/address-pool: console-onprem-ingress`.
6. Keep the live OCI `traefik` Application intact until cutover. Do not sync
   `traefik-onprem` side-by-side with the live OCI hostPort Application in the
   same cluster.

## Render and pre-flight checks

Run these from the repository root after replacing the site-specific VIP values:

```sh
kubectl kustomize deploy/apps/vip-ingress | tee /tmp/vip-ingress-app.yaml
kubectl kustomize deploy/apps/vip-ingress/manifests | tee /tmp/vip-ingress-manifests.yaml
helm template metallb metallb --repo https://metallb.github.io/metallb \
  --version 0.16.1 --namespace metallb-system \
  -f deploy/apps/vip-ingress/values.yaml | tee /tmp/vip-ingress-metallb.yaml

kubectl kustomize deploy/apps/traefik-onprem | tee /tmp/traefik-onprem-app.yaml
helm template traefik traefik --repo https://traefik.github.io/charts \
  --version 40.3.0 --namespace traefik \
  -f deploy/apps/traefik-onprem/values.yaml | tee /tmp/traefik-onprem-chart.yaml

grep -nE "140.245.68.253|hostPort" \
  /tmp/vip-ingress-app.yaml \
  /tmp/vip-ingress-manifests.yaml \
  /tmp/vip-ingress-metallb.yaml \
  /tmp/traefik-onprem-app.yaml \
  /tmp/traefik-onprem-chart.yaml && \
  echo "unexpected OCI coupling in on-prem path" && exit 1 || true
```

Expected success signals:

- the VIP app renders an Argo CD `AppProject`, an Argo CD `Application`, the
  `metallb-system` namespace, an `IPAddressPool`, and an `L2Advertisement`;
- the MetalLB chart renders controller/speaker/RBAC/CRD resources with BGP/FRR
  disabled;
- the on-prem Traefik render has a `LoadBalancer` Service, two Deployment
  replicas, no `hostPort`, and no `140.245.68.253` reference; and
- the OCI guest overlay still renders separately from `deploy/apps/traefik-oci-guest/`.

## Manual Argo CD activation

1. Apply the dark VIP Argo project/application for visibility:
   ```sh
   kubectl apply -k deploy/apps/vip-ingress
   argocd app get vip-ingress-metallb-onprem
   ```
2. Manually sync the VIP provider only after the prerequisites above are complete:
   ```sh
   argocd app sync vip-ingress-metallb-onprem
   argocd app wait vip-ingress-metallb-onprem --health --sync --timeout 300
   kubectl -n metallb-system get pods -o wide
   kubectl -n metallb-system get ipaddresspool,l2advertisement
   ```
3. Apply the dark on-prem Traefik Argo project/application only when the operator
   is intentionally replacing the OCI hostPort ingress in that target cluster:
   ```sh
   kubectl apply -k deploy/apps/traefik-onprem
   argocd app sync traefik-onprem
   argocd app wait traefik-onprem --health --sync --timeout 300
   kubectl -n traefik get deploy,pod,svc -o wide
   ```
4. Confirm the `traefik` Service receives the reserved VIP and the provider owns
   it from the expected pool:
   ```sh
   kubectl -n traefik get svc traefik -o wide
   kubectl -n traefik describe svc traefik | grep -E "LoadBalancer Ingress|metallb|console-onprem-ingress"
   ```
5. Move DNS or upstream routing to the VIP only after the service is healthy and
   the failover validation below passes. Record the DNS TTL, old target, new VIP,
   and rollback target in the activation ticket/runbook.

## VIP failover validation

Run the failover drill from a host on the same L2 network as the worker nodes and
from an operator shell with cluster admin access.

1. Capture the baseline:
   ```sh
   VIP=<reserved-on-prem-vip>
   HOST=<ingress-hostname>
   IFACE=<operator-l2-interface>

   kubectl get nodes -o wide
   kubectl -n metallb-system get pods -l app.kubernetes.io/component=speaker -o wide
   kubectl -n traefik get deploy,pod,svc -o wide
   arping -I "$IFACE" -c 3 "$VIP"
   curl -vk --resolve "$HOST:443:$VIP" "https://$HOST/healthz"
   ```
   Identify the current VIP holder from MetalLB speaker logs and/or the ARP MAC
   returned by `arping` matched to a worker NIC:
   ```sh
   kubectl -n metallb-system logs -l app.kubernetes.io/component=speaker --since=10m | grep "$VIP" || true
   ```
2. Kill, reboot, or isolate the node currently holding the VIP. For a destructive
   drill, power off or reboot the holder out of band. For a non-destructive drill,
   drain only if the site's L2 advertisement/nodeSelector policy removes that
   node from VIP eligibility; a plain `kubectl drain` may leave the speaker
   DaemonSet running and therefore is not proof of VIP movement by itself:
   ```sh
   kubectl drain <vip-holder-node> --ignore-daemonsets --delete-emptydir-data --timeout=5m
   ```
3. Watch the VIP move and ingress stay reachable:
   ```sh
   watch -n 2 "kubectl -n traefik get svc traefik -o wide; kubectl -n metallb-system get pods -l app.kubernetes.io/component=speaker -o wide"
   arping -I "$IFACE" -c 5 "$VIP"
   for i in $(seq 1 30); do
     date -u +%FT%TZ
     curl -fsS --resolve "$HOST:443:$VIP" "https://$HOST/healthz" || exit 1
     sleep 2
   done
   ```
4. Success means all of the following are true:
   - the VIP answers ARP/NDP from a different worker or MetalLB logs show a new
     speaker announcing it;
   - `kubectl -n traefik get svc traefik` still shows the reserved VIP;
   - at least one Traefik pod remains Ready on a surviving worker;
   - repeated HTTPS requests through the VIP succeed after the expected brief ARP
     convergence window; and
   - Argo CD reports both `vip-ingress-metallb-onprem` and `traefik-onprem` as
     Healthy/Synced.
5. Recover the node with `kubectl uncordon <node>` or the site recovery process,
   then rerun the baseline checks. Record timestamps, old holder, new holder,
   outage duration if any, command output, and the final rollback decision.

Troubleshooting checks:

- VIP does not assign: verify the Traefik Service annotation names
  `console-onprem-ingress`, the pool has `autoAssign: false`, and the Service
  requests the pool explicitly.
- VIP does not ARP: verify all selected workers are on the same L2 segment,
  `L2Advertisement` selectors/interfaces match real node labels/NIC names, and
  no switch/router ARP security blocks the VIP MAC.
- VIP remains on the drained node: remember that DaemonSets ignore a normal drain;
  remove the node from L2 advertisement eligibility or use a real power/network
  failure drill.
- Ingress fails while VIP moves: verify Traefik has Ready replicas on surviving
  workers, `externalTrafficPolicy: Local` has local endpoints on the advertising
  node, NetworkPolicy/Cilium allows traffic, and cert-manager/Ingress resources
  are Healthy.

## Rollback guidance

- If activation fails before DNS or upstream routing moves, stop the on-prem sync,
  leave DNS on the existing target, and delete the manually applied dark Argo
  Applications only after collecting failure evidence:
  ```sh
  argocd app delete traefik-onprem --cascade || true
  argocd app delete vip-ingress-metallb-onprem --cascade || true
  kubectl delete -k deploy/apps/traefik-onprem || true
  kubectl delete -k deploy/apps/vip-ingress || true
  ```
- If traffic already moved and the on-prem path fails, move DNS/upstream routing
  back to the previous target first. For the OCI guest target, the previous path
  is the preserved `deploy/apps/traefik-oci-guest/` hostPort/reserved-IP overlay
  and live `deploy/argocd/apps/traefik.yaml` Application.
- If the failover drill caused data-plane uncertainty, do not keep retrying VIP
  changes. Quarantine the failed node, restore ingress to the known-good target,
  and open a follow-up card with the captured MetalLB, Traefik, Argo CD, and node
  evidence.

## BGP fallback note

BGP is not staged here. If the final on-prem fabric cannot provide a shared L2
segment, coordinate the fallback with issue #377 before enabling MetalLB BGP or
frr-k8s. That follow-up must add explicit placeholders for local ASN, peer ASN,
peer IPs, advertised VIP pool, and Cilium/BGP ownership boundaries.
