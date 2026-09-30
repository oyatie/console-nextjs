#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Real `console_rt` coverage for People & Workforce employee creation.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const EMPLOYEES_PATH: &str = "/api/v1/employees";
const HR_READINESS_SUMMARY_PATH: &str = "/api/v1/hr/readiness-summary";

struct Keys {
    private_pem: String,
    public_pem: String,
}
struct JsonResponse {
    status: StatusCode,
    json: Value,
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn readiness_counts_only_inspectable_active_payroll_close_statuses(pool: PgPool) {
    let keys = keys();
    let org = OrgId::knl();
    let user = UserId::new();
    seed_user(&pool, org, user).await;

    for (source_label, status, calculation_enabled) in [
        ("terminal-issued", "ISSUED", true),
        ("terminal-void", "VOID", false),
    ] {
        sqlx::query(
            "INSERT INTO payroll_draft_runs \
             (id, org_id, period_start, period_end, source_label, status, calculation_enabled, created_by) \
             VALUES ($1, $2, '2026-07-01', '2026-07-31', $3, $4, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(*org.as_uuid())
        .bind(source_label)
        .bind(status)
        .bind(calculation_enabled)
        .bind(*user.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    }

    let service = build_router(
        app_state(
            runtime_role_pool(&pool).await,
            leave_command_role_pool(&pool).await,
            keys.public_pem.clone(),
        )
        .unwrap(),
    );
    let token = bearer(&keys, org, user, &["SUPER_ADMIN"]);

    let terminal_only = get(service.clone(), HR_READINESS_SUMMARY_PATH, &token).await;
    assert_eq!(
        terminal_only.status,
        StatusCode::OK,
        "{:?}",
        terminal_only.json
    );
    assert_eq!(terminal_only.json["payroll"]["draft_runs"], 2);
    assert_eq!(terminal_only.json["payroll"]["active_close_runs"], 0);

    for (source_label, status, calculation_enabled) in [
        ("active-staged", "STAGED", false),
        ("active-blocked", "BLOCKED_LEGAL_GATE", false),
        ("active-ready", "READY_FOR_REVIEW", true),
        ("active-approved", "APPROVED", true),
    ] {
        sqlx::query(
            "INSERT INTO payroll_draft_runs \
             (id, org_id, period_start, period_end, source_label, status, calculation_enabled, created_by) \
             VALUES ($1, $2, '2026-08-01', '2026-08-31', $3, $4, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(*org.as_uuid())
        .bind(source_label)
        .bind(status)
        .bind(calculation_enabled)
        .bind(*user.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    }

    let active = get(service, HR_READINESS_SUMMARY_PATH, &token).await;
    assert_eq!(active.status, StatusCode::OK, "{:?}", active.json);
    assert_eq!(active.json["payroll"]["active_close_runs"], 4);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn employee_create_is_idempotent_unique_and_tenant_scoped(pool: PgPool) {
    let keys = keys();
    let org = OrgId::knl();
    let user = UserId::new();
    seed_user(&pool, org, user).await;
    let branch = seed_branch(&pool, org, "People test branch").await;
    let service = build_router(
        app_state(
            runtime_role_pool(&pool).await,
            leave_command_role_pool(&pool).await,
            keys.public_pem.clone(),
        )
        .unwrap(),
    );
    let token = bearer(&keys, org, user, &["SUPER_ADMIN"]);
    let body = create_body(branch, "PEOPLE-001", "same-key", "010-1234-5678", "Kim");

    let (first, second) = tokio::join!(
        post(service.clone(), EMPLOYEES_PATH, &token, body.clone()),
        post(service.clone(), EMPLOYEES_PATH, &token, body),
    );
    assert!(
        [first.status, second.status].contains(&StatusCode::CREATED)
            && [first.status, second.status].contains(&StatusCode::OK),
        "same-key requests must produce one 201 and one replayed 200: {:?} / {:?}",
        first.json,
        second.json
    );
    let created = if first.status == StatusCode::CREATED {
        &first.json
    } else {
        &second.json
    };
    let employee_id = created["employee"]["id"].as_str().unwrap();
    assert_eq!(created["employment"]["phone_e164"], "+821012345678");
    assert_employee_create_omits_compensation_pii(created);
    let signoffs: Value = sqlx::query_scalar(
        "SELECT signoffs FROM employee_lifecycle_events WHERE org_id = $1 AND employee_id = $2",
    )
    .bind(*org.as_uuid())
    .bind(Uuid::parse_str(employee_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        signoffs,
        json!({}),
        "employee creation must not fabricate acknowledgements"
    );
    let row_counts: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM employees WHERE org_id = $1), \
         (SELECT count(*) FROM employee_employment_profiles WHERE org_id = $1), \
         (SELECT count(*) FROM employee_lifecycle_events WHERE org_id = $1), \
         (SELECT count(*) FROM employee_create_idempotency WHERE org_id = $1), \
         (SELECT count(*) FROM persons WHERE org_id = $1), \
         (SELECT count(*) FROM employee_person_bindings WHERE org_id = $1)",
    )
    .bind(*org.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row_counts,
        (1, 1, 1, 1, 1, 1),
        "same-key race must write exactly one row in each employee creation table"
    );
    let bound_person: Uuid = sqlx::query_scalar(
        "SELECT person_id FROM employee_person_bindings WHERE org_id = $1 AND employee_id = $2",
    )
    .bind(*org.as_uuid())
    .bind(Uuid::parse_str(employee_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        bound_person,
        Uuid::parse_str(employee_id).unwrap(),
        "trusted create binds person_id = employee_id"
    );

    // 1a: the employee ROW is created through the Employment port and its
    // `employee.create` audit, while the first home-branch routing authority is
    // established post-commit through the leave command channel, which writes
    // its own `employee.home_branch_set` audit.
    let home_branch_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events \
         WHERE org_id = $1 AND action = 'employee.home_branch_set'",
    )
    .bind(*org.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        home_branch_audits, 1,
        "create must establish the first home-branch routing authority through \
         the command channel (exactly one employee.home_branch_set audit)"
    );

    let changed = post(
        service.clone(),
        EMPLOYEES_PATH,
        &token,
        create_body(branch, "PEOPLE-001", "same-key", "010-1234-5678", "Changed"),
    )
    .await;
    assert_eq!(changed.status, StatusCode::CONFLICT, "{:?}", changed.json);

    let duplicate = post(
        service.clone(),
        EMPLOYEES_PATH,
        &token,
        create_body(
            branch,
            "PEOPLE-001",
            "new-key",
            "010-1234-5678",
            "Duplicate",
        ),
    )
    .await;
    assert_eq!(
        duplicate.status,
        StatusCode::CONFLICT,
        "{:?}",
        duplicate.json
    );

    for (employee_number, key, base_pay, expected) in [
        ("PEOPLE-LOWER", "pay-lower", "0", "0.00"),
        (
            "PEOPLE-UPPER",
            "pay-upper",
            "999999999999.99",
            "999999999999.99",
        ),
    ] {
        let mut boundary = create_body(branch, employee_number, key, "010-1234-5678", "Pay bound");
        boundary["base_pay"] = json!(base_pay);
        let response = post(service.clone(), EMPLOYEES_PATH, &token, boundary).await;
        assert_eq!(response.status, StatusCode::CREATED, "{:?}", response.json);
        assert_eq!(response.json["employment"]["base_pay"], expected);
    }
    for (employee_number, key, base_pay) in [
        ("PEOPLE-SCALE", "pay-scale", "1.001"),
        ("PEOPLE-RANGE", "pay-range", "1000000000000"),
        ("PEOPLE-CANON", "pay-canon", "01.00"),
        ("PEOPLE-EXP", "pay-exp", "1e2"),
    ] {
        let mut invalid = create_body(branch, employee_number, key, "010-1234-5678", "Invalid pay");
        invalid["base_pay"] = json!(base_pay);
        let response = post(service.clone(), EMPLOYEES_PATH, &token, invalid).await;
        assert_eq!(
            response.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{:?}",
            response.json
        );
    }

    let executive = UserId::new();
    seed_user_with_roles(&pool, org, executive, &["EXECUTIVE"]).await;
    let before_executive_denial = people_write_counts(&pool, org).await;
    let executive_denied = post(
        service.clone(),
        EMPLOYEES_PATH,
        &bearer(&keys, org, executive, &["EXECUTIVE"]),
        create_body(
            branch,
            "PEOPLE-EXEC",
            "executive-key",
            "010-1234-5678",
            "Executive",
        ),
    )
    .await;
    assert_eq!(
        executive_denied.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        executive_denied.json
    );
    assert_eq!(
        people_write_counts(&pool, org).await,
        before_executive_denial
    );
    assert_no_employee_writes(&pool, org, "PEOPLE-EXEC", "executive-key").await;

    let custom_grantee = UserId::new();
    seed_user_with_roles(&pool, org, custom_grantee, &["EXECUTIVE"]).await;
    seed_manage_grant(&pool, org, custom_grantee, None, None).await;
    // HIGH-1: an EXECUTIVE with an org-wide employee_directory_manage grant is a
    // legitimate creator (base behavior 201, which this lane must NOT narrow).
    // The create recheck and the first-home-branch routing now share the SAME
    // assert_employee_directory_manager predicate, so this succeeds instead of
    // 403ing an orphan after the row already committed.
    let custom_created = post(
        service.clone(),
        EMPLOYEES_PATH,
        &bearer(&keys, org, custom_grantee, &["EXECUTIVE"]),
        create_body(
            branch,
            "PEOPLE-CUSTOM",
            "custom-key",
            "010-1234-5678",
            "Custom",
        ),
    )
    .await;
    assert_eq!(
        custom_created.status,
        StatusCode::CREATED,
        "{:?}",
        custom_created.json
    );
    assert_eq!(
        custom_created.json["employee"]["home_branch_id"],
        json!(branch.to_string()),
        "a custom-grant EXECUTIVE must establish the first home-branch routing authority"
    );
    let matching_team_grantee = UserId::new();
    seed_user_with_roles(&pool, org, matching_team_grantee, &["EXECUTIVE"]).await;
    set_user_team(&pool, org, matching_team_grantee, "정비").await;
    seed_manage_grant(&pool, org, matching_team_grantee, None, Some("MAINTENANCE")).await;
    let matching_team_created = post(
        service.clone(),
        EMPLOYEES_PATH,
        &bearer(&keys, org, matching_team_grantee, &["EXECUTIVE"]),
        create_body(
            branch,
            "PEOPLE-TEAM-MATCH",
            "team-match-key",
            "010-1234-5678",
            "Matching team",
        ),
    )
    .await;
    assert_eq!(
        matching_team_created.status,
        StatusCode::CREATED,
        "{:?}",
        matching_team_created.json
    );
    let before_team_denial = people_write_counts(&pool, org).await;

    let mismatched_team_grantee = UserId::new();
    seed_user_with_roles(&pool, org, mismatched_team_grantee, &["EXECUTIVE"]).await;
    set_user_team(&pool, org, mismatched_team_grantee, "정비").await;
    seed_manage_grant(
        &pool,
        org,
        mismatched_team_grantee,
        None,
        Some("PREVENTION"),
    )
    .await;
    let mismatched_team_denied = post(
        service.clone(),
        EMPLOYEES_PATH,
        &bearer(&keys, org, mismatched_team_grantee, &["EXECUTIVE"]),
        create_body(
            branch,
            "PEOPLE-TEAM-MISMATCH",
            "team-mismatch-key",
            "010-1234-5678",
            "Mismatched team",
        ),
    )
    .await;
    assert_eq!(
        mismatched_team_denied.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        mismatched_team_denied.json
    );
    assert_eq!(people_write_counts(&pool, org).await, before_team_denial);
    assert_no_employee_writes(&pool, org, "PEOPLE-TEAM-MISMATCH", "team-mismatch-key").await;

    let denied_user = UserId::new();
    seed_user_with_roles(&pool, org, denied_user, &["MEMBER"]).await;
    let before_member_denial = people_write_counts(&pool, org).await;
    let denied_create = post(
        service.clone(),
        EMPLOYEES_PATH,
        &bearer(&keys, org, denied_user, &["MEMBER"]),
        create_body(
            branch,
            "PEOPLE-DENIED",
            "denied-key",
            "010-1234-5678",
            "Denied",
        ),
    )
    .await;
    assert_eq!(
        denied_create.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        denied_create.json
    );
    assert_eq!(
        people_write_counts(&pool, org).await,
        before_member_denial,
        "a MEMBER must write no employee row, profile, lifecycle event, \
         idempotency reservation, or employee.create audit"
    );
    assert_no_employee_writes(&pool, org, "PEOPLE-DENIED", "denied-key").await;
    let before_denials = people_write_counts(&pool, org).await;

    let branch_grantee = UserId::new();
    seed_user_with_roles(&pool, org, branch_grantee, &["EXECUTIVE"]).await;
    seed_manage_grant(&pool, org, branch_grantee, Some(branch), None).await;
    let branch_denied = post(
        service.clone(),
        EMPLOYEES_PATH,
        &bearer(&keys, org, branch_grantee, &["EXECUTIVE"]),
        create_body(
            branch,
            "PEOPLE-BRANCH",
            "branch-key",
            "010-1234-5678",
            "Branch",
        ),
    )
    .await;
    assert_eq!(
        branch_denied.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        branch_denied.json
    );
    assert_eq!(people_write_counts(&pool, org).await, before_denials);
    assert_no_employee_writes(&pool, org, "PEOPLE-BRANCH", "branch-key").await;

    let admin = UserId::new();
    seed_user_with_roles(&pool, org, admin, &["ADMIN"]).await;
    let admin_denied = post(
        service.clone(),
        EMPLOYEES_PATH,
        &bearer(&keys, org, admin, &["ADMIN"]),
        create_body(
            branch,
            "PEOPLE-ADMIN",
            "admin-key",
            "010-1234-5678",
            "Admin",
        ),
    )
    .await;
    assert_eq!(
        admin_denied.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        admin_denied.json
    );
    assert_eq!(people_write_counts(&pool, org).await, before_denials);
    assert_no_employee_writes(&pool, org, "PEOPLE-ADMIN", "admin-key").await;

    let other_org = OrgId::from_uuid(Uuid::new_v4());
    seed_org(&pool, other_org).await;
    let other_user = UserId::new();
    seed_user(&pool, other_org, other_user).await;
    let other_branch = seed_branch(&pool, other_org, "People other-org branch").await;
    let other_token = bearer(&keys, other_org, other_user, &["SUPER_ADMIN"]);

    // Cross-tenant read is 404 (detail) or empty omit (directory), never a
    // count or object-existence leak of org A's employee.
    let denied = get(
        service.clone(),
        &format!("{EMPLOYEES_PATH}/{employee_id}"),
        &other_token,
    )
    .await;
    assert_eq!(
        denied.status,
        StatusCode::NOT_FOUND,
        "cross-org detail must not be visible: {:?}",
        denied.json
    );
    assert!(
        denied.json.get("employee").is_none(),
        "cross-org 404 must omit the employee object: {:?}",
        denied.json
    );
    let omitted = get(service.clone(), EMPLOYEES_PATH, &other_token).await;
    assert_eq!(omitted.status, StatusCode::OK, "{:?}", omitted.json);
    assert_eq!(
        omitted.json["total"], 0,
        "cross-org directory must omit the other tenant: {:?}",
        omitted.json
    );
    assert_eq!(omitted.json["items"], json!([]));

    let org_a_before_foreign_create = people_write_counts(&pool, org).await;
    let foreign_create = post(
        service.clone(),
        EMPLOYEES_PATH,
        &other_token,
        create_body(
            other_branch,
            "PEOPLE-001",
            "same-key",
            "010-1234-5678",
            "Other tenant",
        ),
    )
    .await;
    assert_eq!(
        foreign_create.status,
        StatusCode::CREATED,
        "{:?}",
        foreign_create.json
    );
    assert_employee_create_omits_compensation_pii(&foreign_create.json);
    let foreign_employee_id = foreign_create.json["employee"]["id"].as_str().unwrap();
    assert_ne!(
        foreign_employee_id, employee_id,
        "same idempotency key in another org must not collide into org A's employee"
    );
    assert_eq!(
        people_write_counts(&pool, org).await,
        org_a_before_foreign_create,
        "another org's same-key create must not write org A's employee tables"
    );
    assert_eq!(people_write_counts(&pool, other_org).await, (1, 1, 1, 1, 1));
    let org_a_same_key: Uuid = sqlx::query_scalar(
        "SELECT employee_id FROM employee_create_idempotency \
         WHERE org_id = $1 AND idempotency_key = $2",
    )
    .bind(*org.as_uuid())
    .bind("same-key")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        org_a_same_key.to_string(),
        employee_id,
        "org B reusing same-key must not retarget org A's idempotency row"
    );

    let still_denied = get(
        service.clone(),
        &format!("{EMPLOYEES_PATH}/{employee_id}"),
        &other_token,
    )
    .await;
    assert_eq!(
        still_denied.status,
        StatusCode::NOT_FOUND,
        "cross-org detail must stay omitted after the other tenant creates: {:?}",
        still_denied.json
    );
    let foreign_list = get(service.clone(), EMPLOYEES_PATH, &other_token).await;
    assert_eq!(
        foreign_list.status,
        StatusCode::OK,
        "{:?}",
        foreign_list.json
    );
    let listed_ids: Vec<&str> = foreign_list.json["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["id"].as_str())
        .collect();
    assert!(
        !listed_ids.contains(&employee_id),
        "cross-org directory must omit org A's employee: {:?}",
        foreign_list.json
    );
    assert!(
        listed_ids.contains(&foreign_employee_id),
        "org B directory must contain only org B's employee: {:?}",
        foreign_list.json
    );

    let other_way = get(
        service,
        &format!("{EMPLOYEES_PATH}/{foreign_employee_id}"),
        &token,
    )
    .await;
    assert_eq!(
        other_way.status,
        StatusCode::NOT_FOUND,
        "org A must not read org B's employee: {:?}",
        other_way.json
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn employee_create_rejects_unknown_org_unit_uuid(pool: PgPool) {
    let keys = keys();
    let org = OrgId::knl();
    let user = UserId::new();
    seed_user(&pool, org, user).await;
    let branch = seed_branch(&pool, org, "People uuid-ref branch").await;
    let service = build_router(
        app_state(
            runtime_role_pool(&pool).await,
            leave_command_role_pool(&pool).await,
            keys.public_pem.clone(),
        )
        .unwrap(),
    );
    let token = bearer(&keys, org, user, &["SUPER_ADMIN"]);
    let mut body = create_body(
        branch,
        "PEOPLE-UUID-ORG",
        "uuid-org-key",
        "010-1234-5678",
        "UuidOrg",
    );
    body["org_unit"] = json!(Uuid::new_v4().to_string());
    let refused = post(service, EMPLOYEES_PATH, &token, body).await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        refused.json
    );
    let employees: i64 = sqlx::query_scalar("SELECT count(*) FROM employees WHERE org_id = $1")
        .bind(*org.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        employees, 0,
        "unknown OrgUnit UUID must not insert an employee"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn deactivated_super_admin_cannot_create_employee(pool: PgPool) {
    let keys = keys();
    let org = OrgId::knl();
    let user = UserId::new();
    seed_user(&pool, org, user).await;
    let branch = seed_branch(&pool, org, "People deactivated-admin branch").await;
    let service = build_router(
        app_state(
            runtime_role_pool(&pool).await,
            leave_command_role_pool(&pool).await,
            keys.public_pem.clone(),
        )
        .unwrap(),
    );
    // Mint the token FIRST, then deactivate: the token's SUPER_ADMIN roles stay
    // valid until expiry, so the live write-boundary recheck in
    // `create_employee_core` is the only thing that must refuse this actor.
    let token = bearer(&keys, org, user, &["SUPER_ADMIN"]);
    sqlx::query("UPDATE users SET is_active = false WHERE id = $1 AND org_id = $2")
        .bind(*user.as_uuid())
        .bind(*org.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    let before = people_write_counts(&pool, org).await;
    let response = post(
        service.clone(),
        EMPLOYEES_PATH,
        &token,
        create_body(
            branch,
            "PEOPLE-DEACT",
            "deactivated-key",
            "010-1234-5678",
            "Deactivated",
        ),
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        response.json
    );
    assert_eq!(
        people_write_counts(&pool, org).await,
        before,
        "a deactivated SUPER_ADMIN must write no employee row, employment profile, \
         lifecycle event, idempotency reservation, or employee.create audit"
    );
    assert_no_employee_writes(&pool, org, "PEOPLE-DEACT", "deactivated-key").await;

    // Same still-valid token, but now the actor is active again while the LIVE
    // SUPER_ADMIN role has been revoked: the write-boundary recheck must refuse
    // the token-only role, not just `is_active`.
    sqlx::query(
        "UPDATE users SET is_active = true, roles = '{}'::text[] WHERE id = $1 AND org_id = $2",
    )
    .bind(*user.as_uuid())
    .bind(*org.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let before_role_revocation = people_write_counts(&pool, org).await;
    let revoked = post(
        service.clone(),
        EMPLOYEES_PATH,
        &token,
        create_body(
            branch,
            "PEOPLE-ROLE-REVOKED",
            "role-revoked-key",
            "010-1234-5678",
            "Role revoked",
        ),
    )
    .await;
    assert_eq!(revoked.status, StatusCode::FORBIDDEN, "{:?}", revoked.json);
    assert_eq!(
        people_write_counts(&pool, org).await,
        before_role_revocation,
        "a role-revoked SUPER_ADMIN must write no employee row, employment profile, \
         lifecycle event, idempotency reservation, or employee.create audit"
    );
    assert_no_employee_writes(&pool, org, "PEOPLE-ROLE-REVOKED", "role-revoked-key").await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn revoked_grant_executive_cannot_create_employee(pool: PgPool) {
    let keys = keys();
    let org = OrgId::knl();
    let branch = seed_branch(&pool, org, "People revoked-grant branch").await;
    let executive = UserId::new();
    seed_user_with_roles(&pool, org, executive, &["EXECUTIVE"]).await;
    seed_manage_grant(&pool, org, executive, None, None).await;
    let service = build_router(
        app_state(
            runtime_role_pool(&pool).await,
            leave_command_role_pool(&pool).await,
            keys.public_pem.clone(),
        )
        .unwrap(),
    );
    let token = bearer(&keys, org, executive, &["EXECUTIVE"]);

    // Revoke the org-wide grant AFTER the actor was seeded and the token minted:
    // the live write-boundary recheck (and the request-time grant resolution)
    // must refuse the token-only EXECUTIVE, not trust the formerly-present grant.
    sqlx::query("DELETE FROM user_role_assignments WHERE org_id = $1 AND user_id = $2")
        .bind(*org.as_uuid())
        .bind(*executive.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    let before = people_write_counts(&pool, org).await;
    let response = post(
        service.clone(),
        EMPLOYEES_PATH,
        &token,
        create_body(
            branch,
            "PEOPLE-GRANT-REVOKED",
            "grant-revoked-key",
            "010-1234-5678",
            "Grant revoked",
        ),
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        response.json
    );
    assert_eq!(
        people_write_counts(&pool, org).await,
        before,
        "a grant-revoked EXECUTIVE must write no employee row, employment profile, \
         lifecycle event, idempotency reservation, or employee.create audit"
    );
    assert_no_employee_writes(&pool, org, "PEOPLE-GRANT-REVOKED", "grant-revoked-key").await;
}

fn assert_employee_create_omits_compensation_pii(body: &Value) {
    // Canonical import is the 13-field set (name, employee_number, org_unit,
    // job, position, worksite_name, worksite_address, hire_date, exit_date,
    // leave_accrued, leave_used, leave_remaining, company). Compensation /
    // unique-id keys that are not in that set must not appear on employee.
    // Production employment returns phone_e164 / base_pay — do not treat those
    // as leaks.
    if let Some(employee) = body.get("employee") {
        for key in ["bank_account", "rrn", "salary"] {
            assert!(
                employee.get(key).is_none(),
                "create employee must not leak {key}: {employee:?}"
            );
        }
    }
}

fn create_body(branch: Uuid, employee_number: &str, key: &str, phone: &str, name: &str) -> Value {
    json!({
        "employee_number": employee_number,
        "name": name,
        "company": "테스트 법인",
        "employment_type": "REGULAR",
        "phone": phone,
        "org_unit": "인사",
        "position": "사원",
        "site": "서울",
        "home_branch_id": branch,
        "base_pay": "50000000",
        "idempotency_key": key,
    })
}

async fn seed_org(pool: &PgPool, org: OrgId) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(*org.as_uuid())
        .bind(format!(
            "people-{}",
            &org.as_uuid().simple().to_string()[..12]
        ))
        .bind("People test org")
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_user(pool: &PgPool, org: OrgId, user: UserId) {
    seed_user_with_roles(pool, org, user, &["SUPER_ADMIN"]).await;
}

async fn seed_user_with_roles(pool: &PgPool, org: OrgId, user: UserId, roles: &[&str]) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user.as_uuid())
        .bind("People super administrator")
        .bind(roles)
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn set_user_team(pool: &PgPool, org: OrgId, user: UserId, team: &str) {
    sqlx::query("UPDATE users SET team = $1 WHERE id = $2 AND org_id = $3")
        .bind(team)
        .bind(*user.as_uuid())
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_manage_grant(
    pool: &PgPool,
    org: OrgId,
    user: UserId,
    branch: Option<Uuid>,
    team: Option<&str>,
) {
    let role_id: Uuid = sqlx::query_scalar(
        "INSERT INTO policy_roles (org_id, role_key, display_name, status) \
         VALUES ($1, $2, $3, 'ACTIVE') RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(format!("people_manage_{}", Uuid::new_v4().simple()))
    .bind("People manager")
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO policy_role_permissions (org_id, role_id, feature_key, permission_level) \
         VALUES ($1, $2, 'employee_directory_manage', 'allow')",
    )
    .bind(*org.as_uuid())
    .bind(role_id)
    .execute(pool)
    .await
    .unwrap();
    if let Some(branch) = branch {
        sqlx::query(
            "INSERT INTO policy_role_conditions \
             (org_id, role_id, condition_key, attribute, operator, condition_values) \
             VALUES ($1, $2, 'branch_scope', 'branch', 'equals', ARRAY[$3::text])",
        )
        .bind(*org.as_uuid())
        .bind(role_id)
        .bind(branch)
        .execute(pool)
        .await
        .unwrap();
    }
    if let Some(team) = team {
        sqlx::query(
            "INSERT INTO policy_role_conditions \
             (org_id, role_id, condition_key, attribute, operator, condition_values) \
             VALUES ($1, $2, 'team_scope', 'team', 'equals', ARRAY[$3::text])",
        )
        .bind(*org.as_uuid())
        .bind(role_id)
        .bind(team)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO user_role_assignments (org_id, user_id, role_id) VALUES ($1, $2, $3)")
        .bind(*org.as_uuid())
        .bind(*user.as_uuid())
        .bind(role_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn people_write_counts(pool: &PgPool, org: OrgId) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM employees WHERE org_id = $1), \
         (SELECT count(*) FROM employee_employment_profiles WHERE org_id = $1), \
         (SELECT count(*) FROM employee_lifecycle_events WHERE org_id = $1), \
         (SELECT count(*) FROM employee_create_idempotency WHERE org_id = $1), \
         (SELECT count(*) FROM audit_events WHERE org_id = $1 AND action = 'employee.create')",
    )
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// MEDIUM-3 mutation-test-style guard: a denied create must leave BOTH no
/// employee row for the attempted `employee_number` AND no idempotency
/// reservation for the attempted key. Because the live authority recheck in
/// `create_employee_core` runs BEFORE the idempotency INSERT, this assertion
/// goes red if anyone reorders the write ahead of the authority check.
async fn assert_no_employee_writes(pool: &PgPool, org: OrgId, employee_number: &str, key: &str) {
    let (employees, reservations): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM employees WHERE org_id = $1 AND employee_number = $2), \
         (SELECT count(*) FROM employee_create_idempotency WHERE org_id = $1 AND idempotency_key = $3)",
    )
    .bind(*org.as_uuid())
    .bind(employee_number)
    .bind(key)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        (employees, reservations),
        (0, 0),
        "a denied create must leave no employee row or idempotency reservation for the attempted key"
    );
}

async fn seed_branch(pool: &PgPool, org: OrgId, name: &str) -> Uuid {
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{name} region"))
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region)
    .bind(name)
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn post(service: axum::Router, uri: &str, token: &str, body: Value) -> JsonResponse {
    send(service, "POST", uri, token, Some(body)).await
}
async fn get(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    send(service, "GET", uri, token, None).await
}

async fn send(
    service: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> JsonResponse {
    let mut builder = Request::builder()
        .uri(uri)
        .method(method)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let request = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        builder.body(Body::from(body.to_string())).unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    JsonResponse {
        status,
        json: serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({})),
    }
}

fn keys() -> Keys {
    let signing_key = SigningKey::random(&mut OsRng);
    Keys {
        private_pem: signing_key
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string(),
        public_pem: signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    }
}

fn bearer(keys: &Keys, org: OrgId, user: UserId, roles: &[&str]) -> String {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
    )
    .unwrap();
    issuer
        .issue_access_token(AccessTokenInput {
            subject: user,
            org_id: org,
            roles: roles.iter().map(|role| (*role).to_owned()).collect(),
            branches: Vec::new(),
            platform: false,
            view_as: false,
            read_only: false,
            display_name: None,
            feature_grants: Vec::new(),
            authz_subject_version: 0,
            authz_policy_version: 0,
            session_generation: 0,
            issued_at: OffsetDateTime::now_utc(),
        })
        .unwrap()
}

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    scoped_role_pool(owner_pool, "console_rt").await
}

async fn leave_command_role_pool(owner_pool: &PgPool) -> PgPool {
    scoped_role_pool(owner_pool, "console_leave_cmd").await
}

async fn scoped_role_pool(owner_pool: &PgPool, role: &'static str) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _| {
            Box::pin(async move {
                match role {
                    "console_rt" => sqlx::query("SET ROLE console_rt").execute(conn).await?,
                    "console_leave_cmd" => {
                        sqlx::query("SET ROLE console_leave_cmd")
                            .execute(conn)
                            .await?
                    }
                    _ => unreachable!("test role is fixed by its helper"),
                };
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

fn app_state(
    runtime_pool: PgPool,
    leave_command_pool: PgPool,
    public_key_pem: String,
) -> Result<AppState, console_app::AppError> {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
    ])?;
    AppState::new(config, DatabaseDependency::Postgres(runtime_pool))
        .map(|state| state.with_leave_command_database(leave_command_pool))
}
