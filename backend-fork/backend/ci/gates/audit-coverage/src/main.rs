//! CI entry point for the audit-coverage gate.

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
        "console-gate-audit-coverage: checking workspace at {}",
        workspace_dir.display()
    );

    let result = console_gate_audit_coverage::check_workspace(workspace_dir).unwrap_or_else(|e| {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    });

    if result.passed() {
        eprintln!("console-gate-audit-coverage: PASSED");
        std::process::exit(0);
    }

    eprintln!(
        "console-gate-audit-coverage: FAILED - {} violation(s):",
        result.violations.len()
    );
    for violation in &result.violations {
        eprintln!("  {violation}");
    }
    std::process::exit(1);
}
