# Maintenance on-prem CNPG HA overlay

This is a DARK staging overlay for ADR-0024 / GitHub issue #379. It is not wired into `deploy/argocd/apps/`, so the current OCI guest deployment continues to use `deploy/apps/console/overlays/prod` and the base CNPG single-instance posture.

The overlay patches only the CNPG `Cluster/console-db` shape for on-prem HA:

- `spec.instances: 3`
- `spec.storage.storageClass: console-pg-hot`
- synchronous replication quorum of one standby, with CNPG failover quorum enabled
- switchover-based primary update behavior and immediate failover detection
- pod anti-affinity plus topology spread by `kubernetes.io/hostname`
- evidence object storage (`CONSOLE_S3_ENDPOINT_URL`) and CNPG Barman
  (`ObjectStore/console-backups` `endpointURL`) to the DARK self-hosted S3 Service
  contract `http://console-object-store-s3.console-object-store.svc.cluster.local:8333`
- CNPG Barman object-store credentials from an on-prem `console-cnpg-objectstore-creds`
  secret projected by OpenBao/External Secrets for production activation (manual
  creation is acceptable only for a rehearsal)

`console-pg-hot` is the stable maintenance StorageClass contract selected by the upstream validation task. The current DARK Longhorn storage app binds that canonical name to replicated block storage; live activation still requires a real multi-node on-prem/Talos substrate and a captured failover drill.

Object storage remains path-style/SigV4. This overlay changes only deployment
configuration: the backend S3 client code is intentionally untouched,
`CONSOLE_S3_FORCE_PATH_STYLE` stays inherited as `true`, and the on-prem SigV4 region
is `us-east-1` for the self-hosted S3-compatible service. Barman retention stays
explicitly unpruned/indefinite until the activation ticket records a retention,
WORM/evidence, and second-site replication policy. The base CNPG cluster keeps the
OCI-only `AWS_REQUEST_CHECKSUM_CALCULATION=when_required` /
`AWS_RESPONSE_CHECKSUM_VALIDATION=when_required` workaround for OCI Object Storage,
but this on-prem overlay removes `spec.env` so self-hosted S3 uses default
boto3/Barman checksum behavior.

Verification commands:

```sh
kubectl kustomize deploy/apps/console/overlays/on-prem
kubectl kustomize deploy/apps/console/overlays/oci-guest
kubectl kustomize deploy/apps/console/base
kubectl kustomize deploy/apps/console/overlays/prod
```

Use `overlays/on-prem-observability` only when the DARK observability stack is
active. The plain `on-prem` overlay deliberately leaves
`OTEL_EXPORTER_OTLP_ENDPOINT` unset so the app can deploy without an in-cluster
collector.

Before any live Argo CD wiring, verify in a >=3-node on-prem cluster that Longhorn is healthy, PVCs using `console-pg-hot` bind, the CNPG cluster reaches three instances, and killing/draining the primary pod/node promotes a replica without data loss.
