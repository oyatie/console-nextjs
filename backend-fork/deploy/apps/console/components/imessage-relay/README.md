# iMessage relay component

Opt-in Kustomize component for the non-OCI Talos cluster. It is intentionally
not referenced by `base/` or `overlays/prod/`, so the current OCI app/web/API
deployment and OCI Email Delivery SMTP path stay unchanged.

Talos hosts only the `imessage-relay` proxy workload. iMessage must run on a
macOS Messages bridge outside the cluster, reached only through mTLS, VPN, or
another private endpoint. Public bridge endpoints are not allowed.

Required secrets are names and keys only:

| Secret | Key | Purpose |
|---|---|---|
| `imessage-relay-secrets` | `relay-token` | Authenticates callers to the relay |
| `imessage-relay-secrets` | `allowed-recipients` | Static comma-separated phone or Apple ID allowlist |
| `imessage-relay-secrets` | `messages-bridge-url` | Private macOS Messages bridge URL |
| `imessage-relay-secrets` | `messages-bridge-token` | Authenticates relay to bridge |
| `imessage-relay-secrets` | `bridge-ca.crt` | Bridge mTLS CA bundle |
| `imessage-relay-secrets` | `bridge-client.crt` | Bridge mTLS client certificate |
| `imessage-relay-secrets` | `bridge-client.key` | Bridge mTLS client key |

The relay and bridge token keys are mounted as read-only files under
`/var/run/imessage-relay/secrets/`; the component passes only
`IMESSAGE_RELAY_TOKEN_FILE` and `MESSAGES_BRIDGE_TOKEN_FILE` path variables to
the container. Do not patch token values into environment variables.

The component is stateless by default and intentionally does not mount
`console-db-rt`. If a future platform-recipient database source is needed, add a
dedicated least-privilege relay database role, secret, and NetworkPolicy in that
future change instead of reusing the app runtime database secret.

Before enabling:

- Patch `allow-imessage-relay-ingress-private-callers` from the non-routable
  TEST-NET placeholder `192.0.2.0/24` to the exact private caller CIDR.
- Patch `allow-imessage-relay-egress-private-bridge` from the non-routable
  TEST-NET placeholder `192.0.2.0/24` to the exact private bridge CIDR used by
  the non-OCI Talos network.
- Replace the `registry.invalid/...@sha256:000...` image placeholder with a
  signed immutable digest produced by the non-OCI relay image pipeline.
- Verify the non-OCI Talos CNI enforces Kubernetes NetworkPolicy before relying
  on these ingress or egress boundaries.
