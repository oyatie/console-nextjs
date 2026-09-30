//! CI entry point for the vendor lock-in phase-out gate.
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
    let repo_root = console_gate_vendor_lockin::git_root(cwd).unwrap_or_else(|e| {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    });
    let registry_path = repo_root.join(console_gate_vendor_lockin::registry_rel_path());

    eprintln!(
        "console-gate-vendor-lockin: checking managed-cloud seams under {}",
        repo_root.display()
    );

    let registry = console_gate_vendor_lockin::load_registry(&registry_path).unwrap_or_else(|e| {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    });
    let artifacts = console_gate_vendor_lockin::collect_artifacts(&repo_root).unwrap_or_else(|e| {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    });
    let result = console_gate_vendor_lockin::audit(&artifacts, &registry);

    if result.passed() {
        eprintln!(
            "console-gate-vendor-lockin: PASSED — {} repository text artifact(s) checked, 0 violations",
            artifacts.len()
        );
        std::process::exit(0);
    }

    eprintln!(
        "console-gate-vendor-lockin: FAILED — {} violation(s):",
        result.violations.len()
    );
    for violation in &result.violations {
        eprintln!("  {violation}");
    }
    std::process::exit(1);
}
