---
id: ADR-0022
status: accepted
doc_status: published
date: 2026-07-04
owner: jasonlee
consensus: product correction on 2026-07-04
amends: [ADR-0010]
amended_by: [ADR-0027]
supersedes: [ADR-0017]
related: [ADR-0004, ADR-0010, ADR-0017, ADR-0027]
---

# ADR-0022: Local identity only; no speculative external IdP seam

## Status

Accepted. Supersedes the identity-provider portion of ADR-0010 and all of ADR-0017.

## Date

2026-07-04

## Context

Maintenance does not have an external identity provider. The product creates and manages its own identities through local passkey-backed accounts, rotating refresh-token families, branch-scoped authorization, and audited account administration.

A prior cleanup audit found a deferred identity-provider port and DTO contract that was not exercised by production code. Product correction confirmed that the old branded future-provider wording came from non-authoritative user language, not from a real provider contract or launch dependency.

Keeping such a port in code makes a nonexistent integration look architectural, encourages mock/stub justification, and blurs the identity source of truth.

## Decision

Do not ship a speculative external IdP seam.

- Maintenance-owned passkey accounts remain the production identity source (ADR-0004).
- `console-identity-application` must expose only local org/account administration commands, read models, and audit builders unless a real future integration is approved. This clause decides nothing about a platform-tier identity handle either way; ADR-0027 records that scope finding and narrows the permitted form of cross-tenant identity linkage to a human assertion.
- Retire the unused identity-provider trait, DTOs, boxed futures, provider error type, and contract-only tests.
- HR roster, attendance, payroll, or person-data integrations are data integrations by default. They must not authenticate users, assert sessions, grant roles, link two accounts to one identity, or decide account status unless a separate identity-federation ADR names the real IdP/protocol/claims and passes security review.
- Any future external HR/attendance/payroll integration requires a named provider, owner, validated production contract, privacy/security review, production configuration path, adapter implementation, and tests before a code seam is restored.

## Alternatives Considered

### Rename the provider port generically
Rejected. A generic port still makes an unowned future integration look real and keeps code that no production path exercises.

### Keep the port in code until an adapter lands
Rejected. Issue #115 identified this as over-engineered contract-only code, and no adapter owner or external IdP exists.

### Add a mock adapter to exercise the port
Rejected. The project forbids mock/stub shipped paths for these integration seams.

## Consequences

- The identity layer is easier to understand: local account administration is real, external identity-provider code is absent.
- Future provider work must start from live evidence rather than inherited naming.
- The current HR-core surface can continue as imported people/org/attendance product data without becoming authentication authority.
