# Secrets

> **POST-PIVOT UNVERIFIED / HOLD:** The secret-store and projection procedures
> below are retained as operator and historical reference; they are not proof of
> current deployment state and do not authorize reading, rotating, copying, or
> projecting any production secret. The repository currently authorizes zero
> production mutations. Start with the
> [disk-wipe consolidation handoff](../docs/handoffs/2026-08-03-disk-wipe-consolidation.md).

Secret values never live in git. Their source and Kubernetes projection path are
deployment-context specific: use the `oci-guest` instructions only for the live
OCI guest, and use the `on-prem-ha` OpenBao/External-Secrets contract only after
that DARK context is activated. Do not combine the two procedures.

## Deployment-context secret-store contract

| Context | Acceptable secret store and projection path |
|---|---|
| `oci-guest` (live) | **OCI Vault** is the authoritative recovery store for Talos, kubeconfig, app, database, and OCI Object Storage credentials. Operators project the needed values into Kubernetes `Secret` objects (`console-secrets`, `oci-objectstore-creds`, `console-db-rt`, and namespace-specific integration secrets) with one-time `kubectl create secret` commands. This is honest for the current single-node guest; it is not an automatic GitOps secret controller. |
| `on-prem-ha` (ADR-0024 / DARK until activation) | **OpenBao HA Raft + External Secrets Operator** is the expected production secret root and Kubernetes projection path. OpenBao must be initialized, unsealed, audited, backed up, and operated by named custodians before production data moves. CNPG Barman, evidence S3, app, mail, and integration credentials should be projected from OpenBao/ESO into context-local Kubernetes secrets; OCI Vault is allowed only as the previous `oci-guest` rollback source, not as a requirement for on-prem HA. |

Never commit, log, paste, or checkpoint secret values, OpenBao unseal shares, root
tokens, OCI customer-secret keys, Talos secrets, kubeconfigs, or generated JWT
keys. Sealed Secrets remains an acceptable per-context alternative only after a
specific activation decision; do not treat it as already deployed.

## `console-secrets` — application secrets

Consumed by `console-app` / `console-worker` via `envFrom`. Required keys:

| Key | What |
|---|---|
| `CONSOLE_JWT_PRIVATE_KEY_PEM` | ES256 private key (signs access/refresh JWTs) |
| `CONSOLE_JWT_PUBLIC_KEY_PEM` | ES256 public key (verifies JWTs) |
| `CONSOLE_S3_ACCESS_KEY_ID` | Context-local S3-compatible access key (OCI Object Storage on `oci-guest`; SeaweedFS on `on-prem-ha`) |
| `CONSOLE_S3_SECRET_ACCESS_KEY` | Matching context-local S3-compatible secret key |
| `CONSOLE_MAIL_MASTER_KEY` | Base64-encoded 32-byte webmail credential KEK from the context's authoritative secret store |
| `CONSOLE_PRODUCTION_SERVICE_PRINCIPAL_HMAC_KEY` | Base64-encoded 32-byte server HMAC key for production source service-principal verifiers; never disclose or reuse it as a client secret |
| `CONSOLE_MAIL_MOX_WEBHOOK_SECRET` | Hex/base64url shared secret mox uses as `Authorization: Bearer ...` for the internal delivery webhook |

Optional (enable when the integrations go live — operator-blocked on KCC 신고 /
Kakao / FCM credentials): `CONSOLE_FCM_*`, `CONSOLE_SOLAPI_*`.

| Key | What |
|---|---|
| `CONSOLE_EMAIL_SMTP_USERNAME` | Context-approved SMTP relay credential — username (open-signup OTP relay) |
| `CONSOLE_EMAIL_SMTP_PASSWORD` | Context-approved SMTP relay credential — password |

Outbound OTP email relay. The non-secret host/port/sender live on the
`console-config` ConfigMap (`CONSOLE_EMAIL_SMTP_HOST`, `CONSOLE_EMAIL_SMTP_PORT`,
`CONSOLE_EMAIL_FROM`, `CONSOLE_EMAIL_FROM_NAME`); only these two credentials are secret.
On `oci-guest`, they come from **OCI Vault → `console-secrets`**. On `on-prem-ha`,
they come from **OpenBao → External Secrets Operator → `console-secrets`** and may
target a context-approved relay rather than OCI Email Delivery. Because the
production ConfigMap sets the relay fields, the `console-app` and `console-worker`
workload manifests require both credential keys with explicit `secretKeyRef`
entries; missing keys fail the rollout instead of silently degrading OTP
delivery to stub logs. Local/dev/e2e stub-email configurations must omit the
whole `CONSOLE_EMAIL_*` relay group. Setting any `CONSOLE_EMAIL_*` member requires the
full group.

`CONSOLE_MAIL_MASTER_KEY` is the webmail envelope-encryption key (KEK) used to seal
tenant mail-server credentials. Generate exactly 32 random bytes, base64-encode
them, store the value in the context's authoritative secret store, then project
it into `console-secrets`. Never commit, log, paste into tickets, or reuse this key
across environments. With
`CONSOLE_MAIL_ENABLED=true` but this key absent, the app still boots and mail APIs
return 503; once it is present, the IMAP sync worker can run when object storage
is also configured.

`CONSOLE_MAIL_MOX_WEBHOOK_SECRET` is a separate mox→app webhook bearer secret. It is
not the `CONSOLE_MAIL_MASTER_KEY`, not a mox account password, and not an admin
credential. Generate it as a log-safe single-line random value (for example
`openssl rand -hex 32`), store it in the context's authoritative secret store,
project it into `console-secrets`, and rotate it by updating `console-secrets` plus
restarting `statefulset/console-mox`, `rollout/console-app`, and
`deployment/console-worker` so both sides reload the same credential. The committed
mox bootstrap template only
contains a placeholder; the StatefulSet renders the real value on first boot and
refreshes only the existing `domains.conf` webhook `Authorization: Bearer ...`
line on later starts, without logging the secret or overwriting other
PVC-resident mox config.

Dark mox account/bootstrap credentials belong in the context's authoritative
secret store and are not committed to Kubernetes manifests:

| Secret name | What | Used by |
|---|---|---|
| `console-mox-postmaster-password` | Initial `postmaster@knllogistic.com`/mox webapi account password | Operator pipes it to `mox setaccountpassword postmaster` after `console-mox` is Ready; tenants can then store mox account credentials through the app's sealed webmail credential flow |
| `console-mox-admin-password` | Reserved break-glass mox admin password | Keep in the context's authoritative secret store only; the dark deployment disables the mox admin interface by default. If an operator intentionally enables admin later, expose it only over an internal port-forward/VPN and record the change. |
| `console-mox-dkim-private-keys` | Future DKIM selector private keys if public MX/outbound deliverability is approved | Do not mount or generate for the dark lane. Public MX/DKIM is a separate operator/founder gate. |

### `oci-guest`: create and rotate `console-secrets`

The commands in this subsection are specific to the live OCI guest. Retrieve
values from OCI Vault without echoing them into history, then create the
out-of-band Kubernetes Secret that Argo CD intentionally does not manage.

Set the initial postmaster password:

```sh
set -euo pipefail
set +x
# Retrieve from OCI Vault/Secrets Manager without printing or logging the value.
MOX_PASS_SECRET_OCID="${MOX_PASS_SECRET_OCID:?set to the console-mox-postmaster-password OCI Vault secret OCID}"
MOX_PASS="$(oci secrets secret-bundle get --secret-id "$MOX_PASS_SECRET_OCID" \
  --query 'data."secret-bundle-content".content' --raw-output | base64 -d)"
test -n "$MOX_PASS"
printf '%s' "$MOX_PASS" | kubectl exec -i -n console statefulset/console-mox -- \
  /bin/mox -config /mox-data/config/mox.conf setaccountpassword postmaster
unset MOX_PASS MOX_PASS_SECRET_OCID
```

```sh
set -euo pipefail
set +x
# Generate a fresh ES256 keypair (do NOT reuse ops/.dev-secrets — those are dev-only).
# The private key MUST be PKCS#8 PEM: jsonwebtoken's
# from_ec_pem rejects the legacy SEC1 EC-private-key PEM format that
# `openssl ecparam -genkey` emits (the app fails to boot with jwt InvalidKeyFormat).
# `openssl genpkey` produces PKCS#8 directly.
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out jwt-private.pem
openssl pkey -in jwt-private.pem -pubout -out jwt-public.pem

umask 077
SECRET_TMP="$(mktemp -d "${TMPDIR:-/tmp}/console-secrets.XXXXXX")"
trap 'rm -rf "$SECRET_TMP" jwt-private.pem jwt-public.pem' EXIT
CONSOLE_S3_ACCESS_KEY_ID_OCID="${CONSOLE_S3_ACCESS_KEY_ID_OCID:?set to the OCI access-key secret OCID}"
CONSOLE_S3_SECRET_ACCESS_KEY_OCID="${CONSOLE_S3_SECRET_ACCESS_KEY_OCID:?set to the OCI secret-key secret OCID}"
CONSOLE_MAIL_MASTER_KEY_OCID="${CONSOLE_MAIL_MASTER_KEY_OCID:?set to the mail master-key OCI Vault secret OCID}"
CONSOLE_MAIL_MOX_WEBHOOK_SECRET_OCID="${CONSOLE_MAIL_MOX_WEBHOOK_SECRET_OCID:?set to the mox webhook-secret OCI Vault secret OCID}"
for spec in \
  "CONSOLE_S3_ACCESS_KEY_ID:$CONSOLE_S3_ACCESS_KEY_ID_OCID" \
  "CONSOLE_S3_SECRET_ACCESS_KEY:$CONSOLE_S3_SECRET_ACCESS_KEY_OCID" \
  "CONSOLE_MAIL_MASTER_KEY:$CONSOLE_MAIL_MASTER_KEY_OCID" \
  "CONSOLE_MAIL_MOX_WEBHOOK_SECRET:$CONSOLE_MAIL_MOX_WEBHOOK_SECRET_OCID"; do
  key="${spec%%:*}"
  ocid="${spec#*:}"
  oci secrets secret-bundle get --secret-id "$ocid" \
    --query 'data."secret-bundle-content".content' --raw-output | base64 -d > "$SECRET_TMP/$key"
  test -s "$SECRET_TMP/$key"
  chmod 600 "$SECRET_TMP/$key"
done

kubectl create secret generic console-secrets -n console \
  --from-file=CONSOLE_JWT_PRIVATE_KEY_PEM=jwt-private.pem \
  --from-file=CONSOLE_JWT_PUBLIC_KEY_PEM=jwt-public.pem \
  --from-file=CONSOLE_S3_ACCESS_KEY_ID="$SECRET_TMP/CONSOLE_S3_ACCESS_KEY_ID" \
  --from-file=CONSOLE_S3_SECRET_ACCESS_KEY="$SECRET_TMP/CONSOLE_S3_SECRET_ACCESS_KEY" \
  --from-file=CONSOLE_MAIL_MASTER_KEY="$SECRET_TMP/CONSOLE_MAIL_MASTER_KEY" \
  --from-file=CONSOLE_MAIL_MOX_WEBHOOK_SECRET="$SECRET_TMP/CONSOLE_MAIL_MOX_WEBHOOK_SECRET" \
  --from-literal=CONSOLE_EMAIL_SMTP_USERNAME=<oci-email-smtp-username> \
  --from-literal=CONSOLE_EMAIL_SMTP_PASSWORD=<oci-email-smtp-password>
rm -rf "$SECRET_TMP" jwt-private.pem jwt-public.pem
trap - EXIT

# Example local generator for the value that must be stored in OCI Vault first:
python3 - <<'PY'
import base64, os
print(base64.b64encode(os.urandom(32)).decode())
PY
```

### `on-prem-ha`: project application and mail secrets from OpenBao

Do not run the OCI commands above for `on-prem-ha`. Before activating that
context, provision OpenBao HA Raft, enable audit logging and tested backup/
restore, assign named unseal and recovery custodians, and configure External
Secrets Operator with a context-local `SecretStore` or `ClusterSecretStore`.
The resulting `ExternalSecret` must project all required `console-secrets` keys.
`CONSOLE_S3_ACCESS_KEY_ID` and `CONSOLE_S3_SECRET_ACCESS_KEY` must be credentials for the
on-prem SeaweedFS S3 endpoint; mail, JWT, and relay values must come from
OpenBao paths scoped to this context. OCI Vault OCIDs and OCI Customer Secret
Keys are neither inputs nor fallback requirements for this procedure.

Activation remains fail-closed until OpenBao audit/backup recovery and
External-Secrets refresh/rotation are exercised without exposing values in Git,
logs, tickets, or shell history.

## `oci-guest`: `oci-objectstore-creds` CNPG backup credentials

Consumed by the Barman `ObjectStore` for DB backups. The keys are an **OCI
Customer Secret Key** (Identity → your user → Customer Secret Keys), which is an
S3-compatible access/secret pair. Can be the same pair as the evidence keys.
This secret name and credential source are the live `oci-guest` contract. The
`on-prem-ha` overlay must use an OpenBao/External-Secrets-projected secret for
the selected self-hosted S3 endpoint instead of making OCI Customer Secret Keys a
universal database-backup requirement.

```sh
set -euo pipefail
set +x
OBJSTORE_SECRET_TMP="$(mktemp -d "${TMPDIR:-/tmp}/oci-objectstore-creds.XXXXXX")"
trap 'rm -rf "$OBJSTORE_SECRET_TMP"' EXIT
OCI_OBJECTSTORE_ACCESS_KEY_ID_OCID="${OCI_OBJECTSTORE_ACCESS_KEY_ID_OCID:?set to the OCI object-store access-key secret OCID}"
OCI_OBJECTSTORE_SECRET_KEY_OCID="${OCI_OBJECTSTORE_SECRET_KEY_OCID:?set to the OCI object-store secret-key secret OCID}"
oci secrets secret-bundle get --secret-id "$OCI_OBJECTSTORE_ACCESS_KEY_ID_OCID" \
  --query 'data."secret-bundle-content".content' --raw-output | base64 -d \
  > "$OBJSTORE_SECRET_TMP/ACCESS_KEY_ID"
oci secrets secret-bundle get --secret-id "$OCI_OBJECTSTORE_SECRET_KEY_OCID" \
  --query 'data."secret-bundle-content".content' --raw-output | base64 -d \
  > "$OBJSTORE_SECRET_TMP/ACCESS_SECRET_KEY"
test -s "$OBJSTORE_SECRET_TMP/ACCESS_KEY_ID"
test -s "$OBJSTORE_SECRET_TMP/ACCESS_SECRET_KEY"
chmod 600 "$OBJSTORE_SECRET_TMP"/*
kubectl create secret generic oci-objectstore-creds -n console \
  --from-file=ACCESS_KEY_ID="$OBJSTORE_SECRET_TMP/ACCESS_KEY_ID" \
  --from-file=ACCESS_SECRET_KEY="$OBJSTORE_SECRET_TMP/ACCESS_SECRET_KEY"
rm -rf "$OBJSTORE_SECRET_TMP"
trap - EXIT
```

## `on-prem-ha`: CNPG backup credentials

The on-prem overlay must define a context-local Kubernetes Secret for the CNPG
Barman `ObjectStore`, projected from OpenBao by External Secrets Operator. Its
access key and secret key must address the independent SeaweedFS S3 backup
target selected by the overlay. It must not reuse the live `oci-guest`
`oci-objectstore-creds` value or depend on OCI Vault. The overlay activation
gate must prove backup, restore, credential rotation, and failure-domain
independence before production data is admitted.

## Database connections — owner vs. runtime split

The cluster has **two** roles, with two secrets, deliberately separated:

| Role | Secret | Used by | Privileges |
|---|---|---|---|
| `console_app` (owner) | `console-db-app` | **migrations only**, via the `console-migrate` PreSync Job | owns every table; runs DDL |
| `console_rt` (runtime) | `console-db-rt` | `console-app` / `console-worker` `DATABASE_URL` | least-privilege DML, **subject to RLS** |

The running application **never** connects as the owner. Connecting as the owner
would let a compromised app `DROP POLICY` / `DISABLE ROW LEVEL SECURITY` and turn
the entire tenant-isolation boundary off, and (without `FORCE RLS`) bypass RLS
outright. `console_rt` is `NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE`, owns
nothing, and only receives the GRANTs from migration `0031`.

### `console-db-app` — owner / migration connection (auto-generated)

Created by CloudNativePG for the `console-db` cluster's `console_app` owner user. **Do
not create this manually.** It is consumed **only** by the `console-migrate` Job,
which runs schema migrations automatically as an Argo CD **PreSync hook** (the
`console-app` image in `CONSOLE_APP_ROLE=migrate` mode reads its `uri` key) — never by a
serving workload. The PreSync Job completes before the `console-app`/`console-worker`
Deployments roll, so the runtime `console_rt` role never needs DDL. See the
"Database migrations" section in [`README.md`](README.md). Migrations are
idempotent (sqlx `_sqlx_migrations` ledger), so the Job is safe to re-run on
every sync.

> Cutover ordering: create the **`console-db-rt`** runtime secret (below) **before**
> the first sync. The de-owned `console_rt` role (NOSUPERUSER, NOBYPASSRLS, owns
> nothing — migration 0031) is what makes the owner/runtime split meaningful;
> without that secret CNPG cannot bind the role and the app cannot start.

### `console-db-rt` — runtime connection (you create this)

The password for the managed `console_rt` role. CNPG reads it from this secret
(`database.yaml` `managed.roles[].passwordSecret: console-db-rt`) and attaches it to
the role with LOGIN. The app/worker read `DATABASE_URL` from its `uri` key.
Create it **before** the `maintenance` app first syncs so CNPG can bind the role:

```sh
set -euo pipefail
set +x
# Pick a strong password; it must match what CNPG attaches to console_rt.
RT_PASSWORD="$(openssl rand -hex 32)"
# host = the CNPG read/write service for the cluster; db = maintenance.
RT_URI="postgresql://console_rt:${RT_PASSWORD}@console-db-rw.console.svc:5432/console"
RT_SECRET_TMP="$(mktemp -d "${TMPDIR:-/tmp}/console-db-rt.XXXXXX")"
trap 'rm -rf "$RT_SECRET_TMP"' EXIT
printf '%s' "$RT_PASSWORD" > "$RT_SECRET_TMP/password"
printf '%s' "$RT_URI" > "$RT_SECRET_TMP/uri"
chmod 600 "$RT_SECRET_TMP"/*

kubectl create secret generic console-db-rt -n console \
  --type=kubernetes.io/basic-auth \
  --from-literal=username=console_rt \
  --from-file=password="$RT_SECRET_TMP/password" \
  --from-file=uri="$RT_SECRET_TMP/uri"
kubectl label secret console-db-rt -n console cnpg.io/reload=true
rm -rf "$RT_SECRET_TMP"
trap - EXIT
unset RT_PASSWORD RT_URI
```

Hex keeps the password URI-safe. If an operator uses another character set,
percent-encode the password component before constructing `uri`; never place a
raw `+`, `/`, `@`, `:`, or `%` in that component. The
`kubernetes.io/basic-auth` type makes the username/password contract explicit,
and `cnpg.io/reload=true` tells CloudNativePG to reconcile the managed login
after a password change.

Updating a Kubernetes Secret does **not** update environment variables in
already-running containers. After CNPG has reconciled a rotated `console_rt`
password, deliberately restart both `console-app` and `console-worker`, then prove their
readiness and prove the retired password is rejected. Do not claim zero-downtime
credential rotation without observed workload and request-level evidence.

The six-role, two-command-credential topology proposed by PR 473 is intentionally
DARK and is not part of this live two-role procedure. Its complete secret
contract and activation evidence are documented in
[`apps/secrets-management/components/governed-command-database/README.md`](apps/secrets-management/components/governed-command-database/README.md).

## Platform-admin cold start + tenant onboarding

The SaaS-vendor **PLATFORM** tier sits ABOVE every tenant. It is bootstrapped
once, out-of-band, then drives tenant onboarding.

- `CONSOLE_COLDSTART_OTP` (optional, on `console-secrets`) — a one-time secret supplied
  at boot. It seeds a single bootstrap credential for the **PLATFORM admin** (the
  `Cold Start Admin` SUPER_ADMIN, re-homed to the platform sentinel org
  `00000000-0000-0000-0000-00000000face` by migration 0036), NOT a tenant admin.
  Redeeming it signs the platform admin in for first passkey enrollment; once a
  passkey exists the OTP is dead. Leave it UNSET once the platform admin has a
  passkey (the normal steady state). `CONSOLE_COLDSTART_OTP_TTL_SECS` (default 3600)
  bounds the redeem window; the value is never logged or written to audit.
  - The platform admin's login mints a **platform token** (`platform = true` in
    the JWT), the only token accepted on the platform data API (`/api/platform/*`).
    A tenant token is rejected there (403), and a platform token is rejected on
    tenant `/api/*`.

- **Tenant #1 (KNL) and every later tenant** get their own admin via the
  platform onboarding flow, NOT via `CONSOLE_COLDSTART_OTP`:
  - `POST /api/platform/orgs {slug,name}` (platform token) creates the
    `organizations` row, seeds that tenant's first SUPER_ADMIN, and returns a
    fresh **per-org** one-time OTP to deliver to the tenant out-of-band. This is
    the ONLY path that inserts org rows (the app's `console_rt` role is SELECT-only on
    `organizations` under RLS; creation runs via the audited SECURITY DEFINER
    `platform_create_organization`). The fixed `coss0000` seed removed in
    migration 0023 is never reintroduced — every onboarding OTP is generated
    fresh per org.
  - `GET /api/platform/orgs` lists tenants (audited cross-tenant read); the platform
    sentinel org is never listed.
  - `PATCH /api/platform/orgs/{id} {status}` suspends/reactivates a tenant (audited
    to the target org).

  > KNL was historically backfilled by migration 0028 and remains tenant #1; its
  > admin should be (re)issued through `POST /api/platform/orgs` semantics rather than
  > the global cold-start OTP, which now belongs to the platform tier.

## Native app-link association + session TTLs (non-secret config)

These are non-secret runtime config and live on the `console-config` ConfigMap (NOT
`console-secrets`). They are listed here so the operator knows where they come from
during native-app rollout.

- **Native passkeys** are inert until the platform serves the Apple App Site
  Association + Android Digital Asset Links documents at the fixed `/.well-known/*`
  paths over the RP origin. These are served public + unauthenticated; the values
  come from the ConfigMap (comma-separated lists, empty until provisioned — the
  endpoints then serve a valid empty document):
  - `CONSOLE_IOS_APP_IDS` — iOS app identifiers `<TeamID>.<bundle-id>` (the Team ID
    from the Apple Developer account + the app's bundle id), e.g.
    `ABCDE12345.com.knl.fsm`. Multiple builds (prod/dev) are comma-separated.
  - `CONSOLE_ANDROID_PACKAGE` — the Android `applicationId`, e.g. `com.knl.fsm`.
  - `CONSOLE_ANDROID_CERT_SHA256` — the SHA-256 fingerprint(s) of the app's signing
    cert(s), colon-separated hex (from `keytool -list -v` / Play App Signing).
    Comma-separate multiple signing keys (e.g. upload + Play-managed).
- **Refresh-family absolute TTL** (`CONSOLE_REFRESH_FAMILY_ABSOLUTE_TTL_SECS`, default
  `86400` = 24h) — the NIST 800-63B AAL2 absolute session-lifetime cap. A refresh
  family rotates freely within this window of its creation; past it the next
  rotation is rejected and the family revoked, forcing a fresh primary sign-in.

## CI / release secrets (GitHub repo settings)

- `RELEASE_PLEASE_TOKEN` — required scheduling-capable repository token with
  Contents read/write, used only by protected convergence for the final
  force-with-lease update of the bot-created release branch. It is not passed to
  Release Please or used for tag creation. Without it, required PR workflows do
  not schedule and the release PR remains blocked (see `release-please.yml` and
  `docs/CI-GATES.md`).
- Mobile signing + store credentials — see `docs/release/SECRETS.md`.
- GHCR push uses the built-in `GITHUB_TOKEN`; image signing is keyless (OIDC) —
  no secret needed.
