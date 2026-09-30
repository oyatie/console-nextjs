//! External key-custody seal signer (charter §4, gh#271).
//!
//! The production [`SealSigner`]. Unlike [`crate::InMemoryEd25519Signer`],
//! whose `verify` reconstructs the public key from the seal's own
//! attacker-writable `key_ref`, this signer:
//!
//! * holds NO private key in-crate — signing is delegated to an external
//!   key-custody service through a [`SealSignTransport`]; and
//! * verifies signatures ONLY against a set of trust anchors pinned LOCALLY at
//!   construction (config/secret-mount), keyed by `key_ref`.
//!
//! This is the trust boundary. A DB writer who rewrites a sealed row and
//! re-signs under a fresh keypair produces a seal whose `key_ref` is not a
//! pinned anchor → [`ExternalSealSigner::verify`] returns `Ok(false)` → the
//! chain verify routine reports `BadSignature`. Key material never travels
//! through the seal, so the writer cannot forge trust.
//!
//! # Fail-closed
//! Any transport failure surfaces as [`SealSignError::Unavailable`], which the
//! seal worker propagates as an error and rolls the seal transaction back — no
//! seal row is written. There is deliberately no in-crate fallback key: a
//! self-signed seal the DB writer could reproduce would not be evidence.
//!
//! # Custody-independent verification
//! [`ExternalSealSigner::verify`] never touches the transport; it uses only the
//! pinned anchors. Attestation and integrity checks therefore keep working while
//! the signing service is unreachable, so a denial-of-service against the signer
//! cannot blind tamper detection.

use std::collections::BTreeMap;
use std::sync::Arc;

use ring::signature::{ED25519, UnparsedPublicKey};

use crate::{SealSignError, SealSigner};

/// Transport to the external key-custody service that holds the seal private
/// key (self-host signing daemon first; cloud KMS/HSM adapters — including OCI
/// Vault only in the OCI context — implement the same trait).
///
/// Implementations MUST map every unreachable/refused/timeout condition to
/// [`SealSignError::Unavailable`] so the caller fails closed. Returning a
/// non-signature value or a fabricated signature is a contract violation.
pub trait SealSignTransport: Send + Sync {
    /// Sign `message` (a `seal_hash`) with the custody-held private key named by
    /// `key_ref`. `Err(SealSignError::Unavailable(_))` on any inability to reach
    /// or use the service.
    fn sign(&self, key_ref: &str, message: &[u8]) -> Result<Vec<u8>, SealSignError>;
}

/// A production [`SealSigner`] whose private key lives in external custody and
/// whose verification trust anchors are pinned locally.
///
/// Construct with [`ExternalSealSigner::new`], which requires the active signing
/// `key_ref` to be among the pinned `anchors` (you must be able to verify what
/// you sign). The `anchors` map may hold additional (e.g. rotated-out) public
/// keys so chains signed under an older key still verify.
pub struct ExternalSealSigner {
    /// The `key_ref` written into every seal this instance signs, e.g.
    /// `external:selfhost:2026-08`. Persisted in `audit_chain_seals.key_ref`.
    key_ref: String,
    /// Custody transport holding the private key for `key_ref`.
    transport: Arc<dyn SealSignTransport>,
    /// Trust anchors: `key_ref` → raw Ed25519 public key bytes. The ONLY source
    /// `verify` consults; a stored `key_ref` absent here is untrusted.
    anchors: BTreeMap<String, Vec<u8>>,
}

impl ExternalSealSigner {
    /// Build a signer that signs under `active_key_ref` via `transport` and
    /// verifies against `anchors`.
    ///
    /// # Errors
    /// [`SealSignError::KeyRef`] if `active_key_ref` is not present in
    /// `anchors` — signing with a key you cannot locally verify is an
    /// operational foot-gun (a chain you can never attest as your own).
    pub fn new(
        active_key_ref: String,
        transport: Arc<dyn SealSignTransport>,
        anchors: BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, SealSignError> {
        if !anchors.contains_key(&active_key_ref) {
            return Err(SealSignError::KeyRef(format!(
                "active key_ref {active_key_ref} is not a pinned trust anchor"
            )));
        }
        Ok(Self {
            key_ref: active_key_ref,
            transport,
            anchors,
        })
    }
}

impl SealSigner for ExternalSealSigner {
    fn key_ref(&self) -> &str {
        &self.key_ref
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, SealSignError> {
        self.transport.sign(&self.key_ref, message)
    }

    fn verify(
        &self,
        message: &[u8],
        signature: &[u8],
        key_ref: &str,
    ) -> Result<bool, SealSignError> {
        // Consult ONLY the locally pinned anchors. A stored key_ref we did not
        // pin — an attacker's fresh keypair, or a key we never trusted — is not
        // verifiable and fails closed as a bad signature (NOT an Err, matching
        // verify_org_chain's contract that anything an attacker can write to
        // storage yields a tamper verdict). Public key material is never derived
        // from the attacker-writable seal.
        let Some(public_key) = self.anchors.get(key_ref) else {
            return Ok(false);
        };
        let peer = UnparsedPublicKey::new(&ED25519, public_key);
        Ok(peer.verify(message, signature).is_ok())
    }
}
