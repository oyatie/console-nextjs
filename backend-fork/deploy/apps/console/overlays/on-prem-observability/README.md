# Maintenance on-prem DARK observability overlay

This overlay extends `../on-prem` only for clusters where the DARK self-hosted observability stack under `deploy/apps/observability/manifests` is intentionally synced.

It does three things:

- sets `OTEL_EXPORTER_OTLP_ENDPOINT` to the collector gRPC service endpoint `http://otel-collector.console-observability.svc.cluster.local:4317`;
- enables the existing opt-in Prometheus monitoring component (`ServiceMonitor` + `PrometheusRule`) without editing its `/metrics` contract;
- adds the scoped NetworkPolicies that let `console-app`/`console-worker` export OTLP to the collector and let the collector mirror the `console-app` `/metrics` scrape.

Use `deploy/apps/console/overlays/on-prem` instead when the DARK collector or Prometheus Operator CRDs are not installed. `base` and `prod` intentionally do not set `OTEL_EXPORTER_OTLP_ENDPOINT`.

Verification commands:

```sh
kubectl kustomize deploy/apps/console/overlays/on-prem-observability
kubectl kustomize deploy/apps/console/overlays/on-prem
kubectl kustomize deploy/apps/console/overlays/prod
```
