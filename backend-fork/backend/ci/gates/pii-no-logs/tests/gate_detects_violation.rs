use console_gate_pii_no_logs::{ViolationKind, check_source_tree};
use std::fs;
use std::path::{Path, PathBuf};

fn temp_workspace(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!(
        "console-pii-gate-test-{name}-{}",
        std::process::id()
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn write_file(path: &Path, content: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

#[test]
fn gate_flags_korean_phone_number_in_tracing_macro() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("phone")?;
    write_file(
        &ws.join("src/lib.rs"),
        r#"
pub fn log_phone() {
    tracing::info!("driver phone 010-1234-5678");
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(!result.passed(), "expected phone-number violation");
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::KoreanPhoneNumber),
        "expected KoreanPhoneNumber, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_flags_gps_coordinate_pair_in_log_macro() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("gps")?;
    write_file(
        &ws.join("src/lib.rs"),
        r#"
pub fn log_coords() {
    log::warn!("raw coordinate pair 37.5665, 126.9780");
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(!result.passed(), "expected GPS coordinate violation");
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::GpsCoordinatePair),
        "expected GpsCoordinatePair, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_flags_longitude_first_gps_coordinate_pair_in_log_macro()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("gps-lon-lat")?;
    write_file(
        &ws.join("src/lib.rs"),
        r#"
pub fn log_coords() {
    log::warn!("raw coordinate pair 126.9780, 37.5665");
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(
        !result.passed(),
        "expected longitude-first GPS coordinate violation"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::GpsCoordinatePair),
        "expected GpsCoordinatePair, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_flags_resident_registration_number_in_bare_log_macro()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("rrn")?;
    write_file(
        &ws.join("src/lib.rs"),
        r#"
pub fn log_rrn() {
    info!("resident id 900101-1234567");
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(!result.passed(), "expected RRN violation");
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::ResidentRegistrationNumber),
        "expected ResidentRegistrationNumber, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_ignores_pii_outside_log_macro_calls() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("outside")?;
    write_file(
        &ws.join("src/lib.rs"),
        r#"
const FIXTURE_PHONE: &str = "010-1234-5678";

pub fn no_log() {
    let _ = FIXTURE_PHONE;
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(
        result.passed(),
        "expected non-log fixture to pass, got {:#?}",
        result.violations
    );
    Ok(())
}

/// A gate that examined NO source file must fail, not pass.
///
/// Zero files yield zero violations, so the binary printed "PASSED" over an empty
/// directory — measured before this floor existed. The workspace holds thousands
/// of Rust files, so zero means the scan did not find them, never that none of
/// them log personal data. Absence of evidence was being reported as evidence of
/// absence, which for a PII gate is the whole point of the gate.
#[test]
fn examining_no_source_file_is_refused_not_passed() -> Result<(), Box<dyn std::error::Error>> {
    let dir = temp_workspace("empty-subject-set")?;
    // No `panic!`: clippy forbids it here. An Ok result collapses to an empty
    // string, which fails the `contains` below with the result printed.
    let outcome = match console_gate_pii_no_logs::check_workspace(&dir) {
        Ok(_) => String::new(),
        Err(message) => message,
    };
    assert!(
        outcome.contains("examined no Rust source files"),
        "an empty workspace must be REFUSED, naming the empty subject set; got {outcome:?}"
    );
    Ok(())
}

/// The positive control: a workspace WITH a clean source file still passes, so the
/// floor cannot be satisfied by a gate that refuses everything.
#[test]
fn a_workspace_with_a_clean_source_file_still_passes() -> Result<(), Box<dyn std::error::Error>> {
    let dir = temp_workspace("floor-positive-control")?;
    write_file(
        &dir.join("backend/crates/thing/src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )?;
    let result = console_gate_pii_no_logs::check_workspace(&dir)?;
    assert!(
        result.violations.is_empty(),
        "a clean source file must not be charged: {:#?}",
        result.violations
    );
    Ok(())
}
