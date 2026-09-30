---
id: ADR-0017
status: superseded
superseded_by: [ADR-0022]

doc_status: published
date: 2026-06-12
owner: codex/t61
consensus: superseded by product correction on 2026-07-04
related: [ADR-0001, ADR-0004, ADR-0010, ADR-0022]
---

# ADR-0017 - Superseded identity-provider port contract

## Status
Superseded by ADR-0022.

## Context
This ADR previously created a deferred identity-provider port from a non-authoritative future-provider label. That label was not an identified external IdP, vendor contract, adapter owner, or launch dependency.

The corrected product architecture is simpler: maintenance owns identity locally through passkey-backed accounts (ADR-0004). HR roster, organization, attendance, and payroll-adjacent data may have import or integration needs later, but those are not authentication authority.

## Decision
Supersede this ADR. Do not define or restore the retired `IdentityProviderPort`, provider DTOs, boxed futures, provider error type, or contract-only tests.

`console-identity-application` remains an application-layer crate for local org/account administration commands, read models, and audit builders. Any future HR, attendance, payroll, or roster integration must start from a new ADR or implementation issue that names the real provider, owner, validated contract, production configuration path, privacy/security review, adapter, and tests.

## Consequences
+ The codebase no longer implies a nonexistent external identity provider.
+ Local passkey-backed accounts remain the only identity implementation.
+ Future integration work must arrive with real evidence and ownership instead of a speculative seam.
− If a real external HR/attendance/payroll provider appears later, the contract must be designed then from the real API and landed with its adapter or with explicit ADR approval.
