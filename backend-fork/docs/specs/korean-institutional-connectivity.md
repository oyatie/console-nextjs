> **EXECUTABLE-CONTRACT / NON-AUTHORITY.** CI-bound gate contract (machine-checked). Cannot authorize product scope, clear HOLDs, release, or production readiness. Current product authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Korean institutional connectivity foundation spec

## Goal

Create a connector coverage factory for Korean institutional APIs and workflows: banking/open banking, Financial MyData/partner rails, NTS/tax paths, NHIS/COMWEL/4insure public/labor filings, and certificate-backed workflows. This is a foundation slice, not a live filing or live banking product.

## Boundary statement

This spec implements the approved official-rail-first hybrid direction. It rejects aggregator runtime dependencies and central credential custody. It allows public official API/documentation scraping only for source discovery and deterministic fixture generation.

## Coverage catalog record

Each catalog entry must include:

- `institution_id`: stable lowercase id.
- `domain`: `banking`, `tax`, `public_labor`, `insurance`, `securities`, `cards`, or `local_agent`.
- `workflow_ids`: stable workflow ids exposed by the connector.
- `auth_modes`: one or more of `official_oauth`, `mydata_token`, `nts_asp_certified`, `local_agent_cert_session`, `manual_file_upload`, `fixture_only`.
- `side_effect_class`: `read_only`, `generated_file`, `human_approved_filing`, `automated_filing`, `payment_transfer`, or `certified_issuance`.
- `legal_status`: `approved`, `counsel_review`, `partner_required`, `prohibited`, or `research_only`.
- `capability_state`: `research_only`, `fixture_only`, `sandbox`, `partner_approved`, `live_read`, `live_write`, `prohibited`, or `deprecated`.
- `fixture_path`: repo-local deterministic fixture path; required before leaving `research_only`.
- `evidence_policy`: receipt/status, redacted transcript, parser version, intent hash, source URLs, data classification, and retention class.
- `source_evidence`: official URLs or source documents with `source_type`, `fetched_at_policy`, `license_or_terms_status`, and `scraping_allowed_scope`.
- `forbidden_data`: explicit list of credential/session artifacts that must not cross into the platform.

## Official-source/API scraping policy

Allowed scraping scope is narrow and public. The research scaffold may fetch or cache public official documentation pages, OpenAPI/Swagger specs, static HTML docs, public file-format specs, and sandbox/testbed references. Each fetch must be read-only, URL-attributed, rate-limited, reproducible, and stored as source metadata or a deterministic fixture.

Forbidden scraping scope includes authenticated customer portals, production banking/tax/public sessions, customer browser sessions, certificate dialogs, institution security-plugin internals, keyboard security modules, MFA prompts, anti-automation bypasses, raw session cookies, browser storage, passwords, OTP/security-card values, real certificate files, or private customer/institution data.

## Trust-zone contract

Allowed data crossing from local-agent/customer session to platform is limited to signed result/challenge response generated locally under explicit consent, receipt/status identifiers, generated documents, redacted transcript excerpts, attested metadata, and encrypted delegated official tokens issued by official API or partner rails.

Forbidden data crossing includes 공동인증서/private keys, PFX files, signPri.key/signCert.der pairs, certificate passwords, OTPs, security-card values, bank passwords, raw bank credentials, session cookies, browser local/session storage, unredacted HTML containing identifiers/secrets, and institution security-plugin internals.

The platform must not proxy, intercept, replay, or bypass customer browser sessions, institution security plugins, anti-automation controls, keyboard security modules, certificate dialogs, or MFA prompts.

Local certificate signing must happen in a customer-controlled local agent. The platform receives signed login proof envelopes only: challenge hash, signature, certificate fingerprint, and attested metadata. Raw signPri.key/signCert.der files, certificate passwords, and production session material must not cross into the platform.

## Initial catalog exemplars

The fixture catalog starts with four source-backed exemplars:

1. KFTC Open Banking account and transaction shape in `sandbox` or `fixture_only`, using `official_oauth` and no real bank credentials.
2. Financial MyData standard API shape in `research_only` or `partner_approved`, using `mydata_token` only after licensed/partner gates.
3. NHIS EDI 4대보험 자격상실 generated-file workflow in `fixture_only`, using `manual_file_upload` and no live filing.
4. NTS e-tax invoice ASP/ERP readiness in `research_only`, using `nts_asp_certified` only after certification/partner gates.
5. Local-agent certificate/session simulator in `fixture_only`, using `local_agent_cert_session` but only simulated signatures and allowed evidence.

## Transition gates

`research_only -> fixture_only` requires source evidence, data classification, fixture plan, and no-login/no-secret proof. `fixture_only -> sandbox` requires official sandbox/testbed access, no-live-network tests, redaction tests, and consent model. `sandbox -> partner_approved` requires legal/business enrollment or partner approval. `partner_approved -> live_read` requires DPIA/security review, audit logging, data retention, incident playbook, and user consent UX. `live_read -> live_write` requires side-effect legal signoff, dual approval, idempotency/rollback strategy, receipt evidence, and operations runbook. Any state can move to `prohibited` when forbidden data-flow or institution-term violations are found.

## Implementation notes for later stories

Later stories should add fixture-only adapter interfaces, evidence ledger persistence, a public-doc scraping scaffold, Open Banking fixture connector, local-agent simulator, and NHIS generated-file workflow. This G001 artifact only defines the policy, schema, and gate that those stories must satisfy.
