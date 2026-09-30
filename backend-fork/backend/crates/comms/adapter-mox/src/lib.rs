//! mox mail-server transport adapter for webmail.
//!
//! Legacy mox integration for webmail. Outbound preparation fails closed: its
//! combined form carries Bcc in the protocol body and has no fenced egress.
//! The only network request in this adapter is the authenticated connection
//! probe. The selected v1 mail server is Stalwart.
//!
//! * **Inbound delivery** ([`Incoming`]) is mox's webhook payload for an arriving
//!   message. [`Incoming::to_fetched_message`] maps it into the mail domain's
//!   [`FetchedMessage`] so the REST webhook receiver can UPSERT it into the read
//!   model through the existing inbound store — no IMAP poll needed for new mail.
//!
//! Which path each operation uses:
//!   * send / reply / forward → unarmed
//!   * new inbound / read-model ingest → mox webhook       — this crate + rest
//!   * folder/backfill IMAP sync against mox               → DEFERRED (mox speaks
//!     IMAP4rev2, but its localserve dev ports (1143/1993) sit outside the app's
//!     IMAP port allowlist + TLS enforcement; wiring that is a later slice).
//!
//! # TLS
//! The workspace `reqwest` ships with no TLS backend, so this adapter speaks
//! plaintext HTTP to a trusted, network-local mox (the dev-stack default and the
//! in-cluster prod topology where mox is not internet-exposed). An HTTPS base
//! URL needs a `reqwest` rustls feature — deferred with the prod k8s manifests.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_comms_application::{
    FetchedMessage, MailFuture, MailServiceError, PreparedOutbound, SendMessageCommand, SmtpSender,
    SmtpTransportConfig, TestConnectionResult,
};
use console_comms_domain::MessageAddress;
use secrecy::ExposeSecret;
use serde::Deserialize;
use time::OffsetDateTime;

/// The mox webapi transport. `base_url` is the trusted mox origin (e.g.
/// `http://mox:1080` in the dev stack); the per-tenant login (mox account) is
/// carried on the [`SmtpTransportConfig`] as the SMTP username/password.
#[derive(Debug, Clone)]
pub struct MoxWebapiSender {
    base_url: String,
    client: reqwest::Client,
}

impl MoxWebapiSender {
    /// Build a sender for the mox instance at `base_url` (scheme + host + port,
    /// no trailing slash required).
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            client: reqwest::Client::new(),
        }
    }
}

impl SmtpSender for MoxWebapiSender {
    fn prepare(
        &self,
        _config: &SmtpTransportConfig,
        _message: &SendMessageCommand,
        _from_address: &str,
    ) -> Result<PreparedOutbound, MailServiceError> {
        // The legacy mox form mixes Bcc into the protocol body and lacks a
        // separately persisted envelope. This transport remains unarmed.
        Err(MailServiceError::Transport {
            code: "mox_submission_unarmed",
        })
    }

    fn test_connection<'a>(
        &'a self,
        config: &'a SmtpTransportConfig,
    ) -> MailFuture<'a, Result<TestConnectionResult, MailServiceError>> {
        Box::pin(async move {
            // Reachability + auth probe: a basic-auth GET to the webapi base.
            // mox answers authenticated requests here with 2xx; wrong credentials
            // get 401/403 (still a successful HTTP round-trip, just not authed) —
            // only a genuine 2xx counts as `ok`. A transport error means the
            // server is unreachable.
            // ponytail: reachability probe, not a deep credential check — the
            // webapi has no dedicated no-op auth endpoint and we must not send.
            let resp = self
                .client
                .get(format!("{}/webapi/v0/", self.base_url))
                .basic_auth(&config.username, Some(config.password.expose_secret()))
                .send()
                .await;
            match resp {
                Ok(resp) if resp.status().is_success() => Ok(TestConnectionResult {
                    ok: true,
                    error_code: None,
                }),
                Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED => {
                    Ok(TestConnectionResult {
                        ok: false,
                        error_code: Some("auth_failed".to_owned()),
                    })
                }
                Ok(_) => Ok(TestConnectionResult {
                    ok: false,
                    error_code: Some("connect_failed".to_owned()),
                }),
                Err(_) => Ok(TestConnectionResult {
                    ok: false,
                    error_code: Some("connect_failed".to_owned()),
                }),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// mox webhook wire types (Incoming delivery)
// ---------------------------------------------------------------------------

/// mox's `Incoming` webhook payload for an arriving message. Only the fields the
/// read-model ingest needs are decoded; unknown fields are ignored.
#[derive(Debug, Deserialize)]
pub struct Incoming {
    #[serde(rename = "From", default)]
    pub from: Vec<NameAddressPub>,
    #[serde(rename = "To", default)]
    pub to: Vec<NameAddressPub>,
    #[serde(rename = "CC", default)]
    pub cc: Vec<NameAddressPub>,
    #[serde(rename = "Subject", default)]
    pub subject: String,
    #[serde(rename = "MessageID", default)]
    pub message_id: String,
    #[serde(rename = "InReplyTo", default)]
    pub in_reply_to: String,
    #[serde(rename = "References", default)]
    pub references: Vec<String>,
    #[serde(rename = "Text", default)]
    pub text: String,
    #[serde(rename = "HTML", default)]
    pub html: String,
    #[serde(rename = "Meta")]
    pub meta: IncomingMeta,
}

/// The public wire addressee (mirrors [`NameAddress`] but exported for the
/// webhook payload).
#[derive(Debug, Deserialize)]
pub struct NameAddressPub {
    #[serde(rename = "Name", default)]
    pub name: String,
    #[serde(rename = "Address", default)]
    pub address: String,
}

/// mox `IncomingMeta`: storage + SMTP envelope details.
#[derive(Debug, Deserialize)]
pub struct IncomingMeta {
    /// mox's internal per-account message id (stable across webhook redelivery).
    #[serde(rename = "MsgID")]
    pub msg_id: i64,
    /// The SMTP `RCPT TO` — the local recipient this delivery is for.
    #[serde(rename = "RcptTo", default)]
    pub rcpt_to: String,
    /// The destination mailbox name (defaults to `Inbox`).
    #[serde(rename = "MailboxName", default)]
    pub mailbox_name: String,
}

impl Incoming {
    /// The local recipient address this delivery landed for. Prefers the SMTP
    /// envelope `RcptTo`; falls back to the first `To` header address.
    #[must_use]
    pub fn recipient_address(&self) -> Option<String> {
        let rcpt = self.meta.rcpt_to.trim();
        if !rcpt.is_empty() {
            return Some(rcpt.to_owned());
        }
        self.to.first().map(|a| a.address.clone())
    }

    /// The destination mailbox name, defaulting to `Inbox`.
    #[must_use]
    pub fn mailbox_name(&self) -> &str {
        let name = self.meta.mailbox_name.trim();
        if name.is_empty() { "Inbox" } else { name }
    }

    /// Map this delivery into a [`FetchedMessage`] for the inbound store. The
    /// mox `MsgID` becomes the dedupe UID (stable on redelivery); the RFC
    /// `Message-ID` is the secondary idempotency key.
    #[must_use]
    pub fn to_fetched_message(&self) -> FetchedMessage {
        let to = self
            .to
            .iter()
            .filter_map(NameAddressPub::to_domain)
            .collect();
        let cc = self
            .cc
            .iter()
            .filter_map(NameAddressPub::to_domain)
            .collect();
        let from = self.from.first().and_then(NameAddressPub::to_domain);
        let message_id = trim_angle(&self.message_id);
        let in_reply_to = trim_angle(&self.in_reply_to);
        FetchedMessage {
            // ponytail: mox MsgID is i64; the store UID is u32. A single account
            // exceeding ~4.2B lifetime messages would collide — the RFC
            // Message-ID secondary dedupe (below) is the authoritative
            // idempotency key, so a UID collision at most refreshes flags.
            imap_uid: self.meta.msg_id as u32,
            message_id,
            in_reply_to,
            references: self
                .references
                .iter()
                .map(|r| trim_angle(r).unwrap_or_default())
                .filter(|r| !r.is_empty())
                .collect(),
            from,
            to,
            cc,
            subject: self.subject.clone(),
            body_text: (!self.text.is_empty()).then(|| self.text.clone()),
            body_html: (!self.html.is_empty()).then(|| self.html.clone()),
            seen: false,
            flagged: false,
            answered: false,
            draft: false,
            // ponytail: use ingestion time — a webhook fires at delivery, so
            // now() ≈ received. mox's Date header parse (RFC3339 → time) is not
            // worth the serde-format wiring for slice 1.
            received_at: OffsetDateTime::now_utc(),
            attachments: Vec::new(),
        }
    }
}

impl NameAddressPub {
    fn to_domain(&self) -> Option<MessageAddress> {
        MessageAddress::new(self.address.clone())
            .ok()
            .map(|a| a.with_name(Some(self.name.clone())))
    }
}

/// Strip a single enclosing `<>` pair and surrounding whitespace; `None` if the
/// result is empty.
fn trim_angle(raw: &str) -> Option<String> {
    let t = raw
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    (!t.is_empty()).then(|| t.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_comms_domain::MailSecurity;

    #[test]
    fn incoming_maps_to_fetched_message() {
        let body = r#"{
            "Version":0,
            "From":[{"Name":"Alice","Address":"a@localhost"}],
            "To":[{"Address":"b@localhost"}],
            "Subject":"Hello",
            "MessageID":"<m1@localhost>",
            "InReplyTo":"<orig@localhost>",
            "References":["<orig@localhost>"],
            "Text":"body here",
            "Meta":{"MsgID":42,"RcptTo":"b@localhost","MailboxName":"Inbox"}
        }"#;
        let inc: Incoming = serde_json::from_str(body).unwrap();
        assert_eq!(inc.recipient_address().as_deref(), Some("b@localhost"));
        assert_eq!(inc.mailbox_name(), "Inbox");
        let fm = inc.to_fetched_message();
        assert_eq!(fm.imap_uid, 42);
        assert_eq!(fm.message_id.as_deref(), Some("m1@localhost"));
        assert_eq!(fm.in_reply_to.as_deref(), Some("orig@localhost"));
        assert_eq!(fm.references, vec!["orig@localhost".to_owned()]);
        assert_eq!(fm.from.unwrap().address, "a@localhost");
        assert_eq!(fm.body_text.as_deref(), Some("body here"));
        assert!(!fm.seen);
    }

    #[test]
    fn incoming_falls_back_to_to_header_when_no_rcpt() {
        let body = r#"{"To":[{"Address":"x@localhost"}],"Subject":"s","MessageID":"<m@l>","Meta":{"MsgID":1}}"#;
        let inc: Incoming = serde_json::from_str(body).unwrap();
        assert_eq!(inc.recipient_address().as_deref(), Some("x@localhost"));
        assert_eq!(inc.mailbox_name(), "Inbox");
    }

    // -----------------------------------------------------------------------
    // test_connection: only a genuine authenticated 2xx counts as `ok`.
    // -----------------------------------------------------------------------

    fn transport_config() -> SmtpTransportConfig {
        SmtpTransportConfig {
            host: "unused".to_owned(),
            port: 0,
            security: MailSecurity::StartTls,
            username: "b".to_owned(),
            password: secrecy::SecretString::from("pw".to_owned()),
            from_address: "b@localhost".to_owned(),
            from_name: None,
        }
    }

    /// Serve exactly one raw HTTP response on an ephemeral localhost port and
    /// return its base URL. No mock-HTTP crate needed: a minimal hand-written
    /// status line is enough for reqwest's client to parse.
    async fn serve_once(status_line: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let response =
                    format!("{status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn test_connection_reports_ok_on_authenticated_success() {
        let base_url = serve_once("HTTP/1.1 200 OK").await;
        let sender = MoxWebapiSender::new(base_url);
        let result = sender.test_connection(&transport_config()).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.error_code, None);
    }

    #[tokio::test]
    async fn test_connection_reports_failure_on_wrong_credentials() {
        let base_url = serve_once("HTTP/1.1 401 Unauthorized").await;
        let sender = MoxWebapiSender::new(base_url);
        let result = sender.test_connection(&transport_config()).await.unwrap();
        assert!(!result.ok);
        assert_eq!(result.error_code.as_deref(), Some("auth_failed"));
    }
}
