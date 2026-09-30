#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! HTTP-level authz + recipient-scoping for the notice board.
//!
//! Proves over the real router: draft creation/publish/progress require the
//! publish tier (a plain ADMIN gets 403, not a silent 200); a published
//! notice is readable by anyone; 수령확인 is recipient-scoped from the JWT.

use axum::body::{Body, to_bytes};
use console_kernel_core::{AuditAction, AuditEvent, BranchId, OrgId, TraceContext, UserId};
use console_notices_adapter_postgres::PgNoticeStore;
use console_notices_rest::{NoticeRestState, router};
use console_notifications_adapter_postgres::PgNotificationStore;
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_db::{DbError, with_audit};
use console_platform_test_support::{grant_console_rt, runtime_role_pool};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::Value;
use sqlx::PgPool;
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn notice_board_rest_is_publish_tier_gated_and_recipient_scoped(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_key_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();

        let manager = UserId::new();
        let plain_admin = UserId::new();
        seed_user(&pool, manager, "Manager").await;
        seed_user(&pool, plain_admin, "Plain Admin").await;

        let verifier = JwtVerifier::from_es256_public_pem(
            JwtSettings {
                issuer: TEST_ISSUER.to_owned(),
                audience: TEST_AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            public_key_pem.as_bytes(),
        )
        .unwrap();

        grant_console_rt(
            &pool,
            &[
                "GRANT SELECT, INSERT, UPDATE ON notices TO console_rt",
                "GRANT SELECT, INSERT, UPDATE ON notice_receipts TO console_rt",
                "GRANT SELECT, INSERT, UPDATE ON notifications TO console_rt",
                "GRANT SELECT, INSERT, UPDATE ON object_code_counters TO console_rt",
                "GRANT SELECT ON object_types TO console_rt",
            ],
        )
        .await;
        let rt_pool = runtime_role_pool(&pool).await;
        let notifications = PgNotificationStore::new(rt_pool.clone());
        let store =
            PgNoticeStore::new(rt_pool.clone()).with_notification_sink(Arc::new(notifications));
        let service = router(NoticeRestState::new(store, Some(verifier)));

        let manager_token = issue_token(
            private_pem.as_bytes(),
            public_key_pem.as_bytes(),
            manager,
            &["SUPER_ADMIN"],
        );
        let plain_token = issue_token(
            private_pem.as_bytes(),
            public_key_pem.as_bytes(),
            plain_admin,
            &["ADMIN"],
        );

        // A plain ADMIN cannot create a draft: 403, not a silent success.
        let denied = post_json(
            service.clone(),
            "/api/v1/notices",
            &plain_token,
            serde_json::json!({"title": "전사 공지", "body": "본문"}),
        )
        .await;
        assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);

        // The publish-tier manager creates a draft.
        let created = post_json(
            service.clone(),
            "/api/v1/notices",
            &manager_token,
            serde_json::json!({"title": "전사 공지", "body": "본문"}),
        )
        .await;
        assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.json);
        let notice_id = created.json["id"].as_str().unwrap().to_owned();
        assert_eq!(created.json["status"].as_str(), Some("draft"));

        // A plain ADMIN cannot see the draft (get -> 404, list excludes it).
        let hidden = get_json(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}"),
            &plain_token,
        )
        .await;
        assert_eq!(hidden.status, StatusCode::NOT_FOUND);

        let plain_list = get_json(service.clone(), "/api/v1/notices", &plain_token).await;
        assert!(plain_list.json.as_array().unwrap().is_empty());

        // A plain ADMIN cannot publish: 403.
        let publish_denied = post_json_empty(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}/publish"),
            &plain_token,
        )
        .await;
        assert_eq!(publish_denied.status, StatusCode::FORBIDDEN);

        // The manager publishes.
        let published = post_json_empty(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}/publish"),
            &manager_token,
        )
        .await;
        assert_eq!(published.status, StatusCode::OK, "{:?}", published.json);
        assert_eq!(published.json["status"].as_str(), Some("published"));
        assert!(published.json["code"].as_str().unwrap().starts_with("NT-"));

        // Now the plain ADMIN can read it (published notices are open).
        let visible = get_json(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}"),
            &plain_token,
        )
        .await;
        assert_eq!(visible.status, StatusCode::OK);

        // Both manager + plain admin were snapshotted as recipients (every
        // active org member); each can acknowledge exactly their own receipt.
        let ack = post_json_empty(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}/ack"),
            &plain_token,
        )
        .await;
        assert_eq!(ack.status, StatusCode::NO_CONTENT, "{:?}", ack.json);

        // Progress-read requires the publish tier: plain admin -> 403.
        let progress_denied = get_json(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}/progress"),
            &plain_token,
        )
        .await;
        assert_eq!(progress_denied.status, StatusCode::FORBIDDEN);

        let progress = get_json(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}/progress"),
            &manager_token,
        )
        .await;
        assert_eq!(progress.status, StatusCode::OK, "{:?}", progress.json);
        assert_eq!(progress.json["total"].as_i64(), Some(2));
        assert_eq!(progress.json["acknowledged"].as_i64(), Some(1));

        // Unauthenticated request is rejected.
        let anon = service
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/notices")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn notice_board_rest_scoped_audience_draft_edit_and_receipts(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_key_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();

        let manager = UserId::new();
        let member_in = UserId::new();
        let member_out = UserId::new();
        seed_user(&pool, manager, "Manager").await;
        seed_user(&pool, member_in, "Member In").await;
        seed_user(&pool, member_out, "Member Out").await;
        let branch_a = seed_branch(&pool, "창원지사").await;
        let branch_b = seed_branch(&pool, "부산지사").await;
        let branch_empty = seed_branch(&pool, "신설지사").await;
        join_branch(&pool, member_in, branch_a).await;
        join_branch(&pool, member_out, branch_b).await;

        let verifier = JwtVerifier::from_es256_public_pem(
            JwtSettings {
                issuer: TEST_ISSUER.to_owned(),
                audience: TEST_AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            public_key_pem.as_bytes(),
        )
        .unwrap();

        grant_console_rt(
            &pool,
            &[
                "GRANT SELECT, INSERT, UPDATE ON notices TO console_rt",
                "GRANT SELECT, INSERT, UPDATE ON notice_receipts TO console_rt",
                "GRANT SELECT, INSERT, UPDATE ON notifications TO console_rt",
                "GRANT SELECT, INSERT, UPDATE ON object_code_counters TO console_rt",
                "GRANT SELECT ON object_types TO console_rt",
            ],
        )
        .await;
        let rt_pool = runtime_role_pool(&pool).await;
        let notifications = PgNotificationStore::new(rt_pool.clone());
        let store =
            PgNoticeStore::new(rt_pool.clone()).with_notification_sink(Arc::new(notifications));
        let service = router(NoticeRestState::new(store, Some(verifier)));

        let manager_token = issue_token(
            private_pem.as_bytes(),
            public_key_pem.as_bytes(),
            manager,
            &["SUPER_ADMIN"],
        );
        let in_token = issue_token(
            private_pem.as_bytes(),
            public_key_pem.as_bytes(),
            member_in,
            &["ADMIN"],
        );
        let out_token = issue_token(
            private_pem.as_bytes(),
            public_key_pem.as_bytes(),
            member_out,
            &["ADMIN"],
        );

        // A typed, branch-scoped draft.
        let created = post_json(
            service.clone(),
            "/api/v1/notices",
            &manager_token,
            serde_json::json!({
                "title": "지사 안전교육 안내",
                "body": "대상 지사는 일정을 확인해 주세요.",
                "category": "legal",
                "audience": {"scope": "branches", "branch_ids": [branch_b]}
            }),
        )
        .await;
        assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.json);
        let notice_id = created.json["id"].as_str().unwrap().to_owned();
        assert_eq!(created.json["category"].as_str(), Some("legal"));
        assert_eq!(created.json["audience_scope"].as_str(), Some("branches"));
        assert_eq!(
            created.json["audience_branches"][0]["name"].as_str(),
            Some("부산지사")
        );
        assert!(created.json["progress"].is_object(), "{:?}", created.json);

        // Validation is fail-closed: unknown category, incoherent audience,
        // foreign branch id — all 422.
        for bad_body in [
            serde_json::json!({"title": "t", "body": "b", "category": "urgent"}),
            serde_json::json!({"title": "t", "body": "b",
                "audience": {"scope": "branches", "branch_ids": []}}),
            serde_json::json!({"title": "t", "body": "b",
                "audience": {"scope": "branches", "branch_ids": [uuid::Uuid::new_v4()]}}),
        ] {
            let rejected =
                post_json(service.clone(), "/api/v1/notices", &manager_token, bad_body).await;
            assert_eq!(
                rejected.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{:?}",
                rejected.json
            );
            assert_eq!(rejected.json["error"]["code"].as_str(), Some("validation"));
        }

        // Draft edit is manager-only (403 for a plain admin) and replaces the
        // audience whole.
        let denied_patch = patch_json(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}"),
            &in_token,
            serde_json::json!({"title": "탈취"}),
        )
        .await;
        assert_eq!(denied_patch.status, StatusCode::FORBIDDEN);

        let retargeted = patch_json(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}"),
            &manager_token,
            serde_json::json!({
                "category": "training",
                "audience": {"scope": "branches", "branch_ids": [branch_a]}
            }),
        )
        .await;
        assert_eq!(retargeted.status, StatusCode::OK, "{:?}", retargeted.json);
        assert_eq!(retargeted.json["category"].as_str(), Some("training"));
        assert_eq!(
            retargeted.json["audience_branches"][0]["name"].as_str(),
            Some("창원지사")
        );

        // Publishing to an audience with no members is a 422, not a silent
        // empty snapshot.
        let empty_draft = post_json(
            service.clone(),
            "/api/v1/notices",
            &manager_token,
            serde_json::json!({
                "title": "빈 대상", "body": "게시 불가",
                "audience": {"scope": "branches", "branch_ids": [branch_empty]}
            }),
        )
        .await;
        assert_eq!(empty_draft.status, StatusCode::CREATED);
        let empty_id = empty_draft.json["id"].as_str().unwrap();
        let empty_publish = post_json_empty(
            service.clone(),
            &format!("/api/v1/notices/{empty_id}/publish"),
            &manager_token,
        )
        .await;
        assert_eq!(
            empty_publish.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{:?}",
            empty_publish.json
        );

        // Publish; the audience is frozen afterwards (PATCH -> 409).
        let published = post_json_empty(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}/publish"),
            &manager_token,
        )
        .await;
        assert_eq!(published.status, StatusCode::OK, "{:?}", published.json);
        assert_eq!(published.json["progress"]["total"].as_i64(), Some(1));

        let frozen = patch_json(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}"),
            &manager_token,
            serde_json::json!({"title": "사후 수정"}),
        )
        .await;
        assert_eq!(frozen.status, StatusCode::CONFLICT, "{:?}", frozen.json);
        assert_eq!(frozen.json["error"]["code"].as_str(), Some("conflict"));

        // The audience member sees their own receipt state; a non-manager row
        // never carries progress. The out-of-audience member still reads the
        // published notice but has no receipt.
        let in_list = get_json(service.clone(), "/api/v1/notices", &in_token).await;
        let row = &in_list.json.as_array().unwrap()[0];
        assert!(row["my_receipt"].is_object(), "{row:?}");
        assert!(row["my_receipt"]["acknowledged_at"].is_null());
        assert!(row["progress"].is_null(), "{row:?}");

        let out_view = get_json(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}"),
            &out_token,
        )
        .await;
        assert_eq!(out_view.status, StatusCode::OK);
        assert!(out_view.json["my_receipt"].is_null());

        // 수령확인 is audience-bound: outsider 404 (never 403), member 204,
        // idempotent on repeat.
        let out_ack = post_json_empty(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}/ack"),
            &out_token,
        )
        .await;
        assert_eq!(out_ack.status, StatusCode::NOT_FOUND, "{:?}", out_ack.json);

        for _ in 0..2 {
            let in_ack = post_json_empty(
                service.clone(),
                &format!("/api/v1/notices/{notice_id}/ack"),
                &in_token,
            )
            .await;
            assert_eq!(in_ack.status, StatusCode::NO_CONTENT, "{:?}", in_ack.json);
        }

        // Receipts drill is manager-only; the outstanding filter is the chase
        // list.
        let denied_receipts = get_json(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}/receipts"),
            &in_token,
        )
        .await;
        assert_eq!(denied_receipts.status, StatusCode::FORBIDDEN);

        let receipts = get_json(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}/receipts"),
            &manager_token,
        )
        .await;
        assert_eq!(receipts.status, StatusCode::OK, "{:?}", receipts.json);
        assert_eq!(receipts.json["total"].as_i64(), Some(1));
        assert!(
            receipts.json["items"][0]["display_name"]
                .as_str()
                .unwrap()
                .starts_with("Member In")
        );
        assert!(receipts.json["items"][0]["acknowledged_at"].is_string());

        let outstanding = get_json(
            service.clone(),
            &format!("/api/v1/notices/{notice_id}/receipts?acknowledged=false"),
            &manager_token,
        )
        .await;
        assert_eq!(outstanding.status, StatusCode::OK);
        assert_eq!(outstanding.json["total"].as_i64(), Some(0));

        // Receipts and progress for a notice that does not exist are 404 even
        // for managers — never a fabricated empty page or 0/0.
        let ghost = uuid::Uuid::new_v4();
        let missing = get_json(
            service.clone(),
            &format!("/api/v1/notices/{ghost}/receipts"),
            &manager_token,
        )
        .await;
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
        let missing_progress = get_json(
            service.clone(),
            &format!("/api/v1/notices/{ghost}/progress"),
            &manager_token,
        )
        .await;
        assert_eq!(missing_progress.status, StatusCode::NOT_FOUND);
    })
    .await;
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

async fn get_json(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    request(service, "GET", uri, Some(token), None).await
}

async fn post_json_empty(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    request(service, "POST", uri, Some(token), None).await
}

async fn post_json(service: axum::Router, uri: &str, token: &str, body: Value) -> JsonResponse {
    request(service, "POST", uri, Some(token), Some(body)).await
}

async fn patch_json(service: axum::Router, uri: &str, token: &str, body: Value) -> JsonResponse {
    request(service, "PATCH", uri, Some(token), Some(body)).await
}

async fn request(
    service: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> JsonResponse {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request_body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    let response = service
        .oneshot(builder.body(request_body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    JsonResponse { status, json }
}

fn issue_token(
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    roles: &[&str],
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
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
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

async fn seed_branch(pool: &PgPool, name: &str) -> BranchId {
    let org_uuid = *OrgId::knl().as_uuid();
    let region: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("region-{name}"))
            .bind(org_uuid)
            .fetch_one(pool)
            .await
            .unwrap();
    BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region)
        .bind(name)
        .bind(org_uuid)
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}

async fn join_branch(pool: &PgPool, user: UserId, branch: BranchId) {
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(user.as_uuid())
        .bind(branch.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_user(pool: &PgPool, user_id: UserId, name: &str) {
    let name = name.to_owned();
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_user").unwrap(),
        "user",
        user_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(OrgId::knl());
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO users (id, display_name, roles, org_id, is_active) VALUES ($1, $2, $3, $4, true)",
            )
            .bind(user_id.as_uuid())
            .bind(format!("{name} {}", uuid::Uuid::new_v4()))
            .bind(Vec::from(["ADMIN"]))
            .bind(OrgId::knl().as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
}
