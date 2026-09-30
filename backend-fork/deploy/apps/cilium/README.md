# Dark on-prem Cilium activation and rollback runbook

This directory stages the ADR-0024 on-prem Cilium app without deploying it live.
It is intentionally under `deploy/apps/cilium/`, not `deploy/argocd/apps/`, and
`application.yaml` has no `syncPolicy.automated` block. A merge to `main` is a
no-op for the current Argo app-of-apps root until an operator deliberately applies
this directory and manually syncs the `cilium-onprem` Application.

## What is staged

- `project.yaml` creates an isolated Argo CD project that allows the Cilium Helm
  repo and the maintenance repo values file without broadening the live
  `maintenance` AppProject.
- `application.yaml` defines a dark, manual-sync Argo CD Application for the
  upstream Cilium Helm chart. It has sync wave `0` because the CNI must be ready
  before workload apps when a future cutover syncs this manually.
- `values.yaml` lifts provider-neutral Talos settings from the sibling oyatie
  `infra/talos/cilium-values.yaml`: Kubernetes IPAM, kube-proxy replacement for
  eBPF service load-balancing, Talos cgroup handling, and Hubble metrics with no
  Prometheus-operator assumption. The dark Argo CD render also switches Hubble
  TLS generation from Helm-rendered secrets to Cilium's in-cluster certgen
  Job/CronJob so future manual syncs do not see random certificate drift.

## Keep it dark until explicit activation

The safety contract is that this app is visible in Git but inert in the live
GitOps root:

1. Do not move or copy these files into `deploy/argocd/apps/` during staging.
   `deploy/argocd/root.yaml` watches that live app directory, so adding a Cilium
   Application there would make the app part of the root app-of-apps flow.
2. Do not add `syncPolicy.automated` to `application.yaml` before a founder or
   operator activation ticket authorizes the substrate cutover.
3. `kubectl apply -k deploy/apps/cilium` is a visibility step only: it creates the
   AppProject/Application objects, but Argo CD should report the Cilium app as a
   manually controlled app until an operator runs an explicit sync.
4. Keep BGP/L2 speaker values disabled in this app unless the VIP/ingress lane
   has selected Cilium as the speaker and recorded router peers, IP pools, and
   failover ownership. Cilium as the CNI does not require BGP.

Use these quick checks before every handoff:

```sh
test ! -e deploy/argocd/apps/cilium.yaml
kubectl kustomize deploy/apps/cilium
python3 - <<'PY'
from pathlib import Path
raise SystemExit(1 if "automated:" in Path("deploy/apps/cilium/application.yaml").read_text() else 0)
PY
```

## Environment decision: on-prem vs `oci-guest`

The on-prem and OCI guest contexts must make an explicit CNI decision. Do not let
Cilium activation for one context imply the other.

| Context | Default posture | If you keep flannel | If you adopt Cilium |
|---|---|---|---|
| `on-prem` / ADR-0024 | Intended to use this Cilium stage. `deploy/talos/on-prem/cluster.patch.yaml` sets `cluster.network.cni.name=none` and `cluster.proxy.disabled=true` so Cilium owns the dataplane and kube-proxy replacement. | Not acceptable for production NetworkPolicy enforcement unless a different policy-capable CNI such as Calico/Canal is explicitly selected and documented. Plain flannel leaves Maintenance NetworkPolicies inert. | Preferred path. Activate only after real node inventory, Talos configs, API/VIP readiness, rollback target, and NetworkPolicy verification are recorded. |
| `oci-guest` | Current live single-node OCI/Talos path is separate and should remain stable unless a ticket deliberately changes it. | Lowest-risk option for the current single-node A1 guest. Consequence: do not claim live NetworkPolicy isolation; render checks remain desired-state proof only, and `CONSOLE_NETWORKPOLICY_PREFLIGHT=require` is expected to fail until a policy enforcer is present. | Possible only as a separate OCI maintenance/rebuild lane. Consequence: schedule downtime risk for the single node, change Talos CNI/proxy posture intentionally, preserve OCI rollback artifacts, and do not claim HA just because Cilium is installed. |

Decision criteria for `oci-guest`:

- Keep flannel if the goal is to preserve the known live OCI path, avoid single
  node CNI migration risk, and accept that NetworkPolicy manifests are not packet
  enforcement evidence on that cluster.
- Adopt Cilium only if the current live OCI context needs actual NetworkPolicy
  enforcement, Hubble/eBPF observability, or kube-proxy replacement strongly
  enough to justify a planned maintenance window or rebuild. Record the rollback
  target first, because a single-node CNI failure is customer-visible.
- If OCI adopts Cilium, do it in an OCI-specific lane with OCI-specific Talos
  config evidence. Do not reuse the on-prem Application name or activation proof
  as if it had changed OCI automatically.

## NetworkPolicy enforcement contract

This stage is the intended ADR-0024 policy-capable CNI for production
NetworkPolicy enforcement. The Maintenance policies in
`deploy/apps/console/base/networkpolicy.yaml` can render before this app is
live, but they do not isolate traffic on plain Talos/flannel because flannel does
not implement NetworkPolicy. If a future activation chooses Calico or Canal
(flannel dataplane plus Calico policy) instead of Cilium, update this doc and the
Talos promotion evidence before claiming equivalent enforcement.

Applying this directory or passing `scripts/render-k8s-manifests.sh` /
`npm run check:production-hardening` is therefore only manifest proof. A
production cutover must also show Cilium Ready status and a Maintenance
deny/allow connectivity smoke that proves the default-deny ingress/egress policy
set is enforced before routing production traffic through the cluster.

## Prerequisites before manual sync

Record these facts in the activation ticket before any `argocd app sync`:

1. Target context and rollback target:
   - exact Kubernetes context and Talos context;
   - whether this is `on-prem` activation, OCI adoption, or a scratch rehearsal;
   - where traffic will roll back first if the cutover fails.
2. Talos/CNI compatibility:
   - for on-prem, generated machine configs include
     `cluster.network.cni.name=none` and `cluster.proxy.disabled=true` from
     `deploy/talos/on-prem/cluster.patch.yaml`;
   - `values.yaml` still points to the Talos KubePrism API endpoint
     `localhost:7445`, or the activation ticket records the replacement
     `k8sServiceHost` / `k8sServicePort` values;
   - kube-proxy is not still managing Services on the same cluster when Cilium
     kube-proxy replacement is enabled.
3. Substrate readiness:
   - control-plane API endpoint/VIP is reachable and stable;
   - every node has correct time and MTU for the site;
   - rollback artifacts exist: Talos secrets/talosconfig, recent etcd snapshot,
     and previous traffic target details;
   - image pull access for the Cilium chart images exists, or internal mirror
     overrides are recorded.
4. GitOps safety:
   - Argo CD is installed on the target cluster;
   - `deploy/apps/cilium/` renders cleanly;
   - the app is still outside `deploy/argocd/apps/` and has no automated sync;
   - no unrelated substrate upgrade, storage failover drill, or VIP migration is
     running at the same time.
5. VIP/BGP coordination:
   - BGP is optional and not required for this CNI activation;
   - `bgpControlPlane.enabled=false` and `l2announcements.enabled=false` remain
     the default;
   - if the site needs BGP for ingress VIPs, coordinate with the VIP/BGP lane
     before changing Cilium speaker settings.

## Activation sequence

The safest path is a fresh or dark on-prem cluster that has not received
production traffic yet. Migrating a live flannel cluster to Cilium is a separate,
higher-risk maintenance operation.

1. Freeze substrate changes and confirm the target context:

   ```sh
   kubectl config current-context
   kubectl get nodes -o wide
   kubectl -n kube-system get pods -o wide
   ```

2. Render the staged app locally and review the Argo diff:

   ```sh
   kubectl kustomize deploy/apps/cilium
   helm template cilium cilium/cilium \
     --version 1.19.4 \
     --namespace kube-system \
     -f deploy/apps/cilium/values.yaml
   ```

3. Ensure Talos is already in the Cilium-ready posture. On-prem machine configs
   should have been generated from `deploy/talos/on-prem/cluster.patch.yaml`
   before bootstrap or before the approved maintenance step. Do not attempt a
   silent in-place flannel-to-Cilium swap on a production cluster without a
   rollback target and downtime window.

4. Apply the dark Argo objects only after the above checks pass:

   ```sh
   kubectl apply -k deploy/apps/cilium
   kubectl -n argocd get app cilium-onprem -o yaml
   ```

   At this point Cilium is still not installed unless an operator syncs the app.

5. Run the explicit cutover sync from Argo CD UI or CLI:

   ```sh
   argocd app diff cilium-onprem
   argocd app sync cilium-onprem
   argocd app wait cilium-onprem --health --sync --timeout 600
   ```

6. Wait for the CNI and operator to converge before syncing workload apps:

   ```sh
   kubectl -n kube-system rollout status daemonset/cilium --timeout=10m
   kubectl -n kube-system rollout status deployment/cilium-operator --timeout=10m
   kubectl -n kube-system get pods -l k8s-app=cilium -o wide
   kubectl -n kube-system get configmap cilium-config \
     -o jsonpath='enable-policy={.data.enable-policy}{"\n"}enable-k8s-networkpolicy={.data.enable-k8s-networkpolicy}{"\n"}'
   kubectl get ciliumnodes.cilium.io
   kubectl get nodes -o wide
   ```

   If the Cilium CLI is installed, also run `cilium status --wait` and the
   upstream Cilium connectivity test appropriate for the site.

7. Apply or resync the Maintenance namespace/workload manifests only after Cilium
   is ready, then prove policy enforcement:

   ```sh
   CONSOLE_NETWORKPOLICY_PREFLIGHT=require \
     CONSOLE_NETWORKPOLICY_EXPECTED_ENFORCER=cilium \
     npm run check:k8s:networkpolicy

   CONSOLE_NETWORKPOLICY_EXPECTED_ENFORCER=cilium \
     CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES=auto \
     npm run smoke:k8s:networkpolicy-deny
   ```

   Use `CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES=required` when CNPG is expected to be
   live, and `skip` only for a rehearsal cluster that intentionally has no
   database Service yet. If the site blocks public image pulls or generic public
   egress, override `CONSOLE_NETWORKPOLICY_SMOKE_CLIENT_IMAGE`,
   `CONSOLE_NETWORKPOLICY_SMOKE_TARGET_IMAGE`,
   `CONSOLE_NETWORKPOLICY_SMOKE_POSTGRES_CLIENT_IMAGE`, and
   `CONSOLE_NETWORKPOLICY_SMOKE_HTTPS_URL` with approved internal mirrors/probes.

   The readback must pass against the target cluster. The smoke transcript must
   show all of these assertions before claiming isolation:

   - allowed same-namespace ingress: an unlabeled control pod reaches the
     temporary `app=console-app` target on TCP/8080;
   - allowed DNS egress: an `app=console-app` client resolves
     `kubernetes.default.svc.cluster.local` through kube-dns;
   - allowed outbound HTTPS egress: the same app-tier client reaches the
     configured HTTPS probe on TCP/443, matching the existing broad 443 allowance
     for OCI Object Storage, FCM, Solapi, or an approved site proxy;
   - allowed Postgres access when applicable: an app-labelled Postgres client
     reaches `console-db-rw.console.svc.cluster.local:5432`, proving the
     `allow-app-egress-postgres` and `allow-postgres-from-app` path;
   - explicit denied flow: the `app=console-app` client fails to reach the temporary
     HTTP target on TCP/8080. On pre-cutover plain flannel, if the
     preflight were bypassed, this denied connection would normally succeed
     because flannel is not enforcing the app-tier default-deny egress policy.

8. Only after Cilium and NetworkPolicy verification are green should downstream
   lanes move production traffic through the on-prem VIP/Traefik path. VIP
   failover, DNS, storage, CNPG, and OpenBao checks remain separate activation
   gates owned by their runbooks.

## Downtime and risk notes

- Fresh on-prem activation before traffic should not cause customer downtime, but
  the cluster is not ready for workload syncs until Cilium is healthy on every
  node.
- Live flannel-to-Cilium migration can disrupt pod networking, Service routing,
  DNS, admission webhooks, and Argo CD itself. Treat it as a maintenance window;
  do not combine it with Kubernetes upgrades, storage failover, or ingress VIP
  failover drills.
- `kubeProxyReplacement: true` assumes the Talos proxy is disabled. A mismatch
  between kube-proxy and Cilium eBPF replacement can produce confusing partial
  Service failures.
- If the target cluster cannot pull Cilium images or if Hubble certgen jobs are
  blocked by policy/RBAC, stop before routing traffic and preserve Argo/Cilium
  events for the incident log.
- BGP route convergence is not part of the default Cilium activation here. If BGP
  is later enabled by the VIP/BGP lane, add router-peer rollback and route-withdraw
  evidence to that lane's runbook.

## Rollback considerations

Choose the rollback based on how far activation progressed:

1. Before manual sync:
   - delete or leave the dark Argo Application/Project;
   - no Cilium workloads or dataplane changes should exist from this directory.
2. During sync before production traffic:
   - stop workload promotion;
   - capture `argocd app get cilium-onprem`, `kubectl -n kube-system get events`,
     and Cilium pod logs;
   - if Cilium never became the active CNI, delete the failed Cilium resources and
     return to the previous Talos/bootstrap plan;
   - for a cluster already bootstrapped with `cni.name=none`, expect that rollback
     may mean rebuilding or resetting nodes with the previous machine config
     rather than hot-swapping back to flannel.
3. After production traffic moved:
   - move DNS, upstream routing, or the ingress VIP back to the previous target
     first. If the previous target is OCI guest, follow `deploy/OPS-RUNBOOK.md`
     and the preserved OCI Traefik/reserved-IP path;
   - pause Argo syncs for dependent workload apps;
   - preserve etcd snapshots, Cilium events/logs, Hubble flow evidence if
     available, and the exact failure timeline;
   - do not remove the only active CNI from a serving cluster unless the incident
     commander has chosen rebuild/restore as the rollback path.

Never run plain flannel and Cilium as competing CNIs on the same node set, and do
not re-enable kube-proxy behind Cilium's back without an explicit Talos/CNI
rollback plan.

## Evidence to attach after cutover

- Activation ticket identifying `on-prem` vs `oci-guest`, the rollback target,
  and whether the OCI guest kept flannel or adopted Cilium in a separate lane.
- Render/diff evidence for `deploy/apps/cilium/` and the Cilium Helm chart.
- Talos machine-config evidence showing CNI `none`, proxy disabled, and the chosen
  KubePrism/API endpoint values for the activated context.
- Argo CD sync/health output for `cilium-onprem`.
- Cilium readiness output: DaemonSet/Operator rollout, pod placement, node status,
  `cilium-config` policy settings, and `cilium status --wait` when available.
- NetworkPolicy proof from `npm run check:k8s:networkpolicy` with
  `CONSOLE_NETWORKPOLICY_PREFLIGHT=require` and from
  `npm run smoke:k8s:networkpolicy-deny` showing the allowed control, DNS,
  HTTPS, and Postgres-if-present paths plus the denied `app=console-app` TCP/8080
  path.
- If BGP was enabled later by the VIP lane, the peer/ASN/VIP-pool config, route
  convergence evidence, and rollback/route-withdraw proof from that lane.
