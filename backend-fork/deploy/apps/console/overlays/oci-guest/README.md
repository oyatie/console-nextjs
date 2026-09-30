# Maintenance OCI guest overlay

This overlay names the current Oracle Cloud guest deployment context explicitly for ADR-0024 / GitHub issue #370. It is an alias of `../prod`, so it preserves the live production image pins, WebAuthn host settings, and OCI Object Storage endpoints.

Object-storage expectations:

- evidence endpoint: `CONSOLE_S3_ENDPOINT_URL=https://axdotp9iv3ua.compat.objectstorage.ap-chuncheon-1.oraclecloud.com`
- CNPG Barman endpoint: `spec.configuration.endpointURL=https://axdotp9iv3ua.compat.objectstorage.ap-chuncheon-1.oraclecloud.com`
- SigV4 region remains `ap-chuncheon-1`
- path-style remains enabled through `CONSOLE_S3_FORCE_PATH_STYLE=true`

Use `../on-prem` only for the DARK self-hosted S3 context. Do not activate the on-prem object-store endpoint on the OCI guest unless an explicit rollback/bridge ticket says to do so.

Verification commands:

```sh
kubectl kustomize deploy/apps/console/overlays/oci-guest
kubectl kustomize deploy/apps/console/overlays/on-prem
kubectl kustomize deploy/apps/console/overlays/prod
```
