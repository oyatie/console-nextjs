# monitoring component (opt-in)

Kustomize [component](https://kubectl.docs.kubernetes.io/guides/config_management/components/)
that adds Prometheus Operator resources for the `console-app` API and dark mox mail
stack:

- `servicemonitor.yaml` — scrapes the `console-app` Service on the named `http`
  port (8080) and the `console-mox` Service on the named `metrics` port (8010) at
  path `/metrics`, every 30s.
- `prometheusrule.yaml` — SLO-burn alerts derived from
  `backend/app/slos/*.openslo.yaml`:
  - **ConsoleApiAvailabilityBurn** — 5xx ratio burning the 99.5% availability budget.
  - **ConsoleApiLatencyP99High** — p99 HTTP latency above 500ms.
  - **ConsoleMoxDown** — the internal mox metrics target is not scrapeable.
  - **ConsoleMoxWebhookFailures** — mox webhook delivery attempts are failing.
  - **ConsoleMoxQueueBacklog** — held mail is accumulating.
  - **ConsoleMoxPvcSaturation** — `/mox-data` PVC usage is above 85%.

## Requirements

These manifests use the `monitoring.coreos.com/v1` `ServiceMonitor` and
`PrometheusRule` CRDs. A **Prometheus Operator** (e.g. the
`kube-prometheus-stack` Helm chart) MUST be installed in the cluster first —
otherwise the API server rejects these resources.

## Enabling

This component is **opt-in** and is intentionally **not** referenced from
`base/` or `overlays/prod/`, so the default `kustomize build` does not emit it
(the CRDs are absent in the base cluster, which would fail validation).

Enable it from an overlay that targets a cluster where the operator is
installed by adding:

```yaml
components:
  - ../../components/monitoring
```
