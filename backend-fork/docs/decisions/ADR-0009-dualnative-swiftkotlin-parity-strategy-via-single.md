---
id: ADR-0009
status: accepted
doc_status: published
date: 2026-06-12
owner: jasonlee
consensus: ralplan iteration 3 (Planner/Architect/Critic APPROVE, 2026-06-12)
amended_by: [ADR-0031]
related: [ADR-0012, ADR-0031]
---

# ADR-0009 — Dual-native (Swift+Kotlin) parity strategy via single OpenAPI contract + CI parity gate

## Status
Accepted (consensus-approved plan §2.9).

> **Current reconciliation (2026-08-03):** This accepted record is retained as
> historical decision text. The canonical pivot removed `web/`, `clients/`,
> `ios/`, and `android/`; those clauses do not authorize recreating deleted
> surfaces. ADR-0031 amends only the contract mechanism. Current work must follow
> [`docs/PIVOT-2026-07-28.md`](../PIVOT-2026-07-28.md), and any further ADR-graph
> reconciliation must be additive rather than another rewrite of the decision.

## Context
The user chose native Swift (iOS) + Kotlin (Android) apps over a single React Native codebase (informed decision, interview R6) for maximal mobile UX. Two codebases create parity-drift risk — the plan's #1 pre-mortem scenario.

## Decision
Parity is enforced structurally, not by discipline. **Target, per ADR-0031 and not yet built:** one `openapi.yaml` **emitted** from the wire-DTO contracts crate, with a CI gate failing when the committed document differs from the emitted one. **What HEAD has, stated so this clause is not read as describing the present:** no contracts crate exists, no emitter exists, and `openapi.yaml` is hand-maintained and served verbatim via `include_str!` in `backend/app/src/lib.rs`; `check-openapi-app.mjs` compares the app-served document to the committed file, which `include_str!` makes tautologically equal, so nothing today verifies an emitted document against a committed one. **Withdrawn as describing deleted surfaces:** ts/swift/kotlin client generation and its drift gate (T1.9), the dual-build gate over both apps (T1.8), and per-slice web+Android-then-iOS sequencing — `clients/`, `web/`, `android/` and `ios/` are all absent from HEAD and the generator scripts are gone from `package.json`. **Retained:** the per-release parity checklist gating release, for the surfaces that exist.

## Consequences
+ Best platform UX (camera pipeline, push handling, passkeys are platform-native anyway).
− Roughly 2× client implementation cost per slice — accepted by the user explicitly; the contract/codegen machinery is the mitigation, and it has its own CI gate.

## Alternatives considered
Expo/React Native single codebase (research-recommended; user overrode); RN + native modules hybrid (offered; declined).
