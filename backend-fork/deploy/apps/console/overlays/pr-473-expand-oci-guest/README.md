# PR 473 governed command database — OCI-guest rehearsal (DARK)

This unreferenced overlay composes the current OCI production adapter with the
portable governed command-database component. It does not modify the live
`prod` overlay or its Argo CD `Application`. OCI remains a first-class substrate,
including OCI Vault recovery and OCI Object Storage; those capabilities remain
outside the application's database-credential port.

## Render contract

```sh
kubectl kustomize deploy/apps/console/overlays/pr-473-expand-oci-guest
```

The render must contain the six roles, `console-db-topology`, waves 0–3, both API
command URI projections, and the expanded NetworkPolicies. It must not be piped
to `kubectl apply`. Rendering is not activation authority or runtime evidence.

## OCI secret adapter

For an approved OCI non-production rehearsal, OCI Vault is the authoritative
recovery store. Operators manually project these two additional Secrets into
namespace `maintenance`:

| Secret | Username | Required keys |
|---|---|---|
| `console-db-leave-command` | `console_leave_cmd` | `username`, `password`, `uri` |
| `console-db-ontology-command` | `console_ontology_cmd` | `username`, `password`, `uri` |

Generate each password as an independent 32-byte hexadecimal value and construct
its URI with host `console-db-rw.console.svc`, port `5432`, and database
`maintenance`. Hex is URI-safe; another alphabet requires correct percent
encoding of the password component. Create both Secrets as
`kubernetes.io/basic-auth` and label each `cnpg.io/reload=true`.

The two new passwords, the existing `console_rt` password, and CloudNativePG's
`console_app` password must be non-empty and pairwise distinct. Do not inspect or
record their values to prove this; the wave-1 topology gate performs the decoded
comparison inside the cluster and emits only a pass/fail result.

For an approved activation window, create the two Secrets without placing
passwords in command arguments or leaving files behind:

```sh
set -euo pipefail
set +x
COMMAND_SECRET_TMP="$(mktemp -d "${TMPDIR:-/tmp}/console-command-db.XXXXXX")"
trap 'rm -rf "$COMMAND_SECRET_TMP"' EXIT

LEAVE_COMMAND_PASSWORD="$(openssl rand -hex 32)"
ONTOLOGY_COMMAND_PASSWORD="$(openssl rand -hex 32)"
test "$LEAVE_COMMAND_PASSWORD" != "$ONTOLOGY_COMMAND_PASSWORD"

printf '%s' "$LEAVE_COMMAND_PASSWORD" > "$COMMAND_SECRET_TMP/leave-password"
printf 'postgresql://console_leave_cmd:%s@console-db-rw.console.svc:5432/console' \
  "$LEAVE_COMMAND_PASSWORD" > "$COMMAND_SECRET_TMP/leave-uri"
printf '%s' "$ONTOLOGY_COMMAND_PASSWORD" > "$COMMAND_SECRET_TMP/ontology-password"
printf 'postgresql://console_ontology_cmd:%s@console-db-rw.console.svc:5432/console' \
  "$ONTOLOGY_COMMAND_PASSWORD" > "$COMMAND_SECRET_TMP/ontology-uri"
chmod 600 "$COMMAND_SECRET_TMP"/*

kubectl create secret generic console-db-leave-command -n console \
  --type=kubernetes.io/basic-auth \
  --from-literal=username=console_leave_cmd \
  --from-file=password="$COMMAND_SECRET_TMP/leave-password" \
  --from-file=uri="$COMMAND_SECRET_TMP/leave-uri"
kubectl label secret console-db-leave-command -n console cnpg.io/reload=true

kubectl create secret generic console-db-ontology-command -n console \
  --type=kubernetes.io/basic-auth \
  --from-literal=username=console_ontology_cmd \
  --from-file=password="$COMMAND_SECRET_TMP/ontology-password" \
  --from-file=uri="$COMMAND_SECRET_TMP/ontology-uri"
kubectl label secret console-db-ontology-command -n console cnpg.io/reload=true

rm -rf "$COMMAND_SECRET_TMP"
trap - EXIT
unset LEAVE_COMMAND_PASSWORD ONTOLOGY_COMMAND_PASSWORD
```

This first-create procedure must fail if either Secret already exists. Rotation
updates the authoritative OCI Vault value and the Kubernetes Secret in one
controlled window; it must not overwrite an existing Secret through an
unreviewed create command.

Preserve recovery-store records for the two command credentials in the same
custody model as `console-app-secrets-bundle`. A recovery is incomplete until an
operator can recreate both typed Secrets with matching key names and URI
encoding without consulting the cluster.

## OCI rehearsal gates

Follow the shared
[`promotion, evidence, rotation, recovery, and rollback contract`](../../components/governed-command-database/README.md).
In addition, record:

- the OCI Vault object and version identifiers for both command credentials;
- the exact OCI guest render and immutable application image digests;
- CNPG backup/restore evidence from the OCI Object Storage target;
- whole-Application wave ordering without selective sync;
- API readiness and intrinsically audited leave and ontology command outcomes;
- rejection of direct table DML and of every retired password; and
- the live two-role/prod and Argo CD paths remaining unchanged by the rehearsal.

The OCI guest is single-node. Successful blue/green application behavior does
not prove node or database high availability, and a Secret refresh does not
refresh an existing container environment. Restart affected consumers
deliberately and make no zero-downtime rotation claim without request-level
evidence.
