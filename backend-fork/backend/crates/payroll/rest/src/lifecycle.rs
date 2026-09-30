//! Payroll run lifecycle endpoints (CAP-PAYROLL-CONSOLE):
//! close-attendance → calculate → exception review → SoD approval →
//! operator-attested disbursement → release-gated payslip issuance.
//!
//! Authorization: reads reuse `Feature::PayrollRunRead`; every mutation
//! requires the write tier `Feature::PayrollRunManage` — the SAME org-wide
//! gate (`authorize_org_wide`): built-in EXECUTIVE/SUPER_ADMIN only, ADMIN
//! solely via a custom org-wide PBAC grant, branch-scoped callers 403.
//!
//! Every mutation runs inside `with_audits` (mutation + audit event atomic);
//! admin reads of run-scoped data are themselves audited the same way. The
//! close preflight is audited too, but computes its verdict in a read-only
//! transaction so the verdict itself can never be persisted and later trusted
//! (TOCTOU); the close recomputes it in the closing transaction. A 404
//! never distinguishes "does not exist" from "another org's run" (RLS,
//! deny-by-omission). Lifecycle guards make every mutation replay-safe: a
//! second identical POST hits a state guard and returns a typed 409 instead
//! of double-applying (state-machine idempotency; there is no client
//! idempotency key to desynchronize).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use console_inbox_adapter_postgres::{PgInboxError, PgInboxStore};
use console_inbox_application::EmitInboxDocCommand;
use console_inbox_domain::{InboxDocKind, NewInboxDoc};
use console_kernel_core::{AuditAction, AuditEvent, TraceContext, UserId};
use console_payroll_adapter_postgres::lifecycle::{
    self, ClosePreflight, Disbursement, ExceptionPage, PayrollException, PayslipDeliverySummary,
};
use console_payroll_adapter_postgres::{PayrollRunDetail, get_run_in_tx};
use console_platform_authz::{Action, Feature, Principal, authorize_org_wide};
use console_platform_db::{with_audit, with_audits, with_org_conn};
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{PageParams, PayrollRestState, RestError, principal_from_headers, require_run_read};

/// Org-wide payroll lifecycle write gate; see module docs.
pub(crate) fn require_run_manage(principal: &Principal) -> Result<(), RestError> {
    authorize_org_wide(principal, Action::new(Feature::PayrollRunManage))
        .map_err(RestError::from_kernel)
}

impl RestError {
    fn from_lifecycle(err: lifecycle::LifecycleError) -> Self {
        use lifecycle::LifecycleError as E;
        match err {
            E::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", "run not found"),
            E::PreflightBlocked(preflight) => {
                let details = serde_json::to_value(&preflight).ok();
                Self::new(
                    StatusCode::CONFLICT,
                    "preflight_blocked",
                    "attendance close preflight is blocked",
                )
                .with_details(details)
            }
            E::InvalidState(message) => Self::new(StatusCode::CONFLICT, "invalid_state", message),
            E::ExceptionsOpen(open) => Self::new(
                StatusCode::CONFLICT,
                "exceptions_open",
                format!("{open} exceptions are still open"),
            )
            .with_details(Some(json!({ "open": open }))),
            E::SodViolation => Self::new(
                StatusCode::CONFLICT,
                "sod_violation",
                "the decider must not be the submitter",
            ),
            E::AlreadyResolved => Self::new(
                StatusCode::CONFLICT,
                "already_resolved",
                "exception is already resolved",
            ),
            E::InvalidTransition(message) => {
                Self::new(StatusCode::CONFLICT, "invalid_transition", message)
            }
            E::LegalGate(message) => Self::new(StatusCode::CONFLICT, "legal_gate", message),
            E::Validation(message) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", message)
            }
            E::Db(err) => {
                tracing::error!(error = %err, "payroll lifecycle db error");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal server error",
                )
            }
        }
    }

    fn from_inbox(err: PgInboxError) -> Self {
        match err {
            PgInboxError::Domain(err) => Self::from_kernel(err),
            other => {
                tracing::error!(error = %other, "payslip inbox delivery error");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal server error",
                )
            }
        }
    }
}

fn audit_event(
    actor: UserId,
    org: console_kernel_core::OrgId,
    action: &str,
    target_type: &'static str,
    target_id: impl ToString,
    after: Option<Value>,
) -> Result<AuditEvent, RestError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new(action).map_err(RestError::from_kernel)?,
        target_type,
        target_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(None, after))
}

async fn run_detail_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
) -> Result<PayrollRunDetail, RestError> {
    get_run_in_tx(tx, run_id, None, None)
        .await
        .map_err(RestError::from_store)?
        .ok_or_else(|| RestError::new(StatusCode::NOT_FOUND, "not_found", "run not found"))
}

// ---------------------------------------------------------------------------
// Close preflight + close
// ---------------------------------------------------------------------------

pub(crate) async fn get_close_preflight(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_run_read(&principal)?;
    let (org, actor) = (principal.org_id, principal.user_id);
    let pool = state.store.pool().clone();
    // The verdict is computed in a read-only transaction, so no path here can
    // persist it — and `close_attendance_in_tx` therefore has no stored verdict
    // to trust instead of recomputing (TOCTOU). The read is still audited, in
    // its own transaction below, before the blocking refs are disclosed.
    let preflight = with_org_conn::<_, Option<ClosePreflight>, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            lifecycle::close_preflight_in_tx(tx, run_id)
                .await
                .map_err(RestError::from_lifecycle)
        })
    })
    .await?;
    let preflight = preflight
        .ok_or_else(|| RestError::new(StatusCode::NOT_FOUND, "not_found", "run not found"))?;
    let read_event = audit_event(
        actor,
        org,
        "payroll_run.preflight_read",
        "payroll_draft_run",
        run_id,
        // No `after_snap`: this row records THAT the run was read, never the
        // verdict. A verdict in the audit log is a readable cache, and
        // `close_attendance_in_tx` must recompute rather than trust one.
        None,
    )?;
    with_audit::<_, (), RestError>(&pool, read_event, |_tx| Box::pin(async { Ok(()) })).await?;
    Ok(Json(preflight).into_response())
}

#[derive(Debug, Deserialize)]
pub(crate) struct CloseAttendanceBody {
    attest: bool,
}

pub(crate) async fn close_attendance(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    Json(body): Json<CloseAttendanceBody>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_run_manage(&principal)?;
    if !body.attest {
        return Err(RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "attest: true is required to close attendance",
        ));
    }
    let (org, actor) = (principal.org_id, principal.user_id);
    let pool = state.store.pool().clone();
    let detail = with_audits::<_, PayrollRunDetail, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            let receipt = lifecycle::close_attendance_in_tx(
                tx,
                run_id,
                *actor.as_uuid(),
                OffsetDateTime::now_utc(),
            )
            .await
            .map_err(RestError::from_lifecycle)?;
            let detail = run_detail_in_tx(tx, run_id).await?;
            let event = audit_event(
                actor,
                org,
                "payroll_run.close",
                "payroll_draft_run",
                run_id,
                Some(receipt),
            )?;
            Ok((detail, vec![event]))
        })
    })
    .await?;
    Ok(Json(detail).into_response())
}

// ---------------------------------------------------------------------------
// Calculation
// ---------------------------------------------------------------------------

pub(crate) async fn calculate_run(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_run_manage(&principal)?;
    let (org, actor) = (principal.org_id, principal.user_id);
    let pool = state.store.pool().clone();
    let detail = with_audits::<_, PayrollRunDetail, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            let outcome = lifecycle::calculate_run_in_tx(tx, run_id)
                .await
                .map_err(RestError::from_lifecycle)?;
            let detail = run_detail_in_tx(tx, run_id).await?;
            let event = audit_event(
                actor,
                org,
                "payroll_run.calculate",
                "payroll_draft_run",
                run_id,
                Some(json!({
                    "version": outcome.version,
                    "calculated_lines": outcome.calculated_lines,
                    "blocked_lines": outcome.blocked_lines,
                    "exceptions_created": outcome.exceptions_created,
                })),
            )?;
            Ok((detail, vec![event]))
        })
    })
    .await?;
    Ok(Json(detail).into_response())
}

// ---------------------------------------------------------------------------
// Exceptions
// ---------------------------------------------------------------------------

pub(crate) async fn list_exceptions(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    Query(params): Query<PageParams>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_run_read(&principal)?;
    let (org, actor) = (principal.org_id, principal.user_id);
    let pool = state.store.pool().clone();
    let page = with_audits::<_, Option<ExceptionPage>, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            let page = lifecycle::list_exceptions_in_tx(tx, run_id, params.limit, params.offset)
                .await
                .map_err(RestError::from_lifecycle)?;
            let events = if page.is_some() {
                vec![audit_event(
                    actor,
                    org,
                    "payroll_run.exceptions_read",
                    "payroll_draft_run",
                    run_id,
                    None,
                )?]
            } else {
                Vec::new()
            };
            Ok((page, events))
        })
    })
    .await?;
    let page =
        page.ok_or_else(|| RestError::new(StatusCode::NOT_FOUND, "not_found", "run not found"))?;
    Ok(Json(page).into_response())
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResolveExceptionBody {
    action: String,
    reason: Option<String>,
}

pub(crate) async fn resolve_exception(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path((run_id, exception_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<ResolveExceptionBody>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_run_manage(&principal)?;
    let (org, actor) = (principal.org_id, principal.user_id);
    let pool = state.store.pool().clone();
    let exception = with_audits::<_, PayrollException, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            let exception = lifecycle::resolve_exception_in_tx(
                tx,
                run_id,
                exception_id,
                *actor.as_uuid(),
                &body.action,
                body.reason.as_deref(),
            )
            .await
            .map_err(RestError::from_lifecycle)?;
            let event = audit_event(
                actor,
                org,
                "payroll_run.exception_resolve",
                "payroll_run_exception",
                exception_id,
                Some(json!({
                    "run_id": run_id,
                    "action": body.action,
                    "reason": body.reason,
                    "status": exception.status,
                })),
            )?;
            Ok((exception, vec![event]))
        })
    })
    .await?;
    Ok(Json(exception).into_response())
}

// ---------------------------------------------------------------------------
// Submission / decision / withdrawal
// ---------------------------------------------------------------------------

pub(crate) async fn submit_run(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_run_manage(&principal)?;
    let (org, actor) = (principal.org_id, principal.user_id);
    let pool = state.store.pool().clone();
    let detail = with_audits::<_, PayrollRunDetail, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            lifecycle::submit_run_in_tx(tx, run_id, *actor.as_uuid())
                .await
                .map_err(RestError::from_lifecycle)?;
            let detail = run_detail_in_tx(tx, run_id).await?;
            let event = audit_event(
                actor,
                org,
                "payroll_run.submit",
                "payroll_draft_run",
                run_id,
                None,
            )?;
            Ok((detail, vec![event]))
        })
    })
    .await?;
    Ok(Json(detail).into_response())
}

#[derive(Debug, Deserialize)]
pub(crate) struct DecisionBody {
    decision: String,
    reason: Option<String>,
}

pub(crate) async fn decide_run(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    Json(body): Json<DecisionBody>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_run_manage(&principal)?;
    let (org, actor) = (principal.org_id, principal.user_id);
    let pool = state.store.pool().clone();
    let detail = with_audits::<_, PayrollRunDetail, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            lifecycle::decide_run_in_tx(
                tx,
                run_id,
                *actor.as_uuid(),
                &body.decision,
                body.reason.as_deref(),
            )
            .await
            .map_err(RestError::from_lifecycle)?;
            let detail = run_detail_in_tx(tx, run_id).await?;
            let event = audit_event(
                actor,
                org,
                "payroll_run.decide",
                "payroll_draft_run",
                run_id,
                Some(json!({ "decision": body.decision, "reason": body.reason })),
            )?;
            Ok((detail, vec![event]))
        })
    })
    .await?;
    Ok(Json(detail).into_response())
}

pub(crate) async fn withdraw_run(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_run_manage(&principal)?;
    let (org, actor) = (principal.org_id, principal.user_id);
    let pool = state.store.pool().clone();
    let detail = with_audits::<_, PayrollRunDetail, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            lifecycle::withdraw_run_in_tx(tx, run_id)
                .await
                .map_err(RestError::from_lifecycle)?;
            let detail = run_detail_in_tx(tx, run_id).await?;
            let event = audit_event(
                actor,
                org,
                "payroll_run.withdraw",
                "payroll_draft_run",
                run_id,
                None,
            )?;
            Ok((detail, vec![event]))
        })
    })
    .await?;
    Ok(Json(detail).into_response())
}

// ---------------------------------------------------------------------------
// Disbursement
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct ScheduleDisbursementBody {
    scheduled_at: String,
}

pub(crate) async fn schedule_disbursement(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    Json(body): Json<ScheduleDisbursementBody>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_run_manage(&principal)?;
    let scheduled_at = OffsetDateTime::parse(&body.scheduled_at, &Rfc3339).map_err(|err| {
        RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            format!("scheduled_at must be an RFC 3339 timestamp: {err}"),
        )
    })?;
    if scheduled_at <= OffsetDateTime::now_utc() {
        return Err(RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "scheduled_at must be in the future",
        ));
    }
    let (org, actor) = (principal.org_id, principal.user_id);
    let pool = state.store.pool().clone();
    let disbursement = with_audits::<_, Disbursement, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            let disbursement = lifecycle::schedule_disbursement_in_tx(tx, run_id, scheduled_at)
                .await
                .map_err(RestError::from_lifecycle)?;
            let event = audit_event(
                actor,
                org,
                "payroll_run.disburse_schedule",
                "payroll_disbursement",
                disbursement.id,
                Some(json!({ "run_id": run_id, "scheduled_at": body.scheduled_at })),
            )?;
            Ok((disbursement, vec![event]))
        })
    })
    .await?;
    Ok((StatusCode::CREATED, Json(disbursement)).into_response())
}

#[derive(Debug, Deserialize)]
pub(crate) struct AttestDisbursementBody {
    status: String,
    reason: Option<String>,
}

pub(crate) async fn attest_disbursement(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    Json(body): Json<AttestDisbursementBody>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_run_manage(&principal)?;
    let (org, actor) = (principal.org_id, principal.user_id);
    let pool = state.store.pool().clone();
    let disbursement = with_audits::<_, Disbursement, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            let disbursement = lifecycle::attest_disbursement_in_tx(
                tx,
                run_id,
                *actor.as_uuid(),
                &body.status,
                body.reason.as_deref(),
            )
            .await
            .map_err(RestError::from_lifecycle)?;
            let event = audit_event(
                actor,
                org,
                "payroll_run.disburse_attest",
                "payroll_disbursement",
                disbursement.id,
                Some(json!({
                    "run_id": run_id,
                    "status": body.status,
                    "reason": body.reason,
                })),
            )?;
            Ok((disbursement, vec![event]))
        })
    })
    .await?;
    Ok(Json(disbursement).into_response())
}

// ---------------------------------------------------------------------------
// Payslip issuance + delivery readback
// ---------------------------------------------------------------------------

pub(crate) async fn issue_payslips(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_run_manage(&principal)?;
    let (org, actor) = (principal.org_id, principal.user_id);
    let pool = state.store.pool().clone();

    // 1. Guard state + release gate and load the deliverable lines.
    let (run, lines) = with_org_conn::<_, _, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            lifecycle::load_payslip_issuance_in_tx(tx, run_id)
                .await
                .map_err(RestError::from_lifecycle)
        })
    })
    .await?;

    // 2. Deliver one payslip document per line into the recipient's vault.
    //    Deduped by (recipient, payroll-run:line) so an interrupted issuance
    //    re-driven from step 1 never double-delivers.
    let inbox = PgInboxStore::new(pool.clone());
    let mut deliveries: Vec<(Uuid, Uuid, Uuid)> = Vec::with_capacity(lines.len());
    for line in &lines {
        let payload = json!({
            "run_id": run_id,
            "line_id": line.line_id,
            "period_start": run.period_start.to_string(),
            "period_end": run.period_end.to_string(),
            "gross_won": line.gross_won,
            "deductions": line.deductions,
            "total_deductions_won": line.total_deductions_won,
            "net_won": line.net_won,
            "tax_table_version": line.tax_table_version,
            "calculation_version": line.version,
        });
        let doc = NewInboxDoc::new(
            InboxDocKind::Payslip,
            &format!(
                "급여명세서 {} ~ {} · {}",
                run.period_start, run.period_end, line.employee_display_name
            ),
            None,
            None,
            Some("payroll_run"),
            Some(&run_id.to_string()),
            payload,
        )
        .map_err(RestError::from_kernel)?;
        let summary = inbox
            .emit_inbox_doc(EmitInboxDocCommand {
                actor: Some(actor),
                recipient: UserId::from_uuid(line.recipient_user_id),
                doc,
                dedup_key: Some(format!("payroll-run:{run_id}:line:{}", line.line_id)),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .map_err(RestError::from_inbox)?;
        deliveries.push((line.line_id, line.employee_id, *summary.id.as_uuid()));
    }

    // 3. Record the delivery links, flip the run to ISSUED, and audit — one
    //    atomic transaction.
    let summary = with_audits::<_, PayslipDeliverySummary, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            lifecycle::record_payslip_deliveries_in_tx(tx, run_id, &deliveries)
                .await
                .map_err(RestError::from_lifecycle)?;
            let summary = lifecycle::payslip_delivery_in_tx(tx, run_id, None, None)
                .await
                .map_err(RestError::from_lifecycle)?
                .ok_or_else(|| {
                    RestError::new(StatusCode::NOT_FOUND, "not_found", "run not found")
                })?;
            let event = audit_event(
                actor,
                org,
                "payroll_run.payslip_issue",
                "payroll_draft_run",
                run_id,
                Some(json!({ "issued": summary.issued })),
            )?;
            Ok((summary, vec![event]))
        })
    })
    .await?;
    Ok(Json(summary).into_response())
}

pub(crate) async fn get_payslip_delivery(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    Query(params): Query<PageParams>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_run_read(&principal)?;
    let (org, actor) = (principal.org_id, principal.user_id);
    let pool = state.store.pool().clone();
    let summary =
        with_audits::<_, Option<PayslipDeliverySummary>, RestError>(&pool, org, move |tx| {
            Box::pin(async move {
                let summary =
                    lifecycle::payslip_delivery_in_tx(tx, run_id, params.limit, params.offset)
                        .await
                        .map_err(RestError::from_lifecycle)?;
                let events = if summary.is_some() {
                    vec![audit_event(
                        actor,
                        org,
                        "payroll_run.payslip_delivery_read",
                        "payroll_draft_run",
                        run_id,
                        None,
                    )?]
                } else {
                    Vec::new()
                };
                Ok((summary, events))
            })
        })
        .await?;
    let summary = summary
        .ok_or_else(|| RestError::new(StatusCode::NOT_FOUND, "not_found", "run not found"))?;
    Ok(Json(summary).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_platform_authz::Role;
    use std::collections::BTreeSet;

    fn principal(role: Role, scope: console_kernel_core::BranchScope) -> Principal {
        Principal::new(
            console_kernel_core::UserId::new(),
            console_kernel_core::OrgId::knl(),
            BTreeSet::from([role]),
            scope,
        )
    }

    #[test]
    fn manage_gate_mirrors_read_gate_exactly() {
        // Denied: MEMBER, built-in ADMIN (any scope), branch-scoped ADMIN.
        for p in [
            principal(Role::Member, console_kernel_core::BranchScope::All),
            principal(Role::Admin, console_kernel_core::BranchScope::All),
            principal(
                Role::Admin,
                console_kernel_core::BranchScope::Branches(BTreeSet::from([
                    console_kernel_core::BranchId::new(),
                ])),
            ),
        ] {
            assert_eq!(
                require_run_manage(&p).unwrap_err().status,
                StatusCode::FORBIDDEN
            );
        }
        // Allowed: EXECUTIVE + SUPER_ADMIN org-wide.
        for role in [Role::Executive, Role::SuperAdmin] {
            let p = principal(role, console_kernel_core::BranchScope::All);
            assert!(require_run_manage(&p).is_ok(), "{role:?} must manage runs");
        }
    }
}
