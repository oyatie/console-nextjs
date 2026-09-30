> **EXECUTABLE-CONTRACT / NON-AUTHORITY.** CI-bound gate contract (machine-checked). Cannot authorize product scope, clear HOLDs, release, or production readiness. Current product authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Local 공동인증서 certificate agent POC

## Purpose

Prepare the safe local signing boundary for institution login adapters that receive a challenge/nonce and require a 공동인증서-style proof. This slice is deliberately a proof generator and contract test, not a live 4대보험, bank, tax, or government submission client.

## Non-negotiable boundary

signPri.key, signCert.der, and certificate password never leave the local agent. The platform receives signed login proof envelopes only: challenge hash, signature, certificate fingerprint, and attestation metadata. The platform must not store or transmit encrypted private keys, certificate files, certificate passwords, browser cookies, browser storage, OTP values, security-card values, or raw institution session material.

## Fixture POC behavior

The committed POC supports a fixture-safe standard encrypted PKCS#8 RSA private key surrogate and a DER public-key/certificate surrogate. It proves the end-to-end mechanics:

1. Server/connector prepares a canonical challenge with institution id, workflow id, nonce, session id, scopes, expiry, and purpose.
2. Customer-controlled local agent signs the canonical challenge locally.
3. Local agent returns only a signed proof envelope.
4. Server verifies that the envelope contains no credential material and can route the proof to a fixture connector.

This module is not a production 공동인증서 CMS/VID implementation. Real Korean 공동인증서 portal adapters may require institution-specific CMS/PKCS#7 signing, VID handling, NPKI encrypted-key parsing, certificate chain checks, and legal/terms approval before any sandbox or live use.

## CLI safety

Default CLI mode generates an ephemeral fixture key pair and writes only a proof envelope under `.tmp` or another caller-provided output path.

A local-only manual mode exists for future lab validation:

```sh
KIC_CERT_PASSWORD='...' npm run kic:local-cert-login-fixture -- \
  --allow-local-key-files \
  --sign-pri-key /local/path/signPri.key \
  --sign-cert-der /local/path/signCert.der \
  --cert-password-env KIC_CERT_PASSWORD \
  --out .tmp/local-cert-proof.json
```

Do not pass the certificate password as a command-line argument. Do not send real key files or passwords to the platform server. This command performs no network calls and no live filing.
