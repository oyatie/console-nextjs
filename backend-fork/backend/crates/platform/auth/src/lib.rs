//! Authentication platform crate.
//!
//! T0.5 covers server-side passkey ceremonies, ES256 access JWTs, rotating
//! refresh-token families, and static app-link metadata for the RP domain.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod account;
pub mod browser_session;
mod error;
mod jwt;
mod refresh;
mod webauthn;
mod well_known;

pub use account::{authentication_time_tx, delete_self_passkey_tx, lock_account_tx};
pub use error::AuthError;
pub use jwt::{
    AccessClaims, AccessTokenInput, ActorSession, JwtIssuer, JwtSettings, JwtVerifier,
    TenantAccessContext,
};
pub use refresh::{
    RefreshRotation, RefreshToken, RefreshTokenIssue, RefreshTokenStore, RefreshTokenUseError,
};
pub use webauthn::{
    AuthenticationCeremony, AuthenticationOutcome, MobilePasskeyStepUpAssertion,
    MobilePasskeyStepUpBinding, MobilePasskeyStepUpEnvelope, MobilePasskeyStepUpVerificationError,
    MobileStepUpActionKind, MobileStepUpBindingError, PasskeyAuthenticationCredential,
    PasskeyRegistrationCredential, PasskeyRegistrationStart, PasskeyService, RegistrationCeremony,
    StoredPasskey, WebauthnSettings,
};
pub use well_known::{
    AndroidAssetLinksConfig, AppleAppSiteAssociationConfig, WELL_KNOWN_AASA_PATH,
    WELL_KNOWN_ASSETLINKS_PATH, android_assetlinks_json, apple_app_site_association_json,
};
