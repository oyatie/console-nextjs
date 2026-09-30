> **EXECUTABLE-CONTRACT / NON-AUTHORITY.** CI-bound gate contract (machine-checked). Cannot authorize product scope, clear HOLDs, release, or production readiness. Current product authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# NHIS EDI 4대보험 자격상실 generated-file fixture workflow

## Status

Fixture-only POC. This workflow prepares a deterministic generated-file artifact and evidence record for human review. It does not log in to NHIS/4insure/EDI, does not submit a filing, and does not use real resident registration numbers.

## Official-source basis

- NHIS official form page for 국민연금/건강보험/고용보험/산재보험 자격상실 신고: https://www.nhis.or.kr/static/html/wbdb/f/wbdbf0201.html
- NHIS EDI file-spec page for 사업장(직장)가입자 자격상실 신고서 and upload constraints: https://edi.nhis.or.kr/webedi/file_sy/all_sangsil.html

## Generated artifact

`scripts/korean-connectivity/nhis-edi-loss-report.mjs` renders a UTF-8 CSV fixture with Korean headers approximating the EDI loss-report data shape. The fixture intentionally uses `resident_registration_number_token` placeholders instead of real 주민등록번호 values.

The generated CSV is a scaffold for the connector/evidence workflow. A production-ready XLS/XLSX formatter and real filing payload are blocked until legal/security/ops approval, real customer consent, and a customer-controlled upload/signing path exist.

## Human-upload runbook

1. Operator creates a fixture or customer-reviewed draft from validated HR offboarding data.
2. Platform stores only intent hash, generated-file hash, source URLs, parser version, and redacted evidence.
3. A human reviewer confirms employer, employee rows, loss dates, insurance flags, and legal basis.
4. If this later becomes a live process, the customer/operator performs NHIS/EDI upload in their authorized session. The platform must not capture certificate passwords, session cookies, OTPs, security-card values, or browser storage.
5. Receipt/status evidence may be attached after manual upload, but only redacted identifiers and receipt metadata cross into the platform.

## Live filing prohibition

`assertNoLiveSubmission()` always throws in this fixture slice. Removing that guard requires a fresh legal/security ADR and connector state transition beyond `fixture_only`.
