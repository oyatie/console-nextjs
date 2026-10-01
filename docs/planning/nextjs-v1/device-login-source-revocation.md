# Current recorded passkey authority at QR login consumption

Base: `62a9c3b8f1b4f95e78583f32138dbc25704e29fb`.
Worktree: `device-login-source-revocation`; root is the sole source/cache writer.
No original Console, shared-root fork, deployment, secret or live service writes.

## Observed workflow and bounded owner change

A desktop starts `/api/v1/auth/device-login/start`, a phone approves through a
signed WebAuthn assertion or the existing session approval route, and the
held desktop poll token calls `/api/v1/auth/device-login/poll` for a session.
Both approval routes persist `approved_passkey_id`. The poll owner locks the
Account and handoff, compares the recorded identities and issues a family,
but does not read that exact credential again. The transient handoff has no
credential foreign key. Removing approving key A while retaining key B can
therefore leave a stale recorded UUID which polling accepts. This is source
inference until the independently reviewed real HTTP/PostgreSQL probe fails.

Extend only `poll_device_login`: after the existing Account/handoff locks and
identity comparisons, require a non-null recorded key which is currently in
`auth_webauthn_credentials` with the exact approved Account and Company.
Reuse the existing Account mutex: both self-removal aliases, reset and
Account deactivation already hold it before credential removal. Keep the
credential check before the existing post-lock database-clock sample and
before consumption, family/token/signature issuance or consume-audit insert.
Missing/foreign/removed/null key returns the same generic 401. Database
failure remains a real error and rolls the transaction back. A surviving
other key is never substituted for the recorded key.

Do not change approval routes, family/refresh semantics, JWTs, request DTOs,
rate limits, cookies, audit payloads, schema/migrations, dependencies or UI
exposure. The session approval route still records its existing latest key;
this repair does not claim that it has a fresh signed assertion of that key.
Existing pending/expired, identity-change, inactivity, single consumption,
clock, Account-first locking and audit failure behavior remain mandatory.

## Test-first proof

Reuse `console-identity-rest`'s `authentication_locking` target and its real
runtime-role routers, signed SoftPasskey assertion fixture, Account-wait
observer and family-bound self-removal helper. Fixtures exist only in
isolated disposable PostgreSQL and are not production records or browser
proof. Keep every existing test unchanged.

1. Exercise both real approval producers: signed approval by A and the
   bound-target session approval which records A as its latest key. Retain B,
   remove recorded A through each real self-removal HTTP alias, then require
   repeated polling to deny without consuming the handoff, creating a new
   family/token or consume audit. Account for existing authenticated-family
   baselines. B must still complete genuine signed HTTP passkey login.
2. Hold the existing Account lock, start real polling, observe that exact
   production Account query blocked by the controller, then remove A with
   the shared audited owner in that transaction and commit. Polling must
   recheck after the wait and deny with no downstream issuance; B survives.
3. After genuine approval, controlled corruption of the recorded key to
   null, another Account's key or another Company's key must deny without
   substituting any retained key. Such corruption is a hostile fixture,
   not a supported public action.
4. For both approval producers, retain recorded A and add newer B; polling
   still succeeds using A exactly once. A replay cannot create another family,
   token or consume audit. A latest-key/count substitute is not acceptable.
5. Run the complete unchanged-plus-new target, the relevant auth owner and
   identity owner checks, strict Clippy, formatting and source/migration
   custody. Extend CI with this target and preserve every existing producer.

Pin design and exact tests through independent adversarial review. The same
new tests must fail at the intended denial assertions on unchanged source,
then pass unchanged after implementation. Observe actual blocked query and
controller before concurrent mutation; never treat sleep as synchronization.
Record commands, discovered/executed counts, hashes, invalid attempts,
cleanup and honest existing auth HOLD failures. A compile/setup/authority
failure is not admitted behavioral RED. Obtain final exact-candidate source
and evidence approval and a revision-bound COMMENT before protected queue.

## Risk, rollback and stop conditions

Blast radius is one normal-session producer. Before any deployment, revert
this bounded source/CI change while retaining evidence; it needs no schema
or data rollback, but reintroduces stale recorded-key acceptance. Stop for
an unreviewed public/authentication semantic change, false RED, weakened
Company/Account authority, lost valid-key login, skipped required probes,
source/test drift, unsafe mixed writers or unavailable protected queue.

The existing signed-authentication purpose/provenance, genuine add-device
intent, operation/status/lost-response custody, conservative historical
family classification, writer fencing and two-site confirmation HOLDs
remain. No Next business session, production prototype, attended identity,
legal approval, full HR/Org/Payroll/Foundry or release acceptance is claimed.
