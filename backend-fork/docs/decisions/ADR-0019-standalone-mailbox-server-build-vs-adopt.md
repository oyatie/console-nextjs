---
id: ADR-0019
status: accepted
doc_status: published
date: 2026-06-30
owner: jasonlee
amended_by: [ADR-0024]
related: [ADR-0005, ADR-0023, ADR-0024]
---

# ADR-0019: Standalone corporate mailbox server build-vs-adopt decision

## Status
Accepted; deployment-resource envelope amended by ADR-0024

ADR-0024 replaces this record's OCI/free-tier-first deployment assumption with
the owner-controlled self-host reference as the first portability gate. The
current OCI A1 deployment remains a supported, resource-constrained context.
This amendment does not change the clean-room Rust build-vs.-adopt decision or
authorize public MX.

The Mox-oriented design mirror and DARK deployment manifests are implementation
divergence/prototype evidence; they do not supersede this decision. Mox remains
inactive pending an explicit owner choice and a newer accepted ADR.

## Date
2026-06-30

## Context
The platform needs a standalone corporate mailbox server, not only the existing external SMTP/IMAP webmail mirror. Required target capability includes authoritative MX inbound SMTP, IMAP, JMAP Mail, domain/mailbox administration, group/org policy integration, passkey step-up for sensitive mail-admin actions, audit, retention/legal hold, resource-bounded operation in the ADR-0024 self-host reference and current OCI A1 context, and out-of-the-box mailbox availability without tenant-visible SMTP/IMAP server configuration.

Stalwart is the strongest product benchmark for modern mail-server capabilities: SMTP, IMAP, JMAP, collaboration protocols, DKIM/SPF/DMARC/ARC, MTA-STS/TLS-RPT/DANE, spam/phishing controls, queue management, and Kubernetes support. However, Stalwart is AGPL-3.0 upstream, which does not satisfy the current MIT/Apache-only dependency constraint for product adoption.

## Decision
Use Stalwart as a feature benchmark only. Do not embed, fork, copy source, copy assets, or derive implementation from Stalwart.

Default path: build a clean-room Rust-native mailbox foundation inside the platform and keep public MX disabled until production gates pass. The console must expose domain/DNS readiness, mailbox lifecycle, aliases, delegation, retention, and policy; it must not expose a generic mail-server configuration form to tenants. The first accepted slice is:

- `console-comms-mailbox`: pure Rust value objects and SMTP transaction state machine that enforces local-domain recipient resolution, no-open-relay behavior, command ordering, message-size bounds, recipient limits, DATA dot handling, and reset semantics.
- `0082_create_mailbox_server_spine.sql`: group/org-aware domain, mailbox, alias, message, and delivery/queue metadata with RLS and immutable-org triggers.
- `docs/specs/standalone-corporate-mailbox-server.md`: Stalwart parity matrix, adoption-candidate matrix, target architecture, phases, and production gates.

Run a bounded adoption spike for Apache James only if we need an off-the-shelf full-protocol server sooner than the native implementation can mature. Apache James is Apache-2.0 and supports SMTP, IMAP, JMAP, POP3, and distributed deployment options, but it may be too heavy for the declared self-host resource envelope and the supported OCI A1 context and would still need identity/policy/audit integration work. Any adopted server remains an internal platform component, not something tenants configure through host/port/password forms.

## Alternatives considered

- **Adopt Stalwart:** best functional benchmark, rejected by AGPL-3.0 license constraint.
- **Adopt Apache James:** best permissive full-protocol candidate; keep as a bounded spike, not the default, because JVM/distributed operational footprint and integration burden may be high.
- **Adopt Mailu or docker-mailserver:** MIT project-level conventional mail stacks with SMTP/IMAP/security features, but no JMAP; possible stopgap for MX/IMAP only.
- **Adopt Mox:** MIT, strong all-in-one SMTP/IMAP/DKIM/DMARC/MTA-STS/metrics, but JMAP is roadmap, not current parity.
- **Adopt Haraka/Postal:** MIT, useful SMTP/outbound components, not standalone mailbox servers.
- **Adopt WildDuck/Maddy/Cyrus:** rejected or deferred due license/JMAP/fit gaps under current constraints.

## Consequences

- Public MX remains gated. No production port 25 exposure is authorized by this ADR.
- JMAP and IMAP must be implemented or integrated over the same mailbox store; the product should prefer JMAP/app-native access for web/mobile clients.
- Mailbox administration must remain platform-native: group/org aware, policy-controlled, audited, passkey-step-up gated for sensitive actions, and focused on domains/mailboxes/policy rather than SMTP/IMAP server settings.
- Future code review must reject non-permissive copied mail-server code, including Stalwart source/assets/types.

## References

- Full spec and parity matrix: `docs/specs/standalone-corporate-mailbox-server.md`
- Stalwart: <https://github.com/stalwartlabs/stalwart>, <https://github.com/stalwartlabs/stalwart/blob/main/LICENSES/AGPL-3.0-only.txt>
- Apache James: <https://github.com/apache/james-project>, <https://github.com/apache/james-project/blob/master/LICENSE>
- JMAP Core/Mail: <https://www.rfc-editor.org/rfc/rfc8620>, <https://www.rfc-editor.org/rfc/rfc8621>
- IMAP4rev2: <https://www.rfc-editor.org/rfc/rfc9051>
- SMTP: <https://www.rfc-editor.org/rfc/rfc5321>
