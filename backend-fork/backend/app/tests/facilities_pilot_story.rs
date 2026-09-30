#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Real HTTP + Postgres contract for the scheduled HVAC pilot.  This deliberately
//! uses the app router, JWT authorization, and the disposable SQLx database.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const ISSUER: &str = "console-platform-auth";
const AUDIENCE: &str = "console-api";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn scheduled_hvac_story_materializes_and_closes_with_persisted_photo(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    let obligation = fixture
        .obligation(OffsetDateTime::now_utc() - Duration::minutes(5))
        .await;

    assert_eq!(
        console_facilities_rest::poll_scheduled_hvac(&pool)
            .await
            .unwrap(),
        1
    );
    let case_id = fixture.only_case().await;
    let due: OffsetDateTime =
        sqlx::query_scalar("SELECT occurrence_due_at FROM facilities_cases WHERE id=$1")
            .bind(case_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let response_due: OffsetDateTime =
        sqlx::query_scalar("SELECT response_due_at FROM facilities_cases WHERE id=$1")
            .bind(case_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        response_due > due,
        "SLA deadline must be derived from the occurrence due time"
    );

    let service = fixture.router();
    assert_eq!(
        post(
            service.clone(),
            &format!("/api/v1/facilities/cases/{case_id}/triage"),
            &fixture.admin,
            json!({"scheduledFor": rfc3339(OffsetDateTime::now_utc() + Duration::hours(1))})
        )
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(
        post(
            service.clone(),
            &format!("/api/v1/facilities/cases/{case_id}/assign"),
            &fixture.admin,
            json!({"assigneeId": fixture.tech_id})
        )
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(
        post(
            service.clone(),
            &format!("/api/v1/facilities/cases/{case_id}/start"),
            &fixture.tech,
            json!({})
        )
        .await
        .status,
        StatusCode::OK
    );

    let observed = post(service.clone(), &format!("/api/v1/facilities/cases/{case_id}/observations"), &fixture.tech, json!({"preKwh":"100.000", "postKwh":"91.500", "costKrw":42000, "observedAt":rfc3339(OffsetDateTime::now_utc())})).await;
    assert_eq!(observed.status, StatusCode::OK);
    assert_eq!(observed.json["energyDeltaKwh"], "-8.500");
    assert_eq!(observed.json["totalCostKrw"], 42000);

    let evidence = fixture.admissible_evidence(3).await;
    let submitted = post(service.clone(), &format!("/api/v1/facilities/cases/{case_id}/submit"), &fixture.tech, json!({"safetyChecklistEvidenceId": evidence[0], "serviceReportEvidenceId": evidence[1], "photoEvidenceId": evidence[2]})).await;
    assert_eq!(submitted.status, StatusCode::OK);
    assert_eq!(submitted.json["status"], "AWAITING_ACCEPTANCE");
    let photo_links: i64 = sqlx::query_scalar("SELECT count(*) FROM facilities_execution_evidence_links WHERE case_id=$1 AND evidence_kind='PHOTO' AND evidence_id=$2")
        .bind(case_id).bind(evidence[2]).fetch_one(&pool).await.unwrap();
    assert_eq!(
        photo_links, 1,
        "submitted photo must be persisted as execution evidence"
    );

    let accepted = post(
        service,
        &format!("/api/v1/facilities/cases/{case_id}/acceptance"),
        &fixture.admin,
        json!({"decision":"ACCEPTED"}),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::OK);
    assert_eq!(accepted.json["status"], "CLOSED");
    let history: Vec<String> = sqlx::query_scalar(
        "SELECT to_status FROM facilities_case_history WHERE case_id=$1 ORDER BY occurred_at, id",
    )
    .bind(case_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        history,
        [
            "DUE",
            "SCHEDULED",
            "ASSIGNED",
            "IN_PROGRESS",
            "AWAITING_ACCEPTANCE",
            "CLOSED"
        ]
    );
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE target_id=$1 AND action LIKE 'facilities.case.%'",
    )
    .bind(case_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        audit_count, 7,
        "scheduler plus every user-visible mutation must be audited"
    );
    let next_due: OffsetDateTime =
        sqlx::query_scalar("SELECT next_due_at FROM facilities_obligations WHERE id=$1")
            .bind(obligation)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(next_due > OffsetDateTime::now_utc());
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn scheduler_is_idempotent_and_preserves_overdue_sla_truth(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    let overdue = OffsetDateTime::now_utc() - Duration::hours(2);
    let obligation = fixture.obligation(overdue).await;

    assert_eq!(
        console_facilities_rest::poll_scheduled_hvac(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        console_facilities_rest::poll_scheduled_hvac(&pool)
            .await
            .unwrap(),
        0
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM facilities_cases WHERE obligation_id=$1")
            .bind(obligation)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count, 1,
        "a second scheduler run must not duplicate an occurrence"
    );
    let (response_due, status): (OffsetDateTime, String) = sqlx::query_as(
        "SELECT response_due_at,status FROM facilities_cases WHERE obligation_id=$1",
    )
    .bind(obligation)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        response_due < OffsetDateTime::now_utc(),
        "overdue occurrence retains its true response SLA"
    );
    assert_eq!(status, "DUE");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn create_replay_is_idempotent_and_changed_payload_conflicts(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    let obligation = fixture
        .obligation(OffsetDateTime::now_utc() + Duration::days(1))
        .await;
    let service = fixture.router();
    let body = json!({"obligationId": obligation, "idempotencyKey":"facilities-replay-key-0001"});
    let first = post(
        service.clone(),
        "/api/v1/facilities/cases",
        &fixture.admin,
        body.clone(),
    )
    .await;
    let replay = post(
        service.clone(),
        "/api/v1/facilities/cases",
        &fixture.admin,
        body,
    )
    .await;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(first.json["id"], replay.json["id"]);

    let changed_obligation = fixture
        .obligation(OffsetDateTime::now_utc() + Duration::days(2))
        .await;
    let changed = post(
        service,
        "/api/v1/facilities/cases",
        &fixture.admin,
        json!({"obligationId": changed_obligation, "idempotencyKey":"facilities-replay-key-0001"}),
    )
    .await;
    assert_eq!(changed.status, StatusCode::CONFLICT);
    let cases: i64 = sqlx::query_scalar("SELECT count(*) FROM facilities_cases")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        cases, 1,
        "a changed replay must not create another occurrence"
    );
}

/// The facilities intake must not empty out. `FacilitiesManage` is `[D,D,D,A,D,A]`
/// and `FacilitiesObserve` is `[D,A,A,A,A,A]`; routing create/read through
/// `authorize_org_wide` collapsed both to SUPER_ADMIN and locked out every
/// branch-scoped ADMIN, even though the branch check that follows admits them.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn branch_scoped_admin_can_create_and_read_a_facilities_case(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    let obligation = fixture
        .obligation(OffsetDateTime::now_utc() + Duration::days(1))
        .await;
    let service = fixture.router();

    let created = post(
        service.clone(),
        "/api/v1/facilities/cases",
        &fixture.admin,
        json!({"obligationId": obligation, "idempotencyKey":"facilities-admin-key-0001"}),
    )
    .await;
    assert_eq!(
        created.status,
        StatusCode::OK,
        "a branch-scoped ADMIN holds FacilitiesManage on its own branch"
    );
    assert_eq!(created.json["branchId"], json!(fixture.branch.as_uuid()));

    let case_id = created.json["id"].as_str().unwrap();
    let read = get(
        service.clone(),
        &format!("/api/v1/facilities/cases/{case_id}"),
        &fixture.admin,
    )
    .await;
    assert_eq!(
        read.status,
        StatusCode::OK,
        "a branch-scoped ADMIN holds FacilitiesObserve on its own branch"
    );
    assert_eq!(read.json["id"], created.json["id"]);
    for field in ["responseDueAt", "completionDueAt", "acceptanceDueAt"] {
        assert_date_time(&read.json, field);
    }

    let listed = get(service, "/api/v1/facilities/cases", &fixture.admin).await;
    assert_eq!(
        listed.status,
        StatusCode::OK,
        "the list route must admit the same branch-scoped ADMIN the single-case route admits"
    );
    assert_eq!(listed.json, json!([read.json]));
}

/// The list route reads every row through the per-row `FacilitiesObserve` branch gate,
/// which errors rather than skips, so an org-wide select would make one foreign-branch
/// case 403 the entire list for a branch-scoped caller.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn list_cases_is_confined_to_the_callers_branch_scope(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    let service = fixture.router();
    let mine = post(
        service.clone(),
        "/api/v1/facilities/cases",
        &fixture.admin,
        json!({
            "obligationId": fixture.obligation(OffsetDateTime::now_utc() + Duration::days(1)).await,
            "idempotencyKey": "facilities-list-own-key-0001"
        }),
    )
    .await;
    assert_eq!(mine.status, StatusCode::OK);

    let other_branch = fixture.other_branch().await;
    let other_obligation = seed_obligation(
        &pool,
        other_branch,
        OffsetDateTime::now_utc() + Duration::days(1),
    )
    .await;
    let other_admin = fixture
        .token_for(UserId::new(), vec!["ADMIN"], vec![other_branch])
        .await;
    let theirs = post(
        service.clone(),
        "/api/v1/facilities/cases",
        &other_admin,
        json!({"obligationId": other_obligation, "idempotencyKey":"facilities-list-other-key-01"}),
    )
    .await;
    assert_eq!(theirs.status, StatusCode::OK);

    let listed = get(service, "/api/v1/facilities/cases", &fixture.admin).await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(
        listed.json,
        json!([mine.json]),
        "a foreign-branch case must be filtered out of the select, not 403 the whole list"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn rls_pbac_and_terminal_case_protect_facilities_mutations(pool: PgPool) {
    let fixture = Fixture::new(&pool).await;
    fixture
        .obligation(OffsetDateTime::now_utc() - Duration::minutes(1))
        .await;
    console_facilities_rest::poll_scheduled_hvac(&pool)
        .await
        .unwrap();
    let case_id = fixture.only_case().await;
    let service = fixture.router();

    let before_start_observation = post(
        service.clone(),
        &format!("/api/v1/facilities/cases/{case_id}/observations"),
        &fixture.tech,
        json!({"preKwh":"1.000","observedAt":rfc3339(OffsetDateTime::now_utc())}),
    )
    .await;
    assert_eq!(
        before_start_observation.status,
        StatusCode::CONFLICT,
        "observations must not be accepted before the assigned technician starts work"
    );

    let other_branch = fixture.other_branch().await;
    let scoped_admin = fixture
        .token_for(UserId::new(), vec!["ADMIN"], vec![other_branch])
        .await;
    let denied = post(
        service.clone(),
        &format!("/api/v1/facilities/cases/{case_id}/triage"),
        &scoped_admin,
        json!({"scheduledFor":rfc3339(OffsetDateTime::now_utc())}),
    )
    .await;
    assert_eq!(
        denied.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PBAC branch scope must deny dispatch outside its branch"
    );

    assert_eq!(
        post(
            service.clone(),
            &format!("/api/v1/facilities/cases/{case_id}/triage"),
            &fixture.admin,
            json!({"scheduledFor":rfc3339(OffsetDateTime::now_utc())})
        )
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(
        post(
            service.clone(),
            &format!("/api/v1/facilities/cases/{case_id}/assign"),
            &fixture.admin,
            json!({"assigneeId":fixture.tech_id})
        )
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(
        post(
            service.clone(),
            &format!("/api/v1/facilities/cases/{case_id}/start"),
            &fixture.tech,
            json!({})
        )
        .await
        .status,
        StatusCode::OK
    );
    let evidence = fixture.admissible_evidence(2).await;
    assert_eq!(
        post(
            service.clone(),
            &format!("/api/v1/facilities/cases/{case_id}/submit"),
            &fixture.tech,
            json!({"safetyChecklistEvidenceId":evidence[0],"serviceReportEvidenceId":evidence[1]})
        )
        .await
        .status,
        StatusCode::OK
    );
    let rejected_without_reason = post(
        service.clone(),
        &format!("/api/v1/facilities/cases/{case_id}/acceptance"),
        &fixture.admin,
        json!({"decision":"REJECTED"}),
    )
    .await;
    assert_eq!(
        rejected_without_reason.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a rework decision must preserve a non-empty reason for the technician"
    );

    assert_eq!(
        post(
            service.clone(),
            &format!("/api/v1/facilities/cases/{case_id}/acceptance"),
            &fixture.admin,
            json!({"decision":"ACCEPTED"})
        )
        .await
        .status,
        StatusCode::OK
    );

    let mutation = post(
        service,
        &format!("/api/v1/facilities/cases/{case_id}/observations"),
        &fixture.tech,
        json!({"preKwh":"1.000","observedAt":rfc3339(OffsetDateTime::now_utc())}),
    )
    .await;
    assert_eq!(
        mutation.status,
        StatusCode::CONFLICT,
        "a terminal case must reject new observations"
    );
}

/// Every facilities `date-time` field is declared `{type: string, format: date-time}`
/// in `openapi.yaml`. Sending `OffsetDateTime` through `json!` instead runs the same
/// numeric-tuple serializer the handler used to accept, so the array round-trips and
/// the suite stays blind to a response no spec-conformant client can read.
fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap()
}

/// Asserts a response field is an RFC 3339 string, not `time`'s component array.
fn assert_date_time(response: &Value, field: &str) {
    let raw = response[field].as_str().unwrap_or_else(|| {
        panic!(
            "{field} is declared {{type: string, format: date-time}}, got {}",
            response[field]
        )
    });
    OffsetDateTime::parse(raw, &Rfc3339).unwrap();
}

struct Response {
    status: StatusCode,
    json: Value,
}
async fn post(service: axum::Router, uri: &str, token: &str, body: Value) -> Response {
    send(
        service,
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}
async fn get(service: axum::Router, uri: &str, token: &str) -> Response {
    send(
        service,
        Request::builder()
            .method("GET")
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}
async fn send(service: axum::Router, request: Request<Body>) -> Response {
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    Response {
        status,
        json: serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({})),
    }
}

struct Fixture {
    pool: PgPool,
    branch: BranchId,
    admin: String,
    tech: String,
    tech_id: Uuid,
    private: String,
    public: String,
}
impl Fixture {
    async fn new(pool: &PgPool) -> Self {
        let signing = SigningKey::random(&mut OsRng);
        let private = signing.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
        let public = signing
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let branch = seed_branch(pool, "Facilities").await;
        let admin_id = UserId::new();
        let tech = UserId::new();
        seed_user(pool, admin_id, "ADMIN", branch).await;
        seed_user(pool, tech, "MECHANIC", branch).await;
        let f = Self {
            pool: pool.clone(),
            branch,
            admin: String::new(),
            tech: String::new(),
            tech_id: *tech.as_uuid(),
            private,
            public,
        };
        let admin = f.token(admin_id, vec!["ADMIN".to_owned()], vec![]);
        let technician = f.token(tech, vec!["MECHANIC".to_owned()], vec![branch]);
        Self {
            admin,
            tech: technician,
            ..f
        }
    }
    fn token(&self, id: UserId, roles: Vec<String>, branches: Vec<BranchId>) -> String {
        JwtIssuer::from_es256_pem(
            JwtSettings {
                issuer: ISSUER.to_owned(),
                audience: AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            self.private.as_bytes(),
            self.public.as_bytes(),
        )
        .unwrap()
        .issue_access_token(AccessTokenInput {
            subject: id,
            org_id: OrgId::knl(),
            roles,
            branches,
            platform: false,
            view_as: false,
            read_only: false,
            display_name: None,
            feature_grants: vec![],
            authz_subject_version: 0,
            authz_policy_version: 0,
            session_generation: 0,
            issued_at: OffsetDateTime::now_utc(),
        })
        .unwrap()
    }
    async fn token_for(&self, id: UserId, roles: Vec<&str>, branches: Vec<BranchId>) -> String {
        seed_user(&self.pool, id, roles[0], branches[0]).await;
        self.token(id, roles.into_iter().map(str::to_owned).collect(), branches)
    }
    fn router(&self) -> axum::Router {
        let config = AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
            ("CONSOLE_JWT_ISSUER", ISSUER.to_owned()),
            ("CONSOLE_JWT_AUDIENCE", AUDIENCE.to_owned()),
            ("CONSOLE_JWT_PUBLIC_KEY_PEM", self.public.clone()),
        ])
        .unwrap();
        build_router(
            AppState::new(config, DatabaseDependency::Postgres(self.pool.clone())).unwrap(),
        )
    }
    async fn other_branch(&self) -> BranchId {
        seed_branch(&self.pool, "Other Facilities").await
    }
    async fn obligation(&self, due: OffsetDateTime) -> Uuid {
        seed_obligation(&self.pool, self.branch, due).await
    }
    async fn only_case(&self) -> Uuid {
        sqlx::query_scalar("SELECT id FROM facilities_cases")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    async fn admissible_evidence(&self, count: usize) -> Vec<Uuid> {
        let mut ids = Vec::new();
        for n in 0..count {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO docs_evidence_objects(id,org_id,code,title,source_type,source_id,classification,admissibility_status,created_by,updated_by) VALUES($1,$2,$3,$4,'external_document',$5,'GENERAL','ADMISSIBLE',$6,$6)").bind(id).bind(*OrgId::knl().as_uuid()).bind(format!("EV-FAC-{n}-{}", id.simple().to_string().to_uppercase())).bind(format!("Facility evidence {n}")).bind(format!("source-{n}")).bind(self.tech_id).execute(&self.pool).await.unwrap();
            ids.push(id);
        }
        ids
    }
}

async fn seed_branch(pool: &PgPool, name: &str) -> BranchId {
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions(name,org_id) VALUES($1,$2) RETURNING id")
            .bind(format!("{name} Region"))
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches(region_id,name,org_id) VALUES($1,$2,$3) RETURNING id",
        )
        .bind(region)
        .bind(name)
        .bind(*OrgId::knl().as_uuid())
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}
async fn seed_user(pool: &PgPool, id: UserId, role: &str, branch: BranchId) {
    sqlx::query("INSERT INTO users(id,display_name,roles,org_id) VALUES($1,$2,$3,$4)")
        .bind(*id.as_uuid())
        .bind(format!("Facilities {role}"))
        .bind(vec![role])
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches(user_id,branch_id,org_id) VALUES($1,$2,$3)")
        .bind(*id.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}
// Called TWICE inside `create_replay_is_idempotent_and_changed_payload_conflicts` against the
// same branch, and every identifier below is a constant. The three inserts therefore collide on
// UNIQUE (branch_id,name), UNIQUE (branch_id,customer_id,name) and UNIQUE (org_id,service_key).
// `DO NOTHING` is wrong for all three — they are `fetch_one`, so a skipped row returns no id.
// The catalog cannot take a discriminator either: 0178:18 CHECKs service_key against exactly one
// literal. Upserting is the only shape that satisfies both the CHECK and the second call.
async fn seed_obligation(pool: &PgPool, branch: BranchId, due: OffsetDateTime) -> Uuid {
    let org = *OrgId::knl().as_uuid();
    let customer:Uuid=sqlx::query_scalar("INSERT INTO registry_customers(branch_id,name,org_id) VALUES($1,'Facility Customer',$2) ON CONFLICT (branch_id,name) DO UPDATE SET name=EXCLUDED.name RETURNING id").bind(*branch.as_uuid()).bind(org).fetch_one(pool).await.unwrap();
    let site:Uuid=sqlx::query_scalar("INSERT INTO registry_sites(branch_id,customer_id,name,org_id) VALUES($1,$2,'Facility Site',$3) ON CONFLICT (branch_id,customer_id,name) DO UPDATE SET name=EXCLUDED.name RETURNING id").bind(*branch.as_uuid()).bind(customer).bind(org).fetch_one(pool).await.unwrap();
    let catalog:Uuid=sqlx::query_scalar("INSERT INTO facilities_catalog_services(org_id,service_key,name) VALUES($1,'HVAC_PREVENTIVE_MAINTENANCE','HVAC PM') ON CONFLICT (org_id,service_key) DO UPDATE SET name=EXCLUDED.name RETURNING id").bind(org).fetch_one(pool).await.unwrap();
    let asset:Uuid=sqlx::query_scalar("INSERT INTO facilities_assets(org_id,branch_id,site_id,catalog_service_id,asset_tag,name) VALUES($1,$2,$3,$4,$5,'HVAC Unit') RETURNING id").bind(org).bind(*branch.as_uuid()).bind(site).bind(catalog).bind(format!("HVAC-{}",Uuid::new_v4())).fetch_one(pool).await.unwrap();
    sqlx::query_scalar("INSERT INTO facilities_obligations(org_id,branch_id,site_id,asset_id,catalog_service_id,recurrence_days,next_due_at,response_due_seconds,completion_due_seconds,acceptance_due_seconds,target_energy_kwh) VALUES($1,$2,$3,$4,$5,30,$6,60,3600,7200,0) RETURNING id").bind(org).bind(*branch.as_uuid()).bind(site).bind(asset).bind(catalog).bind(due).fetch_one(pool).await.unwrap()
}
