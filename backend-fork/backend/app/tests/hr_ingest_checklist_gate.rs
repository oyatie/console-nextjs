#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! §16 ingest-commit gate (85 판정, BE-ingest-checklist-gates) —
//! `POST /api/v1/hr/attendance-import/{run_id}/apply` ("적재"/commit) over the
//! REAL router on a genuine non-owner `console_rt` pool (RLS enforced), JWT-authed.
//!
//! Proves:
//!   * apply WITHOUT `checklist_all_acknowledged` (or with `false`) denies
//!     (403) BEFORE the run row is even read — fail-closed, zero rows written,
//!     the run stays `DRY_RUN`;
//!   * apply WITH `checklist_all_acknowledged: true` passes the gate and the
//!     real mutation runs (run flips to `APPLIED`), and the audit trail carries
//!     the gate outcome.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const SOURCE_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ATTENDANCE_CSV: &str = "\n사번,성명,지점,근무일,出勤,출근시간,퇴근시간,근무분\nE-001,홍길동,본사,2026-07-01,메모,09:00,18:00,540\n\nE-001,홍길동,본사,2026-07-02,메모,09:00,18:00,540\n";

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

// ===========================================================================
// Deny path: no checklist evidence ⇒ 403, before the run row is even touched.
// Nothing is written — the run (seeded DRY_RUN) is untouched, zero events.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn apply_without_checklist_denies_and_writes_nothing(owner_pool: PgPool) {
    let keys = keys();
    let admin = UserId::new();
    seed_admin(&owner_pool, admin).await;
    let run_id = seed_dry_run(&owner_pool, "checklist-deny").await;

    let service = build_router(
        app_state(
            runtime_role_pool(&owner_pool).await,
            keys.public_pem.clone(),
        )
        .unwrap(),
    );
    let token = bearer(&owner_pool, &keys, admin).await;

    // No body at all (no Content-Type ⇒ `checklist_all_acknowledged: None`).
    let denied = send(
        service.clone(),
        &format!("/api/v1/hr/attendance-import/{run_id}/apply"),
        &token,
        None,
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);
    assert!(
        denied.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("checklist"),
        "must deny for the checklist gate specifically, not some other 403: {:?}",
        denied.json
    );

    // Explicit `false` must deny identically (not just "missing").
    let denied_explicit = send(
        service.clone(),
        &format!("/api/v1/hr/attendance-import/{run_id}/apply"),
        &token,
        Some(json!({ "checklist_all_acknowledged": false })),
    )
    .await;
    assert_eq!(
        denied_explicit.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        denied_explicit.json
    );

    let status: String = sqlx::query_scalar("SELECT status FROM data_import_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(status, "DRY_RUN", "a denied gate must not flip the run");
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM attendance_direct_import_events WHERE run_id = $1",
    )
    .bind(run_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(events, 0, "a denied gate must write zero rows");
}

// ===========================================================================
// Allow path: checklist acknowledged ⇒ the real mutation runs (run flips to
// APPLIED) and the audit event carries the gate outcome.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn apply_with_checklist_admits_and_applies(owner_pool: PgPool) {
    let keys = keys();
    let admin = UserId::new();
    seed_admin(&owner_pool, admin).await;
    let run_id = seed_dry_run(&owner_pool, "checklist-allow").await;

    let service = build_router(
        app_state(
            runtime_role_pool(&owner_pool).await,
            keys.public_pem.clone(),
        )
        .unwrap(),
    );
    let token = bearer(&owner_pool, &keys, admin).await;

    let applied = send(
        service,
        &format!("/api/v1/hr/attendance-import/{run_id}/apply"),
        &token,
        Some(json!({ "checklist_all_acknowledged": true })),
    )
    .await;
    assert_eq!(applied.status, StatusCode::OK, "{:?}", applied.json);

    let status: String = sqlx::query_scalar("SELECT status FROM data_import_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(status, "APPLIED", "an admitted gate must apply the run");

    let gate_outcome: Value = sqlx::query_scalar(
        "SELECT after_snap -> 'gate_outcome' FROM audit_events \
         WHERE target_type = 'data_import_run' AND target_id = $1 \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(run_id.to_string())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        gate_outcome["allow"],
        Value::Bool(true),
        "the audit row must carry the passing gate outcome: {gate_outcome:?}"
    );
}

// ===========================================================================
// Helpers.
// ===========================================================================

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn attendance_csv_source_coordinates_survive_preview_and_apply(pool: PgPool) {
    assert_source_coordinates(
        &pool,
        "attendance.csv",
        ATTENDANCE_CSV.as_bytes(),
        "CSV",
        &[3, 5],
    )
    .await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn attendance_xlsx_source_coordinates_survive_preview_and_apply(pool: PgPool) {
    // One sheet: C5:I8; header row 5, data rows 6/8, blank row 7.
    assert_source_coordinates(
        &pool,
        "attendance.xlsx",
        include_bytes!("fixtures/attendance-offset.xlsx"),
        "근태",
        &[6, 8],
    )
    .await;
}

async fn assert_source_coordinates(
    pool: &PgPool,
    filename: &str,
    bytes: &[u8],
    sheet: &str,
    expected: &[i32],
) {
    let keys = keys();
    let admin = UserId::new();
    seed_admin(pool, admin).await;
    seed_attendance_subject(pool).await;
    let runtime = runtime_role_pool(pool).await;
    let bypass: bool =
        sqlx::query_scalar("SELECT rolbypassrls FROM pg_roles WHERE rolname = current_user")
            .fetch_one(&runtime)
            .await
            .unwrap();
    assert!(!bypass);
    let service = build_router(app_state(runtime, keys.public_pem.clone()).unwrap());
    let token = bearer(&pool, &keys, admin).await;
    let initial = preview(service.clone(), &token, filename, bytes).await;
    assert_eq!(initial.status, StatusCode::OK, "{:?}", initial.json);
    assert_eq!(initial.json["input_rows"], 2);
    assert_eq!(initial.json["candidate_rows"], 2);
    assert_eq!(initial.json["preserved_rows"], 0);
    assert_eq!(
        initial.json["mapping_profile"]["policy"]["payroll_effect"],
        "lineage_only_not_payable"
    );
    for (index, source_row) in expected.iter().enumerate() {
        assert_eq!(
            initial.json["sample_rows"][index]["source_row"],
            *source_row
        );
    }
    let run_id = Uuid::parse_str(initial.json["run_id"].as_str().unwrap()).unwrap();
    let source_hash = hex::encode(Sha256::digest(bytes));
    assert_eq!(initial.json["source_sha256"], source_hash);
    let stored: Vec<(i32, String, Value)> = sqlx::query_as(
        "SELECT source_row, source_key, canonical_row FROM data_import_rows WHERE run_id=$1 ORDER BY source_row"
    ).bind(run_id).fetch_all(pool).await.unwrap();
    assert_eq!(stored.len(), expected.len());
    for ((row, key, canonical), expected_row) in stored.iter().zip(expected) {
        assert_eq!(row, expected_row);
        assert_eq!(key, &format!("sheet:{sheet}|row:{expected_row}"));
        assert_eq!(canonical["source_row"], *expected_row);
        assert_eq!(canonical["source_key"], *key);
        assert_eq!(canonical["source_sha256"], source_hash);
    }

    let foreign_org = OrgId::from_uuid(Uuid::new_v4());
    let foreign_admin = console_platform_test_support::seed_org_and_super_admin(
        pool,
        *foreign_org.as_uuid(),
        "Foreign import",
    )
    .await;
    let foreign_token = bearer_in_org(pool, &keys, foreign_admin, foreign_org).await;
    for action in ["dry-run", "apply"] {
        let denied = send(
            service.clone(),
            &format!("/api/v1/hr/attendance-import/{run_id}/{action}"),
            &foreign_token,
            Some(json!({"checklist_all_acknowledged": true})),
        )
        .await;
        assert_eq!(denied.status, StatusCode::NOT_FOUND, "{:?}", denied.json);
    }
    let unchanged: String = sqlx::query_scalar("SELECT status FROM data_import_runs WHERE id=$1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(unchanged, "PREVIEWED");
    // Identical source bytes have their own run and row custody in Company B.
    let foreign_preview = preview(service.clone(), &foreign_token, filename, bytes).await;
    assert_eq!(
        foreign_preview.status,
        StatusCode::OK,
        "{:?}",
        foreign_preview.json
    );
    assert_ne!(foreign_preview.json["run_id"], run_id.to_string());
    assert_eq!(foreign_preview.json["source_sha256"], source_hash);

    let dry = send(
        service.clone(),
        &format!("/api/v1/hr/attendance-import/{run_id}/dry-run"),
        &token,
        None,
    )
    .await;
    assert_eq!(dry.status, StatusCode::OK, "{:?}", dry.json);
    assert_eq!(dry.json["ready_rows"], 2);
    assert_eq!(dry.json["error_rows"], 0);
    let applied = send(
        service.clone(),
        &format!("/api/v1/hr/attendance-import/{run_id}/apply"),
        &token,
        Some(json!({"checklist_all_acknowledged": true})),
    )
    .await;
    assert_eq!(applied.status, StatusCode::OK, "{:?}", applied.json);
    assert_eq!(applied.json["inserted"], 2);
    assert_eq!(applied.json["skipped"], 0);
    let events: Vec<(i32, String, String)> = sqlx::query_as(
        "SELECT source_row, source_key, source_sha256 FROM attendance_direct_import_events WHERE run_id=$1 ORDER BY source_row"
    ).bind(run_id).fetch_all(pool).await.unwrap();
    assert_eq!(
        events,
        expected
            .iter()
            .map(|row| (
                *row,
                format!("sheet:{sheet}|row:{row}"),
                source_hash.clone()
            ))
            .collect::<Vec<_>>()
    );
    let again = preview(service.clone(), &token, filename, bytes).await;
    let repeated_id = again.json["run_id"].as_str().unwrap();
    let repeated = send(
        service.clone(),
        &format!("/api/v1/hr/attendance-import/{repeated_id}/dry-run"),
        &token,
        None,
    )
    .await;
    assert_eq!(repeated.status, StatusCode::OK, "{:?}", repeated.json);
    assert_eq!(repeated.json["duplicate_rows"], 2);
    assert_eq!(repeated.json["ready_rows"], 0);
    let blocked = send(
        service,
        &format!("/api/v1/hr/attendance-import/{repeated_id}/apply"),
        &token,
        Some(json!({"checklist_all_acknowledged": true})),
    )
    .await;
    assert_eq!(
        blocked.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        blocked.json
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM attendance_direct_import_events")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn attendance_ambiguous_headers_reject_without_writes(pool: PgPool) {
    let keys = keys();
    let admin = UserId::new();
    seed_admin(&pool, admin).await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&pool, &keys, admin).await;
    for (headers, expected_error) in [
        (
            "사번,사 번,지점,근무일,출근시간",
            "attendance import has duplicate normalized headers",
        ),
        (
            "사번,employee_number,지점,근무일,출근시간",
            "attendance import has multiple columns for the same field",
        ),
        (
            "사번,메모,메 모,지점,근무일,출근시간",
            "attendance import has duplicate normalized headers",
        ),
    ] {
        let csv = format!("{headers}\nE-001,PRIVATE-SENTINEL,본사,2026-07-01,09:00\n");
        let denied = preview(service.clone(), &token, "ambiguous.csv", csv.as_bytes()).await;
        assert_eq!(
            denied.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{:?}",
            denied.json
        );
        assert_eq!(denied.json["error"]["code"], "workbook");
        assert_eq!(denied.json["error"]["message"], expected_error);
        let counts: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM data_import_runs), (SELECT count(*) FROM data_import_rows), \
             (SELECT count(*) FROM attendance_direct_import_events), \
             (SELECT count(*) FROM audit_events WHERE action='attendance_import.preview')"
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(counts, (0, 0, 0, 0));
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn attendance_legacy_coordinate_collision_preserves_evidence(pool: PgPool) {
    let keys = keys();
    let admin = UserId::new();
    seed_admin(&pool, admin).await;
    let (employee, branch) = seed_attendance_subject(&pool).await;
    let hash = hex::encode(Sha256::digest(ATTENDANCE_CSV.as_bytes()));
    let old_run = seed_dry_run(&pool, "legacy-coordinates").await;
    sqlx::query("UPDATE data_import_runs SET source_sha256=$2, status='APPLIED', input_rows=2, candidate_rows=2 WHERE id=$1")
        .bind(old_run).bind(&hash).execute(&pool).await.unwrap();
    // Exact old-parser fixtures: physical CSV rows 3/5 compacted to 2/3.
    for (old_row, date) in [(2, "2026-07-01"), (3, "2026-07-02")] {
        let source_key = format!("sheet:CSV|row:{old_row}");
        let row_id: Uuid = sqlx::query_scalar(
            "INSERT INTO data_import_rows (org_id,run_id,source_sheet,source_row,source_key,row_status,raw_row,canonical_row,validation) \
             VALUES ($1,$2,'CSV',$3,$4,'CANDIDATE','{}',$5,'{}') RETURNING id"
        ).bind(*OrgId::knl().as_uuid()).bind(old_run).bind(old_row).bind(&source_key)
            .bind(json!({"source_row":old_row,"source_key":source_key,"source_sha256":hash,
                "canonical":{"employee_number":"E-001","branch_name":"본사","work_date":date,"check_in_at":"09:00","check_out_at":"18:00","minutes_worked":540}}))
            .fetch_one(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO attendance_direct_import_events \
             (org_id,run_id,import_row_id,employee_id,branch_id,source_sheet,source_row,source_key,source_sha256, \
              fact_key,employee_number,employee_name,branch_name,work_date,check_in_at,check_out_at,minutes_worked) \
             VALUES ($1,$2,$3,$4,$5,'CSV',$6,$7,$8,$9,'E-001','홍길동','본사',$10,'09:00','18:00',540)"
        ).bind(*OrgId::knl().as_uuid()).bind(old_run).bind(row_id).bind(employee).bind(branch)
            .bind(old_row).bind(&source_key).bind(&hash)
            .bind(format!("employee:{employee}|branch:{branch}|date:{date}|in:09:00|out:18:00|minutes:540"))
            .bind(date).execute(&pool).await.unwrap();
    }
    let before = historical_events(&pool, old_run).await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&pool, &keys, admin).await;
    let new = preview(
        service.clone(),
        &token,
        "attendance.csv",
        ATTENDANCE_CSV.as_bytes(),
    )
    .await;
    let id = new.json["run_id"].as_str().unwrap();
    let dry = send(
        service.clone(),
        &format!("/api/v1/hr/attendance-import/{id}/dry-run"),
        &token,
        None,
    )
    .await;
    assert_eq!(dry.status, StatusCode::OK, "{:?}", dry.json);
    assert_eq!(dry.json["ready_rows"], 0);
    assert_eq!(dry.json["error_rows"], 2);
    assert_eq!(dry.json["duplicate_rows"], 1);
    assert_eq!(dry.json["row_errors"][0]["source_row"], 3);
    assert_eq!(
        dry.json["row_errors"][0]["code"],
        "source_coordinate_conflict"
    );
    assert_eq!(
        dry.json["row_errors"][1]["code"],
        "duplicate_attendance_fact"
    );
    let blocked = send(
        service,
        &format!("/api/v1/hr/attendance-import/{id}/apply"),
        &token,
        Some(json!({"checklist_all_acknowledged": true})),
    )
    .await;
    assert_eq!(
        blocked.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        blocked.json
    );
    assert_eq!(historical_events(&pool, old_run).await, before);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM attendance_direct_import_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
}

async fn historical_events(pool: &PgPool, run: Uuid) -> Value {
    sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(e) ORDER BY source_row) FROM attendance_direct_import_events e WHERE run_id=$1")
        .bind(run).fetch_one(pool).await.unwrap()
}

async fn seed_attendance_subject(pool: &PgPool) -> (Uuid, Uuid) {
    let org = *OrgId::knl().as_uuid();
    let region: Uuid = sqlx::query_scalar(
        "INSERT INTO regions (name,org_id) VALUES ('본사 지역',$1) RETURNING id",
    )
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();
    let branch: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id,name,org_id) VALUES ($1,'본사',$2) RETURNING id",
    )
    .bind(region)
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();
    let employee: Uuid = sqlx::query_scalar(
        "INSERT INTO employees (org_id,company,name,employee_number,source_filename,source_sheet,source_row,source_key) \
         VALUES ($1,'KNL','홍길동','E-001','roster.xlsx','사람',2,'attendance-test-subject') RETURNING id"
    ).bind(org).fetch_one(pool).await.unwrap();
    (employee, branch)
}

async fn preview(service: axum::Router, token: &str, filename: &str, bytes: &[u8]) -> JsonResponse {
    let boundary = "attendance-source-test-boundary";
    let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
    body.extend_from_slice(bytes);
    for (name, value) in [
        ("pay_period_start", "2026-07-01"),
        ("pay_period_end", "2026-07-31"),
    ] {
        body.extend_from_slice(
            format!(
                "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let response = service
        .oneshot(
            Request::builder()
                .uri("/api/v1/hr/attendance-import/preview")
                .method("POST")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    JsonResponse {
        status,
        json: serde_json::from_slice(&bytes).unwrap(),
    }
}

/// SUPER_ADMIN (not ADMIN): `authorize_hr_org_wide` requires `BranchScope::All`,
/// which only SUPER_ADMIN/EXECUTIVE resolve to without a `user_branches` row
/// (`resolve_branch_scope_in_org`); ADMIN would 403 upstream of the §16 gate
/// this test targets.
async fn seed_admin(pool: &PgPool, user_id: UserId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("ingest-gate-admin-{}", user_id.as_uuid()))
        .bind(vec!["SUPER_ADMIN"])
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

/// Seed a `data_import_runs` row already at `DRY_RUN` with zero candidate rows,
/// so `apply` (once past the checklist gate) resolves an empty, error-free
/// summary and applies cleanly — exercising the real mutation path without
/// reconstructing the upload/preview/dry-run pipeline.
async fn seed_dry_run(pool: &PgPool, tag: &str) -> Uuid {
    let run_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO data_import_runs (
            id, org_id, entity_type, status, source_filename, source_format, source_sha256, pay_period_start, pay_period_end)
         VALUES ($1, $2, 'attendance_direct', 'DRY_RUN', $3, 'csv', $4, DATE '2026-06-01', DATE '2026-06-30')
        "#,
    )
    .bind(run_id)
    .bind(*OrgId::knl().as_uuid())
    .bind(format!("{tag}.csv"))
    .bind(SOURCE_SHA256)
    .execute(pool)
    .await
    .unwrap();
    run_id
}

async fn send(service: axum::Router, uri: &str, token: &str, body: Option<Value>) -> JsonResponse {
    let mut builder = Request::builder()
        .uri(uri)
        .method("POST")
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
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
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

async fn bearer(pool: &PgPool, keys: &Keys, user_id: UserId) -> String {
    bearer_in_org(pool, keys, user_id, OrgId::knl()).await
}

async fn bearer_in_org(pool: &PgPool, keys: &Keys, user_id: UserId, org: OrgId) -> String {
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
    console_platform_test_support::issue_session_token(
        pool,
        &issuer,
        AccessTokenInput {
            subject: user_id,
            org_id: org,
            roles: vec!["SUPER_ADMIN".to_owned()],
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
        },
        None,
        Vec::new(),
    )
    .await
}

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

fn app_state(pool: PgPool, public_key_pem: String) -> Result<AppState, console_app::AppError> {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
    ])?;
    AppState::new(config, DatabaseDependency::Postgres(pool))
}
