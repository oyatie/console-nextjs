//! Machine-only Basic authentication for production source ingress.
//!
//! The opaque secret is never persisted.  The database holds an HMAC verifier
//! that is bound to the service principal, tenant, branch, generation, and the
//! single feature this credential may exercise.

use base64::Engine as _;
use console_kernel_core::{BranchId, OrgId, ServicePrincipalId};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

pub const PRODUCTION_SOURCE_INGEST_FEATURE: &str = "production_source_ingest";

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct BasicCredentials {
    pub(crate) client_id: ServicePrincipalId,
    secret: [u8; 32],
}

impl std::fmt::Debug for BasicCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BasicCredentials")
            .field("client_id", &self.client_id)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

impl BasicCredentials {
    pub(crate) const fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
}

/// Parse the only accepted wire format. Any malformed form deliberately maps
/// to the same caller-facing authentication failure as an unknown credential.
pub(crate) fn parse_basic_credentials(value: Option<&str>) -> Option<BasicCredentials> {
    let encoded = value?.strip_prefix("Basic ")?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let split = decoded.iter().position(|byte| *byte == b':')?;
    if decoded[split + 1..].len() != 32 {
        return None;
    }
    let client_id = std::str::from_utf8(&decoded[..split]).ok()?.parse().ok()?;
    let mut secret = [0_u8; 32];
    secret.copy_from_slice(&decoded[split + 1..]);
    Some(BasicCredentials { client_id, secret })
}

// `Hmac::new_from_slice` is infallible for every input `Hmac` accepts: keys
// shorter than the block size are zero-padded and longer ones are hashed down,
// so `InvalidLength` is unreachable — and the key here is a fixed `[u8; 32]`.
// Propagating a `Result` would add a dead error branch to an auth path.
#[allow(clippy::expect_used)]
#[must_use]
pub(crate) fn verifier(
    key: &[u8; 32],
    secret: &[u8; 32],
    org_id: OrgId,
    principal_id: ServicePrincipalId,
    branch_id: BranchId,
    generation: i32,
) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("32-byte HMAC keys are valid");
    mac.update(b"console.production.service-principal.v1\0");
    mac.update(org_id.as_uuid().as_bytes());
    mac.update(principal_id.as_uuid().as_bytes());
    mac.update(branch_id.as_uuid().as_bytes());
    mac.update(&generation.to_be_bytes());
    mac.update(PRODUCTION_SOURCE_INGEST_FEATURE.as_bytes());
    mac.update(b"\0");
    mac.update(secret);
    mac.finalize().into_bytes().into()
}

#[must_use]
pub(crate) fn verifier_matches(expected: &[u8], actual: &[u8; 32]) -> bool {
    expected.ct_eq(actual).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_requires_basic_uuid_and_exact_32_byte_secret() {
        let id = ServicePrincipalId::new();
        let mut wire = id.to_string().into_bytes();
        wire.push(b':');
        wire.extend([7_u8; 32]);
        let header = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(wire)
        );
        assert_eq!(
            parse_basic_credentials(Some(&header)).unwrap().client_id,
            id
        );
        assert!(parse_basic_credentials(Some("Bearer nope")).is_none());
        assert!(parse_basic_credentials(Some("Basic bm90LWEtdXVpZDph")).is_none());
    }

    #[test]
    fn verifier_is_bound_to_every_authority_dimension() {
        let key = [1_u8; 32];
        let secret = [2_u8; 32];
        let org = OrgId::new();
        let principal = ServicePrincipalId::new();
        let branch = BranchId::new();
        let expected = verifier(&key, &secret, org, principal, branch, 3);
        assert!(verifier_matches(&expected, &expected));
        assert_ne!(
            expected,
            verifier(&key, &secret, OrgId::new(), principal, branch, 3)
        );
        assert_ne!(
            expected,
            verifier(&key, &secret, org, ServicePrincipalId::new(), branch, 3)
        );
        assert_ne!(
            expected,
            verifier(&key, &secret, org, principal, BranchId::new(), 3)
        );
        assert_ne!(expected, verifier(&key, &secret, org, principal, branch, 4));
    }
}
