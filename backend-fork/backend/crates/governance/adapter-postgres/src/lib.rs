//! Postgres governance adapter.
//!
//! Every mutation flows through `with_audit` (mutation + audit row in one tx),
//! every read through `with_org_conn`, so `app.current_org` is armed before any
//! statement and RLS scopes it to the tenant. All three tables run FORCE RLS;
//! the two record tables are append-only (REVOKE UPDATE/DELETE).
//!
//! # Distinct-natural-person four-eyes (console-dgo.1)
//!
//! For kinds under `company.*` / `hr.*` / `payroll.*` only, open and decide
//! resolve each party via `users.employee_id → employee_person_bindings →
//! Person`. Unbound or non-person accounts fail closed; two accounts that map
//! to the same `person_id` are denied even when `user_id` differs. Generic
//! kinds keep the account-level `approver_id <> requested_by` bar so NULL
//! `users.employee_id` (migration 0076's normal admin-gated state) still works.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_governance_application::{
    ApprovalDecision, ApprovalRequestSummary, ApprovalSummary, ConfigureTransitionCommand,
    CreateApprovalCommand, DecideApprovalCommand, DecidePendingApprovalCommand,
    LifecycleTransitionConfig, OpenOverrideCommand, OverrideSummary, governance_audit_event,
};
use console_governance_domain::{
    AuthorityEffect, GateChainConfig, GateChainOutcome, GateEvidence, LifecycleState,
    TransitionRequirements, evaluate_gate_chain, requires_natural_person_four_eyes,
    validate_lifecycle_transition,
};
use console_kernel_core::{KernelError, UserId};
use console_platform_authz::cedar_pbac::DecisionEffect;
use console_platform_db::{DbError, with_audit, with_org_conn};
use console_platform_request_context::current_org;
use sqlx::{PgConnection, PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

/// One open four-eyes request visible to a distinct approver (inbox list only).
#[derive(Debug, Clone)]
pub struct GovernanceActionInboxItem {
    pub id: Uuid,
    pub request_ref: Uuid,
    pub kind: String,
    pub created_at: OffsetDateTime,
}

/// Map the Cedar evaluator's decision effect onto the domain's Authority-gate
/// input. This is the seam where the guardrail Authority gate "calls the Cedar
/// evaluator": the ontology action lane runs `engine::evaluate(...)`, converts
/// its `DecisionEffect` here, and feeds it to `evaluate_gate_chain`.
#[must_use]
pub fn authority_effect_from_cedar(effect: DecisionEffect) -> AuthorityEffect {
    match effect {
        DecisionEffect::Allow => AuthorityEffect::Allow,
        DecisionEffect::Deny => AuthorityEffect::Deny,
    }
}

/// Which four-eyes decision contract `record_decision` is enforcing.
///
/// Deliberately NOT `PartialEq`: selecting a contract by `contract == Hardened`
/// would let a third variant silently inherit the weak behaviour. Every
/// selection must be an exhaustive `match`, so the compiler makes the author of
/// a new variant choose.
#[derive(Debug, Clone, Copy)]
enum DecisionContract {
    /// The contract every authenticated surface uses: an open pending request is
    /// required, so the requester the approver must differ from is one an
    /// authenticated requester recorded.
    Hardened,
    /// Deprecated: decides without an open request, falling back to the
    /// client-supplied `requested_by` / `target_ref`.
    DeprecatedCompat,
}

/// The approval kind a lifecycle four-eyes gate binds to. Server-derived: the
/// preflight peek and the committing writeback must agree on it, so it is a
/// constant here rather than anything a caller supplies — and `pub` so callers
/// can import THIS one instead of keeping a copy. NOT YET the single source:
/// `backend/crates/ontology/rest/src/lib.rs` still declares its own private
/// `LIFECYCLE_FOUR_EYES_KIND`, so the peek here and the consuming writeback can
/// still drift apart and no test compares them.
pub const LIFECYCLE_FOUR_EYES_KIND: &str = "ontology.lifecycle";

/// Inputs to [`PgGovernanceStore::lifecycle_preflight`]. `authority_allow` is the
/// Cedar effect the console already has (the writeback lane re-runs Cedar
/// itself); absent ⇒ fail-closed. The four-eyes verdict is NOT an input — it is
/// read from the DB under `four_eyes_request_ref`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecyclePreflightQuery {
    pub object_type_id: Uuid,
    pub from_state: LifecycleState,
    pub to_state: LifecycleState,
    pub authority_allow: Option<bool>,
    pub checklist_all_acknowledged: Option<bool>,
    pub four_eyes_request_ref: Option<Uuid>,
    pub egress_cleared: Option<bool>,
}

/// The advisory verdict of [`PgGovernanceStore::lifecycle_preflight`]. Advisory
/// only: nothing about it is persisted, and the committing gate re-evaluates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecyclePreflight {
    /// `false` when the edge is not configured for this object type — an
    /// unconfigured edge is denied (fail-closed), even if the base FSM allows it.
    pub configured: bool,
    pub config: GateChainConfig,
    pub outcome: GateChainOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum PgGovernanceError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl From<sqlx::Error> for PgGovernanceError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

#[derive(Debug, Clone)]
pub struct PgGovernanceStore {
    pool: PgPool,
}

impl PgGovernanceStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // -- §3b post-draft override --------------------------------------------

    pub async fn open_override(
        &self,
        command: OpenOverrideCommand,
    ) -> Result<OverrideSummary, PgGovernanceError> {
        if command.reason.trim().is_empty() {
            return Err(KernelError::validation("override reason is required").into());
        }
        if !command.before_snapshot.is_object() {
            return Err(
                KernelError::validation("override before-snapshot must be a JSON object").into(),
            );
        }
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let override_id = Uuid::new_v4();
        let event = governance_audit_event(
            "governance.override.open",
            command.actor,
            "gov_override",
            override_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org)
        .with_snapshots(Some(command.before_snapshot.clone()), None);

        with_audit::<_, OverrideSummary, PgGovernanceError>(&self.pool, event, |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    INSERT INTO gov_overrides
                        (id, org_id, target_type, target_id, actor, reason, before_snapshot, created_at)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    "#,
                )
                .bind(override_id)
                .bind(org_uuid)
                .bind(command.target_type.trim())
                .bind(command.target_id)
                .bind(*command.actor.as_uuid())
                .bind(command.reason.trim())
                .bind(&command.before_snapshot)
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;
                override_row_conn(tx.as_mut(), override_id).await
            })
        })
        .await
    }

    // -- approvals-create (open a pending request) --------------------------

    /// Open a pending four-eyes request (arch §19). Records who is asking and a
    /// payload summary; a *distinct* approver decides it later via
    /// [`Self::decide_pending_approval`] keyed by the same `request_ref`. One open request
    /// per `(org, request_ref)` — a second open for the same ref conflicts.
    pub async fn create_approval(
        &self,
        command: CreateApprovalCommand,
    ) -> Result<ApprovalRequestSummary, PgGovernanceError> {
        if command.kind.trim().is_empty() {
            return Err(KernelError::validation("approval kind is required").into());
        }
        if !command.payload_summary.is_object() {
            return Err(
                KernelError::validation("approval payload_summary must be a JSON object").into(),
            );
        }
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let request_id = Uuid::new_v4();
        let event = governance_audit_event(
            "governance.approval.request",
            command.requester,
            "gov_approval_request",
            request_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org)
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "request_ref": command.request_ref,
                "kind": command.kind,
            })),
        );

        with_audit::<_, ApprovalRequestSummary, PgGovernanceError>(&self.pool, event, |tx| {
            Box::pin(async move {
                // Company/HR/Payroll opens fail closed when the requester cannot
                // be resolved to a natural person. Generic kinds skip this so
                // NULL employee_id accounts keep working.
                require_natural_person_bound_conn(
                    tx.as_mut(),
                    command.kind.trim(),
                    command.requester,
                    "requester",
                )
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO gov_approval_requests
                        (id, org_id, request_ref, kind, requested_by, target_ref, payload_summary, created_at)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    "#,
                )
                .bind(request_id)
                .bind(org_uuid)
                .bind(command.request_ref)
                .bind(command.kind.trim())
                .bind(*command.requester.as_uuid())
                .bind(command.target_ref)
                .bind(&command.payload_summary)
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;
                approval_request_row_conn(tx.as_mut(), request_id).await
            })
        })
        .await
    }

    /// Keyset page of open pending four-eyes requests the caller did not open.
    ///
    /// Org isolation is RLS via `with_org_conn`. Who may act (RoleManage) is
    /// enforced by the inbox composition root; this query only applies SoD and
    /// as-of predicates, matching `decide_pending_approval`'s requester bar.
    pub async fn list_pending_action_inbox_page(
        &self,
        caller: UserId,
        as_of: OffsetDateTime,
        after: Option<(OffsetDateTime, String)>,
        limit: i64,
    ) -> Result<(Vec<GovernanceActionInboxItem>, i64, bool), PgGovernanceError> {
        let limit = limit.clamp(1, 200);
        let org = current_org().map_err(KernelError::from)?;
        let caller_id = *caller.as_uuid();
        let (after_created_at, after_id) = after.map_or((None, None), |(created_at, id)| {
            (Some(created_at), Some(id))
        });
        with_org_conn::<_, _, PgGovernanceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let total: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM gov_approval_requests r \
                     WHERE r.requested_by <> $1 \
                       AND r.created_at <= $2 \
                       AND NOT EXISTS ( \
                         SELECT 1 FROM gov_approvals a \
                         WHERE a.request_ref = r.request_ref AND a.org_id = r.org_id \
                       )",
                )
                .bind(caller_id)
                .bind(as_of)
                .fetch_one(tx.as_mut())
                .await?;
                let rows = sqlx::query(
                    "SELECT r.id, r.request_ref, r.kind, r.created_at \
                     FROM gov_approval_requests r \
                     WHERE r.requested_by <> $1 \
                       AND r.created_at <= $2 \
                       AND NOT EXISTS ( \
                         SELECT 1 FROM gov_approvals a \
                         WHERE a.request_ref = r.request_ref AND a.org_id = r.org_id \
                       ) \
                       AND ($4::text IS NULL OR r.created_at > $3 \
                         OR (r.created_at = $3 AND ('governance:' || r.id::text) > $4)) \
                     ORDER BY r.created_at ASC, ('governance:' || r.id::text) ASC \
                     LIMIT $5",
                )
                .bind(caller_id)
                .bind(as_of)
                .bind(after_created_at)
                .bind(after_id.as_deref())
                .bind(limit + 1)
                .fetch_all(tx.as_mut())
                .await?;
                let has_more = i64::try_from(rows.len()).unwrap_or(0) > limit;
                let items = rows
                    .iter()
                    .take(usize::try_from(limit).unwrap_or(rows.len()))
                    .map(|row| {
                        Ok(GovernanceActionInboxItem {
                            id: row.try_get("id")?,
                            request_ref: row.try_get("request_ref")?,
                            kind: row.try_get("kind")?,
                            created_at: row.try_get("created_at")?,
                        })
                    })
                    .collect::<Result<Vec<_>, PgGovernanceError>>()?;
                Ok((items, total, has_more))
            })
        })
        .await
    }

    // -- four-eyes decision --------------------------------------------------

    /// Decide a four-eyes request that MUST already be open as a pending request
    /// (arch §19). The requester, kind and binding target all come from that row
    /// inside the decision transaction; the command carries no `requested_by` at
    /// all, so an approver cannot name the account they are supposedly distinct
    /// from. This is the contract the REST surface uses.
    pub async fn decide_pending_approval(
        &self,
        command: DecidePendingApprovalCommand,
    ) -> Result<ApprovalSummary, PgGovernanceError> {
        self.record_decision(
            DecideApprovalCommand {
                approver: command.approver,
                request_ref: command.request_ref,
                kind: command.kind,
                // Never read on this path — a pending row is required. Were that
                // ever to regress, approver == requested_by trips the
                // self-approval guard below, so the failure is closed.
                requested_by: command.approver,
                // Ditto: the pending row's target is authoritative.
                target_ref: None,
                decision: command.decision,
                trace: command.trace,
                occurred_at: command.occurred_at,
            },
            DecisionContract::Hardened,
        )
        .await
    }

    /// Deprecated compatibility contract: decides even when no pending request is
    /// open, taking the client-supplied `requested_by` / `target_ref` on trust.
    /// Retained only for in-process test fixtures in ontology/docs that seed a
    /// decision directly; it has no caller in any `src/`. Every authenticated
    /// surface uses [`Self::decide_pending_approval`].
    ///
    /// When a pending request DOES exist it is still authoritative here — the
    /// `(requester, kind, target)` binding is enforced for both contracts.
    pub async fn decide_approval(
        &self,
        command: DecideApprovalCommand,
    ) -> Result<ApprovalSummary, PgGovernanceError> {
        self.record_decision(command, DecisionContract::DeprecatedCompat)
            .await
    }

    async fn record_decision(
        &self,
        command: DecideApprovalCommand,
        contract: DecisionContract,
    ) -> Result<ApprovalSummary, PgGovernanceError> {
        if !matches!(
            command.decision,
            ApprovalDecision::Approved | ApprovalDecision::Rejected
        ) {
            return Err(KernelError::validation(
                "a four-eyes decision must be 'approved' or 'rejected'",
            )
            .into());
        }
        if command.kind.trim().is_empty() {
            return Err(KernelError::validation("approval kind is required").into());
        }
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let approval_id = Uuid::new_v4();
        let event = governance_audit_event(
            "governance.approval.decide",
            command.approver,
            "gov_approval",
            approval_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org)
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "request_ref": command.request_ref,
                "kind": command.kind,
                "decision": command.decision.as_db_str(),
            })),
        );

        with_audit::<_, ApprovalSummary, PgGovernanceError>(&self.pool, event, |tx| {
            Box::pin(async move {
                // If a pending request exists for this ref, IT is authoritative for
                // every field a §16 gate later matches on — never the
                // client-supplied values — so an approver can neither spoof the
                // requester to dodge the self-approval bar, nor redirect the
                // binding target, nor swap the kind to open a gate that was never
                // requested. Read it in THIS tx (RLS-armed, TOCTOU-safe).
                let pending =
                    pending_request_binding_conn(tx.as_mut(), command.request_ref).await?;
                let requires_open_request = match contract {
                    DecisionContract::Hardened => true,
                    DecisionContract::DeprecatedCompat => false,
                };
                if requires_open_request && pending.is_none() {
                    return Err(KernelError::conflict(
                        "a four-eyes decision requires an open pending approval request",
                    )
                    .into());
                }
                let requested_by = pending
                    .as_ref()
                    .map_or(command.requested_by, |p| p.requested_by);
                let target_ref = match pending.as_ref() {
                    // The open row is authority for the target INCLUDING when that
                    // target is NULL (0164 supports NULL-target create-style
                    // requests). Falling back to the command there would bind the
                    // approval to an object the requester never named.
                    Some(pending) => pending.target_ref,
                    None => command.target_ref,
                };
                // `kind` is rejected on mismatch rather than silently overridden:
                // the audit event's payload was snapshotted from the command before
                // this closure runs, so overriding would leave the audit row and the
                // approval row disagreeing about which gate was opened.
                if let Some(pending) = pending.as_ref()
                    && pending.kind != command.kind.trim()
                {
                    return Err(KernelError::conflict(format!(
                        "approval kind {:?} does not match the open request's kind {:?}",
                        command.kind.trim(),
                        pending.kind
                    ))
                    .into());
                }
                // Self-approval is blocked here (fast, clear error) and at the DB
                // CHECK (`approver_id <> requested_by`). Defense in depth.
                if command.approver == requested_by {
                    return Err(KernelError::forbidden(
                        "self-approval is not allowed: approver must differ from requester",
                    )
                    .into());
                }
                // Company/HR/Payroll: distinct *natural persons*, not merely
                // distinct accounts. Same Person under two capacities is denied;
                // unbound/service (non-person) parties fail closed.
                enforce_natural_person_four_eyes_conn(
                    tx.as_mut(),
                    command.kind.trim(),
                    requested_by,
                    command.approver,
                )
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO gov_approvals
                        (id, org_id, request_ref, kind, requested_by, approver_id, target_ref, decision, decided_at)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                    "#,
                )
                .bind(approval_id)
                .bind(org_uuid)
                .bind(command.request_ref)
                .bind(command.kind.trim())
                .bind(*requested_by.as_uuid())
                .bind(*command.approver.as_uuid())
                .bind(target_ref)
                .bind(command.decision.as_db_str())
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;
                approval_row_conn(tx.as_mut(), approval_id).await
            })
        })
        .await
    }

    // -- §15 lifecycle FSM config -------------------------------------------

    pub async fn configure_transition(
        &self,
        command: ConfigureTransitionCommand,
    ) -> Result<LifecycleTransitionConfig, PgGovernanceError> {
        // The configured edge can only be a subset of the base FSM.
        validate_lifecycle_transition(command.from_state, command.to_state)?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = governance_audit_event(
            "governance.lifecycle.configure",
            command.actor,
            "gov_lifecycle_transition",
            format!(
                "{}:{}->{}",
                command.object_type_id,
                command.from_state.as_db_str(),
                command.to_state.as_db_str()
            ),
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);
        let requirements = command.requirements;

        with_audit::<_, LifecycleTransitionConfig, PgGovernanceError>(&self.pool, event, |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    INSERT INTO gov_lifecycle_transitions
                        (org_id, object_type_id, from_state, to_state,
                         requires_reason, requires_four_eyes, requires_checklist, created_by)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    ON CONFLICT (org_id, object_type_id, from_state, to_state) DO UPDATE
                    SET requires_reason    = EXCLUDED.requires_reason,
                        requires_four_eyes = EXCLUDED.requires_four_eyes,
                        requires_checklist = EXCLUDED.requires_checklist,
                        updated_at         = now()
                    "#,
                )
                .bind(org_uuid)
                .bind(command.object_type_id)
                .bind(command.from_state.as_db_str())
                .bind(command.to_state.as_db_str())
                .bind(requirements.requires_reason)
                .bind(requirements.requires_four_eyes)
                .bind(requirements.requires_checklist)
                .bind(*command.actor.as_uuid())
                .execute(tx.as_mut())
                .await?;
                Ok(LifecycleTransitionConfig {
                    object_type_id: command.object_type_id,
                    from_state: command.from_state,
                    to_state: command.to_state,
                    requirements,
                })
            })
        })
        .await
    }

    /// Read the configured requirements for one edge. `None` means the edge is
    /// not configured for this object type — callers must treat an unconfigured
    /// edge as **not permitted** (fail-closed), even if the base FSM allows it.
    pub async fn transition_requirements(
        &self,
        object_type_id: Uuid,
        from_state: LifecycleState,
        to_state: LifecycleState,
    ) -> Result<Option<TransitionRequirements>, PgGovernanceError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgGovernanceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                transition_requirements_conn(tx.as_mut(), object_type_id, from_state, to_state)
                    .await
            })
        })
        .await
    }

    // -- §16 lifecycle preflight (read-only) --------------------------------

    /// Evaluate the §16 gate chain for one lifecycle edge and COMMIT NOTHING.
    ///
    /// This is the whole of `POST /api/v1/governance/lifecycle/preflight` below
    /// authorization; the handler adds only auth and JSON shaping. Keeping the
    /// body here rather than in the handler is what lets
    /// `governance_rls_as_runtime_role::preflight_writes_no_row_in_any_table`
    /// assert its zero-row-delta against the code that actually ships instead of
    /// against a re-assembled copy of the chain.
    ///
    /// Every step is a read: base-FSM validation, the configured requirements,
    /// a NON-consuming four-eyes peek, and the pure chain evaluation. No receipt,
    /// no approval consumption, no audit row, no outbox row.
    pub async fn lifecycle_preflight(
        &self,
        query: LifecyclePreflightQuery,
    ) -> Result<LifecyclePreflight, PgGovernanceError> {
        // Base-FSM check first: an illegal edge can never preflight to allow.
        validate_lifecycle_transition(query.from_state, query.to_state)?;

        // An unconfigured edge is fail-closed: report not-configured with a
        // denying authority gate so the caller cannot proceed.
        let (configured, reqs) = match self
            .transition_requirements(query.object_type_id, query.from_state, query.to_state)
            .await?
        {
            Some(reqs) => (true, reqs),
            None => (
                false,
                TransitionRequirements {
                    requires_reason: false,
                    requires_four_eyes: false,
                    requires_checklist: false,
                },
            ),
        };

        // Lifecycle transitions always pass the Authority gate; four-eyes/checklist
        // are required per the configured flags. Egress/DLP is not part of a pure
        // lifecycle transition (it gates outbound action side-effects).
        let config = GateChainConfig {
            authority: true,
            self_checklist: reqs.requires_checklist,
            four_eyes: reqs.requires_four_eyes,
            egress_dlp: false,
        };

        // Four-eyes evidence comes from the DB, never from the client. This is an
        // advisory, config-level preview (no concrete instance), so it peeks the
        // approval bound to the object type; the enforcing gate in the ontology
        // lifecycle writeback binds to the specific instance and consumes
        // single-use.
        let four_eyes_approved = match query.four_eyes_request_ref {
            Some(request_ref) => {
                self.four_eyes_approved(
                    request_ref,
                    LIFECYCLE_FOUR_EYES_KIND,
                    Some(query.object_type_id),
                )
                .await?
            }
            None => None,
        };

        let evidence = GateEvidence {
            // Unconfigured edge ⇒ force an authority deny so the outcome cannot allow.
            authority: if configured {
                query.authority_allow.map(|allow| {
                    if allow {
                        AuthorityEffect::Allow
                    } else {
                        AuthorityEffect::Deny
                    }
                })
            } else {
                Some(AuthorityEffect::Deny)
            },
            checklist_all_acknowledged: query.checklist_all_acknowledged,
            four_eyes_approved,
            egress_cleared: query.egress_cleared,
        };

        Ok(LifecyclePreflight {
            configured,
            config,
            outcome: evaluate_gate_chain(config, &evidence),
        })
    }

    /// Four-eyes evidence for a request, read under the armed org — a NON-consuming
    /// peek for preview/preflight only (the committing gate uses
    /// [`four_eyes_consume_conn`]). Binds to the action: the approval must match the
    /// server-derived `expected_kind` and `expected_target` and be unconsumed.
    ///
    /// `Some(true)`  — a matching `approved`, unconsumed decision by a distinct
    ///                 principal exists.
    /// `Some(false)` — no matching unconsumed approval (wrong kind/target, not yet
    ///                 approved, or already consumed) — the gate fails closed.
    pub async fn four_eyes_approved(
        &self,
        request_ref: Uuid,
        expected_kind: &str,
        expected_target: Option<Uuid>,
    ) -> Result<Option<bool>, PgGovernanceError> {
        let org = current_org().map_err(KernelError::from)?;
        let expected_kind = expected_kind.to_owned();
        with_org_conn::<_, _, PgGovernanceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                four_eyes_check_conn(tx.as_mut(), request_ref, &expected_kind, expected_target)
                    .await
            })
        })
        .await
    }

    /// Bind-match AND consume a four-eyes approval in one committed step — for
    /// gated actions whose mutation runs in a tx this crate does not own (docs hold
    /// release, projected dispatch). Returns `Some(true)` iff a matching approval
    /// was newly consumed; `Some(false)` if none matched or it was already consumed
    /// (replay). Callers with their own write tx MUST use [`four_eyes_consume_conn`]
    /// so the consumption is atomic with the mutation.
    pub async fn four_eyes_consume(
        &self,
        request_ref: Uuid,
        expected_kind: &str,
        expected_target: Option<Uuid>,
        consumed_by: UserId,
    ) -> Result<Option<bool>, PgGovernanceError> {
        let org = current_org().map_err(KernelError::from)?;
        let expected_kind = expected_kind.to_owned();
        with_org_conn::<_, _, PgGovernanceError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                four_eyes_consume_conn(
                    tx.as_mut(),
                    request_ref,
                    &expected_kind,
                    expected_target,
                    consumed_by,
                )
                .await
            })
        })
        .await
    }
}

// The `&mut PgConnection` readers below are shared by the `with_audit` /
// `with_org_conn` closures (pass `tx.as_mut()`). The gated-action lanes call
// `four_eyes_consume_conn` inside their OWN writeback transaction to bind-match AND
// consume the four-eyes approval in the same tx as the mutation (TOCTOU-safe,
// single-use); `four_eyes_check_conn` is the non-consuming peek for previews.

async fn override_row_conn(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<OverrideSummary, PgGovernanceError> {
    let row = sqlx::query(
        r#"
        SELECT id, target_type, target_id, actor, reason, before_snapshot, created_at
        FROM gov_overrides WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_one(conn)
    .await?;
    Ok(OverrideSummary {
        id: row.try_get("id")?,
        target_type: row.try_get("target_type")?,
        target_id: row.try_get("target_id")?,
        actor: UserId::from_uuid(row.try_get("actor")?),
        reason: row.try_get("reason")?,
        before_snapshot: row.try_get("before_snapshot")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn approval_request_row_conn(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<ApprovalRequestSummary, PgGovernanceError> {
    let row = sqlx::query(
        r#"
        SELECT id, request_ref, kind, requested_by, payload_summary, created_at
        FROM gov_approval_requests WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_one(conn)
    .await?;
    Ok(ApprovalRequestSummary {
        id: row.try_get("id")?,
        request_ref: row.try_get("request_ref")?,
        kind: row.try_get("kind")?,
        requested_by: UserId::from_uuid(row.try_get("requested_by")?),
        payload_summary: row.try_get("payload_summary")?,
        created_at: row.try_get("created_at")?,
    })
}

/// The authoritative binding of a pending approval request, if one is open for
/// `request_ref`. `None` = no pending request (decide falls back to the
/// client-supplied values). RLS-scoped by the caller's armed org.
///
/// These are ALL THREE fields the §16 gate matches on
/// (`0164_bind_consume_four_eyes.sql`: `(request_ref, kind, target_ref)`) plus the
/// requester the approver must differ from. Anything the gate binds on must be
/// read from here rather than from the approver's command, or the approver
/// chooses which gate their decision opens.
struct PendingRequestBinding {
    requested_by: UserId,
    kind: String,
    target_ref: Option<Uuid>,
}

async fn pending_request_binding_conn(
    conn: &mut PgConnection,
    request_ref: Uuid,
) -> Result<Option<PendingRequestBinding>, PgGovernanceError> {
    let row = sqlx::query(
        "SELECT requested_by, kind, target_ref FROM gov_approval_requests WHERE request_ref = $1",
    )
    .bind(request_ref)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|row| PendingRequestBinding {
        requested_by: UserId::from_uuid(row.get("requested_by")),
        kind: row.get("kind"),
        target_ref: row.get("target_ref"),
    }))
}

/// Resolve a platform account to its bound natural `person_id`, or `None` when
/// the account has no `users.employee_id`, no binding row, or is otherwise not a
/// natural person (service / unbound). RLS-scoped by the armed org.
async fn natural_person_id_conn(
    conn: &mut PgConnection,
    user_id: Uuid,
) -> Result<Option<Uuid>, PgGovernanceError> {
    let person_id: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT b.person_id
        FROM users u
        INNER JOIN employee_person_bindings b
            ON b.org_id = u.org_id
           AND b.employee_id = u.employee_id
        WHERE u.id = $1
          AND u.employee_id IS NOT NULL
        "#,
    )
    .bind(user_id)
    .fetch_optional(conn)
    .await?;
    Ok(person_id)
}

/// For Company/HR/Payroll kinds, the named party must resolve to a Person.
/// No-op for every other kind (generic four-eyes path).
async fn require_natural_person_bound_conn(
    conn: &mut PgConnection,
    kind: &str,
    user: UserId,
    role: &str,
) -> Result<(), PgGovernanceError> {
    if !requires_natural_person_four_eyes(kind) {
        return Ok(());
    }
    match natural_person_id_conn(conn, *user.as_uuid()).await? {
        Some(_) => Ok(()),
        None => Err(KernelError::forbidden(format!(
            "natural-person four-eyes: {role} is unbound or not a natural person"
        ))
        .into()),
    }
}

/// Deny same-Person (different capacity) and fail closed on unbound parties for
/// Company/HR/Payroll kinds. Account-level self-approval is already rejected
/// before this runs; this layer catches two user_ids that share one Person.
async fn enforce_natural_person_four_eyes_conn(
    conn: &mut PgConnection,
    kind: &str,
    requester: UserId,
    approver: UserId,
) -> Result<(), PgGovernanceError> {
    if !requires_natural_person_four_eyes(kind) {
        return Ok(());
    }
    let requester_person = natural_person_id_conn(conn, *requester.as_uuid())
        .await?
        .ok_or_else(|| {
            KernelError::forbidden(
                "natural-person four-eyes: requester is unbound or not a natural person",
            )
        })?;
    let approver_person = natural_person_id_conn(conn, *approver.as_uuid())
        .await?
        .ok_or_else(|| {
            KernelError::forbidden(
                "natural-person four-eyes: approver is unbound or not a natural person",
            )
        })?;
    if requester_person == approver_person {
        return Err(KernelError::forbidden(
            "natural-person four-eyes: approver and requester resolve to the same Person",
        )
        .into());
    }
    Ok(())
}

async fn approval_row_conn(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<ApprovalSummary, PgGovernanceError> {
    let row = sqlx::query(
        r#"
        SELECT id, request_ref, kind, requested_by, approver_id, decision, decided_at
        FROM gov_approvals WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_one(conn)
    .await?;
    let decision: String = row.try_get("decision")?;
    Ok(ApprovalSummary {
        id: row.try_get("id")?,
        request_ref: row.try_get("request_ref")?,
        kind: row.try_get("kind")?,
        requested_by: UserId::from_uuid(row.try_get("requested_by")?),
        approver_id: UserId::from_uuid(row.try_get("approver_id")?),
        decision: ApprovalDecision::from_db_str(&decision)?,
        decided_at: row.try_get("decided_at")?,
    })
}

async fn transition_requirements_conn(
    conn: &mut PgConnection,
    object_type_id: Uuid,
    from_state: LifecycleState,
    to_state: LifecycleState,
) -> Result<Option<TransitionRequirements>, PgGovernanceError> {
    let row = sqlx::query(
        r#"
        SELECT requires_reason, requires_four_eyes, requires_checklist
        FROM gov_lifecycle_transitions
        WHERE object_type_id = $1 AND from_state = $2 AND to_state = $3
        "#,
    )
    .bind(object_type_id)
    .bind(from_state.as_db_str())
    .bind(to_state.as_db_str())
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|row| TransitionRequirements {
        requires_reason: row.get("requires_reason"),
        requires_four_eyes: row.get("requires_four_eyes"),
        requires_checklist: row.get("requires_checklist"),
    }))
}

// The bound-approval predicate below is shared, word-for-word, by the peek and the
// consume: an `approved` decision for THIS `request_ref` whose recorded (kind,
// target) match the action's server-derived `expected_kind` / `expected_target`,
// and which has not already been consumed. `target_ref IS NOT DISTINCT FROM $3`
// matches a NULL expected target against a NULL bound target and nothing else — a
// legacy/unbound row never satisfies a target-bound gate. RLS scopes every row to
// the caller's org. It is inlined as a literal in each query (not a `const` +
// `format!`) because sqlx only accepts `&'static str` SQL.

/// Non-consuming peek: does a matching, unconsumed, approved decision exist? For
/// preview/preflight only. `Some(true)` = the gate would pass now; `Some(false)` =
/// it would fail closed (wrong kind/target, unapproved, or already consumed).
pub async fn four_eyes_check_conn(
    conn: &mut PgConnection,
    request_ref: Uuid,
    expected_kind: &str,
    expected_target: Option<Uuid>,
) -> Result<Option<bool>, PgGovernanceError> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM gov_approvals a
            WHERE a.request_ref = $1
              AND a.kind = $2
              AND a.target_ref IS NOT DISTINCT FROM $3
              AND a.decision = 'approved'
              AND NOT EXISTS (SELECT 1 FROM gov_approval_consumptions c WHERE c.approval_id = a.id)
        )
        "#,
    )
    .bind(request_ref)
    .bind(expected_kind)
    .bind(expected_target)
    .fetch_one(conn)
    .await?;
    Ok(Some(exists))
}

/// Bind-match AND consume in ONE statement, inside the caller's write tx so the
/// consumption is atomic with the gated mutation (a rolled-back action un-consumes
/// the approval). The `INSERT … SELECT … ON CONFLICT DO NOTHING RETURNING` both
/// enforces the binding predicate and makes the single-use atomic: two concurrent
/// consumers of the same approval serialize on the `(org_id, approval_id)` unique
/// index and exactly one gets a returned row — the other sees `Some(false)` (replay
/// denied). `Some(true)` = newly consumed (gate passes); `Some(false)` = no match or
/// already consumed (gate fails closed).
pub async fn four_eyes_consume_conn(
    conn: &mut PgConnection,
    request_ref: Uuid,
    expected_kind: &str,
    expected_target: Option<Uuid>,
    consumed_by: UserId,
) -> Result<Option<bool>, PgGovernanceError> {
    let consumed: Option<Uuid> = sqlx::query_scalar(
        r#"
        INSERT INTO gov_approval_consumptions (org_id, approval_id, consumed_by)
        SELECT a.org_id, a.id, $4
        FROM gov_approvals a
        WHERE a.request_ref = $1
          AND a.kind = $2
          AND a.target_ref IS NOT DISTINCT FROM $3
          AND a.decision = 'approved'
          AND NOT EXISTS (SELECT 1 FROM gov_approval_consumptions c WHERE c.approval_id = a.id)
        ON CONFLICT (org_id, approval_id) DO NOTHING
        RETURNING approval_id
        "#,
    )
    .bind(request_ref)
    .bind(expected_kind)
    .bind(expected_target)
    .bind(*consumed_by.as_uuid())
    .fetch_optional(conn)
    .await?;
    Ok(Some(consumed.is_some()))
}
