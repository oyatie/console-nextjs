use console_kernel_core::{AuditAction, AuditEvent, KernelError, OrgId, TraceContext, UserId};
use console_platform_db::insert_audit_event;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use time::{Duration, OffsetDateTime};
use url::Url;
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, DiscoverableAuthentication, DiscoverableKey, Passkey,
    PasskeyRegistration, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse, Webauthn, WebauthnBuilder,
};

use crate::AuthError;

pub type PasskeyRegistrationCredential = RegisterPublicKeyCredential;
pub type PasskeyAuthenticationCredential = PublicKeyCredential;

#[derive(Debug, Clone)]
pub struct WebauthnSettings {
    pub rp_id: String,
    pub rp_origin: Url,
    pub rp_name: String,
    pub extra_allowed_origins: Vec<Url>,
    pub ceremony_ttl: Duration,
}

#[derive(Clone)]
pub struct PasskeyService {
    webauthn: Webauthn,
    ceremony_ttl: Duration,
}

#[derive(Debug, Clone)]
pub struct PasskeyRegistrationStart {
    pub user_id: Uuid,
    pub username: String,
    pub display_name: String,
}

#[derive(Debug)]
pub struct RegistrationCeremony {
    pub ceremony_id: Uuid,
    pub challenge: CreationChallengeResponse,
    pub expires_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct AuthenticationCeremony {
    pub ceremony_id: Uuid,
    pub challenge: RequestChallengeResponse,
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MobileStepUpActionKind {
    ApprovalDecision,
    PollVote,
}

impl MobileStepUpActionKind {
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::ApprovalDecision => "APPROVAL_DECISION",
            Self::PollVote => "POLL_VOTE",
        }
    }

    pub const fn expected_reason_key(self) -> &'static str {
        match self {
            Self::ApprovalDecision => "operations_passkey_approval_decision",
            Self::PollVote => "operations_passkey_poll_vote",
        }
    }

    fn from_wire(raw: &str) -> Result<Self, AuthError> {
        match raw {
            "APPROVAL_DECISION" => Ok(Self::ApprovalDecision),
            "POLL_VOTE" => Ok(Self::PollVote),
            _ => Err(AuthError::InvalidStoredData(format!(
                "unknown mobile step-up action kind: {raw}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MobilePasskeyStepUpBinding {
    pub action_kind: MobileStepUpActionKind,
    pub object_id: Uuid,
    pub reason_key: String,
    pub replay_attempt: Option<i32>,
}

impl<'de> Deserialize<'de> for MobilePasskeyStepUpBinding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct BindingVisitor;

        enum Field {
            ActionKind,
            ObjectId,
            ReasonKey,
            ReplayAttempt,
            Ignore,
        }

        impl<'de> Deserialize<'de> for Field {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct FieldVisitor;

                impl serde::de::Visitor<'_> for FieldVisitor {
                    type Value = Field;

                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str("a mobile passkey step-up binding field")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        Ok(match value {
                            "action_kind" => Field::ActionKind,
                            "object_id" => Field::ObjectId,
                            "reason_key" => Field::ReasonKey,
                            "replay_attempt" => Field::ReplayAttempt,
                            _ => Field::Ignore,
                        })
                    }
                }

                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        impl<'de> serde::de::Visitor<'de> for BindingVisitor {
            type Value = MobilePasskeyStepUpBinding;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a mobile passkey step-up binding")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut action_kind = None;
                let mut object_id = None;
                let mut reason_key = None;
                let mut replay_attempt = None;

                while let Some(key) = map.next_key::<Field>()? {
                    match key {
                        Field::ActionKind => {
                            if action_kind.is_some() {
                                return Err(serde::de::Error::duplicate_field("action_kind"));
                            }
                            action_kind = Some(map.next_value()?);
                        }
                        Field::ObjectId => {
                            if object_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("object_id"));
                            }
                            object_id = Some(map.next_value()?);
                        }
                        Field::ReasonKey => {
                            if reason_key.is_some() {
                                return Err(serde::de::Error::duplicate_field("reason_key"));
                            }
                            reason_key = Some(map.next_value()?);
                        }
                        Field::ReplayAttempt => {
                            if replay_attempt.is_some() {
                                return Err(serde::de::Error::duplicate_field("replay_attempt"));
                            }
                            replay_attempt = Some(map.next_value::<Option<i32>>()?);
                        }
                        Field::Ignore => {
                            let _ = map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }

                Ok(MobilePasskeyStepUpBinding {
                    action_kind: action_kind
                        .ok_or_else(|| serde::de::Error::missing_field("action_kind"))?,
                    object_id: object_id
                        .ok_or_else(|| serde::de::Error::missing_field("object_id"))?,
                    reason_key: reason_key
                        .ok_or_else(|| serde::de::Error::missing_field("reason_key"))?,
                    replay_attempt: replay_attempt
                        .ok_or_else(|| serde::de::Error::missing_field("replay_attempt"))?,
                })
            }
        }

        const FIELDS: &[&str] = &["action_kind", "object_id", "reason_key", "replay_attempt"];
        deserializer.deserialize_struct("MobilePasskeyStepUpBinding", FIELDS, BindingVisitor)
    }
}

impl MobilePasskeyStepUpBinding {
    pub fn approval_decision(object_id: Uuid, replay_attempt: Option<i32>) -> Self {
        Self::new(
            MobileStepUpActionKind::ApprovalDecision,
            object_id,
            replay_attempt,
        )
    }

    pub fn poll_vote(object_id: Uuid, replay_attempt: Option<i32>) -> Self {
        Self::new(MobileStepUpActionKind::PollVote, object_id, replay_attempt)
    }

    fn new(
        action_kind: MobileStepUpActionKind,
        object_id: Uuid,
        replay_attempt: Option<i32>,
    ) -> Self {
        Self {
            action_kind,
            object_id,
            reason_key: action_kind.expected_reason_key().to_owned(),
            replay_attempt,
        }
    }

    pub fn validate(&self) -> Result<(), MobileStepUpBindingError> {
        if self.reason_key != self.action_kind.expected_reason_key() {
            return Err(MobileStepUpBindingError::ReasonKeyMismatch);
        }
        if self.replay_attempt.is_some_and(|attempt| attempt < 1) {
            return Err(MobileStepUpBindingError::InvalidReplayAttempt);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MobileStepUpBindingError {
    #[error("reason_key is not supported for action_kind")]
    ReasonKeyMismatch,

    #[error("replay_attempt must be null or a positive 1-based integer")]
    InvalidReplayAttempt,
}

#[derive(Debug, Deserialize)]
pub struct MobilePasskeyStepUpAssertion {
    pub ceremony_id: Uuid,
    pub credential: PasskeyAuthenticationCredential,
}

#[derive(Debug, Deserialize)]
pub struct MobilePasskeyStepUpEnvelope {
    pub binding: MobilePasskeyStepUpBinding,
    pub assertion: MobilePasskeyStepUpAssertion,
}

#[derive(Debug, thiserror::Error)]
pub enum MobilePasskeyStepUpVerificationError {
    #[error("mobile passkey step-up binding mismatch")]
    BindingMismatch,

    #[error(transparent)]
    Auth(#[from] AuthError),
}

impl From<sqlx::Error> for MobilePasskeyStepUpVerificationError {
    fn from(value: sqlx::Error) -> Self {
        Self::Auth(value.into())
    }
}

impl From<serde_json::Error> for MobilePasskeyStepUpVerificationError {
    fn from(value: serde_json::Error) -> Self {
        Self::Auth(value.into())
    }
}

impl From<webauthn_rs::prelude::WebauthnError> for MobilePasskeyStepUpVerificationError {
    fn from(value: webauthn_rs::prelude::WebauthnError) -> Self {
        Self::Auth(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPasskey {
    pub id: Uuid,
    pub user_id: Uuid,
    pub credential_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationOutcome {
    pub user_id: Uuid,
    pub passkey_id: Uuid,
    /// The tenant the asserting credential belongs to, resolved from the
    /// credential id BEFORE the RLS-gated read. The login handler uses it to arm
    /// the GUC for the subsequent `users` read + session mint, since the passkey
    /// login route runs before the tenant middleware.
    pub org_id: OrgId,
}

impl PasskeyService {
    pub fn new(settings: WebauthnSettings) -> Result<Self, AuthError> {
        let mut builder =
            WebauthnBuilder::new(&settings.rp_id, &settings.rp_origin)?.rp_name(&settings.rp_name);
        for origin in &settings.extra_allowed_origins {
            builder = builder.append_allowed_origin(origin);
        }
        Ok(Self {
            webauthn: builder.build()?,
            ceremony_ttl: settings.ceremony_ttl,
        })
    }

    pub async fn start_registration(
        &self,
        pool: &PgPool,
        org: OrgId,
        input: PasskeyRegistrationStart,
    ) -> Result<RegistrationCeremony, AuthError> {
        self.start_registration_with_discoverability(pool, org, input, false)
            .await
    }

    /// Request a resident credential for future usernameless sign-in.
    /// This selection does not certify the authenticator's storage properties.
    pub async fn start_discoverable_registration(
        &self,
        pool: &PgPool,
        org: OrgId,
        input: PasskeyRegistrationStart,
    ) -> Result<RegistrationCeremony, AuthError> {
        self.start_registration_with_discoverability(pool, org, input, true)
            .await
    }

    async fn start_registration_with_discoverability(
        &self,
        pool: &PgPool,
        org: OrgId,
        input: PasskeyRegistrationStart,
        require_discoverable: bool,
    ) -> Result<RegistrationCeremony, AuthError> {
        // Authenticated path: `org` comes from the verified JWT's `org` claim.
        // `load_user_passkeys` reads the FORCE-RLS `auth_webauthn_credentials`, so
        // the org is armed inside it to avoid an empty exclude-credentials list
        // (which would let a user re-register an already-registered authenticator).
        let existing = load_user_passkeys(pool, org, input.user_id).await?;
        let exclude_credentials = existing
            .into_iter()
            .map(|passkey| passkey.cred_id().clone())
            .collect::<Vec<_>>();
        let exclude_credentials = if exclude_credentials.is_empty() {
            None
        } else {
            Some(exclude_credentials)
        };

        let (mut challenge, state) = self.webauthn.start_passkey_registration(
            input.user_id,
            &input.username,
            &input.display_name,
            exclude_credentials,
        )?;
        if require_discoverable {
            let selection = challenge
                .public_key
                .authenticator_selection
                .as_mut()
                .ok_or_else(|| {
                    AuthError::InvalidStoredData(
                        "registration challenge has no authenticator selection".to_owned(),
                    )
                })?;
            // webauthn-rs does not re-export this public field's enum.
            selection.resident_key = Some(serde_json::from_str("\"required\"")?);
            selection.require_resident_key = true;
        }
        let ceremony_id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        let expires_at = now + self.ceremony_ttl;

        persist_ceremony(
            pool,
            ceremony_id,
            Some(input.user_id),
            "registration",
            &challenge,
            &state,
            expires_at,
        )
        .await?;

        Ok(RegistrationCeremony {
            ceremony_id,
            challenge,
            expires_at,
        })
    }

    /// Count the authenticated user's existing passkeys (RLS-armed).
    ///
    /// Used by the add-device flow to decide whether a fresh step-up assertion is
    /// REQUIRED: a user with zero passkeys is doing initial enrollment (no
    /// existing credential to assert), while a user with one or more must prove
    /// possession of an existing passkey before a new one is issued.
    pub async fn count_user_passkeys(
        &self,
        pool: &PgPool,
        org: OrgId,
        user_id: Uuid,
    ) -> Result<usize, AuthError> {
        Ok(load_user_passkeys(pool, org, user_id).await?.len())
    }

    /// Verify a FRESH step-up assertion of one of `expected_user_id`'s OWN
    /// existing passkeys, with user verification (UV) required.
    ///
    /// This is the anti-silent-add gate for self-service device enrollment: before
    /// a new credential is issued to an already-enrolled user, the caller must
    /// assert an existing passkey of THE SAME user with UV=true, so a stolen
    /// session (bearer token only, no authenticator) cannot add a credential.
    ///
    /// The assertion ceremony is claimed atomically (single-use, like login), the
    /// discoverable assertion is verified against the resolved credential, and the
    /// assertion is rejected unless (a) `user_verified()` is true and (b) the
    /// asserting credential belongs to `expected_user_id`. The credential's org is
    /// resolved + the GUC armed exactly as in `finish_authentication`, but NO
    /// token is minted and NO session is created — this only proves possession.
    pub async fn verify_step_up_for_user(
        &self,
        pool: &PgPool,
        ceremony_id: Uuid,
        credential: PublicKeyCredential,
        expected_user_id: Uuid,
    ) -> Result<(), AuthError> {
        let mut tx = pool.begin().await?;
        let locator = lock_assertion_account_tx(&mut tx, &credential).await?;
        let claim = lock_ceremony_tx(&mut tx, ceremony_id, "authentication").await?;
        self.verify_assertion_in_tx(
            &mut tx,
            ceremony_id,
            claim,
            locator,
            credential,
            Some(expected_user_id),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn start_mobile_step_up(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        binding: MobilePasskeyStepUpBinding,
    ) -> Result<AuthenticationCeremony, AuthError> {
        binding
            .validate()
            .map_err(|err| AuthError::InvalidStoredData(err.to_string()))?;
        let (challenge, state) = self.webauthn.start_discoverable_authentication()?;
        let ceremony_id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        let expires_at = now + self.ceremony_ttl;

        let mut tx = pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO auth_webauthn_ceremonies (
                id, user_id, ceremony_kind, challenge_json, state_json, expires_at
            ) VALUES ($1, $2, 'authentication', $3, $4, $5)
            "#,
        )
        .bind(ceremony_id)
        .bind(user_id)
        .bind(serde_json::to_value(&challenge)?)
        .bind(serde_json::to_value(&state)?)
        .bind(expires_at)
        // rls-arming: ok auth_webauthn_ceremonies is a global auth table (no org_id, no RLS)
        .execute(tx.as_mut())
        .await?;
        insert_mobile_step_up_binding_tx(&mut tx, ceremony_id, &binding).await?;
        tx.commit().await?;

        Ok(AuthenticationCeremony {
            ceremony_id,
            challenge,
            expires_at,
        })
    }

    pub async fn verify_mobile_step_up_for_user(
        &self,
        pool: &PgPool,
        envelope: MobilePasskeyStepUpEnvelope,
        expected_user_id: Uuid,
        expected_binding: &MobilePasskeyStepUpBinding,
    ) -> Result<(), MobilePasskeyStepUpVerificationError> {
        if envelope.binding != *expected_binding {
            return Err(MobilePasskeyStepUpVerificationError::BindingMismatch);
        }

        let mut tx = pool.begin().await?;
        let ceremony_id = envelope.assertion.ceremony_id;
        let locator = lock_assertion_account_tx(&mut tx, &envelope.assertion.credential).await?;
        let claim = lock_ceremony_tx(&mut tx, ceremony_id, "authentication").await?;
        if claim.user_id != Some(expected_user_id) {
            return Err(AuthError::InvalidStoredData(
                "step-up ceremony does not belong to the authenticated user".to_owned(),
            )
            .into());
        }
        let persisted = load_mobile_step_up_binding_tx(&mut tx, ceremony_id)
            .await?
            .ok_or(MobilePasskeyStepUpVerificationError::BindingMismatch)?;
        if persisted != *expected_binding {
            return Err(MobilePasskeyStepUpVerificationError::BindingMismatch);
        }

        self.verify_assertion_in_tx(
            &mut tx,
            ceremony_id,
            claim,
            locator,
            envelope.assertion.credential,
            Some(expected_user_id),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn finish_registration(
        &self,
        pool: &PgPool,
        org: OrgId,
        ceremony_id: Uuid,
        credential: RegisterPublicKeyCredential,
    ) -> Result<StoredPasskey, AuthError> {
        let now = OffsetDateTime::now_utc();
        let mut tx = pool.begin().await?;
        let stored = self
            .finish_registration_in_tx(&mut tx, org, ceremony_id, credential, now)
            .await?;
        tx.commit().await?;
        Ok(stored)
    }

    /// Finish a passkey registration inside a caller-provided transaction.
    ///
    /// Performs the atomic ceremony claim, verifies the credential against the
    /// locked state, inserts the passkey, and appends the audit row — all in the
    /// caller's transaction. The bootstrap cold-start path uses this so the
    /// passkey insert and the single-use bootstrap-credential consume commit (or
    /// roll back) atomically together. The caller owns commit/rollback, including
    /// after cancellation. The legacy time argument is retained for compatibility;
    /// admission and accepted timestamps use the database clock.
    pub async fn finish_registration_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        org: OrgId,
        ceremony_id: Uuid,
        credential: RegisterPublicKeyCredential,
        _now: OffsetDateTime,
    ) -> Result<StoredPasskey, AuthError> {
        // The caller is AUTHENTICATED: `org` comes from the verified JWT's `org`
        // claim (never read from a user row under RLS — chicken-and-egg). Arm the
        // tenant GUC for this transaction so the FORCE-RLS WITH CHECK on
        // `auth_webauthn_credentials` (migration 0035) accepts the passkey INSERT
        // stamped with THIS org, and the consume of the bootstrap credential in
        // the same caller transaction also passes.
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org.as_uuid().to_string())
            .execute(tx.as_mut())
            .await?;

        let user_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT user_id FROM auth_webauthn_ceremonies WHERE id = $1 AND ceremony_kind = 'registration'")
            .bind(ceremony_id).fetch_optional(tx.as_mut()).await?.flatten();
        let user_id = user_id.ok_or_else(invalid_ceremony)?;
        match crate::lock_account_tx(tx, org, user_id).await? {
            None => return Err(KernelError::not_found("user not found").into()),
            Some(false) => {
                return Err(KernelError::conflict(
                    "비활성화된 사용자는 패스키를 등록할 수 없습니다.",
                )
                .into());
            }
            Some(true) => {}
        }
        let claim = lock_ceremony_tx(tx, ceremony_id, "registration").await?;
        if claim.user_id != Some(user_id) {
            return Err(invalid_ceremony());
        }

        // Failed verification leaves no accepted consumption or registration.
        let state: PasskeyRegistration = serde_json::from_value(claim.state_json)?;
        let passkey = self
            .webauthn
            .finish_passkey_registration(&credential, &state)?;
        let passkey_json = serde_json::to_value(&passkey)?;
        let credential_id = serialize_to_string(passkey.cred_id(), "passkey credential id")?;
        let passkey_id = Uuid::new_v4();
        let now = consume_ceremony_tx(tx, ceremony_id).await?;

        sqlx::query(
            r#"
            INSERT INTO auth_webauthn_credentials (
                id, user_id, credential_id, passkey_json, created_at, org_id
            ) VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(passkey_id)
        .bind(user_id)
        .bind(&credential_id)
        .bind(passkey_json)
        .bind(now)
        .bind(*org.as_uuid())
        .execute(tx.as_mut())
        .await?;

        let audit = AuditEvent::new(
            Some(UserId::from_uuid(user_id)),
            AuditAction::new("auth.passkey.register")?,
            "auth_webauthn_credential",
            passkey_id.to_string(),
            TraceContext::generate(),
            now,
        )
        .with_org(org)
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "credential_id": credential_id,
                "user_id": user_id,
            })),
        );
        insert_audit_event(tx, &audit).await?;

        Ok(StoredPasskey {
            id: passkey_id,
            user_id,
            credential_id,
        })
    }

    /// Start a usernameless (discoverable) authentication ceremony.
    ///
    /// The challenge carries an EMPTY `allowCredentials` list: the client
    /// discovers the resident credential to use without the server naming a user.
    /// The persisted ceremony has a NULL `user_id` because the asserting user is
    /// only known once the client returns the credential at finish time.
    pub async fn start_authentication(
        &self,
        pool: &PgPool,
    ) -> Result<AuthenticationCeremony, AuthError> {
        self.start_authentication_with_mediation(pool, false).await
    }

    /// Start usernameless sign-in with ordinary browser mediation.
    pub async fn start_explicit_authentication(
        &self,
        pool: &PgPool,
    ) -> Result<AuthenticationCeremony, AuthError> {
        self.start_authentication_with_mediation(pool, true).await
    }

    async fn start_authentication_with_mediation(
        &self,
        pool: &PgPool,
        explicit: bool,
    ) -> Result<AuthenticationCeremony, AuthError> {
        let (mut challenge, state) = self.webauthn.start_discoverable_authentication()?;
        if explicit {
            challenge.mediation = None;
        }
        let ceremony_id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        let expires_at = now + self.ceremony_ttl;

        persist_ceremony(
            pool,
            ceremony_id,
            None,
            "authentication",
            &challenge,
            &state,
            expires_at,
        )
        .await?;

        Ok(AuthenticationCeremony {
            ceremony_id,
            challenge,
            expires_at,
        })
    }

    /// Finish a usernameless (discoverable) authentication ceremony.
    ///
    /// The user is resolved FROM the asserted credential — by credential id,
    /// which is unique per credential and always present in the assertion — so no
    /// `user_id` is required from the client. When the authenticator returns a
    /// user handle (a true resident key), it is cross-checked against the
    /// resolved credential's owner. The atomic single-use ceremony claim from the
    /// harden-1 fix is preserved under a row lock, so a replayed ceremony is rejected.
    pub async fn finish_authentication(
        &self,
        pool: &PgPool,
        ceremony_id: Uuid,
        credential: PublicKeyCredential,
    ) -> Result<AuthenticationOutcome, AuthError> {
        let mut tx = pool.begin().await?;
        let outcome = self
            .finish_authentication_in_tx(&mut tx, ceremony_id, credential)
            .await?;
        tx.commit().await?;
        Ok(outcome)
    }

    /// Verify and consume a signed login proof in the caller's transaction.
    /// Session creation and its audit must commit with this proof, or all roll back.
    pub async fn finish_authentication_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        ceremony_id: Uuid,
        credential: PublicKeyCredential,
    ) -> Result<AuthenticationOutcome, AuthError> {
        let locator = lock_assertion_account_tx(tx, &credential).await?;
        let claim = lock_ceremony_tx(tx, ceremony_id, "authentication").await?;
        self.verify_assertion_in_tx(tx, ceremony_id, claim, locator, credential, None)
            .await
    }

    // Mobile and generic step-up retain their own binding checks, while all
    // assertion paths serialize the same key read, WebAuthn verification and save.
    async fn verify_assertion_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        ceremony_id: Uuid,
        claim: CeremonyRow,
        locator: (Uuid, Uuid, Uuid),
        credential: PublicKeyCredential,
        expected_user_id: Option<Uuid>,
    ) -> Result<AuthenticationOutcome, AuthError> {
        let credential_id =
            serialize_to_string(&credential.raw_id, "authentication credential id")?;

        let (org_uuid, expected_key, locked_user) = locator;
        let row = sqlx::query(
            r#"
            SELECT id, user_id, passkey_json
            FROM auth_webauthn_credentials
            WHERE credential_id = $1 AND org_id = $2 AND id = $3 AND user_id = $4
            FOR UPDATE
            "#,
        )
        .bind(&credential_id)
        .bind(org_uuid)
        .bind(expected_key)
        .bind(locked_user)
        .fetch_optional(tx.as_mut())
        .await?
        .ok_or_else(|| {
            AuthError::InvalidStoredData("asserted credential is not registered".to_owned())
        })?;
        let passkey_id: Uuid = row.try_get("id")?;
        let user_id: Uuid = row.try_get("user_id")?;
        let passkey_json: serde_json::Value = row.try_get("passkey_json")?;

        if expected_user_id.is_some_and(|expected| expected != user_id) {
            return Err(AuthError::InvalidStoredData(
                "step-up credential does not belong to the authenticated user".to_owned(),
            ));
        }
        if let Some(asserted_handle) = credential.get_user_unique_id()
            && Uuid::from_slice(asserted_handle).ok() != Some(user_id)
        {
            return Err(AuthError::InvalidStoredData(
                "asserted user handle does not match the credential owner".to_owned(),
            ));
        }

        // Verify against the locked ceremony and latest locked credential.
        // A failure leaves the ceremony unconsumed and rolls back key changes.
        let state: DiscoverableAuthentication = serde_json::from_value(claim.state_json)?;
        let mut passkey: Passkey = serde_json::from_value(passkey_json)?;
        let discoverable_key = DiscoverableKey::from(&passkey);
        let result = self.webauthn.finish_discoverable_authentication(
            &credential,
            state,
            &[discoverable_key],
        )?;
        if expected_user_id.is_some() && !result.user_verified() {
            return Err(AuthError::InvalidStoredData(
                "step-up assertion did not perform user verification".to_owned(),
            ));
        }

        // Both rows are locked. Admit only after crypto and all lock waits,
        // using the same database instant for expiry and accepted timestamps.
        let now = consume_ceremony_tx(tx, ceremony_id).await?;
        let changed = passkey.update_credential(&result).unwrap_or(false);

        if changed {
            sqlx::query(
                r#"
                UPDATE auth_webauthn_credentials
                SET passkey_json = $1, last_used_at = $2
                WHERE id = $3
                "#,
            )
            .bind(serde_json::to_value(&passkey)?)
            .bind(now)
            .bind(passkey_id)
            .execute(tx.as_mut())
            .await?;
        } else {
            sqlx::query("UPDATE auth_webauthn_credentials SET last_used_at = $1 WHERE id = $2")
                .bind(now)
                .bind(passkey_id)
                .execute(tx.as_mut())
                .await?;
        }

        Ok(AuthenticationOutcome {
            user_id,
            passkey_id,
            org_id: OrgId::from_uuid(org_uuid),
        })
    }
}

// Resolve without child locks, then freeze the exact Account. The verifier
// rereads this key identity after the wait; a replacement cannot change subject.
async fn lock_assertion_account_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    credential: &PublicKeyCredential,
) -> Result<(Uuid, Uuid, Uuid), AuthError> {
    let id = serialize_to_string(&credential.raw_id, "authentication credential id")?;
    let org = resolve_credential_org(tx, &id)
        .await?
        .ok_or_else(invalid_ceremony)?;
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.to_string())
        .execute(tx.as_mut())
        .await?;
    let (key, user): (Uuid, Uuid) = sqlx::query_as(
        "SELECT id, user_id FROM auth_webauthn_credentials WHERE credential_id = $1 AND org_id = $2")
        .bind(id).bind(org).fetch_optional(tx.as_mut()).await?.ok_or_else(invalid_ceremony)?;
    if crate::lock_account_tx(tx, OrgId::from_uuid(org), user).await? != Some(true) {
        return Err(invalid_ceremony());
    }
    Ok((org, key, user))
}

struct CeremonyRow {
    user_id: Option<Uuid>,
    state_json: serde_json::Value,
}

async fn persist_ceremony<C, S>(
    pool: &PgPool,
    id: Uuid,
    user_id: Option<Uuid>,
    kind: &str,
    challenge: &C,
    state: &S,
    expires_at: OffsetDateTime,
) -> Result<(), AuthError>
where
    C: serde::Serialize,
    S: serde::Serialize,
{
    sqlx::query(
        r#"
        INSERT INTO auth_webauthn_ceremonies (
            id, user_id, ceremony_kind, challenge_json, state_json, expires_at
        ) VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(id)
    .bind(user_id)
    .bind(kind)
    .bind(serde_json::to_value(challenge)?)
    .bind(serde_json::to_value(state)?)
    .bind(expires_at)
    // rls-arming: ok auth_webauthn_ceremonies is a global pre-auth table (no org_id, no RLS)
    .execute(pool)
    .await?;
    Ok(())
}

fn invalid_ceremony() -> AuthError {
    AuthError::InvalidStoredData("ceremony not found or already consumed".to_owned())
}

// The lock spans verification and final consumption. Failed or cancelled owning
// transactions roll back; borrowing callers must themselves roll back or drop.
async fn lock_ceremony_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    kind: &str,
) -> Result<CeremonyRow, AuthError> {
    let row = sqlx::query(
        r#"
        SELECT user_id, state_json, expires_at
        FROM auth_webauthn_ceremonies
        WHERE id = $1 AND ceremony_kind = $2 AND consumed_at IS NULL
        FOR UPDATE
        "#,
    )
    .bind(id)
    .bind(kind)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(invalid_ceremony)?;

    // A row-lock wait can outlive the deadline. now() is transaction-start time.
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(tx.as_mut())
        .await?;
    let expires_at: OffsetDateTime = row.try_get("expires_at")?;
    if expires_at <= now {
        return Err(invalid_ceremony());
    }
    Ok(CeremonyRow {
        user_id: row.try_get("user_id")?,
        state_json: row.try_get("state_json")?,
    })
}

// Called only while holding the ceremony and all relevant Account/key locks.
// A single sampled instant defines proof admission, not commit or delivery time.
async fn consume_ceremony_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<OffsetDateTime, AuthError> {
    sqlx::query_scalar(
        r#"
        WITH admission AS MATERIALIZED (SELECT clock_timestamp() AS instant)
        UPDATE auth_webauthn_ceremonies c
        SET consumed_at = admission.instant
        FROM admission
        WHERE c.id = $1 AND c.consumed_at IS NULL AND c.expires_at > admission.instant
        RETURNING c.consumed_at
        "#,
    )
    .bind(id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(invalid_ceremony)
}

async fn insert_mobile_step_up_binding_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ceremony_id: Uuid,
    binding: &MobilePasskeyStepUpBinding,
) -> Result<(), AuthError> {
    sqlx::query(
        r#"
        INSERT INTO auth_webauthn_ceremony_bindings (
            ceremony_id, action_kind, object_id, reason_key, replay_attempt
        ) VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(ceremony_id)
    .bind(binding.action_kind.as_wire())
    .bind(binding.object_id)
    .bind(&binding.reason_key)
    .bind(binding.replay_attempt)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn load_mobile_step_up_binding_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ceremony_id: Uuid,
) -> Result<Option<MobilePasskeyStepUpBinding>, AuthError> {
    let row = sqlx::query(
        r#"
        SELECT action_kind, object_id, reason_key, replay_attempt
        FROM auth_webauthn_ceremony_bindings
        WHERE ceremony_id = $1
        "#,
    )
    .bind(ceremony_id)
    .fetch_optional(tx.as_mut())
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };
    let action_kind: String = row.try_get("action_kind")?;
    Ok(Some(MobilePasskeyStepUpBinding {
        action_kind: MobileStepUpActionKind::from_wire(&action_kind)?,
        object_id: row.try_get("object_id")?,
        reason_key: row.try_get("reason_key")?,
        replay_attempt: row.try_get("replay_attempt")?,
    }))
}

/// Resolve a webauthn credential's tenant from its credential id, via the narrow
/// SECURITY DEFINER resolver `platform_resolve_credential_org` (migration 0038).
///
/// `auth_webauthn_credentials` is FORCE RLS, so the app's non-owner `console_rt` role
/// cannot read a credential row by credential id until `app.current_org` is armed
/// — but the org is exactly what we need to arm it. This resolver returns ONLY the
/// org_id, breaking that chicken-and-egg without widening any read surface.
/// Returns `None` for an unknown credential id.
async fn resolve_credential_org(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    credential_id: &str,
) -> Result<Option<Uuid>, AuthError> {
    Ok(
        sqlx::query_scalar("SELECT platform_resolve_credential_org($1)")
            .bind(credential_id)
            .fetch_one(tx.as_mut())
            .await?,
    )
}

async fn load_user_passkeys(
    pool: &PgPool,
    org: OrgId,
    user_id: Uuid,
) -> Result<Vec<Passkey>, AuthError> {
    // `auth_webauthn_credentials` is FORCE RLS; arm the tenant GUC for this
    // transaction so the non-owner `console_rt` role sees the user's existing
    // passkeys. The org is the authenticated request's verified tenant.
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await?;
    let rows = sqlx::query(
        "SELECT passkey_json FROM auth_webauthn_credentials WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(tx.as_mut())
    .await?;
    tx.commit().await?;

    rows.into_iter()
        .map(|row| {
            let value: serde_json::Value = row.try_get("passkey_json")?;
            Ok(serde_json::from_value(value)?)
        })
        .collect()
}

fn serialize_to_string<T>(value: &T, label: &str) -> Result<String, AuthError>
where
    T: serde::Serialize,
{
    let value = serde_json::to_value(value)?;
    value
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| AuthError::InvalidStoredData(format!("{label} did not serialize as string")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mobile_step_up_binding_replay_attempt_is_required_but_nullable() {
        let object_id = Uuid::nil();
        let missing_replay_attempt = json!({
            "action_kind": "APPROVAL_DECISION",
            "object_id": object_id,
            "reason_key": "operations_passkey_approval_decision"
        });

        let error = serde_json::from_value::<MobilePasskeyStepUpBinding>(missing_replay_attempt)
            .expect_err("replay_attempt must be present, even when null");
        assert!(error.to_string().contains("replay_attempt"));

        let online_binding: MobilePasskeyStepUpBinding = serde_json::from_value(json!({
            "action_kind": "APPROVAL_DECISION",
            "object_id": object_id,
            "reason_key": "operations_passkey_approval_decision",
            "replay_attempt": null
        }))
        .unwrap();
        assert_eq!(online_binding.replay_attempt, None);

        let replay_binding: MobilePasskeyStepUpBinding = serde_json::from_value(json!({
            "action_kind": "POLL_VOTE",
            "object_id": object_id,
            "reason_key": "operations_passkey_poll_vote",
            "replay_attempt": 1
        }))
        .unwrap();
        assert_eq!(replay_binding.replay_attempt, Some(1));
    }

    #[test]
    fn mobile_step_up_binding_rejects_zero_replay_attempt() {
        let binding: MobilePasskeyStepUpBinding = serde_json::from_value(json!({
            "action_kind": "APPROVAL_DECISION",
            "object_id": Uuid::nil(),
            "reason_key": "operations_passkey_approval_decision",
            "replay_attempt": 0
        }))
        .unwrap();

        assert!(matches!(
            binding.validate(),
            Err(MobileStepUpBindingError::InvalidReplayAttempt)
        ));
    }
}
