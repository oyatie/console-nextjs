//! CI entry point for the layer-boundary gate.
//! Exits 0 on success, 1 on any violation.

use std::path::Path;

fn main() {
    // Default: check the workspace containing this binary's Cargo workspace.
    // In CI we run `cargo run -p console-gate-layer-boundary` from `backend/`.
    let workspace_dir = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("ERROR: cannot determine cwd: {e}");
            std::process::exit(1);
        }
    };
    run_gate(&workspace_dir);
}

fn run_gate(workspace_dir: &Path) {
    eprintln!(
        "console-gate-layer-boundary: checking workspace at {}",
        workspace_dir.display()
    );

    let (metadata, workspace_edition) = console_gate_layer_boundary::load_metadata(workspace_dir)
        .unwrap_or_else(|e| {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        });

    let mut result = console_gate_layer_boundary::check(&metadata, &workspace_edition);

    // MFL-0001: repo-wide unresolved-conflict-marker scan over git-tracked files.
    match console_gate_layer_boundary::git_tracked_files(workspace_dir) {
        Ok(files) => result
            .violations
            .extend(console_gate_layer_boundary::check_conflict_markers(&files)),
        Err(e) => {
            eprintln!("ERROR: conflict-marker scan: {e}");
            std::process::exit(1);
        }
    }

    // ADR-0041: source-level smuggled UI surfaces in non-ui crates.
    // Examined-zero / unreadable sources fail closed (Err → exit 1).
    match console_gate_layer_boundary::check_ui_surfaces(&metadata) {
        Ok(ui_violations) => result.violations.extend(ui_violations),
        Err(e) => {
            eprintln!("ERROR: ui-surface scan: {e}");
            std::process::exit(1);
        }
    }

    if result.passed() {
        eprintln!(
            "console-gate-layer-boundary: PASSED — {} workspace crates checked, 0 violations",
            metadata.workspace_members.len()
        );
        std::process::exit(0);
    }

    eprintln!(
        "console-gate-layer-boundary: FAILED — {} violation(s):",
        result.violations.len()
    );
    for v in &result.violations {
        eprintln!("  {v}");
    }
    std::process::exit(1);
}
