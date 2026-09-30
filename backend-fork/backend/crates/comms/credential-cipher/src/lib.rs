//! Webmail credential cipher — envelope AEAD for SMTP/IMAP passwords at rest.
//!
//! This crate provides the CONCRETE [`EnvelopeCredentialCipher`] implementation
//! of the [`CredentialCipher`] PORT. The trait and its value types ([`Aad`],
//! [`SealedCredential`], [`CipherError`]) are owned by the application layer
//! (`console-comms-application`); this crate depends on that abstraction and
//! implements it, so the dependency direction stays clean (the application /
//! adapter layers speak only to the port, and only the app composition root
//! wires this concrete cipher).
//!
//! # Scheme (envelope encryption)
//!
//! Each encrypted secret gets its OWN random 256-bit data-encryption key (DEK):
//!
//! 1. A fresh DEK is generated per `encrypt` call (`OsRng`).
//! 2. The plaintext secret is sealed under the DEK with `XChaCha20Poly1305`
//!    (24-byte random nonce), with the caller's [`Aad`] bound as associated
//!    data — so a ciphertext copied to another row/field fails authentication.
//! 3. The DEK itself is sealed ("wrapped") under the master KEK with
//!    `XChaCha20Poly1305` (its own 24-byte random nonce), with the same [`Aad`]
//!    bound. Only the *wrapped* DEK is ever persisted.
//!
//! The store therefore persists, per secret: `(ciphertext, nonce, dek_wrapped,
//! dek_nonce, key_version)` — and NEVER a plaintext password nor the bare DEK.
//! KEK rotation re-wraps the small DEK (a follow-on job), not every secret.
//!
//! # Master key (KEK)
//!
//! The KEK is loaded from the `CONSOLE_MAIL_MASTER_KEY` environment variable — a
//! base64 (standard alphabet) encoding of exactly 32 bytes — sourced from OCI
//! Vault into the `console-secrets` env in production. It is NEVER hardcoded, logged,
//! or written to disk by this crate, and is held in a [`SecretBox`] so it is
//! zeroized on drop and prints as `[REDACTED]`.
//!
//! # Security invariants
//!
//! * `XChaCha20Poly1305` (AEAD): tampering with ciphertext, nonce, the wrapped
//!   DEK, or the AAD makes decryption FAIL (no silent acceptance).
//! * 24-byte nonces drawn from the OS CSPRNG (`OsRng`) — the 192-bit XChaCha
//!   nonce makes random-per-row collision negligible.
//! * Decrypted secrets and the KEK live only inside [`SecretBox`]; the inner
//!   bytes are zeroized on drop. This crate logs NOTHING.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use secrecy::{ExposeSecret, SecretBox};
use zeroize::Zeroize;

pub use console_comms_application::credential_cipher::{
    Aad, CipherError, CredentialCipher, SealedCredential,
};

/// The environment variable carrying the base64-encoded 32-byte master KEK.
pub const MASTER_KEY_ENV: &str = "CONSOLE_MAIL_MASTER_KEY";

/// The key-derivation version stamped onto every freshly encrypted row. Bumped
/// (with a re-wrap job) on KEK rotation.
pub const CURRENT_KEY_VERSION: i16 = 1;

const KEY_LEN: usize = 32;

/// `XChaCha20Poly1305` envelope cipher holding the master KEK in a [`SecretBox`].
pub struct EnvelopeCredentialCipher {
    /// The master key-encryption key (32 bytes), zeroized on drop.
    kek: SecretBox<[u8; KEY_LEN]>,
    key_version: i16,
}

impl EnvelopeCredentialCipher {
    /// Build the cipher from the base64-encoded 32-byte KEK in the
    /// `CONSOLE_MAIL_MASTER_KEY` environment variable.
    pub fn from_env() -> Result<Self, CipherError> {
        let encoded = std::env::var(MASTER_KEY_ENV).map_err(|_| CipherError::MasterKey)?;
        Self::from_base64_key(&encoded)
    }

    /// Build the cipher from a base64 (standard alphabet) encoding of exactly
    /// 32 key bytes. The decoded buffer is zeroized after the key is copied in.
    pub fn from_base64_key(encoded: &str) -> Result<Self, CipherError> {
        let mut decoded = BASE64
            .decode(encoded.trim())
            .map_err(|_| CipherError::MasterKey)?;
        let result = Self::from_key_bytes(&decoded);
        decoded.zeroize();
        result
    }

    /// Build the cipher directly from raw key bytes (must be exactly 32).
    pub fn from_key_bytes(bytes: &[u8]) -> Result<Self, CipherError> {
        if bytes.len() != KEY_LEN {
            return Err(CipherError::MasterKey);
        }
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(bytes);
        let kek = SecretBox::new(Box::new(key));
        // `key` is a Copy array on the stack; overwrite our local copy too.
        key.zeroize();
        Ok(Self {
            kek,
            key_version: CURRENT_KEY_VERSION,
        })
    }

    /// The KEK cipher instance. Constructed per call so the expanded key
    /// schedule never outlives the operation.
    fn kek_cipher(&self) -> XChaCha20Poly1305 {
        let key = Key::from_slice(self.kek.expose_secret().as_slice());
        XChaCha20Poly1305::new(key)
    }
}

impl CredentialCipher for EnvelopeCredentialCipher {
    fn encrypt(&self, plaintext: &[u8], aad: Aad<'_>) -> Result<SealedCredential, CipherError> {
        let aad_bytes = aad.encode();

        // 1. Fresh per-row DEK.
        let dek = XChaCha20Poly1305::generate_key(&mut OsRng);

        // 2. Seal the secret under the DEK, AAD-bound.
        let dek_cipher = XChaCha20Poly1305::new(&dek);
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = dek_cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: &aad_bytes,
                },
            )
            .map_err(|_| CipherError::Encrypt)?;

        // 3. Wrap the DEK under the KEK, AAD-bound.
        let kek_cipher = self.kek_cipher();
        let dek_nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let mut dek_bytes = dek;
        let dek_wrapped = kek_cipher
            .encrypt(
                &dek_nonce,
                Payload {
                    msg: dek_bytes.as_slice(),
                    aad: &aad_bytes,
                },
            )
            .map_err(|_| CipherError::Encrypt)?;
        // The bare DEK never leaves this function — zeroize the working copy.
        dek_bytes.zeroize();

        Ok(SealedCredential {
            ciphertext,
            nonce: nonce.to_vec(),
            dek_wrapped,
            dek_nonce: dek_nonce.to_vec(),
            key_version: self.key_version,
        })
    }

    fn decrypt(
        &self,
        sealed: &SealedCredential,
        aad: Aad<'_>,
    ) -> Result<SecretBox<Vec<u8>>, CipherError> {
        if sealed.key_version != self.key_version {
            return Err(CipherError::KeyVersion);
        }
        let aad_bytes = aad.encode();

        // 1. Unwrap the DEK under the KEK (authenticates the wrap + AAD).
        let kek_cipher = self.kek_cipher();
        let dek_nonce = nonce_from_slice(&sealed.dek_nonce)?;
        let mut dek_bytes = kek_cipher
            .decrypt(
                &dek_nonce,
                Payload {
                    msg: &sealed.dek_wrapped,
                    aad: &aad_bytes,
                },
            )
            .map_err(|_| CipherError::Decrypt)?;
        if dek_bytes.len() != KEY_LEN {
            dek_bytes.zeroize();
            return Err(CipherError::Decrypt);
        }

        // 2. Decrypt the secret under the DEK (authenticates the secret + AAD).
        let dek = Key::from_slice(&dek_bytes).to_owned();
        let dek_cipher = XChaCha20Poly1305::new(&dek);
        dek_bytes.zeroize();
        let nonce = nonce_from_slice(&sealed.nonce)?;
        let plaintext = dek_cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &sealed.ciphertext,
                    aad: &aad_bytes,
                },
            )
            .map_err(|_| CipherError::Decrypt)?;

        Ok(SecretBox::new(Box::new(plaintext)))
    }
}

/// Build a 24-byte XChaCha nonce from a stored slice, rejecting the wrong size.
fn nonce_from_slice(bytes: &[u8]) -> Result<XNonce, CipherError> {
    if bytes.len() != 24 {
        return Err(CipherError::Decrypt);
    }
    Ok(*XNonce::from_slice(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic 32-byte test KEK, base64-encoded.
    fn test_key_b64() -> String {
        let key = [7u8; KEY_LEN];
        BASE64.encode(key)
    }

    fn cipher() -> EnvelopeCredentialCipher {
        EnvelopeCredentialCipher::from_base64_key(&test_key_b64()).unwrap()
    }

    fn aad<'a>() -> Aad<'a> {
        Aad {
            org_id: "11111111-1111-1111-1111-111111111111",
            account_id: "22222222-2222-2222-2222-222222222222",
            field: "smtp_password",
        }
    }

    #[test]
    fn round_trip_recovers_plaintext() {
        let c = cipher();
        let secret = b"super-secret-smtp-pw";
        let sealed = c.encrypt(secret, aad()).unwrap();
        let out = c.decrypt(&sealed, aad()).unwrap();
        assert_eq!(out.expose_secret().as_slice(), secret);
    }

    #[test]
    fn ciphertext_is_not_plaintext_and_nonces_are_24_bytes() {
        let c = cipher();
        let secret = b"another-pw";
        let sealed = c.encrypt(secret, aad()).unwrap();
        assert_ne!(sealed.ciphertext, secret);
        assert_eq!(sealed.nonce.len(), 24);
        assert_eq!(sealed.dek_nonce.len(), 24);
        assert_eq!(sealed.key_version, CURRENT_KEY_VERSION);
        // The wrapped DEK is 32 bytes + 16-byte Poly1305 tag.
        assert_eq!(sealed.dek_wrapped.len(), KEY_LEN + 16);
    }

    #[test]
    fn fresh_dek_and_nonce_per_call_yield_distinct_ciphertext() {
        let c = cipher();
        let secret = b"same-input";
        let a = c.encrypt(secret, aad()).unwrap();
        let b = c.encrypt(secret, aad()).unwrap();
        // Random DEK + random nonces => different ciphertext for identical input.
        assert_ne!(a.ciphertext, b.ciphertext);
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.dek_wrapped, b.dek_wrapped);
        // Both still decrypt to the same plaintext.
        assert_eq!(
            c.decrypt(&a, aad()).unwrap().expose_secret().as_slice(),
            secret
        );
        assert_eq!(
            c.decrypt(&b, aad()).unwrap().expose_secret().as_slice(),
            secret
        );
    }

    #[test]
    fn wrong_kek_fails_to_decrypt() {
        let c = cipher();
        let sealed = c.encrypt(b"pw", aad()).unwrap();
        let other = EnvelopeCredentialCipher::from_key_bytes(&[9u8; KEY_LEN]).unwrap();
        assert!(matches!(
            other.decrypt(&sealed, aad()),
            Err(CipherError::Decrypt)
        ));
    }

    #[test]
    fn tampered_ciphertext_fails_auth() {
        let c = cipher();
        let mut sealed = c.encrypt(b"pw", aad()).unwrap();
        sealed.ciphertext[0] ^= 0xff;
        assert!(matches!(
            c.decrypt(&sealed, aad()),
            Err(CipherError::Decrypt)
        ));
    }

    #[test]
    fn tampered_nonce_fails_auth() {
        let c = cipher();
        let mut sealed = c.encrypt(b"pw", aad()).unwrap();
        sealed.nonce[0] ^= 0xff;
        assert!(matches!(
            c.decrypt(&sealed, aad()),
            Err(CipherError::Decrypt)
        ));
    }

    #[test]
    fn tampered_wrapped_dek_fails_auth() {
        let c = cipher();
        let mut sealed = c.encrypt(b"pw", aad()).unwrap();
        sealed.dek_wrapped[0] ^= 0xff;
        assert!(matches!(
            c.decrypt(&sealed, aad()),
            Err(CipherError::Decrypt)
        ));
    }

    #[test]
    fn tampered_dek_nonce_fails_auth() {
        let c = cipher();
        let mut sealed = c.encrypt(b"pw", aad()).unwrap();
        sealed.dek_nonce[0] ^= 0xff;
        assert!(matches!(
            c.decrypt(&sealed, aad()),
            Err(CipherError::Decrypt)
        ));
    }

    #[test]
    fn wrong_aad_org_fails_auth() {
        let c = cipher();
        let sealed = c.encrypt(b"pw", aad()).unwrap();
        let mut bad = aad();
        bad.org_id = "99999999-9999-9999-9999-999999999999";
        assert!(matches!(c.decrypt(&sealed, bad), Err(CipherError::Decrypt)));
    }

    #[test]
    fn wrong_aad_account_fails_auth() {
        let c = cipher();
        let sealed = c.encrypt(b"pw", aad()).unwrap();
        let mut bad = aad();
        bad.account_id = "00000000-0000-0000-0000-000000000000";
        assert!(matches!(c.decrypt(&sealed, bad), Err(CipherError::Decrypt)));
    }

    #[test]
    fn wrong_aad_field_fails_auth() {
        // The crux of envelope AAD-binding: a ciphertext sealed for
        // `smtp_password` must NOT decrypt under the `imap_password` field.
        let c = cipher();
        let sealed = c.encrypt(b"pw", aad()).unwrap();
        let mut bad = aad();
        bad.field = "imap_password";
        assert!(matches!(c.decrypt(&sealed, bad), Err(CipherError::Decrypt)));
    }

    #[test]
    fn aad_encoding_is_unambiguous() {
        // Length-prefixing keeps ("ab","c") distinct from ("a","bc").
        let one = Aad {
            org_id: "ab",
            account_id: "c",
            field: "f",
        }
        .encode();
        let two = Aad {
            org_id: "a",
            account_id: "bc",
            field: "f",
        }
        .encode();
        assert_ne!(one, two);
    }

    #[test]
    fn wrong_key_version_is_rejected() {
        let c = cipher();
        let mut sealed = c.encrypt(b"pw", aad()).unwrap();
        sealed.key_version = 99;
        assert!(matches!(
            c.decrypt(&sealed, aad()),
            Err(CipherError::KeyVersion)
        ));
    }

    #[test]
    fn bad_master_key_inputs_are_rejected() {
        assert!(matches!(
            EnvelopeCredentialCipher::from_base64_key("not!base64!"),
            Err(CipherError::MasterKey)
        ));
        // Valid base64 but wrong length (16 bytes, not 32).
        let short = BASE64.encode([1u8; 16]);
        assert!(matches!(
            EnvelopeCredentialCipher::from_base64_key(&short),
            Err(CipherError::MasterKey)
        ));
        assert!(matches!(
            EnvelopeCredentialCipher::from_key_bytes(&[0u8; 31]),
            Err(CipherError::MasterKey)
        ));
    }

    #[test]
    fn secret_debug_is_redacted() {
        // `SecretBox`'s Debug never prints the secret bytes.
        let c = cipher();
        let sealed = c.encrypt(b"top-secret-value", aad()).unwrap();
        let recovered = c.decrypt(&sealed, aad()).unwrap();
        let dbg = format!("{recovered:?}");
        assert!(
            dbg.contains("REDACTED"),
            "secret Debug must redact, got: {dbg}"
        );
        assert!(!dbg.contains("top-secret-value"));
    }

    #[test]
    fn empty_plaintext_round_trips() {
        let c = cipher();
        let sealed = c.encrypt(b"", aad()).unwrap();
        let out = c.decrypt(&sealed, aad()).unwrap();
        assert!(out.expose_secret().is_empty());
    }

    #[test]
    fn wrong_nonce_length_is_rejected() {
        let c = cipher();
        let mut sealed = c.encrypt(b"pw", aad()).unwrap();
        sealed.nonce.truncate(12);
        assert!(matches!(
            c.decrypt(&sealed, aad()),
            Err(CipherError::Decrypt)
        ));
    }
}
