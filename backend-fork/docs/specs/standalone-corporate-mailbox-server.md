> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Spec: Standalone corporate MX/IMAP/JMAP mailbox server

## Objective
Build a standalone corporate mailbox server for the platform: authoritative MX inbound SMTP, authenticated outbound/submission path, IMAP access, JMAP Mail access, and first-class integration with platform identity, tenancy, groups/orgs, policy, audit, workflow, messenger, notifications, and future HR/payroll document delivery.

This is not just the existing tenant-configured webmail mirror. The existing webmail stack connects to an external IMAP/SMTP account. The target system must be able to host corporate mailboxes itself.

## Non-negotiable constraints
- **License:** production dependencies embedded into our product must be MIT or Apache-2.0 unless separately approved. Stalwart is a feature benchmark, not an embeddable dependency under this constraint.
- **No open relay:** SMTP must reject relay abuse by default. RCPT acceptance is only for verified, enabled local domains or authenticated submission.
- **No live MX exposure until gates pass:** port 25 must not be exposed on `knllogistic.com` / `console.knllogistic.com` until DNS, TLS, queueing, abuse, monitoring, backup, and rollback gates pass.
- **Self-host-first and context-aware:** the resource footprint and every required dependency must fit the owner-controlled ADR-0024 self-host reference before production rollout. The current OCI A1 context remains supported and retains its own constrained resource envelope; neither context may make its substrate mandatory for the other.
- **Out-of-the-box operation:** tenants and group/org admins must not configure SMTP/IMAP hostnames, ports, passwords, or mail-server credentials in the console. The platform owns the mailbox service; admins manage domains, DNS readiness, mailbox lifecycle, aliases, delegation, retention, and policy.
- **Identity-native:** mailbox users are platform users/employees scoped by group/org/department/team/role/policy; sensitive mail-admin actions require passkey step-up and audit.
- **Data safety:** mail content and metadata are personal/corporate data. Retention, deletion, legal hold, purpose, access logs, masking, and export must be explicit.

## Current repo baseline
- Existing mail crates are `backend/crates/comms/*`:
  - `console-comms-rest`: webmail REST API under `/api/v1/mail/*`.
  - `console-comms-adapter-smtp`: outbound SMTP client to a configured tenant SMTP server.
  - `console-comms-adapter-imap`: inbound IMAP sync client from a configured tenant IMAP server.
  - `console-comms-adapter-postgres`: stores mirrored mail accounts/folders/threads/messages.
- Existing migrations `0053..0057` create `email_accounts`, `email_folders`, `email_threads`, `email_messages`, and related webmail support.
- That model is **external-account webmail**, not authoritative mailbox hosting. A standalone server needs domain/mailbox/routing/queue/protocol tables and service roles. The old SMTP/IMAP server-settings UI must be treated as legacy/migration-only and removed from normal navigation; product users should see an automatically provisioned mailbox or a platform readiness state, not a server configuration form.

## Stalwart parity baseline
Stalwart is the feature/capability benchmark because its upstream README positions it as a secure, scalable mail and collaboration server with IMAP, JMAP, SMTP, CalDAV, CardDAV, WebDAV, DKIM/SPF/DMARC/ARC, DANE/MTA-STS/TLS-RPT, queue management, spam/phishing controls, and Kubernetes support.

We should measure our mailbox server against these capability groups:

| Capability group | Stalwart-style target | Our production target | Priority |
| --- | --- | --- | --- |
| MX inbound SMTP | Authoritative SMTP server, local domain routing, queue, bounce handling | Accept verified local domains, durable inbound delivery, no open relay, greylist/rate limits, observability | P0 |
| Submission/outbound SMTP | Authenticated submission, queue, retries, DKIM signing | Prefer JMAP/app send path first; SMTP submission only after app-password/OAuth story is safe | P1 |
| IMAP | IMAP4rev2/rev1 plus common extensions | IMAP4rev2-compatible read/write mailbox access, TLS-only, app-password/OAuth, quotas | P1 |
| JMAP Mail | RFC 8620/8621 server | JMAP Core + Mail for web/mobile/native client, source-object links, push/event hooks | P0/P1 |
| POP3 | Legacy client access | Not required for this product unless a customer requires it | P3 |
| Mail authentication | SPF, DKIM, DMARC, ARC validation/signing | SPF/DKIM/DMARC required before public MX; ARC and DMARC/TLS reports P1 | P0/P1 |
| Transport security | STARTTLS/TLS, DANE, MTA-STS, TLS-RPT | STARTTLS/TLS required; MTA-STS/TLS-RPT before production; DANE when DNSSEC path is operationally ready | P0/P1 |
| Spam/phishing | Spam classifier, phishing protection, traps, rate limit | Start with Rspamd-style integration point or internal policy engine; quarantine/junk training; no silent drops | P1 |
| Storage/search | Mailbox folders, metadata, full text, attachments | Postgres metadata + raw MIME through the target provider-neutral object-storage port (self-host S3-compatible reference; current S3 adapter pointed at OCI Object Storage in `oci-guest`) + future search index; quotas, retention, legal hold | P0/P1 |
| Admin | Domains, aliases, users, policies, DNS status | Group/org-aware domain, mailbox, alias, shared-mailbox, DNS-readiness, retention, delegation, and ownership UI. No tenant SMTP/IMAP server configuration UI. | P0 |
| Collaboration | Contacts/calendar/file sharing protocols | Mail first. Calendar/contact integration via platform calendar/people modules; protocol parity later if needed | P2 |
| Observability | Metrics, logs, queue/admin visibility | Prometheus metrics, structured redacted logs, audit trail, queue depth, delivery rejection reasons, alerting | P0 |
| Kubernetes/HA | Cloud/orchestrator support | Dedicated `console-mailbox` workload, health/readiness, safe rollouts, internal-only until gates pass | P0 |

## Adoption candidates

| Candidate | License fit | Feature fit vs Stalwart | Operational fit | Recommendation |
| --- | --- | --- | --- | --- |
| Stalwart | **No** under current MIT/Apache-only rule. Upstream is AGPL-3.0. | Best benchmark: Rust, SMTP/IMAP/JMAP, collaboration protocols, security features. | Likely strong, but license blocks embedding/forking/copying. | Benchmark only. Do not adopt or copy source/assets/types. |
| Apache James | **Yes**, Apache-2.0. | Strongest permissive full-server candidate: SMTP, IMAP, JMAP, POP3, distributed app options. | JVM and distributed stack can be heavy for the self-host reference and the supported OCI A1 context; integration with platform identity/policy/audit would be sidecar/adapter work. | Best legal full-protocol adoption candidate if we need external server now. Run a resource and integration spike before committing. |
| Mailu | MIT at project level; component-license review still required before production. | Mature SMTP/IMAP stack with DKIM/DMARC/SPF/antispam, but no native JMAP. | Multi-container Docker/mailops stack; good conventional mail server, weaker identity-native integration. | Possible stopgap for MX/IMAP if JMAP is deferred; not sufficient for requested JMAP parity. |
| docker-mailserver | MIT at project level; component-license review still required. | Postfix/Dovecot/Rspamd/SpamAssassin/OpenDKIM/OpenDMARC; no JMAP. | Mature conventional stack, but multiple services/config files and not platform-native. | Similar to Mailu: useful fallback for MX/IMAP, not JMAP parity. |
| Mox | MIT. | Excellent modern all-in-one SMTP/IMAP/DKIM/DMARC/MTA-STS/metrics; JMAP is on roadmap, not current feature. | Single Go binary, simpler than Postfix/Dovecot stacks; integration still external. | Good candidate if we accept no JMAP initially. Not enough for strict MX/IMAP/JMAP ask today. |
| Haraka | MIT. | SMTP MTA/MSA only; not mailbox store/IMAP/JMAP. | Good front-door/filtering component. | Not standalone mailbox server; possible future SMTP-edge adapter only. |
| Postal | MIT. | Outbound/website transactional mail server, not full IMAP/JMAP mailbox suite. | Useful for bulk/transactional outbound only. | Not a fit for corporate mailbox hosting. |
| WildDuck | EUPL-1.2. | IMAP/POP3 store, no JMAP baseline. | Node/Mongo stack. | Reject under current license preference and JMAP gap. |
| Maddy | GPL-3.0. | SMTP/IMAP-oriented, no JMAP parity. | Simpler than multi-container stacks. | Reject under current license constraint. |
| Cyrus IMAP | Not MIT/Apache; license is not a clean fit under current rule. | Strong IMAP/JMAP ecosystem candidate historically, but adoption requires legal review. | Mature but C stack and integration burden. | Do not adopt without explicit legal/license approval. |

## Recommendation
Use a **two-track decision**:

1. **Product-native track (default): build our own clean-room Rust mailbox foundation.**
   - Reason: no MIT/Apache candidate currently gives the exact combination of Stalwart-like MX + IMAP + JMAP plus deep platform tenancy, passkey, policy, audit, portable object storage, workflow, group/org, Korean compliance integration, no tenant-visible server configuration, and a resource envelope proven in both the self-host reference and supported OCI A1 context.
   - Benchmark Stalwart, Mox, Apache James, Mailu, docker-mailserver, Gmail/Proton UI, and Slack-style workflow integrations, but do not copy non-permissive implementation code.

2. **Adoption spike track (parallel, bounded): evaluate Apache James as the only current permissive full-protocol server candidate.**
   - Pass criteria: runs within the declared self-host resource envelope and the supported OCI A1 budget, supports required JMAP/IMAP/SMTP flows, can delegate identity to our platform or safely sync users/mailboxes, stays hidden behind platform-native admin/domain UX with no tenant server-config form, exposes observable delivery/audit signals, and can store/backup data safely.
   - Fail criteria: too heavy, too hard to integrate with group/org/policy/passkey/audit, or creates a second source of truth for employees/mailboxes.

If the business needs a public MX faster than our native JMAP server can mature, the pragmatic stopgap is **Mox or Mailu/docker-mailserver for MX/IMAP only**, plus our app-native JMAP/webmail bridge later. That is not full requested parity and must be labelled as a temporary adoption choice.

## Target architecture

### Services
- `console-mailbox` service role, disabled from public ingress until gates pass.
- SMTP listener:
  - `25` for MX inbound only after production gates.
  - `2525`/internal port for tests and cluster-only smoke.
- JMAP HTTPS:
  - Preferred behind existing app/API ingress using platform JWT/session/passkey context.
  - Implements JMAP Core and Mail; app/web/mobile clients should use this path instead of IMAP when possible.
- IMAP TLS:
  - `993` only; no plaintext auth.
  - Auth via app-issued mailbox tokens/app-passwords or future OAUTHBEARER, not raw platform password.

### Storage
- Postgres for domain/mailbox/routing/folder/message metadata, RLS scope, queue state, audit pointers.
- The target provider-neutral object-storage port for immutable raw RFC 5322 message bytes and large attachments: a self-hosted S3-compatible adapter in the reference stack and the current S3 adapter pointed at OCI Object Storage in `oci-guest`.
- Optional search index later for full-text/mailbox search; keep canonical metadata in Postgres.

### Core data model
- `mailbox_domains`: verified corporate domains by group/org/tenant, DNS status, DKIM keys, SPF/DMARC/MTA-STS/TLS-RPT status.
- `mailboxes`: local mailbox identity, owning employee/user/org/group, status, quota, retention policy.
- `mailbox_aliases`: aliases, shared addresses, role addresses, plus-addressing/catch-all policy.
- `mailbox_messages`: immutable message metadata, raw object pointer, normalized subject/thread keys, sensitivity class.
- `mailbox_folders`: folders and system roles.
- `mailbox_deliveries`: inbound/outbound queue, delivery attempts, rejection/bounce reasons, audit IDs.
- `mailbox_auth_tokens`: scoped IMAP/submission credentials or OAuth token state, revocable and audited.

### Security and compliance
- Passkey step-up for domain changes, mailbox delegation, alias/shared mailbox changes, retention/legal-hold changes, export, purge, and impersonation/break-glass.
- Console UX exposes mailbox administration, not mail-server plumbing: users never enter SMTP/IMAP passwords, hostnames, or ports for the corporate mailbox.
- Redacted logs: never log full addresses, subject, body, or attachment names outside explicit audit views.
- Every admin action produces audit events tied to the human principal, org/group scope, IP/device/session, and step-up state.
- Retention/legal hold must be policy-driven before enabling broad mailbox export/delete.

### Observability
Required metrics and logs before production MX:
- `mailbox_smtp_sessions_total{result,reason}`
- `mailbox_delivery_total{direction,result,reason}`
- `mailbox_delivery_latency_seconds`
- `mailbox_queue_depth{direction,state}`
- `mailbox_auth_failures_total{protocol,reason}`
- `mailbox_dns_status{domain,record}`
- `mailbox_storage_bytes{org,domain}`
- structured redacted delivery/audit events with trace IDs.

## Delivery phases

### Phase 0: safety gates and schema foundation
- Add spec/ADR, migrations for domain/mailbox/routing/queue metadata, and a disabled `console-mailbox` service config.
- No public port 25 exposure.
- Tests for address/domain validation, no-open-relay routing, quotas, and audit-required admin commands.

### Phase 1: local SMTP ingestion
- Implement SMTP transaction state machine: EHLO/HELO, MAIL FROM, RCPT TO, DATA, RSET, QUIT.
- Accept only verified local recipients; reject relay attempts and disabled domains.
- Store raw MIME to object storage, metadata to Postgres, and project to the existing mail UI read model.
- Internal-only smoke tests.

### Phase 2: JMAP Core + Mail
- Implement account discovery, mailbox/get, email/query/get, email/set for send/draft/delete/read state, push/event hooks.
- Web and mobile clients prefer JMAP for corporate mail.
- Existing `/api/v1/mail/*` can be a compatibility facade over JMAP/domain services.

### Phase 3: IMAP TLS access
- Implement or adopt an IMAP server layer that exposes the same mailbox store.
- TLS-only; auth via scoped mailbox tokens/app-password/OAuth; rate-limited; audited.

### Phase 4: production deliverability
- DKIM signing/verification, SPF/DMARC checks, MTA-STS/TLS-RPT, bounce/DSN behavior, queue retry policy, quarantine/junk path, abuse dashboards.
- DNS admin checks and rollout checklist for each hosted domain.

### Phase 5: parity/maturity
- Search, retention/legal hold, shared mailboxes, Sieve/rules, delegation, calendar/contact protocol compatibility if needed, migration tooling, import/export, AI-assisted triage only after deterministic/audited mechanics are stable.

## Production gate checklist
- Domain DNS verified: MX, SPF, DKIM, DMARC, MTA-STS, TLS-RPT.
- TLS certificates valid and rotation tested.
- Backups/PITR and object-store retention tested.
- Queue retry/dead-letter dashboard working.
- Abuse controls tested: open relay negative test, rate limit, oversized messages, invalid recipients, spam/quarantine path.
- Audit and passkey step-up verified for mailbox admin actions.
- JMAP and IMAP e2e tests pass for group admin, org admin, regular employee, offboarded employee, and cross-org/shared mailbox cases.
- Argo rollout health, Prometheus alerts, and rollback procedure verified.

## Source anchors
- Stalwart README/license: <https://github.com/stalwartlabs/stalwart>, <https://github.com/stalwartlabs/stalwart/blob/main/LICENSES/AGPL-3.0-only.txt>
- Apache James README/license: <https://github.com/apache/james-project>, <https://github.com/apache/james-project/blob/master/LICENSE>
- Mailu README/license: <https://github.com/Mailu/Mailu>, <https://github.com/Mailu/Mailu/blob/master/LICENSE.md>
- docker-mailserver README/license: <https://github.com/docker-mailserver/docker-mailserver>, <https://github.com/docker-mailserver/docker-mailserver/blob/master/LICENSE>
- Mox README/license: <https://github.com/mjl-/mox>, <https://github.com/mjl-/mox/blob/main/LICENSE.MIT>
- Haraka README/license: <https://github.com/haraka/Haraka>, <https://github.com/haraka/Haraka/blob/master/LICENSE>
- JMAP Core/Mail: <https://www.rfc-editor.org/rfc/rfc8620>, <https://www.rfc-editor.org/rfc/rfc8621>
- IMAP4rev2: <https://www.rfc-editor.org/rfc/rfc9051>
- SMTP: <https://www.rfc-editor.org/rfc/rfc5321>
- DKIM/SPF/DMARC: <https://www.rfc-editor.org/rfc/rfc6376>, <https://www.rfc-editor.org/rfc/rfc7208>, <https://www.rfc-editor.org/rfc/rfc7489>
