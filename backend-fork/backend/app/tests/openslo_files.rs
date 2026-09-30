#![allow(clippy::panic)]

use std::fs;
use std::path::Path;

const SLO_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/slos");

#[test]
fn openslo_v1_files_have_required_slo_shape() {
    let availability = read_slo("api-availability.openslo.yaml");
    assert_required_shape(&availability, "console-api-availability");
    assert_contains(&availability, "target: 0.995");
    assert_contains(&availability, "duration: 30d");
    assert_contains(&availability, "budgetingMethod: Occurrences");
    assert_contains(&availability, "ratioMetric:");
    assert_contains(&availability, "good:");
    assert_contains(&availability, "total:");

    let latency = read_slo("api-latency.openslo.yaml");
    assert_required_shape(&latency, "console-api-latency");
    assert_contains(&latency, "target: 0.99");
    assert_contains(&latency, "duration: 30d");
    assert_contains(&latency, "op: lte");
    assert_contains(&latency, "value: 0.5");
    assert_contains(&latency, "thresholdMetric:");
    assert_contains(&latency, "histogram_quantile(0.99");
}

fn read_slo(name: &str) -> String {
    let path = Path::new(SLO_DIR).join(name);
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("failed to read {path:?}: {err}"))
}

fn assert_required_shape(contents: &str, name: &str) {
    assert_contains(contents, "apiVersion: openslo/v1");
    assert_contains(contents, "kind: SLO");
    assert_contains(contents, "metadata:");
    assert_contains(contents, &format!("name: {name}"));
    assert_contains(contents, "spec:");
    assert_contains(contents, "service: console-app-api");
    assert_contains(contents, "indicator:");
    assert_contains(contents, "objectives:");
    assert_contains(contents, "metricSource:");
    assert_contains(contents, "type: Prometheus");
    assert_contains(contents, "spec:");
    assert_contains(contents, "query:");
}

fn assert_contains(contents: &str, needle: &str) {
    assert!(
        contents.contains(needle),
        "OpenSLO file missing required fragment: {needle}"
    );
}
