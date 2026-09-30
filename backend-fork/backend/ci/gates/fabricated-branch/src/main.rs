//! CI entry point for the fabricated-branch gate.

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
        "console-gate-fabricated-branch: checking workspace at {}",
        workspace_dir.display()
    );

    let result =
        console_gate_fabricated_branch::check_workspace(workspace_dir).unwrap_or_else(|e| {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        });

    if result.passed() {
        // A PASSED line that overstates itself is how a grep gets mistaken for a
        // control. State the scope on the same line as the green.
        eprintln!(
            "console-gate-fabricated-branch: PASSED ({} Rust files scanned) - text scan, \
             defence in depth: three known blind spots are listed in the module doc \
             (backend/ci/gates/fabricated-branch/src/lib.rs) and docs/CI-GATES.md",
            result.files_scanned
        );
        std::process::exit(0);
    }

    eprintln!(
        "console-gate-fabricated-branch: FAILED - {} violation(s) across {} files:",
        result.violations.len(),
        result.files_scanned
    );
    for violation in &result.violations {
        eprintln!("  {violation}");
    }
    eprintln!(
        "\nA branch derived from the principal makes authorize()'s branch check a \
         tautology. Pass the resource's own branch, or use \
         console_platform_authz::authorize_capability (no branch) / \
         authorize_org_wide (all branches)."
    );
    std::process::exit(1);
}
