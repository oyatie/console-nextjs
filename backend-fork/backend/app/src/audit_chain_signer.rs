//! Production audit-chain seal signer wiring (console-ae5 / gh#631).
//!
//! [`console_platform_audit_chain::ExternalSealSigner`] landed in the crate
//! (train-2 / #734). This module is the **in-app** construction path: parse
//! pinned trust anchors + custody URL from process env, build an HTTP
//! [`SealSignTransport`] client, and hand the resulting `Arc<dyn SealSigner>`
//! to the seal worker and attestation endpoint.
//!
//! The custody *daemon* that holds the private key is out of process (HOLD —
//! deploy/daemon lane). Until that daemon exists, operators either leave the
//! seal worker OFF (default) or set `CONSOLE_AUDIT_CHAIN_ALLOW_DEV_SIGNER=true`
//! for explicit non-evidentiary local sealing. Enabling the seal worker without
//! custody config and without the allow-dev escape hatch fails closed (worker
//! does not start).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use console_platform_audit_chain::{
    ExternalSealSigner, InMemoryEd25519Signer, SealSignError, SealSignTransport, SealSigner,
};
use serde::Deserialize;
use url::Url;

use crate::AppError;

/// Default connect/read budget for the sync custody HTTP client. Seal ticks are
/// infrequent; a stuck daemon must not pin a worker thread forever.
const CUSTODY_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Env-driven external seal configuration (anchors + custody transport).
#[derive(Debug, Clone)]
pub struct AuditChainExternalConfig {
    /// Active signing `key_ref` written into every new seal.
    pub key_ref: String,
    /// Locally pinned trust anchors: `key_ref` → raw Ed25519 public key bytes.
    pub anchors: BTreeMap<String, Vec<u8>>,
    /// HTTP URL of the custody signing endpoint (plain `http://` only in this
    /// client — self-host loopback / mesh sidecar). TLS adapters are a follow-up.
    pub custody_url: Url,
    /// Optional bearer token sent as `Authorization: Bearer …`.
    pub custody_token: Option<String>,
}

/// Parse the optional external-seal env group.
///
/// All of `CONSOLE_AUDIT_CHAIN_SEAL_KEY_REF`, `CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS`,
/// and `CONSOLE_AUDIT_CHAIN_CUSTODY_URL` must be present together, or all
/// absent. A partial set is a config error (fail closed — never silently drop
/// half a trust root).
pub fn external_config_from_vars(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Option<AuditChainExternalConfig>, AppError> {
    let key_ref = non_empty(get("CONSOLE_AUDIT_CHAIN_SEAL_KEY_REF"));
    let anchors_raw = non_empty(get("CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS"));
    let custody_url_raw = non_empty(get("CONSOLE_AUDIT_CHAIN_CUSTODY_URL"));
    let custody_token = non_empty(get("CONSOLE_AUDIT_CHAIN_CUSTODY_TOKEN"));

    match (key_ref, anchors_raw, custody_url_raw) {
        (None, None, None) => {
            if custody_token.is_some() {
                return Err(AppError::Config(
                    "CONSOLE_AUDIT_CHAIN_CUSTODY_TOKEN set without KEY_REF/ANCHORS/CUSTODY_URL"
                        .to_owned(),
                ));
            }
            Ok(None)
        }
        (Some(key_ref), Some(anchors_raw), Some(custody_url_raw)) => {
            let anchors = parse_anchors(&anchors_raw)?;
            if !anchors.contains_key(&key_ref) {
                return Err(AppError::Config(format!(
                    "CONSOLE_AUDIT_CHAIN_SEAL_KEY_REF {key_ref} is not present in CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS"
                )));
            }
            let custody_url = Url::parse(&custody_url_raw).map_err(|err| {
                AppError::Config(format!("invalid CONSOLE_AUDIT_CHAIN_CUSTODY_URL: {err}"))
            })?;
            if custody_url.scheme() != "http" {
                return Err(AppError::Config(
                    "CONSOLE_AUDIT_CHAIN_CUSTODY_URL must use http:// (TLS custody transport is a follow-up)"
                        .to_owned(),
                ));
            }
            if custody_url.host_str().is_none() {
                return Err(AppError::Config(
                    "CONSOLE_AUDIT_CHAIN_CUSTODY_URL must include a host".to_owned(),
                ));
            }
            Ok(Some(AuditChainExternalConfig {
                key_ref,
                anchors,
                custody_url,
                custody_token,
            }))
        }
        _ => Err(AppError::Config(
            "CONSOLE_AUDIT_CHAIN_SEAL_KEY_REF, CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS, and \
             CONSOLE_AUDIT_CHAIN_CUSTODY_URL must be set together (or all unset)"
                .to_owned(),
        )),
    }
}

/// Build the attestation / seal `SealSigner` for this process.
///
/// * External config present → [`ExternalSealSigner`] (pinned anchors + HTTP custody).
/// * Otherwise → [`InMemoryEd25519Signer`] (dev/test only; not evidentiary).
pub fn build_attestation_signer(
    external: Option<&AuditChainExternalConfig>,
) -> Result<Arc<dyn SealSigner>, AppError> {
    match external {
        Some(cfg) => Ok(build_external_signer(cfg)?),
        None => Ok(Arc::new(InMemoryEd25519Signer::generate().map_err(
            |err| AppError::Internal(format!("audit-chain attestation signer init failed: {err}")),
        )?)),
    }
}

/// Resolve the seal-worker signer when `CONSOLE_AUDIT_CHAIN_SEAL_ENABLED=true`.
///
/// Returns `Ok(None)` when the worker must not start (fail-closed misconfig).
/// Returns `Ok(Some(_))` with either an external or (explicitly allowed) dev signer.
pub fn build_seal_worker_signer(
    external: Option<&AuditChainExternalConfig>,
    allow_dev_signer: bool,
) -> Result<Option<Arc<dyn SealSigner>>, AppError> {
    match external {
        Some(cfg) => Ok(Some(build_external_signer(cfg)?)),
        None if allow_dev_signer => Ok(Some(Arc::new(InMemoryEd25519Signer::generate().map_err(
            |err| AppError::Internal(format!("audit-chain dev seal signer init failed: {err}")),
        )?))),
        None => {
            tracing::error!(
                "CONSOLE_AUDIT_CHAIN_SEAL_ENABLED=true but external custody config is absent; \
                 seal worker NOT started (set CONSOLE_AUDIT_CHAIN_SEAL_KEY_REF + \
                 CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS + CONSOLE_AUDIT_CHAIN_CUSTODY_URL, or \
                 CONSOLE_AUDIT_CHAIN_ALLOW_DEV_SIGNER=true for explicit non-evidentiary local sealing)"
            );
            Ok(None)
        }
    }
}

fn build_external_signer(cfg: &AuditChainExternalConfig) -> Result<Arc<dyn SealSigner>, AppError> {
    let transport: Arc<dyn SealSignTransport> = Arc::new(HttpCustodyTransport::new(
        cfg.custody_url.clone(),
        cfg.custody_token.clone(),
    ));
    let signer = ExternalSealSigner::new(cfg.key_ref.clone(), transport, cfg.anchors.clone())
        .map_err(|err| AppError::Config(format!("audit-chain external signer: {err}")))?;
    Ok(Arc::new(signer))
}

/// Sync HTTP client for the self-host custody signing endpoint.
///
/// Wire format (v1):
/// ```text
/// POST <path>
/// Content-Type: application/json
/// Authorization: Bearer <token>   # optional
///
/// {"key_ref":"…","message_hex":"…"}
/// → 200 {"signature_hex":"…"}
/// ```
/// Any connect/HTTP/parse failure maps to [`SealSignError::Unavailable`].
pub struct HttpCustodyTransport {
    url: Url,
    token: Option<String>,
}

impl HttpCustodyTransport {
    #[must_use]
    pub fn new(url: Url, token: Option<String>) -> Self {
        Self { url, token }
    }
}

impl SealSignTransport for HttpCustodyTransport {
    fn sign(&self, key_ref: &str, message: &[u8]) -> Result<Vec<u8>, SealSignError> {
        let host = self
            .url
            .host_str()
            .ok_or_else(|| SealSignError::Unavailable("custody URL missing host".to_owned()))?;
        let port = self
            .url
            .port_or_known_default()
            .ok_or_else(|| SealSignError::Unavailable("custody URL missing port".to_owned()))?;
        let path = if self.url.path().is_empty() {
            "/"
        } else {
            self.url.path()
        };
        let query = self
            .url
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        let body = serde_json::json!({
            "key_ref": key_ref,
            "message_hex": hex::encode(message),
        })
        .to_string();
        let mut request = format!(
            "POST {path}{query} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        if let Some(token) = &self.token {
            request.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(&body);

        let mut stream = TcpStream::connect((host, port))
            .map_err(|err| SealSignError::Unavailable(format!("custody connect failed: {err}")))?;
        stream
            .set_read_timeout(Some(CUSTODY_IO_TIMEOUT))
            .map_err(|err| {
                SealSignError::Unavailable(format!("custody set_read_timeout: {err}"))
            })?;
        stream
            .set_write_timeout(Some(CUSTODY_IO_TIMEOUT))
            .map_err(|err| {
                SealSignError::Unavailable(format!("custody set_write_timeout: {err}"))
            })?;
        stream
            .write_all(request.as_bytes())
            .map_err(|err| SealSignError::Unavailable(format!("custody write failed: {err}")))?;

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|err| SealSignError::Unavailable(format!("custody read failed: {err}")))?;
        let response = String::from_utf8_lossy(&response);
        parse_custody_http_response(&response)
    }
}

#[derive(Debug, Deserialize)]
struct CustodySignResponse {
    signature_hex: String,
}

fn parse_custody_http_response(response: &str) -> Result<Vec<u8>, SealSignError> {
    let (header, body) = response.split_once("\r\n\r\n").ok_or_else(|| {
        SealSignError::Unavailable("custody response missing header/body separator".to_owned())
    })?;
    let status_line = header.lines().next().unwrap_or_default();
    if !status_line.contains(" 200 ") && !status_line.ends_with(" 200") {
        return Err(SealSignError::Unavailable(format!(
            "custody HTTP non-200: {status_line}"
        )));
    }
    let parsed: CustodySignResponse = serde_json::from_str(body.trim())
        .map_err(|err| SealSignError::Unavailable(format!("custody response JSON: {err}")))?;
    let bytes = hex::decode(parsed.signature_hex.trim()).map_err(|_| {
        SealSignError::Unavailable("custody signature_hex is not valid hex".to_owned())
    })?;
    if bytes.len() != 64 {
        return Err(SealSignError::Unavailable(format!(
            "custody signature length {} != 64",
            bytes.len()
        )));
    }
    Ok(bytes)
}

/// `key_ref=hexpk[,key_ref=hexpk…]` — hex is raw 32-byte Ed25519 public keys.
fn parse_anchors(raw: &str) -> Result<BTreeMap<String, Vec<u8>>, AppError> {
    let mut out = BTreeMap::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (key_ref, hex_pk) = entry.split_once('=').ok_or_else(|| {
            AppError::Config(format!(
                "CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS entry must be key_ref=hexpk, got {entry:?}"
            ))
        })?;
        let key_ref = key_ref.trim();
        let hex_pk = hex_pk.trim();
        if key_ref.is_empty() {
            return Err(AppError::Config(
                "CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS entry has empty key_ref".to_owned(),
            ));
        }
        let pk = hex::decode(hex_pk).map_err(|_| {
            AppError::Config(format!(
                "CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS public key for {key_ref} is not valid hex"
            ))
        })?;
        if pk.len() != 32 {
            return Err(AppError::Config(format!(
                "CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS public key for {key_ref} must be 32 bytes, got {}",
                pk.len()
            )));
        }
        if out.insert(key_ref.to_owned(), pk).is_some() {
            return Err(AppError::Config(format!(
                "CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS duplicate key_ref {key_ref}"
            )));
        }
    }
    if out.is_empty() {
        return Err(AppError::Config(
            "CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS is empty".to_owned(),
        ));
    }
    Ok(out)
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_owned())
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::thread;

    use console_platform_audit_chain::{InMemoryEd25519Signer, SealSignTransport, SealSigner};

    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn external_config_absent_when_unset() {
        let map = vars(&[]);
        let cfg = external_config_from_vars(|k| map.get(k).cloned()).unwrap();
        assert!(cfg.is_none());
    }
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn external_config_rejects_partial() {
        let map = vars(&[("CONSOLE_AUDIT_CHAIN_SEAL_KEY_REF", "external:selfhost:a")]);
        let err = external_config_from_vars(|k| map.get(k).cloned()).unwrap_err();
        assert!(err.to_string().contains("must be set together"), "{err}");
    }
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn external_config_requires_active_key_in_anchors() {
        let pk = hex::encode([7u8; 32]);
        let map = vars(&[
            ("CONSOLE_AUDIT_CHAIN_SEAL_KEY_REF", "external:selfhost:a"),
            (
                "CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS",
                &format!("external:other={pk}"),
            ),
            ("CONSOLE_AUDIT_CHAIN_CUSTODY_URL", "http://127.0.0.1:9/sign"),
        ]);
        let err = external_config_from_vars(|k| map.get(k).cloned()).unwrap_err();
        assert!(err.to_string().contains("not present in"), "{err}");
    }
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn build_attestation_signer_uses_external_when_configured() {
        let inner = InMemoryEd25519Signer::generate().unwrap();
        let key_ref = "external:selfhost:active";
        let pk_hex = hex::encode(inner.public_key());
        let map = vars(&[
            ("CONSOLE_AUDIT_CHAIN_SEAL_KEY_REF", key_ref),
            (
                "CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS",
                &format!("{key_ref}={pk_hex}"),
            ),
            ("CONSOLE_AUDIT_CHAIN_CUSTODY_URL", "http://127.0.0.1:9/sign"),
        ]);
        let cfg = external_config_from_vars(|k| map.get(k).cloned())
            .unwrap()
            .expect("external config");
        let signer = build_attestation_signer(Some(&cfg)).unwrap();
        assert_eq!(signer.key_ref(), key_ref);
        // Pinned-key forged signature must fail closed (F3 branch at the app wire).
        let msg = b"seal-hash-under-test";
        let good = inner.sign(msg).unwrap();
        assert!(signer.verify(msg, &good, key_ref).unwrap());
        let attacker = InMemoryEd25519Signer::generate().unwrap();
        let forged = attacker.sign(msg).unwrap();
        assert!(
            !signer.verify(msg, &forged, key_ref).unwrap(),
            "forged signature under pinned key_ref must not verify"
        );
    }
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn seal_worker_fails_closed_without_custody_or_allow_dev() {
        let out = build_seal_worker_signer(None, false).unwrap();
        assert!(out.is_none());
    }
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn seal_worker_allows_explicit_dev_signer() {
        let out = build_seal_worker_signer(None, true).unwrap().unwrap();
        assert!(out.key_ref().starts_with("test:ed25519:"));
    }
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn http_custody_transport_signs_via_loopback_stub() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let custody = Arc::new(InMemoryEd25519Signer::generate().unwrap());
        let custody_thread = Arc::clone(&custody);
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            let body = req.split("\r\n\r\n").nth(1).unwrap_or("");
            let v: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
            let message = hex::decode(v["message_hex"].as_str().unwrap()).unwrap();
            let key_ref = v["key_ref"].as_str().unwrap();
            assert_eq!(key_ref, "external:selfhost:active");
            let sig = custody_thread.sign(&message).unwrap();
            let resp_body = serde_json::json!({ "signature_hex": hex::encode(sig) }).to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                resp_body.len()
            );
            stream.write_all(resp.as_bytes()).unwrap();
        });

        let url = Url::parse(&format!("http://{addr}/sign")).unwrap();
        let transport = HttpCustodyTransport::new(url, None);
        let message = b"hello-custody";
        let sig = transport
            .sign("external:selfhost:active", message)
            .expect("custody stub signs");
        assert_eq!(sig, custody.sign(message).unwrap());
        server.join().unwrap();
    }
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn http_custody_transport_maps_connect_failure_to_unavailable() {
        let url = Url::parse("http://127.0.0.1:1/sign").unwrap();
        let transport = HttpCustodyTransport::new(url, None);
        let err = transport.sign("k", b"m").unwrap_err();
        assert!(matches!(err, SealSignError::Unavailable(_)), "{err:?}");
    }
}
