#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! GATE tests for the cross-cutting tower layers on the FULLY-merged router
//! (issue #20). Proves that, after the fix, the merged domain routes actually
//! carry the body limit (and, by composition, the trace layer + timeout), while
//! the long-lived realtime route is reachable and the 16 MiB import allowance
//! still overrides the 2 MiB global default.
//!
//! The timeout-exclusion proof for the realtime route lives in the
//! `router_layer_tests` unit module in `lib.rs` (it asserts the merge-order
//! semantics directly); here we additionally confirm the realtime WS route is
//! mounted and reachable through the real `build_router`, even with a tiny
//! `CONSOLE_REQUEST_TIMEOUT_SECS`, i.e. nothing about the timeout blocks it.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::OrgId;
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const TWO_MIB: usize = 2 * 1024 * 1024;

fn keys() -> (String, String) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key
        .to_pkcs8_pem(LineEnding::LF)
        .unwrap()
        .to_string();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    (private_pem, public_pem)
}

fn app_state(pool: PgPool, public_key_pem: String) -> AppState {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
        // A tiny request timeout proves the realtime WS route below is reachable
        // regardless of the timeout (it is merged outside the timeout layer).
        ("CONSOLE_REQUEST_TIMEOUT_SECS", "1".to_owned()),
    ])
    .unwrap();
    AppState::new(config, DatabaseDependency::Postgres(pool)).unwrap()
}

async fn issue_token(
    pool: &PgPool,
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    role: &str,
) -> String {
    let branch =
        console_platform_test_support::seed_branch(pool, "Layer region", "Layer branch").await;
    let user = console_platform_test_support::seed_user(pool, "Layer actor", role, branch).await;
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
    console_platform_test_support::issue_session_token(
        pool,
        &issuer,
        AccessTokenInput {
            subject: user,
            org_id: OrgId::knl(),
            roles: vec![role.to_owned()],
            branches: vec![branch],
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
    .await
}

/// Body limit (2 MiB) is now applied to the FULLY-merged router, so a normal
/// JSON domain route rejects an oversized body with 413 — before the fix it was
/// applied only to the base router and the merged domain routes had NO limit.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn oversized_json_to_a_domain_route_is_rejected_413(pool: PgPool) {
    let (private_pem, public_pem) = keys();
    let token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_pem.as_bytes(),
        "ADMIN",
    )
    .await;
    let service = build_router(app_state(
        console_platform_test_support::runtime_role_pool(&pool).await,
        public_pem,
    ));

    // 3 MiB JSON body, well over the 2 MiB global default, to a normal domain
    // route (`POST /api/work-orders`). The body limit rejects it with 413
    // before the handler ever runs.
    let oversized = vec![b'a'; 3 * 1024 * 1024];
    let body = format!("{{\"x\":\"{}\"}}", String::from_utf8(oversized).unwrap());

    let response = service
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/work-orders")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "a >2 MiB JSON body to a merged domain route must be rejected 413 by the global body limit"
    );
}

/// A body just over the 2 MiB default but under the 16 MiB import allowance is
/// NOT rejected by the body limit on the import route: the per-route
/// `DefaultBodyLimit::max(16 MiB)` (applied deeper, in `registry/rest`) wins
/// over the 2 MiB global default applied on the merged router. The request is
/// allowed past the limit layer and fails later for a content reason (not 413).
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn import_route_keeps_16_mib_allowance_over_global_2_mib(pool: PgPool) {
    let (private_pem, public_pem) = keys();
    let token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_pem.as_bytes(),
        "SUPER_ADMIN",
    )
    .await;
    let service = build_router(app_state(
        console_platform_test_support::runtime_role_pool(&pool).await,
        public_pem,
    ));

    // 3 MiB upload: over the 2 MiB global default, under 16 MiB. If the global
    // limit applied here it would be 413; instead the per-route 16 MiB wins.
    let payload = vec![b'a'; 3 * 1024 * 1024];
    assert!(payload.len() > TWO_MIB);
    assert!(matches!(
        console_registry_adapter_postgres::parse_master_list_bytes(&payload),
        Err(console_registry_adapter_postgres::PgRegistryError::Workbook(_))
    ));

    let boundary = "X-BOUNDARY";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"m.xlsx\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&payload);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let response = service
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/equipment/import")
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

    assert_ne!(
        response.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "a 3 MiB upload to the import route must NOT be 413: its per-route 16 MiB limit overrides the 2 MiB global default"
    );
    // The inherited parser maps malformed workbooks to this opaque error.
    // Authentication/permission failure must not masquerade as limit proof.
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let error: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(error["error"]["code"], "internal");
    assert_eq!(error["error"]["message"], "registry request failed");
}

/// The realtime WS route is mounted and reachable through the real
/// `build_router` even with a 1s request timeout (it is merged OUTSIDE the
/// timeout layer). With JWT configured but no bearer header, the pre-upgrade
/// auth check returns 401 — proving the route is wired and not blocked by the
/// timeout layer.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn realtime_ws_route_is_reachable_under_a_short_timeout(pool: PgPool) {
    let (_private_pem, public_pem) = keys();
    let state = app_state(pool, public_pem);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, build_router(state)).await.unwrap();
    });

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(
            format!(
                "GET /api/v1/ws HTTP/1.1\r\n\
                 Host: {addr}\r\n\
                 Connection: Upgrade\r\n\
                 Upgrade: websocket\r\n\
                 Sec-WebSocket-Version: 13\r\n\
                 Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                 \r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut buf = vec![0_u8; 4096];
    let n = stream.read(&mut buf).await.unwrap();
    server.abort();
    let response = String::from_utf8_lossy(&buf[..n]);
    let status_line = response.lines().next().unwrap_or_default();

    assert!(
        status_line.starts_with("HTTP/1.1 401"),
        "realtime WS route must be mounted and reach its own auth check (401 without a bearer), \
         not be swallowed by the timeout layer; got: {status_line:?}"
    );
}
