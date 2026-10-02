//! Bulk roster provisioning and passkey cold-start.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::{BTreeMap, BTreeSet};

use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, KernelError, OrgId, TraceContext, UserId,
};
use console_platform_auth::{
    RefreshTokenIssue, RefreshTokenStore, authentication_time_tx, lock_account_tx,
};
use console_platform_db::{insert_audit_event, with_audit, with_audits};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction, types::Json};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const VALID_ROLES: &[&str] = &[
    "SUPER_ADMIN",
    "ADMIN",
    "MECHANIC",
    "RECEPTIONIST",
    "EXECUTIVE",
    "MEMBER",
];
const VALID_GROUP_ROLES: &[&str] = &["GROUP_ADMIN", "GROUP_VIEWER", "GROUP_FINANCE"];
const VALID_TEAMS: &[&str] = &["정비", "예방", "관리", "접수"];

#[derive(Debug, thiserror::Error)]
pub enum ProvisioningError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("database helper error: {0}")]
    Db(#[from] console_platform_db::DbError),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("auth error: {0}")]
    Auth(#[from] console_platform_auth::AuthError),

    #[error("kernel error: {0}")]
    Kernel(#[from] KernelError),

    #[error("invalid roster: {0}")]
    InvalidRoster(String),

    #[error("unknown branch {region}/{branch} for roster phone {phone}")]
    UnknownBranch {
        phone: String,
        region: String,
        branch: String,
    },

    /// Generic OTP-redeem failure. Unknown, expired, revoked, and
    /// already-consumed all collapse to this single variant so the REST layer can
    /// surface one "invalid or expired" message without revealing which it was.
    #[error("invalid bootstrap credential")]
    InvalidBootstrapCredential,

    #[error("user already has a registered passkey")]
    UserAlreadyHasPasskey,

    #[error("user already has an active bootstrap credential")]
    ActiveBootstrapCredentialExists,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapToken(String);

impl BootstrapToken {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapCredentialIssue {
    pub credential_id: Uuid,
    pub user_id: Uuid,
    pub token: BootstrapToken,
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RosterImportReport {
    pub users_created: u32,
    pub users_updated: u32,
    pub users_unchanged: u32,
    pub branch_memberships_added: u32,
    pub branch_memberships_removed: u32,
    pub bootstrap_credentials_issued: Vec<BootstrapCredentialIssue>,
}

impl RosterImportReport {
    #[must_use]
    pub fn changed_count(&self) -> u32 {
        self.users_created
            + self.users_updated
            + self.branch_memberships_added
            + self.branch_memberships_removed
            + u32::try_from(self.bootstrap_credentials_issued.len()).unwrap_or(u32::MAX)
    }
}

#[derive(Debug, Clone)]
pub struct RosterProvisioner {
    bootstrap_ttl: Duration,
}

impl RosterProvisioner {
    #[must_use]
    pub const fn new(bootstrap_ttl: Duration) -> Self {
        Self { bootstrap_ttl }
    }

    pub async fn import_json(
        &self,
        pool: &PgPool,
        roster_json: &str,
        now: OffsetDateTime,
    ) -> Result<RosterImportReport, ProvisioningError> {
        let roster: RosterImport = serde_json::from_str(roster_json)?;
        let users = normalize_roster(roster)?;
        let user_count = users.len();
        let bootstrap_ttl = self.bootstrap_ttl;

        let audit = AuditEvent::new(
            None,
            AuditAction::new("roster.import")?,
            "roster_import",
            Uuid::new_v4().to_string(),
            TraceContext::generate(),
            now,
        )
        // Roster import stamps every row with KNL today (single-tenant import).
        // Arm KNL as the GUC so the FORCE-RLS WITH CHECK on `users`,
        // `user_branches`, and `auth_bootstrap_credentials` accepts these writes
        // under the non-owner `console_rt` role.
        .with_org(OrgId::knl())
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "user_rows": user_count,
                "format": "json.v1"
            })),
        );

        with_audit::<_, RosterImportReport, ProvisioningError>(pool, audit, |tx| {
            Box::pin(async move { apply_roster_tx(tx, users, now, bootstrap_ttl).await })
        })
        .await
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BootstrapCredentialStore;

/// A separate pool authenticated as the restricted authentication command
/// role. The role is still NOLOGIN in the default topology; provisioning a
/// distinct credential and routing traffic are later cutover operations.
#[derive(Clone)]
pub struct FirstEnrollmentCommandPool {
    pool: PgPool,
}

#[derive(Debug)]
pub enum FirstEnrollmentOutcome {
    /// A local PostgreSQL commit only. The bearer stays internal and must not
    /// be acknowledged or released until a future two-site confirmation
    /// barrier and purpose-aware validator cutover both succeed.
    LocallyCommitted {
        receipt_id: Uuid,
        refresh: RefreshTokenIssue,
        enrollment_expires_at: OffsetDateTime,
    },
    /// Local committed status only. The bearer is deliberately unavailable on
    /// replay: only its hash exists; this is not a two-site acknowledgment.
    AlreadyCommitted { receipt_id: Uuid },
}

impl FirstEnrollmentCommandPool {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Consume exactly one v1 first-enrollment proof, issue only an enrollment
    /// family, finalize the operation receipt and append a fixed audit in one
    /// transaction. The client holds `operation_id` and a random status secret
    /// before calling; only the secret's SHA-256 hash reaches this method.
    /// There is intentionally no HTTP caller until purpose-aware validation,
    /// recovery and old-process fencing have passed their separate gates.
    pub async fn admit(
        &self,
        otp: &str,
        operation_id: Uuid,
        status_secret_hash: [u8; 32],
        trusted_now: OffsetDateTime,
    ) -> Result<FirstEnrollmentOutcome, ProvisioningError> {
        let mut tx = self.pool.begin().await?;
        let role: String = sqlx::query_scalar("SELECT current_user")
            .fetch_one(tx.as_mut())
            .await?;
        if role != "console_auth_cmd" {
            return Err(ProvisioningError::InvalidBootstrapCredential);
        }
        let token_hash = hash_token(otp);
        let org_uuid = resolve_bootstrap_org(&mut tx, &token_hash)
            .await?
            .ok_or(ProvisioningError::InvalidBootstrapCredential)?;
        sqlx::query("SELECT set_config('app.current_org',$1,true)")
            .bind(org_uuid.to_string())
            .execute(tx.as_mut())
            .await?;
        let source = sqlx::query(
            "SELECT id,user_id,issued_generation,source_operation_id,expires_at \
                 FROM auth_bootstrap_credentials \
                 WHERE org_id=$1 AND token_hash=$2 AND issuance_version=1",
        )
        .bind(org_uuid)
        .bind(&token_hash)
        .fetch_optional(tx.as_mut())
        .await?;
        let source = source.ok_or(ProvisioningError::InvalidBootstrapCredential)?;
        let source_id: Uuid = source.try_get("id")?;
        let user_id: Uuid = source.try_get("user_id")?;
        let source_generation: Option<i64> = source.try_get("issued_generation")?;
        let source_operation: Option<Uuid> = source.try_get("source_operation_id")?;
        let source_expires_at: OffsetDateTime = source.try_get("expires_at")?;
        let active: Option<bool> =
            sqlx::query_scalar("SELECT auth_security.lock_account_for_auth($1,$2)")
                .bind(org_uuid)
                .bind(user_id)
                .fetch_one(tx.as_mut())
                .await?;
        if active != Some(true) {
            return Err(ProvisioningError::InvalidBootstrapCredential);
        }

        // The source token identifies a Company, but only the locked Account
        // and same-Company private state authorize an admission. A committed
        // retry can return its receipt without minting another bearer.
        let existing = sqlx::query(
            "SELECT status_secret_hash,status,receipt_id,source_row_id,account_id, \
                 session_purpose,auth_generation,source_operation_id,family_id \
                 FROM auth_security.session_admissions \
                 WHERE operation_id=$1 AND home_org_id=$2 FOR UPDATE",
        )
        .bind(operation_id)
        .bind(org_uuid)
        .fetch_optional(tx.as_mut())
        .await?;
        if let Some(existing) = existing {
            let saved_hash: Vec<u8> = existing.try_get("status_secret_hash")?;
            let receipt_id: Option<Uuid> = existing.try_get("receipt_id")?;
            let saved_source: Option<Uuid> = existing.try_get("source_row_id")?;
            let saved_account: Uuid = existing.try_get("account_id")?;
            let generation: i64 = existing.try_get("auth_generation")?;
            let issuance: Option<Uuid> = existing.try_get("source_operation_id")?;
            let family: Option<Uuid> = existing.try_get("family_id")?;
            if saved_hash.as_slice() == status_secret_hash.as_slice()
                && existing.try_get::<&str, _>("status")? == "committed"
                && saved_source == Some(source_id)
                && saved_account == user_id
                && existing.try_get::<&str, _>("session_purpose")? == "enrollment"
                && Some(generation) == source_generation
                && issuance == source_operation
                && family.is_some()
                && let Some(receipt_id) = receipt_id
            {
                tx.commit().await?;
                return Ok(FirstEnrollmentOutcome::AlreadyCommitted { receipt_id });
            }
            return Err(ProvisioningError::InvalidBootstrapCredential);
        }

        let state: Option<(i64, Option<bool>, String)> = sqlx::query_as(
            "SELECT auth_generation,ever_enrolled,status \
             FROM auth_security.account_state \
             WHERE account_id=$1 AND home_org_id=$2 FOR SHARE",
        )
        .bind(user_id)
        .bind(org_uuid)
        .fetch_optional(tx.as_mut())
        .await?;
        let Some((generation, Some(false), status)) = state else {
            return Err(ProvisioningError::InvalidBootstrapCredential);
        };
        if status != "never_enrolled" || source_generation != Some(generation) {
            return Err(ProvisioningError::InvalidBootstrapCredential);
        }
        let source_operation =
            source_operation.ok_or(ProvisioningError::InvalidBootstrapCredential)?;
        let now = authentication_time_tx(&mut tx, trusted_now).await?;
        // This original deadline is stored once. Refresh or challenge renewal
        // cannot extend it when those paths are later switched to version 1.
        let deadline = source_expires_at.min(now + Duration::minutes(15));
        if deadline <= now + Duration::seconds(1) {
            return Err(ProvisioningError::InvalidBootstrapCredential);
        }
        sqlx::query(
            "INSERT INTO auth_security.session_admissions \
             (operation_id,status_secret_hash,account_id,home_org_id,auth_generation,session_purpose,expires_at) \
             VALUES($1,$2,$3,$4,$5,'enrollment',$6)",
        )
        .bind(operation_id)
        .bind(status_secret_hash.as_slice())
        .bind(user_id)
        .bind(org_uuid)
        .bind(generation)
        .bind(deadline)
        .execute(tx.as_mut())
        .await?;

        let proof = BootstrapCredentialStore
            .redeem_otp_in_tx(&mut tx, otp, now)
            .await?;
        if proof.source.issuance_version != 1
            || proof.source.credential_id != source_id
            || proof.source.issued_generation != Some(generation)
            || proof.source.source_operation_id != Some(source_operation)
            || proof.user_id != user_id
            || *proof.org_id.as_uuid() != org_uuid
        {
            return Err(ProvisioningError::InvalidBootstrapCredential);
        }
        let refresh = RefreshTokenStore
            .issue_first_enrollment_family_in_tx(
                &mut tx,
                user_id,
                proof.org_id,
                generation,
                operation_id,
                deadline,
            )
            .await?;
        let completed_at = authentication_time_tx(&mut tx, now).await?;
        let receipt_id = Uuid::new_v4();
        let committed = sqlx::query(
            "UPDATE auth_security.session_admissions \
             SET status='committed',source_kind='bootstrap_otp',source_row_id=$2, \
                 source_operation_id=$3,family_id=$4,receipt_id=$5,finalized_at=$6 \
             WHERE operation_id=$1 AND account_id=$7 AND home_org_id=$8 \
               AND auth_generation=$9 AND status='pending' AND expires_at>$6",
        )
        .bind(operation_id)
        .bind(source_id)
        .bind(source_operation)
        .bind(refresh.family_id)
        .bind(receipt_id)
        .bind(completed_at)
        .bind(user_id)
        .bind(org_uuid)
        .bind(generation)
        .execute(tx.as_mut())
        .await?;
        if committed.rows_affected() != 1 {
            return Err(ProvisioningError::InvalidBootstrapCredential);
        }
        sqlx::query("SELECT auth_security.append_first_enrollment_audit($1)")
            .bind(operation_id)
            .execute(tx.as_mut())
            .await?;
        tx.commit().await?;
        Ok(FirstEnrollmentOutcome::LocallyCommitted {
            receipt_id,
            refresh,
            enrollment_expires_at: deadline,
        })
    }
}

impl BootstrapCredentialStore {
    pub async fn issue_for_zero_credential_user(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        org: OrgId,
        now: OffsetDateTime,
        ttl: Duration,
    ) -> Result<BootstrapCredentialIssue, ProvisioningError> {
        with_audits::<_, BootstrapCredentialIssue, ProvisioningError>(pool, org, |tx| {
            Box::pin(async move {
                // Admin-issued OTP path: the request's tenant `org` is threaded in
                // from the authenticated admin's verified token. `with_audits`
                // arms `app.current_org` to that org, so the FORCE-RLS WITH CHECK on
                // `auth_bootstrap_credentials` accepts the row for ANY tenant — not
                // just KNL — and the credential is stamped with the real org.
                let issue = issue_bootstrap_if_needed_tx(
                    tx,
                    user_id,
                    org,
                    now,
                    ttl,
                    IssueMode::RejectIfPresent,
                )
                .await?
                .ok_or(ProvisioningError::ActiveBootstrapCredentialExists)?;
                let event = AuditEvent::new(
                    None,
                    AuditAction::new("auth.bootstrap.issue")?,
                    "auth_bootstrap_credential",
                    user_id.to_string(),
                    TraceContext::generate(),
                    issue.expires_at - ttl,
                )
                .with_org(org)
                .with_snapshots(
                    None,
                    Some(serde_json::json!({
                        "user_id": user_id,
                        "expires_at": issue.expires_at,
                    })),
                );
                Ok((issue, vec![event]))
            })
        })
        .await
    }

    /// Open self-service signup (#38): create a brand-new low-privilege user in
    /// the default (KNL) org and mint its first-sign-in OTP, ATOMICALLY and
    /// AUDITED. Unlike [`Self::issue_for_zero_credential_user`] (which needs a
    /// pre-provisioned user), this is the SIGNUP half: it makes the user row too.
    ///
    /// The new user gets the single lowest-privilege `MEMBER` role and no branch
    /// memberships, so it can authenticate but sees almost nothing until an admin
    /// elevates it (the authz matrix denies `MEMBER` everything but `Login`).
    /// `display_name` is the caller-supplied label (the email's local part); the
    /// email itself is only used out-of-band to deliver the returned OTP.
    ///
    /// In ONE transaction with `app.current_org` armed to KNL by [`with_audits`],
    /// so the `users` + `auth_bootstrap_credentials` inserts pass the FORCE-RLS
    /// WITH CHECK as the non-owner `console_rt` role. Returns the one-time OTP exactly
    /// like the admin path; the OTP value is NEVER audited or logged.
    pub async fn signup_open_member(
        &self,
        pool: &PgPool,
        display_name: &str,
        now: OffsetDateTime,
        ttl: Duration,
    ) -> Result<BootstrapCredentialIssue, ProvisioningError> {
        let display_name = display_name.trim().to_owned();
        if display_name.is_empty() {
            return Err(ProvisioningError::InvalidRoster(
                "display name is required to sign up".to_owned(),
            ));
        }
        let org = OrgId::knl();

        with_audits::<_, BootstrapCredentialIssue, ProvisioningError>(pool, org, |tx| {
            Box::pin(async move {
                // (1) Create the MEMBER user in KNL. The GUC armed by `with_audits`
                // scopes the insert to KNL so it passes the FORCE-RLS WITH CHECK.
                let user_id: Uuid = sqlx::query_scalar(
                    r#"
                    INSERT INTO users (display_name, roles, is_active, org_id)
                    VALUES ($1, $2, true, $3)
                    RETURNING id
                    "#,
                )
                .bind(&display_name)
                .bind(&["MEMBER"] as &[&str])
                .bind(*org.as_uuid())
                .fetch_one(tx.as_mut())
                .await?;

                // (2) Mint the first-sign-in OTP. The user has zero passkeys and no
                // open code, so RejectIfPresent always issues a fresh one.
                let issue = issue_bootstrap_if_needed_tx(
                    tx,
                    user_id,
                    org,
                    now,
                    ttl,
                    IssueMode::RejectIfPresent,
                )
                .await?
                .ok_or(ProvisioningError::ActiveBootstrapCredentialExists)?;

                let event = AuditEvent::new(
                    Some(UserId::from_uuid(user_id)),
                    AuditAction::new("auth.signup")?,
                    "users",
                    user_id.to_string(),
                    TraceContext::generate(),
                    now,
                )
                .with_org(org)
                .with_snapshots(
                    None,
                    Some(serde_json::json!({
                        "user_id": user_id,
                        "role": "MEMBER",
                        "expires_at": issue.expires_at,
                    })),
                );

                Ok((issue, vec![event]))
            })
        })
        .await
    }

    /// Admin account recovery: revoke ALL target sessions and passkeys, then
    /// mint a fresh single-use bootstrap OTP, atomically and audited.
    ///
    /// The normal admin-OTP path ([`Self::issue_for_zero_credential_user`]) refuses
    /// a user who already has a passkey (returns [`ProvisioningError::UserAlreadyHasPasskey`]),
    /// and self-revoke refuses the last passkey, so neither can recover a locked-out
    /// user. This method is the deliberate, admin-only override.
    ///
    /// Behaviour in ONE transaction (tenant `org` armed as `app.current_org` by
    /// `with_audits`, so every FORCE-RLS read/write/delete on the target's
    /// `auth_webauthn_credentials` and `auth_bootstrap_credentials` is scoped to the
    /// caller's tenant — a user in another org is invisible and cannot be reset):
    ///   1. Revoke all target refresh families/tokens, preserving previous revocations.
    ///   2. DELETE every `auth_webauthn_credentials` row for `user_id`, each audited
    ///      as `auth.passkey.admin_reset`. The old passkeys then fail login.
    ///   3. Mint a fresh bootstrap OTP via [`issue_bootstrap_if_needed_tx`] in
    ///      [`IssueMode::ForceReset`] (bypasses the now-stale passkey check and
    ///      revokes any leftover open code), audited as `auth.otp.issue`.
    ///
    /// Returns the one-time OTP exactly like [`Self::issue_for_zero_credential_user`]
    /// so the admin can hand it to the user. The OTP value is NEVER audited or logged.
    pub async fn reset_credentials_for_user(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        org: OrgId,
        now: OffsetDateTime,
        ttl: Duration,
    ) -> Result<BootstrapCredentialIssue, ProvisioningError> {
        with_audits::<_, BootstrapCredentialIssue, ProvisioningError>(pool, org, |tx| {
            Box::pin(async move {
                require_active_account_tx(tx, org, user_id).await?;
                let now = authentication_time_tx(tx, now).await?;
                // Recovery invalidates every existing session. Keep this out of
                // generic ForceReset: self-enrollment handoffs retain sessions.
                let revoked_families = sqlx::query(
                    "UPDATE auth_refresh_token_families \
                     SET revoked_at=$1, revoked_reason='admin_reset' \
                     WHERE user_id=$2 AND org_id=$3 AND revoked_at IS NULL",
                )
                .bind(now)
                .bind(user_id)
                .bind(*org.as_uuid())
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                // Include residual tokens under already-revoked families.
                let revoked_tokens = sqlx::query(
                    "UPDATE auth_refresh_tokens SET revoked_at=$1 \
                     WHERE user_id=$2 AND org_id=$3 AND revoked_at IS NULL",
                )
                .bind(now)
                .bind(user_id)
                .bind(*org.as_uuid())
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                // (1) Revoke ALL of the target's passkeys. RETURNING the row ids so
                // each deletion is audited individually. The GUC armed by
                // `with_audits` scopes this DELETE to the caller's tenant, so a user
                // in another org matches zero rows (cross-org reset is a no-op +
                // generic "user not found" surfaced by the REST layer's prior read).
                let deleted = sqlx::query(
                    r#"
                    DELETE FROM auth_webauthn_credentials
                    WHERE user_id = $1
                    RETURNING id, credential_id
                    "#,
                )
                .bind(user_id)
                .fetch_all(tx.as_mut())
                .await?;

                let mut events = Vec::with_capacity(deleted.len() + 2);
                events.push(
                    AuditEvent::new(
                        // This legacy owner does not receive the initiating admin.
                        None,
                        AuditAction::new("auth.refresh.revoke_all")?,
                        "user",
                        user_id.to_string(),
                        TraceContext::generate(),
                        now,
                    )
                    .with_org(org)
                    .with_snapshots(
                        None,
                        Some(serde_json::json!({
                            "reason": "admin_reset",
                            "revoked_family_count": revoked_families,
                            "revoked_token_count": revoked_tokens,
                        })),
                    ),
                );
                for row in deleted {
                    let credential_uuid: Uuid = row.try_get("id")?;
                    let credential_id: String = row.try_get("credential_id")?;
                    events.push(
                        AuditEvent::new(
                            Some(UserId::from_uuid(user_id)),
                            AuditAction::new("auth.passkey.admin_reset")?,
                            "auth_webauthn_credential",
                            credential_uuid.to_string(),
                            TraceContext::generate(),
                            now,
                        )
                        .with_org(org)
                        .with_snapshots(
                            Some(serde_json::json!({
                                "credential_id": credential_id,
                                "user_id": user_id,
                            })),
                            None,
                        ),
                    );
                }

                // (2) Mint a fresh single-use OTP. ForceReset bypasses the
                // passkey-existence rejection (the rows were just deleted in this same
                // transaction) and revokes any stale open code so the new OTP is the
                // user's sole valid recovery credential.
                let issue =
                    issue_bootstrap_if_needed_tx(tx, user_id, org, now, ttl, IssueMode::ForceReset)
                        .await?
                        .ok_or(ProvisioningError::ActiveBootstrapCredentialExists)?;

                events.push(
                    AuditEvent::new(
                        Some(UserId::from_uuid(user_id)),
                        AuditAction::new("auth.otp.issue")?,
                        "auth_bootstrap_credential",
                        issue.credential_id.to_string(),
                        TraceContext::generate(),
                        now,
                    )
                    .with_org(org)
                    .with_snapshots(
                        None,
                        Some(serde_json::json!({
                            "user_id": user_id,
                            "expires_at": issue.expires_at,
                            "reason": "admin_reset",
                        })),
                    ),
                );

                Ok((issue, events))
            })
        })
        .await
    }

    /// Cross-device SELF passkey-enrollment handoff: mint a fresh single-use,
    /// short-TTL one-time code for the CURRENTLY AUTHENTICATED user so they can
    /// finish passkey enrollment on a SECOND device (typically a phone scanning a
    /// QR on the desktop console). The returned code is scoped to THAT SAME user
    /// and is redeemed through the ordinary `/auth/otp/redeem` first-sign-in path
    /// on the phone, which lands the phone on its own onboarding page to enroll a
    /// platform passkey — no Bluetooth / caBLE hybrid tunnel involved.
    ///
    /// This NEVER mints a handoff for another user: `user_id` and `org` are taken
    /// from the caller's VERIFIED access token at the REST layer, never from the
    /// request body, so a caller can only ever hand off to itself.
    ///
    /// Issuance semantics ([`IssueMode::ForceReset`], but WITHOUT touching
    /// passkeys — the REST layer enforces the step-up gate for an already-enrolled
    /// user before calling this):
    ///   * A user MID-ONBOARDING (just redeemed, zero passkeys) typically already
    ///     holds one OPEN bootstrap code (the one they redeemed; it is consumed
    ///     only at passkey registration). The partial-unique
    ///     `idx_auth_bootstrap_credentials_one_open_per_user` permits only ONE open
    ///     code per user, so this REVOKES that stale open code (reason `reset`) and
    ///     mints a fresh one in its place. The original code therefore stops
    ///     working the moment a handoff is issued — at most one live code exists.
    ///   * A user ADDING A DEVICE (already has a passkey) gets a fresh handoff code
    ///     too; `ForceReset` bypasses the passkey-existence rejection that the
    ///     admin path uses, but does NOT delete any passkey.
    ///
    /// The TTL is SHORT (the caller passes e.g. 5 min — distinct from the 4h admin
    /// OTP) to keep the credential-handoff window tight. The code value is NEVER
    /// audited or logged; only the issuance event (action
    /// `auth.passkey.enroll_handoff_issued`, target = the user) is recorded, armed
    /// to the caller's tenant so the FORCE-RLS WITH CHECK accepts the new row.
    pub async fn issue_self_enroll_handoff(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        org: OrgId,
        now: OffsetDateTime,
        ttl: Duration,
    ) -> Result<BootstrapCredentialIssue, ProvisioningError> {
        with_audits::<_, BootstrapCredentialIssue, ProvisioningError>(pool, org, |tx| {
            Box::pin(async move {
                // ForceReset: revoke any stale OPEN code for this user and mint a
                // fresh one, WITHOUT requiring (or deleting) passkeys. Works both
                // mid-onboarding (an open redeemed code is superseded) and for an
                // already-enrolled add-device user (no open code -> clean insert).
                let issue =
                    issue_bootstrap_if_needed_tx(tx, user_id, org, now, ttl, IssueMode::ForceReset)
                        .await?
                        .ok_or(ProvisioningError::ActiveBootstrapCredentialExists)?;

                let event = AuditEvent::new(
                    Some(UserId::from_uuid(user_id)),
                    AuditAction::new("auth.passkey.enroll_handoff_issued")?,
                    "auth_bootstrap_credential",
                    issue.credential_id.to_string(),
                    TraceContext::generate(),
                    now,
                )
                .with_org(org)
                .with_snapshots(
                    None,
                    Some(serde_json::json!({
                        "user_id": user_id,
                        "expires_at": issue.expires_at,
                        "purpose": "passkey_enrollment_handoff",
                    })),
                );

                Ok((issue, vec![event]))
            })
        })
        .await
    }

    /// Redeem a one-time OTP (bootstrap token) as a FIRST SIGN-IN.
    ///
    /// This is a sign-in, not signup: the user row was pre-provisioned by the
    /// admin who issued the OTP (or seeded for the cold-start admin). The
    /// redeeming user's id is returned so the caller can mint a session. Passkey
    /// enrollment is NOT bundled here — the user adds a passkey afterwards from
    /// authenticated initial settings.
    ///
    /// Security properties:
    /// * Legacy version-0 redemption is VERIFY-ONLY: it does NOT consume the
    ///   code. Single-use is enforced at passkey REGISTRATION by
    ///   [`Self::consume_open_credentials_tx`], which burns the code atomically
    ///   with the passkey insert (the harden-1 pattern). An incomplete or
    ///   cancelled enrollment therefore never burns the code — the user can
    ///   re-redeem until a passkey actually sticks; once a passkey exists the code
    ///   is dead and can never mint another session.
    /// * A WRONG guess never consumes or invalidates a credential — an unknown
    ///   token simply finds no row. There is deliberately no per-OTP attempt cap
    ///   (that would let an attacker burn a victim's OTP); brute-force is bounded
    ///   by the caller's per-client rate limit plus the short TTL.
    /// * Expiry and revocation are still enforced here, so an expired or revoked
    ///   OTP cannot mint a session.
    ///
    /// All failure modes collapse to [`ProvisioningError::InvalidBootstrapCredential`]
    /// so the caller can return a single generic "invalid or expired" message
    /// without revealing whether the token was unknown, expired, or already used.
    pub async fn redeem_otp(
        &self,
        pool: &PgPool,
        token: &str,
        now: OffsetDateTime,
    ) -> Result<OtpRedemption, ProvisioningError> {
        let mut tx = pool.begin().await?;
        let redemption = self.redeem_otp_in_tx(&mut tx, token, now).await?;
        // The compatibility wrapper has no family/admission owner. A version-1
        // source may be consumed only by its caller's complete transaction.
        if redemption.source.issuance_version != 0 {
            tx.rollback().await?;
            return Err(ProvisioningError::InvalidBootstrapCredential);
        }
        tx.commit().await?;
        Ok(redemption)
    }

    /// Verify and audit an OTP while its exact source row remains locked in the
    /// caller's transaction. A session owner can add its family, token and audit
    /// before committing; a failed later step rolls this verification back too.
    /// Version 0 retains verify-only semantics. Version 1 requires the private
    /// never-enrolled generation and consumes its source once in this same
    /// transaction; the caller must also own family/admission completion.
    pub async fn redeem_otp_in_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        token: &str,
        now: OffsetDateTime,
    ) -> Result<OtpRedemption, ProvisioningError> {
        let token_hash = hash_token(token);

        // Resolve the credential's tenant from the token hash FIRST, then arm the
        // GUC, THEN do the RLS-gated read. `auth_bootstrap_credentials` is FORCE
        // RLS (migration 0035), so as the non-owner `console_rt` role a lookup-by-hash
        // returns ZERO rows until `app.current_org` is set — but the org is what we
        // need to set it. The narrow SECURITY DEFINER resolver
        // `platform_resolve_bootstrap_org` (migration 0038) returns only the
        // credential's org_id, breaking that chicken-and-egg so OTP first sign-in
        // works for ANY tenant. A NULL means the token is unknown: keep the same
        // generic invalid-OTP error (no row), revealing nothing.
        let Some(org_uuid) = resolve_bootstrap_org(tx, &token_hash).await? else {
            return Err(ProvisioningError::InvalidBootstrapCredential);
        };
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_uuid.to_string())
            .execute(tx.as_mut())
            .await?;

        let locator: Option<(Uuid, Uuid, i16)> = sqlx::query_as(
            "SELECT id, user_id, issuance_version FROM auth_bootstrap_credentials WHERE token_hash=$1 AND org_id=$2",
        )
        .bind(&token_hash)
        .bind(org_uuid)
        .fetch_optional(tx.as_mut())
        .await?;
        let Some((credential_id, user_id, issuance_version)) = locator else {
            return Err(ProvisioningError::InvalidBootstrapCredential);
        };
        if issuance_version == 1 {
            // The command role cannot lock `users` directly; this narrow
            // definer takes the same NO KEY UPDATE Account mutex and checks
            // the armed Company without exposing profile columns.
            let active: Option<bool> =
                sqlx::query_scalar("SELECT auth_security.lock_account_for_auth($1,$2)")
                    .bind(org_uuid)
                    .bind(user_id)
                    .fetch_one(tx.as_mut())
                    .await?;
            if active != Some(true) {
                return Err(ProvisioningError::InvalidBootstrapCredential);
            }
        } else {
            require_active_account_tx(tx, OrgId::from_uuid(org_uuid), user_id).await?;
        }
        // Account/security state precedes source-row custody in the lock order.
        // Version 0 remains readable under the existing runtime role; only the
        // dormant version-1 command path needs the private state grant.
        let first_enrollment_state: Option<(i64, Option<bool>, String)> = if issuance_version == 1 {
            sqlx::query_as(
                "SELECT auth_generation, ever_enrolled, status \
                     FROM auth_security.account_state \
                     WHERE account_id=$1 AND home_org_id=$2 FOR SHARE",
            )
            .bind(user_id)
            .bind(org_uuid)
            .fetch_optional(tx.as_mut())
            .await?
        } else {
            None
        };
        let source = sqlx::query(
            "SELECT expires_at, issuance_version, issued_generation, issuance_purpose, source_operation_id FROM auth_bootstrap_credentials WHERE id=$1 AND user_id=$2 AND org_id=$3 AND token_hash=$4 AND consumed_at IS NULL AND revoked_at IS NULL FOR UPDATE")
            .bind(credential_id).bind(user_id).bind(org_uuid).bind(&token_hash)
            .fetch_optional(tx.as_mut()).await?;
        let source = source.ok_or(ProvisioningError::InvalidBootstrapCredential)?;
        let now = authentication_time_tx(tx, now).await?;
        let expires: OffsetDateTime = source.try_get("expires_at")?;
        if expires <= now {
            return Err(ProvisioningError::InvalidBootstrapCredential);
        }

        let requires_passkey_setup = count_user_passkeys_tx(tx, user_id).await? == 0;

        if issuance_version == 1 {
            let (generation, ever_enrolled, status) =
                first_enrollment_state.ok_or(ProvisioningError::InvalidBootstrapCredential)?;
            if source.try_get::<i16, _>("issuance_version")? != 1
                || source.try_get::<Option<i64>, _>("issued_generation")? != Some(generation)
                || source.try_get::<Option<&str>, _>("issuance_purpose")?
                    != Some("first_enrollment")
                || source
                    .try_get::<Option<Uuid>, _>("source_operation_id")?
                    .is_none()
                || status != "never_enrolled"
                || ever_enrolled != Some(false)
                || !requires_passkey_setup
            {
                return Err(ProvisioningError::InvalidBootstrapCredential);
            }
            // One-use proof is consumed in the caller's transaction, before it
            // mints a restricted family and finalizes an admission. The public
            // wrapper above refuses to commit this consumption on its own.
            let consumed = sqlx::query(
                "UPDATE auth_bootstrap_credentials SET consumed_at=$1 \
                 WHERE id=$2 AND user_id=$3 AND org_id=$4 AND token_hash=$5 \
                   AND issuance_version=1 AND issued_generation=$6 \
                   AND issuance_purpose='first_enrollment' \
                   AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at>$1",
            )
            .bind(now)
            .bind(credential_id)
            .bind(user_id)
            .bind(org_uuid)
            .bind(&token_hash)
            .bind(generation)
            .execute(tx.as_mut())
            .await?;
            if consumed.rows_affected() != 1 {
                return Err(ProvisioningError::InvalidBootstrapCredential);
            }
        }

        if issuance_version == 0 {
            let audit = AuditEvent::new(
                Some(UserId::from_uuid(user_id)),
                AuditAction::new("auth.otp.redeem")?,
                "auth_bootstrap_credential",
                credential_id.to_string(),
                TraceContext::generate(),
                now,
            )
            .with_org(OrgId::from_uuid(org_uuid))
            .with_snapshots(
                None,
                Some(serde_json::json!({
                    "user_id": user_id,
                    "requires_passkey_setup": requires_passkey_setup,
                })),
            );
            insert_audit_event(tx, &audit).await?;
        }
        // The version-1 command appends its one source/family/admission-bound
        // audit only after all three agree; it has no general audit INSERT.

        Ok(OtpRedemption {
            user_id,
            org_id: OrgId::from_uuid(org_uuid),
            requires_passkey_setup,
            source: OtpSource {
                credential_id,
                issuance_version: source.try_get("issuance_version")?,
                issued_generation: source.try_get("issued_generation")?,
                issuance_purpose: source.try_get("issuance_purpose")?,
                source_operation_id: source.try_get("source_operation_id")?,
            },
        })
    }

    /// Consume the user's open one-time code(s) inside the caller's transaction.
    ///
    /// Called by the legacy version-0 passkey registration path so its open code
    /// is consumed atomically with the passkey insert. Version-1 redemption
    /// consumes its exact source earlier, in the family/admission transaction.
    /// A returning user adding a second passkey has no open code (0 rows).
    pub async fn consume_open_credentials_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        org: OrgId,
        user_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<(), ProvisioningError> {
        require_active_account_tx(tx, org, user_id).await?;
        lock_open_bootstrap_tx(tx, user_id).await?;
        let now = authentication_time_tx(tx, now).await?;
        let consumed = sqlx::query(
            r#"
            UPDATE auth_bootstrap_credentials
            SET consumed_at = $2
            WHERE user_id = $1
              AND consumed_at IS NULL
              AND revoked_at IS NULL
              AND expires_at > $2
            RETURNING id
            "#,
        )
        .bind(user_id)
        .bind(now)
        .fetch_all(tx.as_mut())
        .await?;

        for row in consumed {
            let credential_id: Uuid = row.try_get("id")?;
            let audit = AuditEvent::new(
                Some(UserId::from_uuid(user_id)),
                AuditAction::new("auth.otp.consume")?,
                "auth_bootstrap_credential",
                credential_id.to_string(),
                TraceContext::generate(),
                now,
            )
            // Stamp the caller's org so this consume event is visible to a
            // tenant-scoped `/api/audit` read (RLS `USING (org_id = GUC)`). The
            // caller's tx already armed `app.current_org` to this org, so the
            // insert passes; a NULL org_id would pass WITH CHECK too but stay
            // invisible to the tenant.
            .with_org(org)
            .with_snapshots(None, Some(serde_json::json!({ "user_id": user_id })));
            insert_audit_event(tx, &audit).await?;
        }
        Ok(())
    }

    /// Complete a legacy registration with the exact OTP accepted at start.
    /// A later code cannot rescue a revoked, expired or consumed source.
    pub async fn consume_bound_credential_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        org: OrgId,
        user_id: Uuid,
        credential_id: Uuid,
        trusted_now: OffsetDateTime,
    ) -> Result<(), ProvisioningError> {
        require_active_account_tx(tx, org, user_id).await?;
        let source: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM auth_bootstrap_credentials \
             WHERE id=$1 AND user_id=$2 AND org_id=$3 AND issuance_version=0 FOR UPDATE",
        )
        .bind(credential_id)
        .bind(user_id)
        .bind(*org.as_uuid())
        .fetch_optional(tx.as_mut())
        .await?;
        if source != Some(credential_id) {
            return Err(ProvisioningError::InvalidBootstrapCredential);
        }
        let now = authentication_time_tx(tx, trusted_now).await?;
        let consumed = sqlx::query(
            "UPDATE auth_bootstrap_credentials SET consumed_at=$1 \
             WHERE id=$2 AND user_id=$3 AND org_id=$4 AND issuance_version=0 \
               AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at>$1",
        )
        .bind(now)
        .bind(credential_id)
        .bind(user_id)
        .bind(*org.as_uuid())
        .execute(tx.as_mut())
        .await?;
        if consumed.rows_affected() != 1 {
            return Err(ProvisioningError::InvalidBootstrapCredential);
        }
        let audit = AuditEvent::new(
            Some(UserId::from_uuid(user_id)),
            AuditAction::new("auth.otp.consume")?,
            "auth_bootstrap_credential",
            credential_id.to_string(),
            TraceContext::generate(),
            now,
        )
        .with_org(org)
        .with_snapshots(None, Some(serde_json::json!({ "user_id": user_id })));
        insert_audit_event(tx, &audit).await?;
        Ok(())
    }

    /// Seed a deploy-time cold-start OTP for the PLATFORM cold-start SUPER_ADMIN
    /// at app boot.
    ///
    /// The global `CONSOLE_COLDSTART_OTP` bootstraps the PLATFORM admin — the first
    /// account ABOVE all tenants — NOT a tenant admin. The "Cold Start Admin"
    /// user is re-homed to the platform sentinel org [`OrgId::platform`] by
    /// migration 0036, so this arms that sentinel as the GUC; the seeded
    /// bootstrap credential carries the sentinel org_id. KNL's own tenant admin
    /// is created via the per-org onboarding flow (POST /api/platform/orgs), not here.
    ///
    /// ONLY IF that admin has neither a registered passkey nor an already-open
    /// bootstrap credential, this inserts a single bootstrap credential whose
    /// `token_hash = hash_token(otp)` and that expires one TTL after issuance. The insert
    /// is audited via [`with_audits`] (action `auth.coldstart.seed`, target = the
    /// admin user id); the OTP value is NEVER written to the audit snapshot or any
    /// log.
    ///
    /// Returns `Ok(true)` when a credential was seeded and `Ok(false)` when it was
    /// skipped (no cold-start admin, or the admin already has a passkey or an open
    /// credential). Idempotent and race-safe: the existence checks and the insert
    /// run inside one transaction with `FOR UPDATE` on the admin row, exactly like
    /// [`issue_bootstrap_if_needed_tx`], so two concurrent boots cannot double-seed.
    pub async fn seed_cold_start_credential(
        &self,
        pool: &PgPool,
        otp: &str,
        ttl: Duration,
        now: OffsetDateTime,
    ) -> Result<bool, ProvisioningError> {
        let token_hash = hash_token(otp);

        with_audits::<_, bool, ProvisioningError>(pool, OrgId::platform(), |tx| {
            Box::pin(async move {
                match seed_cold_start_if_needed_tx(tx, &token_hash, now, ttl).await? {
                    Some((admin_id, credential_id, issued_at)) => {
                        // The OTP value is NEVER placed in the audit trail; only the
                        // admin id and the new credential id are recorded.
                        let event = AuditEvent::new(
                            Some(UserId::from_uuid(admin_id)),
                            AuditAction::new("auth.coldstart.seed")?,
                            "auth_bootstrap_credential",
                            credential_id.to_string(),
                            TraceContext::generate(),
                            issued_at,
                        )
                        .with_snapshots(None, Some(serde_json::json!({ "user_id": admin_id })));
                        Ok((true, vec![event]))
                    }
                    None => Ok((false, Vec::new())),
                }
            })
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// dev-auth: local role-switch principal provisioning.
//
// Reached ONLY from the `dev-auth` cargo feature in `console-platform-auth-rest`
// (never compiled into a default/release build — see that crate's
// `#[cfg(feature = "dev-auth")]` route). A dev persona needs a REAL `users` +
// `user_branches` row: `resolve_branch_scope_in_org` re-resolves branch
// membership from the DB, by user id, on every request, so a token-only
// identity with no backing row would see zero branch-scoped data for any
// non-admin role. This mirrors `apply_roster_tx`'s find-or-update-else-insert
// shape (same tables), scoped to one row per (org, role) instead of a whole
// roster — but atomically, via `INSERT ... ON CONFLICT`, since a `SELECT ...
// FOR UPDATE` finding zero rows locks nothing and cannot serialize two
// concurrent FIRST mints of the same persona.
// ---------------------------------------------------------------------------

/// One dev-auth role-switch request: the org/role/branches an engineer wants to
/// exercise locally. `role` is a canonical DB role string, already validated by
/// the caller (the REST boundary parses it through the authz `Role` enum before
/// this is built, exactly like every other role-accepting endpoint).
#[derive(Debug, Clone)]
pub struct DevPrincipalRequest {
    pub org_id: OrgId,
    pub display_name: String,
    pub role: String,
    pub branch_ids: Vec<BranchId>,
}

/// The backing user row for a dev-auth session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DevPrincipal {
    pub user_id: Uuid,
}

/// Idempotently create-or-update the ONE dev principal for a given (org, role):
/// calling this again with a different display name or branch set moves that
/// same persona rather than accumulating throwaway rows. Identified by a
/// synthetic `phone` value (`dev-auth:<org>:<role>`) — `phone` carries no format
/// CHECK, and this prefix can never collide with a real telephone number.
#[derive(Debug, Clone, Copy, Default)]
pub struct DevPrincipalProvisioner;

impl DevPrincipalProvisioner {
    pub async fn upsert(
        &self,
        pool: &PgPool,
        request: DevPrincipalRequest,
        now: OffsetDateTime,
    ) -> Result<DevPrincipal, ProvisioningError> {
        let org_uuid = *request.org_id.as_uuid();
        let dev_key = format!("dev-auth:{org_uuid}:{}", request.role);
        let branch_ids: Vec<Uuid> = request
            .branch_ids
            .iter()
            .map(|b| *b.as_uuid())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let roles = vec![request.role.clone()];

        let event = AuditEvent::new(
            None,
            AuditAction::new("dev_auth.principal.upsert")?,
            "users",
            dev_key.clone(),
            TraceContext::generate(),
            now,
        )
        .with_org(request.org_id)
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "role": request.role,
                "branch_ids": branch_ids,
            })),
        );

        with_audit::<_, DevPrincipal, ProvisioningError>(pool, event, move |tx| {
            Box::pin(async move {
                // Fail closed on an unknown/inactive org with a clean 404 rather
                // than letting the `users_org_fk` FK violation surface as a raw
                // 500 from the INSERT below.
                let org_status: Option<String> =
                    sqlx::query_scalar("SELECT status FROM organizations WHERE id = $1")
                        .bind(org_uuid)
                        .fetch_optional(tx.as_mut())
                        .await?;
                if org_status.as_deref() != Some("ACTIVE") {
                    return Err(ProvisioningError::NotFound(
                        "no such active organization".to_owned(),
                    ));
                }

                if !branch_ids.is_empty() {
                    let found: Vec<Uuid> = sqlx::query_scalar(
                        "SELECT id FROM branches WHERE id = ANY($1) AND org_id = $2",
                    )
                    .bind(&branch_ids)
                    .bind(org_uuid)
                    .fetch_all(tx.as_mut())
                    .await?;
                    if found.len() != branch_ids.len() {
                        return Err(ProvisioningError::NotFound(
                            "one or more branch_ids do not belong to this org".to_owned(),
                        ));
                    }
                }

                // `ON CONFLICT (phone) WHERE phone IS NOT NULL` targets
                // `idx_users_phone_unique_present` (0006_create_provisioning.sql)
                // atomically: a `SELECT ... FOR UPDATE` on zero rows locks
                // nothing, so two concurrent FIRST mints for the same (org,
                // role) would both insert and 500 on the unique-index conflict.
                let user_id: Uuid = sqlx::query_scalar(
                    r#"
                    INSERT INTO users (display_name, phone, roles, is_active, org_id)
                    VALUES ($1, $2, $3, true, $4)
                    ON CONFLICT (phone) WHERE phone IS NOT NULL
                    DO UPDATE SET display_name = EXCLUDED.display_name,
                                  roles = EXCLUDED.roles,
                                  is_active = true
                    RETURNING id
                    "#,
                )
                .bind(&request.display_name)
                .bind(&dev_key)
                .bind(&roles)
                .bind(org_uuid)
                .fetch_one(tx.as_mut())
                .await?;

                let existing_branch_rows: Vec<Uuid> =
                    sqlx::query_scalar("SELECT branch_id FROM user_branches WHERE user_id = $1")
                        .bind(user_id)
                        .fetch_all(tx.as_mut())
                        .await?;
                let existing_branches: BTreeSet<Uuid> = existing_branch_rows.into_iter().collect();
                let desired_branches: BTreeSet<Uuid> = branch_ids.iter().copied().collect();

                for branch_id in desired_branches.difference(&existing_branches) {
                    sqlx::query(
                        "INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)",
                    )
                    .bind(user_id)
                    .bind(branch_id)
                    .bind(org_uuid)
                    .execute(tx.as_mut())
                    .await?;
                }
                for branch_id in existing_branches.difference(&desired_branches) {
                    sqlx::query("DELETE FROM user_branches WHERE user_id = $1 AND branch_id = $2")
                        .bind(user_id)
                        .bind(branch_id)
                        .execute(tx.as_mut())
                        .await?;
                }

                Ok(DevPrincipal { user_id })
            })
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// Platform tenant onboarding.
// ---------------------------------------------------------------------------

/// Summary of a tenant row for the platform tenant-list / status APIs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OrganizationSummary {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub status: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub group_id: Option<Uuid>,
    pub group_slug: Option<String>,
    pub group_name: Option<String>,
}

/// Organization member identity inside a platform group response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupMemberSummary {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub status: String,
}

/// Platform group identity + its subsidiary organization memberships.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupSummary {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub status: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub member_count: i64,
    pub members: Vec<GroupMemberSummary>,
}

/// Tenant-anchored account that holds one or more group-level grants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupAccountSummary {
    pub user_id: Uuid,
    pub display_name: String,
    pub phone: Option<String>,
    pub tenant_roles: Vec<String>,
    pub is_active: bool,
    pub has_passkey: bool,
    pub account_status: String,
    pub org_id: Uuid,
    pub org_slug: String,
    pub org_name: String,
    pub group_roles: Vec<String>,
    pub created_at: OffsetDateTime,
}

/// Result of creating a group account: the account plus the one-time setup OTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupAccountOnboarding {
    pub account: GroupAccountSummary,
    pub otp: BootstrapToken,
    pub otp_expires_at: OffsetDateTime,
}

/// Per-tenant health/usage rollup for the platform ops dashboard.
///
/// Produced by the SECURITY DEFINER `platform_org_health()` function (the only
/// sanctioned cross-tenant aggregation path) and surfaced through an audited
/// platform read. Carries only counts + a last-activity timestamp; never any
/// row-level tenant data.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TenantHealth {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub status: String,
    pub group_id: Option<Uuid>,
    pub group_slug: Option<String>,
    pub group_name: Option<String>,
    pub user_count: i64,
    pub active_user_count: i64,
    pub active_work_orders: i64,
    pub open_work_orders: i64,
    pub last_activity_at: Option<OffsetDateTime>,
    pub route_adoption: Vec<RouteAdoptionMetric>,
    pub zero_legacy_release_cycles: i64,
}

/// Per-release route-adoption/RUM rollup for one tenant.
///
/// A release cycle with `legacy_route_events = 0` and `console_route_events > 0`
/// counts toward the D5 endgame evidence. Two consecutive release cycles with no
/// legacy route traffic are enough to consider retiring the legacy lane.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RouteAdoptionMetric {
    pub release_cycle: String,
    pub console_route_events: i64,
    pub legacy_route_events: i64,
    pub rum_error_events: i64,
    pub rum_perf_p95_ms: Option<i64>,
    pub last_event_at: OffsetDateTime,
}

/// Outcome of onboarding a new tenant: the created org plus the ONE-TIME OTP for
/// its first SUPER_ADMIN, to be delivered out-of-band.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantOnboarding {
    pub organization: OrganizationSummary,
    pub admin_user_id: Uuid,
    /// The one-time cold-start OTP for the new org's first SUPER_ADMIN. Returned
    /// to the platform caller exactly once; never logged or audited.
    pub admin_otp: BootstrapToken,
    pub admin_otp_expires_at: OffsetDateTime,
}

/// Result of a guarded tenant hard-removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantRemovalOutcome {
    /// The empty/test tenant + its shell were deleted; its audit trail was
    /// re-homed to the platform sentinel and preserved.
    Removed,
    /// The tenant owns real operational data and was NOT touched; the caller
    /// surfaces a 409 telling the operator to archive instead. Only the GUARDED
    /// [`PlatformProvisioner::remove_tenant`] path can return this.
    BlockedHasData,
    /// The FORCE path was refused because the tenant is not ARCHIVED; nothing was
    /// touched. The caller surfaces a 409 telling the operator to archive the
    /// tenant (reversible) before force-removing it. Only the FORCE
    /// [`PlatformProvisioner::force_remove_tenant`] path can return this.
    BlockedActive,
    /// No such tenant (or the platform sentinel was targeted); nothing changed.
    NotFound,
}

/// Cross-tenant tenant-provisioning operations for the PLATFORM tier.
///
/// Every method here is a privileged, AUDITED, cross-tenant action. The REST
/// layer only reaches it behind the platform extractor (a tenant token is
/// rejected), so these can never be driven by a tenant admin.
#[derive(Debug, Clone, Copy)]
pub struct PlatformProvisioner {
    onboarding_otp_ttl: Duration,
}

impl PlatformProvisioner {
    #[must_use]
    pub const fn new(onboarding_otp_ttl: Duration) -> Self {
        Self { onboarding_otp_ttl }
    }

    /// Onboard a NEW tenant: create the `organizations` row, seed its first
    /// SUPER_ADMIN user (org_id = the new org), and issue a per-org cold-start
    /// OTP — all in ONE transaction, audited to the TARGET org so the action
    /// shows in both the platform and the tenant's audit trail.
    ///
    /// The org row is INSERTed via the SECURITY DEFINER `platform_create_organization`
    /// (migration 0036): `organizations` is SELECT-only + FORCE RLS for the app's
    /// `console_rt` role, so this is the ONLY path that can create an org. The function
    /// arms `app.current_org` to the new id, so the rest of the transaction (the
    /// admin user + the OTP credential, both stamped with the new org) passes the
    /// FORCE-RLS WITH CHECK on every tenant table as the non-owner role.
    pub async fn onboard_tenant(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        slug: &str,
        name: &str,
        now: OffsetDateTime,
    ) -> Result<TenantOnboarding, ProvisioningError> {
        let slug = slug.trim().to_owned();
        let name = name.trim().to_owned();
        if slug.is_empty() || name.is_empty() {
            return Err(ProvisioningError::InvalidRoster(
                "slug and name are required to onboard a tenant".to_owned(),
            ));
        }
        let ttl = self.onboarding_otp_ttl;

        let mut tx = pool.begin().await?;

        // (1) Create the org via the privileged DEFINER function. It arms the GUC
        // to the new id, so every subsequent tenant write in this tx passes RLS.
        let org_id: Uuid = sqlx::query_scalar("SELECT platform_create_organization($1, $2)")
            .bind(&slug)
            .bind(&name)
            .fetch_one(tx.as_mut())
            .await?;

        let organization = fetch_org_tx(&mut tx, org_id).await?;

        // (2) Seed the first SUPER_ADMIN for the new tenant.
        let admin_user_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO users (display_name, roles, is_active, org_id)
            VALUES ($1, $2, true, $3)
            RETURNING id
            "#,
        )
        .bind("Tenant Admin")
        .bind(&["SUPER_ADMIN"] as &[&str])
        .bind(org_id)
        .fetch_one(tx.as_mut())
        .await?;

        // (3) Issue the per-org cold-start OTP for that admin. Fresh per-org —
        // never the removed fixed `coss0000` seed.
        let issue = issue_bootstrap_if_needed_tx(
            &mut tx,
            admin_user_id,
            OrgId::from_uuid(org_id),
            now,
            ttl,
            IssueMode::RejectIfPresent,
        )
        .await?
        .ok_or(ProvisioningError::ActiveBootstrapCredentialExists)?;

        // (4) Audit the onboarding to the TARGET org (so it lands in the tenant's
        // trail too). The OTP value is NEVER recorded.
        let event = AuditEvent::new(
            actor,
            AuditAction::new("platform.tenant.create")?,
            "organizations",
            org_id.to_string(),
            TraceContext::generate(),
            now,
        )
        .with_org(OrgId::from_uuid(org_id))
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "org_id": org_id,
                "slug": slug,
                "admin_user_id": admin_user_id,
                "group_id": organization.group_id,
            })),
        );
        insert_audit_event(&mut tx, &event).await?;

        tx.commit().await?;

        Ok(TenantOnboarding {
            organization,
            admin_user_id,
            admin_otp: issue.token,
            admin_otp_expires_at: issue.expires_at,
        })
    }

    /// List all tenants (cross-tenant read) via the SECURITY DEFINER
    /// `platform_list_organizations` so the platform tier sees every org even
    /// though `organizations` is RLS-gated for `console_rt`. Audited by the caller.
    pub async fn list_tenants(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        now: OffsetDateTime,
    ) -> Result<Vec<OrganizationSummary>, ProvisioningError> {
        let mut tx = pool.begin().await?;
        let rows = sqlx::query(
            r#"
            SELECT
                id, slug, name, status, created_at, updated_at,
                group_id, group_slug, group_name
            FROM platform_list_organizations()
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .fetch_all(tx.as_mut())
        .await?;

        let summaries = rows
            .into_iter()
            .map(|row| {
                Ok(OrganizationSummary {
                    id: row.try_get("id")?,
                    slug: row.try_get("slug")?,
                    name: row.try_get("name")?,
                    status: row.try_get("status")?,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                    group_id: row.try_get("group_id")?,
                    group_slug: row.try_get("group_slug")?,
                    group_name: row.try_get("group_name")?,
                })
            })
            .collect::<Result<Vec<_>, ProvisioningError>>()?;

        // Audited cross-tenant read. This is a PLATFORM-tier event (no single
        // target tenant), so it carries org_id = NULL — the audit_events WITH
        // CHECK allows a NULL-org platform row even with no tenant GUC armed.
        let event = AuditEvent::new(
            actor,
            AuditAction::new("platform.tenant.list")?,
            "organizations",
            "list",
            TraceContext::generate(),
            now,
        )
        .with_snapshots(None, Some(serde_json::json!({ "count": summaries.len() })));
        insert_audit_event(&mut tx, &event).await?;

        tx.commit().await?;
        Ok(summaries)
    }

    /// Cross-tenant ops health rollup for EVERY tenant (audited platform read).
    ///
    /// The aggregation runs through the SECURITY DEFINER `platform_org_health()`
    /// function — the single sanctioned cross-tenant path — which scans all
    /// tenants with `row_security` briefly disabled and restored. This is a
    /// PLATFORM-tier read (org_id = NULL on the audit row) recorded as
    /// `platform.tenant.health`, so no cross-tenant read is ever unaudited.
    pub async fn list_tenant_health(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        now: OffsetDateTime,
    ) -> Result<Vec<TenantHealth>, ProvisioningError> {
        let mut tx = pool.begin().await?;
        let rows = sqlx::query(
            r#"
            SELECT
                id, slug, name, status, group_id, group_slug, group_name,
                user_count, active_user_count,
                active_work_orders, open_work_orders, last_activity_at
            FROM platform_org_health()
            "#,
        )
        .fetch_all(tx.as_mut())
        .await?;

        let adoption_rows = sqlx::query(
            r#"
            SELECT
                org_id, release_cycle, console_route_events, legacy_route_events,
                rum_error_events, rum_perf_p95_ms, last_event_at
            FROM platform_console_route_adoption()
            "#,
        )
        .fetch_all(tx.as_mut())
        .await?;

        let mut adoption_by_org: BTreeMap<Uuid, Vec<RouteAdoptionMetric>> = BTreeMap::new();
        for row in adoption_rows {
            adoption_by_org
                .entry(row.try_get("org_id")?)
                .or_default()
                .push(RouteAdoptionMetric {
                    release_cycle: row.try_get("release_cycle")?,
                    console_route_events: row.try_get("console_route_events")?,
                    legacy_route_events: row.try_get("legacy_route_events")?,
                    rum_error_events: row.try_get("rum_error_events")?,
                    rum_perf_p95_ms: row.try_get("rum_perf_p95_ms")?,
                    last_event_at: row.try_get("last_event_at")?,
                });
        }

        let mut health = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.try_get("id")?;
            let route_adoption = adoption_by_org.remove(&id).unwrap_or_default();
            let zero_legacy_release_cycles = route_adoption
                .iter()
                .filter(|metric| metric.console_route_events > 0 && metric.legacy_route_events == 0)
                .count() as i64;
            health.push(TenantHealth {
                id,
                slug: row.try_get("slug")?,
                name: row.try_get("name")?,
                status: row.try_get("status")?,
                group_id: row.try_get("group_id")?,
                group_slug: row.try_get("group_slug")?,
                group_name: row.try_get("group_name")?,
                user_count: row.try_get("user_count")?,
                active_user_count: row.try_get("active_user_count")?,
                active_work_orders: row.try_get("active_work_orders")?,
                open_work_orders: row.try_get("open_work_orders")?,
                last_activity_at: row.try_get("last_activity_at")?,
                route_adoption,
                zero_legacy_release_cycles,
            });
        }

        // Audited cross-tenant read (PLATFORM-tier; org_id = NULL).
        let event = AuditEvent::new(
            actor,
            AuditAction::new("platform.tenant.health")?,
            "organizations",
            "health",
            TraceContext::generate(),
            now,
        )
        .with_snapshots(None, Some(serde_json::json!({ "count": health.len() })));
        insert_audit_event(&mut tx, &event).await?;

        tx.commit().await?;
        Ok(health)
    }

    /// List platform groups with subsidiary organization identities.
    ///
    /// This is an audited PLATFORM-tier read. It returns group topology only
    /// (group identity + member org identity) and never tenant row-level data.
    pub async fn list_groups(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        now: OffsetDateTime,
    ) -> Result<Vec<GroupSummary>, ProvisioningError> {
        let mut tx = pool.begin().await?;
        let rows = sqlx::query(
            r#"
            SELECT id, slug, name, status, created_at, updated_at, member_count, members
            FROM platform_list_groups()
            "#,
        )
        .fetch_all(tx.as_mut())
        .await?;

        let groups = rows
            .into_iter()
            .map(group_from_row)
            .collect::<Result<Vec<_>, ProvisioningError>>()?;

        let event = AuditEvent::new(
            actor,
            AuditAction::new("platform.group.list")?,
            "groups",
            "list",
            TraceContext::generate(),
            now,
        )
        .with_snapshots(None, Some(serde_json::json!({ "count": groups.len() })));
        insert_audit_event(&mut tx, &event).await?;

        tx.commit().await?;
        Ok(groups)
    }

    /// Create a top-level group identity (not a tenant) for subsidiary management.
    pub async fn create_group(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        slug: &str,
        name: &str,
        now: OffsetDateTime,
    ) -> Result<GroupSummary, ProvisioningError> {
        let slug = slug.trim();
        let name = name.trim();
        if slug.is_empty() || name.is_empty() {
            return Err(ProvisioningError::InvalidRoster(
                "slug and name are required to create a group".to_owned(),
            ));
        }

        let mut tx = pool.begin().await?;
        let group_id: Uuid = sqlx::query_scalar("SELECT platform_create_group($1, $2)")
            .bind(slug)
            .bind(name)
            .fetch_one(tx.as_mut())
            .await
            .map_err(map_group_write_error)?;
        let group = fetch_group_tx(&mut tx, group_id).await?;

        let event = AuditEvent::new(
            actor,
            AuditAction::new("platform.group.create")?,
            "groups",
            group_id.to_string(),
            TraceContext::generate(),
            now,
        )
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "group_id": group_id,
                "slug": slug,
                "name": name,
            })),
        );
        insert_audit_event(&mut tx, &event).await?;

        tx.commit().await?;
        Ok(group)
    }

    /// Update group identity/status. Membership is managed by the explicit
    /// assign/remove endpoints so lifecycle and topology changes stay auditable.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_group(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        group_id: Uuid,
        slug: Option<&str>,
        name: Option<&str>,
        status: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<GroupSummary, ProvisioningError> {
        let slug = slug.map(str::trim).filter(|value| !value.is_empty());
        let name = name.map(str::trim).filter(|value| !value.is_empty());
        if let Some(status) = status
            && !matches!(status, "ACTIVE" | "SUSPENDED" | "ARCHIVED")
        {
            return Err(ProvisioningError::InvalidRoster(format!(
                "invalid group status {status:?}"
            )));
        }
        if slug.is_none() && name.is_none() && status.is_none() {
            return Err(ProvisioningError::InvalidRoster(
                "at least one group field is required".to_owned(),
            ));
        }

        let mut tx = pool.begin().await?;
        let updated_id: Option<Uuid> =
            sqlx::query_scalar("SELECT platform_update_group($1, $2, $3, $4)")
                .bind(group_id)
                .bind(slug)
                .bind(name)
                .bind(status)
                .fetch_one(tx.as_mut())
                .await
                .map_err(map_group_write_error)?;
        let Some(updated_id) = updated_id else {
            tx.rollback().await?;
            return Err(ProvisioningError::NotFound("group not found".to_owned()));
        };
        let group = fetch_group_tx(&mut tx, updated_id).await?;

        let event = AuditEvent::new(
            actor,
            AuditAction::new("platform.group.update")?,
            "groups",
            group_id.to_string(),
            TraceContext::generate(),
            now,
        )
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "group_id": group_id,
                "slug": slug,
                "name": name,
                "status": status,
            })),
        );
        insert_audit_event(&mut tx, &event).await?;

        tx.commit().await?;
        Ok(group)
    }

    /// Assign or move an organization into a group. This updates both the
    /// user-facing `organizations.group_id` identity and the owner-only
    /// `group_memberships` authorization table in one transaction.
    pub async fn assign_org_to_group(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        group_id: Uuid,
        org_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<OrganizationSummary, ProvisioningError> {
        let mut tx = pool.begin().await?;
        let assigned_id: Option<Uuid> =
            sqlx::query_scalar("SELECT platform_assign_org_to_group($1, $2)")
                .bind(group_id)
                .bind(org_id)
                .fetch_one(tx.as_mut())
                .await
                .map_err(map_group_write_error)?;
        let Some(assigned_id) = assigned_id else {
            tx.rollback().await?;
            return Err(ProvisioningError::NotFound(
                "group or organization not found".to_owned(),
            ));
        };
        let organization = fetch_org_tx(&mut tx, assigned_id).await?;

        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_id.to_string())
            .execute(tx.as_mut())
            .await?;

        let event = AuditEvent::new(
            actor,
            AuditAction::new("platform.group.assign_org")?,
            "group_memberships",
            format!("{group_id}:{org_id}"),
            TraceContext::generate(),
            now,
        )
        .with_org(OrgId::from_uuid(org_id))
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "group_id": group_id,
                "org_id": org_id,
            })),
        );
        insert_audit_event(&mut tx, &event).await?;

        tx.commit().await?;
        Ok(organization)
    }

    /// List group-level account grants. Accounts remain homed in a concrete
    /// tenant org; group roles are cross-org grants layered on top.
    pub async fn list_group_accounts(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        group_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<Vec<GroupAccountSummary>, ProvisioningError> {
        let mut tx = pool.begin().await?;
        // Preserve 404 semantics: an empty grant list for an existing group is
        // different from a typoed group id.
        let _group = fetch_group_tx(&mut tx, group_id).await?;
        let rows = sqlx::query(
            r#"
            SELECT
                user_id, display_name, phone, tenant_roles, is_active,
                has_passkey, account_status, org_id, org_slug, org_name,
                group_roles, created_at
            FROM platform_list_group_accounts($1)
            "#,
        )
        .bind(group_id)
        .fetch_all(tx.as_mut())
        .await?;
        let accounts = rows
            .into_iter()
            .map(group_account_from_row)
            .collect::<Result<Vec<_>, _>>()?;

        let event = AuditEvent::new(
            actor,
            AuditAction::new("platform.group.accounts.list")?,
            "group_role_grants",
            group_id.to_string(),
            TraceContext::generate(),
            now,
        )
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "group_id": group_id,
                "count": accounts.len(),
            })),
        );
        insert_audit_event(&mut tx, &event).await?;

        tx.commit().await?;
        Ok(accounts)
    }

    /// Create one tenant-anchored user account and grant a group role.
    ///
    /// The tenant role defaults to MEMBER at the REST layer. This method accepts
    /// an explicit role set so platform operators can intentionally make a user a
    /// tenant admin too; group authority remains stored in group_role_grants.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_group_account(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        group_id: Uuid,
        org_id: Uuid,
        display_name: &str,
        phone: Option<&str>,
        tenant_roles: &[String],
        group_role: &str,
        now: OffsetDateTime,
    ) -> Result<GroupAccountOnboarding, ProvisioningError> {
        let display_name = display_name.trim();
        if display_name.is_empty() {
            return Err(ProvisioningError::InvalidRoster(
                "display_name is required".to_owned(),
            ));
        }
        if tenant_roles.is_empty()
            || tenant_roles
                .iter()
                .any(|role| !VALID_ROLES.contains(&role.as_str()))
        {
            return Err(ProvisioningError::InvalidRoster(
                "tenant role is invalid".to_owned(),
            ));
        }
        if !VALID_GROUP_ROLES.contains(&group_role) {
            return Err(ProvisioningError::InvalidRoster(
                "group role is invalid".to_owned(),
            ));
        }
        let phone = phone.map(str::trim).filter(|value| !value.is_empty());
        let ttl = self.onboarding_otp_ttl;

        let mut tx = pool.begin().await?;
        let user_id: Option<Uuid> =
            sqlx::query_scalar("SELECT platform_create_group_account($1, $2, $3, $4, $5, $6, $7)")
                .bind(group_id)
                .bind(org_id)
                .bind(display_name)
                .bind(phone)
                .bind(tenant_roles)
                .bind(group_role)
                .bind(actor.map(|id| *id.as_uuid()))
                .fetch_one(tx.as_mut())
                .await
                .map_err(map_group_write_error)?;
        let Some(user_id) = user_id else {
            tx.rollback().await?;
            return Err(ProvisioningError::NotFound(
                "group or organization not found".to_owned(),
            ));
        };

        // platform_create_group_account arms app.current_org to org_id for this
        // transaction; keep it armed so the bootstrap credential passes FORCE RLS.
        let issue = issue_bootstrap_if_needed_tx(
            &mut tx,
            user_id,
            OrgId::from_uuid(org_id),
            now,
            ttl,
            IssueMode::RejectIfPresent,
        )
        .await?
        .ok_or(ProvisioningError::ActiveBootstrapCredentialExists)?;

        let account = fetch_group_account_tx(&mut tx, group_id, user_id).await?;

        let event = AuditEvent::new(
            actor,
            AuditAction::new("platform.group.account.create")?,
            "group_role_grants",
            format!("{group_id}:{user_id}:{group_role}"),
            TraceContext::generate(),
            now,
        )
        .with_org(OrgId::from_uuid(org_id))
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "group_id": group_id,
                "org_id": org_id,
                "user_id": user_id,
                "tenant_roles": tenant_roles,
                "group_role": group_role,
            })),
        );
        insert_audit_event(&mut tx, &event).await?;

        tx.commit().await?;
        Ok(GroupAccountOnboarding {
            account,
            otp: issue.token,
            otp_expires_at: issue.expires_at,
        })
    }

    /// Revoke one group role from a tenant-anchored account. The user row is not
    /// deleted; tenant admins can still manage/deactivate it from the tenant UI.
    pub async fn revoke_group_role(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        group_id: Uuid,
        user_id: Uuid,
        group_role: &str,
        now: OffsetDateTime,
    ) -> Result<(), ProvisioningError> {
        if !VALID_GROUP_ROLES.contains(&group_role) {
            return Err(ProvisioningError::InvalidRoster(
                "group role is invalid".to_owned(),
            ));
        }
        let mut tx = pool.begin().await?;
        // Resolve the account through the group helper instead of reading
        // `users` directly under tenant RLS. This also gives us a stable target
        // org for the audit row before the grant is deleted.
        let account = fetch_group_account_tx(&mut tx, group_id, user_id).await?;
        let revoked: Option<Uuid> =
            sqlx::query_scalar("SELECT platform_revoke_group_role($1, $2, $3)")
                .bind(group_id)
                .bind(user_id)
                .bind(group_role)
                .fetch_one(tx.as_mut())
                .await
                .map_err(map_group_write_error)?;
        let Some(_revoked) = revoked else {
            tx.rollback().await?;
            return Err(ProvisioningError::NotFound(
                "group account role not found".to_owned(),
            ));
        };

        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(account.org_id.to_string())
            .execute(tx.as_mut())
            .await?;

        let event = AuditEvent::new(
            actor,
            AuditAction::new("platform.group.account.revoke")?,
            "group_role_grants",
            format!("{group_id}:{user_id}:{group_role}"),
            TraceContext::generate(),
            now,
        )
        .with_snapshots(
            Some(serde_json::json!({
                "group_id": group_id,
                "user_id": user_id,
                "group_role": group_role,
            })),
            None,
        )
        .with_org(OrgId::from_uuid(account.org_id));
        insert_audit_event(&mut tx, &event).await?;

        tx.commit().await?;
        Ok(())
    }

    /// Remove an organization from one group without deleting the organization.
    pub async fn remove_org_from_group(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        group_id: Uuid,
        org_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<OrganizationSummary, ProvisioningError> {
        let mut tx = pool.begin().await?;
        let before_group_id: Option<Uuid> =
            sqlx::query_scalar::<_, Uuid>("SELECT group_id FROM platform_get_organization($1)")
                .bind(org_id)
                .fetch_optional(tx.as_mut())
                .await?;
        let removed_id: Option<Uuid> =
            sqlx::query_scalar("SELECT platform_remove_org_from_group($1, $2)")
                .bind(group_id)
                .bind(org_id)
                .fetch_one(tx.as_mut())
                .await
                .map_err(map_group_write_error)?;
        let Some(removed_id) = removed_id else {
            tx.rollback().await?;
            return Err(ProvisioningError::NotFound(
                "group or organization not found".to_owned(),
            ));
        };
        let organization = fetch_org_tx(&mut tx, removed_id).await?;

        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_id.to_string())
            .execute(tx.as_mut())
            .await?;

        let after_snap = match (before_group_id, organization.group_id) {
            (Some(before), Some(after)) if after != before => Some(serde_json::json!({
                "group_id": after,
                "org_id": org_id,
            })),
            _ => None,
        };
        let event = AuditEvent::new(
            actor,
            AuditAction::new("platform.group.remove_org")?,
            "group_memberships",
            format!("{group_id}:{org_id}"),
            TraceContext::generate(),
            now,
        )
        .with_org(OrgId::from_uuid(org_id))
        .with_snapshots(
            Some(serde_json::json!({
                "group_id": group_id,
                "org_id": org_id,
            })),
            after_snap,
        );
        insert_audit_event(&mut tx, &event).await?;

        tx.commit().await?;
        Ok(organization)
    }

    /// GUARDED hard-removal of a tenant: delete the org AND its empty onboarding
    /// shell, in ONE transaction, AUDITED — but ONLY for an empty/test tenant.
    ///
    /// Runs the SECURITY DEFINER `platform_remove_organization` (migration 0051),
    /// which REFUSES (returns `blocked_has_data`) if the tenant owns any real
    /// operational data (registry equipment / work orders / sites / customers /
    /// inspections / sales / financial / messenger / consents / attendance /
    /// findings), and otherwise deletes the shell children-first and the org row,
    /// re-homing the tenant's immutable `audit_events` to the platform sentinel so
    /// the audit trail survives the tenant. A non-empty tenant is NEVER hard-
    /// deleted — the caller surfaces a 409 telling the operator to archive instead.
    ///
    /// The removal + the `platform.tenant.remove` audit row commit atomically: the
    /// audit row carries `org_id = NULL` (PLATFORM-tier), because the target org no
    /// longer exists and so could not satisfy the `audit_events` org FK / WITH
    /// CHECK — exactly like the `platform.tenant.list`/`health` reads.
    pub async fn remove_tenant(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        org_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<TenantRemovalOutcome, ProvisioningError> {
        let mut tx = pool.begin().await?;

        // Capture the slug for the audit snapshot BEFORE the org row is deleted by
        // the function below (so the trail records WHAT was removed, by name).
        let slug: Option<String> = fetch_org_tx(&mut tx, org_id).await.map(|org| org.slug).ok();

        let outcome_code: String = sqlx::query_scalar("SELECT platform_remove_organization($1)")
            .bind(org_id)
            .fetch_one(tx.as_mut())
            .await?;

        let outcome = match outcome_code.as_str() {
            "removed" => TenantRemovalOutcome::Removed,
            "blocked_has_data" => {
                // Nothing was changed by the function; roll back and refuse so the
                // REST layer returns 409 "archive instead".
                tx.rollback().await?;
                return Ok(TenantRemovalOutcome::BlockedHasData);
            }
            "not_found" => {
                tx.rollback().await?;
                return Ok(TenantRemovalOutcome::NotFound);
            }
            other => {
                tx.rollback().await?;
                return Err(ProvisioningError::InvalidRoster(format!(
                    "unexpected tenant-removal outcome {other:?}"
                )));
            }
        };

        // Audit the removal as a PLATFORM-tier event (org_id = NULL): the target
        // org is gone, so a tenant-scoped audit row would fail the org FK. The GUC
        // is deliberately left unarmed — the `audit_events` WITH CHECK allows a
        // NULL-org platform row with no tenant armed.
        let event = AuditEvent::new(
            actor,
            AuditAction::new("platform.tenant.remove")?,
            "organizations",
            org_id.to_string(),
            TraceContext::generate(),
            now,
        )
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "org_id": org_id,
                "slug": slug,
            })),
        );
        insert_audit_event(&mut tx, &event).await?;

        tx.commit().await?;
        Ok(outcome)
    }

    /// FORCE hard-removal of a tenant: delete the org AND ALL of its data —
    /// registry, work orders, financials, messenger, audit, the lot — in ONE
    /// transaction, AUDITED. The DESTRUCTIVE counterpart to [`Self::remove_tenant`].
    ///
    /// Runs the SECURITY DEFINER `platform_force_remove_organization` (migration
    /// 0059). Unlike the guarded path there is NO has_data guard — erasing real
    /// data is the point — so the function is fail-closed by a different rail: it
    /// REFUSES (returns `blocked_active`) unless the tenant is ARCHIVED. Archiving
    /// (`platform_set_organization_status`) is reversible and is the mandatory
    /// first step, so an ACTIVE or merely SUSPENDED tenant (e.g. KNL) can never be
    /// force-wiped by one call; the caller surfaces a 409 telling the operator to
    /// archive first. The platform sentinel is refused (`not_found`), as in the
    /// guarded path.
    ///
    /// The function deletes every tenant row children-first (a topological sort of
    /// the ~63 ON DELETE RESTRICT FKs) and re-homes the tenant's immutable
    /// `audit_events` to the platform sentinel, exactly like the guarded path, so
    /// the audit trail — including the record of THIS force-wipe — survives the
    /// tenant.
    ///
    /// Before deleting anything we snapshot a small summary (org slug + per-table
    /// row counts of the operational tables we are about to erase) into the
    /// DISTINCT `platform.tenant.force_remove` audit event (`org_id = NULL`,
    /// platform-tier), so the immutable trail records WHAT was wiped even though
    /// the rows themselves are gone. The removal + audit commit atomically.
    pub async fn force_remove_tenant(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        org_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<TenantRemovalOutcome, ProvisioningError> {
        let actor = actor.ok_or_else(|| {
            ProvisioningError::InvalidRoster(
                "platform force removal requires an authenticated actor".to_owned(),
            )
        })?;
        let trace = TraceContext::generate();
        let outcome_code: String = sqlx::query_scalar(
            "SELECT platform_force_remove_organization_command($1, $2, $3, $4, $5)",
        )
        .bind(org_id)
        .bind(*actor.as_uuid())
        .bind(trace.trace_id())
        .bind(trace.span_id())
        .bind(now)
        // rls-arming: ok isolated SECURITY DEFINER control-plane command; the pool has no tenant-table grants.
        .fetch_one(pool)
        .await?;

        match outcome_code.as_str() {
            "removed" => Ok(TenantRemovalOutcome::Removed),
            "blocked_active" => Ok(TenantRemovalOutcome::BlockedActive),
            "not_found" => Ok(TenantRemovalOutcome::NotFound),
            other => Err(ProvisioningError::InvalidRoster(format!(
                "unexpected tenant force-removal outcome {other:?}"
            ))),
        }
    }

    /// Suspend / reactivate a tenant by setting its `status`. Audited to the
    /// TARGET org. Runs via the SECURITY DEFINER `platform_set_organization_status`
    /// (organizations is not UPDATE-able by `console_rt`).
    pub async fn set_tenant_status(
        &self,
        pool: &PgPool,
        actor: Option<UserId>,
        org_id: Uuid,
        status: &str,
        now: OffsetDateTime,
    ) -> Result<OrganizationSummary, ProvisioningError> {
        if !matches!(status, "ACTIVE" | "SUSPENDED" | "ARCHIVED") {
            return Err(ProvisioningError::InvalidRoster(format!(
                "invalid tenant status {status:?}"
            )));
        }

        let mut tx = pool.begin().await?;
        let updated: Option<Uuid> =
            sqlx::query_scalar("SELECT platform_set_organization_status($1, $2)")
                .bind(org_id)
                .bind(status)
                .fetch_one(tx.as_mut())
                .await?;
        let Some(updated_id) = updated else {
            tx.rollback().await?;
            return Err(ProvisioningError::InvalidBootstrapCredential);
        };
        let organization = fetch_org_tx(&mut tx, updated_id).await?;

        // Arm the TARGET org so the audit insert below passes the audit_events
        // WITH CHECK (org_id = GUC). The org row write itself went through the
        // SECURITY DEFINER function, so the GUC was not armed before now.
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(updated_id.to_string())
            .execute(tx.as_mut())
            .await?;

        let event = AuditEvent::new(
            actor,
            AuditAction::new("platform.tenant.status")?,
            "organizations",
            org_id.to_string(),
            TraceContext::generate(),
            now,
        )
        .with_org(OrgId::from_uuid(org_id))
        .with_snapshots(None, Some(serde_json::json!({ "status": status })));
        insert_audit_event(&mut tx, &event).await?;

        tx.commit().await?;
        Ok(organization)
    }
}

/// Outcome of a successful OTP first sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtpRedemption {
    /// The pre-provisioned user the OTP belonged to; the caller mints a session
    /// for this user.
    pub user_id: Uuid,
    /// The tenant the redeemed credential belongs to, resolved from the token
    /// hash BEFORE the RLS-gated read. The caller arms the GUC with it to load
    /// the user and mint the session, since the OTP redeem route runs before the
    /// tenant middleware.
    pub org_id: OrgId,
    /// True when the user has no registered passkey yet, so the frontend should
    /// force passkey enrollment during initial settings.
    pub requires_passkey_setup: bool,
    /// Exact row and immutable provenance read under the source lock. Version 0
    /// retains NULL provenance; callers must not upgrade it by inference.
    pub source: OtpSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtpSource {
    pub credential_id: Uuid,
    pub issuance_version: i16,
    pub issued_generation: Option<i64>,
    pub issuance_purpose: Option<String>,
    pub source_operation_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct RosterImport {
    users: Vec<RosterUser>,
}

#[derive(Debug, Deserialize)]
struct RosterUser {
    display_name: String,
    phone: String,
    team: Option<String>,
    roles: Vec<String>,
    branches: Vec<RosterBranchMembership>,
}

#[derive(Debug, Deserialize)]
struct RosterBranchMembership {
    region: String,
    branch: String,
}

#[derive(Debug, Clone)]
struct NormalizedRosterUser {
    display_name: String,
    phone: String,
    team: Option<String>,
    roles: Vec<String>,
    branches: Vec<BranchRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct BranchRef {
    region: String,
    branch: String,
}

fn normalize_roster(roster: RosterImport) -> Result<Vec<NormalizedRosterUser>, ProvisioningError> {
    if roster.users.is_empty() {
        return Err(ProvisioningError::InvalidRoster(
            "at least one user row is required".to_owned(),
        ));
    }

    let mut phones = BTreeSet::new();
    let mut users = Vec::with_capacity(roster.users.len());

    for row in roster.users {
        let display_name = row.display_name.trim().to_owned();
        if display_name.is_empty() {
            return Err(ProvisioningError::InvalidRoster(
                "display_name is required".to_owned(),
            ));
        }

        let phone = row.phone.trim().to_owned();
        if phone.is_empty() {
            return Err(ProvisioningError::InvalidRoster(
                "phone is required for idempotent roster import".to_owned(),
            ));
        }
        if !phones.insert(phone.clone()) {
            return Err(ProvisioningError::InvalidRoster(format!(
                "duplicate phone {phone}"
            )));
        }

        let team = row.team.map(|team| team.trim().to_owned());
        if let Some(team) = &team
            && !VALID_TEAMS.contains(&team.as_str())
        {
            return Err(ProvisioningError::InvalidRoster(format!(
                "invalid team {team:?} for phone {phone}"
            )));
        }

        let mut roles = row
            .roles
            .into_iter()
            .map(|role| role.trim().to_owned())
            .collect::<Vec<_>>();
        roles.sort();
        roles.dedup();
        if roles.is_empty() {
            return Err(ProvisioningError::InvalidRoster(format!(
                "roles are required for phone {phone}"
            )));
        }
        if let Some(invalid) = roles
            .iter()
            .find(|role| !VALID_ROLES.contains(&role.as_str()))
        {
            return Err(ProvisioningError::InvalidRoster(format!(
                "invalid role {invalid:?} for phone {phone}"
            )));
        }

        let mut branches = row
            .branches
            .into_iter()
            .map(|branch| BranchRef {
                region: branch.region.trim().to_owned(),
                branch: branch.branch.trim().to_owned(),
            })
            .collect::<Vec<_>>();
        branches.sort();
        branches.dedup();
        if branches
            .iter()
            .any(|item| item.region.is_empty() || item.branch.is_empty())
        {
            return Err(ProvisioningError::InvalidRoster(format!(
                "branch region/name are required for phone {phone}"
            )));
        }

        users.push(NormalizedRosterUser {
            display_name,
            phone,
            team,
            roles,
            branches,
        });
    }

    Ok(users)
}

async fn apply_roster_tx(
    tx: &mut Transaction<'_, Postgres>,
    users: Vec<NormalizedRosterUser>,
    now: OffsetDateTime,
    bootstrap_ttl: Duration,
) -> Result<RosterImportReport, ProvisioningError> {
    let mut users = users;
    users.sort_by(|left, right| left.phone.cmp(&right.phone));
    let mut mapping = BTreeMap::new();
    for user in &users {
        let id: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM users WHERE phone=$1 AND org_id=$2")
                .bind(&user.phone)
                .bind(*OrgId::knl().as_uuid())
                .fetch_optional(tx.as_mut())
                .await?;
        mapping.insert(user.phone.clone(), id);
    }
    let existing: BTreeSet<Uuid> = mapping.values().filter_map(|id| *id).collect();
    for id in existing {
        if lock_account_tx(tx, OrgId::knl(), id).await?.is_none() {
            return Err(ProvisioningError::Conflict(
                "roster Account disappeared".into(),
            ));
        }
    }
    for (phone, original) in &mapping {
        let current: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM users WHERE phone=$1 AND org_id=$2")
                .bind(phone)
                .bind(*OrgId::knl().as_uuid())
                .fetch_optional(tx.as_mut())
                .await?;
        if current != *original {
            return Err(ProvisioningError::Conflict(
                "roster Account mapping changed".into(),
            ));
        }
    }
    let mut resolved_branches: BTreeMap<(String, String), Uuid> = BTreeMap::new();
    let mut seen_branch_keys = BTreeSet::new();
    for user in &users {
        for branch_ref in &user.branches {
            let branch_key = (branch_ref.region.clone(), branch_ref.branch.clone());
            if seen_branch_keys.insert(branch_key.clone()) {
                let branch_id = resolve_branch_tx(tx, &user.phone, branch_ref).await?;
                resolved_branches.insert(branch_key, branch_id);
            }
        }
    }

    let mut report = RosterImportReport::default();

    for user in users {
        let desired_branches = user
            .branches
            .iter()
            .filter_map(|branch_ref| {
                resolved_branches
                    .get(&(branch_ref.region.clone(), branch_ref.branch.clone()))
                    .copied()
            })
            .collect::<BTreeSet<_>>();

        // Frozen identity only: never adopt an intervening phone match.
        let existing = if let Some(id) = mapping[&user.phone] {
            Some(
                sqlx::query(
                    "SELECT id, display_name, roles, team FROM users WHERE id=$1 AND org_id=$2",
                )
                .bind(id)
                .bind(*OrgId::knl().as_uuid())
                .fetch_one(tx.as_mut())
                .await?,
            )
        } else {
            None
        };

        let user_id = if let Some(row) = existing {
            let user_id: Uuid = row.try_get("id")?;
            let existing_display_name: String = row.try_get("display_name")?;
            let existing_roles: Vec<String> = row.try_get("roles")?;
            let existing_team: Option<String> = row.try_get("team")?;

            if existing_display_name != user.display_name
                || existing_roles != user.roles
                || existing_team != user.team
            {
                sqlx::query(
                    r#"
                    UPDATE users
                    SET display_name = $1, roles = $2, team = $3
                    WHERE id = $4
                    "#,
                )
                .bind(&user.display_name)
                .bind(&user.roles)
                .bind(&user.team)
                .bind(user_id)
                .execute(tx.as_mut())
                .await?;
                report.users_updated += 1;
            } else {
                report.users_unchanged += 1;
            }

            user_id
        } else {
            let user_id: Uuid = sqlx::query_scalar(
                r#"
                INSERT INTO users (display_name, phone, roles, team, org_id)
                VALUES ($1, $2, $3, $4, $5)
                RETURNING id
                "#,
            )
            .bind(&user.display_name)
            .bind(&user.phone)
            .bind(&user.roles)
            .bind(&user.team)
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(tx.as_mut())
            .await?;
            report.users_created += 1;

            if let Some(issue) = issue_bootstrap_if_needed_tx(
                tx,
                user_id,
                OrgId::knl(),
                now,
                bootstrap_ttl,
                IssueMode::SkipIfPresent,
            )
            .await?
            {
                report.bootstrap_credentials_issued.push(issue);
            }

            user_id
        };

        let existing_branch_rows: Vec<Uuid> =
            sqlx::query_scalar("SELECT branch_id FROM user_branches WHERE user_id = $1")
                .bind(user_id)
                .fetch_all(tx.as_mut())
                .await?;
        let existing_branches = existing_branch_rows.into_iter().collect::<BTreeSet<_>>();

        for branch_id in desired_branches.difference(&existing_branches) {
            sqlx::query(
                "INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)",
            )
            .bind(user_id)
            .bind(*branch_id)
            .bind(*OrgId::knl().as_uuid())
            .execute(tx.as_mut())
            .await?;
            report.branch_memberships_added += 1;
        }

        for branch_id in existing_branches.difference(&desired_branches) {
            sqlx::query("DELETE FROM user_branches WHERE user_id = $1 AND branch_id = $2")
                .bind(user_id)
                .bind(*branch_id)
                .execute(tx.as_mut())
                .await?;
            report.branch_memberships_removed += 1;
        }
    }

    Ok(report)
}

fn group_from_row(row: sqlx::postgres::PgRow) -> Result<GroupSummary, ProvisioningError> {
    let Json(members): Json<Vec<GroupMemberSummary>> = row.try_get("members")?;
    Ok(GroupSummary {
        id: row.try_get("id")?,
        slug: row.try_get("slug")?,
        name: row.try_get("name")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        member_count: row.try_get("member_count")?,
        members,
    })
}

fn group_account_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<GroupAccountSummary, ProvisioningError> {
    Ok(GroupAccountSummary {
        user_id: row.try_get("user_id")?,
        display_name: row.try_get("display_name")?,
        phone: row.try_get("phone")?,
        tenant_roles: row.try_get("tenant_roles")?,
        is_active: row.try_get("is_active")?,
        has_passkey: row.try_get("has_passkey")?,
        account_status: row.try_get("account_status")?,
        org_id: row.try_get("org_id")?,
        org_slug: row.try_get("org_slug")?,
        org_name: row.try_get("org_name")?,
        group_roles: row.try_get("group_roles")?,
        created_at: row.try_get("created_at")?,
    })
}

/// Read one platform group by id via the SECURITY DEFINER `platform_get_group`.
async fn fetch_group_tx(
    tx: &mut Transaction<'_, Postgres>,
    group_id: Uuid,
) -> Result<GroupSummary, ProvisioningError> {
    let row = sqlx::query(
        r#"
        SELECT id, slug, name, status, created_at, updated_at, member_count, members
        FROM platform_get_group($1)
        "#,
    )
    .bind(group_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| ProvisioningError::NotFound("group not found".to_owned()))?;

    group_from_row(row)
}

/// Read one platform group account through the narrow account listing function.
async fn fetch_group_account_tx(
    tx: &mut Transaction<'_, Postgres>,
    group_id: Uuid,
    user_id: Uuid,
) -> Result<GroupAccountSummary, ProvisioningError> {
    let row = sqlx::query(
        r#"
        SELECT
            user_id, display_name, phone, tenant_roles, is_active,
            has_passkey, account_status, org_id, org_slug, org_name,
            group_roles, created_at
        FROM platform_list_group_accounts($1)
        WHERE user_id = $2
        "#,
    )
    .bind(group_id)
    .bind(user_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| ProvisioningError::NotFound("group account not found".to_owned()))?;

    group_account_from_row(row)
}

fn map_group_write_error(err: sqlx::Error) -> ProvisioningError {
    if let sqlx::Error::Database(db) = &err {
        match db.code().as_deref() {
            Some("23505") => {
                return ProvisioningError::Conflict(
                    "group slug or organization membership already exists".to_owned(),
                );
            }
            Some("23514") => {
                return ProvisioningError::InvalidRoster(
                    "group input violates status or slug constraints".to_owned(),
                );
            }
            _ => {}
        }
    }
    ProvisioningError::Sqlx(err)
}

/// Read one organization by id via the SECURITY DEFINER `platform_get_organization`
/// so the platform path sees the row regardless of the tenant GUC state.
async fn fetch_org_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
) -> Result<OrganizationSummary, ProvisioningError> {
    let row = sqlx::query(
        r#"
        SELECT
            id, slug, name, status, created_at, updated_at,
            group_id, group_slug, group_name
        FROM platform_get_organization($1)
        "#,
    )
    .bind(org_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or(ProvisioningError::InvalidBootstrapCredential)?;

    Ok(OrganizationSummary {
        id: row.try_get("id")?,
        slug: row.try_get("slug")?,
        name: row.try_get("name")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        group_id: row.try_get("group_id")?,
        group_slug: row.try_get("group_slug")?,
        group_name: row.try_get("group_name")?,
    })
}

async fn resolve_branch_tx(
    tx: &mut Transaction<'_, Postgres>,
    phone: &str,
    branch_ref: &BranchRef,
) -> Result<Uuid, ProvisioningError> {
    sqlx::query_scalar(
        r#"
        SELECT b.id
        FROM branches b
        JOIN regions r ON r.id = b.region_id
        WHERE r.name = $1 AND b.name = $2
        "#,
    )
    .bind(&branch_ref.region)
    .bind(&branch_ref.branch)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| ProvisioningError::UnknownBranch {
        phone: phone.to_owned(),
        region: branch_ref.region.clone(),
        branch: branch_ref.branch.clone(),
    })
}

/// Controls how [`issue_bootstrap_if_needed_tx`] reacts to a user who already has
/// a registered passkey or an open bootstrap credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IssueMode {
    /// Roster cold-start: a user who already has a passkey or an open credential is
    /// silently skipped (`Ok(None)`). Used for idempotent bulk import.
    SkipIfPresent,
    /// Admin-issued OTP for a zero-credential user: a user who already has a passkey
    /// or an open credential is an ERROR (the caller surfaces a 409). This is the
    /// safe default that keeps an admin from clobbering a user who can already log in.
    RejectIfPresent,
    /// Admin credential RESET (account-recovery escape hatch): the caller has ALREADY
    /// revoked the user's passkeys in this same transaction, so the passkey-existence
    /// check is bypassed and any leftover open bootstrap credential is revoked before
    /// a fresh one is minted. Always issues a new OTP.
    ForceReset,
}

async fn require_active_account_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    user: Uuid,
) -> Result<(), ProvisioningError> {
    if lock_account_tx(tx, org, user).await? != Some(true) {
        return Err(ProvisioningError::InvalidBootstrapCredential);
    }
    Ok(())
}

async fn lock_open_bootstrap_tx(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT id FROM auth_bootstrap_credentials WHERE user_id=$1 AND consumed_at IS NULL AND revoked_at IS NULL ORDER BY id FOR UPDATE")
        .bind(user).fetch_all(tx.as_mut()).await?;
    Ok(())
}

async fn issue_bootstrap_if_needed_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    org: OrgId,
    now: OffsetDateTime,
    ttl: Duration,
    mode: IssueMode,
) -> Result<Option<BootstrapCredentialIssue>, ProvisioningError> {
    require_active_account_tx(tx, org, user_id).await?;
    lock_open_bootstrap_tx(tx, user_id).await?;
    let now = authentication_time_tx(tx, now).await?;
    let expired: Vec<Uuid> = sqlx::query_scalar(
        r#"
        UPDATE auth_bootstrap_credentials
        SET revoked_at = $1, revoked_reason = 'expired'
        WHERE user_id = $2
          AND consumed_at IS NULL
          AND revoked_at IS NULL
          AND expires_at <= $1
        RETURNING id
        "#,
    )
    .bind(now)
    .bind(user_id)
    .fetch_all(tx.as_mut())
    .await?;
    revoke_legacy_otp_families_for_sources_tx(tx, org, user_id, &expired, now).await?;

    // ForceReset is the account-recovery escape hatch: the caller has just revoked
    // every passkey for this user in the SAME transaction, so a non-zero count here
    // would be a stale read of rows already deleted — skip the lockout-preserving
    // passkey check entirely. SkipIfPresent / RejectIfPresent keep enforcing it.
    if mode != IssueMode::ForceReset {
        let passkey_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM auth_webauthn_credentials WHERE user_id = $1")
                .bind(user_id)
                .fetch_one(tx.as_mut())
                .await?;
        if passkey_count > 0 {
            if mode == IssueMode::RejectIfPresent {
                return Err(ProvisioningError::UserAlreadyHasPasskey);
            }
            return Ok(None);
        }
    }

    let existing_active: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM auth_bootstrap_credentials
        WHERE user_id = $1
          AND consumed_at IS NULL
          AND revoked_at IS NULL
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .fetch_optional(tx.as_mut())
    .await?;
    if existing_active.is_some() {
        match mode {
            // Reset always supersedes any stale open code: revoke it so the freshly
            // minted OTP is the user's single valid recovery code.
            IssueMode::ForceReset => {
                let replaced: Vec<Uuid> = sqlx::query_scalar(
                    r#"
                    UPDATE auth_bootstrap_credentials
                    SET revoked_at = $1, revoked_reason = 'reset'
                    WHERE user_id = $2
                      AND consumed_at IS NULL
                      AND revoked_at IS NULL
                    RETURNING id
                    "#,
                )
                .bind(now)
                .bind(user_id)
                .fetch_all(tx.as_mut())
                .await?;
                revoke_legacy_otp_families_for_sources_tx(tx, org, user_id, &replaced, now).await?;
            }
            IssueMode::RejectIfPresent => {
                return Err(ProvisioningError::ActiveBootstrapCredentialExists);
            }
            IssueMode::SkipIfPresent => {
                return Ok(None);
            }
        }
    }

    let credential_id = Uuid::new_v4();
    let token = generate_bootstrap_token();
    let token_hash = hash_token(token.as_str());
    let expires_at = now + ttl;

    // Stamp the credential with the caller-supplied tenant `org`. The caller
    // must arm the SAME org as `app.current_org` for the transaction so the row
    // passes the FORCE-RLS WITH CHECK on `auth_bootstrap_credentials` (KNL roster
    // import, a newly-onboarded org, or an admin-issued OTP).
    sqlx::query(
        r#"
        INSERT INTO auth_bootstrap_credentials (
            id, user_id, token_hash, issued_at, expires_at, org_id
        ) VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(token_hash)
    .bind(now)
    .bind(expires_at)
    .bind(*org.as_uuid())
    .execute(tx.as_mut())
    .await?;

    Ok(Some(BootstrapCredentialIssue {
        credential_id,
        user_id,
        token,
        expires_at,
    }))
}

async fn revoke_legacy_otp_families_for_sources_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    user_id: Uuid,
    sources: &[Uuid],
    now: OffsetDateTime,
) -> Result<(), ProvisioningError> {
    if sources.is_empty() {
        return Ok(());
    }
    let families: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE auth_refresh_token_families f \
         SET revoked_at=$1, revoked_reason='bootstrap_source_revoked' \
         FROM auth_legacy_otp_family_sources s \
         WHERE f.id=s.family_id AND s.source_id=ANY($2) \
           AND f.user_id=$3 AND f.org_id=$4 AND s.user_id=$3 AND s.org_id=$4 \
           AND f.revoked_at IS NULL RETURNING f.id",
    )
    .bind(now)
    .bind(sources)
    .bind(user_id)
    .bind(*org.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;
    if !families.is_empty() {
        sqlx::query(
            "UPDATE auth_refresh_tokens SET revoked_at=COALESCE(revoked_at,$1) \
             WHERE family_id=ANY($2) AND user_id=$3 AND org_id=$4",
        )
        .bind(now)
        .bind(&families)
        .bind(user_id)
        .bind(*org.as_uuid())
        .execute(tx.as_mut())
        .await?;
    }
    Ok(())
}

/// Seed the cold-start admin's bootstrap credential inside the caller's
/// transaction, if and only if it is needed.
///
/// Returns `Some((admin_id, credential_id))` when a credential was opened and
/// `None` when seeding was skipped (no cold-start admin row, the admin already
/// has a passkey or an open bootstrap credential, or the supplied OTP hash is
/// already in use by another row that cannot be revived). The admin row is locked
/// `FOR UPDATE` so concurrent boots serialize on it and cannot double-seed.
///
/// `token_hash` is globally UNIQUE on `auth_bootstrap_credentials`. On an
/// environment that ran migration 0021 and then 0023, a REVOKED row with the
/// fixed `coss0000` hash still exists; re-seeding the same OTP would collide. The
/// insert is therefore an UPSERT that REVIVES a previously revoked-OR-expired,
/// unconsumed row owned by this same admin (clearing `revoked_at`/`consumed_at`
/// and refreshing the expiry) instead of inserting a duplicate. A conflicting row
/// owned by a different user, one already consumed, or one still valid is left
/// untouched and seeding is reported as skipped.
async fn seed_cold_start_if_needed_tx(
    tx: &mut Transaction<'_, Postgres>,
    token_hash: &[u8],
    now: OffsetDateTime,
    ttl: Duration,
) -> Result<Option<(Uuid, Uuid, OffsetDateTime)>, ProvisioningError> {
    let admin_id: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM users
        WHERE display_name = 'Cold Start Admin'
          AND roles @> ARRAY['SUPER_ADMIN']::TEXT[]
        ORDER BY id
        LIMIT 1
        FOR UPDATE
        "#,
    )
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(admin_id) = admin_id else {
        return Ok(None);
    };
    let now = authentication_time_tx(tx, now).await?;

    let passkey_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM auth_webauthn_credentials WHERE user_id = $1")
            .bind(admin_id)
            .fetch_one(tx.as_mut())
            .await?;
    if passkey_count > 0 {
        return Ok(None);
    }

    // Only a still-VALID (unexpired, unconsumed, unrevoked) credential blocks
    // re-seeding. An EXPIRED open row must not wedge cold-start forever: once the
    // short TTL lapses the operator needs a fresh window on the next boot, and the
    // UPSERT below revives that expired row.
    let existing_open: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM auth_bootstrap_credentials
        WHERE user_id = $1
          AND consumed_at IS NULL
          AND revoked_at IS NULL
          AND expires_at > $2
        LIMIT 1
        "#,
    )
    .bind(admin_id)
    .bind(now)
    .fetch_optional(tx.as_mut())
    .await?;
    if existing_open.is_some() {
        return Ok(None);
    }

    let credential_id = Uuid::new_v4();
    let expires_at = now + ttl;
    // UPSERT on the globally-unique token_hash: a fresh OTP inserts; a previously
    // REVOKED, unconsumed row owned by THIS admin (e.g. the migration-0021/0023
    // coss0000 row) is revived. Any other conflict (different user, or consumed)
    // fails the WHERE, updates nothing, and returns no row -> reported as skipped.
    let opened: Option<(Uuid, OffsetDateTime)> = sqlx::query_as(
        r#"
        INSERT INTO auth_bootstrap_credentials (
            id, user_id, token_hash, issued_at, expires_at, org_id
        ) VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (token_hash) DO UPDATE
            SET issued_at = EXCLUDED.issued_at,
                expires_at = EXCLUDED.expires_at,
                revoked_at = NULL,
                revoked_reason = NULL,
                consumed_at = NULL
            WHERE auth_bootstrap_credentials.user_id = EXCLUDED.user_id
              AND auth_bootstrap_credentials.consumed_at IS NULL
              AND (auth_bootstrap_credentials.revoked_at IS NOT NULL
                   OR auth_bootstrap_credentials.expires_at <= EXCLUDED.issued_at)
        RETURNING id, issued_at
        "#,
    )
    .bind(credential_id)
    .bind(admin_id)
    .bind(token_hash)
    .bind(now)
    .bind(expires_at)
    // The platform admin lives in the platform sentinel org; the credential
    // carries it so the FORCE-RLS WITH CHECK (org_id = GUC) accepts the row.
    .bind(*OrgId::platform().as_uuid())
    .fetch_optional(tx.as_mut())
    .await?;

    // The cold-start seed may revive the same legacy row and token hash. Its
    // previous families must remain dead so an old ceremony cannot inherit the
    // new issuance window.
    if let Some((source_id, _)) = opened {
        revoke_legacy_otp_families_for_sources_tx(
            tx,
            OrgId::platform(),
            admin_id,
            &[source_id],
            now,
        )
        .await?;
    }

    Ok(opened.map(|(credential_id, issued_at)| (admin_id, credential_id, issued_at)))
}

/// Resolve a bootstrap credential's tenant from its token hash, via the narrow
/// SECURITY DEFINER resolver `platform_resolve_bootstrap_org` (migration 0038).
///
/// `auth_bootstrap_credentials` is FORCE RLS, so the app's non-owner `console_rt`
/// role cannot read a credential row by hash until `app.current_org` is armed —
/// but the org is exactly what we need to arm it. This resolver returns ONLY the
/// org_id (nothing else), breaking that chicken-and-egg without widening any read
/// surface. Returns `None` for an unknown hash.
async fn resolve_bootstrap_org(
    tx: &mut Transaction<'_, Postgres>,
    token_hash: &[u8],
) -> Result<Option<Uuid>, ProvisioningError> {
    Ok(
        sqlx::query_scalar("SELECT platform_resolve_bootstrap_org($1)")
            .bind(token_hash)
            .fetch_one(tx.as_mut())
            .await?,
    )
}

async fn count_user_passkeys_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<i64, ProvisioningError> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM auth_webauthn_credentials WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(tx.as_mut())
            .await?,
    )
}

/// Admin-issued OTP length and alphabet.
///
/// 8 characters over a 72-symbol copy-paste-safe alphabet: A-Z, a-z, 0-9 and the
/// special set `!@#$%^&*-_`. That is 72^8 ≈ 2^49.3 of entropy. Eight characters
/// is the product's explicit choice; the brute-force guarantee therefore rests on
/// the per-client rate limit plus the short (default 24h, configurable) TTL and
/// single-use-on-success consume, NOT on the token length alone.
const OTP_LEN: usize = 8;
const OTP_ALPHABET: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*-_";

/// Generate a cryptographically-random 8-character OTP using rejection sampling
/// over [`OTP_ALPHABET`] so every symbol is equiprobable (no modulo bias).
fn generate_bootstrap_token() -> BootstrapToken {
    let alphabet_len = OTP_ALPHABET.len();
    // Largest multiple of the alphabet length that fits in a byte; bytes at or
    // above this are rejected to keep the distribution uniform.
    let limit = (256 / alphabet_len) * alphabet_len;
    let mut out = String::with_capacity(OTP_LEN);
    while out.len() < OTP_LEN {
        for &byte in fill_random().iter() {
            if (byte as usize) < limit {
                out.push(OTP_ALPHABET[byte as usize % alphabet_len] as char);
                if out.len() == OTP_LEN {
                    break;
                }
            }
        }
    }
    BootstrapToken(out)
}

/// 16 cryptographically-random bytes. `Uuid::new_v4` draws from the OS CSPRNG via
/// `getrandom`, so this reuses the project's existing randomness source without a
/// new dependency.
fn fill_random() -> [u8; 16] {
    *Uuid::new_v4().as_bytes()
}

fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}
