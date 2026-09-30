//! SMTP MIME preparation and connection probe for webmail.
//!
//! Implements the [`SmtpSender`] port over `lettre`'s tokio + rustls transport:
//!
//! * **SSRF guard** ([`ssrf`]): before any connect, the host is resolved once
//!   via `hickory-resolver`, every resolved IP is denylist-checked, and the
//!   first allowed IP is PINNED for the dial (DNS-rebinding safe). Only the mail
//!   submission port allowlist (587 / 465) is accepted (the application layer
//!   already validates this; we re-check defensively). Port 25 is NOT allowed —
//!   webmail performs only authenticated submission.
//! * **TLS enforced**: `SslTls` uses implicit TLS (`relay`, SMTPS/465);
//!   `StartTls` uses STARTTLS (`starttls_relay`, 587). There is no plaintext
//!   path — `MailSecurity` has no plaintext variant. Certificate verification is
//!   lettre's rustls default (the webpki roots); we never install a permissive
//!   verifier. The pinned-IP dial keeps SNI set to the ORIGINAL hostname so cert
//!   verification still matches the certificate's name.
//! * **MIME build**: To/Cc/Subject/body + attachments, with `In-Reply-To` /
//!   `References` for reply/forward. Bcc stays in the separate envelope.
//!
//! No send method is exposed. Prepared bytes cannot leave until a future
//! two-site-confirmed, epoch-fenced claim owns external egress.
//!
//! No secret, recipient, body, or host is logged; transport failures map to a
//! fixed, non-secret error code.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod ssrf;

use std::str::FromStr;

use console_comms_application::{
    ALLOWED_SMTP_PORTS, MailServiceError, PreparedFormat, PreparedOutbound, SendMessageCommand,
    SmtpSender, SmtpTransportConfig, TestConnectionResult,
};
use console_comms_domain::{MailSecurity, MessageAddress};
use hickory_resolver::Resolver;
use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, MessageBuilder, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, Message, Tokio1Executor};
use secrecy::ExposeSecret;

use crate::ssrf::SsrfError;

/// The outbound SMTP sender. Stateless: each call resolves + validates the host
/// and builds a one-shot transport, so a credential or host change always takes
/// effect immediately and no secret is cached across sends.
#[derive(Debug, Default, Clone)]
pub struct LettreMailSender;

impl LettreMailSender {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Resolve + validate the host and build the TLS-pinned transport.
    async fn build_transport(
        config: &SmtpTransportConfig,
    ) -> Result<AsyncSmtpTransport<Tokio1Executor>, MailServiceError> {
        // Defense-in-depth: re-assert the mail-port allowlist at the transport.
        if !ALLOWED_SMTP_PORTS.contains(&config.port) {
            return Err(MailServiceError::Transport {
                code: "port_not_allowed",
            });
        }

        let pinned = resolve_and_pin(&config.host)
            .await
            .map_err(|err| MailServiceError::Transport { code: err.code() })?;

        // SNI / certificate name = the ORIGINAL hostname, so cert verification
        // still matches even though we dial the pinned IP. lettre's default
        // verifier (rustls + webpki roots) is used — no permissive override.
        let tls_params = TlsParameters::builder(config.host.clone())
            .build()
            .map_err(|_| MailServiceError::Transport { code: "tls_setup" })?;

        // Dial the pinned IP literal, not the hostname, so no second DNS lookup
        // can swap in a private address between the check and the connect.
        let pinned_host = pinned.to_string();
        let builder = match config.security {
            // Implicit TLS on connect (SMTPS / 465).
            MailSecurity::SslTls => AsyncSmtpTransport::<Tokio1Executor>::relay(&pinned_host)
                .map_err(|_| MailServiceError::Transport { code: "tls_setup" })?
                .tls(Tls::Wrapper(tls_params)),
            // Upgrade a plaintext connection via STARTTLS (587). lettre's
            // `Tls::Required` rejects a server that will not negotiate TLS, so
            // there is no silent plaintext fallback.
            MailSecurity::StartTls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&pinned_host)
                    .map_err(|_| MailServiceError::Transport { code: "tls_setup" })?
                    .tls(Tls::Required(tls_params))
            }
        };

        let transport = builder
            .port(config.port)
            .credentials(Credentials::new(
                config.username.clone(),
                config.password.expose_secret().to_owned(),
            ))
            .build();
        Ok(transport)
    }
}

impl SmtpSender for LettreMailSender {
    fn prepare(
        &self,
        config: &SmtpTransportConfig,
        message: &SendMessageCommand,
        from_address: &str,
    ) -> Result<PreparedOutbound, MailServiceError> {
        let rfc_message_id = generate_message_id(from_address);
        let email = build_message(config, message, from_address, &rfc_message_id)?;
        Ok(PreparedOutbound {
            format: PreparedFormat::SmtpMime,
            bytes: email.formatted(),
            rfc_message_id: Some(rfc_message_id),
        })
    }

    fn test_connection<'a>(
        &'a self,
        config: &'a SmtpTransportConfig,
    ) -> console_comms_application::MailFuture<'a, Result<TestConnectionResult, MailServiceError>>
    {
        Box::pin(async move {
            let transport = match Self::build_transport(config).await {
                Ok(t) => t,
                Err(MailServiceError::Transport { code }) => {
                    return Ok(TestConnectionResult {
                        ok: false,
                        error_code: Some(code.to_owned()),
                    });
                }
                Err(other) => return Err(other),
            };
            match transport.test_connection().await {
                Ok(true) => Ok(TestConnectionResult {
                    ok: true,
                    error_code: None,
                }),
                Ok(false) | Err(_) => Ok(TestConnectionResult {
                    ok: false,
                    // A fixed, non-secret token: never echo the raw rustls/SMTP
                    // string (it can carry host detail or auth specifics).
                    error_code: Some("connect_failed".to_owned()),
                }),
            }
        })
    }
}

/// Resolve `host` once and return the single pinned IP to dial. A host that is
/// itself an IP literal is validated directly (no DNS).
async fn resolve_and_pin(host: &str) -> Result<std::net::IpAddr, SsrfError> {
    let host = host.trim();
    if host.is_empty() {
        return Err(SsrfError::InvalidHost);
    }
    // An IP literal: validate it directly; do not resolve.
    if let Ok(ip) = std::net::IpAddr::from_str(host) {
        return ssrf::pick_pinned_ip(&[ip]);
    }

    let resolver = Resolver::builder_tokio()
        .map_err(|_| SsrfError::Unresolvable)?
        .build()
        .map_err(|_| SsrfError::Unresolvable)?;
    let lookup = resolver
        .lookup_ip(host)
        .await
        .map_err(|_| SsrfError::Unresolvable)?;
    let addresses: Vec<std::net::IpAddr> = lookup.iter().collect();
    ssrf::pick_pinned_ip(&addresses)
}

/// Build the RFC 5322 message: From (the account address), To/Cc, Subject,
/// body, attachments, and the reply/forward threading headers.
/// Mint an RFC 5322 Message-ID of the form `<uuid@domain>` from the sender's
/// address domain (falling back to `localhost`).
fn generate_message_id(from_address: &str) -> String {
    let domain = from_address
        .rsplit_once('@')
        .map(|(_, domain)| domain.trim())
        .filter(|domain| !domain.is_empty())
        .unwrap_or("localhost");
    format!("<{}@{}>", uuid::Uuid::new_v4(), domain)
}

fn build_message(
    config: &SmtpTransportConfig,
    command: &SendMessageCommand,
    from_address: &str,
    rfc_message_id: &str,
) -> Result<Message, MailServiceError> {
    let from = mailbox(from_address, config.from_name.as_deref())?;

    let mut builder: MessageBuilder = Message::builder()
        .from(from)
        .subject(&command.subject)
        .message_id(Some(rfc_message_id.to_owned()));

    for addr in &command.to {
        builder = builder.to(to_mailbox(addr)?);
    }
    for addr in &command.cc {
        builder = builder.cc(to_mailbox(addr)?);
    }
    if let Some(in_reply_to) = &command.in_reply_to {
        builder = builder.in_reply_to(in_reply_to.clone());
    }
    if !command.references.is_empty() {
        builder = builder.references(command.references.join(" "));
    }

    let body_part = SinglePart::builder()
        .header(ContentType::TEXT_PLAIN)
        .body(command.body_text.clone());

    let email = if command.attachments.is_empty() {
        builder
            .singlepart(body_part)
            .map_err(|_| MailServiceError::Transport {
                code: "build_failed",
            })?
    } else {
        let mut multipart = MultiPart::mixed().singlepart(body_part);
        for att in &command.attachments {
            // An unparseable content-type is a bad request, not something to
            // silently coerce — surface it as a transport/build error.
            let content_type =
                ContentType::parse(&att.content_type).map_err(|_| MailServiceError::Transport {
                    code: "bad_content_type",
                })?;
            let part = Attachment::new(att.filename.clone()).body(att.bytes.clone(), content_type);
            multipart = multipart.singlepart(part);
        }
        builder
            .multipart(multipart)
            .map_err(|_| MailServiceError::Transport {
                code: "build_failed",
            })?
    };
    Ok(email)
}

fn mailbox(address: &str, name: Option<&str>) -> Result<Mailbox, MailServiceError> {
    let parsed = address
        .parse::<lettre::Address>()
        .map_err(|_| MailServiceError::Transport {
            code: "bad_address",
        })?;
    Ok(Mailbox::new(name.map(ToOwned::to_owned), parsed))
}

fn to_mailbox(addr: &MessageAddress) -> Result<Mailbox, MailServiceError> {
    mailbox(&addr.address, addr.name.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_comms_application::SendKind;
    use console_kernel_core::{TraceContext, UserId};
    use secrecy::SecretString;
    use time::OffsetDateTime;

    fn config() -> SmtpTransportConfig {
        SmtpTransportConfig {
            host: "smtp.example.com".to_owned(),
            port: 587,
            security: MailSecurity::StartTls,
            username: "ops".to_owned(),
            password: SecretString::from("secret"),
            from_address: "ops@knl.example".to_owned(),
            from_name: Some("KNL Ops".to_owned()),
        }
    }

    fn command() -> SendMessageCommand {
        SendMessageCommand {
            operation_id: uuid::Uuid::new_v4(),
            actor: UserId::new(),
            kind: SendKind::New,
            to: vec![MessageAddress::new("a@b.com").unwrap()],
            cc: vec![MessageAddress::new("c@d.com").unwrap()],
            bcc: vec![],
            subject: "Quote".to_owned(),
            body_text: "Hello there".to_owned(),
            attachments: vec![],
            in_reply_to: None,
            references: vec![],
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn generate_message_id_uses_sender_domain() {
        let id = generate_message_id("ops@knl.example");
        assert!(id.starts_with('<') && id.ends_with('>'), "got {id:?}");
        assert!(id.contains("@knl.example>"));
        // A malformed sender still yields a well-formed id.
        assert!(generate_message_id("no-domain").ends_with("@localhost>"));
    }

    #[test]
    fn builds_message_with_from_to_cc_and_message_id() {
        let mid = generate_message_id("ops@knl.example");
        let email = build_message(&config(), &command(), "ops@knl.example", &mid).unwrap();
        let formatted = email.formatted();
        let raw = String::from_utf8_lossy(&formatted);
        assert!(raw.contains("From:"));
        assert!(raw.contains("To:"));
        assert!(raw.contains("Cc:"));
        assert!(raw.contains("Subject:"));
        assert!(
            raw.contains(&mid),
            "the chosen Message-ID must be on the wire"
        );
    }

    #[test]
    fn reply_sets_in_reply_to_and_references() {
        let mut cmd = command();
        cmd.kind = SendKind::Reply;
        cmd.in_reply_to = Some("<orig@host>".to_owned());
        cmd.references = vec!["<a@h>".to_owned(), "<orig@host>".to_owned()];
        let mid = generate_message_id("ops@knl.example");
        let email = build_message(&config(), &cmd, "ops@knl.example", &mid).unwrap();
        let formatted = email.formatted();
        let raw = String::from_utf8_lossy(&formatted);
        assert!(raw.contains("In-Reply-To: <orig@host>"));
        assert!(raw.contains("References:"));
    }

    #[test]
    fn attachment_produces_multipart() {
        let mut cmd = command();
        cmd.attachments = vec![console_comms_application::OutboundAttachment {
            filename: "quote.pdf".to_owned(),
            content_type: "application/pdf".to_owned(),
            bytes: b"%PDF-1.4 test".to_vec(),
        }];
        let mid = generate_message_id("ops@knl.example");
        let email = build_message(&config(), &cmd, "ops@knl.example", &mid).unwrap();
        let formatted = email.formatted();
        let body = String::from_utf8_lossy(&formatted);
        assert!(body.contains("multipart/mixed"));
        assert!(body.contains("quote.pdf"));
    }

    #[test]
    fn prepared_mime_never_exposes_bcc_header_or_address() {
        let mut cmd = command();
        cmd.bcc = vec![MessageAddress::new("private@example.com").unwrap()];
        let prepared = LettreMailSender::new()
            .prepare(&config(), &cmd, "ops@knl.example")
            .unwrap();
        let mime = String::from_utf8_lossy(&prepared.bytes);
        assert!(!mime.contains("Bcc:"));
        assert!(!mime.contains("private@example.com"));
        assert_eq!(prepared.format, PreparedFormat::SmtpMime);
    }

    #[tokio::test]
    async fn resolve_and_pin_rejects_loopback_literal() {
        assert_eq!(
            resolve_and_pin("127.0.0.1").await,
            Err(SsrfError::DisallowedAddress)
        );
    }

    #[tokio::test]
    async fn resolve_and_pin_rejects_metadata_literal() {
        assert_eq!(
            resolve_and_pin("169.254.169.254").await,
            Err(SsrfError::DisallowedAddress)
        );
    }

    #[tokio::test]
    async fn resolve_and_pin_accepts_public_literal() {
        assert_eq!(
            resolve_and_pin("8.8.8.8").await.unwrap(),
            "8.8.8.8".parse::<std::net::IpAddr>().unwrap()
        );
    }
}
