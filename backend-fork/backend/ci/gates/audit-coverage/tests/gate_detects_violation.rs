use console_gate_audit_coverage::{ViolationKind, allowed_audit_exclusions, check_source_tree};
use std::fs;
use std::path::{Path, PathBuf};

fn temp_workspace(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!(
        "console-audit-gate-test-{name}-{}",
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
fn allowed_exclusion_set_is_the_bound_carveouts() {
    let exclusions = allowed_audit_exclusions();
    assert_eq!(exclusions.len(), 3);
    // Each exemption is bound to the REAL writer: repo-relative file + function.
    // This is what prevents the carve-out from silently applying to the wrong
    // handler (ADR-0014 "exactly one path" invariant).
    assert_eq!(exclusions[0].reason, "location_ping_ingestion");
    assert_eq!(
        exclusions[0].file,
        "crates/compliance/adapter-postgres/src/lib.rs"
    );
    assert_eq!(exclusions[0].function, "record_location_ping");
    // The retention purge erases expired location-derived data; not a business
    // write. Bound to the retention writer.
    assert_eq!(exclusions[1].reason, "location_data_retention_purge");
    assert_eq!(
        exclusions[1].file,
        "crates/compliance/adapter-postgres/src/lib.rs"
    );
    assert_eq!(exclusions[1].function, "purge_expired_location_data");
    // High-volume console RUM/route telemetry must not flood audit_events.
    assert_eq!(exclusions[2].reason, "console_route_telemetry_ingestion");
    assert_eq!(exclusions[2].file, "app/src/console_telemetry.rs");
    assert_eq!(exclusions[2].function, "insert_route_telemetry");
}

#[test]
fn gate_fails_state_changing_handler_without_audit() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("missing-audit")?;
    write_file(
        &ws.join("crates/workorder/rest/src/lib.rs"),
        r#"
// console-gate: state-changing-handler
pub async fn approve_work_order(pool: &PgPool, id: WorkOrderId) {
    sqlx::query!("UPDATE work_orders SET status = 'APPROVED' WHERE id = $1", id)
        .execute(pool)
        .await;
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(!result.passed(), "expected missing audit violation");
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::MissingAuditEvent),
        "expected MissingAuditEvent, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_fails_unmarked_mutating_rest_handler() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("unmarked-mutation")?;
    write_file(
        &ws.join("crates/workorder/rest/src/lib.rs"),
        r#"
pub async fn approve_work_order(pool: &PgPool, id: WorkOrderId) {
    sqlx::query!("UPDATE work_orders SET status = 'APPROVED' WHERE id = $1", id)
        .execute(pool)
        .await;
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(
        !result.passed(),
        "expected unmarked mutating REST handler violation"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::MissingAuditEvent),
        "expected MissingAuditEvent, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_ignores_transaction_scoped_mutation_helper_but_not_its_unaudited_handler()
-> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("transaction-helper")?;
    write_file(
        &ws.join("crates/workorder/rest/src/lib.rs"),
        r#"
async fn insert_history(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: WorkOrderId,
) {
    sqlx::query!("INSERT INTO work_order_history (id) VALUES ($1)", id)
        .execute(&mut **tx)
        .await;
}

pub async fn approve_work_order(pool: &PgPool, id: WorkOrderId) {
    sqlx::query!("UPDATE work_orders SET status = 'APPROVED' WHERE id = $1", id)
        .execute(pool)
        .await;
}
"#,
    )?;

    let result = check_source_tree(&ws);
    let missing_audits: Vec<_> = result
        .violations
        .iter()
        .filter(|violation| violation.kind == ViolationKind::MissingAuditEvent)
        .collect();
    assert_eq!(
        missing_audits.len(),
        1,
        "only the transaction-owning handler should require audit coverage: {:#?}",
        result.violations
    );
    assert_eq!(
        missing_audits[0].function_name.as_deref(),
        Some("approve_work_order")
    );
    Ok(())
}

#[test]
fn gate_does_not_treat_select_for_update_as_mutation() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("select-for-update")?;
    write_file(
        &ws.join("crates/workorder/rest/src/lib.rs"),
        r#"
pub async fn lock_work_order(pool: &PgPool, id: WorkOrderId) {
    sqlx::query!("SELECT id FROM work_orders WHERE id = $1 FOR UPDATE", id)
        .fetch_one(pool)
        .await;
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(
        result.passed(),
        "row locking without a state mutation should not require an audit event: {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_does_not_accept_audit_words_in_comments() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("comment-only-audit")?;
    write_file(
        &ws.join("crates/workorder/rest/src/lib.rs"),
        r#"
// console-gate: state-changing-handler
pub async fn approve_work_order(pool: &PgPool, id: WorkOrderId) {
    // TODO: add with_audit and AuditEvent later.
    sqlx::query!("UPDATE work_orders SET status = 'APPROVED' WHERE id = $1", id)
        .execute(pool)
        .await;
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(
        !result.passed(),
        "expected comment-only audit words to be rejected"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::MissingAuditEvent),
        "expected MissingAuditEvent, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_passes_state_changing_handler_with_audit() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("with-audit")?;
    write_file(
        &ws.join("crates/workorder/rest/src/lib.rs"),
        r#"
// console-gate: state-changing-handler
pub async fn approve_work_order(pool: &PgPool) {
    let event = AuditEvent::new(
        Some(actor),
        action,
        "work_order",
        id,
        trace,
        occurred_at,
    );
    with_audit(pool, event, |tx| Box::pin(async move {
        sqlx::query!("UPDATE work_orders SET status = 'APPROVED' WHERE id = $1", id)
            .execute(tx.as_mut())
            .await?;
        Ok(())
    })).await
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(
        result.passed(),
        "expected audited handler to pass, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_allows_exactly_one_location_ping_exemption() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("location-exemption")?;
    write_file(
        &ws.join("crates/compliance/adapter-postgres/src/lib.rs"),
        r#"
// console-gate: state-changing-handler
// console-gate: audit-exempt location_ping_ingestion
pub async fn record_location_ping(pool: &PgPool, ping: LocationPing) {
    sqlx::query!("INSERT INTO location_pings (user_id, lat, lon) VALUES ($1, $2, $3)")
        .execute(pool)
        .await;
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(
        result.passed(),
        "expected LocationPing carve-out to pass, got {:#?}",
        result.violations
    );
    assert_eq!(result.observed_exclusions.len(), 1);
    assert_eq!(
        result.observed_exclusions[0].reason,
        "location_ping_ingestion"
    );
    Ok(())
}

#[test]
fn gate_rejects_location_exemption_on_a_different_handler() -> Result<(), Box<dyn std::error::Error>>
{
    // The location_ping_ingestion reason is path-bound: applying it to ANY other
    // handler (here a work-order REST handler) must be rejected, proving the
    // carve-out cannot silently migrate to the wrong writer.
    let ws = temp_workspace("misbound-location-exemption")?;
    write_file(
        &ws.join("crates/workorder/rest/src/lib.rs"),
        r#"
// console-gate: state-changing-handler
// console-gate: audit-exempt location_ping_ingestion
pub async fn approve_work_order(pool: &PgPool, id: WorkOrderId) {
    sqlx::query!("UPDATE work_orders SET status = 'APPROVED' WHERE id = $1", id)
        .execute(pool)
        .await;
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(
        !result.passed(),
        "expected the misbound location exemption to be rejected"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::MisboundAuditExclusion),
        "expected MisboundAuditExclusion, got {:#?}",
        result.violations
    );
    assert!(
        result.observed_exclusions.is_empty(),
        "a misbound exemption must not count as an observed exclusion, got {:#?}",
        result.observed_exclusions
    );
    Ok(())
}

#[test]
fn gate_rejects_location_exemption_on_wrong_function_in_bound_file()
-> Result<(), Box<dyn std::error::Error>> {
    // Even inside the bound adapter-postgres file, the exemption only applies to
    // the bound `record_location_ping` function. A different function in the same
    // file must be rejected.
    let ws = temp_workspace("wrong-fn-location-exemption")?;
    write_file(
        &ws.join("crates/compliance/adapter-postgres/src/lib.rs"),
        r#"
// console-gate: state-changing-handler
// console-gate: audit-exempt location_ping_ingestion
pub async fn delete_location_ping(pool: &PgPool, id: PingId) {
    sqlx::query!("DELETE FROM location_pings WHERE id = $1", id)
        .execute(pool)
        .await;
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(
        !result.passed(),
        "expected the exemption on the wrong function to be rejected"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::MisboundAuditExclusion),
        "expected MisboundAuditExclusion, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_rejects_unknown_audit_exemption() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("unknown-exemption")?;
    write_file(
        &ws.join("crates/workorder/rest/src/lib.rs"),
        r#"
// console-gate: state-changing-handler
// console-gate: audit-exempt temporary_backfill
pub async fn backfill_work_order(pool: &PgPool) {
    sqlx::query!("UPDATE work_orders SET status = 'APPROVED'")
        .execute(pool)
        .await;
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(!result.passed(), "expected unknown exemption violation");
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UnknownAuditExclusion),
        "expected UnknownAuditExclusion, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_fails_unmarked_mutating_app_handler() -> Result<(), Box<dyn std::error::Error>> {
    // Regression for console-937 / gh#396: backend/app/src is a handler surface.
    let ws = temp_workspace("app-unmarked-mutation")?;
    write_file(
        &ws.join("app/src/collaboration.rs"),
        r#"
pub async fn create_calendar_event(pool: &PgPool) {
    sqlx::query!("INSERT INTO collaboration_calendar_events (id) VALUES ($1)", id)
        .execute(pool)
        .await;
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(
        !result.passed(),
        "expected unmarked mutating app handler violation"
    );
    assert!(
        result
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::MissingAuditEvent
                && v.function_name.as_deref() == Some("create_calendar_event")),
        "expected MissingAuditEvent on app collaboration handler, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_passes_audited_app_workflow_studio_handler() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("app-workflow-studio-audit")?;
    write_file(
        &ws.join("app/src/workflow_studio.rs"),
        r#"
pub async fn publish_definition(pool: &PgPool) {
    let event = AuditEvent::new(Some(actor), action, "workflow_definition", id, trace, occurred_at);
    with_audit(pool, event, |tx| Box::pin(async move {
        sqlx::query!("UPDATE workflow_definitions SET status = 'PUBLISHED' WHERE id = $1", id)
            .execute(tx.as_mut())
            .await?;
        Ok(())
    })).await
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(
        result.passed(),
        "expected audited workflow studio app handler to pass, got {:#?}",
        result.violations
    );
    Ok(())
}

#[test]
fn gate_allows_bound_console_telemetry_exemption() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("app-telemetry-exemption")?;
    write_file(
        &ws.join("app/src/console_telemetry.rs"),
        r#"
// console-gate: state-changing-handler
// console-gate: audit-exempt console_route_telemetry_ingestion
pub async fn insert_route_telemetry(pool: &PgPool) {
    sqlx::query!("INSERT INTO console_route_telemetry (org_id) VALUES ($1)")
        .execute(pool)
        .await;
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(
        result.passed(),
        "expected bound console telemetry carve-out to pass, got {:#?}",
        result.violations
    );
    assert_eq!(result.observed_exclusions.len(), 1);
    assert_eq!(
        result.observed_exclusions[0].reason,
        "console_route_telemetry_ingestion"
    );
    Ok(())
}

#[test]
fn gate_does_not_treat_header_insert_as_sql_mutation() -> Result<(), Box<dyn std::error::Error>> {
    let ws = temp_workspace("header-insert-not-sql")?;
    write_file(
        &ws.join("app/src/hr.rs"),
        r#"
pub async fn export_csv(pool: &PgPool) {
    let rows = sqlx::query!("SELECT id FROM employees")
        .fetch_all(pool)
        .await;
    let mut response = String::new().into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv"),
    );
}
"#,
    )?;

    let result = check_source_tree(&ws);
    assert!(
        result.passed(),
        "Rust HeaderMap::insert must not count as SQL INSERT: {:#?}",
        result.violations
    );
    Ok(())
}
