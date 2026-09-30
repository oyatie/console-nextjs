//! Integration tests: create throwaway Cargo workspaces in std::env::temp_dir(),
//! run the gate's check logic against them, and assert red/green behavior.
//!
//! Tests return `Result<(), Box<dyn std::error::Error>>` so they can use `?`
//! without triggering the `expect_used` / `unwrap_used` / `panic` lints.

use console_gate_layer_boundary::{
    Layer, ViolationKind, check, check_ui_surfaces, classify_crate, load_metadata,
};
use std::fs;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_workspace(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("console-gate-test-{name}-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn write_file(path: &std::path::Path, content: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Red case: domain → adapter (illegal edge)
// ---------------------------------------------------------------------------

#[test]
fn gate_detects_domain_depends_on_adapter() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("red")?;

    // Workspace Cargo.toml
    write_file(
        &ws.join("Cargo.toml"),
        r#"
[workspace]
resolver = "3"
members = ["crates/demo/domain", "crates/demo/adapter-postgres"]

[workspace.package]
edition = "2024"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
"#,
    )?;

    // console-demo-adapter-postgres (adapter layer)
    let adapter_dir = ws.join("crates/demo/adapter-postgres");
    write_file(
        &adapter_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-demo-adapter-postgres"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[lints]
workspace = true
"#,
    )?;
    write_file(&adapter_dir.join("src/lib.rs"), "// adapter placeholder\n")?;

    // console-demo-domain (domain layer) — ILLEGALLY depends on the adapter
    let domain_dir = ws.join("crates/demo/domain");
    write_file(
        &domain_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-demo-domain"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
console-demo-adapter-postgres = { path = "../adapter-postgres" }

[lints]
workspace = true
"#,
    )?;
    write_file(&domain_dir.join("src/lib.rs"), "// domain placeholder\n")?;

    let (metadata, edition) = load_metadata(&ws)?;
    let result = check(&metadata, &edition);

    assert!(
        !result.passed(),
        "expected gate to FAIL for domain→adapter edge, but it passed"
    );

    let has_illegal_edge = result
        .violations
        .iter()
        .any(|v| v.kind == ViolationKind::IllegalLayerEdge);
    assert!(
        has_illegal_edge,
        "expected an IllegalLayerEdge violation, got: {:#?}",
        result.violations
    );

    // Find the edge violation and check it names the domain crate
    let edge_violation = result
        .violations
        .iter()
        .find(|v| v.kind == ViolationKind::IllegalLayerEdge);
    assert!(
        edge_violation.is_some(),
        "should have an IllegalLayerEdge violation"
    );
    if let Some(ev) = edge_violation {
        assert_eq!(
            ev.crate_name, "console-demo-domain",
            "violation should be on the domain crate"
        );
    }

    eprintln!("RED case violations: {:#?}", result.violations);
    Ok(())
}

// ---------------------------------------------------------------------------
// Green case: legal edges only (kernel ← domain ← application)
// ---------------------------------------------------------------------------

#[test]
fn gate_passes_legal_edges() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("green")?;

    write_file(
        &ws.join("Cargo.toml"),
        r#"
[workspace]
resolver = "3"
members = [
    "crates/kernel/core",
    "crates/demo/domain",
    "crates/demo/application",
]

[workspace.package]
edition = "2024"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
"#,
    )?;

    // console-kernel-core
    let kernel_dir = ws.join("crates/kernel/core");
    write_file(
        &kernel_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-kernel-core"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[lints]
workspace = true
"#,
    )?;
    write_file(&kernel_dir.join("src/lib.rs"), "// kernel\n")?;

    // console-demo-domain → console-kernel-core (legal)
    let domain_dir = ws.join("crates/demo/domain");
    write_file(
        &domain_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-demo-domain"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
console-kernel-core = { path = "../../kernel/core" }

[lints]
workspace = true
"#,
    )?;
    write_file(&domain_dir.join("src/lib.rs"), "// domain\n")?;

    // console-demo-application → console-demo-domain (legal)
    let app_dir = ws.join("crates/demo/application");
    write_file(
        &app_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-demo-application"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
console-demo-domain = { path = "../domain" }
console-kernel-core = { path = "../../kernel/core" }

[lints]
workspace = true
"#,
    )?;
    write_file(&app_dir.join("src/lib.rs"), "// application\n")?;

    let (metadata, edition) = load_metadata(&ws)?;
    let result = check(&metadata, &edition);

    assert!(
        result.passed(),
        "expected gate to PASS for legal edges, but got violations: {:#?}",
        result.violations
    );

    eprintln!("GREEN case: PASSED with 0 violations");
    Ok(())
}

// ---------------------------------------------------------------------------
// Purity rule: domain layer forbids sqlx/axum/tokio as external deps
// (Validated via classify + forbidden_external_deps; no real dep resolution
//  needed — the gate enforces this at metadata parse time.)
// ---------------------------------------------------------------------------

#[test]
fn gate_detects_domain_depending_on_sqlx() {
    let layer = classify_crate(
        "console-workorder-domain",
        "/fake/ws/crates/workorder/domain/Cargo.toml",
        "/fake/ws",
    );
    assert_eq!(layer, Layer::Domain);
    let forbidden = layer.forbidden_external_deps();
    assert!(
        forbidden.contains(&"sqlx"),
        "domain should forbid sqlx; got {forbidden:?}"
    );
    assert!(
        forbidden.contains(&"axum"),
        "domain should forbid axum; got {forbidden:?}"
    );
    assert!(
        forbidden.contains(&"tokio"),
        "domain should forbid tokio; got {forbidden:?}"
    );
}

// ---------------------------------------------------------------------------
// Manifest hygiene: missing console- prefix is detected
// ---------------------------------------------------------------------------

#[test]
fn gate_detects_missing_console_prefix() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("prefix")?;

    write_file(
        &ws.join("Cargo.toml"),
        r#"
[workspace]
resolver = "3"
members = ["crates/kernel/core"]

[workspace.package]
edition = "2024"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
"#,
    )?;

    let kernel_dir = ws.join("crates/kernel/core");
    write_file(
        &kernel_dir.join("Cargo.toml"),
        r#"
[package]
name = "kernel-core"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[lints]
workspace = true
"#,
    )?;
    write_file(&kernel_dir.join("src/lib.rs"), "// kernel\n")?;

    let (metadata, edition) = load_metadata(&ws)?;
    let result = check(&metadata, &edition);

    let has_prefix_violation = result
        .violations
        .iter()
        .any(|v| v.kind == ViolationKind::MissingConsolePrefix);
    assert!(
        has_prefix_violation,
        "expected MissingConsolePrefix violation, got: {:#?}",
        result.violations
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Manifest hygiene: missing publish=false convention is detected
// ---------------------------------------------------------------------------

#[test]
fn gate_detects_missing_publish_false_convention() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("publish")?;

    write_file(
        &ws.join("Cargo.toml"),
        r#"
[workspace]
resolver = "3"
members = ["crates/kernel/core"]

[workspace.package]
edition = "2024"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
"#,
    )?;

    let kernel_dir = ws.join("crates/kernel/core");
    write_file(
        &kernel_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-kernel-core"
version = "0.1.0"
edition.workspace = true

[lints]
workspace = true
"#,
    )?;
    write_file(&kernel_dir.join("src/lib.rs"), "// kernel\n")?;

    let (metadata, edition) = load_metadata(&ws)?;
    let result = check(&metadata, &edition);

    assert!(
        !result.passed(),
        "expected missing publish=false convention to fail, but gate passed"
    );
    assert_eq!(
        result.violations.len(),
        1,
        "missing publish=false fixture should fail only for the intended manifest hygiene rule; got: {:#?}",
        result.violations
    );
    let violation = &result.violations[0];
    assert_eq!(
        violation.kind,
        ViolationKind::MissingPublishFalse,
        "expected MissingPublishFalse violation, got: {:#?}",
        result.violations
    );
    assert_eq!(
        violation.crate_name, "console-kernel-core",
        "violation should be scoped to the crate missing publish=false"
    );
    assert!(
        violation.detail.contains("publish = false"),
        "expected publish=false diagnostic, got: {:#?}",
        result.violations
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Manifest hygiene: missing [lints] workspace = true is detected
// ---------------------------------------------------------------------------

#[test]
fn gate_detects_missing_lints_workspace() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("lints")?;

    write_file(
        &ws.join("Cargo.toml"),
        r#"
[workspace]
resolver = "3"
members = ["crates/kernel/core"]

[workspace.package]
edition = "2024"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
"#,
    )?;

    let kernel_dir = ws.join("crates/kernel/core");
    // Deliberately omit [lints] section
    write_file(
        &kernel_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-kernel-core"
version = "0.1.0"
edition.workspace = true
publish.workspace = true
"#,
    )?;
    write_file(&kernel_dir.join("src/lib.rs"), "// kernel\n")?;

    let (metadata, edition) = load_metadata(&ws)?;
    let result = check(&metadata, &edition);

    let has_lints_violation = result
        .violations
        .iter()
        .any(|v| v.kind == ViolationKind::MissingLintsWorkspace);
    assert!(
        has_lints_violation,
        "expected MissingLintsWorkspace violation, got: {:#?}",
        result.violations
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Contracts layer: crates/contracts/* is its own layer, not a generic adapter.
//
// Rule under test: DOMAIN crates must NOT depend on console-contracts, while
// REST adapters MAY. Contracts itself sits near the leaf (kernel only), so a
// contracts → domain edge is also forbidden — that is the edge a plain
// Adapter fallback would have silently allowed.
// ---------------------------------------------------------------------------

/// Builds a temp workspace containing `console-contracts` (at crates/contracts)
/// plus one extra crate whose manifest body is supplied by the caller.
fn contracts_workspace(
    tag: &str,
    other_dir: &str,
    other_manifest: &str,
    contracts_deps: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let ws = temp_workspace(tag)?;
    write_file(
        &ws.join("Cargo.toml"),
        &format!(
            r#"
[workspace]
resolver = "3"
members = ["crates/contracts", "{other_dir}"]

[workspace.package]
edition = "2024"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
"#
        ),
    )?;

    let contracts_dir = ws.join("crates/contracts");
    write_file(
        &contracts_dir.join("Cargo.toml"),
        &format!(
            r#"
[package]
name = "console-contracts"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
{contracts_deps}

[lints]
workspace = true
"#
        ),
    )?;
    write_file(&contracts_dir.join("src/lib.rs"), "// contracts\n")?;

    let other = ws.join(other_dir);
    write_file(&other.join("Cargo.toml"), other_manifest)?;
    write_file(&other.join("src/lib.rs"), "// other\n")?;
    Ok(ws)
}

#[test]
fn contracts_crate_is_classified_as_its_own_layer() {
    assert_eq!(
        classify_crate(
            "console-contracts",
            "/ws/crates/contracts/Cargo.toml",
            "/ws"
        ),
        Layer::Contracts,
        "crates/contracts must classify as the Contracts layer, not the Adapter fallback"
    );
}

#[test]
fn gate_detects_domain_depends_on_contracts() -> Result<(), Box<dyn std::error::Error>> {
    let ws = contracts_workspace(
        "contracts-domain",
        "crates/demo/domain",
        r#"
[package]
name = "console-demo-domain"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
console-contracts = { path = "../../contracts" }

[lints]
workspace = true
"#,
        "",
    )?;

    let (metadata, edition) = load_metadata(&ws)?;
    let result = check(&metadata, &edition);

    let edge = result
        .violations
        .iter()
        .find(|v| v.kind == ViolationKind::IllegalLayerEdge);
    assert!(
        edge.is_some(),
        "domain → console-contracts must be an IllegalLayerEdge, got: {:#?}",
        result.violations
    );
    let detail = edge.map(|v| v.detail.clone()).unwrap_or_default();
    assert!(
        detail.contains("console-demo-domain (domain) → console-contracts (contracts)"),
        "violation must name the contracts layer explicitly, got: {detail}"
    );
    Ok(())
}

#[test]
fn gate_forbids_contracts_depending_on_domain() -> Result<(), Box<dyn std::error::Error>> {
    let ws = contracts_workspace(
        "contracts-inverted",
        "crates/demo/domain",
        r#"
[package]
name = "console-demo-domain"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[lints]
workspace = true
"#,
        r#"console-demo-domain = { path = "../demo/domain" }"#,
    )?;

    let (metadata, edition) = load_metadata(&ws)?;
    let result = check(&metadata, &edition);

    let detail = result
        .violations
        .iter()
        .find(|v| v.kind == ViolationKind::IllegalLayerEdge)
        .map(|v| v.detail.clone())
        .unwrap_or_default();
    assert!(
        detail.contains("console-contracts (contracts) → console-demo-domain (domain)"),
        "contracts must not be allowed to reach back into domain, got: {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_allows_rest_depends_on_contracts() -> Result<(), Box<dyn std::error::Error>> {
    let ws = contracts_workspace(
        "contracts-rest",
        "crates/demo/rest",
        r#"
[package]
name = "console-demo-rest"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
console-contracts = { path = "../../contracts" }

[lints]
workspace = true
"#,
        "",
    )?;

    let (metadata, edition) = load_metadata(&ws)?;
    let result = check(&metadata, &edition);

    assert!(
        result.passed(),
        "rest → console-contracts must be allowed, got: {:#?}",
        result.violations
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ADR-0030 §8 residual (console-cvh): Ui classification + smuggled HTML sighting
// ---------------------------------------------------------------------------

#[test]
fn gate_detects_html_smuggled_inside_existing_rest_crate() -> Result<(), Box<dyn std::error::Error>>
{
    let ws = temp_workspace("smuggled-html")?;

    write_file(
        &ws.join("Cargo.toml"),
        r#"
[workspace]
resolver = "3"
members = ["crates/demo/rest"]

[workspace.package]
edition = "2024"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
"#,
    )?;

    let rest_dir = ws.join("crates/demo/rest");
    write_file(
        &rest_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-demo-rest"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[lints]
workspace = true
"#,
    )?;
    // Hostile: browser HTML served from a conforming -rest crate with no -ui
    // name and no leptos dependency — invisible to route-inventory's build-graph
    // tiers, which is exactly the 9ze residual this lane closes.
    write_file(
        &rest_dir.join("src/lib.rs"),
        "use axum::response::Html;\n\
         pub async fn page() -> Html<&'static str> {\n\
             Html(\"<!DOCTYPE html><html><body>payroll</body></html>\")\n\
         }\n",
    )?;

    let (metadata, edition) = load_metadata(&ws)?;
    let layer_result = check(&metadata, &edition);
    assert!(
        layer_result.passed(),
        "manifest/layer checks alone must stay green for a conforming -rest crate; got: {:#?}",
        layer_result.violations
    );

    let ui_violations = check_ui_surfaces(&metadata)?;
    let smuggled = ui_violations
        .iter()
        .find(|v| v.kind == ViolationKind::SmuggledUiSurface);
    assert!(
        smuggled.is_some(),
        "expected SmuggledUiSurface for axum Html inside -rest, got: {ui_violations:#?}"
    );
    let detail = smuggled.map(|v| v.detail.clone()).unwrap_or_default();
    assert!(
        detail.contains("console-demo-rest"),
        "violation must name the smuggling crate, got: {detail}"
    );
    assert!(
        detail.contains("ADR-0041"),
        "violation must cite ADR-0041, got: {detail}"
    );
    Ok(())
}

#[test]
fn gate_allows_ui_crate_and_skips_ui_needles() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("allow-ui")?;

    write_file(
        &ws.join("Cargo.toml"),
        r#"
[workspace]
resolver = "3"
members = ["crates/demo/ui", "crates/platform/ui"]

[workspace.package]
edition = "2024"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
"#,
    )?;

    let ui_dir = ws.join("crates/demo/ui");
    write_file(
        &ui_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-demo-ui"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[lints]
workspace = true
"#,
    )?;
    // Chartered surface: the view! needle is legal here and must not be
    // SmuggledUiSurface (needle scan is skipped for Layer::Ui).
    write_file(
        &ui_dir.join("src/lib.rs"),
        &format!("pub fn shell() {{}}\n{}{}\n", "view", "!"),
    )?;

    let platform_ui_dir = ws.join("crates/platform/ui");
    write_file(
        &platform_ui_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-platform-ui"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[lints]
workspace = true
"#,
    )?;
    write_file(
        &platform_ui_dir.join("src/lib.rs"),
        "// platform ui shell\n",
    )?;

    assert_eq!(
        classify_crate(
            "console-demo-ui",
            &ui_dir.join("Cargo.toml").to_string_lossy(),
            &ws.to_string_lossy()
        ),
        Layer::Ui
    );
    assert_eq!(
        classify_crate(
            "console-platform-ui",
            &platform_ui_dir.join("Cargo.toml").to_string_lossy(),
            &ws.to_string_lossy()
        ),
        Layer::Ui,
        "console-platform-ui must classify as Ui, not Platform"
    );

    let (metadata, edition) = load_metadata(&ws)?;
    let layer_result = check(&metadata, &edition);
    assert!(
        layer_result.passed(),
        "a legal ui crate must pass layer checks, got: {:#?}",
        layer_result.violations
    );

    let ui_violations = check_ui_surfaces(&metadata)?;
    assert!(
        ui_violations.is_empty(),
        "Ui members must not fail existence or needle scan, got: {ui_violations:#?}"
    );
    Ok(())
}

#[test]
fn gate_detects_illegal_ui_to_domain_edge() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("ui-domain")?;

    write_file(
        &ws.join("Cargo.toml"),
        r#"
[workspace]
resolver = "3"
members = ["crates/demo/domain", "crates/demo/ui"]

[workspace.package]
edition = "2024"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
"#,
    )?;

    let domain_dir = ws.join("crates/demo/domain");
    write_file(
        &domain_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-demo-domain"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[lints]
workspace = true
"#,
    )?;
    write_file(&domain_dir.join("src/lib.rs"), "// domain\n")?;

    let ui_dir = ws.join("crates/demo/ui");
    write_file(
        &ui_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-demo-ui"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
console-demo-domain = { path = "../domain" }

[lints]
workspace = true
"#,
    )?;
    write_file(
        &ui_dir.join("src/lib.rs"),
        "// ui illegally depending on domain\n",
    )?;

    let (metadata, edition) = load_metadata(&ws)?;
    let result = check(&metadata, &edition);
    let edge = result
        .violations
        .iter()
        .find(|v| v.kind == ViolationKind::IllegalLayerEdge);
    assert!(
        edge.is_some(),
        "expected IllegalLayerEdge for ui → domain, got: {:#?}",
        result.violations
    );
    assert_eq!(edge.map(|v| v.crate_name.as_str()), Some("console-demo-ui"));
    Ok(())
}

#[test]
fn gate_detects_ui_depending_on_sqlx() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("ui-sqlx")?;

    write_file(
        &ws.join("Cargo.toml"),
        r#"
[workspace]
resolver = "3"
members = ["crates/demo/ui"]

[workspace.package]
edition = "2024"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
"#,
    )?;

    let ui_dir = ws.join("crates/demo/ui");
    write_file(
        &ui_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-demo-ui"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
sqlx = "0.8"

[lints]
workspace = true
"#,
    )?;
    write_file(&ui_dir.join("src/lib.rs"), "// ui must not take sqlx\n")?;

    let (metadata, edition) = load_metadata(&ws)?;
    let result = check(&metadata, &edition);
    let forbidden = result
        .violations
        .iter()
        .find(|v| v.kind == ViolationKind::ForbiddenExternalDep);
    assert!(
        forbidden.is_some(),
        "expected ForbiddenExternalDep for ui → sqlx, got: {:#?}",
        result.violations
    );
    assert_eq!(
        forbidden.map(|v| v.crate_name.as_str()),
        Some("console-demo-ui")
    );
    let detail = forbidden.map(|v| v.detail.clone()).unwrap_or_default();
    assert!(
        detail.contains("sqlx"),
        "violation must name sqlx, got: {detail}"
    );
    Ok(())
}

#[test]
fn gate_allows_ui_to_contracts_and_app_to_ui() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("ui-legal")?;

    write_file(
        &ws.join("Cargo.toml"),
        r#"
[workspace]
resolver = "3"
members = ["crates/contracts/api", "crates/demo/ui", "app"]

[workspace.package]
edition = "2024"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
"#,
    )?;

    let contracts_dir = ws.join("crates/contracts/api");
    write_file(
        &contracts_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-contracts"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[lints]
workspace = true
"#,
    )?;
    write_file(&contracts_dir.join("src/lib.rs"), "// contracts\n")?;

    let ui_dir = ws.join("crates/demo/ui");
    write_file(
        &ui_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-demo-ui"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
console-contracts = { path = "../../contracts/api" }

[lints]
workspace = true
"#,
    )?;
    write_file(&ui_dir.join("src/lib.rs"), "// ui → contracts\n")?;

    let app_dir = ws.join("app");
    write_file(
        &app_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-app"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
console-demo-ui = { path = "../crates/demo/ui" }

[lints]
workspace = true
"#,
    )?;
    write_file(&app_dir.join("src/lib.rs"), "// app → ui\n")?;

    let (metadata, edition) = load_metadata(&ws)?;
    let result = check(&metadata, &edition);
    assert!(
        result.passed(),
        "ui → contracts and app → ui must be legal, got: {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_passes_clean_rest_crate_without_ui_markers() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("clean-rest-ui")?;

    write_file(
        &ws.join("Cargo.toml"),
        r#"
[workspace]
resolver = "3"
members = ["crates/demo/rest"]

[workspace.package]
edition = "2024"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
"#,
    )?;

    let rest_dir = ws.join("crates/demo/rest");
    write_file(
        &rest_dir.join("Cargo.toml"),
        r#"
[package]
name = "console-demo-rest"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[lints]
workspace = true
"#,
    )?;
    write_file(
        &rest_dir.join("src/lib.rs"),
        "// JSON REST handler — no browser markup\n\
         pub fn health() -> &'static str { \"ok\" }\n",
    )?;

    let (metadata, edition) = load_metadata(&ws)?;
    assert!(check(&metadata, &edition).passed());
    let ui_violations = check_ui_surfaces(&metadata)?;
    assert!(
        ui_violations.is_empty(),
        "clean -rest must not trip ui-surface scan, got: {ui_violations:#?}"
    );
    Ok(())
}
