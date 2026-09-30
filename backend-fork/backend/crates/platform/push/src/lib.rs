//! Push and Alimtalk provider adapters.
//!
//! FCM uses the HTTP v1 endpoint with short-lived OAuth2 access tokens from a
//! Google service-account JWT assertion. Solapi uses its HMAC-SHA256 API
//! authentication header and the common message send endpoint.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use console_kernel_core::{Clock, Timestamp};
use hmac::{Hmac, KeyInit, Mac};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use time::format_description::well_known::Rfc3339;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("invalid push configuration: {0}")]
    Config(String),

    #[error("provider request failed: {0}")]
    Http(String),

    #[error("provider returned {status}: {body}")]
    Provider { status: u16, body: String },

    #[error("serialization failed: {0}")]
    Serialize(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FcmConfig {
    pub project_id: String,
    pub client_email: String,
    pub private_key_pem: String,
    pub token_uri: String,
    pub scope: String,
}

impl FcmConfig {
    pub fn validate(&self) -> Result<(), PushError> {
        require_non_empty(&self.project_id, "FCM project id")?;
        require_non_empty(&self.client_email, "FCM client email")?;
        require_non_empty(&self.private_key_pem, "FCM private key")?;
        require_non_empty(&self.token_uri, "Google token URI")?;
        require_non_empty(&self.scope, "FCM OAuth scope")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolapiConfig {
    pub base_url: String,
    pub api_key: String,
    pub api_secret: String,
    pub from: String,
    pub pf_id: String,
    pub template_id: String,
}

impl SolapiConfig {
    pub fn validate(&self) -> Result<(), PushError> {
        require_non_empty(&self.base_url, "Solapi base URL")?;
        require_non_empty(&self.api_key, "Solapi API key")?;
        require_non_empty(&self.api_secret, "Solapi API secret")?;
        require_non_empty(&self.from, "Solapi sender number")?;
        require_non_empty(&self.pf_id, "Solapi pfId")?;
        require_non_empty(&self.template_id, "Solapi template id")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FcmPushMessage {
    pub token: String,
    pub title: String,
    pub body: String,
    pub data: BTreeMap<String, String>,
    /// Stable provider idempotency key (dispatch_id:alert_id). Carried into the
    /// FCM `data` payload so a retried delivery dedupes provider/client-side.
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlimtalkMessage {
    pub to: String,
    pub variables: BTreeMap<String, String>,
    /// Stable provider idempotency key (dispatch_id:alert_id).
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderMessageId(pub String);

pub trait PushNotifier: Send + Sync {
    fn send_fcm<'a>(
        &'a self,
        message: FcmPushMessage,
    ) -> BoxFuture<'a, Result<ProviderMessageId, PushError>>;

    fn send_alimtalk<'a>(
        &'a self,
        message: AlimtalkMessage,
    ) -> BoxFuture<'a, Result<ProviderMessageId, PushError>>;
}

#[derive(Debug, Clone)]
pub struct ProviderPushNotifier {
    fcm: Option<FcmHttpV1Client>,
    solapi: Option<SolapiAlimtalkClient>,
}

impl ProviderPushNotifier {
    #[must_use]
    pub fn new(fcm: Option<FcmHttpV1Client>, solapi: Option<SolapiAlimtalkClient>) -> Self {
        Self { fcm, solapi }
    }

    #[must_use]
    pub const fn has_fcm(&self) -> bool {
        self.fcm.is_some()
    }

    #[must_use]
    pub const fn has_alimtalk(&self) -> bool {
        self.solapi.is_some()
    }
}

impl PushNotifier for ProviderPushNotifier {
    fn send_fcm<'a>(
        &'a self,
        message: FcmPushMessage,
    ) -> BoxFuture<'a, Result<ProviderMessageId, PushError>> {
        Box::pin(async move {
            let fcm = self
                .fcm
                .as_ref()
                .ok_or_else(|| PushError::Config("FCM adapter is not configured".to_owned()))?;
            fcm.send(message).await
        })
    }

    fn send_alimtalk<'a>(
        &'a self,
        message: AlimtalkMessage,
    ) -> BoxFuture<'a, Result<ProviderMessageId, PushError>> {
        Box::pin(async move {
            let solapi = self.solapi.as_ref().ok_or_else(|| {
                PushError::Config("Solapi Alimtalk adapter is not configured".to_owned())
            })?;
            solapi.send(message).await
        })
    }
}

#[derive(Debug, Clone)]
pub struct FcmHttpV1Client {
    config: FcmConfig,
    client: reqwest::Client,
}

impl FcmHttpV1Client {
    pub fn new(config: FcmConfig) -> Result<Self, PushError> {
        config.validate()?;
        Ok(Self {
            config,
            client: reqwest::Client::new(),
        })
    }

    pub async fn send(&self, message: FcmPushMessage) -> Result<ProviderMessageId, PushError> {
        let access_token = self
            .access_token(console_kernel_core::SystemClock.now())
            .await?;
        let url = format!(
            "https://fcm.googleapis.com/v1/projects/{}/messages:send",
            self.config.project_id
        );
        // Carry the stable idempotency key into the data payload so a retried
        // delivery (e.g. after a worker crash) is deduplicated client-side.
        let mut data = message.data;
        data.insert("idempotency_key".to_owned(), message.idempotency_key);
        let body = serde_json::json!({
            "message": {
                "token": message.token,
                "notification": {
                    "title": message.title,
                    "body": message.body,
                },
                "data": data,
            }
        });
        let bytes =
            serde_json::to_vec(&body).map_err(|err| PushError::Serialize(err.to_string()))?;
        let response = self
            .client
            .post(url)
            .header(AUTHORIZATION, format!("Bearer {access_token}"))
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .body(bytes)
            .send()
            .await
            .map_err(reqwest_error)?;
        provider_message_id(response, "name").await
    }

    async fn access_token(&self, now: Timestamp) -> Result<String, PushError> {
        let iat = now.unix_timestamp();
        let exp = now
            .checked_add(time::Duration::minutes(55))
            .ok_or_else(|| PushError::Config("FCM token expiry overflows time".to_owned()))?
            .unix_timestamp();
        let claims = GoogleServiceAccountClaims {
            iss: self.config.client_email.as_str(),
            scope: self.config.scope.as_str(),
            aud: self.config.token_uri.as_str(),
            iat,
            exp,
        };
        let jwt = encode(
            &Header::new(Algorithm::RS256),
            &claims,
            &EncodingKey::from_rsa_pem(self.config.private_key_pem.as_bytes())
                .map_err(|err| PushError::Config(format!("invalid FCM private key: {err}")))?,
        )
        .map_err(|err| PushError::Config(format!("failed to sign FCM assertion: {err}")))?;
        let form = format!(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={jwt}"
        );
        let response = self
            .client
            .post(&self.config.token_uri)
            .header(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            )
            .body(form)
            .send()
            .await
            .map_err(reqwest_error)?;
        let body = success_body(response).await?;
        let token: GoogleTokenResponse =
            serde_json::from_slice(&body).map_err(|err| PushError::Serialize(err.to_string()))?;
        Ok(token.access_token)
    }
}

#[derive(Debug, Serialize)]
struct GoogleServiceAccountClaims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    iat: i64,
    exp: i64,
}

#[derive(Debug, Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
}

#[derive(Debug, Clone)]
pub struct SolapiAlimtalkClient {
    config: SolapiConfig,
    client: reqwest::Client,
}

impl SolapiAlimtalkClient {
    pub fn new(config: SolapiConfig) -> Result<Self, PushError> {
        config.validate()?;
        Ok(Self {
            config,
            client: reqwest::Client::new(),
        })
    }

    pub async fn send(&self, message: AlimtalkMessage) -> Result<ProviderMessageId, PushError> {
        let now = console_kernel_core::SystemClock.now();
        let auth =
            solapi_authorization_header(&self.config, now, uuid::Uuid::new_v4().to_string())?;
        let url = format!(
            "{}/messages/v4/send-many",
            self.config.base_url.trim_end_matches('/')
        );
        // Carry the stable idempotency key into the template variables so a
        // retried delivery is deduplicated on the provider side.
        let mut variables = message.variables;
        variables.insert("#{idempotency_key}".to_owned(), message.idempotency_key);
        let body = serde_json::json!({
            "messages": [{
                "to": message.to,
                "from": self.config.from,
                "kakaoOptions": {
                    "pfId": self.config.pf_id,
                    "templateId": self.config.template_id,
                    "variables": variables,
                }
            }]
        });
        let bytes =
            serde_json::to_vec(&body).map_err(|err| PushError::Serialize(err.to_string()))?;
        let response = self
            .client
            .post(url)
            .header(AUTHORIZATION, auth)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .body(bytes)
            .send()
            .await
            .map_err(reqwest_error)?;
        provider_message_id(response, "groupId").await
    }
}

pub fn solapi_authorization_header(
    config: &SolapiConfig,
    now: Timestamp,
    salt: String,
) -> Result<String, PushError> {
    config.validate()?;
    let date = now
        .format(&Rfc3339)
        .map_err(|err| PushError::Config(format!("failed to format Solapi date: {err}")))?;
    let payload = format!("{date}{salt}");
    let mut mac = Hmac::<Sha256>::new_from_slice(config.api_secret.as_bytes())
        .map_err(|err| PushError::Config(format!("invalid Solapi secret: {err}")))?;
    mac.update(payload.as_bytes());
    let signature = hex_lower(mac.finalize().into_bytes().as_slice());
    Ok(format!(
        "HMAC-SHA256 apiKey={}, date={date}, salt={salt}, signature={signature}",
        config.api_key
    ))
}

async fn provider_message_id(
    response: reqwest::Response,
    field: &str,
) -> Result<ProviderMessageId, PushError> {
    let body = success_body(response).await?;
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|err| PushError::Serialize(err.to_string()))?;
    let id = value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    Ok(ProviderMessageId(id))
}

async fn success_body(response: reqwest::Response) -> Result<Vec<u8>, PushError> {
    let status = response.status();
    let body = response.bytes().await.map_err(reqwest_error)?.to_vec();
    if status.is_success() {
        Ok(body)
    } else {
        Err(PushError::Provider {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }
}

fn reqwest_error(err: reqwest::Error) -> PushError {
    PushError::Http(err.to_string())
}

fn require_non_empty(value: &str, name: &str) -> Result<(), PushError> {
    if value.trim().is_empty() {
        Err(PushError::Config(format!("{name} is required")))
    } else {
        Ok(())
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    #[test]
    fn solapi_hmac_header_is_deterministic() {
        let config = SolapiConfig {
            base_url: "https://api.solapi.com".to_owned(),
            api_key: "key".to_owned(),
            api_secret: "secret".to_owned(),
            from: "0212345678".to_owned(),
            pf_id: "pf".to_owned(),
            template_id: "template".to_owned(),
        };

        let header = solapi_authorization_header(
            &config,
            datetime!(2026-06-12 09:00 UTC),
            "salt".to_owned(),
        )
        .unwrap();

        assert!(header.starts_with("HMAC-SHA256 apiKey=key, date=2026-06-12T09:00:00Z"));
        assert!(header.contains("salt=salt"));
        assert!(header.contains("signature="));
    }

    #[test]
    fn fcm_config_requires_credentials() {
        let config = FcmConfig {
            project_id: String::new(),
            client_email: "svc@example.invalid".to_owned(),
            private_key_pem: "key".to_owned(),
            token_uri: "https://oauth2.googleapis.com/token".to_owned(),
            scope: "https://www.googleapis.com/auth/firebase.messaging".to_owned(),
        };

        assert!(config.validate().is_err());
    }
}
