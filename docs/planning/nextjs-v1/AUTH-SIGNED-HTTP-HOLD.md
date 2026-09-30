# Signed HTTP authentication cutover — HOLD

Status: **HOLD; no serving activation or acceptance.** This note records the
observed owner gap at backend-fork `956a3aa20cf13fc6fdf5014a4bbf7d8ed827473c`.
It supplements [OTP-CUTOVER.md](OTP-CUTOVER.md); it does not change the saved
v1 product intent, authorize a narrower guard, or approve a migration. The
independent Console source and live services were not changed for this note.

## Reproducible red evidence

From `backend-fork`, the admitted signed HTTP probe was:

```sh
bash tools/lanes/pgtest.sh /Users/jasonlee/Developer/frontend/backend-fork env SQLX_OFFLINE=true cargo test -p console-app --test auth_rest --locked
```

Log: `/private/tmp/auth-v1-signed-http-red-0238.log`, SHA-256
`8c8d39ca498cf4713d57fdd8a9094102e052029cf93c10fe622b52386a5d4347`.
The disposable PostgreSQL run discovered and executed **28** tests: **23
passed, 5 failed, 0 ignored, 0 filtered**. Each failure reached a signed HTTP
authorization assertion; later assertions in those tests are not proven.

| Failing test | Observed boundary |
| --- | --- |
| `first_passkey_registration_rejects_revoked_bootstrap_after_start` | Registration finish returned 201 after the exact OTP was revoked; expected 401. |
| `first_passkey_registration_rejects_replaced_source_after_refresh` | Finish returned 201 after its OTP source was replaced; expected 401. |
| `otp_enrollment_session_cannot_create_business_work` | An OTP-derived access session created a work order (201); expected denial. |
| `otp_family_cannot_gain_business_authority_after_passkey_registration` | Refresh of an OTP-derived family created business authority (201); expected denial. |
| `replaced_otp_family_cannot_create_business_work` | A superseded OTP family's access token created a work order (201); expected denial. |

## Current owner map

| Boundary | Current owner and observed gap |
| --- | --- |
| OTP issuance and source | `provisioning/src/lib.rs` still issues version-0 OTPs through the live paths. Its version-1 first-enrollment source checks are dormant and do not classify recovery or enrolled-device handoff as first enrollment. |
| Proof, admission, family and audit | `FirstEnrollmentCommandPool::admit` locally commits exact version-1 source consumption, an enrollment family, receipt and fixed audit in one restricted transaction. `console_auth_cmd` remains `NOLOGIN`; no HTTP route calls this pool. Migration 0235's deferred guards protect this dormant transaction, not live OTP authority. |
| HTTP bearer issuance and refresh | `auth-rest/src/lib.rs::redeem_otp` redeems a version-0 proof, then separately calls ordinary `issue_token_pair`; `refresh_token` rotates without preserving or checking an enrollment purpose. `approve_device_login_session` checks a live family and passkey, not a current normal-purpose family. |
| Request admission | `request-context/src/lib.rs::validate_current_claims` checks a live family and Account authority but no family purpose, provenance or private authentication generation. A display-only passkey-setup flag cannot restrict business authority. All API, realtime, SSR and export entries need the same current check. |
| Passkey registration | `auth-rest/src/lib.rs::finish_registration` inserts a key then consumes any open OTPs in its transaction; it has no immutable registration intent bound to the accepted source, issuance version, family and generation. The consume can affect zero rows. |
| Durability and activation | The version-1 command returns a **local commit**, not a releasable bearer. Migration 0236 supplies an inert epoch marker and a confirmation primitive; two-site confirmation, current writer/egress fencing and a safe serving switch are not proven for this path. |

## Minimum coherent cutover before serving

1. Inventory all live OTP issuers, Account lifecycle/recovery, add-device and
   desktop/mobile handoffs. Preserve version-0 provenance and classify existing
   families without inferring first enrollment from a current zero-key count.
   Use separate typed proof paths for genuine first enrollment, existing-key
   addition, enrolled-device handoff and approved identity recovery.
2. Make the version-1 issuer and restricted command connection real only after
   its Company-scoped permissions, exact source issuance operation and Account
   generation are proved. A client-held operation ID and random status secret
   must precede proof consumption. The command must atomically consume that
   source once, mint only an enrollment family, finalize the admission receipt
   and append its audit. A lost response resolves by operation ID without
   silently minting another family or reconstructing an unavailable bearer.
3. Bind registration start to an immutable intent naming the accepted
   admission/family, exact OTP source and issuance, Account/Company, generation
   and ceremony. Finish must recheck those facts after locks and commit key,
   `ever_enrolled` transition, consumed intent and audit together. Source
   replacement/revocation must fence dependent enrollment authority. A
   different later OTP cannot rescue an earlier ceremony. Existing-key addition
   uses its distinct signed step-up path.
4. Make normal passkey/device proof issue purpose-bound normal families and
   make refresh retain immutable purpose and generation. The current-request
   validator and every consuming entry, including business, platform, Group,
   realtime, SSR and export, must reject enrollment/unresolved families except
   for exact enrollment/consent actions. Version-0 business denial and normal
   issuer activation move together; an old issuer must not mint a family that
   the new validator rejects, nor can an old validator accept a restricted one.
5. Complete the two-independent-site post-commit WAL confirmation barrier and
   monotonic writer/egress fencing before any bearer, receipt, HTTP success or
   downstream effect is acknowledged. Prove lost-response reconciliation and
   positive old-process fencing. Then switch issuers, validators and refreshers
   under one owned epoch. Rollback must use a purpose-aware compatibility build
   or revoke/fence every family the old binary cannot safely interpret.

Reserve auth schema changes after the mail-owned migration 0239 and review
their applied-byte/rollback compatibility. Stop if any step leaves mixed old
and new bearer semantics, missing current generation/source authority, a
locally committed but externally acknowledged operation, or an unfenced old
sender. The five signed HTTP tests must turn green **with their later
assertions executed**, alongside real normal-login, recovery, handoff, Company
isolation, direct-request, two-site fault and rollback proof. Documentation
alone does not clear this HOLD.

## Current fork checkpoint — 2026-09-27

The reviewed version-0 containment commit `fccad07` closes the revoked/replaced-source registration races for **newly redeemed** OTP families. Its exact signed HTTP suite executes 30 tests: 28 pass and two still fail because OTP-derived families can create business work with HTTP 201 before and after passkey registration/refresh. Historical unbound families are not silently classified; their first registration requires fresh OTP redemption. Migration `0240` and the parser correction pass the current-source PostgreSQL 18 install/upgrade and classification gates, but this is not the version-1 owner/purpose cutover or a two-site acknowledgment.

Two further signed test-only candidates remain isolated and unmerged. `2936f86` proves a passkey A can authorize new-device registration, be revoked while passkey B remains, and still let the pending registration finish with HTTP 201 instead of required 401. The source binding records no exact step-up key, consumed proof instant or deadline. Its red PostgreSQL log is `.artifacts/acceptance/stepup-revocation-fccad07/probe.log` (SHA-256 `f910140031c2e12769e628b9982d05e9254671acc643ac3ea0dde8b5c8aa5d13`; 1 failed, 30 filtered). `aa5075b` proves the advertised established-user cross-device path cannot finish: A's valid step-up issues the handoff, the phone redeems its OTP, then phone passkey registration start returns 401 instead of 200 because the route demands a local old-device step-up. Finish/login remain unreached. Its red log is `.artifacts/acceptance/handoff-established-fccad07/probe.log` (SHA-256 `e5d50aa3bb0ab76550625e8a006e779023f9798bdc115456bd7d0e8e271437be`; 1 failed, 31 filtered). Independent read-only review accepted both failures as real.

A direct-registration-only proof-binding patch would not close the QR handoff path. A joined design must bind the server-verified step-up credential/ceremony and accepted time to an exact scoped handoff or registration intent, authorize the destination device without converting its OTP to general business authority, and recheck current proof/Account authority before key activation. The phone's `requires_passkey_setup` flag currently derives from zero-key count and is false for established-user handoff; it cannot be the authority source. Mixed old writers or rollback to a binary that accepts unbound intents remain unsafe until fenced. Do not claim add-device or v1 authentication acceptance from the contained direct route alone.

## Isolated purpose and source probes — 2026-09-27

The current integrated fork remains `b8c749fb01be788d249791513d810fd0d6132fe0`. Exact isolated candidate `cb462d070f36703c8415c648f1817ad4128e4856` adds **tests only** to `backend/app/tests/auth_rest.rs`; its diff from that base changes no product source or migration. The disposable PostgreSQL signed HTTP suite discovered and executed **33** tests: **28 passed, 5 failed, 0 ignored, 0 filtered**. The copied [receipt](../../../.artifacts/acceptance/auth-purpose-cb462d0/receipt.json) and [log](../../../.artifacts/acceptance/auth-purpose-cb462d0/postgres-red.log) preserve the exact command and outcomes. Log SHA-256: `c9af947ebaa1613ca3468567e6371d2ceb92ce8f63761534c2c7c935b80bb04b`.

| Failing test | Observed boundary |
| --- | --- |
| `enrollment_handoff_rechecks_source_after_account_wait` | Revoking the OTP source during the Account-lock wait still permits a new enrollment handoff. |
| `otp_enrollment_session_cannot_create_business_work` | The enrollment-only session creates a business work order: HTTP 201. |
| `otp_family_cannot_gain_business_authority_after_passkey_registration` | An OTP family refreshed after key registration creates business work: HTTP 201. |
| `privacy_consent_rechecks_otp_source_before_append` | Revoking the OTP source before consent append does not stop acceptance. |
| `unbound_legacy_family_cannot_mint_enrollment_handoff` | An unclassified historical family accepts first-login terms; the later handoff assertion is unreached. |

Independent reviewer `/root/auth_purpose_review` reported **ACCEPT for this exact red test-only evidence** after checking the candidate diff and log hash. That verdict accepts the failing probes, not an authentication implementation, serving switch or release. The five tests stop at their first failing assertions; later consequences are unreached. The earlier contained version-0 source tests remain green in this isolated run. The proposed partial source patch and schema draft in the receipt were **not integrated** because they do not provide a coherent source, purpose, current-generation and atomic family/consent owner boundary.

The HOLD covers both existing and historical families, and all routes that consume their authority. Complete exact-source validation under the Account lock, immutable enrollment versus normal purpose, privacy terms and registration in their owning transaction, and current-source checks after waits. Preserve separate proved add-device/handoff intent; do not infer authority from zero passkeys or a UI setup flag. Cutover and compatible rollback must fence old issuers, validators, refreshers and egress, then pass the two-site confirmation and signed HTTP/fault suites. No product source from this candidate was merged, and the new failures are **not** a regression introduced into the integrated fork.
