//! Mutation suite: the gate is proven to still REJECT.
//!
//! `cargo run -p console-gate-fabricated-branch` exiting 0 against this tree
//! proves nothing on its own — a gate whose scan root stops resolving also exits
//! 0. These tests plant the fabrication in a throwaway tree and assert it is
//! caught, and assert that a tree with no Rust in it is an ERROR rather than a
//! pass.

use console_gate_fabricated_branch::{ViolationKind, check_source_tree, check_workspace};
use std::fs;
use std::path::{Path, PathBuf};

fn temp_workspace(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!(
        "console-fabricated-branch-gate-test-{name}-{}",
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
fn gate_flags_a_reintroduced_representative_branch_helper() -> Result<(), Box<dyn std::error::Error>>
{
    let ws = temp_workspace("representative")?;
    write_file(
        &ws.join("src/lib.rs"),
        r#"
fn representative_branch(principal: &Principal) -> Result<BranchId, RestError> {
    match &principal.branch_scope {
        BranchScope::All => Ok(BranchId::new()),
        BranchScope::Branches(branches) => branches.iter().next().copied().ok_or_else(|| {
            RestError::from_kernel(KernelError::forbidden("principal has no branch scope"))
        }),
    }
}

fn authorize_feature(principal: &Principal, feature: Feature) -> Result<(), RestError> {
    authorize(principal, Action::new(feature), representative_branch(principal)?)
        .map_err(RestError::from_kernel)
}
"#,
    )?;

    let result = check_source_tree(&ws)?;
    assert!(!result.passed(), "expected the fabrication to be flagged");
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::FabricatedAllArm),
        "expected FabricatedAllArm, got {:#?}",
        result.violations
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::TautologicalBranchesArm),
        "expected TautologicalBranchesArm, got {:#?}",
        result.violations
    );
    fs::remove_dir_all(&ws)?;
    Ok(())
}

/// The `.any()` shape reads as stricter than `.next()` and is exactly as vacuous:
/// the disjunction ranges over the principal's own branches.
#[test]
fn gate_flags_the_any_over_own_branches_variant() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("any-variant")?;
    write_file(
        &ws.join("src/hr.rs"),
        r#"
fn authorize_hr_scoped(principal: &Principal, feature: Feature) -> Result<(), HrError> {
    match &principal.branch_scope {
        BranchScope::Branches(branches) => {
            let action = Action::new(feature);
            if branches
                .iter()
                .any(|branch| authorize(principal, action, *branch).is_ok())
            {
                Ok(())
            } else {
                Err(HrError::forbidden())
            }
        }
        BranchScope::All => authorize_hr_org_wide(principal, feature),
    }
}
"#,
    )?;

    let result = check_source_tree(&ws)?;
    assert!(
        !result.passed(),
        "expected the .any() tautology to be flagged"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::TautologicalBranchesArm),
        "expected TautologicalBranchesArm, got {:#?}",
        result.violations
    );
    fs::remove_dir_all(&ws)?;
    Ok(())
}

/// THE SHAPE `cargo fmt` PRODUCES. Planted into `backend/app/src/lib.rs`, run
/// through `cargo fmt --all`, this is byte-for-byte what the formatter emits — and
/// what a scanner that stopped reading an arm at paren depth 0 scanned clean,
/// exit 0. A gate that misses the formatter's own output is worse than no gate,
/// because every future fabrication arrives formatted.
#[test]
fn gate_flags_the_rustfmt_wrapped_chain() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("rustfmt-wrapped")?;
    write_file(
        &ws.join("src/lib.rs"),
        r#"
fn authorize_audit_read(principal: &Principal) -> Result<(), ApiError> {
    let resource_branch = match &principal.branch_scope {
        BranchScope::All => BranchId::new(),
        BranchScope::Branches(branches) => branches
            .iter()
            .next()
            .copied()
            .ok_or_else(|| ApiError::forbidden("principal has no branch scope"))?,
    };
    authorize(
        principal,
        Action::new(Feature::AuditLogRead),
        resource_branch,
    )
    .map_err(ApiError::from_kernel)
}
"#,
    )?;

    let result = check_source_tree(&ws)?;
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::TautologicalBranchesArm),
        "the wrapped chain must be flagged, got {:#?}",
        result.violations
    );
    fs::remove_dir_all(&ws)?;
    Ok(())
}

/// THE MARKER IS THE EXEMPTION, and nothing else is.
///
/// An earlier revision inferred it: sibling `All => None` ⇒ yields `Option` ⇒
/// excused without a marker. That is the gate's first blind spot in executable
/// form — it never looks at what a CALLER does with the `Option`, and the
/// `.unwrap()` below is one function away. The same shape is flagged unmarked and
/// clean marked, which is the only distinction a text scanner can honestly draw.
#[test]
fn gate_requires_a_marker_on_an_option_yielding_pick() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("option-actor-branch")?;
    let unmarked = r#"
fn principal_create_branch(principal: &Principal) -> Option<BranchId> {
    match &principal.branch_scope {
        BranchScope::All => None,
        BranchScope::Branches(branches) => branches.iter().next().copied(),
    }
}

async fn create_site(state: &S, principal: &Principal, row: Row) -> Result<(), RestError> {
    let branch = principal_create_branch(principal).unwrap();
    authorize(principal, Action::new(Feature::X), branch).map_err(RestError::from_kernel)
}
"#;
    write_file(&ws.join("src/lib.rs"), unmarked)?;

    let result = check_source_tree(&ws)?;
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::TautologicalBranchesArm),
        "an unmarked pick must be flagged, got {:#?}",
        result.violations
    );

    write_file(
        &ws.join("src/lib.rs"),
        &unmarked.replace(
            "        BranchScope::Branches(branches) => branches.iter().next().copied(),",
            "        // fabricated-branch: ok default branch for row CREATION; never reaches authorize\n        BranchScope::Branches(branches) => branches.iter().next().copied(),",
        ),
    )?;

    let result = check_source_tree(&ws)?;
    assert!(result.passed(), "{:#?}", result.violations);
    fs::remove_dir_all(&ws)?;
    Ok(())
}

/// "Green" and "scanned nothing" must not be the same observation.
#[test]
fn gate_errors_when_it_walked_zero_rust_files() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("empty")?;
    write_file(&ws.join("README.md"), "no rust here\n")?;

    assert!(
        check_workspace(&ws).is_err(),
        "a gate that scanned nothing must report an ERROR, not PASSED"
    );
    fs::remove_dir_all(&ws)?;
    Ok(())
}

/// A production file may never disappear behind a path-wide handoff. These are
/// the three exact suffixes that were once excluded while still being counted as
/// scanned; planting the known shape at each suffix proves the scanner actually
/// opens every one of them.
#[test]
fn gate_scans_every_formerly_excluded_production_path() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("former-path-exclusions")?;
    let fabrication = r#"
fn authorize_feature(principal: &Principal, feature: Feature) -> Result<(), RestError> {
    let branch = match &principal.branch_scope {
        BranchScope::All => BranchId::new(),
        BranchScope::Branches(branches) => branches.iter().next().copied().unwrap(),
    };
    authorize(principal, Action::new(feature), branch).map_err(RestError::from_kernel)
}
"#;
    for suffix in [
        "crates/reporting/rest/src/lib.rs",
        "crates/registry/rest/src/lib.rs",
        "app/src/hr.rs",
    ] {
        write_file(&ws.join(suffix), fabrication)?;
    }

    let result = check_source_tree(&ws)?;
    assert_eq!(result.files_scanned, 3);
    assert_eq!(
        result
            .violations
            .iter()
            .filter(|violation| violation.kind == ViolationKind::FabricatedAllArm)
            .count(),
        3,
        "every former path exclusion must be scanned: {:#?}",
        result.violations
    );
    assert_eq!(
        result
            .violations
            .iter()
            .filter(|violation| violation.kind == ViolationKind::TautologicalBranchesArm)
            .count(),
        3,
        "every former path exclusion must reject the principal-member pick: {:#?}",
        result.violations
    );
    fs::remove_dir_all(&ws)?;
    Ok(())
}

/// The migrated shape must be clean, or the gate would block its own fix.
#[test]
fn gate_passes_the_migrated_capability_shape() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("migrated")?;
    write_file(
        &ws.join("src/lib.rs"),
        r#"
fn authorize_feature(principal: &Principal, feature: Feature) -> Result<(), RestError> {
    authorize_capability(principal, Action::new(feature)).map_err(RestError::from_kernel)
}

async fn authorize_on_row(store: &S, principal: &Principal, id: Id) -> Result<(), RestError> {
    let branch_id = store.branch_in_scope(id, &principal.branch_scope).await?;
    authorize(principal, Action::new(Feature::X), branch_id).map_err(RestError::from_kernel)
}
"#,
    )?;

    let result = check_source_tree(&ws)?;
    assert!(result.passed(), "{:#?}", result.violations);
    assert_eq!(result.files_scanned, 1);
    fs::remove_dir_all(&ws)?;
    Ok(())
}
