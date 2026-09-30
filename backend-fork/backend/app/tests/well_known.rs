#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Public native app-link association endpoints (`/.well-known/*`).
//!
//! Native passkeys are inert unless the platform serves a valid Apple App Site
//! Association and Android Digital Asset Links document at the exact dotted paths
//! over the RP origin. These tests assert both endpoints are mounted (no auth, no
//! DB), serve `application/json`, and carry the configured app identities — and
//! that an unconfigured deployment still serves a well-formed empty document.

use axum::body::Body;
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use http::{Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

const AASA_PATH: &str = "/.well-known/apple-app-site-association";
const ASSETLINKS_PATH: &str = "/.well-known/assetlinks.json";

async fn get_json(
    config: AppConfig,
    path: &str,
) -> Result<(StatusCode, Option<String>, Value), Box<dyn std::error::Error>> {
    let state = AppState::new(config, DatabaseDependency::NotConfigured)?;
    let response = build_router(state)
        .oneshot(Request::builder().uri(path).body(Body::empty())?)
        .await?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let json: Value = serde_json::from_slice(&body)?;
    Ok((status, content_type, json))
}

fn configured() -> Result<AppConfig, console_app::AppError> {
    AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        (
            "CONSOLE_IOS_APP_IDS",
            "ABCDE12345.com.knl.fsm, ABCDE12345.com.knl.fsm.dev".to_owned(),
        ),
        ("CONSOLE_ANDROID_PACKAGE", "com.knl.fsm".to_owned()),
        (
            "CONSOLE_ANDROID_CERT_SHA256",
            "AA:BB:CC:DD, 11:22:33:44".to_owned(),
        ),
    ])
}

#[tokio::test]
async fn aasa_serves_configured_ios_app_ids() -> Result<(), Box<dyn std::error::Error>> {
    let (status, content_type, json) = get_json(configured()?, AASA_PATH).await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("application/json"));
    let apps = json["webcredentials"]["apps"]
        .as_array()
        .expect("webcredentials.apps must be an array");
    let apps: Vec<&str> = apps.iter().filter_map(Value::as_str).collect();
    assert_eq!(
        apps,
        vec!["ABCDE12345.com.knl.fsm", "ABCDE12345.com.knl.fsm.dev"],
        "comma-separated CONSOLE_IOS_APP_IDS must be trimmed + split into the apps list"
    );
    Ok(())
}

#[tokio::test]
async fn assetlinks_serves_configured_android_package() -> Result<(), Box<dyn std::error::Error>> {
    let (status, content_type, json) = get_json(configured()?, ASSETLINKS_PATH).await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("application/json"));
    let entries = json.as_array().expect("assetlinks must be a JSON array");
    assert_eq!(entries.len(), 1, "one statement for the single package");
    let target = &entries[0]["target"];
    assert_eq!(target["namespace"], "android_app");
    assert_eq!(target["package_name"], "com.knl.fsm");
    let fingerprints: Vec<&str> = target["sha256_cert_fingerprints"]
        .as_array()
        .expect("sha256_cert_fingerprints must be an array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(fingerprints, vec!["AA:BB:CC:DD", "11:22:33:44"]);
    assert_eq!(
        entries[0]["relation"][0],
        "delegate_permission/common.get_login_creds"
    );
    Ok(())
}

#[tokio::test]
async fn well_known_endpoints_serve_empty_but_valid_documents_when_unconfigured()
-> Result<(), Box<dyn std::error::Error>> {
    let base = || {
        AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ])
    };

    let (aasa_status, aasa_ct, aasa_json) = get_json(base()?, AASA_PATH).await?;
    assert_eq!(aasa_status, StatusCode::OK);
    assert_eq!(aasa_ct.as_deref(), Some("application/json"));
    assert!(
        aasa_json["webcredentials"]["apps"]
            .as_array()
            .expect("apps array present even when unset")
            .is_empty(),
        "unset CONSOLE_IOS_APP_IDS yields an empty apps list, not an error"
    );

    let (links_status, links_ct, links_json) = get_json(base()?, ASSETLINKS_PATH).await?;
    assert_eq!(links_status, StatusCode::OK);
    assert_eq!(links_ct.as_deref(), Some("application/json"));
    assert!(
        links_json.as_array().expect("array present").is_empty(),
        "unset CONSOLE_ANDROID_PACKAGE yields an empty asset-links array"
    );
    Ok(())
}
