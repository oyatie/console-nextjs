# Durable Log Persistence — Design + GO/NO-GO

> **CURRENT OCI-SCOPED DESIGN; AMENDED BY [ADR-0024](../decisions/ADR-0024-bare-metal-portability-and-ha.md):** this remains valid for `oci-guest`. ADR-0024 makes the self-hosted telemetry stack the first portable reference implementation, not a ban on managed observability elsewhere. Oyatie Cloud, AWS, OCI, Azure, and GCP contexts may use native telemetry behind the portable collection/SLO contract and explicit optional extensions; the collection/pipeline rationale, audit-chain boundary, and historical reasoning remain useful across contexts.

Author: platform. Last grounded against repo: branch `perf/hr-read-path-indexes-*`, deploy tree under `deploy/`.

> This document is for **operational logs** (observability / incident response). It is **not** the
> tamper-evident business **audit chain** (`backend/crates/platform/audit-chain/`, Postgres
> `audit_events`, sealed + verifiable). See §1 for the boundary. Operational logs may *reference*
> `trace_id` / audit ids for correlation, but they are a separate, lower-assurance durability concern
> and must never be treated as the legal record.

> **Verified OCI free-tier limits (2026)** — confirmed against Oracle's Always-Free / service pricing
> docs + InfoQ coverage, July 2026. These numbers are load-bearing for every "$0" claim below; re-verify
> at implementation and set the alarms in §5/§8 as the runtime canaries.
> - **OCI Logging:** free **10 GB/month** ingest (shared across the tenancy), then **$0.05/GB**.
> - **OCI Monitoring:** free **500 M ingestion + 1 B retrieval data points / month**.
> - **OCI APM:** free **1000 tracing events / hour** → we **head-sample** to stay under it.
> - **Object Storage:** free **20 GB + 50 k requests / month** (already contended by evidence + DB backups).
> - **Ampere A1 Always-Free was HALVED to 2 OCPU / 12 GB on 2026-06-15** (was 4/24). A running VM is not
>   hot-shrunk, so the **live node is likely grandfathered at 4/24**, but **a rebuild or DR yields only
>   2/12** — treat 2/12 as the resilience floor the design must survive (see §5 fold-the-gateway note, §9).

---

## 1. Context & problem

**Today, pod logs are ephemeral.** The backend emits well-structured logs but nothing captures them
off-node:

- `backend/app/src/lib.rs:2107` `init_tracing` builds `tracing_subscriber::fmt::layer().json()` +
  `EnvFilter` (default `info`, `:2108-2109`) → **structured JSON to stdout**. That is the entire log sink.
- Every HTTP request span records a `trace_id` (`record_otel_ids`, `backend/app/src/lib.rs:2006`, called
  from the `TraceLayer` closures at `:1278`/`:1289`; layer built by `http_trace_layer` at `:1255`), so
  **every log line is already correlatable by `trace_id`** and the query string is deliberately dropped to
  avoid PII (`:1282`).
- An **OTLP exporter path exists but is OPT-IN and currently OFF**: `init_tracing` wires
  `opentelemetry_otlp` **only** when `config.otlp_endpoint` is set (`if let Some(endpoint) = …`,
  `lib.rs:2111`; parsed from `OTEL_EXPORTER_OTLP_ENDPOINT` at `lib.rs:365`). The prod ConfigMap keeps it
  commented out and notes "the OTel collector MUST be deployed first"
  (`deploy/apps/console/base/configmap.yaml:62-65`). This exporter ships **traces (spans), not logs** —
  logs stay stdout-only. **Direction A flips this exporter ON and points it at the OTel agent → OCI APM
  (§5).**
- **No log shipper exists.** Grep of `deploy/` for `loki|vector|fluent|promtail|openobserve|opensearch|
  fluentbit|filebeat|logstash|elasticsearch|otel` returns nothing. Confirmed: pod stdout is written to the
  node by the container runtime and rotated/discarded. A pod restart, eviction, or node loss
  **permanently loses its logs.**

**The gap:** no durable retention, no cross-pod search, no post-mortem forensics beyond `kubectl logs`
on a still-running pod. For a multi-tenant operations platform this is both an **incident-response**
hole (can't reconstruct what happened after a crash) and a **compliance** hole (no retained access
record — see §2).

**Boundary with the audit chain (explicit):** the audit chain records *business* facts ("admin X
suspended tenant Y") as sealed, hash-chained `audit_events` in Postgres for legal/tamper-evidence.
Operational logs record *system* facts ("request Z returned 500 in 4.1s on pod P") for debugging and
ops. They overlap only via `trace_id`: an operational log line can point you at the audit event for the
same request, and vice-versa. They have different retention, different assurance, different stores.
**Do not route audit events through the log pipeline, and do not treat logs as audit evidence.**

---

## 2. Requirements

| # | Requirement | Driver |
|---|---|---|
| R1 | **Durable retention** off-node; survives pod/node loss | incident response; small cluster |
| R2 | **Retention tiers** (see below) | cost + compliance |
| R3 | **Search / query** across pods and time by `trace_id`, org, level, message | forensics |
| R4 | **Low ops burden** — installs/upgrades via GitOps, near-zero day-2 toil | 1–2 person team |
| R5 | **Cost-conscious** — must live inside the OCI free tier (§8), not cannibalize evidence storage | Always-Free |
| R6 | **Tenant awareness** — logs carry `org_id`; operator can filter per-tenant during an incident | multi-tenant platform |
| R7 | **PII / secret redaction at source** | Korean PIPA; existing discipline |
| R8 | **Encryption at rest** | PIPA safety measures; OCI default |
| R9 | **Access-controlled + audited** log access (who queried what) | least privilege |
| R10 | **GitOps-deployable** (ArgoCD app) + **IaC** (OpenTofu) | matches existing deploy model |
| R11 | **log ↔ trace ↔ audit correlation by `trace_id`** | cross-store forensics |
| R12 | **Three pillars** — logs, metrics, traces — under **one pane** | solo operator; single glass |

**Retention tiers (proposed, justify against PIPA):**

- **Hot / searchable (Loki on a block-volume PVC): full stream, ~30 days default (bounded by PVC size).**
  Covers the practical incident-response + SLO-forensics window. Fast LogQL; the operator's working store.
- **Durable managed (OCI Logging): full, unfiltered, up to 180 days** (OCI Logging's max retention).
  Independent of Loki — a Loki loss does not touch this copy.
- **Cold / compliance (Object Storage via Service Connector): the 180 d → 365 d tail** as compressed
  objects (query-on-demand, not indexed).
- **> 365 days: deleted** by lifecycle policy, unless a specific legal hold applies. Logs are **not** the
  long-term legal record — that is the audit chain, which has its own (indefinite) retention.

*PIPA rationale (design input — confirm with counsel, not legal advice):* Korea's 개인정보 보호법 and the
개인정보의 안전성 확보조치 기준 (Personal Data Safety Measures) require **access records to personal-information
systems be retained ≥ 1 year** (≥ 2 years for large-scale / sensitive-info processors). Our operational
logs are PII-minimized (§6) so most lines are not "access records," but request/access lines that touch
personal-data processing plausibly fall in scope → the **365-day floor** (OCI Logging ≤180 d + Object
Storage tail) satisfies the 1-year access-log minimum without over-retaining.

---

## 3. Options (tradeoffs for THIS stack)

Fixed constraints that dominate every option: **tiny compute** (the A1 node, likely 4 OCPU/24 GB today but
**must survive a 2 OCPU/12 GB rebuild**, plus the two **1 GB Always-Free AMD micros**); **OCI's managed
free tiers** (Logging 10 GB/mo, Monitoring 500 M pts, APM 1000 events/hr — see callout); **single-founder
ops**; **Object Storage free budget (20 GB / 50k req-mo) already contended** by evidence + DB backups (§8).

Two axes: **(1) the log store**, and **(2) the collector**.

**Log store:**

| Option | Ops burden | Cost | Query UX | OCI fit | Verdict |
|---|---|---|---|---|---|
| **(a) Self-hosted Loki (block-volume PVC) + OCI Logging, both full** ✅ | Med — Loki + Grafana | **$0** (block storage unmetered like Logging; Logging <10 GB/mo free) | LogQL (Grafana) + OCI console | native managed pillars | **CHOSEN — full logs in BOTH free tiers, no filtering** |
| (b) OpenObserve (single binary, S3-backed, built-in UI) | Low — 1 component | Low | built-in SQL/full-text | S3 (checksum caveat) | **Lost:** own store + AGPL + S3-checksum on the hot path; Loki+Grafana also anchors metrics+traces under one pane (R12) |
| (c) Drop Loki entirely — OCI Logging only | Near-zero | $0 <10 GB/mo | OCI console search (basic) | native | **Lost:** OCI console search is weak for LogQL-style forensics and couples the hot path to a metered tier; a runaway loop could blow the 10 GB cap with no fast local buffer |
| (d) ELK / OpenSearch | ❌ REJECTED | ❌ | rich | S3 snapshots | **Rejected:** JVM ≥2 GB heap, wants ≥3 nodes; starves a 12 GB node. Named and out. |

**Collector:**

| Option | Footprint | Verdict |
|---|---|---|
| **OTel Collector (DaemonSet agent + gateway)** ✅ | ~100–150 MB tuned | **CHOSEN — one tool for logs *and* the app's traces; native OCI APM/OTLP path; no second agent** |
| Fluent Bit | ~20 MB | **Fallback only** — the even-lazier collector if a 1 GB micro OOMs the OTel agent; swap-in is a DaemonSet change, not a redesign |
| Vector | ~100 MB | Not chosen — comparable footprint to OTel but a second, non-OTLP-native tool; OTel is one pipeline for all three pillars |

**Reject (d) ELK/OpenSearch:** JVM, ≥2 GB heap per node minimum, ≥3 master-eligible nodes for real HA,
Logstash is a memory hog, ILM is a day-2 job. On a 12–24 GB cluster it would starve the app. Disqualifying.

**Versions (indicative — VERIFY LIVE + pin the image digest at implementation; do not trust these numbers):**
OpenTelemetry Collector Contrib **latest** (filelog receiver, k8sattributes, `probabilistic_sampler`,
`transform`/`redaction` processors, `otlphttp` + `loki`/`otlp` exporters). Grafana **latest** + Grafana
**Loki v3.x** (native OTLP ingest; the standalone `loki` collector exporter is deprecated — prefer
`otlphttp` → Loki's OTLP endpoint). Fluent Bit 3.x/4.x (fallback collector only). Every one must be
renovate-pinned exactly as the repo already pins CNPG/Barman/Argo (`deploy/README.md` table).

---

## 4. Recommendation

**Direction A — all-OTel, no-filter.** Collect with the **OpenTelemetry Collector** (a DaemonSet agent on
every node → a gateway), store the **full, unfiltered** log stream in **BOTH** free tiers simultaneously —
self-hosted **Loki** on a durable **OCI block-volume PVC** (fast LogQL, the load-bearing hot store) **and**
**OCI Logging** (managed durable retention) — and unify all three pillars under **Grafana**:

- **Logs:** Loki (hot/full) + OCI Logging (durable/full). No filtering — see below.
- **Metrics:** OCI Monitoring (500 M free ingestion pts).
- **Traces:** OCI APM, **head-sampled** to stay under 1000 events/hr, fed by the app's **existing** OTLP
  exporter (`lib.rs:2111`) — flipped on and pointed at the OTel agent.
- **One pane:** Grafana with Oracle's OCI datasources (OCI Logging + OCI Monitoring + OCI APM) alongside
  the native Loki datasource.

**Why full-and-unfiltered (the key call):** block storage is **not metered like Logging ingest**, so the
Loki copy of the *entire* stream is effectively free; and at this platform's volume the same full stream
stays **comfortably under the 10 GB/mo OCI Logging free ceiling**, so the managed copy is free too. Full
logs therefore land in **both** free tiers with **no compliance-subset filtering to reason about** — a
strictly simpler design. The only guard needed is an **OCI Monitoring alarm at ~8 GB/mo** as a canary
against a runaway log loop (see §5/§8).

**Why OTel for collection (not Fluent Bit, not the app, not Vector):**
- **One tool for all three pillars.** The OTel agent tails container stdout (filelog receiver) **and**
  receives the app's OTLP traces, then forwards both to the gateway. No separate log agent + trace path.
- **Not the app alone:** app-side push would miss non-app pods (Traefik, Argo, CNPG, the DB) and can't
  capture a crash-looping pod's dying breath. A **node-level DaemonSet tailing `/var/log/pods/*` captures
  everything** — the standard decoupled approach.
- **App already emits JSON with `trace_id`** → the filelog receiver + `transform` processor parse it to
  structured attributes and attach k8s pod/namespace metadata (`k8sattributes`). No Grok, no regex.
- **Fluent Bit is the fallback**, named only for the case where a 1 GB micro OOMs the OTel agent (§9 R-2).

**Named store fallback:** if self-hosted Loki proves too heavy even hard-tuned, **OpenObserve** (single
binary, built-in UI) is the drop-in store swap — the OTel collector + OCI pillars + Vault/Argo/Tofu
plumbing are identical, so it is a store swap, not a redesign. Dropping Loki entirely and relying on OCI
Logging alone (option 3c) is the more drastic fallback (weaker search, metered hot path) — avoided.

---

## 5. Architecture

```
   A1 node (4/24 today, design for 2/12)        Micro 1 (1 GB free)         Micro 2 (1 GB free)
  ┌───────────────────────────────────┐       ┌──────────────────┐       ┌────────────────────┐
  │ app / web / worker / traefik /     │       │ OTel gateway     │       │ Loki               │
  │ argo / cnpg  (+ OTel agent pod)    │       │  (deduped, head- │       │  hot + FULL stream │
  │                                    │  OTLP │   sampled)       │ OTLP  │  on block-vol PVC  │
  │ OTel Collector DaemonSet (agent)   │ ────► │       +          │ ────► │  (WAL on PVC)      │
  │  • filelog: /var/log/pods/*        │       │ Grafana (single  │       └────────────────────┘
  │  • otlp: app traces (lib.rs:2111)  │       │  pane, ingress   │              also:
  │  • k8sattributes + transform/redact│       │  logs.knllogistic│   ┌──────────────────────────┐
  └───────────────────────────────────┘       │  .com, TLS+auth) │──►│ OCI Logging (FULL,       │
   (DaemonSet also runs on both micros)        └───────┬──────────┘   │  UNFILTERED, ≤180 d)     │
                                                       │              │   │ 8 GB/mo alarm canary  │
                            OCI datasources ◄──────────┤              │   ▼                       │
                     (Grafana reads all four)          ├──────────────►│ Service Connector →      │
                                                       │  metrics     │  Object Storage (180d→1yr)│
                                                       ├──────────────►│ OCI Monitoring (500M pts)│
                                                       │  head-sampled └──────────────────────────┘
                                                       └──────────────► OCI APM (<1000 events/hr)
```

**Two-micro layout (offloads observability RAM off the app node):**
- **OTel Collector DaemonSet (agent)** — one pod per node (A1 + both micros). Reads `/var/log/pods/*`
  (hostPath, read-only) via the **filelog receiver**, and exposes an **OTLP receiver** for the app's
  traces. `k8sattributes` + a `transform`/`redaction` processor enrich + scrub, then export OTLP to the
  gateway. ~100–150 MB tuned (mem-limiter processor on). *Fluent Bit is the fallback if a micro OOMs.*
- **Micro 1 — OTel gateway + Grafana.** The gateway concentrates all node agents, applies the
  **head-sampling** processor (traces) and batching, and fans out to the exporters. **Grafana** is the
  always-on single pane behind Traefik ingress `logs.knllogistic.com` (cert-manager TLS + auth, §6).
- **Micro 2 — Loki** (load-bearing primary hot+full store) on a **durable OCI block-volume PVC**. Block
  storage is unmetered vs Logging ingest, so the full stream here is effectively free.
- **Managed pillars:** OCI Logging (full/unfiltered logs), OCI Monitoring (metrics), OCI APM (head-sampled
  traces). Grafana reads all four via Oracle's OCI datasources.

> **A1 = 2/12 rebuild survival:** on the grandfathered 4/24 node the separate gateway pod is fine. If a
> rebuild/DR drops the A1 to **2 OCPU/12 GB**, **fold the gateway into the DaemonSet** (drop the standalone
> gateway pod; agents export straight to the store/OCI) to reclaim RAM. The design survives 2/12.

**Loki hard-tuning (a 1 GB pod must never be able to OOM itself on a query):**
```yaml
# loki config — bound every query so worst-case LogQL cannot blow the pod
limits_config:
  split_queries_by_interval: 24h    # chop large ranges into bounded sub-queries
  max_query_parallelism: 1          # never fan out — a 1 GB pod cannot afford parallel shards
  max_query_length: 744h            # ~31 d ceiling; refuse unbounded scans
  # + per-tenant ingestion + rate caps to bound the write path
ingester:
  wal:
    enabled: true
    dir: /loki/wal                  # WAL on the block-volume PVC → replay on restart, no head loss
# pod spec: hard memory limit (e.g. 768Mi) + GOMEMLIMIT set just under it so Go GC yields before OOMKill
```
**Graceful degradation (a Loki OOM ≠ data loss):** the WAL on the PVC replays the in-flight head on
restart, and — decisively — **the same full stream is independently persisted to OCI Logging**. So a Loki
crash costs at most a brief LogQL gap while the pod restarts; nothing is lost, because the managed copy
(and the block-volume PVC) survive the pod. This redundancy is the whole point of writing full logs to
both free tiers.

**OTel Collector config sketch (verify component names/pins at implementation):**
```yaml
# ---- DaemonSet AGENT (every node) ----
receivers:
  filelog:                          # tail all container stdout
    include: [/var/log/pods/*/*/*.log]
    include_file_path: true
    operators: [ { type: json_parser } ]   # app already emits JSON w/ trace_id
  otlp:                             # app traces — lib.rs:2111 exporter points here
    protocols: { grpc: { endpoint: 0.0.0.0:4317 } }
processors:
  memory_limiter: { check_interval: 1s, limit_mib: 150 }   # keep the agent ~100–150 MB
  k8sattributes: {}                 # attach pod / namespace / org_id labels
  transform/redact: {}              # secondary PII/secret scrub (source redaction is primary, §6)
  batch: {}
exporters:
  otlp/gateway: { endpoint: otel-gateway.observability:4317 }
service:
  pipelines:
    logs:   { receivers: [filelog], processors: [memory_limiter,k8sattributes,transform/redact,batch], exporters: [otlp/gateway] }
    traces: { receivers: [otlp],    processors: [memory_limiter,batch], exporters: [otlp/gateway] }

# ---- GATEWAY (micro 1) ----
receivers:
  otlp: { protocols: { grpc: { endpoint: 0.0.0.0:4317 } } }
processors:
  batch: {}
  probabilistic_sampler:            # HEAD-sampling — keep APM under 1000 events/hr
    sampling_percentage: 5          # tune to real trace volume
exporters:
  otlphttp/loki:  { endpoint: http://loki.observability:3100/otlp }   # Loki 3.x native OTLP ingest
  ocilogging:     {}                # OCI Logging (FULL, unfiltered). VERIFY a maintained OTel OCI-Logging
                                    # exporter exists at pin time; else OCI Unified Agent / Connector Hub.
  otlp/apm:       {}                # OCI APM OTLP ingest endpoint + private data key (traces)
service:
  pipelines:
    logs:   { receivers: [otlp], processors: [batch], exporters: [otlphttp/loki, ocilogging] }
    traces: { receivers: [otlp], processors: [batch, probabilistic_sampler], exporters: [otlp/apm] }
```
There is deliberately **no Fluent Bit config** — the OTel agent is the node collector. (Metrics: the app
posts to **OCI Monitoring** custom metrics / an OTel metrics pipeline in Phase-2; kept out of the log
sketch above for clarity.)

**Retention/tiering mechanism:**
- **Loki:** `retention_period` bounded by the block-volume PVC size (target ~30 d). Loki's compactor
  deletes its own expired chunks.
- **OCI Logging:** log-group retention set ≤ **180 d** (its maximum), full/unfiltered.
- **Object Storage tail:** an **OCI Service Connector** streams Logging → a dedicated bucket for the
  **180 d → 365 d** compliance tail; a bucket **lifecycle policy** deletes objects > 365 d (declared in
  OpenTofu). Notably, **Loki does NOT write to Object Storage in this design** (block-volume PVC), so the
  S3-checksum gotcha (§9 R-1) is off the hot path entirely — it can only affect the Object-Storage tail
  writer.

**Vault-sourced credentials (reuse the existing discipline, do not invent):**
- OCI API/auth material for the OTel OCI exporters (Logging + APM data key + Monitoring) and the Service
  Connector's Object-Storage write, stored in **OCI Vault** (same class as `console-app-secrets-bundle` /
  `oci-objectstore-creds`), projected as k8s secrets in the `observability` ns **out-of-band via
  `kubectl create secret`** — exactly the pattern `deploy/SECRETS.md` blesses for a 1–2 person team
  ("the pragmatic, honest baseline"; External/Sealed Secrets is the noted future upgrade, not adopted here).
- Grafana admin bootstrap password: likewise a k8s secret from Vault.

**ArgoCD app + OpenTofu resources needed:**
- **ArgoCD:** new `deploy/argocd/apps/observability.yaml` Application (project `maintenance`, sync-wave
  after operators), sourcing `deploy/apps/observability/` (kustomize wrapping the OTel Collector + Loki +
  Grafana Helm charts, pinned — consistent with how barman/traefik/rollouts are pulled). App-of-apps `root`
  (`deploy/argocd/root.yaml`) recurses over `deploy/argocd/apps` and picks it up. selfHeal + prune.
- **OpenTofu (`deploy/opentofu/storage.tf`):** add the OCI **Logging** log group + log with ≤180 d
  retention; the **Service Connector** (Logging → Object Storage); the **Object Storage bucket** for the
  1-yr tail (`NoPublicAccess`, no versioning) + its `oci_objectstorage_object_lifecycle_policy` (delete
  > 365 d); the **OCI Monitoring alarm at ~8 GB/mo** Logging ingest (the runaway canary). Mirror the
  existing `db_backups`/`evidence` resources. (The **block volume** for Loki is a k8s PVC via the OCI CSI
  storage class, not a Tofu object.)
- **NetworkPolicy:** if (and only if) the Calico/Canal policy add-on is actually enforcing (the repo notes
  Talos default flannel does **not** enforce — `deploy/apps/console/base/networkpolicy.yaml` header),
  add an egress-allow for the `observability` tier to 443 (OCI) + DNS, mirroring `allow-app-egress-https`.

---

## 6. Security & privacy

**Redaction at SOURCE (primary defense — already largely in place):**
- **Never-log-secrets discipline** is enforced by the CI gate `backend/ci/gates/pii-no-logs/` (literal
  scanner over logging macros) — keep it green; it is the first line.
- **Runtime redaction newtypes** (`backend/crates/kernel/core/src/redact.rs`, `RedactedPhone` etc.) mask
  PII that reaches a log via binding/interpolation (which the literal scanner can't follow). Extend the
  allowlist/newtypes for any tenant PII field that must appear in logs; default is to **not** log it.
- Query strings are already dropped at the span (`lib.rs:1282`). Keep that.
- **Secondary net in the pipeline:** the OTel agent's `transform`/`redaction` processor (field allowlist +
  regex drop for anything resembling a token/JWT/phone) as defense-in-depth **before anything leaves the
  node** — so both the Loki and OCI Logging copies are scrubbed identically. Source redaction is
  authoritative; this is belt-and-suspenders, not a license to log PII.

**Encryption at rest:** OCI encrypts all objects/logs/block-volumes at rest by default (AES-256,
OCI-managed keys); the tenancy master key (`oyatie-cloud-master-key`) can be bound for customer-managed
keys if required. Loki's PVC sits on an OCI block volume (encrypted). **No plaintext log store anywhere.**
In transit: agent→gateway and gateway→Loki are in-cluster OTLP; gateway→OCI (Logging/APM/Monitoring) is
HTTPS; browser→Grafana is cert-manager TLS.

**Access control + AUDIT of who queries logs (R9):**
- Grafana is **not public** — Traefik ingress with auth (Grafana login; add SSO/oauth2-proxy if needed).
  For a 1–2 person team, a single strong operator login behind TLS is the honest baseline; per-user RBAC
  over logs is a Phase-3 concern, not a launch blocker.
- **Query access is itself logged**: Grafana's and Loki's own access logs are shipped by the same OTel
  DaemonSet → "who searched the logs" is captured in the log store (and, if we want it tamper-evident, a
  future hook can emit an `audit_event` on privileged log-query — but that couples logs to the audit
  chain, so **deferred, not built** unless a compliance driver demands it).

**Tenant scoping of log access (R6) — reframed honestly:** operational logs are for the **operator**, not
tenants. Tenants get their "who did what" from the product's **audit chain**, never raw ops logs. So
"tenant scoping" here means: **logs carry an `org_id` label** (attached by `k8sattributes`/`transform`) so
an operator can *filter* per-tenant during an incident — it does **not** mean per-tenant RBAC exposing the
log store to tenants (that store is never tenant-facing). This dissolves the need for heavy multi-tenant
RBAC in the log layer.

**Retention / right-to-erasure:** PIPA erasure requests target **personal data**, which lives in Postgres
and evidence storage, not in PII-minimized ops logs. If a residual identifier lands in logs, the bounded
retention (Loki ~30 d / OCI Logging ≤180 d / Object-Storage tail → 365 d then lifecycle-delete) is the
erasure mechanism — logs age out automatically. Document this in the DR/retention runbook alongside
`ops/dr/DR-POLICY.md`.

---

## 7. Phased rollout

**Phase 1 — Durable log pillar end-to-end (closes the ephemeral-logs gap).**
- Deliverable: OTel Collector DaemonSet (filelog) + gateway deployed via Argo; **full** stream dual-written
  to **Loki** (hot, block-volume PVC) **and OCI Logging** (full/unfiltered, ≤180 d); the **8 GB/mo
  Monitoring alarm** armed; Grafana up as the single pane over both; credentials from Vault.
- Acceptance: (1) kill a pod → its final log lines appear in **both** Loki AND OCI Logging; (2) a LogQL
  `trace_id` search in Grafana lands the exact request's lines; (3) unauthenticated access to Grafana is
  refused; (4) the OTel agent holds ~100–150 MB and Loki stays under its hard mem-limit under a wide LogQL
  query (no OOM).

**Phase 2 — Metrics + correlation.**
- Deliverable: OCI Monitoring wired (custom metrics / OTel metrics pipeline); Grafana OCI datasources for
  Logging + Monitoring; existing SLO rules (`backend/app/slos/`) reused for alert routing.
- Acceptance: (1) a metric appears in OCI Monitoring and renders in Grafana; (2) correlate a `trace_id`
  from a log line to its `audit_events` row in Postgres → **end-to-end log↔trace↔audit** proven (R11).

**Phase 3 — Traces (APM) + retention automation.**
- Deliverable: flip the app OTLP exporter ON (`lib.rs:2111` → `OTEL_EXPORTER_OTLP_ENDPOINT` at the OTel
  agent, uncomment `configmap.yaml:62-65`); gateway **head-sampling** tuned to keep APM < 1000 events/hr;
  Service Connector (Logging → Object-Storage 1-yr tail) + lifecycle policy live.
- Acceptance: (1) a trace shows in OCI APM and in Grafana, exemplar-linked from a log line; (2) APM event
  rate measured < 1000/hr after sampling; (3) objects/streams past retention are provably gone; (4) a
  synthetic error burst fires an alert to the operator.

---

## 8. Cost estimate

**The critical, non-obvious constraint:** every "$0" here rests on the **verified free-tier ceilings**
(callout up top). The design keeps **full logs in two free tiers at once** — Loki on a **block volume**
(not metered like Logging ingest, and separate from the 20 GB Object-Storage free budget contended by
evidence + DB backups) **and** OCI Logging (free < 10 GB/mo). No filtering is needed *at this volume*; the
8 GB/mo Monitoring alarm is the canary if a log loop ever threatens the cap.

Rough monthly (small platform, `info` level, assume well under 10 GB/mo to Logging):

| Item | Estimate |
|---|---|
| **Logs — Loki (block-volume PVC)** | **$0** — block storage in Always-Free allotment; unmetered vs Logging ingest |
| **Logs — OCI Logging (full/unfiltered)** | **$0** while < **10 GB/mo** free; then **$0.05/GB**. 8 GB alarm = canary |
| **Metrics — OCI Monitoring** | **$0** — under **500 M** free ingestion pts/mo |
| **Traces — OCI APM (head-sampled)** | **$0** — head-sampled under **1000 events/hr** free |
| **1-yr tail — Object Storage (via Service Connector)** | ~$0–1/mo — compressed 180 d→365 d tail; small |
| **Compute** | **$0 marginal** — OTel agents (~100–150 MB) + Grafana/gateway (micro 1) + Loki (micro 2, hard-limited) fit the two 1 GB Always-Free micros + A1 headroom; **no new paid node** |
| **Total** | **~$0/mo** while the volume/sampling assumptions hold |

**Honest tension:** hard-**$0** holds **only while** (a) full-log volume stays **< 10 GB/mo** to OCI
Logging (the 8 GB alarm is the early warning) **and** (b) traces stay **head-sampled < 1000/hr**. If daily
log volume grows past the cap, either the Logging copy starts costing $0.05/GB (Loki stays free on block
storage) or we reintroduce a compliance-subset filter to Logging (explicitly *not* done now). Genuinely
cheap; the real pressure is the 10 GB/mo Logging ceiling and the shared 20 GB Object-Storage allotment —
hence the alarm and the block-volume (not Object-Storage) hot store.

---

## 9. Prerequisites & GO/NO-GO

**The DESIGN is deliverable now (this document). The DEPLOY is founder-gated ops** (OCI Vault creds +
cluster access + a maintenance window) — the same gate as every other cluster change.

**Recommendation: GO for Phase 1 (Direction A)** — OTel DaemonSet → Loki (block-volume PVC) + OCI Logging
(full), Grafana pane, 8 GB alarm. Low-risk, ~$0, closes the incident-response gap with the full log pillar.

**Exact prereqs the founder must provide:**
1. **Confirm A1 capacity.** Check the OCI console shape: is the live node grandfathered at **4 OCPU/24 GB**,
   or would a rebuild yield **2 OCPU/12 GB**? Design already survives 2/12 (fold the gateway into the
   DaemonSet, §5) — but confirm so the micro/gateway placement is sized correctly.
2. **Confirm / monitor Logging volume < 10 GB/mo.** Watch OCI Logging usage for a week after Phase-1;
   the **8 GB/mo Monitoring alarm** is the standing canary. This is the single assumption the whole "$0,
   no-filter" call rests on.
3. **OCI creds in Vault.** OCI auth material for the OTel OCI exporters (Logging + APM data key +
   Monitoring) and the Service-Connector/Object-Storage tail writer, stored in **OCI Vault**. Never `/tmp`,
   never git (OPS-RUNBOOK §0).
4. **Cluster access + a maintenance window** to: create the `observability`-ns secrets from Vault, apply
   the new ArgoCD app, `tofu apply` the Logging/Monitoring-alarm/Service-Connector/bucket resources, and
   (only if Calico policy enforcement is on) add the egress allow.

**Risks (ranked):**
- **R-1 (med, scoped): S3 checksum / chunked-encoding incompatibility — ONLY on the Object-Storage 1-yr
  tail writer.** OCI rejects AWS flexible checksums sent via chunked/trailer encoding — the *exact* failure
  that silently broke CNPG backups, fixed there with `AWS_REQUEST_CHECKSUM_CALCULATION=when_required` /
  `AWS_RESPONSE_CHECKSUM_VALIDATION=when_required` (`deploy/apps/console/base/database.yaml:20-26`).
  **This risk is now off the hot path** — Loki writes to a **block-volume PVC**, not Object Storage, and
  OCI Logging is a native managed sink. It can only bite the **180 d→365 d Object-Storage tail**: if that
  tail is written by an S3-SDK client, set the same `when_required` env and make a round-trip a gate; the
  managed **Service Connector** (OCI-native) sidesteps the AWS SDK entirely — prefer it. Verify either way.
- **R-2 (med): 1 GB micro compute pressure.** The OTel agent + Loki/Grafana must fit 1 GB micros.
  Mitigation: OTel `memory_limiter` (~100–150 MB), Loki hard mem-limit + GOMEMLIMIT + the tuning block
  (§5); **Fluent Bit is the drop-in agent fallback** if the OTel agent OOMs a micro.
- **R-3 (med): A1 2/12 rebuild.** A DR/rebuild halves the A1. Mitigation: fold the gateway into the
  DaemonSet to reclaim RAM (§5); the managed OCI pillars carry no node RAM cost.
- **R-4 (med): Logging free-cap breach.** A runaway log loop could cross 10 GB/mo. Mitigation: the **8 GB
  Monitoring alarm** canary; Loki (block storage) stays free regardless; a compliance-subset filter to
  Logging is the held-in-reserve lever (not built now).
- **R-5 (low): no log HA.** A node loss can drop in-flight buffered logs before the next flush. Accepted —
  the cold copies (OCI Logging + block-volume PVC WAL) are the durability guarantee; shortening the flush
  window trades throughput for a smaller loss window.
- **R-6 (low): NetworkPolicy enforcement.** Egress allow only matters if Calico/Canal is actually rolled
  out (flannel default doesn't enforce). No-op otherwise; harmless to declare.

**NO-GO conditions:** if the founder cannot provide a Vault-stored OCI key + a maintenance window, or if
measured log volume blows past 10 GB/mo with no headroom and PAYG stays locked, hold and revisit (either
accept the Logging $0.05/GB spillover, or reintroduce a Logging filter) — Loki alone still delivers the hot
pillar in the interim.

---

## Appendix — deliberate simplifications (ponytail)

- **No Fluent Bit** — the OTel Collector is the node agent for logs *and* traces; Fluent Bit is named only
  as the fallback if a 1 GB micro OOMs the OTel agent (a DaemonSet swap, not a redesign).
- **No filtering to OCI Logging** — full/unfiltered; the volume sits under the 10 GB/mo free cap and the
  8 GB alarm guards runaways. A compliance-subset filter is the reserve lever, not built now.
- **No self-hosted Tempo/Prometheus** — OCI APM (traces) + OCI Monitoring (metrics) are the managed
  backends; no RAM spent on the micros for pillars OCI gives us free.
- **No S3 on the hot path** — Loki uses a block-volume PVC, so the CNPG-class S3 checksum gotcha is off the
  hot path and can only touch the Object-Storage 1-yr tail writer.
- **No External/Sealed Secrets** — out-of-band `kubectl create secret` from Vault, per `SECRETS.md`'s own
  blessed baseline for a 1–2 person team.
- **No per-tenant RBAC in the log store** — logs are operator-only; tenants get the audit chain.
- **No audit-chain integration for log queries** — deferred; add only under a compliance driver.
