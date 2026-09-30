---
id: ADR-0020
status: accepted
doc_status: published
date: 2026-07-03
owner: jasonlee
acceptance_scope: fixture-only foundation; no live institution access
related: [ADR-0010]
---

# ADR-0020: Korean institutional connectivity coverage factory

## Status
Accepted for the fixture-only foundation slice.

## Context

We need an in-house Korean institutional connectivity platform with CODEF/Popbill-like coverage ambition across banking, tax, public institutions, insurance/social insurance, and certificate-backed workflows, but without using CODEF, Popbill, Tilko/Tilkoblet, or similar aggregators as runtime dependencies.

The approved `$ralplan` handoff chooses an official-rail-first hybrid coverage factory. The first execution slice must prove the catalog, state machine, evidence model, and guardrails before any live institution access.

## Decision

Build a repo-native coverage factory around a declarative connector catalog and a strict connector capability state machine. Every connector starts non-live, records source evidence, and advances only through explicit legal/security/ops/consent gates.

Official rails are the primary path: KFTC Open Banking, Financial MyData/partner rails, NTS ASP/ERP certification paths, NHIS/COMWEL/4insure official forms or EDI/file flows, and other sanctioned APIs where eligible. Local-agent/customer-session adapters are quarantined fallback paths only when no official rail exists.

Public official API/documentation scraping is allowed only as source discovery. It may read public official documentation, OpenAPI/Swagger specs, static HTML docs, public file-format specs, and sandbox/testbed references. It must be read-only, attributable by URL, rate-limited, cached as source metadata/fixtures, and must not require login, credentials, private data, browser-session capture, institution security-plugin internals, anti-automation circumvention, or production customer sessions.

## Non-negotiable invariants

- Do not use CODEF, Popbill, Tilko/Tilkoblet, or similar aggregators as runtime dependencies.
- Production must not centrally store or operate raw customer 공동인증서/private keys, certificate passwords, bank passwords, or unmanaged financial credentials.
- Do not collect real customer credentials, real 공동인증서 material, certificate passwords, OTP/security-card values, session cookies, browser local/session storage, or unmanaged institution credentials in this slice.
- Do not proxy, intercept, replay, or bypass customer browser sessions, institution security plugins, anti-automation controls, keyboard security modules, certificate dialogs, or MFA prompts.
- No live filing, payment, production signing, production institution login, or production institution API call is part of the fixture foundation.
- Every connector must carry source evidence, data classification, auth mode, side-effect class, fixture path, evidence policy, legal status, and capability state.

## Connector capability state machine

The only valid states are `research_only`, `fixture_only`, `sandbox`, `partner_approved`, `live_read`, `live_write`, `prohibited`, and `deprecated`.

`research_only` allows evidence gathering only; `fixture_only` allows deterministic offline fixtures only; `sandbox` allows official sandbox/testbed only; `partner_approved` records legal/business approval without live customer workflows; `live_read` allows production read-only after gates; `live_write` allows filing/issuance/transfer/cancellation only after side-effect gates; `prohibited` is the default terminal state for forbidden credential/session behavior unless a fresh legal/security ADR reclassifies it; `deprecated` retires a connector.

Code changes alone cannot promote a connector. Transitions require recorded legal/security/ops/consent evidence. Any connector that requires forbidden data crossing, anti-automation bypass, hidden credential brokerage, or adverse legal/security findings moves to `prohibited`.

## Consequences

- The first slice is slower than an adapter-first clone but creates a durable compliance/security boundary.
- Public API/documentation scraping becomes a controlled source-evidence function, not a credential/session automation path.
- Product coverage must be truthfully labeled by capability state and side-effect class.
- Any future credential custody product is a separate product decision requiring a fresh ADR, threat model, legal/security signoff, and operational controls.

## Verification

The `check:korean-institutional-connectivity` gate verifies this ADR, the catalog spec, the catalog schema/fixture, no-aggregator/no-custody invariants, connector state names, public-source scraping rules, and fixture-only example connectors.
