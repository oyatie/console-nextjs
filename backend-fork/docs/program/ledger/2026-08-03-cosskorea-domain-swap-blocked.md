# Cosskorea domain swap — round-5 breaker disposition

## Verdict

**BLOCKED.** Correction round 5 was the fifth and final permitted fix loop. Both fresh whole-change reviewers returned `NEEDS_FIXES`, and independently reproduced load-bearing defects remain in the exact reviewed bytes. No sixth correction writer is permitted. Exact-byte acceptance verification did not run and must remain blocked.

This is a review disposition, not authorization to mutate repository bytes, publish a branch, operate Cloudflare/DNS/Kubernetes, reset credentials, perform WebAuthn/OTP actions, or touch production.

## Evidence identity

- Immutable base B: `9200e875b5362ef88b9a1af20dfc43ed3f07a970`
- Committed HEAD: `f3a0d9d8ead09e2151b3fd0a88eeb8638c43bb99`
- Original worktree: `/private/tmp/claude-501/-Users-jasonlee-Developer-console/e20cddb1-0286-44d1-b630-e8ec52be3200/scratchpad/wt-domain`
- Frozen correction brief SHA-256: `16431b6a7b83208b616ab66e6f3176d299813a27062e4b444720f2ed11b8438a`
- Frozen review package SHA-256: `49ede562f928ef98831467ee5ac93b92eaa33f6725856f5c5d2600a2d0c1696f`
- Original breaker report SHA-256: `e6c04c186cde11409bdec94f8f169d9a61529d3cc200f377d049cbc7f67612f7`
- Final reviewed repository hashes:
  - `b2945de234225345700266fee352e2ce69ce45db703d9e0eb08abaf248a2d092  ops/launch/multi-tenant-cutover-runbook.md`
  - `88d3364bc53959a79b0e76fc43a67010b96bd99981f210e6e9e91bece872f171  deploy/infra/cert-manager/cluster-issuer.yaml`
  - `8d26e5c55d1440d6cfc498494154d26b947f4cb56e6310405e66428930633e2b  docs/ios-swift-client-drift-handoff.md`

Controller verification before package freeze established unchanged branch/ref/HEAD, exactly 13 B-to-deliverable paths, middleware as the sole deletion, 11 preservation paths B-exact, clean `git diff --check`, 14/14 shell fences parsed, exact-worktree hardening 57/57, evaluator 234/234, and clean deploy Bash syntax/ShellCheck. Those green checks qualify the behaviors they exercise; they do not overrule the independently reproduced review defects below.

## Sustained load-bearing findings

### 1. Rollback certificate cleanup always destroys its saved status

The rollback cleanup handler assigns `rc=$?`, unsets `rc`, then executes `exit "$rc"` under nounset. Both success and failure become `rc: unbound variable`, so the rollback certificate gate cannot complete and the legacy DNS restoration boundary is unreachable.

- Location: `ops/launch/multi-tenant-cutover-runbook.md:765-794` (defect at approximately 770-772)
- Independent reproduction: 10/10 confidence; inert execution exits nonzero with `rc: unbound variable`
- Classification: operational correctness / rollback availability
- Minimum future correction: retain `rc` until after `exit "$rc"`; add a copied-function success and failure fixture for this exact rollback handler

### 2. Recovery evidence is not bound to the current ceremony, execution, or new credential

The gate starts a fresh registration ceremony, then accepts externally supplied finish-status, finish-response, and fresh-login files that only match user/org/origin and contain nonempty IDs. It never compares those artifacts with the current `ceremony_id`, rejects pre-existing files, proves they were produced by this execution, or correlates the fresh login with the newly returned credential/passkey. Reset deletes every passkey, so stale success evidence can advance the gate while the target still has zero passkeys.

- Location: `ops/launch/multi-tenant-cutover-runbook.md:616-643`
- Independent corroboration: 10/10 confidence
- Classification: operational recovery-proof correctness; not separately classified as a confidentiality/integrity exploit under trusted evidence-file assumptions
- Minimum future correction: a newly produced evidence envelope carrying the exact current `ceremony_id`, execution nonce/time, finish request/result, returned credential/passkey ID, and fresh login bound to that new credential; reject/remove pre-existing evidence and wait for new evidence

### 3. Ordinary-user recovery incorrectly requires `UserManage`

The reusable recovery gate always calls authenticated `GET /api/v1/users?limit=1` using the target bootstrap bearer. The backend protects this route with `authorize_org_manage(..., Feature::UserManage)`. The runbook prescribes the same gate for ordinary users, who therefore redeem OTP and then necessarily fail with 403 before registration.

- Runbook location: `ops/launch/multi-tenant-cutover-runbook.md:582-587`
- Backend contract: `backend/crates/identity/rest/src/lib.rs:2857-2881`
- Classification: authorization-contract / recovery correctness
- Minimum future correction: keep server-resolved `/me/authz` user/org binding for every target, but require the separate `/users` capability proof only for administrative anchors or perform it with the authorized reset operator rather than an ordinary target

### 4. Operator automation records the target's legal acceptance

The procedure redeems a target bootstrap bearer and posts both `privacy_collection=true` and `terms_of_service=true` when status is absent. The backend derives identity from that bearer and writes the append-only `privacy.required_accept` event for the target. Possession of a recovery bearer is not evidence that the target person received the current notice and affirmatively accepted each agreement.

- Runbook location: `ops/launch/multi-tenant-cutover-runbook.md:597-606`
- Backend contract: `backend/crates/platform/auth-rest/src/lib.rs:2010-2060`
- Classification: legal/audit provenance and unsafe privileged mutation; not a distinct attacker primitive
- Minimum future correction: operator automation may query/prove status only; present the exact current notice and capture each target-user affirmation through an authoritative secure client interaction

### 5. Certificate/Secret state is not proof of the leaf Traefik serves for each SNI

Forward and rollback gates validate the cert-manager generation, desired SANs, and `Secret/console-tls`, then make the DNS boundary reachable without connecting to Traefik. Informer/reload lag, router admission failure, or default-certificate selection can therefore expose a different leaf despite correct Kubernetes objects.

- Forward location: `ops/launch/multi-tenant-cutover-runbook.md:367-412`
- Rollback location: `ops/launch/multi-tenant-cutover-runbook.md:754-800`
- Serving chain: `deploy/apps/console/base/ingress.yaml:7-19`, production patch `deploy/apps/console/overlays/prod/kustomization.yaml:35-49`, direct Traefik origin configuration `deploy/apps/traefik-oci-guest/values.yaml:7-35`
- Independent adjudication: sustained, approximately 95% confidence
- Classification: operational TLS/availability readiness
- Minimum future correction: before each DNS boundary, connect directly to the origin on 443 with every expected hostname as SNI, require successful hostname/chain validation, and compare the served leaf DER/SHA-256 exactly with the Secret leaf; additionally prove expected Host routing where an inert endpoint exists

### 6. Credential reset preserves attacker-held pre-reset refresh sessions

The final runbook explicitly preserves pre-reset refresh families. Reset deletes all passkeys and creates an OTP but does not revoke target sessions. A holder of another pre-reset refresh token can rotate it after reset, obtain a bearer, satisfy privacy state, use the zero-passkey registration path to enroll an attacker authenticator, and consume the open recovery credential without possessing the OTP. This is a concrete takeover path, not merely generic defense-in-depth.

- Runbook location: `ops/launch/multi-tenant-cutover-runbook.md:668-673`
- Reset source: `backend/crates/platform/provisioning/src/lib.rs:292-395`
- Refresh source: `backend/crates/platform/auth-rest/src/lib.rs:2117-2185`
- Zero-passkey registration source: `backend/crates/platform/auth-rest/src/lib.rs:823-943`
- Independent security adjudication: valid, 9-9.5/10 confidence
- Classification: HIGH authentication/session-invalidation vulnerability
- Supersedes the earlier `NOT_SUSTAINED` ruling, which treated revocation as generic compromise hardening before this exact takeover trace was established
- Minimum future correction: atomically revoke every refresh family/token for the reset target with passkey deletion and OTP minting. This preserves the two-anchor order because resetting B leaves A's family intact, and resetting A leaves B's newly redeemed family intact

## Adjudicated candidate: serving-plane transition ordering

The factual observation is retained: applying the coss-only Ingress/certificate ends legacy HTTPS service while legacy DNS may still resolve; rollback performs the inverse. Independent reviewers disagreed on whether that is itself a defect.

Final ruling for this package: **accurately held maintenance/downtime tradeoff, not an additional hidden blocker**, because the runbook enters maintenance before the mutually exclusive one-Ingress/one-Secret generation swap and forbids ending maintenance while external-client/no-supported-client evidence is absent. Legacy DNS retention provides reversibility, not continuity.

This ruling is conditional on maintenance outage being acceptable. If uninterrupted legacy-client continuity is a requirement, the candidate becomes a sustained defect and dependency inventory/retirement authorization must precede the coss-only swap (or an explicitly designed overlapping serving plane must exist). The final runbook should state this tradeoff plainly in any future correction.

## Other adjudications

- Fixed 10-second deploy-harness nondeterminism: real, inherited, and separate CI-hardening work; not a round-5 domain defect. Preserve all red/green history.
- Cloudflare verify/zone-list calls: insufficient for DNS Edit authority; exact token-policy/resources and permission-group evidence or tied dashboard policy evidence remains required. No mutating permission test.
- Current client: no authoritative frontend/native source, immutable build provenance, secure bootstrap handoff, or browser E2E exists in this tree. Destructive recovery and registration completion remain HOLD.
- Privacy-consent finding: retained as legal/audit correctness even though a security-only filter correctly noted that it does not create a new attacker primitive.

## External HOLDs preserved

- No live A/B immutable identity/org evidence or two independent identity-bound recovery anchors.
- No authoritative current client source/build/owner/supported-release inventory, secure bootstrap handoff, or browser E2E.
- No live deployed-contract proof for identity, reset/redeem, privacy, registration, or privileged target authority.
- No live Cloudflare token-policy proof for both exact zones and both required permissions.
- No current-cluster cert-manager capability/current-generation proof and no served-SNI proof.
- No live DNS, proxy-mode, RP/origin, endpoint-health, certificate, OTP, WebAuthn, or recovery evidence.
- Legacy web DNS retirement remains HOLD pending authoritative external-client evidence or authoritative proof that no supported client depends on legacy hosts.
- All knllogistic mail configuration and DNS remain untouched.
- Korea's six controls remain HOLD pending qualified Korean legal/compliance authority.
- The OCI Ampere A1 must never be destroyed, terminated, resized, or reprovisioned.

## Breaker action

- Repository writer count remains zero for the blocked domain attempt.
- Do not alter the round-5 repository bytes under that task.
- Do not dispatch correction round 6.
- Mark whole-change review as `BLOCKED`.
- Do not run task #3 exact-byte acceptance verification.
- A future attempt requires a newly approved plan/brief and a fresh evidence lineage; it cannot claim continuation of the exhausted five-round loop.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "review",
  "risk_class": "high",
  "risk_domains": [
    "authz",
    "release",
    "production",
    "compliance_sensitive"
  ],
  "selected_lenses": [
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Red Team": "Reproduced the refresh-session takeover path and stale-evidence failure modes.",
    "Systems Thinking": "Traced identity, consent, certificate, DNS, and rollback boundaries together.",
    "Operability / Day-2": "Required executable rollback, recovery, and served-certificate evidence.",
    "Blast-radius / cell-based": "Kept all live domain and credential mutations on HOLD.",
    "Telemetry-first": "Bound the disposition to exact hashes, checks, and independently reproduced findings.",
    "Zero-trust / defense-in-depth": "Rejected unbound ceremony evidence, preserved sessions, and object-state-only TLS proof."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Six independently sustained defects make the reviewed domain-swap implementation unsafe to merge or operate."
  ],
  "decisions_changed_or_rejected": [
    "Rejected a sixth correction loop and preserved only the blocker disposition, not the unsafe implementation."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
