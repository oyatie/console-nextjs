//! Gate: `console-app`'s `dev-auth` cargo feature (the local role-switch endpoint,
//! `console-platform-auth-rest`'s `POST /api/v1/dev-auth/session`) must never be
//! reachable from `console-app`'s DEFAULT feature set — that default set is exactly
//! what every release image builds with (no `--features` flag), so "not in
//! default" IS "not in the shipped binary".
//!
//! This is the mechanical half of the dev-auth absence proof (pre-mortem #1 in
//! the plan). `backend/crates/platform/auth-rest/tests/dev_auth_absence.rs` is
//! the complementary HTTP-level proof (the route 404s in a default build); this
//! gate proves the same fact one layer down, at the feature graph itself, so a
//! future refactor that quietly adds `dev-auth` to `default` (rather than
//! wiring the route unconditionally, which the HTTP test alone would still
//! catch) fails CI too.
//!
//! Symbol-grep (`nm`/`strings`) on the built binary is corroborating evidence
//! only (unreliable for release Rust — inlining/LTO can hide or duplicate
//! symbols); this gate + the HTTP absence test are the primary proofs.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

const TARGET_PACKAGE: &str = "console-app";
const TARGET_FEATURE: &str = "dev-auth";

#[derive(Debug)]
pub struct GateResult {
    pub violations: Vec<String>,
}

impl GateResult {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Run `cargo metadata --no-deps` from `workspace_dir` (CI runs this from
/// `backend/`, same as every other `console-gate-*`) and return the raw JSON.
pub fn load_metadata(workspace_dir: &Path) -> Result<Value, String> {
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(workspace_dir)
        .output()
        .map_err(|e| format!("failed to run `cargo metadata`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`cargo metadata` failed:\n{stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).map_err(|e| format!("failed to parse metadata JSON: {e}"))
}

/// Check that [`TARGET_FEATURE`] is not reachable from [`TARGET_PACKAGE`]'s
/// `default` feature, walking the feature graph (a feature's value is a list of
/// other feature names of the SAME package, `dep:name` optional-dependency
/// activations, or `dep-name/feature-name` cross-crate activations — only the
/// first form can ever re-enter this package's own `dev-auth` feature).
pub fn check(metadata: &Value) -> Result<GateResult, String> {
    let packages = metadata["packages"]
        .as_array()
        .ok_or("metadata JSON has no `packages` array")?;
    let app = packages
        .iter()
        .find(|pkg| pkg["name"].as_str() == Some(TARGET_PACKAGE))
        .ok_or_else(|| format!("workspace package `{TARGET_PACKAGE}` not found"))?;
    let features = app["features"]
        .as_object()
        .ok_or_else(|| format!("`{TARGET_PACKAGE}` has no `features` table in cargo metadata"))?;

    let mut violations = Vec::new();

    // Sanity guard: if the feature has been renamed or deleted outright, this
    // gate would otherwise pass VACUOUSLY (nothing named `dev-auth` to find),
    // silently stopping being a real check. Fail loudly instead so a rename
    // must update this gate deliberately.
    if !features.contains_key(TARGET_FEATURE) {
        violations.push(format!(
            "`{TARGET_PACKAGE}` no longer defines a `{TARGET_FEATURE}` feature — \
             expected `{TARGET_FEATURE} = [\"console-platform-auth-rest/{TARGET_FEATURE}\"]`; \
             has it been renamed or removed without updating this gate?"
        ));
    }

    let mut visited = BTreeSet::new();
    let mut stack = vec!["default".to_owned()];
    while let Some(name) = stack.pop() {
        if name == TARGET_FEATURE {
            violations.push(format!(
                "`{TARGET_PACKAGE}`'s `default` feature set transitively enables `{TARGET_FEATURE}` \
                 — a release build (no `--features` flag) would ship it"
            ));
            continue;
        }
        if !visited.insert(name.clone()) {
            continue;
        }
        if let Some(members) = features.get(&name).and_then(Value::as_array) {
            for member in members.iter().filter_map(Value::as_str) {
                // `dep:foo` (optional-dep activation) and `foo/bar` (a
                // DIFFERENT crate's feature) can never name THIS package's own
                // `dev-auth` feature — only a same-package feature reference
                // (a bare name that is itself a key in `features`) can.
                if !member.contains('/') && !member.starts_with("dep:") {
                    stack.push(member.to_owned());
                }
            }
        }
    }

    Ok(GateResult { violations })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metadata_with_features(features: Value) -> Value {
        json!({
            "packages": [
                { "name": "console-app", "features": features },
                { "name": "console-platform-auth-rest", "features": { "dev-auth": [] } },
            ]
        })
    }

    #[test]
    fn passes_when_dev_auth_is_not_in_default() {
        let metadata = metadata_with_features(json!({
            "dev-auth": ["console-platform-auth-rest/dev-auth"],
        }));
        let result = check(&metadata).unwrap();
        assert!(result.passed(), "{:?}", result.violations);
    }

    #[test]
    fn passes_when_default_only_enables_unrelated_features() {
        let metadata = metadata_with_features(json!({
            "dev-auth": ["console-platform-auth-rest/dev-auth"],
            "default": ["metrics"],
            "metrics": [],
        }));
        let result = check(&metadata).unwrap();
        assert!(result.passed(), "{:?}", result.violations);
    }

    #[test]
    fn fails_when_dev_auth_is_directly_in_default() {
        let metadata = metadata_with_features(json!({
            "dev-auth": ["console-platform-auth-rest/dev-auth"],
            "default": ["dev-auth"],
        }));
        let result = check(&metadata).unwrap();
        assert!(!result.passed());
    }

    #[test]
    fn fails_when_dev_auth_is_transitively_in_default() {
        let metadata = metadata_with_features(json!({
            "dev-auth": ["console-platform-auth-rest/dev-auth"],
            "default": ["convenience"],
            "convenience": ["dev-auth"],
        }));
        let result = check(&metadata).unwrap();
        assert!(!result.passed());
    }

    #[test]
    fn fails_when_the_feature_is_missing_entirely() {
        let metadata = metadata_with_features(json!({}));
        let result = check(&metadata).unwrap();
        assert!(!result.passed());
    }

    #[test]
    fn a_same_named_feature_on_a_dependency_does_not_count() {
        // `some-crate/dev-auth` enables a DIFFERENT crate's `dev-auth`
        // feature, not console-app's own — must not be treated as a graph edge.
        let metadata = metadata_with_features(json!({
            "dev-auth": ["console-platform-auth-rest/dev-auth"],
            "default": ["some-crate/dev-auth"],
        }));
        let result = check(&metadata).unwrap();
        assert!(result.passed(), "{:?}", result.violations);
    }
}
