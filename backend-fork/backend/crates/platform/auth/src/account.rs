//! Transaction-scoped Account mutex shared by authentication and lifecycle owners.
use console_kernel_core::{AuditAction, AuditEvent, KernelError, OrgId, TraceContext, UserId};
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AuthError;

/// The caller arms the Company context. This does not grant authority.
/// NO KEY UPDATE permits child/audit FK checks while serializing Account owners.
pub async fn lock_account_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    user: Uuid,
) -> Result<Option<bool>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT is_active FROM users WHERE id = $1 AND org_id = $2 FOR NO KEY UPDATE",
    )
    .bind(user)
    .bind(*org.as_uuid())
    .fetch_optional(tx.as_mut())
    .await
}

/// Sample after the last lock wait. The argument is a trusted internal instant,
/// never a request-selected timestamp; a past value cannot extend validation.
pub async fn authentication_time_tx(
    tx: &mut Transaction<'_, Postgres>,
    trusted: OffsetDateTime,
) -> Result<OffsetDateTime, sqlx::Error> {
    let current: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(tx.as_mut())
        .await?;
    Ok(trusted.max(current).max(OffsetDateTime::now_utc()))
}

/// Both HTTP aliases share the count, ownership check, deletion and audit.
/// The caller commits the returned audit in this same transaction.
pub async fn delete_self_passkey_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    user: Uuid,
    key: Uuid,
    now: OffsetDateTime,
) -> Result<AuditEvent, AuthError> {
    if lock_account_tx(tx, org, user).await? != Some(true) {
        return Err(AuthError::InvalidStoredData(
            "inactive or missing Account".into(),
        ));
    }
    let credential: Option<String> = sqlx::query_scalar(
        "SELECT credential_id FROM auth_webauthn_credentials WHERE id = $1 AND user_id = $2 AND org_id = $3 FOR UPDATE")
        .bind(key).bind(user).bind(*org.as_uuid()).fetch_optional(tx.as_mut()).await?;
    let credential = credential.ok_or_else(|| KernelError::not_found("passkey not found"))?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_webauthn_credentials WHERE user_id = $1 AND org_id = $2",
    )
    .bind(user)
    .bind(*org.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    if total <= 1 {
        return Err(KernelError::conflict(
            "cannot delete your last passkey; register another first",
        )
        .into());
    }
    sqlx::query(
        "DELETE FROM auth_webauthn_credentials WHERE id = $1 AND user_id = $2 AND org_id = $3",
    )
    .bind(key)
    .bind(user)
    .bind(*org.as_uuid())
    .execute(tx.as_mut())
    .await?;
    Ok(AuditEvent::new(
        Some(UserId::from_uuid(user)),
        AuditAction::new("auth.passkey.revoke")?,
        "auth_webauthn_credential",
        key.to_string(),
        TraceContext::generate(),
        authentication_time_tx(tx, now).await?,
    )
    .with_org(org)
    .with_snapshots(
        Some(serde_json::json!({"credential_id": credential, "user_id": user})),
        None,
    ))
}
