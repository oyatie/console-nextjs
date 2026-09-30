//! Tenant-armed persistence for the bounded equipment 3R pilot.
//!
//! All mutations use `with_audits`: FSM transitions, history rows, and the
//! audit chain commit together.  Every transition locks its aggregate
//! (`FOR UPDATE`) and applies a status-guarded `UPDATE`; `rows_affected != 1`
//! is a conflict, never a silent no-op.  Transition authorization runs inside
//! the transaction against the branch read from the locked row, via the
//! caller-supplied `authorize` closure.
use console_equipment_application::{
    AssessReturn, CompleteDisposition, DecideApproval, DispatchCase, HandoverCase, InspectCase,
    QuoteCase, RegisterUnit,
};
use console_equipment_domain::{Availability, CaseState, DispositionKind, DispositionState};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, ErrorKind, KernelError, OrgId, TraceContext, UserId,
};
use console_platform_db::{DbError, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PgEquipment3rError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Domain(#[from] KernelError),
}
impl From<sqlx::Error> for PgEquipment3rError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}
impl PgEquipment3rError {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(e) => e.kind,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Db(DbError::Sqlx(sqlx::Error::Database(e)))
                if e.code().as_deref() == Some("23505") =>
            {
                ErrorKind::Conflict
            }
            _ => ErrorKind::Internal,
        }
    }
}

/// Branch authorization performed inside the mutation transaction, against
/// the branch of the locked row.
pub type BranchAuthorization = Box<dyn FnOnce(BranchId) -> Result<(), KernelError> + Send>;

#[derive(Debug, Clone)]
pub struct PgEquipment3rStore {
    pool: PgPool,
}

impl PgEquipment3rStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn register_unit(
        &self,
        actor: UserId,
        cmd: RegisterUnit,
    ) -> Result<Value, PgEquipment3rError> {
        required(&cmd.serial_no, "serialNo", 80)?;
        required(&cmd.model_name, "modelName", 120)?;
        required(&cmd.capacity_class, "capacityClass", 40)?;
        if cmd.acquisition_cost_minor < 0 {
            return Err(
                KernelError::validation("acquisitionCostMinor must be non-negative").into(),
            );
        }
        let org = current_org().map_err(KernelError::from)?;
        let id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                // An org-wide principal passes the JWT branch-scope check for
                // any branch id, so existence is validated here under the
                // armed org: a cross-org or unknown branch is a concealed 404,
                // never a foreign-key 500.
                sqlx::query_scalar::<_, i32>("SELECT 1 FROM branches WHERE id=$1")
                    .bind(*cmd.branch_id.as_uuid())
                    .fetch_optional(tx.as_mut())
                    .await?
                    .ok_or_else(|| KernelError::not_found("branch was not found"))?;
                sqlx::query(
                    "INSERT INTO equipment_3r_units (id,org_id,branch_id,serial_no,model_name,capacity_class,acquisition_cost_minor,created_by,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9)",
                )
                .bind(id)
                .bind(*org.as_uuid())
                .bind(*cmd.branch_id.as_uuid())
                .bind(cmd.serial_no.trim())
                .bind(cmd.model_name.trim())
                .bind(cmd.capacity_class.trim())
                .bind(cmd.acquisition_cost_minor)
                .bind(*actor.as_uuid())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                history(tx, org, cmd.branch_id, "unit", id, Availability::Available.as_db(), actor, now).await?;
                let view = json!({
                    "id": id,
                    "serialNo": cmd.serial_no.trim(),
                    "modelName": cmd.model_name.trim(),
                    "capacityClass": cmd.capacity_class.trim(),
                    "availability": Availability::Available.as_db(),
                    "acquisitionCostMinor": cmd.acquisition_cost_minor,
                    "branchId": cmd.branch_id,
                });
                Ok((
                    view,
                    vec![audit(org, actor, cmd.branch_id, "equipment_3r.unit.register", "equipment_3r_unit", id, now)?],
                ))
            })
        })
        .await
    }

    pub async fn list_units(&self) -> Result<Value, PgEquipment3rError> {
        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgEquipment3rError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    "SELECT id,serial_no,model_name,capacity_class,availability,acquisition_cost_minor,branch_id FROM equipment_3r_units ORDER BY created_at DESC, id LIMIT 200",
                )
                .fetch_all(tx.as_mut())
                .await
                .map_err(Into::into)
            })
        })
        .await?;
        let units = rows
            .iter()
            .map(unit_view_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Value::Array(units))
    }

    pub async fn unit_detail(&self, unit: Uuid) -> Result<(Value, BranchId), PgEquipment3rError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgEquipment3rError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT u.id,u.serial_no,u.model_name,u.capacity_class,u.availability,u.acquisition_cost_minor,u.branch_id,u.created_at,u.updated_at, \
                     (SELECT c.id FROM equipment_3r_rental_cases c WHERE c.unit_id=u.id AND c.status IN ('APPROVED','DISPATCHED','HANDED_OVER','RETURNED')) AS active_case_id, \
                     (SELECT d.id FROM equipment_3r_dispositions d WHERE d.unit_id=u.id AND d.status='OPEN') AS open_disposition_id \
                     FROM equipment_3r_units u WHERE u.id=$1",
                )
                .bind(unit)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("equipment unit was not found"))?;
                let branch = BranchId::from_uuid(row.try_get("branch_id")?);
                let mut view = match unit_view_row(&row)? {
                    Value::Object(map) => map,
                    _ => Map::new(),
                };
                view.insert(
                    "activeCaseId".into(),
                    json!(row.try_get::<Option<Uuid>, _>("active_case_id")?),
                );
                view.insert(
                    "openDispositionId".into(),
                    json!(row.try_get::<Option<Uuid>, _>("open_disposition_id")?),
                );
                view.insert(
                    "createdAt".into(),
                    Value::String(rfc3339(row.try_get("created_at")?)?),
                );
                view.insert(
                    "updatedAt".into(),
                    Value::String(rfc3339(row.try_get("updated_at")?)?),
                );
                Ok((Value::Object(view), branch))
            })
        })
        .await
    }

    pub async fn unit_history(&self, unit: Uuid) -> Result<(Value, BranchId), PgEquipment3rError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgEquipment3rError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let branch: Uuid =
                    sqlx::query_scalar("SELECT branch_id FROM equipment_3r_units WHERE id=$1")
                        .bind(unit)
                        .fetch_optional(tx.as_mut())
                        .await?
                        .ok_or_else(|| KernelError::not_found("equipment unit was not found"))?;
                let rows = sqlx::query(
                    "SELECT aggregate_kind,aggregate_id,transition,actor_id,occurred_at FROM equipment_3r_history \
                     WHERE aggregate_id=$1 \
                        OR aggregate_id IN (SELECT id FROM equipment_3r_rental_cases WHERE unit_id=$1) \
                        OR aggregate_id IN (SELECT id FROM equipment_3r_dispositions WHERE unit_id=$1) \
                     ORDER BY occurred_at DESC, id LIMIT 200",
                )
                .bind(unit)
                .fetch_all(tx.as_mut())
                .await?;
                let mut entries = Vec::with_capacity(rows.len());
                for row in &rows {
                    entries.push(json!({
                        "aggregateKind": row.try_get::<String, _>("aggregate_kind")?,
                        "aggregateId": row.try_get::<Uuid, _>("aggregate_id")?,
                        "transition": row.try_get::<String, _>("transition")?,
                        "actorId": row.try_get::<Uuid, _>("actor_id")?,
                        "occurredAt": rfc3339(row.try_get("occurred_at")?)?,
                    }));
                }
                Ok((Value::Array(entries), BranchId::from_uuid(branch)))
            })
        })
        .await
    }

    /// Idempotent quote creation.  Returns `(replayed, view)`; a replay of the
    /// same key + fingerprint returns the stored case, a different fingerprint
    /// conflicts, and quoting never reserves the unit.
    pub async fn quote_case(
        &self,
        actor: UserId,
        cmd: QuoteCase,
        idempotency_key: String,
        fingerprint_input: &Value,
    ) -> Result<(bool, Value), PgEquipment3rError> {
        idem(&idempotency_key)?;
        required(&cmd.customer_name, "customerName", 160)?;
        required(&cmd.site_reference, "siteReference", 200)?;
        if cmd.monthly_rate_minor <= 0 {
            return Err(KernelError::validation("monthlyRateMinor must be positive").into());
        }
        if !(1..=120).contains(&cmd.duration_months) {
            return Err(KernelError::validation("durationMonths must be within 1..120").into());
        }
        if cmd.currency_code != "KRW" {
            return Err(KernelError::validation("currencyCode must be KRW").into());
        }
        let org = current_org().map_err(KernelError::from)?;
        let id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        let fp = fingerprint(fingerprint_input);
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                if let Some(row) = sqlx::query(
                    "SELECT id,unit_id,status,customer_name,site_reference,monthly_rate_minor,duration_months,currency_code,branch_id,request_fingerprint FROM equipment_3r_rental_cases WHERE idempotency_key=$1",
                )
                .bind(&idempotency_key)
                .fetch_optional(tx.as_mut())
                .await?
                {
                    let prior: String = row.try_get("request_fingerprint")?;
                    if prior != fp {
                        return Err(KernelError::conflict(
                            "idempotency key was reused with a different request",
                        )
                        .into());
                    }
                    let mut view = case_view_map(&row)?;
                    view.insert("replayed".into(), Value::Bool(true));
                    return Ok(((true, Value::Object(view)), vec![]));
                }
                let unit_row = sqlx::query(
                    "SELECT availability FROM equipment_3r_units WHERE id=$1 AND branch_id=$2 FOR UPDATE",
                )
                .bind(cmd.unit_id)
                .bind(*cmd.branch_id.as_uuid())
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("equipment unit was not found in branch"))?;
                let availability =
                    Availability::from_db(&unit_row.try_get::<String, _>("availability")?)?;
                if availability == Availability::Sold {
                    return Err(KernelError::conflict("sold unit cannot be quoted").into());
                }
                sqlx::query(
                    "INSERT INTO equipment_3r_rental_cases (id,org_id,branch_id,unit_id,customer_name,site_reference,monthly_rate_minor,duration_months,currency_code,idempotency_key,request_fingerprint,created_by,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$13)",
                )
                .bind(id)
                .bind(*org.as_uuid())
                .bind(*cmd.branch_id.as_uuid())
                .bind(cmd.unit_id)
                .bind(cmd.customer_name.trim())
                .bind(cmd.site_reference.trim())
                .bind(cmd.monthly_rate_minor)
                .bind(cmd.duration_months)
                .bind(&cmd.currency_code)
                .bind(&idempotency_key)
                .bind(&fp)
                .bind(*actor.as_uuid())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                history(tx, org, cmd.branch_id, "case", id, CaseState::Quoted.as_db(), actor, now).await?;
                let view = json!({
                    "id": id,
                    "unitId": cmd.unit_id,
                    "status": CaseState::Quoted.as_db(),
                    "customerName": cmd.customer_name.trim(),
                    "siteReference": cmd.site_reference.trim(),
                    "monthlyRateMinor": cmd.monthly_rate_minor,
                    "durationMonths": cmd.duration_months,
                    "currencyCode": cmd.currency_code,
                    "branchId": cmd.branch_id,
                });
                Ok((
                    (false, view),
                    vec![audit(org, actor, cmd.branch_id, "equipment_3r.case.quote", "equipment_3r_case", id, now)?],
                ))
            })
        })
        .await
    }

    pub async fn list_cases(&self) -> Result<Value, PgEquipment3rError> {
        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgEquipment3rError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    "SELECT id,unit_id,status,customer_name,site_reference,monthly_rate_minor,duration_months,currency_code,branch_id FROM equipment_3r_rental_cases ORDER BY created_at DESC, id LIMIT 200",
                )
                .fetch_all(tx.as_mut())
                .await
                .map_err(Into::into)
            })
        })
        .await?;
        let cases = rows
            .iter()
            .map(|row| case_view_map(row).map(Value::Object))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Value::Array(cases))
    }

    pub async fn case_detail(&self, case: Uuid) -> Result<(Value, BranchId), PgEquipment3rError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgEquipment3rError>(&self.pool, org, move |tx| {
            Box::pin(async move { case_detail_tx(tx, case).await })
        })
        .await
    }

    /// Four-eyes approval decision.  `APPROVED` reserves the unit with a
    /// single-winner guarded update.
    pub async fn decide_approval(
        &self,
        actor: UserId,
        case: Uuid,
        cmd: DecideApproval,
        authorize: BranchAuthorization,
    ) -> Result<Value, PgEquipment3rError> {
        let target = match cmd.decision.as_str() {
            "APPROVED" => CaseState::Approved,
            "DECLINED" => CaseState::Declined,
            _ => {
                return Err(
                    KernelError::validation("decision must be APPROVED or DECLINED").into(),
                );
            }
        };
        match (&cmd.reason, target) {
            (Some(reason), CaseState::Declined) => required(reason, "reason", 500)?,
            (None, CaseState::Declined) => {
                return Err(KernelError::validation("reason is required to decline").into());
            }
            (Some(_), _) => {
                return Err(
                    KernelError::validation("reason accompanies only a DECLINED decision").into(),
                );
            }
            (None, _) => {}
        }
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = lock_case(tx, case).await?;
                let branch = BranchId::from_uuid(row.try_get("branch_id")?);
                authorize(branch).map_err(PgEquipment3rError::Domain)?;
                let state = CaseState::from_db(&row.try_get::<String, _>("status")?)?;
                state.can_transition_to(target)?;
                let created_by: Uuid = row.try_get("created_by")?;
                if created_by == *actor.as_uuid() {
                    return Err(KernelError::forbidden(
                        "four-eyes approval requires an approver other than the case creator",
                    )
                    .into());
                }
                let unit: Uuid = row.try_get("unit_id")?;
                if target == CaseState::Approved {
                    let reserved = sqlx::query(
                        "UPDATE equipment_3r_units SET availability=$1, updated_at=$2 WHERE id=$3 AND availability=$4",
                    )
                    .bind(Availability::Reserved.as_db())
                    .bind(now)
                    .bind(unit)
                    .bind(Availability::Available.as_db())
                    .execute(tx.as_mut())
                    .await?
                    .rows_affected();
                    if reserved != 1 {
                        return Err(KernelError::conflict(
                            "unit is not available for reservation",
                        )
                        .into());
                    }
                    history(tx, org, branch, "unit", unit, Availability::Reserved.as_db(), actor, now).await?;
                }
                let updated = sqlx::query(
                    "UPDATE equipment_3r_rental_cases SET status=$1, approval_decision=$2, approval_reason=$3, approved_by=$4, approved_at=$5, updated_at=$5 WHERE id=$6 AND status=$7",
                )
                .bind(target.as_db())
                .bind(&cmd.decision)
                .bind(cmd.reason.as_deref().map(str::trim))
                .bind(*actor.as_uuid())
                .bind(now)
                .bind(case)
                .bind(CaseState::Quoted.as_db())
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                if updated != 1 {
                    return Err(KernelError::conflict("rental case left the quoted state").into());
                }
                history(tx, org, branch, "case", case, target.as_db(), actor, now).await?;
                let view = case_view_tx(tx, case).await?;
                Ok((
                    view,
                    vec![audit(org, actor, branch, "equipment_3r.case.approval", "equipment_3r_case", case, now)?],
                ))
            })
        })
        .await
    }

    pub async fn dispatch_case(
        &self,
        actor: UserId,
        case: Uuid,
        cmd: DispatchCase,
        authorize: BranchAuthorization,
    ) -> Result<Value, PgEquipment3rError> {
        required(&cmd.carrier_name, "carrierName", 120)?;
        required(&cmd.vehicle_reference, "vehicleReference", 120)?;
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = lock_case(tx, case).await?;
                let branch = BranchId::from_uuid(row.try_get("branch_id")?);
                authorize(branch).map_err(PgEquipment3rError::Domain)?;
                let state = CaseState::from_db(&row.try_get::<String, _>("status")?)?;
                state.can_transition_to(CaseState::Dispatched)?;
                let updated = sqlx::query(
                    "UPDATE equipment_3r_rental_cases SET status=$1, carrier_name=$2, vehicle_reference=$3, dispatched_at=$4, updated_at=$4 WHERE id=$5 AND status=$6",
                )
                .bind(CaseState::Dispatched.as_db())
                .bind(cmd.carrier_name.trim())
                .bind(cmd.vehicle_reference.trim())
                .bind(now)
                .bind(case)
                .bind(CaseState::Approved.as_db())
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                if updated != 1 {
                    return Err(
                        KernelError::conflict("rental case left the approved state").into(),
                    );
                }
                history(tx, org, branch, "case", case, CaseState::Dispatched.as_db(), actor, now).await?;
                let view = case_view_tx(tx, case).await?;
                Ok((
                    view,
                    vec![audit(org, actor, branch, "equipment_3r.case.dispatch", "equipment_3r_case", case, now)?],
                ))
            })
        })
        .await
    }

    pub async fn handover_case(
        &self,
        actor: UserId,
        case: Uuid,
        cmd: HandoverCase,
        authorize: BranchAuthorization,
    ) -> Result<Value, PgEquipment3rError> {
        required(&cmd.recipient_name, "recipientName", 160)?;
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = lock_case(tx, case).await?;
                let branch = BranchId::from_uuid(row.try_get("branch_id")?);
                authorize(branch).map_err(PgEquipment3rError::Domain)?;
                let state = CaseState::from_db(&row.try_get::<String, _>("status")?)?;
                state.can_transition_to(CaseState::HandedOver)?;
                let unit: Uuid = row.try_get("unit_id")?;
                bind_handover_custody(
                    tx,
                    org,
                    branch,
                    case,
                    cmd.evidence_object_id,
                    actor,
                )
                .await?;
                let moved = sqlx::query(
                    "UPDATE equipment_3r_units SET availability=$1, updated_at=$2 WHERE id=$3 AND availability=$4",
                )
                .bind(Availability::OnRent.as_db())
                .bind(now)
                .bind(unit)
                .bind(Availability::Reserved.as_db())
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                if moved != 1 {
                    return Err(KernelError::conflict("unit is not reserved for handover").into());
                }
                let updated = sqlx::query(
                    "UPDATE equipment_3r_rental_cases SET status=$1, recipient_name=$2, handover_evidence_object_id=$3, handed_over_at=$4, updated_at=$5 WHERE id=$6 AND status=$7",
                )
                .bind(CaseState::HandedOver.as_db())
                .bind(cmd.recipient_name.trim())
                .bind(cmd.evidence_object_id)
                .bind(cmd.handed_over_at)
                .bind(now)
                .bind(case)
                .bind(CaseState::Dispatched.as_db())
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                if updated != 1 {
                    return Err(
                        KernelError::conflict("rental case left the dispatched state").into(),
                    );
                }
                history(tx, org, branch, "unit", unit, Availability::OnRent.as_db(), actor, now).await?;
                history(tx, org, branch, "case", case, CaseState::HandedOver.as_db(), actor, now).await?;
                let view = case_view_tx(tx, case).await?;
                Ok((
                    view,
                    vec![audit(org, actor, branch, "equipment_3r.case.handover", "equipment_3r_case", case, now)?],
                ))
            })
        })
        .await
    }

    /// Append-only inspection record; the case must currently be on rent.
    pub async fn inspect_case(
        &self,
        actor: UserId,
        case: Uuid,
        cmd: InspectCase,
        authorize: BranchAuthorization,
    ) -> Result<Value, PgEquipment3rError> {
        if cmd.outcome != "PASS" && cmd.outcome != "MAINTENANCE_PERFORMED" {
            return Err(
                KernelError::validation("outcome must be PASS or MAINTENANCE_PERFORMED").into(),
            );
        }
        required(&cmd.findings, "findings", 2000)?;
        match (&cmd.maintenance_note, cmd.outcome.as_str()) {
            (Some(note), "MAINTENANCE_PERFORMED") => required(note, "maintenanceNote", 2000)?,
            (None, "MAINTENANCE_PERFORMED") => {
                return Err(KernelError::validation(
                    "maintenanceNote is required when maintenance was performed",
                )
                .into());
            }
            (Some(_), _) => {
                return Err(KernelError::validation(
                    "maintenanceNote accompanies only MAINTENANCE_PERFORMED",
                )
                .into());
            }
            (None, _) => {}
        }
        let org = current_org().map_err(KernelError::from)?;
        let id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = lock_case(tx, case).await?;
                let branch = BranchId::from_uuid(row.try_get("branch_id")?);
                authorize(branch).map_err(PgEquipment3rError::Domain)?;
                let state = CaseState::from_db(&row.try_get::<String, _>("status")?)?;
                if state != CaseState::HandedOver {
                    return Err(KernelError::conflict(
                        "inspections are recorded only while the unit is handed over",
                    )
                    .into());
                }
                sqlx::query(
                    "INSERT INTO equipment_3r_inspections (id,org_id,branch_id,case_id,outcome,findings,maintenance_note,inspected_by,inspected_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
                )
                .bind(id)
                .bind(*org.as_uuid())
                .bind(*branch.as_uuid())
                .bind(case)
                .bind(&cmd.outcome)
                .bind(cmd.findings.trim())
                .bind(cmd.maintenance_note.as_deref().map(str::trim))
                .bind(*actor.as_uuid())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                let view = json!({
                    "id": id,
                    "caseId": case,
                    "outcome": cmd.outcome,
                    "findings": cmd.findings.trim(),
                    "maintenanceNote": cmd.maintenance_note.as_deref().map(str::trim),
                    "inspectedBy": actor,
                    "inspectedAt": rfc3339(now)?,
                });
                Ok((
                    view,
                    vec![audit(org, actor, branch, "equipment_3r.case.inspect", "equipment_3r_case", case, now)?],
                ))
            })
        })
        .await
    }

    pub async fn return_case(
        &self,
        actor: UserId,
        case: Uuid,
        returned_at: OffsetDateTime,
        authorize: BranchAuthorization,
    ) -> Result<Value, PgEquipment3rError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = lock_case(tx, case).await?;
                let branch = BranchId::from_uuid(row.try_get("branch_id")?);
                authorize(branch).map_err(PgEquipment3rError::Domain)?;
                let state = CaseState::from_db(&row.try_get::<String, _>("status")?)?;
                state.can_transition_to(CaseState::Returned)?;
                let unit: Uuid = row.try_get("unit_id")?;
                let moved = sqlx::query(
                    "UPDATE equipment_3r_units SET availability=$1, updated_at=$2 WHERE id=$3 AND availability=$4",
                )
                .bind(Availability::InAssessment.as_db())
                .bind(now)
                .bind(unit)
                .bind(Availability::OnRent.as_db())
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                if moved != 1 {
                    return Err(KernelError::conflict("unit is not on rent").into());
                }
                let updated = sqlx::query(
                    "UPDATE equipment_3r_rental_cases SET status=$1, returned_at=$2, updated_at=$3 WHERE id=$4 AND status=$5",
                )
                .bind(CaseState::Returned.as_db())
                .bind(returned_at)
                .bind(now)
                .bind(case)
                .bind(CaseState::HandedOver.as_db())
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                if updated != 1 {
                    return Err(
                        KernelError::conflict("rental case left the handed-over state").into(),
                    );
                }
                history(tx, org, branch, "unit", unit, Availability::InAssessment.as_db(), actor, now).await?;
                history(tx, org, branch, "case", case, CaseState::Returned.as_db(), actor, now).await?;
                let view = case_view_tx(tx, case).await?;
                Ok((
                    view,
                    vec![audit(org, actor, branch, "equipment_3r.case.return", "equipment_3r_case", case, now)?],
                ))
            })
        })
        .await
    }

    /// Post the single return assessment: closes the case, moves the unit to
    /// the disposition branch, and opens the disposition record (REDEPLOY is
    /// inserted already completed with zero cost for a truthful trail).
    pub async fn assess_return(
        &self,
        actor: UserId,
        case: Uuid,
        cmd: AssessReturn,
        authorize: BranchAuthorization,
    ) -> Result<Value, PgEquipment3rError> {
        if !matches!(cmd.condition_grade.as_str(), "A" | "B" | "C" | "D") {
            return Err(KernelError::validation("conditionGrade must be A, B, C, or D").into());
        }
        required(&cmd.findings, "findings", 2000)?;
        let kind = DispositionKind::from_db(&cmd.disposition).map_err(|_| {
            KernelError::validation("disposition must be REPAIR, REFURBISH, RESALE, or REDEPLOY")
        })?;
        let org = current_org().map_err(KernelError::from)?;
        let assessment = Uuid::new_v4();
        let disposition = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = lock_case(tx, case).await?;
                let branch = BranchId::from_uuid(row.try_get("branch_id")?);
                authorize(branch).map_err(PgEquipment3rError::Domain)?;
                let state = CaseState::from_db(&row.try_get::<String, _>("status")?)?;
                state.can_transition_to(CaseState::Closed)?;
                let unit: Uuid = row.try_get("unit_id")?;
                sqlx::query(
                    "INSERT INTO equipment_3r_return_assessments (id,org_id,branch_id,case_id,condition_grade,findings,disposition,assessed_by,assessed_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
                )
                .bind(assessment)
                .bind(*org.as_uuid())
                .bind(*branch.as_uuid())
                .bind(case)
                .bind(&cmd.condition_grade)
                .bind(cmd.findings.trim())
                .bind(kind.as_db())
                .bind(*actor.as_uuid())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                let target = kind.assessment_target();
                let moved = sqlx::query(
                    "UPDATE equipment_3r_units SET availability=$1, updated_at=$2 WHERE id=$3 AND availability=$4",
                )
                .bind(target.as_db())
                .bind(now)
                .bind(unit)
                .bind(Availability::InAssessment.as_db())
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                if moved != 1 {
                    return Err(KernelError::conflict("unit is not in assessment").into());
                }
                let updated = sqlx::query(
                    "UPDATE equipment_3r_rental_cases SET status=$1, updated_at=$2 WHERE id=$3 AND status=$4",
                )
                .bind(CaseState::Closed.as_db())
                .bind(now)
                .bind(case)
                .bind(CaseState::Returned.as_db())
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                if updated != 1 {
                    return Err(
                        KernelError::conflict("rental case left the returned state").into(),
                    );
                }
                if kind == DispositionKind::Redeploy {
                    sqlx::query(
                        "INSERT INTO equipment_3r_dispositions (id,org_id,branch_id,unit_id,case_id,assessment_id,kind,status,cost_minor,completed_by,completed_at,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,0,$9,$10,$10,$10)",
                    )
                    .bind(disposition)
                    .bind(*org.as_uuid())
                    .bind(*branch.as_uuid())
                    .bind(unit)
                    .bind(case)
                    .bind(assessment)
                    .bind(kind.as_db())
                    .bind(DispositionState::Completed.as_db())
                    .bind(*actor.as_uuid())
                    .bind(now)
                    .execute(tx.as_mut())
                    .await?;
                    history(tx, org, branch, "disposition", disposition, DispositionState::Completed.as_db(), actor, now).await?;
                } else {
                    sqlx::query(
                        "INSERT INTO equipment_3r_dispositions (id,org_id,branch_id,unit_id,case_id,assessment_id,kind,status,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9)",
                    )
                    .bind(disposition)
                    .bind(*org.as_uuid())
                    .bind(*branch.as_uuid())
                    .bind(unit)
                    .bind(case)
                    .bind(assessment)
                    .bind(kind.as_db())
                    .bind(DispositionState::Open.as_db())
                    .bind(now)
                    .execute(tx.as_mut())
                    .await?;
                    history(tx, org, branch, "disposition", disposition, DispositionState::Open.as_db(), actor, now).await?;
                }
                history(tx, org, branch, "unit", unit, target.as_db(), actor, now).await?;
                history(tx, org, branch, "case", case, CaseState::Closed.as_db(), actor, now).await?;
                let (view, _) = case_detail_tx(tx, case).await?;
                Ok((
                    view,
                    vec![audit(org, actor, branch, "equipment_3r.case.assess", "equipment_3r_case", case, now)?],
                ))
            })
        })
        .await
    }

    pub async fn complete_disposition(
        &self,
        actor: UserId,
        disposition: Uuid,
        cmd: CompleteDisposition,
        authorize: BranchAuthorization,
    ) -> Result<Value, PgEquipment3rError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT branch_id,unit_id,case_id,kind,status FROM equipment_3r_dispositions WHERE id=$1 FOR UPDATE",
                )
                .bind(disposition)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("disposition was not found"))?;
                let branch = BranchId::from_uuid(row.try_get("branch_id")?);
                authorize(branch).map_err(PgEquipment3rError::Domain)?;
                let state = DispositionState::from_db(&row.try_get::<String, _>("status")?)?;
                state.can_transition_to(DispositionState::Completed)?;
                let kind = DispositionKind::from_db(&row.try_get::<String, _>("kind")?)?;
                let unit: Uuid = row.try_get("unit_id")?;
                let case: Uuid = row.try_get("case_id")?;
                let (cost, sale, buyer) = match kind {
                    DispositionKind::Repair | DispositionKind::Refurbish => {
                        let cost = cmd.cost_minor.ok_or_else(|| {
                            KernelError::validation("costMinor is required to complete this disposition")
                        })?;
                        if cost < 0 {
                            return Err(
                                KernelError::validation("costMinor must be non-negative").into()
                            );
                        }
                        if cmd.sale_amount_minor.is_some() || cmd.buyer_name.is_some() {
                            return Err(KernelError::validation(
                                "sale fields accompany only a RESALE disposition",
                            )
                            .into());
                        }
                        (Some(cost), None, None)
                    }
                    DispositionKind::Resale => {
                        let sale = cmd.sale_amount_minor.ok_or_else(|| {
                            KernelError::validation("saleAmountMinor is required to complete a resale")
                        })?;
                        if sale < 0 {
                            return Err(KernelError::validation(
                                "saleAmountMinor must be non-negative",
                            )
                            .into());
                        }
                        let buyer = cmd.buyer_name.clone().ok_or_else(|| {
                            KernelError::validation("buyerName is required to complete a resale")
                        })?;
                        required(&buyer, "buyerName", 160)?;
                        if cmd.cost_minor.is_some() {
                            return Err(KernelError::validation(
                                "costMinor accompanies only REPAIR or REFURBISH dispositions",
                            )
                            .into());
                        }
                        (None, Some(sale), Some(buyer.trim().to_owned()))
                    }
                    DispositionKind::Redeploy => {
                        // Unreachable through the FSM: REDEPLOY rows are
                        // inserted COMPLETED, so the state guard above already
                        // conflicted. Kept as an explicit fail-closed arm.
                        return Err(KernelError::conflict(
                            "redeploy dispositions complete at assessment time",
                        )
                        .into());
                    }
                };
                let target = kind.completion_target().ok_or_else(|| {
                    KernelError::conflict("redeploy dispositions complete at assessment time")
                })?;
                let moved = sqlx::query(
                    "UPDATE equipment_3r_units SET availability=$1, updated_at=$2 WHERE id=$3 AND availability=$4",
                )
                .bind(target.as_db())
                .bind(now)
                .bind(unit)
                .bind(kind.assessment_target().as_db())
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                if moved != 1 {
                    return Err(KernelError::conflict(
                        "unit is not in the expected disposition state",
                    )
                    .into());
                }
                let updated = sqlx::query(
                    "UPDATE equipment_3r_dispositions SET status=$1, cost_minor=$2, sale_amount_minor=$3, buyer_name=$4, completed_by=$5, completed_at=$6, updated_at=$6 WHERE id=$7 AND status=$8",
                )
                .bind(DispositionState::Completed.as_db())
                .bind(cost)
                .bind(sale)
                .bind(buyer.as_deref())
                .bind(*actor.as_uuid())
                .bind(now)
                .bind(disposition)
                .bind(DispositionState::Open.as_db())
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                if updated != 1 {
                    return Err(KernelError::conflict("disposition left the open state").into());
                }
                history(tx, org, branch, "unit", unit, target.as_db(), actor, now).await?;
                history(tx, org, branch, "disposition", disposition, DispositionState::Completed.as_db(), actor, now).await?;
                let view = json!({
                    "id": disposition,
                    "unitId": unit,
                    "caseId": case,
                    "kind": kind.as_db(),
                    "status": DispositionState::Completed.as_db(),
                    "costMinor": cost,
                    "saleAmountMinor": sale,
                    "buyerName": buyer,
                    "completedBy": actor,
                    "completedAt": rfc3339(now)?,
                    "financeGlPosting": Value::Null,
                });
                Ok((
                    view,
                    vec![audit(org, actor, branch, "equipment_3r.disposition.complete", "equipment_3r_disposition", disposition, now)?],
                ))
            })
        })
        .await
    }
}

async fn lock_case(
    tx: &mut Transaction<'_, Postgres>,
    case: Uuid,
) -> Result<PgRow, PgEquipment3rError> {
    sqlx::query(
        "SELECT id,branch_id,unit_id,status,created_by FROM equipment_3r_rental_cases WHERE id=$1 FOR UPDATE",
    )
    .bind(case)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("rental case was not found").into())
}

async fn case_view_tx(
    tx: &mut Transaction<'_, Postgres>,
    case: Uuid,
) -> Result<Value, PgEquipment3rError> {
    let row = sqlx::query(
        "SELECT id,unit_id,status,customer_name,site_reference,monthly_rate_minor,duration_months,currency_code,branch_id FROM equipment_3r_rental_cases WHERE id=$1",
    )
    .bind(case)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("rental case was not found"))?;
    Ok(Value::Object(case_view_map(&row)?))
}

async fn case_detail_tx(
    tx: &mut Transaction<'_, Postgres>,
    case: Uuid,
) -> Result<(Value, BranchId), PgEquipment3rError> {
    let row = sqlx::query(
        "SELECT c.id,c.unit_id,c.status,c.customer_name,c.site_reference,c.monthly_rate_minor,c.duration_months,c.currency_code,c.branch_id, \
         c.approval_decision,c.approval_reason,c.approved_by,c.approved_at, \
         c.carrier_name,c.vehicle_reference,c.dispatched_at, \
         c.recipient_name,c.handover_evidence_object_id,c.handed_over_at,c.returned_at, \
         c.created_by,c.created_at,c.updated_at, \
         a.condition_grade,a.findings AS assessment_findings,a.disposition AS assessment_disposition,a.assessed_by,a.assessed_at, \
         d.id AS disposition_id \
         FROM equipment_3r_rental_cases c \
         LEFT JOIN equipment_3r_return_assessments a ON a.case_id=c.id \
         LEFT JOIN equipment_3r_dispositions d ON d.case_id=c.id \
         WHERE c.id=$1",
    )
    .bind(case)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("rental case was not found"))?;
    let branch = BranchId::from_uuid(row.try_get("branch_id")?);
    let mut view = case_view_map(&row)?;
    let approval = match row.try_get::<Option<String>, _>("approval_decision")? {
        Some(decision) => json!({
            "decision": decision,
            "reason": row.try_get::<Option<String>, _>("approval_reason")?,
            "decidedBy": row.try_get::<Option<Uuid>, _>("approved_by")?,
            "decidedAt": opt_rfc3339(row.try_get("approved_at")?)?,
        }),
        None => Value::Null,
    };
    view.insert("approval".into(), approval);
    let dispatch = match row.try_get::<Option<String>, _>("carrier_name")? {
        Some(carrier) => json!({
            "carrierName": carrier,
            "vehicleReference": row.try_get::<Option<String>, _>("vehicle_reference")?,
            "dispatchedAt": opt_rfc3339(row.try_get("dispatched_at")?)?,
        }),
        None => Value::Null,
    };
    view.insert("dispatch".into(), dispatch);
    let handover = match row.try_get::<Option<String>, _>("recipient_name")? {
        Some(recipient) => json!({
            "recipientName": recipient,
            "evidenceObjectId": row.try_get::<Option<Uuid>, _>("handover_evidence_object_id")?,
            "handedOverAt": opt_rfc3339(row.try_get("handed_over_at")?)?,
        }),
        None => Value::Null,
    };
    view.insert("handover".into(), handover);
    view.insert(
        "returnedAt".into(),
        opt_rfc3339(row.try_get("returned_at")?)?,
    );
    let assessment = match row.try_get::<Option<String>, _>("condition_grade")? {
        Some(grade) => json!({
            "conditionGrade": grade,
            "findings": row.try_get::<Option<String>, _>("assessment_findings")?,
            "disposition": row.try_get::<Option<String>, _>("assessment_disposition")?,
            "assessedBy": row.try_get::<Option<Uuid>, _>("assessed_by")?,
            "assessedAt": opt_rfc3339(row.try_get("assessed_at")?)?,
        }),
        None => Value::Null,
    };
    view.insert("assessment".into(), assessment);
    view.insert(
        "dispositionId".into(),
        json!(row.try_get::<Option<Uuid>, _>("disposition_id")?),
    );
    let inspection_rows = sqlx::query(
        "SELECT id,case_id,outcome,findings,maintenance_note,inspected_by,inspected_at FROM equipment_3r_inspections WHERE case_id=$1 ORDER BY inspected_at DESC, id",
    )
    .bind(case)
    .fetch_all(tx.as_mut())
    .await?;
    let mut inspections = Vec::with_capacity(inspection_rows.len());
    for inspection in &inspection_rows {
        inspections.push(json!({
            "id": inspection.try_get::<Uuid, _>("id")?,
            "caseId": inspection.try_get::<Uuid, _>("case_id")?,
            "outcome": inspection.try_get::<String, _>("outcome")?,
            "findings": inspection.try_get::<String, _>("findings")?,
            "maintenanceNote": inspection.try_get::<Option<String>, _>("maintenance_note")?,
            "inspectedBy": inspection.try_get::<Uuid, _>("inspected_by")?,
            "inspectedAt": rfc3339(inspection.try_get("inspected_at")?)?,
        }));
    }
    view.insert("inspections".into(), Value::Array(inspections));
    view.insert(
        "createdBy".into(),
        json!(row.try_get::<Uuid, _>("created_by")?),
    );
    view.insert(
        "createdAt".into(),
        Value::String(rfc3339(row.try_get("created_at")?)?),
    );
    view.insert(
        "updatedAt".into(),
        Value::String(rfc3339(row.try_get("updated_at")?)?),
    );
    Ok((Value::Object(view), branch))
}

fn unit_view_row(row: &PgRow) -> Result<Value, PgEquipment3rError> {
    Ok(json!({
        "id": row.try_get::<Uuid, _>("id")?,
        "serialNo": row.try_get::<String, _>("serial_no")?,
        "modelName": row.try_get::<String, _>("model_name")?,
        "capacityClass": row.try_get::<String, _>("capacity_class")?,
        "availability": row.try_get::<String, _>("availability")?,
        "acquisitionCostMinor": row.try_get::<i64, _>("acquisition_cost_minor")?,
        "branchId": row.try_get::<Uuid, _>("branch_id")?,
    }))
}

fn case_view_map(row: &PgRow) -> Result<Map<String, Value>, PgEquipment3rError> {
    let mut map = Map::new();
    map.insert("id".into(), json!(row.try_get::<Uuid, _>("id")?));
    map.insert("unitId".into(), json!(row.try_get::<Uuid, _>("unit_id")?));
    map.insert("status".into(), json!(row.try_get::<String, _>("status")?));
    map.insert(
        "customerName".into(),
        json!(row.try_get::<String, _>("customer_name")?),
    );
    map.insert(
        "siteReference".into(),
        json!(row.try_get::<String, _>("site_reference")?),
    );
    map.insert(
        "monthlyRateMinor".into(),
        json!(row.try_get::<i64, _>("monthly_rate_minor")?),
    );
    map.insert(
        "durationMonths".into(),
        json!(row.try_get::<i32, _>("duration_months")?),
    );
    map.insert(
        "currencyCode".into(),
        json!(row.try_get::<String, _>("currency_code")?),
    );
    map.insert(
        "branchId".into(),
        json!(row.try_get::<Uuid, _>("branch_id")?),
    );
    Ok(map)
}

fn required(value: &str, name: &str, max: usize) -> Result<(), PgEquipment3rError> {
    if value.trim().is_empty() || value.chars().count() > max {
        Err(KernelError::validation(format!(
            "{name} is required and bounded to {max} characters"
        ))
        .into())
    } else {
        Ok(())
    }
}

fn idem(key: &str) -> Result<(), PgEquipment3rError> {
    if key.trim().len() < 16 || key.len() > 200 {
        Err(KernelError::validation("Idempotency-Key must be 16..200 characters").into())
    } else {
        Ok(())
    }
}

/// Bind the Docs-owned immutable custody relation for one handover, inside the
/// caller's audited transaction.
///
/// The eligibility predicate below mirrors the trigger
/// `docs_equipment_handover_custody_eligible()` installed by migration 0184,
/// which stays the enforcing authority: this statement only *resolves* the
/// `ORIGINAL` copy the custody FK requires (at most one exists per object —
/// `docs_evidence_copies_one_original_per_object`) and shapes the refusal. Selecting
/// zero rows conceals every rejection class — foreign tenant (RLS), unknown
/// object, non-`ADMISSIBLE`, disposed, or an original whose WORM copy was never
/// verified — behind one `not_found`, so a caller cannot probe another org's
/// evidence estate. A predicate drift between the two is caught by the
/// `handover_conceals_ineligible_or_foreign_evidence_and_denies_foreign_branch`
/// story test, which drives each rejection class through the HTTP boundary: if
/// this query ever admits a row the trigger refuses, the trigger raises and the
/// case fails to transition instead of returning 404.
///
/// The equipment adapter issues this SQL directly rather than calling
/// `console-docs-adapter-postgres`, because adapter → adapter is an illegal layer
/// edge (`console-gate-layer-boundary`).
async fn bind_handover_custody(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    branch: BranchId,
    case: Uuid,
    evidence_object: Uuid,
    actor: UserId,
) -> Result<(), PgEquipment3rError> {
    let bound = sqlx::query(
        "INSERT INTO docs_equipment_handover_custody \
         (org_id,branch_id,equipment_case_id,evidence_object_id,original_copy_id,created_by) \
         SELECT $1,$2,$3,o.id,c.id,$5 \
         FROM docs_evidence_objects o \
         JOIN docs_evidence_copies c ON c.evidence_object_id=o.id AND c.org_id=o.org_id \
         WHERE o.id=$4 \
           AND o.admissibility_status='ADMISSIBLE' \
           AND o.disposed_at IS NULL \
           AND o.current_custody_stage <> 'DISPOSED' \
           AND c.copy_kind='ORIGINAL' \
           AND c.worm_status='VERIFIED' \
           AND c.verified_at IS NOT NULL",
    )
    .bind(*org.as_uuid())
    .bind(*branch.as_uuid())
    .bind(case)
    .bind(evidence_object)
    .bind(*actor.as_uuid())
    .execute(tx.as_mut())
    .await?
    .rows_affected();
    if bound == 1 {
        Ok(())
    } else {
        Err(KernelError::not_found("eligible handover evidence was not found").into())
    }
}

fn fingerprint(v: &Value) -> String {
    hex::encode(Sha256::digest(v.to_string()))
}

fn rfc3339(at: OffsetDateTime) -> Result<String, PgEquipment3rError> {
    at.format(&Rfc3339)
        .map_err(|e| KernelError::internal(format!("timestamp formatting failed: {e}")).into())
}

fn opt_rfc3339(at: Option<OffsetDateTime>) -> Result<Value, PgEquipment3rError> {
    Ok(match at {
        Some(at) => Value::String(rfc3339(at)?),
        None => Value::Null,
    })
}

fn audit(
    org: OrgId,
    actor: UserId,
    branch: BranchId,
    action: &str,
    kind: &str,
    id: Uuid,
    at: OffsetDateTime,
) -> Result<AuditEvent, PgEquipment3rError> {
    let action = AuditAction::new(action)
        .map_err(|e| KernelError::internal(format!("invalid audit action: {e}")))?;
    Ok(AuditEvent::new(
        Some(actor),
        action,
        kind,
        id.to_string(),
        TraceContext::generate(),
        at,
    )
    .with_org(org)
    .with_branch(branch))
}

#[allow(clippy::too_many_arguments)]
async fn history(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    branch: BranchId,
    kind: &str,
    id: Uuid,
    transition: &str,
    actor: UserId,
    at: OffsetDateTime,
) -> Result<(), PgEquipment3rError> {
    sqlx::query(
        "INSERT INTO equipment_3r_history (org_id,branch_id,aggregate_kind,aggregate_id,transition,actor_id,occurred_at,trace_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(*org.as_uuid())
    .bind(*branch.as_uuid())
    .bind(kind)
    .bind(id)
    .bind(transition)
    .bind(*actor.as_uuid())
    .bind(at)
    .bind(Uuid::new_v4())
    .execute(tx.as_mut())
    .await?;
    Ok(())
}
