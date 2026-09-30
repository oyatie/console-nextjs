---
id: ADR-0004
status: accepted
doc_status: published
date: 2026-06-12
owner: jasonlee
consensus: ralplan iteration 3 (Planner/Architect/Critic APPROVE, 2026-06-12)
related: []
---

# ADR-0004 — Passkeys-first auth with rotating refresh-token families + reuse detection

## Status
Accepted (consensus-approved plan §2.7).

## Context
Field technicians authenticate primarily on phones; office staff on shared/desktop browsers. Password reuse and phishing are the realistic threats. User decision: phone is the WebAuthn trust anchor; desktop uses platform authenticators (Touch ID/Windows Hello) or cross-device (hybrid QR) flows.

## Decision
webauthn-rs (server) passkey ceremonies for registration/login on all surfaces; short-lived ES256/EdDSA access JWTs (jsonwebtoken v10); opaque refresh tokens stored hashed with rotation-on-use, token-family tracking, and reuse-detection → family revocation. OTP/temp-credential bootstrap exists for cold-start enrollment at 300+ scale (T0.12) and as the shared-desk fallback; AASA + assetlinks.json served from the RP domain with both debug and release Android signing origins registered.

## Consequences
+ Phishing-resistant primary path; revocable sessions; no password database to breach.
− Passkey UX requires per-user enrollment logistics (provisioning task T0.12) and careful native-origin configuration (top failure mode — documented in plan §2.7).

## Alternatives considered
Password-first + optional passkeys (rejected: weakest path becomes default); OIDC via external IdP (rejected: no corporate IdP exists, and maintenance owns identity locally; see ADR-0022).
