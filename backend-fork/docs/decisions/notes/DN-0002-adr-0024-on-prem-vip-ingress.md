---
id: DN-0002
kind: design-note
parent_adr: ADR-0024
authority: subordinate
activation: dark
date: 2026-07-10
owner: jasonlee
---

# DN-0002 — ADR-0024 on-prem VIP ingress approach

## Status

Chosen for dark staging. This note does not activate the live OCI ingress path.

## Sources reviewed

- Accepted ADR-0024 at `docs/decisions/ADR-0024-bare-metal-portability-and-ha.md`.
- GitHub issue #378, `[HA] VIP ingress — MetalLB/kube-vip + multi-node Traefik`.
- GitHub issue #377, `[HA] Cilium CNI (enforced NetworkPolicy + eBPF LB)`.
- GitHub issue #8, verified unrelated to Cilium/BGP despite the stale cross-reference in #378.
- Current live OCI ingress app: `deploy/argocd/apps/traefik.yaml`.
- Argo CD app-of-apps root: `deploy/argocd/root.yaml`.
- Current NetworkPolicy/CNI note: `deploy/apps/console/base/networkpolicy.yaml`.
- Sibling oyatie substrate sources under `infra/`, especially `infra/talos/cilium-values.yaml`,
  `infra/talos/controlplane.patch.yaml`, `infra/gitops/values.yaml`, and
  `infra/capi/clusters/values-example.yaml`.

The oyatie manifests available locally provide Cilium/Talos/control-plane VIP patterns, but no
ready-to-lift MetalLB or kube-vip Service LoadBalancer app. The maintenance VIP ingress manifests should
therefore use oyatie's substrate assumptions, not copy a nonexistent MetalLB/kube-vip manifest verbatim.

## Decision

Use **MetalLB in layer-2 mode** for the first on-prem Traefik ingress VIP.

The on-prem ingress lane should stage MetalLB as a dark app under `deploy/apps/vip-ingress/` and expose the
on-prem Traefik variant through a real `Service` of `type: LoadBalancer`. The current OCI app under
`deploy/argocd/apps/traefik.yaml` remains the `oci-guest` path: hostPort-based DaemonSet, disabled Service,
and explicit reserved OCI IP `140.245.68.253` stay intact until a separate cutover decision.

## Required placeholders for the dark app

Downstream implementation should make the site-specific values explicit and impossible to confuse with OCI:

- Namespace: `metallb-system`.
- Address pool name: `console-onprem-ingress`.
- VIP/address placeholder: `ON_PREM_INGRESS_VIP` or `ON_PREM_INGRESS_VIP_POOL`, for example
  `10.0.0.240/32` in documentation only. Do not reuse `140.245.68.253`.
- Layer-2 advertisement name: `console-onprem-l2`.
- Optional L2 scoping placeholders: worker interface/VLAN selectors if the site fabric needs them.
- Traefik Service annotation/selection should point at the `console-onprem-ingress` pool, with
  `type: LoadBalancer`, no hostPort, and no `providers.kubernetesIngress.ingressEndpoint.ip` pin to the OCI
  address.
- Activation docs must record the real VLAN/subnet, reserved VIP, external DNS target, and the failover drill
  result before any Argo CD production wiring is allowed.

## Why not MetalLB BGP first

BGP is not required for the first activation path. It adds router peering, ASN, route-advertisement, and Cilium
coordination requirements before the repo has a concrete on-prem fabric. Choosing BGP now would couple issue
#378 to the Cilium lane unnecessarily and would make dark staging harder to review without real router inputs.

If the final site cannot provide a shared L2 segment for the workers that may hold the ingress VIP, then the
fallback is MetalLB BGP. That fallback must be coordinated with issue #377, not issue #8, and must add explicit
placeholders for local ASN, peer ASN, peer IPs, advertised VIP pool, and Cilium/BGP ownership boundaries.

## Why not kube-vip for ingress Service VIP

The available oyatie sources already use Talos-native L2 VIP semantics for the Kubernetes API/control-plane VIP.
That pattern is appropriate for the control-plane endpoint, but it does not give maintenance a ready Service
LoadBalancer implementation for Traefik. For data-plane ingress, MetalLB is simpler to stage as an Argo CD app,
has first-class `IPAddressPool`/advertisement resources, and keeps the Traefik Service shape provider-neutral.

kube-vip remains acceptable for a future control-plane/API VIP lane if the Talos/CAPI substrate needs it, but it
is not the chosen implementation for the maintenance on-prem Traefik LoadBalancer VIP.

## Argo CD dark-staging rule

The VIP provider app must live under `deploy/apps/vip-ingress/` or another dark `deploy/apps/**` path. Do not
place it under `deploy/argocd/apps/`; `deploy/argocd/root.yaml` auto-syncs only `deploy/argocd/apps`, so adding
it there would risk changing live ingress behavior. A merge of the dark app must render cleanly but must not
change the current production OCI route.

## Activation and verification gates

Before activation, downstream lanes must prove:

1. `helm template` or `kustomize build` renders the MetalLB dark app.
2. The on-prem Traefik variant renders with `Service` enabled as `LoadBalancer`, multi-replica scheduling across
   at least two workers, and no hostPort usage.
3. The OCI `oci-guest` app still renders/exists with hostPort and `140.245.68.253` unchanged.
4. In a scratch multi-node on-prem/Talos cluster, the selected VIP answers ingress traffic, the node currently
   announcing the VIP can be killed or drained, and traffic recovers on another worker.
5. If the implementation deviates to BGP, issue #377 records the Cilium/BGP coordination and the activation
   README names all router and ASN placeholders.

## Consequences for child work

- `t_51fa8f40` should stage a MetalLB L2 dark app and activation README under `deploy/apps/vip-ingress/` with
  the placeholders above.
- `t_42b699cd` should make the on-prem Traefik variant consume the MetalLB pool through a `LoadBalancer`
  Service while preserving the current OCI hostPort app.
- Live activation remains founder/operator gated on a real on-prem HA substrate and a captured failover drill.
