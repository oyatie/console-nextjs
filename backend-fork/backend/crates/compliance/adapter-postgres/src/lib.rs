//! Postgres compliance adapter.
//!
//! Location pings are deliberately destructible and are not written through
//! `with_audit`; consent lifecycle transitions are audited in the same
//! transaction as the state mutation.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod tx_helpers;

use console_compliance_application::{
    AcceptEvidenceBindingCommand, ArrivalEvent, ArrivalEventPage, ArrivalEventQuery,
    AuditStreamAuthorizationFacts, AuditStreamPage, AuditStreamQuery, AuditStreamReadKind,
    AuditStreamRecord, CEO_COVERT_AUDIT_SENSITIVITY, CEO_COVERT_AUDIT_STREAM_KEY,
    ComplianceControlPage, ComplianceControlQuery, ComplianceFrameworkPage,
    ComplianceFrameworkQuery, ComplianceObligationPage, ComplianceObligationQuery,
    ConsentTransitionCommand, ConsentTransitionKind, CreateComplianceControlCommand,
    CreateComplianceFrameworkCommand, CreateComplianceObligationCommand,
    CreateEvidenceBindingCommand, CreateRegulationImpactCommand, EvidenceBindingPage,
    EvidenceBindingQuery, LinkControlObligationCommand, LinkObligationRegulationCommand,
    LocationConsentLedgerEntry, LocationConsentLedgerPage, LocationConsentLedgerQuery,
    RegulationImpactPage, RegulationImpactQuery, audit_stream_access_event, compliance_audit_event,
    consent_audit_event, relation_audit_snapshot,
};
use console_compliance_domain::{
    ComplianceControl, ComplianceFramework, ComplianceObligation, ComplianceRiskLevel,
    ComplianceScope, ComplianceScopeKind, ControlCadence, ControlObligationCoverage, ControlStatus,
    ControlType, CoverageLevel, CoverageStatus, EvidenceBinding, EvidenceBindingStatus,
    EvidenceConfidence, EvidenceTargetType, FrameworkKind, FrameworkStatus, LocationConsent,
    LocationConsentState, LocationPing, ObligationRegulationLink, ObligationRegulationRelationship,
    ObligationStatus, ObligationType, PersistedLocationConsent, RegulationImpact,
    RegulationImpactStatus, ReviewCadence, evaluate_geofence, validate_control_key,
    validate_date_range, validate_evidence_requirements, validate_evidence_status_transition,
    validate_hash_sha256, validate_metadata_object, validate_optional_text, validate_required_text,
};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, ConsentId, DEFAULT_GEOFENCE_RADIUS_M,
    ErrorKind, KernelError, OrgId, Timestamp, TraceContext, UserId,
};
use console_platform_db::{DbError, insert_audit_event, with_audit, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use std::collections::BTreeSet;
use tx_helpers::{insert_obligation_regulation_link_tx, next_compliance_code_tx};

use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction, postgres::PgRow};

#[derive(Debug, thiserror::Error)]
pub enum PgComplianceError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl From<sqlx::Error> for PgComplianceError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

impl PgComplianceError {
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(error) => error.kind,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Db(DbError::Sqlx(_))
            | Self::Db(DbError::Serialize(_))
            | Self::Db(DbError::CodeIssuance(_)) => ErrorKind::Internal,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PgComplianceStore {
    pool: PgPool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPurge {
    pub dropped_ping_partitions: i32,
    pub deleted_collection_logs: i64,
}

impl PgComplianceStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn transition_consent(
        &self,
        command: ConsentTransitionCommand,
    ) -> Result<LocationConsent, PgComplianceError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let before = self
            .current_or_unrecorded(command.user_id, command.branch_id)
            .await?;
        let mut after = before.clone();
        let transition = command.kind.apply(&mut after, command.occurred_at)?;
        let event = consent_audit_event(&command, &before, &after)?.with_org(org);

        let user_uuid = *command.user_id.as_uuid();
        let branch_uuid = *command.branch_id.as_uuid();
        let actor_uuid = command.actor.map(|actor| *actor.as_uuid());
        let action = command.kind.audit_action().to_string();
        let expected_id = if before.state() == LocationConsentState::NoRecord {
            None
        } else {
            Some(*before.id().as_uuid())
        };
        let expected_status = if before.state() == LocationConsentState::NoRecord {
            None
        } else {
            Some(before.state().as_db_str().to_string())
        };
        let consent_id = *after.id().as_uuid();
        let status = after.state().as_db_str().to_string();
        let granted_at = after.granted_at();
        let suspended_at = after.suspended_at();
        let resumed_at = after.resumed_at();
        let withdrawn_at = after.withdrawn_at();
        let updated_at = after.updated_at().unwrap_or(command.occurred_at);
        let from_status = transition.from.as_db_str().to_string();
        let to_status = transition.to.as_db_str().to_string();
        let occurred_at = command.occurred_at;
        let destroy_location_data = command.kind == ConsentTransitionKind::Withdraw;
        let returned = after.clone();

        with_audit::<_, LocationConsent, PgComplianceError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let current = sqlx::query!(
                    r#"
                    SELECT id, status
                    FROM location_consents
                    WHERE user_id = $1
                    FOR UPDATE
                    "#,
                    user_uuid,
                )
                .fetch_optional(tx.as_mut())
                .await?;

                match (current, expected_id, expected_status.as_deref()) {
                    (None, None, None) => {
                        sqlx::query!(
                            r#"
                            INSERT INTO location_consents (
                                id, user_id, branch_id, status,
                                granted_at, suspended_at, resumed_at, withdrawn_at, updated_at,
                                org_id
                            )
                            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                            "#,
                            consent_id,
                            user_uuid,
                            branch_uuid,
                            status,
                            granted_at,
                            suspended_at,
                            resumed_at,
                            withdrawn_at,
                            updated_at,
                            org_uuid,
                        )
                        .execute(tx.as_mut())
                        .await?;
                    }
                    (Some(row), Some(expected_id), Some(expected_status)) => {
                        if row.id != expected_id || row.status != expected_status {
                            return Err(KernelError::conflict(
                                "location consent changed during transition",
                            )
                            .into());
                        }

                        sqlx::query!(
                            r#"
                            UPDATE location_consents
                            SET status = $2,
                                granted_at = $3,
                                suspended_at = $4,
                                resumed_at = $5,
                                withdrawn_at = $6,
                                updated_at = $7
                            WHERE id = $1
                            "#,
                            consent_id,
                            status,
                            granted_at,
                            suspended_at,
                            resumed_at,
                            withdrawn_at,
                            updated_at,
                        )
                        .execute(tx.as_mut())
                        .await?;
                    }
                    _ => {
                        return Err(KernelError::conflict(
                            "location consent changed before transition lock",
                        )
                        .into());
                    }
                }

                sqlx::query!(
                    r#"
                    INSERT INTO location_consent_ledger (
                        consent_id, user_id, branch_id, actor, action,
                        from_status, to_status, occurred_at, org_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                    "#,
                    consent_id,
                    user_uuid,
                    branch_uuid,
                    actor_uuid,
                    action,
                    from_status,
                    to_status,
                    occurred_at,
                    org_uuid,
                )
                .execute(tx.as_mut())
                .await?;

                if destroy_location_data {
                    sqlx::query!(
                        "DELETE FROM location_collection_logs WHERE user_id = $1",
                        user_uuid,
                    )
                    .execute(tx.as_mut())
                    .await?;

                    sqlx::query!("DELETE FROM location_pings WHERE user_id = $1", user_uuid,)
                        .execute(tx.as_mut())
                        .await?;

                    // The geofence presence state is location-derived, so it is
                    // erased with the raw pings on consent withdrawal. The durable,
                    // coordinate-free site_attendance_events (work facts) are NOT
                    // deleted here — they survive withdrawal like a timesheet (#13).
                    sqlx::query!(
                        "DELETE FROM site_geofence_presence WHERE user_id = $1",
                        user_uuid,
                    )
                    .execute(tx.as_mut())
                    .await?;
                }

                Ok(returned)
            })
        })
        .await
    }

    pub async fn current_consent(
        &self,
        user_id: UserId,
        branch_id: BranchId,
    ) -> Result<LocationConsent, PgComplianceError> {
        self.current_or_unrecorded(user_id, branch_id).await
    }

    // console-gate: state-changing-handler
    // console-gate: audit-exempt location_ping_ingestion
    pub async fn record_location_ping(&self, ping: LocationPing) -> Result<(), PgComplianceError> {
        if !ping.on_duty() {
            return Err(KernelError::forbidden(
                "location pings may only be collected while on duty",
            )
            .into());
        }

        let user_uuid = *ping.user_id().as_uuid();
        let branch_uuid = *ping.branch_id().as_uuid();
        let ping_uuid = *ping.id().as_uuid();
        // Consent is per-user (UNIQUE (user_id)); a multi-branch user who granted
        // consent in one branch may ping while on duty in any branch in scope.
        // The ping's own branch_id is still recorded below for audit/retention.
        let latitude = ping.latitude();
        let longitude = ping.longitude();
        let accuracy_m = ping.accuracy_m();
        let recorded_at = ping.recorded_at();

        let on_duty = ping.on_duty();
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                sqlx::query_scalar!("SELECT location_pings_ensure_partition($1)", recorded_at,)
                    .fetch_one(tx.as_mut())
                    .await?;

                let consent = sqlx::query!(
                    r#"
            SELECT status
            FROM location_consents
            WHERE user_id = $1
            FOR SHARE
            "#,
                    user_uuid,
                )
                .fetch_optional(tx.as_mut())
                .await?;

                match consent.as_ref().map(|row| row.status.as_str()) {
                    Some("GRANTED") => {}
                    Some("SUSPENDED" | "WITHDRAWN") | None => {
                        return Err(KernelError::forbidden(
                            "location consent is not granted for ping collection",
                        )
                        .into());
                    }
                    Some(other) => {
                        return Err(KernelError::validation(format!(
                            "unknown location consent status {other:?}"
                        ))
                        .into());
                    }
                }

                sqlx::query!(
                    r#"
            INSERT INTO location_pings (
                id, user_id, branch_id, latitude, longitude,
                accuracy_m, recorded_at, on_duty, org_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
                    ping_uuid,
                    user_uuid,
                    branch_uuid,
                    latitude,
                    longitude,
                    accuracy_m,
                    recorded_at,
                    on_duty,
                    org_uuid,
                )
                .execute(tx.as_mut())
                .await?;

                sqlx::query!(
                    r#"
            INSERT INTO location_collection_logs (
                user_id, branch_id, ping_id, recorded_at, reason, org_id
            )
            VALUES ($1, $2, $3, $4, 'on_duty_location_ping', $5)
            "#,
                    user_uuid,
                    branch_uuid,
                    ping_uuid,
                    recorded_at,
                    org_uuid,
                )
                .execute(tx.as_mut())
                .await?;

                // Derive arrival/departure for this on-duty ping against the
                // mechanic's active work-order site geofences, in the same tx so
                // the ping and any crossing event commit atomically (#13).
                record_geofence_crossings(
                    tx,
                    org_uuid,
                    user_uuid,
                    latitude,
                    longitude,
                    recorded_at,
                )
                .await?;

                Ok(())
            })
        })
        .await
    }

    // console-gate: audit-exempt location_data_retention_purge
    // Automated data-lifecycle maintenance: it ERASES expired location-derived
    // data (ping partitions, collection logs, and now geofence presence) to honour
    // the retention window. It is not an auditable business event and writes no
    // audit row — consistent with the existing ping/collection-log purge it
    // extends. The durable site_attendance_events work facts are never purged here.
    pub async fn purge_expired_location_data(
        &self,
        retain_after: Timestamp,
    ) -> Result<RetentionPurge, PgComplianceError> {
        let org = current_org().map_err(KernelError::from)?;
        let row = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                // site_geofence_presence is location-DERIVED transient state, so it
                // must age out on the same retention horizon as the raw pings —
                // otherwise an orphaned presence row (e.g. its work order went
                // terminal) would outlive the location-data retention window. The
                // org GUC is armed by with_org_conn, so RLS scopes the delete to
                // this tenant. (The durable, coordinate-free site_attendance_events
                // are NOT purged here — they are a work record, not location data.)
                sqlx::query!(
                    "DELETE FROM site_geofence_presence WHERE updated_at < $1",
                    retain_after,
                )
                .execute(tx.as_mut())
                .await?;

                Ok(sqlx::query!(
                    r#"
            SELECT dropped_ping_partitions, deleted_collection_logs
            FROM purge_expired_location_data($1)
            "#,
                    retain_after,
                )
                .fetch_one(tx.as_mut())
                .await?)
            })
        })
        .await?;

        Ok(RetentionPurge {
            dropped_ping_partitions: row.dropped_ping_partitions.unwrap_or_default(),
            deleted_collection_logs: row.deleted_collection_logs.unwrap_or_default(),
        })
    }

    pub async fn list_location_consent_ledger(
        &self,
        branch_scope: &BranchScope,
        query: LocationConsentLedgerQuery,
    ) -> Result<LocationConsentLedgerPage, PgComplianceError> {
        let total = self
            .count_location_consent_ledger(branch_scope, &query)
            .await?;
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, consent_id, user_id, branch_id, actor, action,
                   from_status, to_status, occurred_at, created_at
            FROM location_consent_ledger l
            WHERE
            "#,
        );
        push_location_consent_ledger_filters(&mut builder, branch_scope, &query);
        builder.push(" ORDER BY l.occurred_at DESC, l.id DESC LIMIT ");
        builder.push_bind(query.limit);
        builder.push(" OFFSET ");
        builder.push_bind(query.offset);

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let id: uuid::Uuid = row.try_get("id")?;
            let consent_id: uuid::Uuid = row.try_get("consent_id")?;
            let user_id: uuid::Uuid = row.try_get("user_id")?;
            let branch_id: uuid::Uuid = row.try_get("branch_id")?;
            let actor: Option<uuid::Uuid> = row.try_get("actor")?;
            let from_status: String = row.try_get("from_status")?;
            let to_status: String = row.try_get("to_status")?;

            items.push(LocationConsentLedgerEntry {
                id: id.to_string(),
                consent_id: consent_id.to_string(),
                user_id: UserId::from_uuid(user_id),
                branch_id: BranchId::from_uuid(branch_id),
                actor: actor.map(UserId::from_uuid),
                action: row.try_get("action")?,
                from_status: LocationConsentState::from_db_str(&from_status)?,
                to_status: LocationConsentState::from_db_str(&to_status)?,
                occurred_at: row.try_get("occurred_at")?,
                created_at: row.try_get("created_at")?,
            });
        }

        Ok(LocationConsentLedgerPage {
            items,
            limit: query.limit,
            offset: query.offset,
            total,
        })
    }

    async fn count_location_consent_ledger(
        &self,
        branch_scope: &BranchScope,
        query: &LocationConsentLedgerQuery,
    ) -> Result<i64, PgComplianceError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT COUNT(*)
            FROM location_consent_ledger l
            WHERE
            "#,
        );
        push_location_consent_ledger_filters(&mut builder, branch_scope, query);
        let org = current_org().map_err(KernelError::from)?;
        let total: i64 = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build_query_scalar().fetch_one(tx.as_mut()).await?) })
        })
        .await?;
        Ok(total)
    }

    /// Read the site arrival/departure events log (issue #13), tenant-scoped and
    /// branch-filtered, newest first. The durable, coordinate-free attendance
    /// rows are hydrated with work-order, mechanic, customer, and admin-entered
    /// site coordinate data for dispatch-map display. Raw phone GPS is never
    /// returned from this feed.
    pub async fn list_arrival_events(
        &self,
        branch_scope: &BranchScope,
        query: ArrivalEventQuery,
    ) -> Result<ArrivalEventPage, PgComplianceError> {
        let total = self.count_arrival_events(branch_scope, &query).await?;
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT l.id,
                   l.work_order_id,
                   l.site_id,
                   w.request_no AS work_order_no,
                   s.name AS site_name,
                   c.name AS customer_name,
                   u.display_name AS mechanic_name,
                   s.latitude,
                   s.longitude,
                   l.kind,
                   l.occurred_at
            FROM site_attendance_events l
            JOIN work_orders w    ON w.id = l.work_order_id
            JOIN registry_sites s ON s.id = l.site_id
            JOIN registry_customers c ON c.id = s.customer_id
            JOIN users u ON u.id = l.user_id
            WHERE
            "#,
        );
        push_arrival_event_filters(&mut builder, branch_scope, &query);
        builder.push(" ORDER BY l.occurred_at DESC, l.id DESC LIMIT ");
        builder.push_bind(query.limit);
        builder.push(" OFFSET ");
        builder.push_bind(query.offset);

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let id: uuid::Uuid = row.try_get("id")?;
            let work_order_id: uuid::Uuid = row.try_get("work_order_id")?;
            let site_id: uuid::Uuid = row.try_get("site_id")?;
            items.push(ArrivalEvent {
                id: id.to_string(),
                work_order_id: work_order_id.to_string(),
                site_id: site_id.to_string(),
                work_order_no: row.try_get("work_order_no")?,
                site_name: row.try_get("site_name")?,
                customer_name: row.try_get("customer_name")?,
                mechanic_name: row.try_get("mechanic_name")?,
                latitude: row.try_get("latitude")?,
                longitude: row.try_get("longitude")?,
                kind: row.try_get("kind")?,
                occurred_at: row.try_get("occurred_at")?,
            });
        }
        Ok(ArrivalEventPage {
            items,
            limit: query.limit,
            offset: query.offset,
            total,
        })
    }

    async fn count_arrival_events(
        &self,
        branch_scope: &BranchScope,
        query: &ArrivalEventQuery,
    ) -> Result<i64, PgComplianceError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT COUNT(*)
            FROM site_attendance_events l
            WHERE
            "#,
        );
        push_arrival_event_filters(&mut builder, branch_scope, query);
        let org = current_org().map_err(KernelError::from)?;
        let total: i64 = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build_query_scalar().fetch_one(tx.as_mut()).await?) })
        })
        .await?;
        Ok(total)
    }

    pub async fn audit_stream_authorization_facts(
        &self,
        user_id: UserId,
        stream_key: &str,
    ) -> Result<AuditStreamAuthorizationFacts, PgComplianceError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let user_uuid = *user_id.as_uuid();
        let stream_key = stream_key.to_owned();
        let (clearance_keys, policy_version, subject_version, session_generation) =
            with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
                Box::pin(async move {
                    let clearance_keys: Vec<String> = sqlx::query_scalar(
                        r#"
                        SELECT clearance_key
                        FROM clearance_assignments
                        WHERE user_id = $1
                          AND stream_key = $2
                          AND status = 'ACTIVE'
                          AND starts_at <= now()
                          AND (expires_at IS NULL OR expires_at > now())
                        ORDER BY clearance_key
                        "#,
                    )
                    .bind(user_uuid)
                    .bind(&stream_key)
                    .fetch_all(tx.as_mut())
                    .await?;

                    let policy_version: i64 = sqlx::query_scalar(
                        r#"
                        SELECT COALESCE(
                            (SELECT version FROM policy_versions WHERE org_id = $1),
                            0
                        )::BIGINT
                        "#,
                    )
                    .bind(org_uuid)
                    .fetch_one(tx.as_mut())
                    .await?;

                    let (subject_version, session_generation): (i64, i64) = sqlx::query_as(
                        r#"
                        SELECT
                            COALESCE(s.version, 0)::BIGINT AS subject_version,
                            COALESCE(s.session_generation, 0)::BIGINT AS session_generation
                        FROM (SELECT 1) seed
                        LEFT JOIN subject_authz_versions s
                          ON s.org_id = $1 AND s.user_id = $2
                        "#,
                    )
                    .bind(org_uuid)
                    .bind(user_uuid)
                    .fetch_one(tx.as_mut())
                    .await?;

                    Ok((
                        clearance_keys,
                        policy_version,
                        subject_version,
                        session_generation,
                    ))
                })
            })
            .await?;

        Ok(AuditStreamAuthorizationFacts {
            active_clearance_keys: clearance_keys.into_iter().collect::<BTreeSet<_>>(),
            policy_version,
            subject_version,
            session_generation,
        })
    }

    pub async fn list_ceo_covert_audit_stream(
        &self,
        actor: UserId,
        read_kind: AuditStreamReadKind,
        query: AuditStreamQuery,
    ) -> Result<AuditStreamPage, PgComplianceError> {
        let org = current_org().map_err(KernelError::from)?;
        let event = audit_stream_access_event(
            actor,
            read_kind,
            &query,
            TraceContext::generate(),
            time::OffsetDateTime::now_utc(),
        )?
        .with_org(org);
        let access_audit_id = event.id.to_string();
        let stream_key = CEO_COVERT_AUDIT_STREAM_KEY.to_owned();
        let limit = query.limit;
        let offset = query.offset;

        with_audit::<_, AuditStreamPage, PgComplianceError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let (items, total) = match read_kind {
                    AuditStreamReadKind::Events => {
                        fetch_labeled_audit_stream_events(tx, &stream_key, limit, offset).await?
                    }
                    AuditStreamReadKind::AccessEvents => {
                        fetch_audit_stream_access_events(tx, &stream_key, limit, offset).await?
                    }
                };
                Ok(AuditStreamPage {
                    items,
                    limit,
                    offset,
                    total,
                    stream_key,
                    read_kind,
                    access_audit_id,
                })
            })
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn create_regulation_impact(
        &self,
        command: CreateRegulationImpactCommand,
    ) -> Result<RegulationImpact, PgComplianceError> {
        validate_create_regulation_impact(&command)?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let id = uuid::Uuid::new_v4();

        with_audits::<_, RegulationImpact, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let code = next_compliance_code_tx(tx, org_uuid, "RG").await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO compliance_regulation_impacts (
                        id, org_id, code, title, jurisdiction, regulator, citation,
                        source_url, impact_area, impact_summary, risk_level, status,
                        effective_from, effective_to, review_due_on, owner_user_id,
                        metadata, created_by, updated_by, created_at, updated_at
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, $6, $7,
                        $8, $9, $10, $11, 'DRAFT',
                        $12, $13, $14, $15,
                        $16, $17, $17, $18, $18
                    )
                    RETURNING id, code, title, jurisdiction, regulator, citation,
                              source_url, impact_area, impact_summary, risk_level,
                              status, effective_from, effective_to, review_due_on,
                              owner_user_id, metadata, created_by, updated_by,
                              created_at, updated_at
                    "#,
                )
                .bind(id)
                .bind(org_uuid)
                .bind(&code)
                .bind(command.title.trim())
                .bind(command.jurisdiction.trim())
                .bind(command.regulator.as_deref().map(str::trim))
                .bind(command.citation.trim())
                .bind(command.source_url.as_deref().map(str::trim))
                .bind(command.impact_area.trim())
                .bind(command.impact_summary.trim())
                .bind(command.risk_level.as_db_str())
                .bind(command.effective_from)
                .bind(command.effective_to)
                .bind(command.review_due_on)
                .bind(command.owner_user_id.map(|id| *id.as_uuid()))
                .bind(command.metadata)
                .bind(*command.actor.as_uuid())
                .bind(command.occurred_at)
                .fetch_one(tx.as_mut())
                .await?;
                let item = regulation_impact_from_row(&row)?;
                let event = compliance_audit_event(
                    "compliance.regulation_impact.create",
                    command.actor,
                    "compliance_regulation_impacts",
                    item.id,
                    command.trace,
                    command.occurred_at,
                    None,
                    Some(relation_audit_snapshot(&item)?),
                )?
                .with_org(org);
                Ok((item, vec![event]))
            })
        })
        .await
    }

    pub async fn list_regulation_impacts(
        &self,
        query: RegulationImpactQuery,
    ) -> Result<RegulationImpactPage, PgComplianceError> {
        let mut count_builder = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*)::BIGINT FROM compliance_regulation_impacts r WHERE TRUE",
        );
        push_regulation_filters(&mut count_builder, &query);
        let org = current_org().map_err(KernelError::from)?;
        let total: i64 = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(count_builder
                    .build_query_scalar()
                    .fetch_one(tx.as_mut())
                    .await?)
            })
        })
        .await?;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, code, title, jurisdiction, regulator, citation,
                   source_url, impact_area, impact_summary, risk_level, status,
                   effective_from, effective_to, review_due_on, owner_user_id,
                   metadata, created_by, updated_by, created_at, updated_at
            FROM compliance_regulation_impacts r
            WHERE TRUE
            "#,
        );
        push_regulation_filters(&mut builder, &query);
        builder.push(" ORDER BY r.updated_at DESC, r.id DESC LIMIT ");
        builder.push_bind(query.page.limit);
        builder.push(" OFFSET ");
        builder.push_bind(query.page.offset);

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        let items = rows
            .iter()
            .map(regulation_impact_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RegulationImpactPage {
            items,
            limit: query.page.limit,
            offset: query.page.offset,
            total,
        })
    }

    // console-gate: state-changing-handler
    pub async fn create_compliance_obligation(
        &self,
        command: CreateComplianceObligationCommand,
    ) -> Result<ComplianceObligation, PgComplianceError> {
        validate_create_compliance_obligation(&command)?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let id = uuid::Uuid::new_v4();

        with_audits::<_, ComplianceObligation, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let code = next_compliance_code_tx(tx, org_uuid, "CP").await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO compliance_obligations (
                        id, org_id, code, title, description, obligation_type,
                        scope_type, scope_ref, branch_id, site_id, owner_user_id,
                        severity, status, effective_from, effective_to,
                        review_cadence, next_review_on, metadata,
                        created_by, updated_by, created_at, updated_at
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, $6,
                        $7, $8, $9, $10, $11,
                        $12, 'DRAFT', $13, $14,
                        $15, $16, $17,
                        $18, $18, $19, $19
                    )
                    RETURNING id, code, title, description, obligation_type,
                              scope_type, scope_ref, branch_id, site_id, owner_user_id,
                              severity, status, effective_from, effective_to,
                              review_cadence, next_review_on, metadata,
                              created_by, updated_by, created_at, updated_at
                    "#,
                )
                .bind(id)
                .bind(org_uuid)
                .bind(&code)
                .bind(command.title.trim())
                .bind(command.description.trim())
                .bind(command.obligation_type.as_db_str())
                .bind(command.scope.kind.as_db_str())
                .bind(command.scope.scope_ref)
                .bind(command.scope.branch_id.map(|id| *id.as_uuid()))
                .bind(command.scope.site_id.map(|id| *id.as_uuid()))
                .bind(command.owner_user_id.map(|id| *id.as_uuid()))
                .bind(command.severity.as_db_str())
                .bind(command.effective_from)
                .bind(command.effective_to)
                .bind(command.review_cadence.map(ReviewCadence::as_db_str))
                .bind(command.next_review_on)
                .bind(command.metadata)
                .bind(*command.actor.as_uuid())
                .bind(command.occurred_at)
                .fetch_one(tx.as_mut())
                .await?;
                let item = compliance_obligation_from_row(&row)?;
                let mut events = vec![
                    compliance_audit_event(
                        "compliance.obligation.create",
                        command.actor,
                        "compliance_obligations",
                        item.id,
                        command.trace.clone(),
                        command.occurred_at,
                        None,
                        Some(relation_audit_snapshot(&item)?),
                    )?
                    .with_org(org),
                ];

                for link in command.regulation_links {
                    let relation = insert_obligation_regulation_link_tx(
                        tx,
                        org_uuid,
                        item.id,
                        link.regulation_impact_id,
                        link.relationship,
                        link.rationale.as_deref(),
                        command.actor,
                        command.occurred_at,
                    )
                    .await?;
                    events.push(
                        compliance_audit_event(
                            "compliance.obligation_regulation.link",
                            command.actor,
                            "compliance_obligation_regulations",
                            relation.id,
                            command.trace.clone(),
                            command.occurred_at,
                            None,
                            Some(relation_audit_snapshot(&relation)?),
                        )?
                        .with_org(org),
                    );
                }

                Ok((item, events))
            })
        })
        .await
    }

    pub async fn list_compliance_obligations(
        &self,
        query: ComplianceObligationQuery,
    ) -> Result<ComplianceObligationPage, PgComplianceError> {
        let mut count_builder = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*)::BIGINT FROM compliance_obligations o WHERE",
        );
        push_obligation_filters(&mut count_builder, &query);
        let org = current_org().map_err(KernelError::from)?;
        let total: i64 = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(count_builder
                    .build_query_scalar()
                    .fetch_one(tx.as_mut())
                    .await?)
            })
        })
        .await?;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, code, title, description, obligation_type,
                   scope_type, scope_ref, branch_id, site_id, owner_user_id,
                   severity, status, effective_from, effective_to,
                   review_cadence, next_review_on, metadata,
                   created_by, updated_by, created_at, updated_at
            FROM compliance_obligations o
            WHERE
            "#,
        );
        push_obligation_filters(&mut builder, &query);
        builder.push(" ORDER BY o.updated_at DESC, o.id DESC LIMIT ");
        builder.push_bind(query.page.limit);
        builder.push(" OFFSET ");
        builder.push_bind(query.page.offset);
        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        let items = rows
            .iter()
            .map(compliance_obligation_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ComplianceObligationPage {
            items,
            limit: query.page.limit,
            offset: query.page.offset,
            total,
        })
    }

    // console-gate: state-changing-handler
    pub async fn link_obligation_regulation(
        &self,
        command: LinkObligationRegulationCommand,
    ) -> Result<ObligationRegulationLink, PgComplianceError> {
        validate_optional_text("rationale", command.rationale.as_deref(), 2_000)?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        with_audits::<_, ObligationRegulationLink, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let relation = insert_obligation_regulation_link_tx(
                    tx,
                    org_uuid,
                    command.obligation_id,
                    command.regulation_impact_id,
                    command.relationship,
                    command.rationale.as_deref(),
                    command.actor,
                    command.occurred_at,
                )
                .await?;
                let event = compliance_audit_event(
                    "compliance.obligation_regulation.link",
                    command.actor,
                    "compliance_obligation_regulations",
                    relation.id,
                    command.trace,
                    command.occurred_at,
                    None,
                    Some(relation_audit_snapshot(&relation)?),
                )?
                .with_org(org);
                Ok((relation, vec![event]))
            })
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn create_compliance_framework(
        &self,
        command: CreateComplianceFrameworkCommand,
    ) -> Result<ComplianceFramework, PgComplianceError> {
        validate_create_compliance_framework(&command)?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let id = uuid::Uuid::new_v4();
        with_audits::<_, ComplianceFramework, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let code = next_compliance_code_tx(tx, org_uuid, "FW").await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO compliance_frameworks (
                        id, org_id, code, name, version_label, framework_kind,
                        status, owner_user_id, effective_from, effective_to,
                        metadata, created_by, updated_by, created_at, updated_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, 'DRAFT', $7, $8, $9, $10, $11, $11, $12, $12)
                    RETURNING id, code, name, version_label, framework_kind, status,
                              owner_user_id, effective_from, effective_to, metadata,
                              created_by, updated_by, created_at, updated_at
                    "#,
                )
                .bind(id)
                .bind(org_uuid)
                .bind(&code)
                .bind(command.name.trim())
                .bind(command.version_label.trim())
                .bind(command.framework_kind.as_db_str())
                .bind(command.owner_user_id.map(|id| *id.as_uuid()))
                .bind(command.effective_from)
                .bind(command.effective_to)
                .bind(command.metadata)
                .bind(*command.actor.as_uuid())
                .bind(command.occurred_at)
                .fetch_one(tx.as_mut())
                .await?;
                let item = compliance_framework_from_row(&row)?;
                let event = compliance_audit_event(
                    "compliance.framework.create",
                    command.actor,
                    "compliance_frameworks",
                    item.id,
                    command.trace,
                    command.occurred_at,
                    None,
                    Some(relation_audit_snapshot(&item)?),
                )?
                .with_org(org);
                Ok((item, vec![event]))
            })
        })
        .await
    }

    pub async fn list_compliance_frameworks(
        &self,
        query: ComplianceFrameworkQuery,
    ) -> Result<ComplianceFrameworkPage, PgComplianceError> {
        let mut count_builder = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*)::BIGINT FROM compliance_frameworks f WHERE TRUE",
        );
        push_framework_filters(&mut count_builder, &query);
        let org = current_org().map_err(KernelError::from)?;
        let total: i64 = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(count_builder
                    .build_query_scalar()
                    .fetch_one(tx.as_mut())
                    .await?)
            })
        })
        .await?;
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, code, name, version_label, framework_kind, status,
                   owner_user_id, effective_from, effective_to, metadata,
                   created_by, updated_by, created_at, updated_at
            FROM compliance_frameworks f
            WHERE TRUE
            "#,
        );
        push_framework_filters(&mut builder, &query);
        builder.push(" ORDER BY f.updated_at DESC, f.id DESC LIMIT ");
        builder.push_bind(query.page.limit);
        builder.push(" OFFSET ");
        builder.push_bind(query.page.offset);
        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        let items = rows
            .iter()
            .map(compliance_framework_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ComplianceFrameworkPage {
            items,
            limit: query.page.limit,
            offset: query.page.offset,
            total,
        })
    }

    // console-gate: state-changing-handler
    pub async fn create_compliance_control(
        &self,
        command: CreateComplianceControlCommand,
    ) -> Result<ComplianceControl, PgComplianceError> {
        validate_create_compliance_control(&command)?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let id = uuid::Uuid::new_v4();
        with_audits::<_, ComplianceControl, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    r#"
                    INSERT INTO compliance_controls (
                        id, org_id, framework_id, control_key, title, objective,
                        control_type, cadence, status, evidence_requirements,
                        owner_user_id, created_by, updated_by, created_at, updated_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'DRAFT', $9, $10, $11, $11, $12, $12)
                    RETURNING id, framework_id, control_key, title, objective,
                              control_type, cadence, status, evidence_requirements,
                              owner_user_id, created_by, updated_by, created_at, updated_at
                    "#,
                )
                .bind(id)
                .bind(org_uuid)
                .bind(command.framework_id)
                .bind(command.control_key.trim())
                .bind(command.title.trim())
                .bind(command.objective.trim())
                .bind(command.control_type.as_db_str())
                .bind(command.cadence.map(ControlCadence::as_db_str))
                .bind(command.evidence_requirements)
                .bind(command.owner_user_id.map(|id| *id.as_uuid()))
                .bind(*command.actor.as_uuid())
                .bind(command.occurred_at)
                .fetch_one(tx.as_mut())
                .await?;
                let item = compliance_control_from_row(&row)?;
                let event = compliance_audit_event(
                    "compliance.control.create",
                    command.actor,
                    "compliance_controls",
                    item.id,
                    command.trace,
                    command.occurred_at,
                    None,
                    Some(relation_audit_snapshot(&item)?),
                )?
                .with_org(org);
                Ok((item, vec![event]))
            })
        })
        .await
    }

    pub async fn list_compliance_controls(
        &self,
        query: ComplianceControlQuery,
    ) -> Result<ComplianceControlPage, PgComplianceError> {
        let mut count_builder = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*)::BIGINT FROM compliance_controls c WHERE c.framework_id = ",
        );
        count_builder.push_bind(query.framework_id);
        push_control_filters(&mut count_builder, &query);
        let org = current_org().map_err(KernelError::from)?;
        let total: i64 = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(count_builder
                    .build_query_scalar()
                    .fetch_one(tx.as_mut())
                    .await?)
            })
        })
        .await?;
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, framework_id, control_key, title, objective,
                   control_type, cadence, status, evidence_requirements,
                   owner_user_id, created_by, updated_by, created_at, updated_at
            FROM compliance_controls c
            WHERE c.framework_id =
            "#,
        );
        builder.push_bind(query.framework_id);
        push_control_filters(&mut builder, &query);
        builder.push(" ORDER BY c.control_key ASC, c.id ASC LIMIT ");
        builder.push_bind(query.page.limit);
        builder.push(" OFFSET ");
        builder.push_bind(query.page.offset);
        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        let items = rows
            .iter()
            .map(compliance_control_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ComplianceControlPage {
            items,
            limit: query.page.limit,
            offset: query.page.offset,
            total,
        })
    }

    // console-gate: state-changing-handler
    pub async fn link_control_obligation(
        &self,
        command: LinkControlObligationCommand,
    ) -> Result<ControlObligationCoverage, PgComplianceError> {
        validate_optional_text(
            "coverage_rationale",
            command.coverage_rationale.as_deref(),
            2_000,
        )?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let id = uuid::Uuid::new_v4();
        with_audits::<_, ControlObligationCoverage, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    r#"
                        INSERT INTO compliance_control_obligations (
                            id, org_id, control_id, obligation_id, coverage_level,
                            coverage_rationale, status, created_by, updated_by,
                            created_at, updated_at
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, 'ACTIVE', $7, $7, $8, $8)
                        ON CONFLICT (org_id, control_id, obligation_id) DO UPDATE
                        SET coverage_level = EXCLUDED.coverage_level,
                            coverage_rationale = EXCLUDED.coverage_rationale,
                            status = 'ACTIVE',
                            updated_by = EXCLUDED.updated_by,
                            updated_at = EXCLUDED.updated_at
                        RETURNING id, control_id, obligation_id, coverage_level,
                                  coverage_rationale, status, created_by, updated_by,
                                  created_at, updated_at
                        "#,
                )
                .bind(id)
                .bind(org_uuid)
                .bind(command.control_id)
                .bind(command.obligation_id)
                .bind(command.coverage_level.as_db_str())
                .bind(command.coverage_rationale.as_deref().map(str::trim))
                .bind(*command.actor.as_uuid())
                .bind(command.occurred_at)
                .fetch_one(tx.as_mut())
                .await?;
                let item = control_obligation_coverage_from_row(&row)?;
                let event = compliance_audit_event(
                    "compliance.control_obligation.link",
                    command.actor,
                    "compliance_control_obligations",
                    item.id,
                    command.trace,
                    command.occurred_at,
                    None,
                    Some(relation_audit_snapshot(&item)?),
                )?
                .with_org(org);
                Ok((item, vec![event]))
            })
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn create_evidence_binding(
        &self,
        command: CreateEvidenceBindingCommand,
    ) -> Result<EvidenceBinding, PgComplianceError> {
        validate_create_evidence_binding(&command)?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let id = uuid::Uuid::new_v4();
        with_audits::<_, EvidenceBinding, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    r#"
                    INSERT INTO compliance_evidence_bindings (
                        id, org_id, control_id, obligation_id, evidence_target_type,
                        evidence_target_id, source_audit_event_id, status, confidence,
                        collected_at, collected_by, valid_from, valid_to, hash_sha256,
                        metadata, created_by, updated_by, created_at, updated_at
                    )
                    VALUES (
                        $1, $2, $3, $4, $5,
                        $6, $7, 'PROPOSED', $8,
                        $9, $10, $11, $12, $13,
                        $14, $15, $15, $16, $16
                    )
                    RETURNING id, control_id, obligation_id, evidence_target_type,
                              evidence_target_id, source_audit_event_id, status,
                              confidence, collected_at, collected_by, valid_from,
                              valid_to, hash_sha256, metadata, created_by, updated_by,
                              created_at, updated_at
                    "#,
                )
                .bind(id)
                .bind(org_uuid)
                .bind(command.control_id)
                .bind(command.obligation_id)
                .bind(command.evidence_target_type.as_db_str())
                .bind(command.evidence_target_id.trim())
                .bind(command.source_audit_event_id)
                .bind(command.confidence.as_db_str())
                .bind(command.collected_at)
                .bind(command.collected_by.map(|id| *id.as_uuid()))
                .bind(command.valid_from)
                .bind(command.valid_to)
                .bind(command.hash_sha256.as_deref().map(str::trim))
                .bind(command.metadata)
                .bind(*command.actor.as_uuid())
                .bind(command.occurred_at)
                .fetch_one(tx.as_mut())
                .await?;
                let item = evidence_binding_from_row(&row)?;
                let event = compliance_audit_event(
                    "compliance.evidence_binding.create",
                    command.actor,
                    "compliance_evidence_bindings",
                    item.id,
                    command.trace,
                    command.occurred_at,
                    None,
                    Some(relation_audit_snapshot(&item)?),
                )?
                .with_org(org);
                Ok((item, vec![event]))
            })
        })
        .await
    }

    /// Accept exactly one proposed binding under the current tenant scope.
    ///
    /// Evidence identity and provenance are never rewritten. The locked
    /// PROPOSED → ACCEPTED transition records before/after snapshots in the
    /// same transaction; a replay or raced decision is a truthful conflict.
    pub async fn accept_evidence_binding(
        &self,
        command: AcceptEvidenceBindingCommand,
    ) -> Result<EvidenceBinding, PgComplianceError> {
        let org = current_org().map_err(KernelError::from)?;
        with_audits::<_, EvidenceBinding, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let before_row = sqlx::query(
                    r#"
                    SELECT id, control_id, obligation_id, evidence_target_type,
                           evidence_target_id, source_audit_event_id, status,
                           confidence, collected_at, collected_by, valid_from,
                           valid_to, hash_sha256, metadata, created_by, updated_by,
                           created_at, updated_at
                    FROM compliance_evidence_bindings
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(command.id)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("evidence binding was not found"))?;
                let before = evidence_binding_from_row(&before_row)?;
                if before.status != EvidenceBindingStatus::Proposed {
                    return Err(KernelError::conflict(
                        "only a proposed evidence binding may be accepted",
                    )
                    .into());
                }
                validate_evidence_status_transition(
                    before.status,
                    EvidenceBindingStatus::Accepted,
                )?;
                let after_row = sqlx::query(
                    r#"
                    UPDATE compliance_evidence_bindings
                    SET status = 'ACCEPTED', updated_by = $1, updated_at = $2
                    WHERE id = $3 AND status = 'PROPOSED'
                    RETURNING id, control_id, obligation_id, evidence_target_type,
                              evidence_target_id, source_audit_event_id, status,
                              confidence, collected_at, collected_by, valid_from,
                              valid_to, hash_sha256, metadata, created_by, updated_by,
                              created_at, updated_at
                    "#,
                )
                .bind(*command.actor.as_uuid())
                .bind(command.occurred_at)
                .bind(command.id)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| {
                    KernelError::conflict("evidence binding changed before it could be accepted")
                })?;
                let after = evidence_binding_from_row(&after_row)?;
                let event = compliance_audit_event(
                    "compliance.evidence_binding.accept",
                    command.actor,
                    "compliance_evidence_bindings",
                    after.id,
                    command.trace,
                    command.occurred_at,
                    Some(relation_audit_snapshot(&before)?),
                    Some(relation_audit_snapshot(&after)?),
                )?
                .with_org(org);
                Ok((after, vec![event]))
            })
        })
        .await
    }

    pub async fn list_evidence_bindings(
        &self,
        query: EvidenceBindingQuery,
    ) -> Result<EvidenceBindingPage, PgComplianceError> {
        let mut count_builder = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*)::BIGINT FROM compliance_evidence_bindings e WHERE TRUE",
        );
        push_evidence_filters(&mut count_builder, &query);
        let org = current_org().map_err(KernelError::from)?;
        let total: i64 = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(count_builder
                    .build_query_scalar()
                    .fetch_one(tx.as_mut())
                    .await?)
            })
        })
        .await?;
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, control_id, obligation_id, evidence_target_type,
                   evidence_target_id, source_audit_event_id, status,
                   confidence, collected_at, collected_by, valid_from,
                   valid_to, hash_sha256, metadata, created_by, updated_by,
                   created_at, updated_at
            FROM compliance_evidence_bindings e
            WHERE TRUE
            "#,
        );
        push_evidence_filters(&mut builder, &query);
        builder.push(" ORDER BY e.updated_at DESC, e.id DESC LIMIT ");
        builder.push_bind(query.page.limit);
        builder.push(" OFFSET ");
        builder.push_bind(query.page.offset);
        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        let items = rows
            .iter()
            .map(evidence_binding_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EvidenceBindingPage {
            items,
            limit: query.page.limit,
            offset: query.page.offset,
            total,
        })
    }

    async fn current_or_unrecorded(
        &self,
        user_id: UserId,
        branch_id: BranchId,
    ) -> Result<LocationConsent, PgComplianceError> {
        let user_uuid = *user_id.as_uuid();
        let org = current_org().map_err(KernelError::from)?;
        let row = with_org_conn::<_, _, PgComplianceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query!(
                    r#"
            SELECT id, user_id, branch_id, status,
                   granted_at, suspended_at, resumed_at, withdrawn_at, updated_at
            FROM location_consents
            WHERE user_id = $1
            "#,
                    user_uuid,
                )
                .fetch_optional(tx.as_mut())
                .await?)
            })
        })
        .await?;

        let Some(row) = row else {
            return Ok(LocationConsent::unrecorded(user_id, branch_id));
        };

        // Consent is per-user (UNIQUE (user_id)); the stored branch is the branch
        // the user consented in and is preserved as-is. A multi-branch user is not
        // rejected when the queried branch differs from the consent's branch.
        Ok(LocationConsent::from_persisted(PersistedLocationConsent {
            id: ConsentId::from_uuid(row.id),
            user_id: UserId::from_uuid(row.user_id),
            branch_id: BranchId::from_uuid(row.branch_id),
            state: LocationConsentState::from_db_str(&row.status)?,
            granted_at: row.granted_at,
            suspended_at: row.suspended_at,
            resumed_at: row.resumed_at,
            withdrawn_at: row.withdrawn_at,
            updated_at: row.updated_at,
        }))
    }
}

fn validate_create_regulation_impact(
    command: &CreateRegulationImpactCommand,
) -> Result<(), PgComplianceError> {
    validate_required_text("title", &command.title, 200)?;
    validate_required_text("jurisdiction", &command.jurisdiction, 80)?;
    validate_optional_text("regulator", command.regulator.as_deref(), 160)?;
    validate_required_text("citation", &command.citation, 300)?;
    validate_optional_text("source_url", command.source_url.as_deref(), 500)?;
    validate_required_text("impact_area", &command.impact_area, 80)?;
    validate_required_text("impact_summary", &command.impact_summary, 4_000)?;
    validate_date_range(
        "regulation impact",
        command.effective_from,
        command.effective_to,
    )?;
    validate_metadata_object(&command.metadata)?;
    Ok(())
}

fn validate_create_compliance_obligation(
    command: &CreateComplianceObligationCommand,
) -> Result<(), PgComplianceError> {
    validate_required_text("title", &command.title, 200)?;
    validate_required_text("description", &command.description, 4_000)?;
    command.scope.validate()?;
    validate_date_range(
        "compliance obligation",
        command.effective_from,
        command.effective_to,
    )?;
    validate_metadata_object(&command.metadata)?;
    for link in &command.regulation_links {
        validate_optional_text("rationale", link.rationale.as_deref(), 2_000)?;
    }
    Ok(())
}

fn validate_create_compliance_framework(
    command: &CreateComplianceFrameworkCommand,
) -> Result<(), PgComplianceError> {
    validate_required_text("name", &command.name, 200)?;
    validate_required_text("version_label", &command.version_label, 80)?;
    validate_date_range(
        "compliance framework",
        command.effective_from,
        command.effective_to,
    )?;
    validate_metadata_object(&command.metadata)?;
    Ok(())
}

fn validate_create_compliance_control(
    command: &CreateComplianceControlCommand,
) -> Result<(), PgComplianceError> {
    validate_control_key(&command.control_key)?;
    validate_required_text("title", &command.title, 200)?;
    validate_required_text("objective", &command.objective, 4_000)?;
    validate_evidence_requirements(&command.evidence_requirements)?;
    Ok(())
}

fn validate_create_evidence_binding(
    command: &CreateEvidenceBindingCommand,
) -> Result<(), PgComplianceError> {
    validate_required_text("evidence_target_id", &command.evidence_target_id, 200)?;
    validate_date_range("evidence binding", command.valid_from, command.valid_to)?;
    validate_hash_sha256(command.hash_sha256.as_deref())?;
    validate_metadata_object(&command.metadata)?;
    Ok(())
}

fn regulation_impact_from_row(row: &PgRow) -> Result<RegulationImpact, PgComplianceError> {
    let risk_level: String = row.try_get("risk_level")?;
    let status: String = row.try_get("status")?;
    Ok(RegulationImpact {
        id: row.try_get("id")?,
        code: row.try_get("code")?,
        title: row.try_get("title")?,
        jurisdiction: row.try_get("jurisdiction")?,
        regulator: row.try_get("regulator")?,
        citation: row.try_get("citation")?,
        source_url: row.try_get("source_url")?,
        impact_area: row.try_get("impact_area")?,
        impact_summary: row.try_get("impact_summary")?,
        risk_level: ComplianceRiskLevel::from_db_str(&risk_level)?,
        status: RegulationImpactStatus::from_db_str(&status)?,
        effective_from: row.try_get("effective_from")?,
        effective_to: row.try_get("effective_to")?,
        review_due_on: row.try_get("review_due_on")?,
        owner_user_id: optional_user_id(row, "owner_user_id")?,
        metadata: row.try_get("metadata")?,
        created_by: user_id(row, "created_by")?,
        updated_by: user_id(row, "updated_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn compliance_obligation_from_row(row: &PgRow) -> Result<ComplianceObligation, PgComplianceError> {
    let obligation_type: String = row.try_get("obligation_type")?;
    let scope_type: String = row.try_get("scope_type")?;
    let severity: String = row.try_get("severity")?;
    let status: String = row.try_get("status")?;
    let review_cadence: Option<String> = row.try_get("review_cadence")?;
    Ok(ComplianceObligation {
        id: row.try_get("id")?,
        code: row.try_get("code")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        obligation_type: ObligationType::from_db_str(&obligation_type)?,
        scope: ComplianceScope {
            kind: ComplianceScopeKind::from_db_str(&scope_type)?,
            scope_ref: row.try_get("scope_ref")?,
            branch_id: optional_branch_id(row, "branch_id")?,
            site_id: optional_site_id(row, "site_id")?,
        },
        owner_user_id: optional_user_id(row, "owner_user_id")?,
        severity: ComplianceRiskLevel::from_db_str(&severity)?,
        status: ObligationStatus::from_db_str(&status)?,
        effective_from: row.try_get("effective_from")?,
        effective_to: row.try_get("effective_to")?,
        review_cadence: review_cadence
            .as_deref()
            .map(ReviewCadence::from_db_str)
            .transpose()?,
        next_review_on: row.try_get("next_review_on")?,
        metadata: row.try_get("metadata")?,
        created_by: user_id(row, "created_by")?,
        updated_by: user_id(row, "updated_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn obligation_regulation_link_from_row(
    row: &PgRow,
) -> Result<ObligationRegulationLink, PgComplianceError> {
    let relationship: String = row.try_get("relationship")?;
    Ok(ObligationRegulationLink {
        id: row.try_get("id")?,
        obligation_id: row.try_get("obligation_id")?,
        regulation_impact_id: row.try_get("regulation_impact_id")?,
        relationship: ObligationRegulationRelationship::from_db_str(&relationship)?,
        rationale: row.try_get("rationale")?,
        created_by: user_id(row, "created_by")?,
        created_at: row.try_get("created_at")?,
    })
}

fn compliance_framework_from_row(row: &PgRow) -> Result<ComplianceFramework, PgComplianceError> {
    let framework_kind: String = row.try_get("framework_kind")?;
    let status: String = row.try_get("status")?;
    Ok(ComplianceFramework {
        id: row.try_get("id")?,
        code: row.try_get("code")?,
        name: row.try_get("name")?,
        version_label: row.try_get("version_label")?,
        framework_kind: FrameworkKind::from_db_str(&framework_kind)?,
        status: FrameworkStatus::from_db_str(&status)?,
        owner_user_id: optional_user_id(row, "owner_user_id")?,
        effective_from: row.try_get("effective_from")?,
        effective_to: row.try_get("effective_to")?,
        metadata: row.try_get("metadata")?,
        created_by: user_id(row, "created_by")?,
        updated_by: user_id(row, "updated_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn compliance_control_from_row(row: &PgRow) -> Result<ComplianceControl, PgComplianceError> {
    let control_type: String = row.try_get("control_type")?;
    let cadence: Option<String> = row.try_get("cadence")?;
    let status: String = row.try_get("status")?;
    Ok(ComplianceControl {
        id: row.try_get("id")?,
        framework_id: row.try_get("framework_id")?,
        control_key: row.try_get("control_key")?,
        title: row.try_get("title")?,
        objective: row.try_get("objective")?,
        control_type: ControlType::from_db_str(&control_type)?,
        cadence: cadence
            .as_deref()
            .map(ControlCadence::from_db_str)
            .transpose()?,
        status: ControlStatus::from_db_str(&status)?,
        evidence_requirements: row.try_get("evidence_requirements")?,
        owner_user_id: optional_user_id(row, "owner_user_id")?,
        created_by: user_id(row, "created_by")?,
        updated_by: user_id(row, "updated_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn control_obligation_coverage_from_row(
    row: &PgRow,
) -> Result<ControlObligationCoverage, PgComplianceError> {
    let coverage_level: String = row.try_get("coverage_level")?;
    let status: String = row.try_get("status")?;
    Ok(ControlObligationCoverage {
        id: row.try_get("id")?,
        control_id: row.try_get("control_id")?,
        obligation_id: row.try_get("obligation_id")?,
        coverage_level: CoverageLevel::from_db_str(&coverage_level)?,
        coverage_rationale: row.try_get("coverage_rationale")?,
        status: CoverageStatus::from_db_str(&status)?,
        created_by: user_id(row, "created_by")?,
        updated_by: user_id(row, "updated_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn evidence_binding_from_row(row: &PgRow) -> Result<EvidenceBinding, PgComplianceError> {
    let evidence_target_type: String = row.try_get("evidence_target_type")?;
    let status: String = row.try_get("status")?;
    let confidence: String = row.try_get("confidence")?;
    Ok(EvidenceBinding {
        id: row.try_get("id")?,
        control_id: row.try_get("control_id")?,
        obligation_id: row.try_get("obligation_id")?,
        evidence_target_type: EvidenceTargetType::from_db_str(&evidence_target_type)?,
        evidence_target_id: row.try_get("evidence_target_id")?,
        source_audit_event_id: row.try_get("source_audit_event_id")?,
        status: EvidenceBindingStatus::from_db_str(&status)?,
        confidence: EvidenceConfidence::from_db_str(&confidence)?,
        collected_at: row.try_get("collected_at")?,
        collected_by: optional_user_id(row, "collected_by")?,
        valid_from: row.try_get("valid_from")?,
        valid_to: row.try_get("valid_to")?,
        hash_sha256: row.try_get("hash_sha256")?,
        metadata: row.try_get("metadata")?,
        created_by: user_id(row, "created_by")?,
        updated_by: user_id(row, "updated_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn user_id(row: &PgRow, column: &str) -> Result<UserId, PgComplianceError> {
    Ok(UserId::from_uuid(row.try_get(column)?))
}

fn optional_user_id(row: &PgRow, column: &str) -> Result<Option<UserId>, PgComplianceError> {
    Ok(row
        .try_get::<Option<uuid::Uuid>, _>(column)?
        .map(UserId::from_uuid))
}

fn optional_branch_id(row: &PgRow, column: &str) -> Result<Option<BranchId>, PgComplianceError> {
    Ok(row
        .try_get::<Option<uuid::Uuid>, _>(column)?
        .map(BranchId::from_uuid))
}

fn optional_site_id(
    row: &PgRow,
    column: &str,
) -> Result<Option<console_kernel_core::SiteId>, PgComplianceError> {
    Ok(row
        .try_get::<Option<uuid::Uuid>, _>(column)?
        .map(console_kernel_core::SiteId::from_uuid))
}

fn push_regulation_filters(builder: &mut QueryBuilder<Postgres>, query: &RegulationImpactQuery) {
    if let Some(status) = query.status {
        builder.push(" AND r.status = ");
        builder.push_bind(status.as_db_str());
    }
    if let Some(risk_level) = query.risk_level {
        builder.push(" AND r.risk_level = ");
        builder.push_bind(risk_level.as_db_str());
    }
    if let Some(q) = normalized_like(&query.q) {
        builder.push(" AND (r.code ILIKE ");
        builder.push_bind(q.clone());
        builder.push(" OR r.title ILIKE ");
        builder.push_bind(q.clone());
        builder.push(" OR r.citation ILIKE ");
        builder.push_bind(q);
        builder.push(")");
    }
}

fn push_obligation_filters(
    builder: &mut QueryBuilder<Postgres>,
    query: &ComplianceObligationQuery,
) {
    push_obligation_scope_filter(builder, &query.branch_scope);
    if let Some(status) = query.status {
        builder.push(" AND o.status = ");
        builder.push_bind(status.as_db_str());
    }
    if let Some(severity) = query.severity {
        builder.push(" AND o.severity = ");
        builder.push_bind(severity.as_db_str());
    }
    if let Some(scope_type) = query.scope_type {
        builder.push(" AND o.scope_type = ");
        builder.push_bind(scope_type.as_db_str());
    }
    if let Some(branch_id) = query.branch_id {
        builder.push(" AND o.branch_id = ");
        builder.push_bind(*branch_id.as_uuid());
    }
    if let Some(site_id) = query.site_id {
        builder.push(" AND o.site_id = ");
        builder.push_bind(*site_id.as_uuid());
    }
    if let Some(q) = normalized_like(&query.q) {
        builder.push(" AND (o.code ILIKE ");
        builder.push_bind(q.clone());
        builder.push(" OR o.title ILIKE ");
        builder.push_bind(q.clone());
        builder.push(" OR o.description ILIKE ");
        builder.push_bind(q);
        builder.push(")");
    }
}

fn push_obligation_scope_filter(builder: &mut QueryBuilder<Postgres>, scope: &BranchScope) {
    match scope {
        BranchScope::All => {
            builder.push(" TRUE ");
        }
        BranchScope::Branches(branches) if branches.is_empty() => {
            builder.push(" FALSE ");
        }
        BranchScope::Branches(branches) => {
            let branch_ids: Vec<uuid::Uuid> =
                branches.iter().map(|branch| *branch.as_uuid()).collect();
            builder.push(" (o.scope_type = 'ORG' OR o.branch_id = ANY(");
            builder.push_bind(branch_ids);
            builder.push(")) ");
        }
    };
}

fn push_framework_filters(builder: &mut QueryBuilder<Postgres>, query: &ComplianceFrameworkQuery) {
    if let Some(status) = query.status {
        builder.push(" AND f.status = ");
        builder.push_bind(status.as_db_str());
    }
    if let Some(kind) = query.kind {
        builder.push(" AND f.framework_kind = ");
        builder.push_bind(kind.as_db_str());
    }
    if let Some(q) = normalized_like(&query.q) {
        builder.push(" AND (f.code ILIKE ");
        builder.push_bind(q.clone());
        builder.push(" OR f.name ILIKE ");
        builder.push_bind(q);
        builder.push(")");
    }
}

fn push_control_filters(builder: &mut QueryBuilder<Postgres>, query: &ComplianceControlQuery) {
    if let Some(status) = query.status {
        builder.push(" AND c.status = ");
        builder.push_bind(status.as_db_str());
    }
    if let Some(q) = normalized_like(&query.q) {
        builder.push(" AND (c.control_key ILIKE ");
        builder.push_bind(q.clone());
        builder.push(" OR c.title ILIKE ");
        builder.push_bind(q.clone());
        builder.push(" OR c.objective ILIKE ");
        builder.push_bind(q);
        builder.push(")");
    }
}

fn push_evidence_filters(builder: &mut QueryBuilder<Postgres>, query: &EvidenceBindingQuery) {
    if let Some(control_id) = query.control_id {
        builder.push(" AND e.control_id = ");
        builder.push_bind(control_id);
    }
    if let Some(obligation_id) = query.obligation_id {
        builder.push(" AND e.obligation_id = ");
        builder.push_bind(obligation_id);
    }
    if let Some(target_type) = query.target_type {
        builder.push(" AND e.evidence_target_type = ");
        builder.push_bind(target_type.as_db_str());
    }
    if let Some(status) = query.status {
        builder.push(" AND e.status = ");
        builder.push_bind(status.as_db_str());
    }
}

fn normalized_like(query: &Option<String>) -> Option<String> {
    query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{value}%"))
}

async fn fetch_labeled_audit_stream_events(
    tx: &mut Transaction<'_, Postgres>,
    stream_key: &str,
    limit: i64,
    offset: i64,
) -> Result<(Vec<AuditStreamRecord>, i64), PgComplianceError> {
    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM audit_stream_event_labels l
        JOIN audit_events e ON e.org_id = l.org_id AND e.id = l.audit_event_id
        WHERE l.stream_key = $1 AND l.sensitivity = $2
        "#,
    )
    .bind(stream_key)
    .bind(CEO_COVERT_AUDIT_SENSITIVITY)
    .fetch_one(tx.as_mut())
    .await?;

    let rows = sqlx::query(
        r#"
        SELECT e.id,
               e.actor,
               e.action,
               e.target_type,
               e.target_id,
               l.sensitivity,
               e.before_snap,
               e.after_snap,
               e.trace_id::TEXT AS trace_id,
               e.span_id::TEXT AS span_id,
               e.occurred_at,
               e.created_at
        FROM audit_stream_event_labels l
        JOIN audit_events e ON e.org_id = l.org_id AND e.id = l.audit_event_id
        WHERE l.stream_key = $1 AND l.sensitivity = $2
        ORDER BY e.occurred_at DESC, e.id DESC
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(stream_key)
    .bind(CEO_COVERT_AUDIT_SENSITIVITY)
    .bind(limit)
    .bind(offset)
    .fetch_all(tx.as_mut())
    .await?;

    rows.into_iter()
        .map(audit_stream_record_from_row)
        .collect::<Result<Vec<_>, _>>()
        .map(|items| (items, total))
}

async fn fetch_audit_stream_access_events(
    tx: &mut Transaction<'_, Postgres>,
    stream_key: &str,
    limit: i64,
    offset: i64,
) -> Result<(Vec<AuditStreamRecord>, i64), PgComplianceError> {
    let read_action = AuditStreamReadKind::Events.access_audit_action();
    let access_action = AuditStreamReadKind::AccessEvents.access_audit_action();
    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM audit_events e
        WHERE e.target_type = 'audit_stream'
          AND e.target_id = $1
          AND e.action IN ($2, $3)
        "#,
    )
    .bind(stream_key)
    .bind(read_action)
    .bind(access_action)
    .fetch_one(tx.as_mut())
    .await?;

    let rows = sqlx::query(
        r#"
        SELECT e.id,
               e.actor,
               e.action,
               e.target_type,
               e.target_id,
               'ACCESS'::TEXT AS sensitivity,
               e.before_snap,
               e.after_snap,
               e.trace_id::TEXT AS trace_id,
               e.span_id::TEXT AS span_id,
               e.occurred_at,
               e.created_at
        FROM audit_events e
        WHERE e.target_type = 'audit_stream'
          AND e.target_id = $1
          AND e.action IN ($2, $3)
        ORDER BY e.occurred_at DESC, e.id DESC
        LIMIT $4 OFFSET $5
        "#,
    )
    .bind(stream_key)
    .bind(read_action)
    .bind(access_action)
    .bind(limit)
    .bind(offset)
    .fetch_all(tx.as_mut())
    .await?;

    rows.into_iter()
        .map(audit_stream_record_from_row)
        .collect::<Result<Vec<_>, _>>()
        .map(|items| (items, total))
}

fn audit_stream_record_from_row(row: PgRow) -> Result<AuditStreamRecord, PgComplianceError> {
    let id: uuid::Uuid = row.try_get("id")?;
    let actor: Option<uuid::Uuid> = row.try_get("actor")?;
    Ok(AuditStreamRecord {
        id: id.to_string(),
        actor: actor.map(UserId::from_uuid),
        action: row.try_get("action")?,
        target_type: row.try_get("target_type")?,
        target_id: row.try_get("target_id")?,
        sensitivity: row.try_get("sensitivity")?,
        before_snap: row.try_get("before_snap")?,
        after_snap: row.try_get("after_snap")?,
        trace_id: row.try_get("trace_id")?,
        span_id: row.try_get("span_id")?,
        occurred_at: row.try_get("occurred_at")?,
        created_at: row.try_get("created_at")?,
    })
}

/// Derive arrival/departure events for one on-duty ping against the mechanic's
/// active work-order site geofences, inside the already-armed ping transaction.
///
/// For each active (non-terminal) work order assigned to the user whose site is
/// geocoded: load the prior inside/outside state (FOR UPDATE), evaluate the
/// haversine distance vs the site's effective radius (per-site override or the
/// 300 m default), and on an inside/outside EDGE upsert the presence row + append
/// a coordinate-free `site_attendance_events` row + an audit event. Edge-
/// triggered, so a steady stream of pings emits nothing.
///
/// The ping itself is audit-exempt, but these derived attendance writes ARE
/// audited (site.arrival / site.departure) — hence the marker + insert_audit_event.
// console-gate: state-changing-handler
async fn record_geofence_crossings(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    org_uuid: uuid::Uuid,
    user_uuid: uuid::Uuid,
    ping_latitude: f64,
    ping_longitude: f64,
    recorded_at: Timestamp,
) -> Result<(), PgComplianceError> {
    // The branch is taken from the WORK ORDER's site (w.branch_id), never the
    // ping's branch: a multi-branch mechanic may ping tagged to branch B while
    // assigned to a work order whose site belongs to branch A, and the durable
    // attendance fact + its audit must be filed under the owning branch A so the
    // branch-scoped arrival read attributes it correctly. Inactive/rejected work
    // orders are excluded so no attendance is derived for them.
    let candidates = sqlx::query!(
        r#"
        SELECT a.work_order_id     AS "work_order_id!",
               w.site_id           AS "site_id!",
               w.branch_id         AS "wo_branch_id!",
               s.latitude          AS "latitude!",
               s.longitude         AS "longitude!",
               s.geofence_radius_m AS "geofence_radius_m"
        FROM work_order_assignments a
        JOIN work_orders w    ON w.id = a.work_order_id
        JOIN registry_sites s ON s.id = w.site_id
        WHERE a.mechanic_id = $1
          AND w.status NOT IN ('FINAL_COMPLETED', 'CANCELLED', 'ARCHIVED', 'REJECTED')
          AND s.latitude IS NOT NULL
          AND s.longitude IS NOT NULL
        "#,
        user_uuid,
    )
    .fetch_all(tx.as_mut())
    .await?;

    for candidate in candidates {
        let work_order_id = candidate.work_order_id;
        let site_id = candidate.site_id;
        let wo_branch_id = candidate.wo_branch_id;
        let radius = candidate
            .geofence_radius_m
            .unwrap_or(DEFAULT_GEOFENCE_RADIUS_M);

        let prior = sqlx::query!(
            r#"
            SELECT inside, since FROM site_geofence_presence
            WHERE org_id = $1 AND user_id = $2 AND work_order_id = $3 AND site_id = $4
            FOR UPDATE
            "#,
            org_uuid,
            user_uuid,
            work_order_id,
            site_id,
        )
        .fetch_optional(tx.as_mut())
        .await?;

        // Edge detection assumes monotonic time, but recorded_at is a client
        // capture time and native apps flush offline-queued pings out of order.
        // A ping older than the last recorded transition must not flip the state
        // (it would emit a phantom crossing and move `since` backwards), so drop it.
        if let Some(ref row) = prior
            && recorded_at < row.since
        {
            continue;
        }
        let prior_inside = prior.as_ref().map(|row| row.inside);

        let (now_inside, crossing) = evaluate_geofence(
            ping_latitude,
            ping_longitude,
            candidate.latitude,
            candidate.longitude,
            radius,
            prior_inside,
        );

        if prior_inside.is_none() {
            // First sighting. ON CONFLICT DO NOTHING serializes concurrent
            // first-pings for the same (user, work order, site): the loser's
            // insert returns no row, so it yields the first-seen crossing to the
            // winner instead of duplicating the event or aborting the ping tx on
            // the unique constraint.
            let inserted = sqlx::query_scalar!(
                r#"
                INSERT INTO site_geofence_presence
                    (org_id, user_id, work_order_id, site_id, inside, since)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (org_id, user_id, work_order_id, site_id) DO NOTHING
                RETURNING inside
                "#,
                org_uuid,
                user_uuid,
                work_order_id,
                site_id,
                now_inside,
                recorded_at,
            )
            .fetch_optional(tx.as_mut())
            .await?;
            if inserted.is_none() {
                continue;
            }
        } else if crossing.is_some() {
            sqlx::query!(
                r#"
                UPDATE site_geofence_presence
                SET inside = $5, since = $6, updated_at = now()
                WHERE org_id = $1 AND user_id = $2 AND work_order_id = $3 AND site_id = $4
                "#,
                org_uuid,
                user_uuid,
                work_order_id,
                site_id,
                now_inside,
                recorded_at,
            )
            .execute(tx.as_mut())
            .await?;
        }

        if let Some(crossing) = crossing {
            let event_id = sqlx::query_scalar!(
                r#"
                INSERT INTO site_attendance_events
                    (org_id, user_id, branch_id, work_order_id, site_id, kind, occurred_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                RETURNING id
                "#,
                org_uuid,
                user_uuid,
                wo_branch_id,
                work_order_id,
                site_id,
                crossing.kind(),
                recorded_at,
            )
            .fetch_one(tx.as_mut())
            .await?;

            // Coordinate-free work fact (no lat/lon) so the durable record is not
            // location data subject to the consent-withdrawal erasure carve-out.
            let after = serde_json::json!({
                "work_order_id": work_order_id,
                "site_id": site_id,
                "kind": crossing.kind(),
                "occurred_at": recorded_at,
            });
            let event = AuditEvent::new(
                Some(UserId::from_uuid(user_uuid)),
                AuditAction::new(crossing.audit_action())?,
                "site_attendance_events",
                event_id.to_string(),
                TraceContext::generate(),
                recorded_at,
            )
            .with_branch(BranchId::from_uuid(wo_branch_id))
            .with_snapshots(None, Some(after))
            .with_org(OrgId::from_uuid(org_uuid));
            insert_audit_event(tx, &event).await?;
        }
    }

    Ok(())
}

fn push_location_consent_ledger_filters(
    builder: &mut QueryBuilder<Postgres>,
    branch_scope: &BranchScope,
    query: &LocationConsentLedgerQuery,
) {
    push_branch_scope_filter(builder, branch_scope);
    if let Some(branch_id) = query.branch_id {
        builder.push(" AND l.branch_id = ");
        builder.push_bind(*branch_id.as_uuid());
    }
    if let Some(user_id) = query.user_id {
        builder.push(" AND l.user_id = ");
        builder.push_bind(*user_id.as_uuid());
    }
}

fn push_arrival_event_filters(
    builder: &mut QueryBuilder<Postgres>,
    branch_scope: &BranchScope,
    query: &ArrivalEventQuery,
) {
    push_branch_scope_filter(builder, branch_scope);
    if let Some(branch_id) = query.branch_id {
        builder.push(" AND l.branch_id = ");
        builder.push_bind(*branch_id.as_uuid());
    }
    if let Some(user_id) = query.user_id {
        builder.push(" AND l.user_id = ");
        builder.push_bind(*user_id.as_uuid());
    }
}

fn push_branch_scope_filter(builder: &mut QueryBuilder<Postgres>, branch_scope: &BranchScope) {
    match branch_scope {
        BranchScope::All => {
            builder.push(" TRUE ");
        }
        BranchScope::Branches(branches) if branches.is_empty() => {
            builder.push(" FALSE ");
        }
        BranchScope::Branches(branches) => {
            let branch_ids: Vec<uuid::Uuid> =
                branches.iter().map(|branch| *branch.as_uuid()).collect();
            builder.push(" l.branch_id = ANY(");
            builder.push_bind(branch_ids);
            builder.push(") ");
        }
    }
}
