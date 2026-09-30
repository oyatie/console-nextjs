---
id: ADR-0024
status: accepted
doc_status: published
date: 2026-07-13
proposed_date: 2026-07-09
owner: jasonlee
decision: self-host-first-portable-seams
amends: [ADR-0005, ADR-0015, ADR-0019]
related: [ADR-0005, ADR-0015, ADR-0019, ADR-0021, ADR-0022]
---

# ADR-0024 — Self-Host-First, Cloud-Portable Multi-Substrate + High Availability

Status: **Accepted 2026-07-13 — self-host first, portable core, provider-native adapters** · **The OCI Talos cluster running today stays supported during the transition** · Oyatie Cloud, AWS, OCI, Azure, and GCP remain first-class target contexts, but context-specific parity may follow the fully working self-host reference stack · Amends ADR-0005 (storage/WORM), ADR-0015 (DR), ADR-0019's OCI-first mailbox deployment envelope (not its build-vs.-adopt decision), and the deployment-context scope of `docs/specs/log-persistence.md` · Adopts applicable patterns from the sibling **oyatie** project only where this local ADR names them.

## Context

The platform must first work end to end as an **owner-controlled self-hosted stack**: its required compute, storage, networking, secrets, observability, ADR-0022 local product identity, workload identity, backup, and recovery path must run without a mandatory provider-managed service. The same reference stack must be deployable on owned bare metal or on generic compute/Kubernetes in any cloud. This self-host proof is the delivery prerequisite for claiming the platform is portable.

Oyatie Cloud, AWS, OCI, Azure, and GCP remain **first-class target contexts**. “Cloud-agnostic” does **not** mean refusing their native features or reducing every context to a lowest-common-denominator deployment. It means the portable application/control core reaches hosting/provider capabilities through explicit ports and context-scoped adapters. A cloud context may use its object store, KMS, load balancer, managed database, telemetry, workload identity, or other native services where useful, so long as those choices remain optional, replaceable, and do not become requirements of the self-host reference path. Product-user authentication/federation is outside this ADR and remains governed by ADR-0022; workload identity must not create a speculative external IdP seam.

The OCI Talos cluster running today remains a supported live context during this work. This ADR does not require its immediate migration or removal. It changes delivery order: **prove the fully working self-host reference first; then complete and optimize provider-specific contexts and adapters**.

A four-part audit established the exact shape of the problem:

- **The app/runtime is OCI-SDK-free, but not yet provider-neutral at every capability.** No OCI SDK, `oracle` crate, OCI Vault SDK, instance-principal authentication, or hardcoded tenancy/region dependency was found in the backend runtime (`backend/Cargo.lock` is clean). The current object-storage boundary is explicitly S3-specific—`S3StorageConfig`, `S3ObjectStore`, SigV4, bucket semantics, and S3 error types—although its endpoint is configurable and already serves SeaweedFS/OCI-compatible endpoints. OCI references also remain in metadata, comments, tests, configuration, and repository gates. This is strong OCI decoupling and a usable S3 adapter, not evidence that native GCS or Azure Blob can plug in without the provider-neutral capability port required below.
- **The CI/CD pipeline is cloud-provider-portable but not yet self-host recovery-independent.** Images build → **GHCR** (not OCIR), digest-pinned + cosign; deploy is **GitOps-pull via ArgoCD**; no `oci` CLI, tenancy, or instance-principal exists in the workflow path, and cert-manager uses portable DNS-01. However GHCR plus GitHub/Fulcio/Rekor are hosted dependencies, so the self-host reference still needs the owner-controlled mirror and trust/restore proof required below.
- **Most hard provider lock is in the *substrate*, with a smaller application-boundary repair still required.** `deploy/opentofu/**` is ~20 `oci_*` resources on the `oracle/oci` provider (compute/VCN/reserved-IP/LB/bastion/object-storage); secrets are OCI Vault; backup + evidence endpoints are OCI Object Storage. At the application edge, the S3-specific storage implementation must become one adapter behind a provider-neutral object-storage capability before native non-S3 providers can satisfy this ADR without a core fork.
- **HA is absent by design.** Single Talos node, single control plane (no etcd quorum), `instances: 1` Postgres on unreplicated node-local `local-path`, reserved single-IP `hostPort` ingress. The previously identified `mail_sync` duplicate-claim defect has an implemented lease/fencing remediation, but real multi-replica HA remains unverified (below).

**oyatie provides adjacent evidence for this pattern, not Maintenance authority.** The reviewed snapshot `oyatie origin/dev@73fea9ffc3d48f331f7cb6f086657a9d7b4a096b` (inspected 2026-07-13) uses a Talos + Cluster API + ArgoCD fleet substrate, stable capability ports, and context-scoped infrastructure adapters. Its accepted ADR-0240 clarifies the older ADR-0173: provider SDKs stay out of business logic, while AWS/GCP/Azure and regional-provider IaC may use native managed capabilities behind one canonical interface. This Maintenance ADR adopts only the patterns named below; every copied component still requires repository-local validation and licensing/security review.

## Decision

**The self-host reference is the portability baseline; cloud-native capabilities are replaceable context adapters, not forbidden features.** Concretely:

1. **Self-host-first delivery gate.** Before hosted/provider-parity work is required, the owner-controlled reference stack must work end to end on self-hosted infrastructure and pass its application, security, backup/restore, observability, and HA evidence gates. The reference may run on bare metal or generic cloud VMs/Kubernetes, but it may not require Oyatie Cloud or AWS-, OCI-, Azure-, or GCP-managed services.
2. **Deployment-context IaC layout.** OpenTofu modules live under `<context>/<primitive>` where contexts include `self-host`, `oyatie-cloud`, `aws`, `oci`, `azure`, `gcp`, and future substrates. Services select context-scoped primitives through thin wrappers. This is a local rule consistent with the accepted provider-module boundary in oyatie ADR-0240. Today’s `oci-guest` stack remains supported while the `self-host` reference is completed. Hosted/provider contexts are first-class destinations, not first-in-sequence delivery blockers.
3. **Ports/adapters rule.** Application and control-plane code depend on capability contracts—such as object storage, key management, load balancing, database, workload identity, messaging, and telemetry—not directly on a provider SDK or resource model. Each context implements those contracts with an independently testable adapter. The self-host implementation is always available as the reference adapter. Product-user identity remains the local passkey-backed contract in ADR-0022.
4. **Context-native capabilities are encouraged at the edge.** Oyatie Cloud and the AWS, OCI, Azure, and GCP adapters may use their native managed services and differentiated capabilities. They need not emulate identical infrastructure or avoid valuable platform features. The boundary is that context-specific configuration, credentials, SDKs, failure semantics, and resources stay inside the relevant adapter/context, with capability and behavioral conformance tests at the port.
5. **No mandatory cross-context managed dependency.** A provider-managed service may optimize a cloud context, but it may not become a prerequisite for the portable core or the self-host reference. If a provider exposes a unique optional capability, surface it as an explicit optional extension rather than silently weakening or contaminating the common contract.
6. **Mechanically enforced.** Deployment-context, dependency, and vendor-boundary gates reject provider primitives leaking into portable application manifests/core modules, require adapter registration and conformance evidence, and validate each committed context honestly. Portability enforcement must not make legitimate provider-specific resources illegal.

## Reference architecture — fully working self-hosted HA stack first

> The “Today” column records the current **`oci-guest`** deployment, which remains supported. The right column is the first delivery target: the provider-independent **`self-host`** reference, initially realized by the repository’s `on-prem`/`on-prem-ha` artifacts. Once that reference works end to end, Oyatie Cloud, AWS, OCI, Azure, and GCP contexts may substitute context-native adapters and add differentiated capabilities without changing the portable core.

| Concern | Today (OCI `oci-guest`, single-node) | First delivery gate (`self-host`, initially bare-metal `on-prem`, HA, multicluster) |
|---|---|---|
| Cluster lifecycle | 1 `oci_core_instance` + `dd` flasher hack | **Talos install-media + Cluster API** (+ `cluster-api-provider-metal3`) + per-cell ArgoCD; **3 control-plane nodes** (etcd quorum) + N workers |
| Provisioning | `oracle/oci` provider, `~/.oci/config` | `on-prem` OpenTofu wrappers + Talos machineconfig against a node inventory; no cloud IaaS provider |
| CNI / policy | flannel (NetworkPolicies inert) | **Cilium** (enforced NetworkPolicy + eBPF LB, optional BGP) |
| Ingress | reserved single IP `140.245.68.253` + `hostPort` DaemonSet | **MetalLB (L2/BGP) or kube-vip VIP** fronting Traefik across ≥2 workers; re-enable the Service; VIP fails over |
| Block storage | node-local `local-path` (1 copy) | **Longhorn or Rook-Ceph** replicated PVCs |
| Postgres | CNPG `instances: 1` | **CNPG `instances: 3`** synchronous replication + auto-failover + pod anti-affinity |
| Object storage | OCI Object Storage endpoints through the current S3 adapter | **self-hosted SeaweedFS** through the S3 adapter for the first reference; provider-neutral object-storage port plus conformance suite before native non-S3 adapters; evidence WORM replica → **a second physical site** |
| Secrets | OCI Vault + out-of-band `kubectl create secret` | **OpenBao (HA Raft) + External Secrets Operator**, declaratively reconciled by Argo (lift oyatie `infra/external-secrets` + `infra/kms/openbao.k8s.yaml`) |
| Multi-cluster | single destination | **ArgoCD ApplicationSet** cluster-generator; primary + warm-standby DR per site |
| Observability | OCI services remain allowed through the OCI telemetry adapter | **OTel Collector → VictoriaMetrics/Mimir + Loki + Tempo → Grafana**, self-hosted; cloud contexts may later replace downstreams through the telemetry port without invalidating this reference or `log-persistence.md`’s scoped `oci-guest` direction |
| Images | single-arch `linux/arm64` (Ampere) | **multi-arch `linux/amd64,linux/arm64`** |
| Time / MTU | OCI link-local NTP `169.254.169.254`; MTU 9000→1500 VNIC workaround | on-prem NTP (chrony/GPS); MTU = real fabric value |
| Registry | GHCR for the supported `oci-guest` context | Owner-controlled **Harbor** (or equivalent OCI registry) containing every required digest; GHCR may be an upstream source, but self-host bootstrap and recovery must succeed while GitHub/GHCR are unavailable |

## Portability-specific app remediation identified by the audit

**`backend/app/src/mail_sync.rs` — remediation implemented; multi-replica proof still required.** Migrations `0116_comms_email_account_sync_lease.sql` and `0117_comms_email_account_claim_token_fencing.sql` add `FOR UPDATE SKIP LOCKED` claiming, an expiring `claimed_until` lease, and a per-claim fencing token. The Postgres adapter compare-and-clears only the live token, and the scheduler bounds each sync within the lease. These controls address the previously identified duplicate-claim defect in code, but they are not evidence that the whole application is horizontally safe. Before scaling API or worker replicas, the exact composed candidate must prove concurrent claimers, process death, timeout, lease expiry/reclaim, stale-token release rejection, and duplicate-side-effect prevention under real multi-replica operation.

## Directional remediation roadmap

Implementation status and priority are revision-bound planning evidence maintained separately under `docs/program`, not facts established by this directional list. Several substrate artifacts are staged/DARK; none becomes live HA evidence without activation proof.

1. **Define and gate the portable capability seams** — enumerate required ports, adapter ownership, self-host reference implementations, provider extension points, and conformance tests. Start with existing S3-compatible storage and environment-based secrets boundaries; do not invent indirection where no provider boundary exists.
2. **Complete portable self-host secrets** — OpenBao + External Secrets Operator; lift oyatie `infra/external-secrets/*` (including the `disable_local_ca_jwt` + `system:auth-delegator` invariant). This removes the out-of-band/provider secret dependency from the reference path.
3. **Complete the self-host object-store path** — point CNPG Barman and evidence storage through the current S3 adapter at SeaweedFS; prove retention, WORM behavior, restore, and independent-site replication. Keep OCI Object Storage active for `oci-guest`. Introduce the provider-neutral object-storage capability/conformance contract before adding native GCS, Azure Blob, or another non-S3 adapter; do not copy endpoint workarounds without endpoint-specific proof.
4. **Owner-controlled registry and provenance** — mirror every required immutable image digest into Harbor (or an equivalent self-hosted OCI registry); establish a self-host-verifiable signature/provenance trust root and admission policy; and prove bootstrap/restore with GitHub, GHCR, Fulcio, and Rekor unavailable. GHCR/GitHub remain valid for `oci-guest` and normal development but are not recovery prerequisites for the self-host reference.
5. **Multi-arch image builds** — extend `image-release.yml` `platforms:` to `linux/amd64,linux/arm64` so the reference is not tied to OCI Ampere.
6. **Self-host substrate and HA proof** — complete `on-prem` deployment-context OpenTofu wrappers + Cluster API + metal3, then prove a 3-node control plane, replicated storage, CNPG `instances: 3`, MetalLB/kube-vip VIP, Cilium, multi-cluster ApplicationSet, failover, and recovery. This is the first end-to-end portability gate.
7. **Self-host observability and operations** — OTel + VictoriaMetrics/LGTM + Grafana, operator runbooks, alerts, upgrade/rollback, backup, and disaster recovery without a cloud-managed dependency.
8. **`mail_sync` multi-replica verification** (above) and remediation of any other application defects exposed by multi-replica operation.
9. **Context-aware CI hardening** — validate `oci-guest` honestly as single-node, require the full self-host HA topology for `on-prem-ha`, and add port/adapter conformance plus dependency-boundary checks.
10. **Hosted/provider contexts after the reference gate** — finish the Oyatie Cloud, AWS, OCI, Azure, and GCP adapters and IaC using native object storage, KMS, load balancers, databases, telemetry, workload identity, or other differentiated capabilities where they improve the context. Each adapter must pass the portable capability contract plus its context-specific tests; simultaneous feature parity is not required before self-host completion. Product-user federation remains blocked by ADR-0022 unless a later accepted local ADR names a real integration.
11. **Doc/ADR alignment** — maintain context-specific runbooks and evidence, amend single-substrate assumptions, and keep the existing OCI deployment documented and supported throughout the transition.

## Adopted pattern sources from oyatie (full-license reuse; repo-relative symbols)

At the reviewed `oyatie origin/dev@73fea9ffc3d48f331f7cb6f086657a9d7b4a096b` snapshot, applicable lift-and-validate candidates include: `infra/external-secrets/*` + `infra/kms/openbao.k8s.yaml` (secrets); `libs/oya-check-iac-tier-discipline/src/lib.rs` (CI gate, pure std); `infra/observability/observability.k8s.yaml` (OTel→VictoriaMetrics→Grafana); `infra/seaweedfs/seaweedfs.k8s.yaml`; `infra/gitops/{root-app.yaml,values.yaml}` (app-of-apps + sync-waves). Those files are bounded implementation evidence; their existence does not promote an Oyatie proposal or prove Maintenance readiness. Patterns adopted for local validation are Talos-media + CAPI + per-cell ArgoCD (oyatie ADR-0375), ApplicationSet multi-cell federation (oyatie ADR-0171), the stable-interface/transitional-adapter rule (oyatie ADR-0520), the `core/ports/adapters/facade` capability shape (oyatie ADR-0562 as amended by ADR-0615), and the provider-edge rule from oyatie ADR-0240: declare native extensions, keep them inside their context adapter, and reject leakage into business logic/the portable core. **Do not** import oyatie's cloud-first sequence, eventual provider phase-out requirement, proposed records, or `cloud/cloud-kernel` (an unrelated OS microkernel).

## Adjacent-project authority boundary

Maintenance user directives and accepted local ADRs remain authoritative. When local intent is genuinely ambiguous, resolve the then-current Oyatie `dev` commit, record that exact revision, start at its `specs/root-hub-pointers.json`, follow every authority entry required by `agent_quick_start_protocol.step_1_read_authority`, then inspect the nearest relevant accepted ADR and every higher accepted amendment touching the scope. An Oyatie record becomes Maintenance direction only when a local accepted ADR adopts a named rule.

Maintenance deliberately does **not** inherit Oyatie's cloud-first delivery sequence. Maintenance proves the self-host reference first. Oyatie Cloud is the north-star hosted context after that contract passes: it must run Maintenance as an ordinary deployable workload without a core fork, while any Oyatie Cloud-native capabilities remain behind the same context adapter and optional-extension rules as hyperscaler-native capabilities.

## Consequences

- **Self-hosted operation is the proof of portability**, not merely a fallback. It provides owner control and data-sovereignty options while allowing the same stack to run on generic cloud compute.
- **Oyatie Cloud, AWS, OCI, Azure, and GCP remain first-class and can be excellent at their own strengths.** Their native services can reduce operational burden or add capabilities, but adoption occurs through explicit adapters rather than core coupling.
- **No lowest-common-denominator mandate.** Portable capability contracts define the minimum behavior the product depends on; provider adapters may expose explicit optional extensions and stronger guarantees.
- The existing OCI deployment remains supported during transition; postponing full provider-adapter parity until the self-host gate is deliberate sequencing, not deprecation.
- Portability-specific effort is concentrated in the substrate. The audit identified remaining `mail_sync` multi-replica verification plus configuration/endpoint work; that does not waive unrelated application correctness blockers.
- Reusing named Oyatie mechanisms may reduce duplicated design effort, but only local validation, licensing/security review, conformance tests, and failure drills can establish a Maintenance risk or delivery benefit.
- Once the roadmap's deployment-context, provider-boundary, and adapter-conformance gates are complete, unintended universal OCI (or any single-cloud) coupling must fail CI while valid context-specific provider resources remain allowed. Today's narrower checks do not yet prove that property.

## Non-goals / open questions

- Exact self-host node inventory (count, arch, per-site layout) and the storage backend choice (Longhorn vs Rook-Ceph) are sizing decisions for the substrate lane.
- The exact Oyatie Cloud, AWS, OCI, Azure, and GCP native feature sets are deferred until the self-host reference passes its end-to-end gate; their adapter contracts must preserve the accepted seams.
- Whether to reuse oyatie's crates as a shared workspace dependency vs copy-in is decided per lane (some carry oyatie-specific kernel deps).
- Sharding/residency is deferred until multicluster is live and a local decision defines it.
