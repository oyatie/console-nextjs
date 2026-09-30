#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The v1 product has no Intelligence or model HTTP surface, including on loopback.

use axum::body::Body;
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use http::{Request, StatusCode};
use tower::ServiceExt;

const BIND: &str = "/internal/intelligence/bind";

async fn get(http_addr: Option<&str>, path: &str) -> StatusCode {
    let mut pairs = vec![("CONSOLE_APP_ROLE", AppRole::Api.to_string())];
    if let Some(addr) = http_addr {
        pairs.push(("CONSOLE_HTTP_ADDR", addr.to_owned()));
    }
    let config = AppConfig::from_pairs(pairs).unwrap();
    let state = AppState::new(config, DatabaseDependency::NotConfigured).unwrap();
    build_router(state)
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn get_internal_intelligence_bind_is_absent_on_loopback_listen_addr() {
    assert_eq!(get(Some("127.0.0.1:0"), BIND).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_internal_intelligence_bind_is_absent_when_listen_addr_is_non_loopback() {
    assert_eq!(get(Some("0.0.0.0:8080"), BIND).await, StatusCode::NOT_FOUND);
    assert_eq!(
        get(Some("10.0.0.1:8080"), BIND).await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn get_internal_intelligence_bind_is_absent_when_listen_addr_is_unset() {
    assert_eq!(get(None, BIND).await, StatusCode::NOT_FOUND);
}
