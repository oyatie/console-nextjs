> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Production hardening deployment-context contract

This note defines the replacement contract for `scripts/check-production-hardening.mjs` under ADR-0024 / issue #371. The gate must validate security and production-hardening properties, not assume every production path has the current OCI single-node shape.

## Decision

Use an explicit deployment-context registry, not repository inference.

Reason: ADR-0024 requires `oci-guest` and `on-prem` artifacts to coexist. The repository can legitimately contain a live OCI runbook, a DARK on-prem overlay, self-hosted storage docs, OCI object-store config, and future contexts at the same time. Inferring the active context from strings or file presence would either reject valid coexistence or silently bless the wrong topology. The script should define a small registry of supported contexts and validate every committed context by default.

Default execution should validate:

1. all portable/global checks; and
2. every context in the registry that is committed in the repository.

A future CLI/env selector may narrow a local run, but CI should keep the default `all` behavior so adding a context means adding its checks and negative tests in the same PR.

## Context registry

The first registry entries are:

| Context | Status | Canonical evidence paths | Intent |
|---|---|---|---|
| `oci-guest` | live/current production substrate | `deploy/OPS-RUNBOOK.md`, `deploy/SECRETS.md`, `deploy/apps/console/base/database.yaml`, `deploy/apps/console/overlays/prod/kustomization.yaml`, `docs/ENTERPRISE-READINESS.md` | Preserve the current Oracle Cloud Ampere A1 single-node target honestly: no false HA, OCI Vault/Object Storage are documented, CNPG remains one instance, and HA-only storage patches are not applied to the live prod overlay. |
| `on-prem-ha` | DARK/additive ADR-0024 target until operator activation | `deploy/OPS-RUNBOOK-baremetal.md`, `deploy/apps/console/overlays/on-prem/kustomization.yaml`, `deploy/apps/console/overlays/on-prem/cnpg-ha-patch.yaml`, `deploy/apps/storage/manifests/storageclass-console-pg-hot.yaml`, `deploy/apps/storage/README.md`, `deploy/apps/console/overlays/on-prem/README.md`, `deploy/apps/observability/README.md` | Stage the portable HA substrate without mutating the live OCI target: OpenBao/External Secrets, self-hosted S3-compatible storage, replicated block storage, CNPG >= 3, topology spread/failover evidence, and explicit DARK/cutover boundaries. |

Implementation shape: keep a registry constant such as `DEPLOYMENT_CONTEXTS = [{ id, status, checks }]`. Each check should return a pass/fail with the context id in its label. Avoid a flat list of OCI-specific `requireIncludes` calls with labels that imply universal production truth.

## Global portable checks

These checks are independent of whether the selected deployment context is OCI, on-prem, colo, or a future substrate. Preserve them as mandatory global checks:

1. **Gate wiring**
   - `package.json` exposes `check:production-hardening`.
   - `.github/workflows/ci.yml` runs `npm run check:production-hardening`.
   - `.github/workflows/security.yml` runs `npm run check:production-hardening`.
2. **Supply chain and release discipline**
   - Image release waits for CI success before promotion.
   - Blocking Trivy image scan remains present for HIGH/CRITICAL findings.
   - Security workflow keeps filesystem/config Trivy scanning, `cargo audit`, `cargo deny`, and `npm audit --audit-level=high`.
   - Image release keeps keyless cosign signing, SLSA/provenance attestation, SBOM/provenance artifacts where configured, and automatic digest bumping.
   - Do not treat an OCI-specific architecture string such as `linux/arm64` as globally portable. If the gate validates image platforms, make it context-aware: `oci-guest` may currently be arm64-only, while `on-prem-ha` must require an explicit multi-arch readiness decision before x86_64 hardware is activated.
3. **Immutable GitOps deployment**
   - Production overlays pin app images by sha256 digest, not mutable tags.
   - Argo roots that own live production still track `targetRevision: main`.
   - Deployment automation keeps an Argo refresh/rollout status guard. Domain names and IPs are context-specific, not global invariants.
4. **Admission and provenance policy**
   - The optional admission-audit component remains present and documents sigstore policy-controller audit/warn posture.
   - ClusterImagePolicy references the expected GHCR images, GitHub OIDC issuer, Fulcio/Rekor, and image-release workflow identity.
5. **Runtime envelope and observability contracts**
   - Backend request timeout, body limit, trace layer, metrics wiring, and route-layer tests remain present.
   - OpenSLO availability and latency files remain present.
   - Monitoring manifests keep a ServiceMonitor for `/metrics` and PrometheusRule alerts for availability/latency.
   - Self-hosted observability may be context-specific, but the app-level metrics/SLO contract is portable.

## `oci-guest` context requirements

The `oci-guest` context validates the current live Oracle Cloud guest posture. It must not be forced to satisfy on-prem HA requirements until a separate OCI paid/multi-node lane exists.

Required properties:

1. **Secret-store documentation**
   - `deploy/OPS-RUNBOOK.md` identifies this as the `oci-guest` runbook.
   - It documents OCI Vault as the recovery source for Talos/kubeconfig/app secret bundles.
   - `deploy/SECRETS.md` documents the current OCI Vault/manual Kubernetes secret bootstrap path and the External Secrets/Sealed Secrets upgrade path.
2. **Object-store endpoint and retention**
   - `deploy/apps/console/base/database.yaml` defines a CNPG Barman `ObjectStore` with `destinationPath: s3://mnt-db-backups/` and an OCI Object Storage S3-compatible `endpointURL`.
   - The credential secret remains explicit, e.g. `oci-objectstore-creds` with `ACCESS_KEY_ID` / `ACCESS_SECRET_KEY`.
   - Retention posture is explicit: either a real `retentionPolicy` is present or the file documents indefinite retention / no pruning and its storage-growth consequence.
   - OCI-specific Barman checksum workarounds may be allowed here, but must be labeled as OCI-specific and must not be required for `on-prem-ha`.
3. **Topology and HA honesty**
   - The runbook documents one Ampere A1 node, one schedulable control-plane node, the reserved OCI IP, and the free-tier guardrail that no second A1 should be run.
   - `docs/ENTERPRISE-READINESS.md` states that `oci-guest` is single-node, not automatic failover, and a node loss is restore-from-backup rather than HA.
   - Any OCI-specific MTU/NTP/Bastion guidance stays scoped to `oci-guest` docs.
4. **Database instance-count and storage shape**
   - `deploy/apps/console/base/database.yaml` keeps CNPG `spec.instances: 1` for the live single-node base.
   - The base must not pin the on-prem replicated storage class.
   - `deploy/apps/console/overlays/prod/kustomization.yaml` inherits the base and must not include `cnpg-ha-patch.yaml`, `console-pg-hot`, `/spec/instances` HA patches, or `/spec/storage/storageClass` patches.

Passing `oci-guest` means the live context is honest and safe for its current substrate. It does not mean the platform has multi-node HA.

## `on-prem-ha` context requirements

The `on-prem-ha` context validates the additive ADR-0024 DARK HA target. It must not make the live OCI path fail merely because the on-prem artifacts coexist in the repo.

Required properties:

1. **Secret-store documentation**
   - `deploy/OPS-RUNBOOK-baremetal.md` identifies OpenBao as the on-prem secret root and External Secrets Operator as the Kubernetes projection path.
   - The docs name OpenBao initialization/unseal/audit/backup expectations and forbid committing or pasting unseal/root material.
   - The on-prem path must not require OCI Vault. OCI Vault may appear only as the previous/rollback `oci-guest` context.
2. **Object-store endpoint and retention**
   - The on-prem runbook documents SeaweedFS as the accepted self-hosted S3-compatible reference and requires a newer accepted decision plus fresh security/readiness evidence for an engine change.
   - It requires configuring CNPG Barman and evidence storage endpoint URLs, credentials from OpenBao/ESO, bucket names, TLS CA material as needed, and retention/replication policy before production data moves.
   - It requires evidence/WORM or backup replication to a second physical site or equivalent independent failure domain before claiming durable HA/DR retention.
   - It explicitly warns not to copy the OCI `AWS_*_CHECKSUM_*=when_required` workaround blindly to self-hosted S3.
3. **Topology HA posture**
   - The runbook requires three control-plane/etcd members, dedicated workers/storage failure domains before HA claims, a stable API endpoint/VIP, site NTP, real fabric MTU, and no OCI IP/hostPort assumptions.
   - VIP/ingress failover evidence is required before traffic cutover.
   - DARK boundaries are explicit: on-prem apps/overlays are not wired into live Argo CD until founder/operator activation.
4. **Database instance-count and replicated storage**
   - `deploy/apps/console/overlays/on-prem/kustomization.yaml` inherits `../../base` and applies `cnpg-ha-patch.yaml` to `Cluster/console-db`.
   - `cnpg-ha-patch.yaml` sets `/spec/instances` to at least `3`.
   - The patch sets `/spec/storage/storageClass` to the canonical replicated storage class, currently `console-pg-hot`.
   - The patch includes synchronous replication / failover posture, and either anti-affinity or topology spread by hostname or stronger failure-domain labels.
   - `deploy/apps/storage/manifests/storageclass-console-pg-hot.yaml` defines `StorageClass/console-pg-hot`, uses a replicated provisioner such as Longhorn (`driver.longhorn.io` for the current DARK lane), has at least three replicas, `Retain`, and `WaitForFirstConsumer`.
5. **Observability and SLO activation**
   - The on-prem observability docs stage self-hosted telemetry (OTel plus metrics/logs/traces backend) or explicitly explain the selected alternative.
   - The app overlay that enables on-prem observability must preserve the portable `/metrics`/OpenSLO contract rather than replacing it with provider-specific monitoring.

Passing `on-prem-ha` means the repo has an implementation-ready staged HA context. It does not authorize live cutover without operator activation and drill evidence.

## Failure model and implementation guidance

The refactor should preserve a fail-closed gate:

1. A missing required evidence file is a failure for that context.
2. A context-specific string is allowed only inside that context's checks and labels.
3. Context checks should validate properties, not prose alone, whenever the property is machine-readable:
   - parse CNPG `instances` and require exact `1` for `oci-guest`, `>= 3` for `on-prem-ha`;
   - parse JSON6902 patch values for `/spec/instances` and `/spec/storage/storageClass`;
   - parse `StorageClass` name/provisioner/replica count;
   - count digest pins and reject mutable `newTag` in live overlays;
   - verify Argo `targetRevision: main` in live roots.
4. Prose checks remain acceptable for operator-only evidence, but label them as documentation evidence and keep them scoped, e.g. `oci-guest secrets runbook: OCI Vault` or `on-prem-ha secrets runbook: OpenBao`.
5. Add negative unit tests for the helper-level checks. Minimum cases:
   - `oci-guest` fails if base CNPG is changed to `instances: 3` or prod loads the on-prem storage patch;
   - `on-prem-ha` fails if the patch sets fewer than three instances;
   - `on-prem-ha` fails if storage uses `local-path` or fewer than three storage replicas;
   - global checks still fail on missing digest pins or mutable tags.
6. Keep implementation output grouped by `global`, `oci-guest`, and `on-prem-ha` so downstream operators can see which substrate failed instead of reading an OCI-only wall of assertions.

## Downstream acceptance checklist

A downstream implementation card is ready to complete when:

- `npm run check:production-hardening` passes and its output identifies global plus context-specific checks.
- The script no longer treats OCI-only strings like `OCI Vault`, `VM.Standard.A1.Flex`, `never run a second A1`, `OCI Object Storage`, `instances: 1`, `local-path`, or `linux/arm64` as universal production requirements.
- The script still fails on missing digest pins, mutable image tags, missing `targetRevision: main`, missing cosign/Trivy/provenance gates, missing OpenSLO files, and missing monitoring manifests.
- `oci-guest` and `on-prem-ha` each have at least one positive and one negative helper/unit test.
- The implementation note here is cited or superseded by a more specific ADR/spec before future contexts are added.
