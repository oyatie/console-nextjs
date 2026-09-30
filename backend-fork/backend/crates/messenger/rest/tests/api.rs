#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, OrgId, TraceContext, UserId,
};
use console_messenger_adapter_postgres::PgMessengerStore;
use console_messenger_application::CreateThreadCommand;
use console_messenger_domain::ThreadKind;
use console_messenger_rest::{MessengerRestState, router};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_db::{DbError, with_audit};
use console_platform_test_support::{issue_session_token, runtime_role_pool};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn messenger_rest_polling_send_read_and_search_are_authorized(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_key_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let branch_id = seed_branch(&pool, "Messenger REST Region", "Messenger REST Branch").await;
        let other_branch_id = seed_branch(
            &pool,
            "Messenger REST Other Region",
            "Messenger REST Other Branch",
        )
        .await;
        let sender = UserId::new();
        let recipient = UserId::new();
        let outsider = UserId::new();
        seed_user_with_branch(&pool, sender, "MECHANIC", branch_id).await;
        seed_user_with_branch(&pool, recipient, "ADMIN", branch_id).await;
        seed_user_with_branch(&pool, outsider, "MECHANIC", other_branch_id).await;
        let store = PgMessengerStore::new(pool.clone());
        let thread = store
            .create_thread(CreateThreadCommand {
                actor: sender,
                branch_scope: BranchScope::single(branch_id),
                branch_id,
                kind: ThreadKind::Team,
                visibility: None,
                title: Some("정비팀".to_owned()),
                work_order_id: None,
                member_ids: vec![sender, recipient],
                trace: console_kernel_core::TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        let token = issue_token(
            &pool,
            private_pem.as_bytes(),
            public_key_pem.as_bytes(),
            sender,
            vec!["MECHANIC".to_owned()],
            vec![branch_id],
        )
        .await
        .unwrap();
        let verifier = JwtVerifier::from_es256_public_pem(
            JwtSettings {
                issuer: TEST_ISSUER.to_owned(),
                audience: TEST_AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            public_key_pem.as_bytes(),
        )
        .unwrap();
        let service = router(MessengerRestState::new(
            PgMessengerStore::new(runtime_role_pool(&pool).await),
            Some(verifier),
        ));

        let members = get_json(
            service.clone(),
            &format!("/api/messenger/members?branch_id={branch_id}&limit=10"),
            &token,
        )
        .await;
        assert_eq!(members.status, StatusCode::OK, "{:?}", members.json);
        let member_ids = members.json["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|member| member["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert!(member_ids.contains(&sender.to_string()));
        assert!(member_ids.contains(&recipient.to_string()));
        assert!(!member_ids.contains(&outsider.to_string()));

        let denied_members = get_json(
            service.clone(),
            &format!("/api/messenger/members?branch_id={other_branch_id}&limit=10"),
            &token,
        )
        .await;
        assert_eq!(
            denied_members.status,
            StatusCode::FORBIDDEN,
            "{:?}",
            denied_members.json
        );

        // GET /api/messenger/members/{id} (person pin, AC4): non-self in-branch
        // → 200 + summary; self → 200; a target not in the branch → 404 (NOT
        // 403 — deny-by-omission via not_found, the audit rolled back).
        let profile = get_json(
            service.clone(),
            &format!("/api/messenger/members/{recipient}?branch_id={branch_id}"),
            &token,
        )
        .await;
        assert_eq!(profile.status, StatusCode::OK, "{:?}", profile.json);
        assert_eq!(profile.json["id"], recipient.to_string());
        assert!(profile.json["display_name"].is_string());

        let self_profile = get_json(
            service.clone(),
            &format!("/api/messenger/members/{sender}?branch_id={branch_id}"),
            &token,
        )
        .await;
        assert_eq!(
            self_profile.status,
            StatusCode::OK,
            "{:?}",
            self_profile.json
        );

        let out_of_branch = get_json(
            service.clone(),
            &format!("/api/messenger/members/{outsider}?branch_id={branch_id}"),
            &token,
        )
        .await;
        assert_eq!(
            out_of_branch.status,
            StatusCode::NOT_FOUND,
            "{:?}",
            out_of_branch.json
        );

        let invalid_thread = post_json(
            service.clone(),
            "/api/messenger/threads",
            &token,
            json!({
                "branch_id": branch_id,
                "kind": "dm",
                "title": "cross-branch should fail",
                "member_ids": [outsider],
            }),
        )
        .await;
        assert_eq!(
            invalid_thread.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{:?}",
            invalid_thread.json
        );

        revoke_user_branch(&pool, sender, branch_id).await;
        let denied = post_json(
            service.clone(),
            &format!("/api/messenger/threads/{}/messages", thread.id),
            &token,
            json!({ "body": "revoked branch should fail closed" }),
        )
        .await;
        assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);
        grant_user_branch(&pool, sender, branch_id).await;

        let sent = post_json(
            service.clone(),
            &format!("/api/messenger/threads/{}/messages", thread.id),
            &token,
            json!({ "body": "긴급 누유 확인" }),
        )
        .await;
        assert_eq!(sent.status, StatusCode::CREATED, "{:?}", sent.json);
        let message_id = sent.json["id"].as_str().unwrap().to_owned();

        let page = get_json(
            service.clone(),
            &format!("/api/messenger/threads/{}/messages?limit=20", thread.id),
            &token,
        )
        .await;
        assert_eq!(page.status, StatusCode::OK, "{:?}", page.json);
        assert_eq!(page.json["items"][0]["id"], message_id);

        let read = put_json(
            service.clone(),
            &format!("/api/messenger/threads/{}/read-receipt", thread.id),
            &token,
            json!({ "last_read_message_id": message_id }),
        )
        .await;
        assert_eq!(read.status, StatusCode::OK, "{:?}", read.json);
        assert_eq!(read.json["last_read_message_id"], message_id);

        let search = get_json(service, "/api/messenger/search?q=누유&limit=10", &token).await;
        assert_eq!(search.status, StatusCode::OK, "{:?}", search.json);
        assert_eq!(search.json["items"][0]["id"], message_id);
    })
    .await;
}

// Safer default (review polish, PR #261 item 3): an API-created `team` thread
// with a title used to silently default to a joinable `channel`, exposing
// full history to anyone who joined even though the caller supplied a
// curated `member_ids` list. Omitting `visibility` must now yield `direct`;
// the caller must explicitly pass `visibility: "channel"` to opt in.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_thread_named_team_thread_defaults_to_direct_via_rest(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_key_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let branch_id = seed_branch(&pool, "Visibility Region", "Visibility Branch").await;
        let sender = UserId::new();
        let recipient = UserId::new();
        seed_user_with_branch(&pool, sender, "MECHANIC", branch_id).await;
        seed_user_with_branch(&pool, recipient, "ADMIN", branch_id).await;
        let token = issue_token(
            &pool,
            private_pem.as_bytes(),
            public_key_pem.as_bytes(),
            sender,
            vec!["MECHANIC".to_owned()],
            vec![branch_id],
        )
        .await
        .unwrap();
        let verifier = JwtVerifier::from_es256_public_pem(
            JwtSettings {
                issuer: TEST_ISSUER.to_owned(),
                audience: TEST_AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            public_key_pem.as_bytes(),
        )
        .unwrap();
        let service = router(MessengerRestState::new(
            PgMessengerStore::new(runtime_role_pool(&pool).await),
            Some(verifier),
        ));

        // No `visibility` in the request body -> direct, not channel.
        let no_visibility = post_json(
            service.clone(),
            "/api/messenger/threads",
            &token,
            json!({
                "branch_id": branch_id,
                "kind": "team",
                "title": "정비팀",
                "member_ids": [sender, recipient],
            }),
        )
        .await;
        assert_eq!(
            no_visibility.status,
            StatusCode::CREATED,
            "{:?}",
            no_visibility.json
        );
        assert_eq!(no_visibility.json["visibility"], "direct");

        // Explicit `visibility: "channel"` still opts in.
        let explicit_channel = post_json(
            service,
            "/api/messenger/threads",
            &token,
            json!({
                "branch_id": branch_id,
                "kind": "team",
                "visibility": "channel",
                "title": "공지 채널",
                "member_ids": [sender, recipient],
            }),
        )
        .await;
        assert_eq!(
            explicit_channel.status,
            StatusCode::CREATED,
            "{:?}",
            explicit_channel.json
        );
        assert_eq!(explicit_channel.json["visibility"], "channel");
    })
    .await;
}

#[derive(Debug)]
struct JsonResponse {
    status: StatusCode,
    json: Value,
}

async fn get_json(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    request_json(service, "GET", uri, token, None).await
}

async fn post_json(service: axum::Router, uri: &str, token: &str, body: Value) -> JsonResponse {
    request_json(service, "POST", uri, token, Some(body)).await
}

async fn put_json(service: axum::Router, uri: &str, token: &str, body: Value) -> JsonResponse {
    request_json(service, "PUT", uri, token, Some(body)).await
}

async fn request_json(
    service: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> JsonResponse {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let request_body = match body {
        Some(body) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = service
        .oneshot(builder.body(request_body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    JsonResponse { status, json }
}

async fn issue_token(
    pool: &PgPool,
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    roles: Vec<String>,
    branches: Vec<BranchId>,
) -> Result<String, Box<dyn std::error::Error>> {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_key_pem,
        public_key_pem,
    )?;

    Ok(issue_session_token(
        pool,
        &issuer,
        AccessTokenInput {
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
        },
        None,
        vec![],
    )
    .await)
}

async fn seed_branch(pool: &PgPool, region_name: &str, branch_name: &str) -> BranchId {
    let region_id = uuid::Uuid::new_v4();
    let branch_id = BranchId::new();
    let region_name = format!("{region_name} {}", uuid::Uuid::new_v4());
    let branch_name = format!("{branch_name} {}", uuid::Uuid::new_v4());
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

async fn revoke_user_branch(pool: &PgPool, user_id: UserId, branch_id: BranchId) {
    let event = AuditEvent::new(
        Some(user_id),
        AuditAction::new("test.revoke_user_branch").unwrap(),
        "user_branch",
        format!("{user_id}:{branch_id}"),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_branch(branch_id)
    .with_org(OrgId::knl());
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "DELETE FROM user_branches WHERE user_id = $1 AND branch_id = $2 AND org_id = $3",
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
}

async fn grant_user_branch(pool: &PgPool, user_id: UserId, branch_id: BranchId) {
    let event = AuditEvent::new(
        Some(user_id),
        AuditAction::new("test.grant_user_branch").unwrap(),
        "user_branch",
        format!("{user_id}:{branch_id}"),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_branch(branch_id)
    .with_org(OrgId::knl());
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                r#"
                INSERT INTO user_branches (user_id, branch_id, org_id)
                VALUES ($1, $2, $3)
                ON CONFLICT (user_id, branch_id) DO NOTHING
                "#,
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
}

async fn seed_user_with_branch(pool: &PgPool, user_id: UserId, role: &str, branch_id: BranchId) {
    let role = role.to_owned();
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
            .bind(format!("Messenger REST {role}"))
            .bind(Vec::from([role]))
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
}
