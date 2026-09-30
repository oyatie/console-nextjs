//! Outbound email provider adapter.
//!
//! Sends transactional email (e.g. the open-signup OTP) over SMTP via an
//! authenticated STARTTLS relay — the OCI Email Delivery endpoint in production.
//! TLS is provided by `lettre`' bundled rustls backend (the workspace ships no
//! other TLS stack: `reqwest` is built `default-features = false`), so this crate
//! pulls `lettre` with `tokio1-rustls-tls` and nothing system-native.
//!
//! Mirrors the provider-adapter shape of `console-platform-push`: a `*Config` with a
//! `validate()`, an async sender trait, a live adapter, and explicit non-prod
//! stub/disabled senders for local/e2e safety.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::fmt::{Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;

use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    #[error("invalid email configuration: {0}")]
    Config(String),

    #[error("failed to build email message: {0}")]
    Message(String),

    #[error("SMTP delivery failed: {0}")]
    Transport(String),
}

/// SMTP relay configuration for outbound transactional email.
///
/// In production these point at the OCI Email Delivery STARTTLS relay; the
/// `username`/`password` are the SMTP credentials issued for the approved
/// sender. Every field is required — a partially-configured relay is a
/// misconfiguration, surfaced by [`SmtpEmailConfig::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpEmailConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from_address: String,
    pub from_name: String,
}

impl SmtpEmailConfig {
    pub fn validate(&self) -> Result<(), EmailError> {
        require_non_empty(&self.host, "SMTP host")?;
        if self.port == 0 {
            return Err(EmailError::Config("SMTP port is required".to_owned()));
        }
        require_non_empty(&self.username, "SMTP username")?;
        require_non_empty(&self.password, "SMTP password")?;
        require_non_empty(&self.from_address, "email from address")?;
        require_non_empty(&self.from_name, "email from name")
    }
}

/// Outbound transactional email port. Async by way of `BoxFuture` so the trait
/// stays object-safe behind `Arc<dyn EmailSender>` (mirrors `PushNotifier`).
pub trait EmailSender: Send + Sync {
    /// Send a one-time-passcode email to `to`. `ttl` is rendered into the body so
    /// the recipient knows how long the code is valid.
    fn send_otp<'a>(
        &'a self,
        to: &'a str,
        code: &'a str,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<(), EmailError>>;
}

/// Explicit non-production modes that are allowed to use the OTP-logging stub.
///
/// Production must use [`LettreSmtpSender`] or a fail-closed sender; constructing
/// [`StubEmailSender`] requires choosing one of these modes at the app boundary so
/// OTP logging is auditable and cannot be reached by accidental missing SMTP
/// configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StubEmailMode {
    Local,
    Development,
    Test,
    E2e,
}

impl StubEmailMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Development => "development",
            Self::Test => "test",
            Self::E2e => "e2e",
        }
    }
}

impl Display for StubEmailMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "unsupported email stub mode {value:?}; expected one of local, dev, development, test, or e2e"
)]
pub struct StubEmailModeParseError {
    value: String,
}

impl FromStr for StubEmailMode {
    type Err = StubEmailModeParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "local" => Ok(Self::Local),
            "dev" | "development" => Ok(Self::Development),
            "test" => Ok(Self::Test),
            "e2e" => Ok(Self::E2e),
            _ => Err(StubEmailModeParseError {
                value: value.to_owned(),
            }),
        }
    }
}

/// Live SMTP sender over an authenticated STARTTLS relay (OCI Email Delivery).
#[derive(Clone)]
pub struct LettreSmtpSender {
    config: SmtpEmailConfig,
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl LettreSmtpSender {
    pub fn new(config: SmtpEmailConfig) -> Result<Self, EmailError> {
        config.validate()?;
        let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)
            .map_err(|err| EmailError::Config(err.to_string()))?
            .port(config.port)
            .credentials(Credentials::new(
                config.username.clone(),
                config.password.clone(),
            ))
            .build();
        Ok(Self { config, transport })
    }
}

impl EmailSender for LettreSmtpSender {
    fn send_otp<'a>(
        &'a self,
        to: &'a str,
        code: &'a str,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<(), EmailError>> {
        Box::pin(async move {
            let from = format!("{} <{}>", self.config.from_name, self.config.from_address);
            let message =
                Message::builder()
                    .from(from.parse().map_err(|err| {
                        EmailError::Message(format!("invalid from address: {err}"))
                    })?)
                    .to(to.parse().map_err(|err| {
                        EmailError::Message(format!("invalid recipient address: {err}"))
                    })?)
                    .subject("Your verification code")
                    .body(otp_body(code, ttl))
                    .map_err(|err| EmailError::Message(err.to_string()))?;
            self.transport
                .send(message)
                .await
                .map_err(|err| EmailError::Transport(err.to_string()))?;
            Ok(())
        })
    }
}

/// Fail-closed sender used when neither live SMTP nor explicit stub mode is
/// configured. It never logs OTPs; callers get a configuration error instead.
#[derive(Debug, Clone, Default)]
pub struct DisabledEmailSender;

impl EmailSender for DisabledEmailSender {
    fn send_otp<'a>(
        &'a self,
        _to: &'a str,
        _code: &'a str,
        _ttl: Duration,
    ) -> BoxFuture<'a, Result<(), EmailError>> {
        Box::pin(async move {
            Err(EmailError::Config(
                "outbound OTP email is disabled because neither live SMTP config nor explicit non-production stub mode is configured"
                    .to_owned(),
            ))
        })
    }
}

/// Stub sender for explicit non-production dev/e2e/test flows. Logs the OTP via
/// `tracing` and returns `Ok(())` — it NEVER sends mail.
#[derive(Debug, Clone)]
pub struct StubEmailSender {
    mode: StubEmailMode,
}

impl StubEmailSender {
    #[must_use]
    pub const fn new(mode: StubEmailMode) -> Self {
        Self { mode }
    }

    #[must_use]
    pub const fn mode(&self) -> StubEmailMode {
        self.mode
    }
}

impl EmailSender for StubEmailSender {
    fn send_otp<'a>(
        &'a self,
        to: &'a str,
        code: &'a str,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<(), EmailError>> {
        Box::pin(async move {
            tracing::info!(
                target: "console::email",
                email_stub_mode = %self.mode,
                "[NONPROD:{}] OTP for {to}: {code} (ttl {ttl:?})",
                self.mode
            );
            Ok(())
        })
    }
}

fn otp_body(code: &str, ttl: Duration) -> String {
    let minutes = ttl.as_secs() / 60;
    format!(
        "Your verification code is {code}.\n\nIt expires in {minutes} minutes.\n\nIf you did not request this, you can ignore this email.\n"
    )
}

fn require_non_empty(value: &str, name: &str) -> Result<(), EmailError> {
    if value.trim().is_empty() {
        Err(EmailError::Config(format!("{name} is required")))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> SmtpEmailConfig {
        SmtpEmailConfig {
            host: "smtp.email.ap-chuncheon-1.oci.oraclecloud.com".to_owned(),
            port: 587,
            username: "ocid1.user.oc1..example".to_owned(),
            password: "secret".to_owned(),
            from_address: "noreply@example.com".to_owned(),
            from_name: "Console".to_owned(),
        }
    }

    #[test]
    fn valid_config_passes_validation() {
        assert!(valid_config().validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_host() {
        let mut config = valid_config();
        config.host = String::new();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_zero_port() {
        let mut config = valid_config();
        config.port = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_blank_from_name() {
        let mut config = valid_config();
        config.from_name = "   ".to_owned();
        assert!(config.validate().is_err());
    }

    #[test]
    fn stub_mode_parses_only_explicit_non_production_values() {
        assert_eq!(
            "local".parse::<StubEmailMode>().unwrap(),
            StubEmailMode::Local
        );
        assert_eq!(
            "dev".parse::<StubEmailMode>().unwrap(),
            StubEmailMode::Development
        );
        assert_eq!(
            "development".parse::<StubEmailMode>().unwrap(),
            StubEmailMode::Development
        );
        assert_eq!(
            "test".parse::<StubEmailMode>().unwrap(),
            StubEmailMode::Test
        );
        assert_eq!("e2e".parse::<StubEmailMode>().unwrap(), StubEmailMode::E2e);
        assert!("production".parse::<StubEmailMode>().is_err());
        assert!("true".parse::<StubEmailMode>().is_err());
    }

    #[tokio::test]
    async fn stub_sender_never_fails_and_does_not_send() {
        let sender = StubEmailSender::new(StubEmailMode::Test);
        let result = sender
            .send_otp("ops@example.com", "123456", Duration::from_secs(300))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn disabled_sender_fails_closed_without_logging_otp() {
        let sender = DisabledEmailSender;
        let result = sender
            .send_otp("ops@example.com", "123456", Duration::from_secs(300))
            .await;
        assert!(matches!(result, Err(EmailError::Config(message)) if message.contains("disabled")));
    }
}
