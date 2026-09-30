//! Authenticated support REST test: the list endpoint must re-resolve the
//! caller's branch scope from the database rather than trusting the JWT
//! `branches` claim, so a branch-membership change takes effect immediately.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::{Body, to_bytes};
use console_kernel_core::{AuditAction, AuditEvent, BranchId, OrgId, TraceContext, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_db::{DbError, with_audit};
use console_platform_test_support::runtime_role_pool;
use console_support_adapter_postgres::PgSupportStore;
use console_support_application::CreateInternalTicketCommand;
use console_support_domain::{TicketCategory, TicketPriority};
use console_support_rest::{SupportRestState, router};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::Value;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

/// The token claims a branch the user is NOT a member of, while the user's real
/// (DB) membership is a different branch that holds a ticket. After the fix the
/// list endpoint resolves the scope from the DB, so the user sees their real
/// branch's ticket and the spoofed claim is ignored.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn list_tickets_resolves_branch_scope_from_db_not_token_claim(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_key_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();

        let real_branch = seed_branch(&pool).await;
        let claimed_branch = seed_branch(&pool).await;
        let user = seed_user_in_branch(&pool, "Support Staff", real_branch).await;

        let store = PgSupportStore::new(pool.clone());
        let now = OffsetDateTime::now_utc();
        let ticket = store
            .create_internal_ticket(CreateInternalTicketCommand {
                actor: user,
                branch_id: real_branch,
                category: TicketCategory::SystemBug,
                priority: TicketPriority::High,
                title: "real branch ticket".to_owned(),
                body: "x".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();

        // Token names only `claimed_branch`, which the user is NOT a member of.
        let token = issue_token(
            private_pem.as_bytes(),
            public_key_pem.as_bytes(),
            user,
            vec!["MECHANIC".to_owned()],
            vec![claimed_branch],
        );
        let verifier = JwtVerifier::from_es256_public_pem(
            JwtSettings {
                issuer: TEST_ISSUER.to_owned(),
                audience: TEST_AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            public_key_pem.as_bytes(),
        )
        .unwrap();
        let service = router(SupportRestState::new(
            PgSupportStore::new(runtime_role_pool(&pool).await),
            Some(verifier),
            None,
        ));

        let response = get_json(service, "/api/v1/support/tickets", &token).await;
        assert_eq!(response.0, StatusCode::OK, "{:?}", response.1);
        let items = response.1["items"]
            .as_array()
            .expect("paginated items array");
        // The user's REAL branch ticket is visible (DB scope), even though the token
        // claimed a different branch.
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0]["id"], Value::String(ticket.id.to_string()));
    })
    .await;
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn issue_token(
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    roles: Vec<String>,
    branches: Vec<BranchId>,
) -> String {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_key_pem,
        public_key_pem,
    )
    .unwrap();
    issuer
        .issue_access_token(AccessTokenInput {
            subject: user_id,
            org_id: OrgId::knl(),
            roles,
            branches,
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

// Seed helpers route their inserts through `with_audit` because this file lives
// on a `rest/` handler surface scanned by the audit-coverage gate (mirroring the
// messenger REST test harness).
async fn seed_branch(pool: &PgPool) -> BranchId {
    let region_id = uuid::Uuid::new_v4();
    let branch_id = BranchId::new();
    let region_name = format!("Support Authz Region {}", uuid::Uuid::new_v4());
    let branch_name = format!("Support Authz Branch {}", uuid::Uuid::new_v4());
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_branch").unwrap(),
        "branch",
        branch_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_branch(branch_id);
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query("INSERT INTO regions (id, name, org_id) VALUES ($1, $2, $3)")
                .bind(region_id)
                .bind(region_name)
                .bind(*OrgId::knl().as_uuid())
                .execute(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
            sqlx::query(
                "INSERT INTO branches (id, region_id, name, org_id) VALUES ($1, $2, $3, $4)",
            )
            .bind(*branch_id.as_uuid())
            .bind(region_id)
            .bind(branch_name)
            .bind(*OrgId::knl().as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<BranchId, DbError>(branch_id)
        })
    })
    .await
    .unwrap()
}

async fn seed_user_in_branch(pool: &PgPool, name: &str, branch_id: BranchId) -> UserId {
    let user_id = UserId::new();
    let name = name.to_owned();
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_user").unwrap(),
        "user",
        user_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_branch(branch_id);
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)",
            )
            .bind(*user_id.as_uuid())
            .bind(name)
            .bind(Vec::from(["MECHANIC".to_owned()]))
            .bind(*OrgId::knl().as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            sqlx::query(
                "INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)",
            )
            .bind(*user_id.as_uuid())
            .bind(*branch_id.as_uuid())
            .bind(*OrgId::knl().as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
    user_id
}

async fn get_json(service: Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    (status, json)
}
