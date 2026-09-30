#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_platform_auth::{
    AndroidAssetLinksConfig, AppleAppSiteAssociationConfig, WELL_KNOWN_AASA_PATH,
    WELL_KNOWN_ASSETLINKS_PATH, android_assetlinks_json, apple_app_site_association_json,
};

#[test]
fn app_link_documents_render_static_passkey_metadata() {
    assert_eq!(
        WELL_KNOWN_AASA_PATH,
        "/.well-known/apple-app-site-association"
    );
    assert_eq!(WELL_KNOWN_ASSETLINKS_PATH, "/.well-known/assetlinks.json");

    let aasa = apple_app_site_association_json(AppleAppSiteAssociationConfig {
        app_ids: vec!["ABCDE12345.com.example.maintenance".to_owned()],
    })
    .unwrap();
    assert_eq!(
        aasa,
        r#"{"webcredentials":{"apps":["ABCDE12345.com.example.maintenance"]}}"#
    );

    let assetlinks = android_assetlinks_json(AndroidAssetLinksConfig {
        package_name: "com.example.maintenance".to_owned(),
        sha256_cert_fingerprints: vec![
            "12:34:56:78:90:AB:CD:EF:12:34:56:78:90:AB:CD:EF:12:34:56:78:90:AB:CD:EF:12:34:56:78:90:AB:CD:EF".to_owned(),
        ],
    })
    .unwrap();

    assert!(assetlinks.contains(r#""namespace":"android_app""#));
    assert!(assetlinks.contains(r#""package_name":"com.example.maintenance""#));
    assert!(assetlinks.contains(r#""delegate_permission/common.get_login_creds""#));
}
