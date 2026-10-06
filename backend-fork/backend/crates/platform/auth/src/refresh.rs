use console_kernel_core::{
    AuditAction, AuditEvent, AuditRequestContext, OrgId, TraceContext, UserId,
};
use console_platform_db::{insert_audit_event, with_audits};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::AuthError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshToken(String);

impl RefreshToken {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct RefreshTokenIssue {
    pub token: RefreshToken,
    pub family_id: Uuid,
    pub token_id: Uuid,
    pub user_id: Uuid,
    pub org_id: OrgId,
    pub expires_at: OffsetDateTime,
    pub family_created_at: OffsetDateTime,
}

/// A terminal security decision has durable revocation/audit effects even
/// though no replacement token is issued. Callers commit it before responding.
pub enum RefreshRotation {
    Issued(RefreshTokenIssue),
    Terminal(RefreshTokenUseError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RefreshTokenUseError {
    #[error("invalid refresh token")]
    InvalidToken,
    #[error("refresh token expired")]
    Expired,
    #[error("refresh token family has been revoked")]
    FamilyRevoked,
    #[error("refresh token reuse detected")]
    ReuseDetected,
    #[error("refresh token storage error")]
    Storage,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RefreshTokenStore;

impl RefreshTokenStore {
    pub async fn issue_family(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        org_id: OrgId,
        now: OffsetDateTime,
        ttl: Duration,
    ) -> Result<RefreshTokenIssue, AuthError> {
        let store = *self;
        with_audits::<_, _, AuthError>(pool, org_id, move |tx| {
            Box::pin(async move {
                let (issue, audit) = store
                    .issue_family_in_tx(tx, user_id, org_id, now, ttl)
                    .await?;
                Ok((issue, vec![audit]))
            })
        })
        .await
    }

    /// Mint one family and its initial token inside a proof owner's transaction.
    /// The caller appends the returned audit and commits both with the proof.
    pub async fn issue_family_in_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        user_id: Uuid,
        org_id: OrgId,
        now: OffsetDateTime,
        ttl: Duration,
    ) -> Result<(RefreshTokenIssue, AuditEvent), AuthError> {
        if crate::lock_account_tx(tx, org_id, user_id).await? != Some(true) {
            return Err(RefreshTokenUseError::InvalidToken.into());
        }
        let now = crate::authentication_time_tx(tx, now).await?;
        let family_id = Uuid::new_v4();
        let token_id = Uuid::new_v4();
        let token = generate_refresh_token();
        let token_hash = hash_token(token.as_str());
        let expires_at = now + ttl;
        let org_uuid = *org_id.as_uuid();

        let audit = AuditEvent::new(
            Some(UserId::from_uuid(user_id)),
            AuditAction::new("auth.refresh.issue")?,
            "auth_refresh_token_family",
            family_id.to_string(),
            TraceContext::generate(),
            now,
        )
        .with_org(org_id)
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "family_id": family_id,
                "token_id": token_id,
                "user_id": user_id,
                "expires_at": expires_at,
            })),
        );

        sqlx::query(
            r#"
            INSERT INTO auth_refresh_token_families (id, user_id, created_at, org_id)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(family_id)
        .bind(user_id)
        .bind(now)
        .bind(org_uuid)
        .execute(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            INSERT INTO auth_refresh_tokens (
                id, family_id, user_id, token_hash, issued_at, expires_at, org_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(token_id)
        .bind(family_id)
        .bind(user_id)
        .bind(token_hash)
        .bind(now)
        .bind(expires_at)
        .bind(org_uuid)
        .execute(tx.as_mut())
        .await?;

        Ok((
            RefreshTokenIssue {
                token: RefreshToken(token),
                family_id,
                token_id,
                user_id,
                org_id,
                expires_at,
                family_created_at: now,
            },
            audit,
        ))
    }

    /// Mint a restricted, source-bound first-enrollment family inside the
    /// command owner's proof/admission transaction. This path is dormant until
    /// all session validators understand purpose and Account generation.
    pub async fn issue_first_enrollment_family_in_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        user_id: Uuid,
        org_id: OrgId,
        auth_generation: i64,
        admission_operation_id: Uuid,
        enrollment_deadline: OffsetDateTime,
    ) -> Result<RefreshTokenIssue, AuthError> {
        let org_uuid = *org_id.as_uuid();
        let active: Option<bool> =
            sqlx::query_scalar("SELECT auth_security.lock_account_for_auth($1,$2)")
                .bind(org_uuid)
                .bind(user_id)
                .fetch_one(tx.as_mut())
                .await?;
        if active != Some(true) {
            return Err(RefreshTokenUseError::InvalidToken.into());
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
        if !matches!(state, Some((generation, Some(false), status))
            if generation == auth_generation && status == "never_enrolled")
        {
            return Err(RefreshTokenUseError::InvalidToken.into());
        }
        let now = crate::authentication_time_tx(tx, OffsetDateTime::now_utc()).await?;
        if enrollment_deadline <= now {
            return Err(RefreshTokenUseError::Expired.into());
        }

        let family_id = Uuid::new_v4();
        let token_id = Uuid::new_v4();
        let token = generate_refresh_token();
        sqlx::query(
            "INSERT INTO auth_refresh_token_families \
             (id,user_id,created_at,org_id,provenance_version,auth_generation,session_purpose,source_kind,source_operation_id) \
             VALUES($1,$2,$3,$4,1,$5,'enrollment','bootstrap_otp',$6)",
        )
        .bind(family_id)
        .bind(user_id)
        .bind(now)
        .bind(org_uuid)
        .bind(auth_generation)
        .bind(admission_operation_id)
        .execute(tx.as_mut())
        .await?;
        sqlx::query(
            "INSERT INTO auth_refresh_tokens \
             (id,family_id,user_id,token_hash,issued_at,expires_at,org_id) \
             VALUES($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(token_id)
        .bind(family_id)
        .bind(user_id)
        .bind(hash_token(&token))
        .bind(now)
        .bind(enrollment_deadline)
        .bind(org_uuid)
        .execute(tx.as_mut())
        .await?;
        Ok(RefreshTokenIssue {
            token: RefreshToken(token),
            family_id,
            token_id,
            user_id,
            org_id,
            expires_at: enrollment_deadline,
            family_created_at: now,
        })
    }

    pub async fn rotate(
        &self,
        pool: &PgPool,
        presented_token: &str,
        now: OffsetDateTime,
        ttl: Duration,
        absolute_ttl: Duration,
    ) -> Result<RefreshTokenIssue, RefreshTokenUseError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|_| RefreshTokenUseError::Storage)?;
        let outcome = self
            .rotate_in_tx(&mut tx, presented_token, now, ttl, absolute_ttl)
            .await
            .map_err(|err| match err {
                AuthError::Refresh(refresh) => refresh,
                _ => RefreshTokenUseError::Storage,
            })?;
        tx.commit()
            .await
            .map_err(|_| RefreshTokenUseError::Storage)?;
        match outcome {
            RefreshRotation::Issued(issue) => Ok(issue),
            RefreshRotation::Terminal(error) => Err(error),
        }
    }

    /// Keep the rotation uncommitted until its caller has also prepared every
    /// credential needed for the response. A failed JWT signature then rolls
    /// back the spent token and its undisclosed replacement together.
    pub async fn rotate_in_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        presented_token: &str,
        now: OffsetDateTime,
        ttl: Duration,
        absolute_ttl: Duration,
    ) -> Result<RefreshRotation, AuthError> {
        let token_hash = hash_token(presented_token);

        // Resolve the family's tenant from the token hash FIRST, then arm the
        // GUC, THEN do the RLS-gated read. `auth_refresh_tokens` is FORCE RLS
        // (migration 0035), so as the non-owner `console_rt` role a lookup-by-hash
        // returns ZERO rows until `app.current_org` is set — but the org is what
        // we need to set it. The narrow SECURITY DEFINER resolver
        // `platform_resolve_token_org` returns only the family's org_id, breaking
        // that chicken-and-egg so refresh works for ANY tenant (not just KNL).
        let Some(org_uuid) = resolve_token_org(tx, &token_hash).await? else {
            return Err(RefreshTokenUseError::InvalidToken.into());
        };
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_uuid.to_string())
            .execute(tx.as_mut())
            .await?;

        let locator: Option<(Uuid, Uuid, Uuid)> = sqlx::query_as(
            "SELECT t.id, t.family_id, t.user_id FROM auth_refresh_tokens t JOIN auth_refresh_token_families f ON f.id=t.family_id AND f.user_id=t.user_id AND f.org_id=t.org_id WHERE t.token_hash=$1 AND t.org_id=$2")
            .bind(&token_hash).bind(org_uuid).fetch_optional(tx.as_mut()).await?;
        let Some((locked_token, locked_family, locked_user)) = locator else {
            return Err(RefreshTokenUseError::InvalidToken.into());
        };
        let active = crate::lock_account_tx(tx, OrgId::from_uuid(org_uuid), locked_user).await?;
        if active.is_none() {
            return Err(RefreshTokenUseError::InvalidToken.into());
        }

        let row = sqlx::query(
            r#"
            SELECT
                t.id AS token_id,
                t.family_id,
                t.user_id,
                t.expires_at,
                t.used_at,
                t.revoked_at AS token_revoked_at,
                f.revoked_at AS family_revoked_at,
                f.created_at AS family_created_at
            FROM auth_refresh_tokens t
            JOIN auth_refresh_token_families f ON f.id = t.family_id
            WHERE t.token_hash = $1 AND t.id = $2 AND t.family_id = $3 AND t.user_id = $4
              AND t.org_id = $5 AND f.org_id = $5 AND f.user_id = $4
            FOR UPDATE OF t, f
            "#,
        )
        .bind(token_hash)
        .bind(locked_token)
        .bind(locked_family)
        .bind(locked_user)
        .bind(org_uuid)
        .fetch_optional(tx.as_mut())
        .await?;

        let Some(row) = row else {
            return Err(RefreshTokenUseError::InvalidToken.into());
        };

        // The request may have waited for the family lock. Never authorize a
        // rotation against a timestamp from before that wait.
        let now = crate::authentication_time_tx(tx, now).await?;
        let token_id: Uuid = row.try_get("token_id")?;
        let family_id: Uuid = row.try_get("family_id")?;
        let user_id: Uuid = row.try_get("user_id")?;
        let expires_at: OffsetDateTime = row.try_get("expires_at")?;
        let used_at: Option<OffsetDateTime> = row.try_get("used_at")?;
        let token_revoked_at: Option<OffsetDateTime> = row.try_get("token_revoked_at")?;
        let family_revoked_at: Option<OffsetDateTime> = row.try_get("family_revoked_at")?;
        let family_created_at: OffsetDateTime = row.try_get("family_created_at")?;

        if family_revoked_at.is_some() {
            return Err(RefreshTokenUseError::FamilyRevoked.into());
        }

        // Preserve a real revoked-family result for offboarded Accounts. An
        // inactive Account with a remaining family still cannot mutate or rotate.
        if active != Some(true) {
            return Err(RefreshTokenUseError::InvalidToken.into());
        }

        // Absolute session-lifetime cap (NIST 800-63B AAL2 reauthentication):
        // a refresh family may rotate freely within `absolute_ttl` of its
        // creation, but past that hard ceiling every rotation is rejected and the
        // family is revoked — forcing a fresh primary authentication. This bounds
        // the lifetime of a silently-rotating session even if individual tokens
        // never expire from inactivity. The revoke commits so a subsequent
        // presentation of any sibling token also fails closed.
        if now >= family_created_at + absolute_ttl {
            sqlx::query(
                r#"
                UPDATE auth_refresh_token_families
                SET revoked_at = $1, revoked_reason = 'absolute_ttl_exceeded'
                WHERE id = $2 AND revoked_at IS NULL
                "#,
            )
            .bind(now)
            .bind(family_id)
            .execute(tx.as_mut())
            .await?;
            sqlx::query(
                "UPDATE auth_refresh_tokens SET revoked_at = COALESCE(revoked_at, $1) WHERE family_id = $2",
            )
            .bind(now)
            .bind(family_id)
            .execute(tx.as_mut())
            .await?;
            insert_audit_in_tx(
                tx,
                OrgId::from_uuid(org_uuid),
                user_id,
                family_id,
                "auth.refresh.absolute_ttl_revoked",
                now,
                serde_json::json!({
                    "family_id": family_id,
                    "revoked_reason": "absolute_ttl_exceeded",
                    "family_created_at": family_created_at,
                }),
            )
            .await?;
            return Ok(RefreshRotation::Terminal(
                RefreshTokenUseError::FamilyRevoked,
            ));
        }

        if used_at.is_some() || token_revoked_at.is_some() {
            revoke_family_for_reuse(tx, family_id, token_id, now).await?;
            insert_audit_in_tx(
                tx,
                OrgId::from_uuid(org_uuid),
                user_id,
                family_id,
                "auth.refresh.reuse_detected",
                now,
                serde_json::json!({
                    "family_id": family_id,
                    "revoked_reason": "reuse_detected",
                    "reused_token_id": token_id,
                }),
            )
            .await?;
            return Ok(RefreshRotation::Terminal(
                RefreshTokenUseError::ReuseDetected,
            ));
        }

        if expires_at <= now {
            sqlx::query("UPDATE auth_refresh_tokens SET revoked_at = $1 WHERE id = $2")
                .bind(now)
                .bind(token_id)
                .execute(tx.as_mut())
                .await?;
            return Ok(RefreshRotation::Terminal(RefreshTokenUseError::Expired));
        }

        let replacement_id = Uuid::new_v4();
        let replacement = generate_refresh_token();
        let replacement_hash = hash_token(replacement.as_str());
        let replacement_expires_at = now + ttl;

        sqlx::query(
            r#"
            INSERT INTO auth_refresh_tokens (
                id, family_id, user_id, token_hash, issued_at, expires_at, org_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(replacement_id)
        .bind(family_id)
        .bind(user_id)
        .bind(replacement_hash)
        .bind(now)
        .bind(replacement_expires_at)
        .bind(org_uuid)
        .execute(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            UPDATE auth_refresh_tokens
            SET used_at = $1, replaced_by = $2
            WHERE id = $3
            "#,
        )
        .bind(now)
        .bind(replacement_id)
        .bind(token_id)
        .execute(tx.as_mut())
        .await?;

        insert_audit_in_tx(
            tx,
            OrgId::from_uuid(org_uuid),
            user_id,
            family_id,
            "auth.refresh",
            now,
            serde_json::json!({
                "family_id": family_id,
                "used_token_id": token_id,
                "replacement_token_id": replacement_id,
                "expires_at": replacement_expires_at,
            }),
        )
        .await?;

        Ok(RefreshRotation::Issued(RefreshTokenIssue {
            token: RefreshToken(replacement),
            family_id,
            token_id: replacement_id,
            user_id,
            org_id: OrgId::from_uuid(org_uuid),
            expires_at: replacement_expires_at,
            family_created_at,
        }))
    }

    pub async fn revoke_family_for_logout(
        &self,
        pool: &PgPool,
        presented_token: &str,
        now: OffsetDateTime,
    ) -> Result<(), RefreshTokenUseError> {
        self.revoke_family_for_logout_inner(pool, presented_token, now)
            .await
            .map_err(|err| match err {
                AuthError::Refresh(refresh) => refresh,
                _ => RefreshTokenUseError::Storage,
            })
    }

    async fn revoke_family_for_logout_inner(
        &self,
        pool: &PgPool,
        presented_token: &str,
        now: OffsetDateTime,
    ) -> Result<(), AuthError> {
        let token_hash = hash_token(presented_token);
        let mut tx = pool.begin().await?;

        // Resolve the family's tenant from the token hash and arm the GUC BEFORE
        // the RLS-gated read, so logout works under `console_rt` for any tenant.
        // See `rotate_inner` for the chicken-and-egg rationale.
        let Some(org_uuid) = resolve_token_org(&mut tx, &token_hash).await? else {
            tx.rollback().await?;
            return Err(RefreshTokenUseError::InvalidToken.into());
        };
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_uuid.to_string())
            .execute(tx.as_mut())
            .await?;

        let locator: Option<(Uuid, Uuid, Uuid)> = sqlx::query_as(
            "SELECT t.id, t.family_id, t.user_id FROM auth_refresh_tokens t JOIN auth_refresh_token_families f ON f.id=t.family_id AND f.user_id=t.user_id AND f.org_id=t.org_id WHERE t.token_hash=$1 AND t.org_id=$2")
            .bind(&token_hash).bind(org_uuid).fetch_optional(tx.as_mut()).await?;
        let Some((locked_token, locked_family, locked_user)) = locator else {
            return Err(RefreshTokenUseError::InvalidToken.into());
        };
        let active =
            crate::lock_account_tx(&mut tx, OrgId::from_uuid(org_uuid), locked_user).await?;
        if active.is_none() {
            return Err(RefreshTokenUseError::InvalidToken.into());
        }

        let row = sqlx::query(
            r#"
            SELECT
                t.family_id,
                t.user_id,
                f.revoked_at AS family_revoked_at
            FROM auth_refresh_tokens t
            JOIN auth_refresh_token_families f ON f.id = t.family_id
            WHERE t.token_hash = $1 AND t.id = $2 AND t.family_id = $3 AND t.user_id = $4
              AND t.org_id = $5 AND f.org_id = $5 AND f.user_id = $4
            FOR UPDATE OF t, f
            "#,
        )
        .bind(token_hash)
        .bind(locked_token)
        .bind(locked_family)
        .bind(locked_user)
        .bind(org_uuid)
        .fetch_optional(tx.as_mut())
        .await?;

        let Some(row) = row else {
            tx.rollback().await?;
            return Err(RefreshTokenUseError::InvalidToken.into());
        };

        let now = crate::authentication_time_tx(&mut tx, now).await?;
        let family_id: Uuid = row.try_get("family_id")?;
        let user_id: Uuid = row.try_get("user_id")?;
        let family_revoked_at: Option<OffsetDateTime> = row.try_get("family_revoked_at")?;

        if family_revoked_at.is_some() {
            tx.rollback().await?;
            return Err(RefreshTokenUseError::FamilyRevoked.into());
        }

        self.revoke_locked_family_for_logout_in_tx(
            &mut tx,
            OrgId::from_uuid(org_uuid),
            user_id,
            family_id,
            now,
        )
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Shared logout effect. The owner must hold Account, exact family and token
    /// locks before any dependent custody row; commit the audit with revocation.
    pub async fn revoke_locked_family_for_logout_in_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        org: OrgId,
        user_id: Uuid,
        family_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<(), AuthError> {
        self.revoke_locked_family_for_logout_with_audit_context_in_tx(
            tx, org, user_id, family_id, now, None,
        )
        .await
    }

    /// Preserve the owner-owned logout effect and atomically persist metadata
    /// supplied by an authenticated transport. Metadata never grants authority.
    pub async fn revoke_locked_family_for_logout_with_audit_context_in_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        org: OrgId,
        user_id: Uuid,
        family_id: Uuid,
        now: OffsetDateTime,
        audit_context: Option<(TraceContext, AuditRequestContext)>,
    ) -> Result<(), AuthError> {
        let already_audited: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM audit_events WHERE org_id=$1 AND action='auth.logout' AND target_type='auth_refresh_token_family' AND target_id=$2)",
        ).bind(*org.as_uuid()).bind(family_id.to_string()).fetch_one(tx.as_mut()).await?;
        let revoked_reason: Option<String> = sqlx::query_scalar(
            r#"
            UPDATE auth_refresh_token_families
            SET revoked_reason = CASE WHEN revoked_at IS NULL THEN 'logout' ELSE revoked_reason END,
                revoked_at = COALESCE(revoked_at, $1)
            WHERE id = $2 AND user_id = $3 AND org_id = $4
            RETURNING revoked_reason
            "#,
        )
        .bind(now)
        .bind(family_id)
        .bind(user_id)
        .bind(*org.as_uuid())
        .fetch_one(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            UPDATE auth_refresh_tokens
            SET revoked_at = COALESCE(revoked_at, $1)
            WHERE family_id = $2 AND user_id = $3 AND org_id = $4
            "#,
        )
        .bind(now)
        .bind(family_id)
        .bind(user_id)
        .bind(*org.as_uuid())
        .execute(tx.as_mut())
        .await?;

        if !already_audited {
            let mut event = family_audit_event(
                org,
                user_id,
                family_id,
                "auth.logout",
                now,
                serde_json::json!({
                    "family_id": family_id,
                    "revoked_reason": revoked_reason,
                }),
            )?;
            if let Some((trace, request)) = audit_context {
                event.trace = trace;
                event.request_context = request;
            }
            insert_audit_event(tx, &event).await?;
        }

        Ok(())
    }
}

/// Resolve a refresh-token family's tenant from a token hash, via the narrow
/// SECURITY DEFINER resolver `platform_resolve_token_org` (migration 0036).
///
/// `auth_refresh_tokens` is FORCE RLS, so the app's non-owner `console_rt` role
/// cannot read a token row by hash until `app.current_org` is armed — but the
/// org is exactly what we need to arm it. This resolver returns ONLY the org_id
/// (nothing else), breaking that chicken-and-egg without widening any read
/// surface. Returns `None` for an unknown hash.
async fn resolve_token_org(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    token_hash: &[u8],
) -> Result<Option<Uuid>, AuthError> {
    Ok(sqlx::query_scalar("SELECT platform_resolve_token_org($1)")
        .bind(token_hash)
        .fetch_one(tx.as_mut())
        .await?)
}

async fn revoke_family_for_reuse(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    family_id: Uuid,
    reused_token_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE auth_refresh_token_families
        SET revoked_at = $1, revoked_reason = 'reuse_detected'
        WHERE id = $2 AND revoked_at IS NULL
        "#,
    )
    .bind(now)
    .bind(family_id)
    .execute(tx.as_mut())
    .await?;

    sqlx::query(
        r#"
        UPDATE auth_refresh_tokens
        SET revoked_at = COALESCE(revoked_at, $1),
            reuse_detected_at = CASE WHEN id = $2 THEN $1 ELSE reuse_detected_at END
        WHERE family_id = $3
        "#,
    )
    .bind(now)
    .bind(reused_token_id)
    .bind(family_id)
    .execute(tx.as_mut())
    .await?;

    Ok(())
}

async fn insert_audit_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org: OrgId,
    user_id: Uuid,
    family_id: Uuid,
    action: &str,
    now: OffsetDateTime,
    after: serde_json::Value,
) -> Result<(), AuthError> {
    let event = family_audit_event(org, user_id, family_id, action, now, after)?;
    insert_audit_event(tx, &event).await?;
    Ok(())
}

fn family_audit_event(
    org: OrgId,
    user_id: Uuid,
    family_id: Uuid,
    action: &str,
    now: OffsetDateTime,
    after: serde_json::Value,
) -> Result<AuditEvent, AuthError> {
    Ok(AuditEvent::new(
        Some(UserId::from_uuid(user_id)),
        AuditAction::new(action)?,
        "auth_refresh_token_family",
        family_id.to_string(),
        TraceContext::generate(),
        now,
    )
    .with_org(org)
    .with_snapshots(None, Some(after)))
}

fn generate_refresh_token() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    format!("console_rt_{}", hex_encode(&bytes))
}

fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
