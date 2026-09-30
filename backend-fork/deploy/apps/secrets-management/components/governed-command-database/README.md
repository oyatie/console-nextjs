# Governed command database secrets component (DARK)

This unreferenced Kustomize component stages External Secrets Operator (ESO)
projections for the three PR 473 command identities. Nothing in the default
secrets-management wiring, the live maintenance base/prod render, or an Argo CD
Application references it. Activation requires a separate reviewed
non-production promotion that includes the matching
[`governed-command-database` component](../../../console/components/governed-command-database/README.md).

## Provider-neutral Secret port

The application and CloudNativePG consume Kubernetes Secrets, not a cloud SDK or
secret-store API. All target Secrets use this contract:

- type: `kubernetes.io/basic-auth`;
- label: `cnpg.io/reload=true`; and
- keys: `username`, `password`, and `uri`.

The checked-in adapter targets the `openbao-console` `ClusterSecretStore`:

| OpenBao KV v2 path under `secret/` | Target Secret | Required username |
|---|---|---|
| `console/db/leave-command` | `console-db-leave-command` | `console_leave_cmd` |
| `console/db/ontology-command` | `console-db-ontology-command` | `console_ontology_cmd` |
| `console/db/platform-force-command` | `console-db-platform-force-command` | `console_platform_force_cmd` |

The `uri` value uses host `console-db-rw.console.svc`, port `5432`, and database
`maintenance`. Generate independent 32-byte hexadecimal passwords so the URI
password component is safe without transformation. If policy selects another
alphabet, percent-encode that component correctly before storing `uri`.

The two command passwords must differ from each other, `console_rt`, and `console_app`.
Do not log or export values to compare them. The database topology gate performs
the decoded in-cluster comparison and reveals only success or failure.

## Store and projection gates

Before a reviewed activation change references this component, prove:

1. OpenBao is unsealed and healthy; its Kubernetes auth, audit log, Raft backup,
   restore, and named-custodian recovery procedures pass.
2. All KV entries contain exactly the required username, password, and URI
   values, with no empty field and no unexpected username.
3. `openbao-console` is Ready and ESO projects all ExternalSecrets as
   `SecretSynced`.
4. Each Kubernetes Secret has the required type, reload label, and three keys.
5. CloudNativePG reconciles all managed login passwords before the API starts.
6. The wave-1 topology gate proves six exact roles, two exact memberships,
   database ownership, five distinct passwords, and five direct login identities.

Inspect metadata and key names only. Never print decoded Secret values into logs,
tickets, evidence bundles, or shell history.

## Activation boundary

The default `deploy/apps/secrets-management/wiring/kustomization.yaml` excludes
this directory. A promotion must compose the secrets component and database
component together, then sync the complete console Application through waves
0–3. Do not activate only the ExternalSecrets, only the database roles, or only
the API environment variables. Do not selectively sync migration or serving
resources.

Self-host/on-prem is the first activation target and uses OpenBao HA Raft plus
ESO. OCI remains first-class through its manual OCI Vault projection adapter,
documented in the
[`OCI-guest rehearsal runbook`](../../../console/overlays/pr-473-expand-oci-guest/README.md).
The Kubernetes Secret port stays identical in both contexts.

## Rotation

ESO refresh changes the Kubernetes Secret; it does not change environment
variables in running containers. Rotate one command identity at a time:

1. Write the new password and matching URI to the authoritative KV entry.
2. Wait for `SecretSynced`, a new target Secret resource version, and
   CloudNativePG password reconciliation.
3. Restart the API deliberately; neither command credential belongs in the
   worker or migration workload.
4. Wait for API readiness and exercise the affected intrinsically audited command.
5. Prove a new direct database session authenticates as the expected role.
6. Prove the retired password is rejected and retain only redacted evidence.

Do not claim zero-downtime rotation based on ESO refresh or pod readiness alone.
Such a claim requires request-level evidence across the full rotation window.

## Rollback and recovery

Before activation, retain an off-cluster recovery bundle that identifies both
OpenBao object versions, the exact Git revision and rendered overlay, the Secret
contract, database backup/restore point, expected roles/memberships, workload
image digests, and named custodians. It must not contain passwords, password-bearing
URIs, root tokens, or unseal shares.

Because the ExternalSecrets use `creationPolicy: Owner`, removing them may remove
their target Secrets. Before rolling back the secret adapter, create and verify
equivalent out-of-band typed Secrets from the approved recovery source. Then
revert the activation reference, sync the whole Application, restart affected
consumers, verify readiness and command behavior, and prove retired credentials
are rejected.

This component is a DARK implementation artifact. It is not evidence that
OpenBao, ESO, CloudNativePG reconciliation, credential rotation, rejection, or
rollback has run in any environment.
