//! ADR-0041: sight browser UI surfaces smuggled into non-ui crates — the
//! class the build-graph gate (`scripts/console/route-inventory.mjs`) cannot
//! see from source text.
//!
//! Residual after Layer::Ui is accepted:
//! 1. A workspace member named `console-<domain>-ui` must classify as
//!    [`Layer::Ui`](crate::Layer::Ui), not silent [`Layer::Adapter`](crate::Layer::Adapter)
//!    or [`Layer::Platform`](crate::Layer::Platform). Existence is legal.
//!    Needle scan is skipped for Ui members; their views are the chartered surface.
//! 2. Hand-rolled HTML / Leptos view macros smuggled inside an existing
//!    conforming crate (e.g. `-rest`) never touch Cargo.toml / Cargo.lock;
//!    this module walks that crate's Rust sources for high-signal markers.
//!
//! Deliberately not covered here (ownerLease residual): a vendored Leptos fork
//! published under a false package name that also renames its macros — that is
//! deliberate-fraud territory below source-needle granularity; do not weaken
//! the 9ze build-graph gate to chase it.

use std::path::{Path, PathBuf};

use crate::{Layer, Metadata, Package, Violation, ViolationKind, classify_crate};

/// High-signal needles for a browser-visible UI surface in Rust source.
/// Built at runtime so this file never contains a ready-to-match `view!` /
/// `axum::response::Html` literal that would flag the gate crate if the Gate
/// exemption were ever removed.
fn ui_surface_needles() -> Vec<(&'static str, String)> {
    vec![
        ("leptos_view_macro", format!("{}{}", "view", "!")),
        ("leptos_or_dioxus_rsx_macro", format!("{}{}", "rsx", "!")),
        (
            "axum_html_response_path",
            format!("{}{}{}", "axum", "::response::", "Html"),
        ),
        (
            "axum_html_response_import",
            format!("{}{}{}", "use axum::response::", "Html", ";"),
        ),
        ("axum_html_constructor", format!("{}{}", "Html", "(")),
        ("html_doctype", format!("{}{}{}", "<!", "DOCTYPE", " html")),
        ("maud_template", format!("{}{}", "maud", "::")),
        ("askama_template", format!("{}{}", "askama", "::")),
        ("leptos_path", format!("{}{}", "leptos", "::")),
        (
            "leptos_axum_path",
            format!("{}{}{}", "leptos", "_axum", "::"),
        ),
    ]
}

/// Scan non-gate, non-ui crate sources for smuggled UI markers.
///
/// [`Layer::Ui`] members are legal (ADR-0041) and are not needle-scanned.
/// Returns `Err` when the scan examined zero Rust files across a non-empty
/// set of scannable packages (examined-zero must not pass).
pub fn check_ui_surfaces(metadata: &Metadata) -> Result<Vec<Violation>, String> {
    let workspace_member_ids: std::collections::HashSet<&str> = metadata
        .workspace_members
        .iter()
        .map(|s| s.as_str())
        .collect();

    let workspace_pkgs: Vec<&Package> = metadata
        .packages
        .iter()
        .filter(|p| workspace_member_ids.contains(p.id.as_str()))
        .collect();

    let mut violations = Vec::new();
    let mut files_examined: usize = 0;
    let mut packages_scanned: usize = 0;
    let needles = ui_surface_needles();

    for pkg in &workspace_pkgs {
        let layer = classify_crate(&pkg.name, &pkg.manifest_path, &metadata.workspace_root);

        // Gate is exempt. Ui is the chartered surface: skip needles so a
        // legal `view!` in console-<domain>-ui is not SmuggledUiSurface.
        if layer == Layer::Gate || layer == Layer::Ui {
            continue;
        }

        packages_scanned += 1;
        let crate_root = Path::new(&pkg.manifest_path)
            .parent()
            .ok_or_else(|| format!("manifest path has no parent: {}", pkg.manifest_path))?;
        let rust_files = rust_sources_under(crate_root)?;
        if rust_files.is_empty() {
            return Err(format!(
                "ui-surface scan: crate '{}' under {} has zero Rust source files (examined-zero fails)",
                pkg.name,
                crate_root.display()
            ));
        }

        for file in rust_files {
            files_examined += 1;
            let Ok(source) = std::fs::read_to_string(&file) else {
                return Err(format!("ui-surface scan: cannot read {}", file.display()));
            };
            // Case-fold only for the HTML doctype needle; other markers are
            // Rust identifiers / paths and stay case-sensitive.
            let source_lower = source.to_ascii_lowercase();
            for (label, needle) in &needles {
                let hit = if *label == "html_doctype" {
                    source_lower.contains(&needle.to_ascii_lowercase())
                } else {
                    source.contains(needle.as_str())
                };
                if hit {
                    let rel = file.strip_prefix(&metadata.workspace_root).unwrap_or(&file);
                    violations.push(Violation {
                        kind: ViolationKind::SmuggledUiSurface,
                        crate_name: pkg.name.clone(),
                        detail: format!(
                            "ADR-0041: {} contains UI surface marker '{}' ({}); browser-visible markup/views belong in a Layer::Ui crate (console-<domain>-ui), not inside {}",
                            rel.display(),
                            needle,
                            label,
                            pkg.name
                        ),
                    });
                    // One hit per file is enough to fail closed; further
                    // needles in the same file would only amplify noise.
                    break;
                }
            }
        }
    }

    if packages_scanned > 0 && files_examined == 0 {
        return Err(
            "ui-surface scan examined zero Rust source files across scannable workspace packages"
                .to_owned(),
        );
    }

    Ok(violations)
}

fn rust_sources_under(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("cannot read_dir {}: {e}", dir.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|e| format!("cannot read entry under {}: {e}", dir.display()))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
            if file_type.is_dir() {
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                // target/ is build output; .git should not appear under a crate
                // root, but skip it if a fixture nests one.
                if name == "target" || name == ".git" {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() && path.extension().and_then(|s| s.to_str()) == Some("rs")
            {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn needles_include_view_macro_and_axum_html() {
        let needles = ui_surface_needles();
        let joined: Vec<&str> = needles.iter().map(|(_, n)| n.as_str()).collect();
        assert!(joined.contains(&"view!"));
        assert!(joined.contains(&"axum::response::Html"));
        assert!(joined.contains(&"Html("));
    }

    #[test]
    fn rust_sources_skips_target_dir() {
        let dir = std::env::temp_dir().join(format!("console-gate-ui-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target/debug")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn x() {}\n").unwrap();
        std::fs::write(dir.join("target/debug/foo.rs"), "fn y() {}\n").unwrap();
        let files = rust_sources_under(&dir).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("src/lib.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
