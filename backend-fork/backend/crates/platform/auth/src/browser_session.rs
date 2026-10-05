//! Browser transport custody for an original native signed proof.
//! This codec never issues, refreshes or authorizes an Account session.

use openssl::rand::rand_bytes;
use openssl::symm::{Cipher, decrypt_aead, encrypt_aead};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const VERSION: i32 = 1;
pub const MAX_PROOF_BYTES: usize = 1_048_576;

#[derive(Clone, Debug)]
pub struct Binding {
    pub codec_version: i32,
    pub account: Uuid,
    pub company: Uuid,
    pub family: Uuid,
    pub source: Uuid,
    pub context: Uuid,
    pub expires_unix_seconds: i64,
}

#[derive(Clone)]
pub struct Key([u8; 32]);

/// Separate purpose-scoped credential for the authenticated browser ingress.
#[derive(Clone)]
pub struct IngressKey([u8; 32]);

impl IngressKey {
    pub fn from_base64url(value: &str) -> Result<Self, ProofError> {
        Ok(Self(decode_base64url_32(value)?))
    }

    #[must_use]
    pub fn accepts_wire(&self, value: &str) -> bool {
        value
            .strip_prefix("bi1.")
            .and_then(|encoded| decode_base64url_32(encoded).ok())
            .is_some_and(|bytes| openssl::memcmp::eq(&self.0, &bytes))
    }
}

impl std::fmt::Debug for IngressKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BrowserIngressKey(<redacted>)")
    }
}

impl Key {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProofError> {
        Ok(Self(bytes.try_into().map_err(|_| ProofError::Shape)?))
    }

    pub fn from_hex(value: &str) -> Result<Self, ProofError> {
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ProofError::Shape);
        }
        let mut bytes = [0; 32];
        for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            let text = std::str::from_utf8(pair).map_err(|_| ProofError::Shape)?;
            bytes[index] = u8::from_str_radix(text, 16).map_err(|_| ProofError::Shape)?;
        }
        Self::from_bytes(&bytes)
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BrowserProofKey(<redacted>)")
    }
}

#[derive(Clone)]
pub struct StoredProof {
    pub codec_version: i32,
    pub token_hash: Vec<u8>,
    pub nonce: Vec<u8>,
    pub tag: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

impl std::fmt::Debug for StoredProof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BrowserStoredProof(<redacted>)")
    }
}

pub struct OpenedProof(String);

impl OpenedProof {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for OpenedProof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BrowserOpenedProof(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProofError {
    #[error("invalid browser proof shape")]
    Shape,
    #[error("browser proof authentication failed")]
    Authentication,
    #[error("invalid browser proof text")]
    Utf8,
    #[error("browser proof cryptography unavailable")]
    Crypto,
}

pub fn aad(binding: &Binding) -> Result<[u8; 125], ProofError> {
    if binding.codec_version != VERSION {
        return Err(ProofError::Shape);
    }
    let mut bytes = [0; 125];
    bytes[..4].copy_from_slice(&29_u32.to_be_bytes());
    bytes[4..33].copy_from_slice(b"console/browser-session-proof");
    bytes[33..37].copy_from_slice(&1_u32.to_be_bytes());
    for (slot, id) in bytes[37..117].as_chunks_mut::<16>().0.iter_mut().zip([
        binding.account,
        binding.company,
        binding.family,
        binding.source,
        binding.context,
    ]) {
        slot.copy_from_slice(id.as_bytes());
    }
    bytes[117..].copy_from_slice(&binding.expires_unix_seconds.to_be_bytes());
    Ok(bytes)
}

fn encrypt_bytes(
    key: &Key,
    nonce: &[u8; 12],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<(Vec<u8>, [u8; 16]), ProofError> {
    let mut tag = [0; 16];
    let ciphertext = encrypt_aead(
        Cipher::aes_256_gcm(),
        &key.0,
        Some(nonce),
        aad,
        plaintext,
        &mut tag,
    )
    .map_err(|_| ProofError::Crypto)?;
    Ok((ciphertext, tag))
}

pub fn seal(
    key: &Key,
    binding: &Binding,
    token_hash: [u8; 32],
    plaintext: &str,
) -> Result<StoredProof, ProofError> {
    if !(1..=MAX_PROOF_BYTES).contains(&plaintext.len()) {
        return Err(ProofError::Shape);
    }
    let aad = aad(binding)?;
    let mut nonce = [0; 12];
    rand_bytes(&mut nonce).map_err(|_| ProofError::Crypto)?;
    let (ciphertext, tag) = encrypt_bytes(key, &nonce, &aad, plaintext.as_bytes())?;
    Ok(StoredProof {
        codec_version: VERSION,
        token_hash: token_hash.to_vec(),
        nonce: nonce.to_vec(),
        tag: tag.to_vec(),
        ciphertext,
    })
}

pub fn open(key: &Key, binding: &Binding, proof: &StoredProof) -> Result<OpenedProof, ProofError> {
    if proof.codec_version != VERSION
        || proof.token_hash.len() != 32
        || proof.nonce.len() != 12
        || proof.tag.len() != 16
        || !(1..=MAX_PROOF_BYTES).contains(&proof.ciphertext.len())
    {
        return Err(ProofError::Shape);
    }
    let plaintext = decrypt_aead(
        Cipher::aes_256_gcm(),
        &key.0,
        Some(&proof.nonce),
        &aad(binding)?,
        &proof.ciphertext,
        &proof.tag,
    )
    .map_err(|_| ProofError::Authentication)?;
    Ok(OpenedProof(
        String::from_utf8(plaintext).map_err(|_| ProofError::Utf8)?,
    ))
}

/// Canonical opaque wire credential. Only its wire-text digest is persisted.
pub struct Handle(String);

impl Handle {
    pub fn generate() -> Result<Self, ProofError> {
        let mut bytes = [0; 32];
        rand_bytes(&mut bytes).map_err(|_| ProofError::Crypto)?;
        Ok(Self(format!("bs1.{}", encode_base64url(&bytes))))
    }

    pub fn parse(value: &str) -> Result<Self, ProofError> {
        let encoded = value.strip_prefix("bs1.").ok_or(ProofError::Shape)?;
        decode_base64url_32(encoded)?;
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        Sha256::digest(self.0.as_bytes()).into()
    }
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BrowserHandle(<redacted>)")
    }
}

fn encode_base64url(bytes: &[u8]) -> String {
    openssl::base64::encode_block(bytes)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

pub fn decode_base64url_32(value: &str) -> Result<[u8; 32], ProofError> {
    if value.len() != 43
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(ProofError::Shape);
    }
    let bytes =
        openssl::base64::decode_block(&format!("{}=", value.replace('-', "+").replace('_', "/")))
            .map_err(|_| ProofError::Shape)?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| ProofError::Shape)?;
    if encode_base64url(&bytes) != value {
        return Err(ProofError::Shape);
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "browser_session/tests.rs"]
mod tests;
