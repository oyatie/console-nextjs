//! Inbound IMAP adapter for webmail (B-mail-3).
//!
//! Implements the [`ImapClient`] port over `async-imap` + `tokio-rustls`:
//!
//! * **SSRF guard** ([`ssrf`]): before any connect, the admin-configured IMAP
//!   host is resolved once via `hickory-resolver`, every resolved IP is
//!   denylist-checked (RFC1918 / loopback / link-local incl. cloud metadata /
//!   ULA / CGNAT, IPv4-mapped IPv6 un-mapped first), and the FIRST allowed IP is
//!   PINNED for the dial — DNS-rebinding safe. The IMAP port allowlist (993/143)
//!   is re-asserted defensively.
//! * **TLS enforced** (single rustls 0.23 stack): `SslTls` (993) does an implicit
//!   TLS handshake on connect; `StartTls` (143) connects plaintext then issues
//!   `STARTTLS` and upgrades. Certificate verification uses the webpki roots via
//!   the rustls `ring` provider; the SNI/cert name is the ORIGINAL hostname even
//!   though we dial the pinned IP, so verification still matches. There is no
//!   plaintext path and no permissive verifier.
//! * **Mirror-in** ([`parse`]): `UID FETCH ... BODY.PEEK[]` (side-effect free —
//!   never sets `\Seen`) → `mail-parser` → the application's `FetchedMessage`.
//!
//! This is the ONLY crate where `async-imap`/`mail-parser`/`tokio-rustls` appear.
//! No secret, recipient, body, or host is logged; transport failures map to a
//! fixed, non-secret error code.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod parse;
pub mod ssrf;

use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_imap::types::{Fetch, Flag, Name};
use async_imap::{Client, Session};
use console_comms_application::{
    ALLOWED_IMAP_PORTS, FetchedMessage, ImapClient, ImapFolder, ImapSelect, ImapSession,
    ImapTransportConfig, MailFuture, MailServiceError, SYNC_FETCH_ITEMS, TestConnectionResult,
};
use console_comms_domain::{FolderRole, MailSecurity};
use futures::StreamExt;
use hickory_resolver::Resolver;
use secrecy::ExposeSecret;
use time::OffsetDateTime;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use crate::parse::MessageFlags;
use crate::ssrf::SsrfError;

/// How long to wait for the TCP connect + TLS handshake before giving up. Bounds
/// the test-connection / sync probe so a black-holed host cannot wedge a worker.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// The inbound IMAP client. Stateless: each connect resolves + validates the host
/// and builds a one-shot TLS session, so a credential or host change always takes
/// effect immediately and no secret is cached across passes.
#[derive(Debug, Default, Clone)]
pub struct AsyncImapClient;

impl AsyncImapClient {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Resolve + pin the host, dial the pinned IP, and complete the TLS handshake
    /// (implicit for 993, STARTTLS for 143). Returns an authenticated session.
    async fn open_session(
        config: &ImapTransportConfig,
    ) -> Result<Session<TlsStream<TcpStream>>, MailServiceError> {
        // Defense-in-depth: re-assert the IMAP port allowlist at the transport.
        if !ALLOWED_IMAP_PORTS.contains(&config.port) {
            return Err(MailServiceError::Transport {
                code: "port_not_allowed",
            });
        }

        let pinned = resolve_and_pin(&config.host)
            .await
            .map_err(|err| MailServiceError::Transport { code: err.code() })?;

        let tls = connect_tls(config, pinned).await?;
        let client = Client::new(tls);

        // LOGIN with the decrypted credential. On failure the tuple's Client half
        // is dropped here (closing the socket); we map to a fixed auth code.
        let session = client
            .login(&config.username, config.password.expose_secret())
            .await
            .map_err(|_| MailServiceError::Transport {
                code: "auth_failed",
            })?;
        Ok(session)
    }
}

/// Establish the TLS stream to the pinned IP, with SNI = the original hostname.
async fn connect_tls(
    config: &ImapTransportConfig,
    pinned: IpAddr,
) -> Result<TlsStream<TcpStream>, MailServiceError> {
    let server_name =
        ServerName::try_from(config.host.clone()).map_err(|_| MailServiceError::Transport {
            code: "invalid_host",
        })?;
    let connector = TlsConnector::from(Arc::new(tls_client_config()?));

    let addr = std::net::SocketAddr::new(pinned, config.port);
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| MailServiceError::Transport {
            code: "connect_timeout",
        })?
        .map_err(|_| MailServiceError::Transport {
            code: "connect_failed",
        })?;

    match config.security {
        // Implicit TLS on connect (IMAPS / 993): handshake immediately.
        MailSecurity::SslTls => {
            tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(server_name, tcp))
                .await
                .map_err(|_| MailServiceError::Transport {
                    code: "tls_timeout",
                })?
                .map_err(|_| MailServiceError::Transport {
                    code: "tls_handshake",
                })
        }
        // STARTTLS (143): negotiate the upgrade on the plaintext socket, THEN
        // wrap it in TLS. async-imap 0.11 exposes no pre-login command method, so
        // we drive the (trivial, well-defined) STARTTLS line exchange directly:
        // read the server greeting, send `STARTTLS`, require a tagged `OK`, and
        // only then hand the socket to rustls. If the server does not reply OK we
        // ERROR — there is NO silent plaintext fallback.
        MailSecurity::StartTls => {
            let mut tcp = tcp;
            tokio::time::timeout(CONNECT_TIMEOUT, negotiate_starttls(&mut tcp))
                .await
                .map_err(|_| MailServiceError::Transport {
                    code: "tls_timeout",
                })??;
            tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(server_name, tcp))
                .await
                .map_err(|_| MailServiceError::Transport {
                    code: "tls_timeout",
                })?
                .map_err(|_| MailServiceError::Transport {
                    code: "tls_handshake",
                })
        }
    }
}

/// Drive the IMAP STARTTLS line exchange on the raw plaintext socket: consume the
/// server greeting, send a tagged `STARTTLS`, and require a tagged `OK` before
/// the caller upgrades to TLS. Any other response (or a connection that refuses
/// the upgrade) is an error — never a plaintext fallback.
async fn negotiate_starttls<S>(stream: &mut S) -> Result<(), MailServiceError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;

    // Read until we have at least one complete line (the untagged greeting). A
    // small bounded read loop; the greeting is a single short line.
    let _greeting = read_line(stream).await?;

    stream
        .write_all(b"a1 STARTTLS\r\n")
        .await
        .map_err(|_| MailServiceError::Transport {
            code: "starttls_failed",
        })?;
    stream.flush().await.ok();

    // Require a tagged OK for our `a1` STARTTLS. Untagged lines (`* ...`) before
    // the tagged response are skipped.
    loop {
        let line = read_line(stream).await?;
        let upper = line.trim().to_ascii_uppercase();
        if upper.starts_with("A1 OK") {
            return Ok(());
        }
        if upper.starts_with("A1 ") {
            // A1 NO / A1 BAD — the server refused STARTTLS.
            return Err(MailServiceError::Transport {
                code: "starttls_refused",
            });
        }
        // An untagged `* ...` continuation: keep reading for the tagged reply.
    }
}

/// Read a single CRLF-terminated line (bounded) from a plaintext stream during
/// STARTTLS negotiation. Errors on EOF or an over-long line so a hostile server
/// cannot wedge or balloon the handshake.
async fn read_line<S>(stream: &mut S) -> Result<String, MailServiceError>
where
    S: AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        let n = stream
            .read(&mut byte)
            .await
            .map_err(|_| MailServiceError::Transport {
                code: "starttls_failed",
            })?;
        if n == 0 {
            return Err(MailServiceError::Transport {
                code: "starttls_failed",
            });
        }
        if byte[0] == b'\n' {
            break;
        }
        if byte[0] != b'\r' {
            buf.push(byte[0]);
        }
        if buf.len() > 4096 {
            return Err(MailServiceError::Transport {
                code: "starttls_failed",
            });
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Build the rustls client config: webpki roots, the `ring` crypto provider, no
/// client auth, default (safe TLS 1.2 + 1.3) protocol versions. No permissive
/// verifier. Built explicitly on the `ring` provider so it never depends on a
/// process-global default provider (which may be unset in a worker process).
fn tls_client_config() -> Result<ClientConfig, MailServiceError> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        // Only errors on an inconsistent provider/version set, which the bundled
        // ring provider is not; map to a fixed code rather than panicking.
        .map_err(|_| MailServiceError::Transport { code: "tls_setup" })?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(config)
}

impl ImapClient for AsyncImapClient {
    fn connect<'a>(
        &'a self,
        config: &'a ImapTransportConfig,
    ) -> MailFuture<'a, Result<Box<dyn ImapSession>, MailServiceError>> {
        Box::pin(async move {
            let session = Self::open_session(config).await?;
            Ok(Box::new(AsyncImapSession {
                session: Some(session),
            }) as Box<dyn ImapSession>)
        })
    }

    fn test_connection<'a>(
        &'a self,
        config: &'a ImapTransportConfig,
    ) -> MailFuture<'a, Result<TestConnectionResult, MailServiceError>> {
        Box::pin(async move {
            match Self::open_session(config).await {
                Ok(mut session) => {
                    // Clean logout; ignore the result (best-effort).
                    let _ = session.logout().await;
                    Ok(TestConnectionResult {
                        ok: true,
                        error_code: None,
                    })
                }
                Err(MailServiceError::Transport { code }) => Ok(TestConnectionResult {
                    ok: false,
                    error_code: Some(code.to_owned()),
                }),
                Err(other) => Err(other),
            }
        })
    }
}

/// An authenticated IMAP session driving one account's mailbox.
struct AsyncImapSession {
    // `Option` so `logout` can take ownership of the session (async-imap's logout
    // consumes `&mut self` but the Drop ordering is simplest with a take()).
    session: Option<Session<TlsStream<TcpStream>>>,
}

impl AsyncImapSession {
    fn session_mut(&mut self) -> Result<&mut Session<TlsStream<TcpStream>>, MailServiceError> {
        self.session.as_mut().ok_or(MailServiceError::Transport {
            code: "session_closed",
        })
    }
}

impl ImapSession for AsyncImapSession {
    fn list_folders(&mut self) -> MailFuture<'_, Result<Vec<ImapFolder>, MailServiceError>> {
        Box::pin(async move {
            let session = self.session_mut()?;
            let mut stream = session.list(Some(""), Some("*")).await.map_err(|_| {
                MailServiceError::Transport {
                    code: "list_failed",
                }
            })?;
            let mut folders = Vec::new();
            while let Some(item) = stream.next().await {
                let name = item.map_err(|_| MailServiceError::Transport {
                    code: "list_failed",
                })?;
                if let Some(folder) = folder_from_name(&name) {
                    folders.push(folder);
                }
            }
            Ok(folders)
        })
    }

    fn select<'a>(
        &'a mut self,
        imap_path: &'a str,
    ) -> MailFuture<'a, Result<ImapSelect, MailServiceError>> {
        Box::pin(async move {
            let session = self.session_mut()?;
            let mailbox =
                session
                    .select(imap_path)
                    .await
                    .map_err(|_| MailServiceError::Transport {
                        code: "select_failed",
                    })?;
            Ok(ImapSelect {
                // A mailbox with no UIDVALIDITY is non-conformant; treat it as 0 so
                // the engine resets the cursor (refetch from the floor).
                uid_validity: mailbox.uid_validity.unwrap_or(0),
                uid_next: mailbox.uid_next,
                exists: mailbox.exists,
            })
        })
    }

    fn fetch_since<'a>(
        &'a mut self,
        since_uid: u32,
        limit: u32,
    ) -> MailFuture<'a, Result<Vec<FetchedMessage>, MailServiceError>> {
        Box::pin(async move {
            // `UID FETCH (since+1):* ...` — everything strictly newer than the
            // cursor. BODY.PEEK[] keeps it side-effect free (no `\Seen`).
            let start = since_uid.saturating_add(1);
            let uid_set = format!("{start}:*");
            let session = self.session_mut()?;
            let mut stream = session
                .uid_fetch(uid_set, SYNC_FETCH_ITEMS)
                .await
                .map_err(|_| MailServiceError::Transport {
                    code: "fetch_failed",
                })?;

            let mut out: Vec<FetchedMessage> = Vec::new();
            while let Some(item) = stream.next().await {
                let fetch = item.map_err(|_| MailServiceError::Transport {
                    code: "fetch_failed",
                })?;
                // A `UID n:*` range always returns at least the highest UID even
                // when none is strictly greater, so re-assert the cursor here.
                let Some(uid) = fetch.uid else { continue };
                if uid < start {
                    continue;
                }
                if let Some(message) = fetched_to_message(uid, &fetch) {
                    out.push(message);
                }
                if out.len() as u32 >= limit {
                    break;
                }
            }
            // Ascending UID order so the cursor advances monotonically.
            out.sort_by_key(|m| m.imap_uid);
            Ok(out)
        })
    }

    fn logout(&mut self) -> MailFuture<'_, ()> {
        Box::pin(async move {
            if let Some(mut session) = self.session.take() {
                let _ = session.logout().await;
            }
        })
    }
}

/// Classify a `LIST` entry into a [`ImapFolder`], or `None` for a `\Noselect`
/// container that holds no messages.
fn folder_from_name(name: &Name) -> Option<ImapFolder> {
    use async_imap::types::NameAttribute;
    if name
        .attributes()
        .iter()
        .any(|a| matches!(a, NameAttribute::NoSelect))
    {
        return None;
    }
    let path = name.name().to_owned();
    let role = classify_role(&path, name.delimiter());
    let display = display_name(&path, name.delimiter());
    Some(ImapFolder {
        imap_path: path,
        role,
        name: display,
    })
}

/// Map a mailbox path to a [`FolderRole`] by its (case-insensitive) leaf name.
fn classify_role(path: &str, delimiter: Option<&str>) -> FolderRole {
    let leaf = leaf_segment(path, delimiter).to_ascii_uppercase();
    match leaf.as_str() {
        "INBOX" => FolderRole::Inbox,
        "SENT" | "SENT ITEMS" | "SENT MAIL" | "SENT MESSAGES" => FolderRole::Sent,
        "DRAFTS" | "DRAFT" => FolderRole::Drafts,
        "ARCHIVE" | "ALL MAIL" => FolderRole::Archive,
        "TRASH" | "DELETED" | "DELETED ITEMS" | "BIN" => FolderRole::Trash,
        "JUNK" | "SPAM" | "JUNK E-MAIL" => FolderRole::Junk,
        _ => FolderRole::Custom,
    }
}

/// The human display name is the last path segment.
fn display_name(path: &str, delimiter: Option<&str>) -> String {
    let leaf = leaf_segment(path, delimiter);
    if leaf.trim().is_empty() {
        path.to_owned()
    } else {
        leaf.to_owned()
    }
}

/// The final hierarchy segment of a mailbox path (`[Gmail]/Sent Mail` → `Sent
/// Mail`), split on the server's hierarchy delimiter (default `/`).
fn leaf_segment<'a>(path: &'a str, delimiter: Option<&str>) -> &'a str {
    let delim = delimiter.filter(|d| !d.is_empty()).unwrap_or("/");
    path.rsplit(delim).next().unwrap_or(path)
}

/// Convert one `Fetch` (UID + FLAGS + INTERNALDATE + BODY.PEEK[]) into the
/// application's parsed message.
fn fetched_to_message(uid: u32, fetch: &Fetch) -> Option<FetchedMessage> {
    let body = fetch.body()?;
    let flags = flags_of(fetch);
    // async-imap's INTERNALDATE is a `chrono::DateTime<FixedOffset>`; convert via
    // its unix-second timestamp so this crate never names `chrono` directly.
    let internal = fetch
        .internal_date()
        .and_then(|dt| OffsetDateTime::from_unix_timestamp(dt.timestamp()).ok());
    parse::parse_message(uid, flags, internal, body)
}

/// Map the IMAP `FLAGS` set to the boolean flags the engine persists.
fn flags_of(fetch: &Fetch) -> MessageFlags {
    let mut flags = MessageFlags::default();
    for flag in fetch.flags() {
        match flag {
            Flag::Seen => flags.seen = true,
            Flag::Flagged => flags.flagged = true,
            Flag::Answered => flags.answered = true,
            Flag::Draft => flags.draft = true,
            _ => {}
        }
    }
    flags
}

/// Resolve `host` once and return the single pinned IP to dial. A host that is
/// itself an IP literal is validated directly (no DNS).
async fn resolve_and_pin(host: &str) -> Result<IpAddr, SsrfError> {
    let host = host.trim();
    if host.is_empty() {
        return Err(SsrfError::InvalidHost);
    }
    if let Ok(ip) = IpAddr::from_str(host) {
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
    let addresses: Vec<IpAddr> = lookup.iter().collect();
    ssrf::pick_pinned_ip(&addresses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_role_maps_well_known_folders() {
        assert_eq!(classify_role("INBOX", None), FolderRole::Inbox);
        assert_eq!(
            classify_role("[Gmail]/Sent Mail", Some("/")),
            FolderRole::Sent
        );
        assert_eq!(classify_role("Drafts", None), FolderRole::Drafts);
        assert_eq!(classify_role("Junk", None), FolderRole::Junk);
        assert_eq!(classify_role("Trash", None), FolderRole::Trash);
        assert_eq!(
            classify_role("Projects/2026", Some("/")),
            FolderRole::Custom
        );
    }

    #[test]
    fn leaf_segment_handles_nested_paths() {
        assert_eq!(leaf_segment("[Gmail]/Sent Mail", Some("/")), "Sent Mail");
        assert_eq!(leaf_segment("INBOX", Some("/")), "INBOX");
        assert_eq!(leaf_segment("a.b.c", Some(".")), "c");
    }

    #[tokio::test]
    async fn resolve_and_pin_rejects_private_and_metadata_literals() {
        assert_eq!(
            resolve_and_pin("127.0.0.1").await,
            Err(SsrfError::DisallowedAddress)
        );
        assert_eq!(
            resolve_and_pin("169.254.169.254").await,
            Err(SsrfError::DisallowedAddress)
        );
        assert_eq!(
            resolve_and_pin("10.0.0.5").await,
            Err(SsrfError::DisallowedAddress)
        );
    }

    #[tokio::test]
    async fn resolve_and_pin_accepts_public_literal() {
        assert_eq!(
            resolve_and_pin("8.8.8.8").await.unwrap(),
            "8.8.8.8".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn tls_config_builds_with_webpki_roots() {
        // Building the client config must succeed (ring provider) and load real
        // roots — the verifier path is wired and the config is usable.
        let cfg = tls_client_config().expect("ring-backed client config must build");
        let _ = Arc::new(cfg);
        assert!(!webpki_roots::TLS_SERVER_ROOTS.is_empty());
    }
}
