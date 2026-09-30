//! Proves the dev-auth route is ABSENT from a default-features build.
//!
//! Cargo features are crate-wide, not per-test-binary, so this file must be
//! gated OUT whenever the crate IS built with `dev-auth` (otherwise this
//! exact test would run against a build where the route legitimately exists,
//! and fail) — the complement of `dev_auth_session.rs`'s
//! `#![cfg(feature = "dev-auth")]`. The meaningful run is the plain
//! `cargo test -p console-platform-auth-rest` (no `--features dev-auth`), which
//! is exactly the build every production image ships.
//! `console-gate-dev-auth-absence` proves the same fact at the `cargo metadata`
//! level (the feature graph); this test proves it at the HTTP-routing level
//! (the literal path 404s).
#![cfg(not(feature = "dev-auth"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use console_platform_auth_rest::{AuthRestState, router};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

#[tokio::test]
async fn dev_auth_session_route_is_absent_without_the_feature() {
    // An unmatched axum route 404s during dispatch, before any handler (and
    // therefore before any pool use) runs, so `connect_lazy` — which opens no
    // real connection until first query — is all a pure routing test needs.
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://console_rt:console_rt@localhost/console_dev_auth_absence_probe")
        .unwrap();
    let app = router(AuthRestState::disabled(pool));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/dev-auth/session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
