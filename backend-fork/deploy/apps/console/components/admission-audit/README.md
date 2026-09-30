# admission-audit component (opt-in)

Kustomize component for **audit-mode image-admission verification** with
[sigstore policy-controller](https://docs.sigstore.dev/policy-controller/overview/).
It verifies that the production `console-app` image is keylessly
signed by the repository's `image-release.yml` GitHub Actions workflow, using the
public Fulcio and Rekor services.

## Why audit mode first?

`spec.mode: warn` allows a pod through while emitting an admission warning when
an image fails policy verification. That matches the current Always-Free single
node constraint: we can observe policy quality without bricking the only rollout
path. After a clean warning burn-in, switch to hard-fail enforcement by removing
`mode: warn` or using the controller's enforcing mode per the policy-controller
version deployed by ops.

## Requirements

1. Install sigstore policy-controller CRDs/controller in the cluster.
2. Enable image policy checks for the target namespace according to the installed
   controller's namespace-selector contract.
3. Add this component from an overlay that targets that cluster:

```yaml
components:
  - ../../components/admission-audit
```

The base/prod overlay intentionally does **not** reference this component until
those CRDs exist; otherwise Argo would reject the unknown `ClusterImagePolicy`.

## Policy contract

- Images covered: `ghcr.io/jason931225/console-app`; `console-worker` shares the
  `console-app` image.
- Issuer: `https://token.actions.githubusercontent.com`.
- Subject: this repository's `.github/workflows/image-release.yml` on `main` or
  a SemVer tag.
- Transparency log: public Rekor.
- Fulcio: public Fulcio.
