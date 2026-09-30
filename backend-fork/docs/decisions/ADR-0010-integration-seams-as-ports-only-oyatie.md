---
id: ADR-0010
status: accepted
amended_by: [ADR-0022]

doc_status: published
date: 2026-06-12
owner: jasonlee
consensus: ralplan iteration 3 (Planner/Architect/Critic APPROVE, 2026-06-12)
related: [ADR-0001, ADR-0022]
---

# ADR-0010 — Integration seams as ports only for oyatie AI — no mock adapters

## Status
Accepted for the oyatie AI seam. The speculative identity-provider seam that was previously bundled into this ADR is superseded by ADR-0022.

## Context
The production-grade-only mandate forbids stubs and demo modes. The oyatie cloud-intelligence assistant is a future integration with a known product boundary but no production adapter yet.

The earlier version of this ADR also described a future identity-provider seam. That was wrong for the current product: maintenance owns identity locally through passkey-backed accounts, and no external identity provider is part of the launch architecture.

## Decision
The oyatie AI assistant seam remains a port definition only: `AiAssistantPort` covers diagnosis suggestions and report drafting, with no mock adapter and no UI affordance until a real adapter is scheduled and owned.

Identity remains local and is not represented as a speculative external provider port. ADR-0022 records that correction and prohibits restoring an identity-provider contract without a named provider, owner, validated production contract, and implementation slice.

## Consequences
+ No dead AI adapter or false UI affordance ships.
+ Local identity authority stays clear: passkey-backed maintenance accounts are the product identity source.
− Future AI adapter work still needs a real implementation issue, production credentials/configuration, and adapter tests before UI exposure.

## Alternatives considered
Mock AI adapters for demos (rejected: violates the production-grade-only mandate); deferring the AI port definition entirely (rejected: the assistant boundary is already a named product seam and remains useful as an inward-facing application contract); keeping a speculative identity-provider port (rejected and superseded by ADR-0022).
