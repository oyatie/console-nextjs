#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! B01's bounded, non-serving Employment-owner read projection. This test is
//! deliberately separate from `employment_port_as_runtime_role`: the latter's
//! five-symptom red probe must remain executable while this new typed API is
//! absent. The adapter uses `console_rt`, not the migration-owner test pool.

use console_kernel_core::{OrgId, UserId};
use console_ontology_canonical_adapter_postgres::employment::{
    EmploymentAttributes, EmploymentCommand, EmploymentEffectiveProjection,
    EmploymentEffectiveStatus, EmploymentError, EmploymentQuery, PgEmploymentPort,
};
use console_ontology_canonical_domain::{CanonicalPort, CommandId, CommandReceipt};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0001);
const FOREIGN_ORG: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0002);
const SALES: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0010);
const TECH: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0011);
const STAFF: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0020);
const LEAD: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0021);

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) -> UserId {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(org)
        .bind(format!("effective-projection-{tag}"))
        .bind(format!("Effective projection {tag}"))
        .execute(owner_pool)
        .await
        .unwrap();
    let actor = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*actor.as_uuid())
        .bind(format!("Actor {tag}"))
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    actor
}

async fn seed_employee(owner_pool: &PgPool, org: Uuid, key: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO employees \
         (org_id, company, name, source_filename, source_sheet, source_row, source_key) \
         VALUES ($1, 'ACME', $2, 'seed.xlsx', 'Sheet1', 1, $2) RETURNING id",
    )
    .bind(org)
    .bind(key)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

async fn seed_structure(owner_pool: &PgPool) {
    for unit in [SALES, TECH] {
        sqlx::query("INSERT INTO org_units (org_id, id) VALUES ($1, $2)")
            .bind(ORG)
            .bind(unit)
            .execute(owner_pool)
            .await
            .unwrap();
    }
    for (position, unit) in [(STAFF, SALES), (LEAD, SALES)] {
        sqlx::query("INSERT INTO job_positions (org_id, id, org_unit_id) VALUES ($1, $2, $3)")
            .bind(ORG)
            .bind(position)
            .bind(unit)
            .execute(owner_pool)
            .await
            .unwrap();
    }
}

fn attributes(
    company: &str,
    unit: Option<Uuid>,
    position: Option<Uuid>,
    status: &str,
) -> EmploymentAttributes {
    EmploymentAttributes {
        company: company.to_owned(),
        org_unit_id: unit,
        job_position_id: position,
        employment_status: status.to_owned(),
    }
}

fn command(org: OrgId, actor: UserId, query: EmploymentQuery) -> EmploymentCommand {
    EmploymentCommand {
        org_id: org,
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        actor_id: actor,
        query,
        action_key: "revise".to_owned(),
        object_type_id: Uuid::nil(),
    }
}

async fn execute(port: &PgEmploymentPort, command: EmploymentCommand) -> CommandReceipt {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.execute(&command))
        .await
        .unwrap()
        .unwrap()
}

async fn effective(
    port: &PgEmploymentPort,
    org: OrgId,
    employee: Uuid,
    as_of: OffsetDateTime,
) -> Result<Option<EmploymentEffectiveProjection>, EmploymentError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.read_effective_for_employee(org, employee, as_of))
        .await
        .unwrap()
}

async fn current(
    port: &PgEmploymentPort,
    org: OrgId,
    employee: Uuid,
) -> Result<Option<EmploymentEffectiveProjection>, EmploymentError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.read_current_effective_for_employee(org, employee))
        .await
        .unwrap()
}

fn expect_state(
    row: &EmploymentEffectiveProjection,
    employment: Uuid,
    as_of: OffsetDateTime,
    state: EmploymentEffectiveStatus,
    active: bool,
    expected_fields: (Option<&str>, Option<Uuid>, Option<Uuid>),
) {
    let (company, unit, position) = expected_fields;
    assert_eq!(row.employment_id, employment);
    assert_eq!(row.captured_as_of, as_of);
    assert_eq!(row.state, state);
    assert_eq!(row.is_active, active);
    assert_eq!(row.company.as_deref(), company);
    assert_eq!(row.org_unit_id, unit);
    assert_eq!(row.job_position_id, position);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn employment_owner_projection_reports_effective_state_and_refuses_ambiguous_binding(
    owner_pool: PgPool,
) {
    let actor = seed_org(&owner_pool, ORG, "home").await;
    seed_structure(&owner_pool).await;
    let org = OrgId::from_uuid(ORG);
    let port = PgEmploymentPort::new(
        runtime_role_pool(&owner_pool).await,
        tokio::runtime::Handle::current(),
    );
    let db_now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let past = db_now - time::Duration::days(1);
    let boundary = db_now + time::Duration::seconds(15);

    let appointment_employee = seed_employee(&owner_pool, ORG, "projection-appointment").await;
    let appointment = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Appoint {
                employee_id: appointment_employee,
                valid_from: boundary,
                attributes: attributes("ACME", Some(SALES), Some(STAFF), "ACTIVE"),
            },
        ),
    )
    .await;
    let appointment_id: Uuid = appointment.result()["employment_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let transfer_employee = seed_employee(&owner_pool, ORG, "projection-transfer").await;
    let transfer = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Appoint {
                employee_id: transfer_employee,
                valid_from: past,
                attributes: attributes("ACME", Some(SALES), Some(STAFF), "ACTIVE"),
            },
        ),
    )
    .await;
    let transfer_id: Uuid = transfer.result()["employment_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Transfer {
                employment_id: transfer_id,
                valid_from: boundary,
                attributes: attributes("ACME", Some(TECH), Some(LEAD), "ACTIVE"),
            },
        ),
    )
    .await;

    let exit_employee = seed_employee(&owner_pool, ORG, "projection-exit").await;
    let exit = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Appoint {
                employee_id: exit_employee,
                valid_from: past,
                attributes: attributes("ACME", Some(SALES), Some(STAFF), "ACTIVE"),
            },
        ),
    )
    .await;
    let exit_id: Uuid = exit.result()["employment_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id: exit_id,
                valid_from: boundary,
                attributes: attributes("ACME", Some(SALES), Some(STAFF), "EXITED"),
            },
        ),
    )
    .await;

    let unknown_employee = seed_employee(&owner_pool, ORG, "projection-unknown").await;
    let unknown = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Appoint {
                employee_id: unknown_employee,
                valid_from: past,
                attributes: attributes("ACME", Some(SALES), Some(STAFF), "ACTIVE"),
            },
        ),
    )
    .await;
    let unknown_id: Uuid = unknown.result()["employment_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id: unknown_id,
                valid_from: boundary,
                attributes: attributes("ACME", Some(SALES), Some(STAFF), "UNKNOWN"),
            },
        ),
    )
    .await;

    let before: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert!(before < boundary, "fixture missed the future boundary");
    expect_state(
        &effective(&port, org, appointment_employee, before)
            .await
            .unwrap()
            .unwrap(),
        appointment_id,
        before,
        EmploymentEffectiveStatus::NotYetEffective,
        false,
        (None, None, None),
    );
    expect_state(
        &effective(&port, org, appointment_employee, boundary)
            .await
            .unwrap()
            .unwrap(),
        appointment_id,
        boundary,
        EmploymentEffectiveStatus::Active,
        true,
        (Some("ACME"), Some(SALES), Some(STAFF)),
    );
    expect_state(
        &effective(&port, org, transfer_employee, before)
            .await
            .unwrap()
            .unwrap(),
        transfer_id,
        before,
        EmploymentEffectiveStatus::Active,
        true,
        (Some("ACME"), Some(SALES), Some(STAFF)),
    );
    expect_state(
        &effective(&port, org, transfer_employee, boundary)
            .await
            .unwrap()
            .unwrap(),
        transfer_id,
        boundary,
        EmploymentEffectiveStatus::Active,
        true,
        (Some("ACME"), Some(TECH), Some(LEAD)),
    );
    expect_state(
        &effective(&port, org, exit_employee, boundary)
            .await
            .unwrap()
            .unwrap(),
        exit_id,
        boundary,
        EmploymentEffectiveStatus::Exited,
        false,
        (Some("ACME"), Some(SALES), Some(STAFF)),
    );
    expect_state(
        &effective(&port, org, unknown_employee, boundary)
            .await
            .unwrap()
            .unwrap(),
        unknown_id,
        boundary,
        EmploymentEffectiveStatus::Unknown,
        false,
        (Some("ACME"), Some(SALES), Some(STAFF)),
    );

    let db_before_current: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let current_appointment = current(&port, org, appointment_employee)
        .await
        .unwrap()
        .unwrap();
    let db_after_current: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert!(db_before_current <= current_appointment.captured_as_of);
    assert!(current_appointment.captured_as_of <= db_after_current);
    assert!(
        current_appointment.captured_as_of < boundary,
        "fixture crossed boundary before current read"
    );
    assert_eq!(
        current_appointment.state,
        EmploymentEffectiveStatus::NotYetEffective
    );
    assert_eq!(
        current_appointment,
        effective(
            &port,
            org,
            appointment_employee,
            current_appointment.captured_as_of
        )
        .await
        .unwrap()
        .unwrap(),
        "current wrapper must select at the same database instant it reports"
    );

    while !sqlx::query_scalar::<_, bool>("SELECT clock_timestamp() >= $1")
        .bind(boundary)
        .fetch_one(&owner_pool)
        .await
        .unwrap()
    {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let after_appointment = current(&port, org, appointment_employee)
        .await
        .unwrap()
        .unwrap();
    assert!(after_appointment.captured_as_of >= boundary);
    assert_eq!(after_appointment.state, EmploymentEffectiveStatus::Active);
    assert!(after_appointment.is_active);
    let after_transfer = current(&port, org, transfer_employee)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_transfer.org_unit_id, Some(TECH));
    let after_exit = current(&port, org, exit_employee).await.unwrap().unwrap();
    assert_eq!(after_exit.state, EmploymentEffectiveStatus::Exited);
    assert!(!after_exit.is_active);
    let after_unknown = current(&port, org, unknown_employee)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_unknown.state, EmploymentEffectiveStatus::Unknown);
    assert!(!after_unknown.is_active);
    expect_state(
        &effective(&port, org, exit_employee, past)
            .await
            .unwrap()
            .unwrap(),
        exit_id,
        past,
        EmploymentEffectiveStatus::Active,
        true,
        (Some("ACME"), Some(SALES), Some(STAFF)),
    );

    // An N:1 reverse binding is representable by migration 0214. Never pick
    // one of the two source rows as if its canonical state were unambiguous.
    let second_source = seed_employee(&owner_pool, ORG, "projection-ambiguous-source").await;
    sqlx::query(
        "INSERT INTO employment_source_bindings \
         (org_id, employee_id, employment_id, actor_id, payload_digest) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(ORG)
    .bind(second_source)
    .bind(transfer_id)
    .bind(*actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .execute(&owner_pool)
    .await
    .unwrap();
    assert!(
        effective(&port, org, transfer_employee, boundary)
            .await
            .is_err(),
        "ambiguous reverse source binding must be an explicit conflict"
    );
    assert!(
        current(&port, org, second_source).await.is_err(),
        "the second source must not bypass the same cardinality check"
    );

    // The rows exist in a second Company, but a home-org read must not expose
    // their state through the owner projection's runtime-role pool.
    let foreign_actor = seed_org(&owner_pool, FOREIGN_ORG, "foreign").await;
    let foreign_org = OrgId::from_uuid(FOREIGN_ORG);
    let foreign_employee = seed_employee(&owner_pool, FOREIGN_ORG, "projection-foreign").await;
    execute(
        &port,
        command(
            foreign_org,
            foreign_actor,
            EmploymentQuery::Appoint {
                employee_id: foreign_employee,
                valid_from: past,
                attributes: attributes("FOREIGN", None, None, "ACTIVE"),
            },
        ),
    )
    .await;
    assert_eq!(
        effective(&port, foreign_org, foreign_employee, boundary)
            .await
            .unwrap()
            .unwrap()
            .company
            .as_deref(),
        Some("FOREIGN"),
        "control: the foreign Employment genuinely exists"
    );
    assert!(
        effective(&port, org, foreign_employee, boundary)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        current(&port, org, foreign_employee)
            .await
            .unwrap()
            .is_none()
    );
}

/// Unreadable revisions and unresolved EXITED→ACTIVE changes cannot become
/// active work through this bounded read-only projection.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn employment_owner_projection_refuses_corruption_and_unresolved_reactivation(
    owner_pool: PgPool,
) {
    let actor = seed_org(&owner_pool, ORG, "invalid").await;
    seed_structure(&owner_pool).await;
    let org = OrgId::from_uuid(ORG);
    let port = PgEmploymentPort::new(
        runtime_role_pool(&owner_pool).await,
        tokio::runtime::Handle::current(),
    );
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let opened = now - time::Duration::days(3);

    let malformed_employee = seed_employee(&owner_pool, ORG, "projection-malformed").await;
    let malformed = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Appoint {
                employee_id: malformed_employee,
                valid_from: opened,
                attributes: attributes("ACME", Some(SALES), Some(STAFF), "ACTIVE"),
            },
        ),
    )
    .await;
    let malformed_id: Uuid = malformed.result()["employment_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    for (version, valid_from, attrs) in [
        (
            2_i64,
            now - time::Duration::days(2),
            serde_json::json!({
                "company": null, "employment_status": "ACTIVE",
                "org_unit_id": "not-a-uuid", "job_position_id": null
            }),
        ),
        (
            3_i64,
            now - time::Duration::days(1),
            serde_json::json!({
                "company": "ACME", "employment_status": "BOGUS",
                "org_unit_id": SALES.to_string(), "job_position_id": STAFF.to_string()
            }),
        ),
    ] {
        sqlx::query(
            "INSERT INTO employment_revisions \
             (org_id, employment_id, version, command_id, actor_id, payload_digest, \
              valid_from, attributes, receipt) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, '{}'::jsonb)",
        )
        .bind(ORG)
        .bind(malformed_id)
        .bind(version)
        .bind(Uuid::new_v4())
        .bind(*actor.as_uuid())
        .bind([0_u8; 32].as_slice())
        .bind(valid_from)
        .bind(attrs)
        .execute(&owner_pool)
        .await
        .unwrap();
    }
    assert!(
        effective(
            &port,
            org,
            malformed_employee,
            now - time::Duration::hours(36)
        )
        .await
        .is_err(),
        "ACTIVE with malformed company/UUID must not grant eligibility"
    );
    assert!(
        effective(&port, org, malformed_employee, now)
            .await
            .is_err(),
        "unknown revision status must not silently become UNKNOWN"
    );

    let no_revision_employee = seed_employee(&owner_pool, ORG, "projection-no-revision").await;
    let no_revision_head: Uuid = sqlx::query_scalar(
        "INSERT INTO employment_heads (org_id, valid_from) VALUES ($1, $2) RETURNING id",
    )
    .bind(ORG)
    .bind(opened)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO employment_source_bindings \
         (org_id, employee_id, employment_id, actor_id, payload_digest) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(ORG)
    .bind(no_revision_employee)
    .bind(no_revision_head)
    .bind(*actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .execute(&owner_pool)
    .await
    .unwrap();
    assert!(
        effective(&port, org, no_revision_employee, now)
            .await
            .is_err(),
        "past-open head without a revision is corruption, not NOT_YET_EFFECTIVE"
    );

    let reactivated_employee = seed_employee(&owner_pool, ORG, "projection-reactivated").await;
    let reactivated = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Appoint {
                employee_id: reactivated_employee,
                valid_from: opened,
                attributes: attributes("ACME", Some(SALES), Some(STAFF), "ACTIVE"),
            },
        ),
    )
    .await;
    let reactivated_id: Uuid = reactivated.result()["employment_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    for (valid_from, status) in [
        (now - time::Duration::days(2), "EXITED"),
        (now - time::Duration::days(1), "ACTIVE"),
    ] {
        execute(
            &port,
            command(
                org,
                actor,
                EmploymentQuery::Promote {
                    employment_id: reactivated_id,
                    valid_from,
                    attributes: attributes("ACME", Some(SALES), Some(STAFF), status),
                },
            ),
        )
        .await;
    }
    let stored_close: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT valid_to FROM employment_heads WHERE org_id = $1 AND id = $2")
            .bind(ORG)
            .bind(reactivated_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(stored_close, Some(now - time::Duration::days(2)));
    assert_eq!(
        effective(
            &port,
            org,
            reactivated_employee,
            now - time::Duration::hours(36)
        )
        .await
        .unwrap()
        .unwrap()
        .state,
        EmploymentEffectiveStatus::Exited
    );
    assert!(
        effective(&port, org, reactivated_employee, now)
            .await
            .is_err(),
        "ACTIVE after an unresolved close must fail closed"
    );
}

/// A malformed history can contain a revision before the relationship opened.
/// That row must not become an ACTIVE assignment merely because the read time
/// has passed the opening instant.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn preopening_revision_cannot_authorize_effective_or_current_reads(owner_pool: PgPool) {
    let actor = seed_org(&owner_pool, ORG, "preopening").await;
    seed_structure(&owner_pool).await;
    let org = OrgId::from_uuid(ORG);
    let port = PgEmploymentPort::new(
        runtime_role_pool(&owner_pool).await,
        tokio::runtime::Handle::current(),
    );
    let db_now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let opened = db_now - time::Duration::days(1);
    let revision_from = opened - time::Duration::days(1);
    let employee = seed_employee(&owner_pool, ORG, "preopening-only-revision").await;
    let employment_id: Uuid = sqlx::query_scalar(
        "INSERT INTO employment_heads (org_id, valid_from) VALUES ($1, $2) RETURNING id",
    )
    .bind(ORG)
    .bind(opened)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO employment_source_bindings \
         (org_id, employee_id, employment_id, actor_id, payload_digest) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(ORG)
    .bind(employee)
    .bind(employment_id)
    .bind(*actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO employment_revisions \
         (org_id, employment_id, version, command_id, actor_id, payload_digest, \
          valid_from, attributes, receipt) \
         VALUES ($1, $2, 1, $3, $4, $5, $6, $7, '{}'::jsonb)",
    )
    .bind(ORG)
    .bind(employment_id)
    .bind(Uuid::new_v4())
    .bind(*actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .bind(revision_from)
    .bind(attributes("ACME", Some(SALES), Some(STAFF), "ACTIVE").to_json())
    .execute(&owner_pool)
    .await
    .unwrap();

    let (revision_count, earliest): (i64, OffsetDateTime) = sqlx::query_as(
        "SELECT count(*)::bigint, min(valid_from) FROM employment_revisions \
         WHERE org_id = $1 AND employment_id = $2",
    )
    .bind(ORG)
    .bind(employment_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(revision_count, 1, "fixture has exactly one revision");
    assert!(earliest < opened, "the sole revision predates opening");
    let as_of: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert!(as_of > opened, "read occurs after opening");

    let typed_as_of = effective(&port, org, employee, as_of).await;
    let typed_current = current(&port, org, employee).await;
    let detail_port = port.clone();
    let detail = tokio::task::spawn_blocking(move || detail_port.get(org, employment_id))
        .await
        .unwrap();
    let as_of_port = port.clone();
    let as_of_detail =
        tokio::task::spawn_blocking(move || as_of_port.get_as_of(org, employment_id, as_of))
            .await
            .unwrap();
    let list_port = port.clone();
    let listed = tokio::task::spawn_blocking(move || list_port.list(org))
        .await
        .unwrap();

    let mut escaped = Vec::new();
    if !matches!(&typed_as_of, Err(EmploymentError::UnreadableEffectiveState { employment_id: id, .. }) if *id == employment_id)
    {
        escaped.push(format!("typed as-of: {typed_as_of:?}"));
    }
    if !matches!(&typed_current, Err(EmploymentError::UnreadableEffectiveState { employment_id: id, .. }) if *id == employment_id)
    {
        escaped.push(format!("typed current: {typed_current:?}"));
    }
    if !matches!(&detail, Err(EmploymentError::UnreadableEffectiveState { employment_id: id, .. }) if *id == employment_id)
    {
        escaped.push(format!("canonical get: {detail:?}"));
    }
    if !matches!(&as_of_detail, Err(EmploymentError::UnreadableEffectiveState { employment_id: id, .. }) if *id == employment_id)
    {
        escaped.push(format!("canonical get_as_of: {as_of_detail:?}"));
    }
    if !matches!(&listed, Err(EmploymentError::UnreadableEffectiveState { employment_id: id, .. }) if *id == employment_id)
    {
        escaped.push(format!("canonical list: {listed:?}"));
    }
    assert!(escaped.is_empty(), "{}", escaped.join("\n"));
}
