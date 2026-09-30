# PR 473 governed command database — self-host/on-prem rehearsal (DARK)

This unreferenced overlay composes the portable governed command-database
component with the existing DARK on-prem HA adapter. Self-host is the first
activation target. The overlay retains the on-prem CNPG, storage, network, and
object-store choices while presenting the same three database URL ports and
Kubernetes Secret contract as OCI.

## Render contract

```sh
kubectl kustomize deploy/apps/console/overlays/pr-473-expand-on-prem
```

The render must contain the six roles, `console-db-topology`, waves 0–3, both API
command URI projections, and the expanded NetworkPolicies. It must not be piped
to `kubectl apply`. Rendering proves composition only; it does not prove the
multi-node substrate, secret store, database, migration, workload, or rollback.

## Self-host secret adapter

Use OpenBao HA Raft as the authoritative secret root and External Secrets
Operator as the projection adapter. The two command entries are:

| OpenBao KV v2 path under `secret/` | Kubernetes Secret | Required keys |
|---|---|---|
| `console/db/leave-command` | `console-db-leave-command` | `username`, `password`, `uri` |
| `console/db/ontology-command` | `console-db-ontology-command` | `username`, `password`, `uri` |

Activate the unreferenced
[`governed-command-database` secrets component](../../../secrets-management/components/governed-command-database/README.md)
in the same reviewed non-production promotion as this database component. Do not
make OCI Vault, OCI Customer Secret Keys, or any OCI endpoint a self-host runtime
dependency. OCI may remain an off-cluster rollback archive only when the approved
custody policy permits it.

OpenBao must already have named unseal and recovery custodians, audit logging,
tested backup/restore, and a healthy ESO Kubernetes-auth path. Each generated
target Secret must be `kubernetes.io/basic-auth`, carry `cnpg.io/reload=true`,
and expose exactly `username`, `password`, and `uri`. The four login passwords
must be pairwise distinct, and URI password components must be hexadecimal or
correctly percent-encoded.

## Self-host rehearsal gates

Follow the shared
[`promotion, evidence, rotation, recovery, and rollback contract`](../../components/governed-command-database/README.md).
In addition, record:

- three-node OpenBao Raft health, audit continuity, backup, restore, and custodian
  recovery evidence;
- ESO store readiness, target Secret type/labels/key sets, and refresh versions;
- multi-node CNPG placement, failover, backup, restore, and connection-budget
  evidence for the selected hardware;
- whole-Application wave ordering without selective sync;
- API/worker readiness, audited command outcomes, denial tests, and rejection of
  retired credentials; and
- a rollback drill that restores both database and secret projection control.

ESO updating a Secret is not workload rotation. Restart API and worker for
`console_rt`, restart API for either command credential, wait for readiness, and
prove the retired credential is rejected. Do not claim zero downtime without
observed request-level evidence across the entire rotation.
