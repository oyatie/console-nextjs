use serde::Serialize;

use crate::AuthError;

pub const WELL_KNOWN_AASA_PATH: &str = "/.well-known/apple-app-site-association";
pub const WELL_KNOWN_ASSETLINKS_PATH: &str = "/.well-known/assetlinks.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleAppSiteAssociationConfig {
    pub app_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidAssetLinksConfig {
    pub package_name: String,
    pub sha256_cert_fingerprints: Vec<String>,
}

#[derive(Serialize)]
struct AppleAppSiteAssociation {
    webcredentials: AppleWebCredentials,
}

#[derive(Serialize)]
struct AppleWebCredentials {
    apps: Vec<String>,
}

#[derive(Serialize)]
struct AndroidAssetLink {
    relation: Vec<&'static str>,
    target: AndroidAssetLinkTarget,
}

#[derive(Serialize)]
struct AndroidAssetLinkTarget {
    namespace: &'static str,
    package_name: String,
    sha256_cert_fingerprints: Vec<String>,
}

pub fn apple_app_site_association_json(
    config: AppleAppSiteAssociationConfig,
) -> Result<String, AuthError> {
    Ok(serde_json::to_string(&AppleAppSiteAssociation {
        webcredentials: AppleWebCredentials {
            apps: config.app_ids,
        },
    })?)
}

pub fn android_assetlinks_json(config: AndroidAssetLinksConfig) -> Result<String, AuthError> {
    Ok(serde_json::to_string(&[AndroidAssetLink {
        relation: vec!["delegate_permission/common.get_login_creds"],
        target: AndroidAssetLinkTarget {
            namespace: "android_app",
            package_name: config.package_name,
            sha256_cert_fingerprints: config.sha256_cert_fingerprints,
        },
    }])?)
}
