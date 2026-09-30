//! CI entry point for the iac-tier gate.
//! Exits 0 on success, 1 on any violation.

use std::path::Path;

fn main() {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("ERROR: cannot determine cwd: {e}");
            std::process::exit(1);
        }
    };
    run_gate(&cwd);
}

fn run_gate(cwd: &Path) {
    let repo_root = console_gate_iac_tier::git_root(cwd).unwrap_or_else(|e| {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    });

    eprintln!(
        "console-gate-iac-tier: checking deploy/ under {}",
        repo_root.display()
    );

    let artifacts = console_gate_iac_tier::collect_artifacts(&repo_root).unwrap_or_else(|e| {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    });

    let result = console_gate_iac_tier::audit(&artifacts);

    if result.passed() {
        eprintln!(
            "console-gate-iac-tier: PASSED — {} IaC artifact(s) checked, 0 violations",
            artifacts.len()
        );
        std::process::exit(0);
    }

    eprintln!(
        "console-gate-iac-tier: FAILED — {} violation(s):",
        result.violations.len()
    );
    for v in &result.violations {
        eprintln!("  {v}");
    }
    std::process::exit(1);
}
