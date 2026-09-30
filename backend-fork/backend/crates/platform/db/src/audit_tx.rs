//! `with_audit` — the L1 transactional building block for every state mutation.
//!
//! Pattern (plan §2.2):
//!   `BEGIN → [caller mutation closure: SELECT FOR UPDATE → validate → UPDATE]
//!    → INSERT audit_events → COMMIT`
//!
//! If the closure returns `Err`, the transaction is rolled back and NEITHER
//! the mutation nor the audit row persists — atomicity is the hard contract.

use console_kernel_core::{AuditEvent, OrgId, UserId};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::error::DbError;

/// Bind the tenant to the transaction-local `app.current_org` GUC.
///
/// `set_config(name, value, true)` scopes the setting to the current
/// transaction (`is_local = true`), so it is automatically cleared on
/// COMMIT/ROLLBACK and never leaks to the next checkout of a pooled
/// connection. Postgres RLS policies read this GUC; an unset GUC fails closed
/// (no rows visible, no writes accepted).
async fn set_current_org(tx: &mut Transaction<'_, Postgres>, org: OrgId) -> Result<(), DbError> {
    let org_text = org.as_uuid().to_string();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org_text)
        .execute(tx.as_mut())
        .await
        .map_err(DbError::Sqlx)?;
    Ok(())
}

/// Execute a mutation closure inside a Postgres transaction, then append an
/// audit row in the **same** transaction before committing.
///
/// # Type parameters
/// - `F`: async closure that receives `&mut Transaction<'_, Postgres>` and
///   returns `Result<T, E>`. The closure is responsible for all domain
///   mutations (SELECT FOR UPDATE, validate transition, UPDATE target row).
/// - `T`: the value returned to the caller on success.
/// - `E`: the closure's error type; must be convertible from `DbError` so the
///   caller sees a single unified error surface.
///
/// # Atomicity guarantee
/// The closure error path rolls back the transaction before returning, so
/// neither the mutation nor the audit row ever lands in the database.
///
/// # Example
/// ```ignore
/// let updated = with_audit(&pool, audit_event, |tx| async move {
///     sqlx::query!("UPDATE users SET is_active = false WHERE id = $1", id)
///         .execute(tx.as_mut())
///         .await?;
///     Ok(())
/// }).await?;
/// ```
pub async fn with_audit<F, T, E>(pool: &PgPool, event: AuditEvent, f: F) -> Result<T, E>
where
    F: for<'tx> FnOnce(
        &'tx mut Transaction<'_, Postgres>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T, E>> + Send + 'tx>,
    >,
    E: From<DbError>,
{
    let mut tx = pool.begin().await.map_err(|e| E::from(DbError::Sqlx(e)))?;

    // Tenant gate: bind the org to the transaction-local `app.current_org` GUC
    // BEFORE the caller's closure runs, so Postgres RLS scopes every
    // INSERT/UPDATE/SELECT the closure performs (and the audit insert below) to
    // this tenant. `None` leaves the GUC unset — legacy, pre-multi-tenant
    // callers keep their existing behavior.
    if let Some(org) = event.org_id {
        set_current_org(&mut tx, org).await.map_err(E::from)?;
    }

    // Run the caller's mutation closure.
    let result = f(&mut tx).await;

    match result {
        Err(e) => {
            // Rollback is best-effort; the transaction is dropped on error
            // regardless, but explicit rollback surfaces the intent.
            let _ = tx.rollback().await;
            Err(e)
        }
        Ok(value) => {
            insert_audit_event_tx(&mut tx, &event)
                .await
                .map_err(E::from)?;
            tx.commit().await.map_err(|e| E::from(DbError::Sqlx(e)))?;
            Ok(value)
        }
    }
}

/// Execute a mutation closure that returns one or more audit events computed
/// from rows locked inside the same transaction.
///
/// Use this when the audit snapshots cannot be known before `SELECT FOR UPDATE`,
/// or when a single business action intentionally updates multiple audited
/// targets atomically.
///
/// # Tenant binding (mandatory)
/// `org` is bound to the transaction-local `app.current_org` GUC immediately
/// after `begin()`, before the caller's closure runs — exactly like
/// [`with_audit`] does from `event.org_id`. Unlike `with_audit` the tenant here
/// is a REQUIRED parameter: a multi-statement audited mutation must never open
/// its transaction without arming the org, or its writes would hit Postgres RLS
/// with an unset GUC and fail closed (or, worse, on a not-yet-RLS table, run
/// untenanted). Passing the org explicitly makes that impossible to forget.
pub async fn with_audits<F, T, E>(pool: &PgPool, org: OrgId, f: F) -> Result<T, E>
where
    F: for<'tx> FnOnce(
        &'tx mut Transaction<'_, Postgres>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(T, Vec<AuditEvent>), E>> + Send + 'tx>,
    >,
    E: From<DbError>,
{
    let mut tx = pool.begin().await.map_err(|e| E::from(DbError::Sqlx(e)))?;
    set_current_org(&mut tx, org).await.map_err(E::from)?;
    let result = f(&mut tx).await;

    match result {
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
        Ok((value, events)) => {
            for event in &events {
                insert_audit_event_tx(&mut tx, event)
                    .await
                    .map_err(E::from)?;
            }
            tx.commit().await.map_err(|e| E::from(DbError::Sqlx(e)))?;
            Ok(value)
        }
    }
}

async fn insert_audit_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    event: &AuditEvent,
) -> Result<(), DbError> {
    let before_json: Option<serde_json::Value> = event.before.clone();
    let after_json: Option<serde_json::Value> = event.after.clone();

    let actor_uuid: Option<uuid::Uuid> = event.actor.map(|uid| *uid.as_uuid());
    let event_id_uuid: uuid::Uuid = *event.id.as_uuid();
    let branch_uuid: Option<uuid::Uuid> = event.branch_id.map(|bid| *bid.as_uuid());
    let org_uuid: Option<uuid::Uuid> = event.org_id.map(|oid| *oid.as_uuid());
    let action_str = event.action.as_str();
    let occurred_at = event.occurred_at;
    let trace_id = event.trace.trace_id();
    let span_id = event.trace.span_id();
    let request_context = &event.request_context;
    let classification = &event.classification;
    let classification_badges = classification.badges.as_deref();

    sqlx::query!(
        r#"
        INSERT INTO audit_events (
            id, actor, action, target_type, target_id,
            branch_id, before_snap, after_snap,
            trace_id, span_id, occurred_at, org_id,
            ip, user_agent, auth_method, device,
            classification_badges, anomaly, reason
        ) VALUES (
            $1, $2, $3, $4, $5,
            $6, $7, $8,
            $9, $10, $11, $12,
            $13, $14, $15, $16,
            $17, $18, $19
        )
        "#,
        event_id_uuid,
        actor_uuid,
        action_str,
        event.target_type,
        event.target_id,
        branch_uuid,
        before_json,
        after_json,
        trace_id,
        span_id,
        occurred_at,
        org_uuid,
        request_context.ip.as_deref(),
        request_context.user_agent.as_deref(),
        request_context.auth_method.as_deref(),
        request_context.device.as_deref(),
        classification_badges,
        classification.anomaly,
        classification.reason.as_deref(),
    )
    .execute(tx.as_mut())
    .await
    .map_err(DbError::Sqlx)?;
    Ok(())
}

/// Append one audit row inside an already-open transaction.
///
/// Used by composite state changes needing one atomic transaction with
/// multiple audit events (e.g. P1 dispatch auto-assignment updating both
/// dispatch state and work-order assignment). Delegates to the compile-time
/// checked insert used by `with_audit`/`with_audits`.
pub async fn insert_audit_event(
    tx: &mut Transaction<'_, Postgres>,
    event: &AuditEvent,
) -> Result<(), DbError> {
    insert_audit_event_tx(tx, event).await
}

/// Run a read-only (or otherwise non-audited) closure inside a tenant-scoped
/// transaction.
///
/// Read paths do not flow through `with_audit`, so they need their own way to
/// arm the `app.current_org` GUC before issuing tenant-scoped SELECTs. This
/// opens a transaction, binds the org GUC transaction-locally, runs the
/// caller's closure, and commits. RLS narrows every row the closure sees to
/// `org`.
///
/// # Example
/// ```ignore
/// let orders = with_org_conn(&pool, org, |tx| Box::pin(async move {
///     sqlx::query_scalar!("SELECT count(*) FROM work_orders")
///         .fetch_one(tx.as_mut())
///         .await
///         .map_err(DbError::Sqlx)
/// })).await?;
/// ```
pub async fn with_org_conn<F, T, E>(pool: &PgPool, org: OrgId, f: F) -> Result<T, E>
where
    F: for<'tx> FnOnce(
        &'tx mut Transaction<'_, Postgres>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T, E>> + Send + 'tx>,
    >,
    E: From<DbError>,
{
    with_org_read(pool, org, false, f).await
}

/// Read multiple tenant-scoped statements from one immutable database snapshot.
/// The isolation mode must be set before `set_current_org` issues its first
/// SELECT; otherwise PostgreSQL keeps the default READ COMMITTED semantics.
pub async fn with_org_snapshot<F, T, E>(pool: &PgPool, org: OrgId, f: F) -> Result<T, E>
where
    F: for<'tx> FnOnce(
        &'tx mut Transaction<'_, Postgres>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T, E>> + Send + 'tx>,
    >,
    E: From<DbError>,
{
    with_org_read(pool, org, true, f).await
}

async fn with_org_read<F, T, E>(pool: &PgPool, org: OrgId, snapshot: bool, f: F) -> Result<T, E>
where
    F: for<'tx> FnOnce(
        &'tx mut Transaction<'_, Postgres>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T, E>> + Send + 'tx>,
    >,
    E: From<DbError>,
{
    let mut tx = pool.begin().await.map_err(|e| E::from(DbError::Sqlx(e)))?;
    if snapshot {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(tx.as_mut())
            .await
            .map_err(|e| E::from(DbError::Sqlx(e)))?;
    }
    set_current_org(&mut tx, org).await.map_err(E::from)?;

    match f(&mut tx).await {
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
        Ok(value) => {
            tx.commit().await.map_err(|e| E::from(DbError::Sqlx(e)))?;
            Ok(value)
        }
    }
}

/// Run a closure inside a tenant-scoped transaction that is ALWAYS rolled back —
/// the DRY-RUN sibling of [`with_org_conn`].
///
/// A caller that wants to know what a write WOULD do runs the real writer here
/// and keeps its verdict without its rows. [`with_org_conn`] commits on `Ok`,
/// which is exactly backwards for a simulation: the one case that must persist
/// nothing is the case that succeeded. Re-deciding a writer's verdict in a
/// second, read-only copy of its checks is the alternative, and two copies of a
/// judgement drift — the copy then reports a green the writer would refuse.
///
/// The rollback is unconditional, so the success path is not a `commit` anyone
/// can reach by editing a branch. A rollback that itself fails is reported
/// rather than swallowed on the success path: the caller would otherwise be told
/// its dry run was clean by a transaction whose fate is unknown.
pub async fn with_org_rollback<F, T, E>(pool: &PgPool, org: OrgId, f: F) -> Result<T, E>
where
    F: for<'tx> FnOnce(
        &'tx mut Transaction<'_, Postgres>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T, E>> + Send + 'tx>,
    >,
    E: From<DbError>,
{
    let mut tx = pool.begin().await.map_err(|e| E::from(DbError::Sqlx(e)))?;
    set_current_org(&mut tx, org).await.map_err(E::from)?;
    let result = f(&mut tx).await;
    let rolled_back = tx.rollback().await;
    match result {
        Ok(value) => {
            rolled_back.map_err(|e| E::from(DbError::Sqlx(e)))?;
            Ok(value)
        }
        Err(e) => Err(e),
    }
}

/// DB-current subject-authorization freshness for a specific `(org, user)`.
///
/// The named fields mirror the access-token freshness claims
/// (`authz_policy_version`, `authz_subject_version`, `session_generation`) so a
/// mint site that stamps this onto a token cannot transpose them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubjectAuthzFreshness {
    pub policy_version: u64,
    pub subject_version: u64,
    pub session_generation: u64,
}

/// Read the DB-current subject-authorization freshness for an EXPLICIT
/// `(org, user)` under that org's armed `app.current_org` GUC (RLS-scoped, via
/// [`with_org_conn`]).
///
/// This is the read the non-login access-token mint paths — the platform
/// view-as / tenant-context tokens and the group-admin tenant-context token —
/// use to stamp REAL freshness instead of the zero baseline (Cedar/PBAC
/// activation, ADR-0021). The Cedar shadow/enforcing guard re-reads the SAME
/// `(org, user)` from the SAME tables at guard time, so a token minted with this
/// snapshot matches the guard read and is not falsely denied as stale/missing.
/// It is keyed by an EXPLICIT `(org, user)` — not the ambient request org —
/// because these mints cross the tenant boundary (the token's org is the TARGET
/// tenant and the subject is the operator/actor).
///
/// Absent rows read as 0 — the same "no bump yet" baseline the login path
/// (`load_user_auth_context_tx`) uses: no `policy_versions` row (no custom policy
/// write) ⇒ `policy_version = 0`; no `subject_authz_versions` row (no subject
/// bump — e.g. a platform operator with no `users` row in the target tenant) ⇒
/// `subject_version = session_generation = 0`.
pub async fn read_subject_authz_freshness(
    pool: &PgPool,
    org: OrgId,
    user: UserId,
) -> Result<SubjectAuthzFreshness, DbError> {
    let org_uuid = *org.as_uuid();
    let user_uuid = *user.as_uuid();
    with_org_conn::<_, _, DbError>(pool, org, move |tx| {
        Box::pin(async move {
            // Per-org policy revision (keyed by org_id alone; version 0 == no
            // custom-policy write yet). Same query the login path uses.
            let policy_version: i64 =
                sqlx::query_scalar("SELECT version FROM policy_versions WHERE org_id = $1")
                    .bind(org_uuid)
                    .fetch_optional(tx.as_mut())
                    .await
                    .map_err(DbError::Sqlx)?
                    .unwrap_or(0);

            // Per-(org,user) subject counters. RLS already narrows to the armed
            // org; the explicit `org_id = $2` is defense-in-depth so this stays
            // correct even if ever handed an owner/BYPASSRLS pool (which would
            // otherwise let `fetch_optional` return an arbitrary matching row).
            // Absent ⇒ (0, 0).
            let subject_row = sqlx::query(
                "SELECT version, session_generation FROM subject_authz_versions WHERE user_id = $1 AND org_id = $2",
            )
            .bind(user_uuid)
            .bind(org_uuid)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            let (subject_version, session_generation): (i64, i64) = match subject_row {
                Some(row) => (
                    row.try_get("version").map_err(DbError::Sqlx)?,
                    row.try_get("session_generation").map_err(DbError::Sqlx)?,
                ),
                None => (0, 0),
            };

            Ok(SubjectAuthzFreshness {
                policy_version: u64::try_from(policy_version).unwrap_or(0),
                subject_version: u64::try_from(subject_version).unwrap_or(0),
                session_generation: u64::try_from(session_generation).unwrap_or(0),
            })
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use console_kernel_core::{
        AuditAction, AuditClassification, AuditEvent, AuditRequestContext, BranchId, OrgId,
        TraceContext, UserId,
    };
    use sqlx::{PgPool, Row};
    use time::OffsetDateTime;

    use super::with_audit;
    use crate::error::DbError;

    /// Seed an organization and return its id. `org_id` is now NOT NULL on the
    /// slice tables (multi-tenant phase 1), so every seeded region/branch/user
    /// must carry one.
    async fn seed_org(pool: &PgPool) -> uuid::Uuid {
        sqlx::query_scalar!(
            "INSERT INTO organizations (slug, name) VALUES ($1, $2) RETURNING id",
            format!("t{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
            "Test Org",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// Convenience: build a minimal valid AuditEvent for test use.
    fn make_event(action: &str) -> AuditEvent {
        AuditEvent::new(
            Some(UserId::new()),
            AuditAction::new(action).unwrap(),
            "users",
            uuid::Uuid::new_v4().to_string(),
            TraceContext::generate(),
            OffsetDateTime::now_utc(),
        )
        .with_branch(BranchId::new())
    }

    struct SeededRehomeAuditRow {
        event_id: uuid::Uuid,
        org_id: uuid::Uuid,
        branch_id: uuid::Uuid,
        user_id: uuid::Uuid,
        before_snap: serde_json::Value,
        after_snap: serde_json::Value,
    }

    async fn seed_rehome_audit_row(pool: &PgPool) -> SeededRehomeAuditRow {
        let org_id = seed_org(pool).await;

        let region_id: uuid::Uuid =
            sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
                .bind("Audit Rehome Region")
                .bind(org_id)
                .fetch_one(pool)
                .await
                .unwrap();

        let branch_id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region_id)
        .bind("Audit Rehome Branch")
        .bind(org_id)
        .fetch_one(pool)
        .await
        .unwrap();

        let user_id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind("Audit Rehome User")
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org_id)
        .fetch_one(pool)
        .await
        .unwrap();

        let before_snap = serde_json::json!({"status": "ACTIVE"});
        let after_snap = serde_json::json!({"status": "ARCHIVED"});
        let event = AuditEvent::new(
            Some(UserId::from_uuid(user_id)),
            AuditAction::new("test.rehome_reference").unwrap(),
            "organizations",
            org_id.to_string(),
            TraceContext::generate(),
            OffsetDateTime::now_utc(),
        )
        .with_org(OrgId::from_uuid(org_id))
        .with_branch(BranchId::from_uuid(branch_id))
        .with_snapshots(Some(before_snap.clone()), Some(after_snap.clone()));
        let event_id = *event.id.as_uuid();

        with_audit::<_, (), DbError>(pool, event, |_tx| Box::pin(async move { Ok(()) }))
            .await
            .unwrap();

        SeededRehomeAuditRow {
            event_id,
            org_id,
            branch_id,
            user_id,
            before_snap,
            after_snap,
        }
    }

    // -----------------------------------------------------------------------
    // (a) Happy path: mutation + audit both visible after commit.
    // -----------------------------------------------------------------------
    #[sqlx::test]
    async fn happy_path_mutation_and_audit_both_persist(pool: PgPool) {
        // Seed prerequisite rows (org → region → branch → user).
        let org_id = seed_org(&pool).await;

        let region_id: uuid::Uuid = sqlx::query_scalar!(
            "INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id",
            "Test Region",
            org_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let branch_id: uuid::Uuid = sqlx::query_scalar!(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
            region_id,
            "Test Branch",
            org_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let user_id: uuid::Uuid = sqlx::query_scalar!(
            "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
            "Alice",
            &["MECHANIC"] as &[&str],
            org_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let before_snap = serde_json::json!({"is_active": true});
        let after_snap = serde_json::json!({"is_active": false});

        let event = AuditEvent::new(
            Some(console_kernel_core::UserId::from_uuid(user_id)),
            AuditAction::new("user.deactivate").unwrap(),
            "users",
            user_id.to_string(),
            TraceContext::generate(),
            OffsetDateTime::now_utc(),
        )
        .with_branch(console_kernel_core::BranchId::from_uuid(branch_id))
        .with_snapshots(Some(before_snap.clone()), Some(after_snap.clone()))
        .with_request_context(AuditRequestContext {
            ip: Some("203.0.113.9".to_owned()),
            user_agent: Some("Maintenance Console/1.0".to_owned()),
            auth_method: Some("passkey".to_owned()),
            device: Some("desktop-web".to_owned()),
        })
        .with_classification(AuditClassification {
            badges: Some(vec!["민감정보".to_owned(), "대외비".to_owned()]),
            anomaly: Some(true),
            reason: Some("manager override".to_owned()),
        });

        let event_id = *event.id.as_uuid();

        with_audit::<_, (), DbError>(&pool, event, |tx| {
            Box::pin(async move {
                sqlx::query!("UPDATE users SET is_active = false WHERE id = $1", user_id,)
                    .execute(tx.as_mut())
                    .await?;
                Ok(())
            })
        })
        .await
        .unwrap();

        // Verify mutation persisted.
        let active: bool =
            sqlx::query_scalar!("SELECT is_active FROM users WHERE id = $1", user_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!active, "user should be deactivated after commit");

        // Verify audit row persisted with correct fields.
        let row = sqlx::query!(
            r#"SELECT action, target_type, target_id, before_snap, after_snap,
                      trace_id, span_id
               FROM audit_events WHERE id = $1"#,
            event_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(row.action, "user.deactivate");
        assert_eq!(row.target_type, "users");
        assert_eq!(row.target_id, user_id.to_string());
        assert_eq!(row.before_snap.unwrap(), before_snap);
        assert_eq!(row.after_snap.unwrap(), after_snap);
        assert_eq!(row.trace_id.len(), 32);
        assert_eq!(row.span_id.len(), 16);

        let row = sqlx::query(
            r#"
            SELECT ip, user_agent, auth_method, device,
                   classification_badges, anomaly, reason
            FROM audit_events WHERE id = $1
            "#,
        )
        .bind(event_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let ip: Option<String> = row.try_get("ip").unwrap();
        let user_agent: Option<String> = row.try_get("user_agent").unwrap();
        let auth_method: Option<String> = row.try_get("auth_method").unwrap();
        let device: Option<String> = row.try_get("device").unwrap();
        let badges: Option<Vec<String>> = row.try_get("classification_badges").unwrap();
        let anomaly: Option<bool> = row.try_get("anomaly").unwrap();
        let reason: Option<String> = row.try_get("reason").unwrap();
        assert_eq!(ip.as_deref(), Some("203.0.113.9"));
        assert_eq!(user_agent.as_deref(), Some("Maintenance Console/1.0"));
        assert_eq!(auth_method.as_deref(), Some("passkey"));
        assert_eq!(device.as_deref(), Some("desktop-web"));
        assert_eq!(
            badges.as_deref(),
            Some(&["민감정보".to_owned(), "대외비".to_owned()][..])
        );
        assert_eq!(anomaly, Some(true));
        assert_eq!(reason.as_deref(), Some("manager override"));
    }

    // -----------------------------------------------------------------------
    // (b) Atomicity: closure error → neither mutation nor audit persists.
    // -----------------------------------------------------------------------
    #[sqlx::test]
    async fn atomicity_rollback_drops_both_mutation_and_audit(pool: PgPool) {
        let org_id = seed_org(&pool).await;

        let region_id: uuid::Uuid = sqlx::query_scalar!(
            "INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id",
            "Rollback Region",
            org_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let _branch_id: uuid::Uuid = sqlx::query_scalar!(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
            region_id,
            "Rollback Branch",
            org_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let user_id: uuid::Uuid = sqlx::query_scalar!(
            "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
            "Bob",
            &["MECHANIC"] as &[&str],
            org_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let event = make_event("user.deactivate");
        let event_id = *event.id.as_uuid();

        let result = with_audit::<_, (), DbError>(&pool, event, |tx| {
            Box::pin(async move {
                // Perform a real mutation first...
                sqlx::query!("UPDATE users SET is_active = false WHERE id = $1", user_id,)
                    .execute(tx.as_mut())
                    .await?;
                // ...then simulate a domain validation failure.
                Err(DbError::Sqlx(sqlx::Error::RowNotFound))
            })
        })
        .await;

        assert!(result.is_err(), "should propagate the closure error");

        // Mutation must NOT have persisted.
        let active: bool =
            sqlx::query_scalar!("SELECT is_active FROM users WHERE id = $1", user_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(active, "mutation should have been rolled back");

        // Audit row must NOT have persisted.
        let count: i64 =
            sqlx::query_scalar!("SELECT COUNT(*) FROM audit_events WHERE id = $1", event_id)
                .fetch_one(&pool)
                .await
                .unwrap()
                .unwrap_or(0);
        assert_eq!(count, 0, "audit row should have been rolled back");
    }

    // -----------------------------------------------------------------------
    // (c) Append-only: UPDATE on audit_events must fail (trigger).
    // -----------------------------------------------------------------------
    #[sqlx::test]
    async fn append_only_update_on_audit_events_is_rejected(pool: PgPool) {
        // Insert a legitimate audit row directly (bypassing with_audit for
        // test setup simplicity — the trigger fires regardless of how the row
        // was inserted).
        let org_id = seed_org(&pool).await;

        let region_id: uuid::Uuid = sqlx::query_scalar!(
            "INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id",
            "AO Region",
            org_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let branch_id: uuid::Uuid = sqlx::query_scalar!(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
            region_id,
            "AO Branch",
            org_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let event = make_event("test.action");
        let event_id = *event.id.as_uuid();

        // Insert a legitimate audit row via with_audit so the INSERT path works.
        let event_for_insert = AuditEvent::new(
            None,
            AuditAction::new("test.action").unwrap(),
            "test",
            "target-1",
            TraceContext::generate(),
            OffsetDateTime::now_utc(),
        )
        .with_branch(console_kernel_core::BranchId::from_uuid(branch_id));
        let insert_event_id = *event_for_insert.id.as_uuid();

        with_audit::<_, (), DbError>(&pool, event_for_insert, |_tx| {
            Box::pin(async move { Ok(()) })
        })
        .await
        .unwrap();

        // Now attempt UPDATE — the trigger must raise an exception.
        let update_result = sqlx::query!(
            "UPDATE audit_events SET action = 'tampered.action' WHERE id = $1",
            insert_event_id,
        )
        .execute(&pool)
        .await;

        assert!(
            update_result.is_err(),
            "UPDATE on audit_events must be rejected by trigger"
        );
        let err_msg = update_result.unwrap_err().to_string();
        assert!(
            err_msg.contains("append-only") || err_msg.contains("forbidden"),
            "error message should mention append-only: {err_msg}"
        );

        // Verify the row is unchanged.
        let action: String = sqlx::query_scalar!(
            "SELECT action FROM audit_events WHERE id = $1",
            insert_event_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(action, "test.action");

        // Suppress unused variable warning.
        let _ = (event_id, event);
    }

    // -----------------------------------------------------------------------
    // (c2) Append-only: DELETE on audit_events must fail (trigger).
    // -----------------------------------------------------------------------
    #[sqlx::test]
    async fn append_only_delete_on_audit_events_is_rejected(pool: PgPool) {
        let org_id = seed_org(&pool).await;

        let region_id: uuid::Uuid = sqlx::query_scalar!(
            "INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id",
            "Del Region",
            org_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let branch_id: uuid::Uuid = sqlx::query_scalar!(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
            region_id,
            "Del Branch",
            org_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let event = AuditEvent::new(
            None,
            AuditAction::new("test.delete.target").unwrap(),
            "test",
            "target-2",
            TraceContext::generate(),
            OffsetDateTime::now_utc(),
        )
        .with_branch(console_kernel_core::BranchId::from_uuid(branch_id));
        let insert_event_id = *event.id.as_uuid();

        with_audit::<_, (), DbError>(&pool, event, |_tx| Box::pin(async move { Ok(()) }))
            .await
            .unwrap();

        let delete_result =
            sqlx::query!("DELETE FROM audit_events WHERE id = $1", insert_event_id,)
                .execute(&pool)
                .await;

        assert!(
            delete_result.is_err(),
            "DELETE on audit_events must be rejected by trigger"
        );
        let err_msg = delete_result.unwrap_err().to_string();
        assert!(
            err_msg.contains("append-only") || err_msg.contains("forbidden"),
            "error message should mention append-only: {err_msg}"
        );

        // Row must still exist.
        let count: i64 = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM audit_events WHERE id = $1",
            insert_event_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap_or(0);
        assert_eq!(count, 1, "row should still exist after failed DELETE");
    }

    // -----------------------------------------------------------------------
    // (c3) Re-home exception: reference release is guarded by app.audit_rehome.
    // -----------------------------------------------------------------------
    #[sqlx::test]
    async fn audit_rehome_reference_update_requires_guard_guc(pool: PgPool) {
        let seeded = seed_rehome_audit_row(&pool).await;
        let platform_org = *OrgId::platform().as_uuid();

        let result = sqlx::query(
            "UPDATE audit_events SET org_id = $2, actor = NULL, branch_id = NULL WHERE id = $1",
        )
        .bind(seeded.event_id)
        .bind(platform_org)
        .execute(&pool)
        .await;

        assert!(
            result.is_err(),
            "reference re-home must be rejected unless app.audit_rehome is armed"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("append-only") || err_msg.contains("forbidden"),
            "error message should mention append-only: {err_msg}"
        );

        let row = sqlx::query("SELECT org_id, actor, branch_id FROM audit_events WHERE id = $1")
            .bind(seeded.event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            row.try_get::<Option<uuid::Uuid>, _>("org_id").unwrap(),
            Some(seeded.org_id),
            "org reference must remain unchanged after rejected re-home"
        );
        assert_eq!(
            row.try_get::<Option<uuid::Uuid>, _>("actor").unwrap(),
            Some(seeded.user_id),
            "actor reference must remain unchanged after rejected re-home"
        );
        assert_eq!(
            row.try_get::<Option<uuid::Uuid>, _>("branch_id").unwrap(),
            Some(seeded.branch_id),
            "branch reference must remain unchanged after rejected re-home"
        );
    }

    #[sqlx::test]
    async fn audit_rehome_guard_allows_reference_release_without_content_rewrite(pool: PgPool) {
        let seeded = seed_rehome_audit_row(&pool).await;
        let platform_org = *OrgId::platform().as_uuid();

        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.audit_rehome', 'on', true)")
            .execute(tx.as_mut())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE audit_events SET org_id = $2, actor = NULL, branch_id = NULL WHERE id = $1",
        )
        .bind(seeded.event_id)
        .bind(platform_org)
        .execute(tx.as_mut())
        .await
        .unwrap();
        sqlx::query("SELECT set_config('app.audit_rehome', 'off', true)")
            .execute(tx.as_mut())
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let row = sqlx::query(
            r#"SELECT org_id, actor, branch_id, action, target_type, target_id,
                      before_snap, after_snap
               FROM audit_events WHERE id = $1"#,
        )
        .bind(seeded.event_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(
            row.try_get::<Option<uuid::Uuid>, _>("org_id").unwrap(),
            Some(platform_org)
        );
        assert_eq!(row.try_get::<Option<uuid::Uuid>, _>("actor").unwrap(), None);
        assert_eq!(
            row.try_get::<Option<uuid::Uuid>, _>("branch_id").unwrap(),
            None
        );
        assert_eq!(
            row.try_get::<String, _>("action").unwrap(),
            "test.rehome_reference"
        );
        assert_eq!(
            row.try_get::<String, _>("target_type").unwrap(),
            "organizations"
        );
        assert_eq!(
            row.try_get::<String, _>("target_id").unwrap(),
            seeded.org_id.to_string()
        );
        assert_eq!(
            row.try_get::<Option<serde_json::Value>, _>("before_snap")
                .unwrap(),
            Some(seeded.before_snap)
        );
        assert_eq!(
            row.try_get::<Option<serde_json::Value>, _>("after_snap")
                .unwrap(),
            Some(seeded.after_snap)
        );
    }

    #[sqlx::test]
    async fn audit_rehome_guard_still_rejects_content_updates(pool: PgPool) {
        let seeded = seed_rehome_audit_row(&pool).await;

        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.audit_rehome', 'on', true)")
            .execute(tx.as_mut())
            .await
            .unwrap();
        let result =
            sqlx::query("UPDATE audit_events SET action = 'tampered.action' WHERE id = $1")
                .bind(seeded.event_id)
                .execute(tx.as_mut())
                .await;

        assert!(
            result.is_err(),
            "app.audit_rehome must not allow audit content rewrites"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("append-only") || err_msg.contains("forbidden"),
            "error message should mention append-only: {err_msg}"
        );
        let _ = tx.rollback().await;

        let action: String = sqlx::query_scalar("SELECT action FROM audit_events WHERE id = $1")
            .bind(seeded.event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(action, "test.rehome_reference");
    }

    #[sqlx::test]
    async fn audit_rehome_guard_still_rejects_deletes(pool: PgPool) {
        let seeded = seed_rehome_audit_row(&pool).await;

        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.audit_rehome', 'on', true)")
            .execute(tx.as_mut())
            .await
            .unwrap();
        let result = sqlx::query("DELETE FROM audit_events WHERE id = $1")
            .bind(seeded.event_id)
            .execute(tx.as_mut())
            .await;

        assert!(
            result.is_err(),
            "app.audit_rehome must not allow audit row deletion"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("append-only") || err_msg.contains("forbidden"),
            "error message should mention append-only: {err_msg}"
        );
        let _ = tx.rollback().await;

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE id = $1")
            .bind(seeded.event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "row should still exist after rejected DELETE");
    }

    // -----------------------------------------------------------------------
    // (d) CHECK constraint: malformed action string rejected at the DB layer.
    // -----------------------------------------------------------------------
    #[sqlx::test]
    async fn action_check_constraint_rejects_malformed_actions(pool: PgPool) {
        // The kernel AuditAction type also validates, but we verify the DB
        // CHECK constraint fires independently (defense in depth).
        let bad_action = "NotValid"; // no dot separator, uppercase

        let result = sqlx::query!(
            r#"INSERT INTO audit_events
               (id, action, target_type, target_id, trace_id, span_id, occurred_at)
               VALUES (gen_random_uuid(), $1, 'test', 'x',
                       $2, $3, now())"#,
            bad_action,
            "a".repeat(32),
            "b".repeat(16),
        )
        .execute(&pool)
        .await;

        assert!(
            result.is_err(),
            "DB CHECK constraint must reject malformed action"
        );
    }
}
