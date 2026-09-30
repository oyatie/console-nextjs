# Cluster registration examples for ApplicationSet federation

These examples document labels and ExternalSecret shape only. Do not commit real
Argo CD cluster Secret data, kubeconfigs, bearer tokens, client certificates,
certificate authorities, or base64-encoded credential payloads.

## Approved sources

Cluster credentials must come from one of these approved paths:

1. External Secrets Operator reading from OpenBao for the ADR-0024 portable
   secrets lane.
2. The repo's currently approved out-of-band secret-management path documented in
   `deploy/SECRETS.md` and `deploy/OPS-RUNBOOK.md`, until External-Secrets/OpenBao
   is live for this substrate.

Committing a Kubernetes `Secret` with `data`, `stringData`, kubeconfig content,
`bearerToken`, `tlsClientConfig`, `clientKeyData`, or similar credential material
is forbidden.

## Primary cluster labels

Metadata-only example for the current primary in a Korean on-prem site:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: console-onprem-kr-a
  namespace: argocd
  labels:
    argocd.argoproj.io/secret-type: cluster
    console.io/federation: enabled
    console.io/environment: prod
    console.io/site: onprem-kr-a
    console.io/dr-role: primary
    console.io/residency: kr
    console.io/traffic: active
    console.io/standby-mode: none
    console.io/storage-profile: replicated
    console.io/registration-source: external-secrets-openbao
# credential data intentionally omitted
```

## Warm-standby cluster labels

Metadata-only example for a warm standby in the same residency boundary:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: console-onprem-kr-b
  namespace: argocd
  labels:
    argocd.argoproj.io/secret-type: cluster
    console.io/federation: enabled
    console.io/environment: prod
    console.io/site: onprem-kr-a
    console.io/dr-role: warm-standby
    console.io/residency: kr
    console.io/traffic: held
    console.io/standby-mode: warm
    console.io/storage-profile: restored-replica
    console.io/registration-source: external-secrets-openbao
# credential data intentionally omitted
```

## ExternalSecret skeleton

This is an illustrative shape for operators after External Secrets + OpenBao are
available. The remote keys are placeholders. The secret payload must be populated
in OpenBao, not in git.

```yaml
apiVersion: external-secrets.io/v1
kind: ExternalSecret
metadata:
  name: console-onprem-kr-a-cluster
  namespace: argocd
spec:
  refreshInterval: 1h
  secretStoreRef:
    kind: ClusterSecretStore
    name: openbao-console
  target:
    name: console-onprem-kr-a
    creationPolicy: Owner
    template:
      metadata:
        labels:
          argocd.argoproj.io/secret-type: cluster
          console.io/federation: enabled
          console.io/environment: prod
          console.io/site: onprem-kr-a
          console.io/dr-role: primary
          console.io/residency: kr
          console.io/traffic: active
          console.io/standby-mode: none
          console.io/storage-profile: replicated
          console.io/registration-source: external-secrets-openbao
  data:
    - secretKey: name
      remoteRef:
        key: console/argocd/clusters/onprem-kr-a
        property: name
    - secretKey: server
      remoteRef:
        key: console/argocd/clusters/onprem-kr-a
        property: server
    - secretKey: config
      remoteRef:
        key: console/argocd/clusters/onprem-kr-a
        property: config
```

Reviewers should reject any diff under this directory that contains real values
for `server` or `config` when those values reveal private cluster endpoints or
credentials. Keep examples abstract and source the live Secret from the approved
secret manager.
