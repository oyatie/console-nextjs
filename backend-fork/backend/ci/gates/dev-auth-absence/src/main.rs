//! CI entry point for the dev-auth-absence gate.
//! Exits 0 on success, 1 on any violation.

use std::path::Path;

fn main() {
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
        "console-gate-dev-auth-absence: checking workspace at {}",
        workspace_dir.display()
    );

    let metadata =
        console_gate_dev_auth_absence::load_metadata(workspace_dir).unwrap_or_else(|e| {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        });

    let result = console_gate_dev_auth_absence::check(&metadata).unwrap_or_else(|e| {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    });

    if result.passed() {
        eprintln!(
            "console-gate-dev-auth-absence: PASSED — dev-auth is not in console-app's default features"
        );
        std::process::exit(0);
    }

    eprintln!(
        "console-gate-dev-auth-absence: FAILED — {} violation(s):",
        result.violations.len()
    );
    for violation in &result.violations {
        eprintln!("  {violation}");
    }
    std::process::exit(1);
}
