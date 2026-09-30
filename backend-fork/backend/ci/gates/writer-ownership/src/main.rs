//! Runs the writer-ownership gate over the backend crate tree.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let report = match console_gate_writer_ownership::scan(&root) {
        Ok(report) => report,
        Err(error) => {
            eprintln!(
                "writer-ownership gate could not walk {}: {error}",
                root.display()
            );
            return ExitCode::FAILURE;
        }
    };
    println!(
        "writer-ownership: scanned {} production source files",
        report.scanned_files
    );

    let unknown = report.unknown();
    for violation in &unknown {
        println!(
            "writer-ownership VIOLATION: {} writes `{}`, owned by {} ({})",
            violation.offending_crate, violation.table, violation.owner_crate, violation.path
        );
    }
    let stale = report.stale_exemptions();
    for known in &stale {
        println!(
            "writer-ownership STALE RATCHET ENTRY: {} no longer writes `{}` — delete the entry ({})",
            known.offending_crate, known.table, known.removed_by
        );
    }
    for known in console_gate_writer_ownership::KNOWN_SECOND_WRITERS {
        println!(
            "writer-ownership known dual writer (ratcheted): {} -> `{}` in {}; removed by {}",
            known.offending_crate, known.table, known.source, known.removed_by
        );
    }

    for violation in &report.shared_violations {
        println!(
            "writer-ownership SHARED-TABLE VIOLATION: {} writes `{}`, permitted writers are {} ({})",
            violation.offending_crate,
            violation.table,
            violation.permitted_crates.join(", "),
            violation.path
        );
    }
    let stale_permitted = report.stale_permitted_writers();
    for (table, permitted) in &stale_permitted {
        println!(
            "writer-ownership STALE SHARED PERMISSION: {permitted} is permitted to write `{table}` \
             but holds no write — remove the permission or the receipt owner that grants it"
        );
    }

    if unknown.is_empty()
        && stale.is_empty()
        && report.shared_violations.is_empty()
        && stale_permitted.is_empty()
    {
        println!(
            "writer-ownership: OK — no new second writer, no stale ratchet entry, \
             no unpermitted shared-table writer"
        );
        return ExitCode::SUCCESS;
    }
    ExitCode::FAILURE
}
