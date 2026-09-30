//! Support-ticket application layer: commands, query DTOs, read models, audit
//! event builders, and the notification port.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, CustomerId, EvidenceObjectId, KernelError,
    OrgId, SiteId, SupportTicketCommentId, SupportTicketId, Timestamp, TraceContext, UserId,
    WorkOrderId,
};
use console_support_domain::{
    AcceptanceChannel, AcceptanceKind, CaseEvent, CaseHistoryEntry, CaseScope,
    DispatchHandoffStatus, FieldSlaState, SupportCase, TicketCategory, TicketOrigin,
    TicketPriority, TicketStatus,
};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Open a ticket as an authenticated staff member. The ticket inherits the
/// requester's branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInternalTicketCommand {
    pub actor: UserId,
    pub branch_id: BranchId,
    pub category: TicketCategory,
    pub priority: TicketPriority,
    pub title: String,
    pub body: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Open a ticket from the unauthenticated customer intake channel. There is no
/// actor and no branch; the customer supplies a name and contact (PII).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCustomerIntakeCommand {
    pub category: TicketCategory,
    pub priority: TicketPriority,
    pub title: String,
    pub body: String,
    pub requester_name: String,
    /// Customer contact PII (phone/email). Never logged; never audited.
    pub requester_contact: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Assign (or reassign) a ticket to a staff member and triage a branch-less
/// customer ticket into a branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignTicketCommand {
    pub actor: UserId,
    pub ticket_id: SupportTicketId,
    pub assignee_user_id: UserId,
    /// Branch to triage a branch-less customer ticket into. Required when the
    /// ticket has no branch yet; ignored once a ticket already carries a branch.
    pub branch_id: Option<BranchId>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Drive the status FSM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionTicketCommand {
    pub actor: UserId,
    pub ticket_id: SupportTicketId,
    pub to_status: TicketStatus,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Append a comment. `is_internal_note` marks a staff-only note that the
/// customer-visible read path never returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddCommentCommand {
    pub actor: UserId,
    pub ticket_id: SupportTicketId,
    pub body: String,
    pub is_internal_note: bool,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Bind a ticket to the field object chain: a customer site and/or the work
/// order dispatched for the visit. `Some(None)` clears a link (explicit JSON
/// `null`); `None` leaves it untouched. Linking a site to an untriaged CUSTOMER
/// ticket also sets `branch_id` from the site — that IS triage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkTicketCommand {
    pub actor: UserId,
    pub ticket_id: SupportTicketId,
    /// Principal's branch scope: the site lookup is confined to it so a
    /// branch-scoped manager can only link sites they can see (deny-by-omission).
    pub branch_scope: BranchScope,
    pub site_id: Option<Option<SiteId>>,
    pub work_order_id: Option<Option<WorkOrderId>>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Record the customer's acceptance verdict for a RESOLVED ticket — the audited
/// closure evidence of the field story. Drives the existing FSM edges
/// (accept ⇒ CLOSED, decline ⇒ reopen IN_PROGRESS + customer-visible comment).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordAcceptanceCommand {
    pub actor: UserId,
    pub ticket_id: SupportTicketId,
    pub kind: AcceptanceKind,
    pub channel: AcceptanceChannel,
    /// Customer-side acknowledger name — a business fact like `requester_name`;
    /// stored, never logged to traces or audit snapshots.
    pub accepted_by: String,
    pub note: Option<String>,
    /// `Idempotency-Key` header value (16..=200 chars, sibling-pilot semantics):
    /// a replay with the same key + fingerprint returns the stored acceptance;
    /// reuse with a different request is a conflict.
    pub idempotency_key: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

// ---------------------------------------------------------------------------
// Internal case lifecycle commands and transactional ports
// ---------------------------------------------------------------------------

/// Server-derived identity and tenant/branch scope for internal case writes.
/// The REST ticket contract deliberately does not expose this type yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportCaseActorContext {
    pub org_id: OrgId,
    pub user_id: UserId,
    pub branch_scope: BranchScope,
}

/// Common mutation metadata. The adapter binds `actor` to the authenticated
/// principal, checks the tenant before this layer runs, and persists one
/// receipt per idempotency key inside the same transaction as the case change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportCaseCommandMetadata {
    pub actor: UserId,
    pub expected_version: u64,
    pub idempotency_key: String,
    pub fingerprint: String,
    pub trace: TraceContext,
}

impl SupportCaseCommandMetadata {
    pub fn validate_preflight(&self, context: &SupportCaseActorContext) -> Result<(), KernelError> {
        if self.actor != context.user_id {
            return Err(KernelError::forbidden(
                "support case command actor must match authenticated principal",
            ));
        }
        if !(16..=200).contains(&self.idempotency_key.trim().len()) {
            return Err(KernelError::validation(
                "support case idempotency key must be 16..=200 characters",
            ));
        }
        if self.fingerprint.trim().is_empty() || self.fingerprint.len() > 128 {
            return Err(KernelError::validation(
                "support case fingerprint is required and bounded",
            ));
        }
        Ok(())
    }

    pub fn validate(
        &self,
        context: &SupportCaseActorContext,
        branch_id: BranchId,
    ) -> Result<(), KernelError> {
        self.validate_preflight(context)?;
        if !context.branch_scope.allows(branch_id) {
            return Err(KernelError::forbidden(
                "support case branch is outside authenticated scope",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestDispatchHandoffCommand {
    pub case_id: SupportTicketId,
    pub work_order_id: WorkOrderId,
    pub metadata: SupportCaseCommandMetadata,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveDispatchHandoffCommand {
    pub case_id: SupportTicketId,
    pub status: DispatchHandoffStatus,
    pub metadata: SupportCaseCommandMetadata,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindCaseEvidenceCommand {
    pub case_id: SupportTicketId,
    pub evidence_object_id: EvidenceObjectId,
    pub metadata: SupportCaseCommandMetadata,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportCaseIdempotency {
    Execute,
    Replay,
}

pub fn support_case_idempotency(
    existing_fingerprint: Option<&str>,
    metadata: &SupportCaseCommandMetadata,
) -> Result<SupportCaseIdempotency, KernelError> {
    match existing_fingerprint {
        None => Ok(SupportCaseIdempotency::Execute),
        Some(existing) if existing == metadata.fingerprint => Ok(SupportCaseIdempotency::Replay),
        Some(_) => Err(KernelError::conflict(
            "support case idempotency key reused with a different payload",
        )),
    }
}

/// Durable outbox payload. `dedupe_key` is derived from immutable case version
/// and event name, never from an invented messenger, mail, or report URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportCaseOutboxIntent {
    pub case_id: SupportTicketId,
    pub version: u64,
    pub dedupe_key: String,
    pub event: CaseEvent,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: Timestamp,
}

impl SupportCaseOutboxIntent {
    #[must_use]
    pub fn from_history(case_id: SupportTicketId, entry: &CaseHistoryEntry) -> Self {
        Self {
            case_id,
            version: entry.version,
            dedupe_key: format!(
                "support-case:{case_id}:{}:{}",
                entry.version,
                entry.event.name()
            ),
            event: entry.event.clone(),
            occurred_at: entry.occurred_at,
        }
    }
}

/// Boxed asynchronous result used by Support persistence ports. The application
/// layer remains runtime-agnostic: adapters select the executor and database client.
pub type SupportCaseFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, KernelError>> + Send + 'a>>;

/// The external bounded contexts answer only existence and same-tenant/branch
/// visibility. They do not mutate Work Order or Evidence state from Support.
pub trait SupportCaseLinkVerifier {
    fn verify_work_order<'a>(
        &'a mut self,
        scope: CaseScope,
        work_order_id: WorkOrderId,
    ) -> SupportCaseFuture<'a, ()>;
    fn verify_evidence_object<'a>(
        &'a mut self,
        scope: CaseScope,
        evidence_object_id: EvidenceObjectId,
    ) -> SupportCaseFuture<'a, ()>;
}

/// Atomic Support persistence intent. Adapters must persist the selected
/// variant and its receipt in one tenant-scoped transaction.
// 968 vs 648 bytes. Boxing the larger variant is the clippy-suggested fix, but
// it is a public type-shape change affecting every constructor and match arm,
// which is out of scope for a lint pass. Left for the owning lane to decide.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupportCaseCommit {
    /// A state mutation: projection, append-only history, outbox, audit, and
    /// receipt must commit together using `expected_version` as CAS.
    Mutation(SupportCaseMutationCommit),
    /// A semantically idempotent command that made no state change. Only its
    /// exact receipt and an explicit no-op audit event are durable; projection,
    /// history, and outbox remain untouched.
    ReceiptOnly(SupportCaseReceiptOnlyCommit),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportCaseMutationCommit {
    /// Compare-and-swap token captured from the authenticated command.
    pub expected_version: u64,
    pub case: SupportCase,
    pub history: CaseHistoryEntry,
    pub outbox: SupportCaseOutboxIntent,
    /// Append-only audit intent persisted in the same transaction as all other
    /// case effects. Its trace is the command trace, never adapter-generated.
    pub audit: AuditEvent,
    pub action: &'static str,
    pub idempotency_key: String,
    pub fingerprint: String,
    pub result: SupportCaseMutationResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportCaseReceiptOnlyCommit {
    pub scope: CaseScope,
    pub case_id: SupportTicketId,
    /// Audit action explicitly denotes that the command was accepted as a
    /// no-op, so it cannot be mistaken for a lifecycle mutation.
    pub audit: AuditEvent,
    pub action: &'static str,
    pub idempotency_key: String,
    pub fingerprint: String,
    pub result: SupportCaseMutationResult,
}

/// Exact response stored with an idempotency key. Replays never synthesize a
/// version from a newer caller payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportCaseIdempotencyReceipt {
    pub fingerprint: String,
    pub result: SupportCaseMutationResult,
}

/// Adapter transaction contract. `commit` must atomically persist exactly one
/// [`SupportCaseCommit`] variant and its idempotency receipt. Mutation commits
/// write projection/history/outbox/audit with CAS; receipt-only commits write
/// no projection, history, or outbox rows.
pub trait SupportCaseUnitOfWork: SupportCaseLinkVerifier {
    fn idempotency_receipt<'a>(
        &'a mut self,
        scope: CaseScope,
        case_id: SupportTicketId,
        actor: UserId,
        action: &'a str,
        key: &'a str,
    ) -> SupportCaseFuture<'a, Option<SupportCaseIdempotencyReceipt>>;
    fn load_case_for_update<'a>(
        &'a mut self,
        case_id: SupportTicketId,
    ) -> SupportCaseFuture<'a, SupportCase>;
    fn commit<'a>(&'a mut self, commit: SupportCaseCommit) -> SupportCaseFuture<'a, ()>;
}

/// Repository transaction boundary. `org_id` is explicit authority that the
/// adapter must arm as tenant RLS context before the callback can load a case.
/// The higher-ranked callback ties every operation to that transaction borrow,
/// preventing it from escaping or mixing identities across async awaits.
pub trait SupportCaseRepository {
    type Transaction: SupportCaseUnitOfWork + Send;

    fn transaction<'a, T, F>(&'a mut self, org_id: OrgId, operation: F) -> SupportCaseFuture<'a, T>
    where
        T: Send + 'a,
        F: for<'tx> FnOnce(&'tx mut Self::Transaction) -> SupportCaseFuture<'tx, T> + Send + 'a;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportCaseMutationResult {
    pub case_id: SupportTicketId,
    pub version: u64,
    pub replayed: bool,
}

pub async fn request_dispatch_handoff<R: SupportCaseRepository>(
    repository: &mut R,
    context: &SupportCaseActorContext,
    command: RequestDispatchHandoffCommand,
) -> Result<SupportCaseMutationResult, KernelError> {
    command.metadata.validate_preflight(context)?;
    let context = context.clone();
    repository
        .transaction(context.org_id, |tx| {
            Box::pin(async move {
                const ACTION: &str = "support.case.dispatch_handoff.request";
                let mut case = tx.load_case_for_update(command.case_id).await?;
                validate_case_context(&context, &command.metadata, &case)?;
                if let Some(result) =
                    replayed_case_result(tx, &case, context.user_id, ACTION, &command.metadata)
                        .await?
                {
                    return Ok(result);
                }
                tx.verify_work_order(case.scope(), command.work_order_id)
                    .await?;
                let changed = case.request_dispatch_handoff(
                    command.metadata.expected_version,
                    command.metadata.actor,
                    command.work_order_id,
                    command.occurred_at,
                )?;
                if !changed {
                    return commit_case_receipt_only(
                        tx,
                        case,
                        ACTION,
                        "support.case.dispatch_handoff.request_noop",
                        command.metadata,
                        command.occurred_at,
                    )
                    .await;
                }
                commit_case_change(tx, case, ACTION, command.metadata).await
            })
        })
        .await
}

pub async fn bind_case_evidence<R: SupportCaseRepository>(
    repository: &mut R,
    context: &SupportCaseActorContext,
    command: BindCaseEvidenceCommand,
) -> Result<SupportCaseMutationResult, KernelError> {
    command.metadata.validate_preflight(context)?;
    let context = context.clone();
    repository
        .transaction(context.org_id, |tx| {
            Box::pin(async move {
                const ACTION: &str = "support.case.evidence.bind";
                let mut case = tx.load_case_for_update(command.case_id).await?;
                validate_case_context(&context, &command.metadata, &case)?;
                if let Some(result) =
                    replayed_case_result(tx, &case, context.user_id, ACTION, &command.metadata)
                        .await?
                {
                    return Ok(result);
                }
                tx.verify_evidence_object(case.scope(), command.evidence_object_id)
                    .await?;
                let changed = case.bind_evidence(
                    command.metadata.expected_version,
                    command.metadata.actor,
                    command.evidence_object_id,
                    command.occurred_at,
                )?;
                if !changed {
                    return commit_case_receipt_only(
                        tx,
                        case,
                        ACTION,
                        "support.case.evidence.bind_noop",
                        command.metadata,
                        command.occurred_at,
                    )
                    .await;
                }
                commit_case_change(tx, case, ACTION, command.metadata).await
            })
        })
        .await
}

pub async fn resolve_dispatch_handoff<R: SupportCaseRepository>(
    repository: &mut R,
    context: &SupportCaseActorContext,
    command: ResolveDispatchHandoffCommand,
) -> Result<SupportCaseMutationResult, KernelError> {
    command.metadata.validate_preflight(context)?;
    let context = context.clone();
    repository
        .transaction(context.org_id, |tx| {
            Box::pin(async move {
                const ACTION: &str = "support.case.dispatch_handoff.resolve";
                let mut case = tx.load_case_for_update(command.case_id).await?;
                validate_case_context(&context, &command.metadata, &case)?;
                if let Some(result) =
                    replayed_case_result(tx, &case, context.user_id, ACTION, &command.metadata)
                        .await?
                {
                    return Ok(result);
                }
                case.resolve_dispatch_handoff(
                    command.metadata.expected_version,
                    command.metadata.actor,
                    command.status,
                    command.occurred_at,
                )?;
                commit_case_change(tx, case, ACTION, command.metadata).await
            })
        })
        .await
}

fn validate_case_context(
    context: &SupportCaseActorContext,
    metadata: &SupportCaseCommandMetadata,
    case: &SupportCase,
) -> Result<(), KernelError> {
    if context.org_id != case.scope().org_id {
        return Err(KernelError::forbidden(
            "support case tenant does not match authenticated principal",
        ));
    }
    metadata.validate(context, case.scope().branch_id)
}

async fn replayed_case_result<T: SupportCaseUnitOfWork>(
    tx: &mut T,
    case: &SupportCase,
    actor: UserId,
    action: &str,
    metadata: &SupportCaseCommandMetadata,
) -> Result<Option<SupportCaseMutationResult>, KernelError> {
    let Some(receipt) = tx
        .idempotency_receipt(
            case.scope(),
            case.id(),
            actor,
            action,
            &metadata.idempotency_key,
        )
        .await?
    else {
        return Ok(None);
    };
    support_case_idempotency(Some(&receipt.fingerprint), metadata)?;
    Ok(Some(SupportCaseMutationResult {
        replayed: true,
        ..receipt.result
    }))
}

async fn commit_case_change<T: SupportCaseUnitOfWork>(
    tx: &mut T,
    case: SupportCase,
    action: &'static str,
    metadata: SupportCaseCommandMetadata,
) -> Result<SupportCaseMutationResult, KernelError> {
    let history = case.history().last().cloned().ok_or_else(|| {
        KernelError::validation("support case mutation must append durable history")
    })?;
    let outbox = SupportCaseOutboxIntent::from_history(case.id(), &history);
    let result = SupportCaseMutationResult {
        case_id: case.id(),
        version: case.version(),
        replayed: false,
    };
    let audit = support_audit_event(
        action,
        Some(metadata.actor),
        Some(case.scope().branch_id),
        "support_case",
        case.id(),
        metadata.trace.clone(),
        history.occurred_at,
    )?
    .with_org(case.scope().org_id);
    tx.commit(SupportCaseCommit::Mutation(SupportCaseMutationCommit {
        expected_version: metadata.expected_version,
        case,
        history,
        outbox,
        audit,
        action,
        idempotency_key: metadata.idempotency_key,
        fingerprint: metadata.fingerprint,
        result: result.clone(),
    }))
    .await?;
    Ok(result)
}

async fn commit_case_receipt_only<T: SupportCaseUnitOfWork>(
    tx: &mut T,
    case: SupportCase,
    action: &'static str,
    audit_action: &'static str,
    metadata: SupportCaseCommandMetadata,
    occurred_at: Timestamp,
) -> Result<SupportCaseMutationResult, KernelError> {
    let result = SupportCaseMutationResult {
        case_id: case.id(),
        version: case.version(),
        replayed: false,
    };
    let audit = support_audit_event(
        audit_action,
        Some(metadata.actor),
        Some(case.scope().branch_id),
        "support_case",
        case.id(),
        metadata.trace.clone(),
        occurred_at,
    )?
    .with_org(case.scope().org_id);
    tx.commit(SupportCaseCommit::ReceiptOnly(
        SupportCaseReceiptOnlyCommit {
            scope: case.scope(),
            case_id: case.id(),
            audit,
            action,
            idempotency_key: metadata.idempotency_key,
            fingerprint: metadata.fingerprint,
            result: result.clone(),
        },
    ))
    .await?;
    Ok(result)
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// Branch-scoped list with optional filters. SUPER_ADMIN/EXECUTIVE resolve to
/// `BranchScope::All` for cross-branch rollups (like reporting).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListTicketsQuery {
    pub branch_scope: BranchScope,
    pub status: Option<TicketStatus>,
    pub priority: Option<TicketPriority>,
    pub category: Option<TicketCategory>,
    pub origin: Option<TicketOrigin>,
    pub assignee_user_id: Option<UserId>,
    /// Include branch-less (untriaged customer) tickets in the result. Only
    /// honoured for `BranchScope::All` principals; branch-scoped staff never see
    /// untriaged cross-org intake.
    pub include_untriaged: bool,
    /// Page size. `None` falls back to the adapter default; the adapter always
    /// clamps to `1..=100` so an unbounded fetch is impossible even when the
    /// client sends no limit.
    pub limit: Option<i64>,
    /// Keyset cursor: return only tickets ordered strictly after this id on the
    /// `(created_at DESC, id)` ordering. `None` starts from the first page.
    /// Mirrors the messenger keyset-pagination pattern.
    pub cursor: Option<SupportTicketId>,
    /// Restrict to tickets linked to one customer site (field-console queue).
    pub site_id: Option<SiteId>,
}

/// Branch-scoped field-site overview query (list layer of `/console/field`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListFieldSitesQuery {
    pub branch_scope: BranchScope,
    /// Substring match on site name or customer name.
    pub q: Option<String>,
    pub customer_id: Option<CustomerId>,
    /// Filter on the derived per-site SLA state.
    pub sla: Option<FieldSlaState>,
    /// Page size; the adapter clamps to `1..=100`.
    pub limit: Option<i64>,
    /// Keyset cursor: the id of the last site from the previous page, on the
    /// `(site_name, site_id)` ascending ordering.
    pub cursor: Option<SiteId>,
}

// ---------------------------------------------------------------------------
// Read models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketSummary {
    pub id: SupportTicketId,
    pub branch_id: Option<BranchId>,
    pub origin: TicketOrigin,
    pub category: TicketCategory,
    pub priority: TicketPriority,
    pub status: TicketStatus,
    pub title: String,
    pub requester_user_id: Option<UserId>,
    pub requester_name: Option<String>,
    pub assignee_user_id: Option<UserId>,
    /// Assignee display name, resolved via a same-org LEFT JOIN on `users`.
    /// `None` for an unassigned ticket or a deleted assignee; the web renders
    /// it through `safeLabel` so a missing name never leaks the UUID.
    pub assignee_name: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub due_at: Option<Timestamp>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: Timestamp,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolved_at: Option<Timestamp>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub closed_at: Option<Timestamp>,
    /// Field object chain (0194, all additive): the linked customer site, its
    /// customer (denormalized on link), and the dispatched work order.
    pub site_id: Option<SiteId>,
    pub site_name: Option<String>,
    pub customer_id: Option<CustomerId>,
    pub customer_name: Option<String>,
    pub work_order_id: Option<WorkOrderId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentView {
    pub id: SupportTicketCommentId,
    pub ticket_id: SupportTicketId,
    pub author_user_id: Option<UserId>,
    /// Author display name, resolved via a same-org LEFT JOIN on `users`. `None`
    /// for a system/customer comment with no author or a deleted author; the web
    /// renders it through `safeLabel` so a missing name never leaks the UUID.
    pub author_name: Option<String>,
    pub body: String,
    pub is_internal_note: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketDetail {
    pub ticket: TicketSummary,
    pub comments: Vec<CommentView>,
}

/// One keyset page of tickets plus the unpaged `total` matching the same
/// filters, so the console can show an honest count while still paging via the
/// cursor. `next_cursor` is the id to pass as `cursor` for the next page, or
/// `None` when this is the last page. Mirrors `MessagePage`, with `total` added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketPage {
    pub items: Vec<TicketSummary>,
    pub next_cursor: Option<SupportTicketId>,
    pub total: i64,
}

// ---------------------------------------------------------------------------
// Field read models (customer-site overview / detail — `/console/field`)
// ---------------------------------------------------------------------------

/// One row of the field-site overview: the site, its customer, and the
/// aggregated issue/visit/SLA state — every count derives from the same query
/// as the rows (stat bar honesty, §4-11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSiteRow {
    pub site_id: SiteId,
    pub site_name: String,
    pub branch_id: BranchId,
    pub customer_id: CustomerId,
    pub customer_name: String,
    pub address: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// Tickets in OPEN/IN_PROGRESS/ON_HOLD linked to the site.
    pub open_ticket_count: i64,
    /// Open tickets whose SLA `due_at` has already passed.
    pub breached_ticket_count: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_due_at: Option<Timestamp>,
    /// Work orders for the site in a non-terminal status.
    pub active_work_order_count: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_arrival_at: Option<Timestamp>,
    pub sla: FieldSlaState,
}

/// One keyset page of field sites plus the unpaged `total` for the same
/// filters. `next_cursor` is the id to pass as `cursor` for the next page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSitePage {
    pub items: Vec<FieldSiteRow>,
    pub next_cursor: Option<SiteId>,
    pub total: i64,
}

/// Site master facts on the field detail panel (registry-owned data, read-only
/// projection here; edits go through the registry API).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSiteSummary {
    pub id: SiteId,
    pub name: String,
    pub branch_id: BranchId,
    pub customer_id: CustomerId,
    pub customer_name: String,
    pub address: Option<String>,
    pub province: Option<String>,
    pub city: Option<String>,
    pub postal_code: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub geofence_radius_m: Option<f64>,
    pub contact_name: Option<String>,
    pub contact_phone: Option<String>,
}

/// Per-site SLA rollup: current open/breached state plus the 90-day resolution
/// record (resolved before vs after `due_at`; tickets without a due date are
/// excluded rather than guessed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldSlaSummary {
    pub state: FieldSlaState,
    pub open: i64,
    pub breached: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_due_at: Option<Timestamp>,
    pub resolved_within_sla_90d: i64,
    pub resolved_breached_90d: i64,
}

/// Read-only reference to a work order dispatched to the site. Status/priority/
/// result are the workorder crate's 16-state vocabulary rendered as chips; all
/// mutations go through the workorder API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldWorkOrderRef {
    pub id: WorkOrderId,
    pub request_no: String,
    pub status: String,
    pub priority: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub target_due_at: Option<Timestamp>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub report_submitted_at: Option<Timestamp>,
    pub result_type: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
}

/// A durable check-in/out business fact (`site_attendance_events`, compliance-
/// owned; carries no coordinates and survives consent withdrawal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldAttendanceEvent {
    pub user_id: UserId,
    /// Same-org display-name lookup; `None` for a deleted user.
    pub user_name: Option<String>,
    pub work_order_id: WorkOrderId,
    /// `ARRIVAL` or `DEPARTURE` (compliance vocabulary, read-only here).
    pub kind: String,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: Timestamp,
}

/// Recorded customer acceptance (append-only closure evidence).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketAcceptanceView {
    pub id: uuid::Uuid,
    pub ticket_id: SupportTicketId,
    pub kind: AcceptanceKind,
    pub channel: AcceptanceChannel,
    pub accepted_by: String,
    pub note: Option<String>,
    pub recorded_by_user_id: UserId,
    /// Same-org display-name lookup; `None` for a deleted recorder.
    pub recorded_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: Timestamp,
}

/// Object + history layers of the field detail panel: the site, its SLA rollup,
/// and the traversable downstream chain (tickets, work orders, attendance,
/// acceptances — each capped at 50, most relevant first).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSiteDetail {
    pub site: FieldSiteSummary,
    pub sla: FieldSlaSummary,
    pub tickets: Vec<TicketSummary>,
    pub work_orders: Vec<FieldWorkOrderRef>,
    pub attendance: Vec<FieldAttendanceEvent>,
    pub acceptances: Vec<TicketAcceptanceView>,
}

/// Audience filter for [`TicketDetail`] reads. The customer-visible path drops
/// internal staff notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentAudience {
    /// Staff: all comments, including internal notes.
    Internal,
    /// Customer-visible: internal notes are excluded.
    CustomerVisible,
}

impl CommentAudience {
    /// Whether a comment with the given `is_internal_note` flag is visible to
    /// this audience.
    #[must_use]
    pub const fn shows_internal_notes(self) -> bool {
        matches!(self, Self::Internal)
    }
}

// ---------------------------------------------------------------------------
// Notification port
// ---------------------------------------------------------------------------

/// Why a notification is being raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TicketNotificationKind {
    /// A ticket was (re)assigned — notify the new assignee.
    Assigned,
    /// The status changed — notify the requester (if internal) and the assignee.
    StatusChanged,
    /// A new customer-visible comment landed — notify requester and assignee.
    Commented,
}

impl TicketNotificationKind {
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Assigned => "Support ticket assigned",
            Self::StatusChanged => "Support ticket updated",
            Self::Commented => "New support ticket reply",
        }
    }

    #[must_use]
    pub const fn data_kind(self) -> &'static str {
        match self {
            Self::Assigned => "support_ticket_assigned",
            Self::StatusChanged => "support_ticket_status",
            Self::Commented => "support_ticket_comment",
        }
    }
}

/// A single notification to deliver. Recipients are staff users (push tokens
/// resolved by the adapter); external customers are notified through other
/// channels out of scope here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketNotification {
    pub ticket_id: SupportTicketId,
    pub recipient: UserId,
    pub kind: TicketNotificationKind,
    pub body: String,
}

impl TicketNotification {
    #[must_use]
    pub fn new(
        ticket_id: SupportTicketId,
        recipient: UserId,
        kind: TicketNotificationKind,
        body: impl Into<String>,
    ) -> Self {
        Self {
            ticket_id,
            recipient,
            kind,
            body: body.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Audit builder
// ---------------------------------------------------------------------------

/// Build a support audit event. `branch_id` is optional because customer-intake
/// tickets are branch-less until triaged (`with_branch` is only attached when a
/// branch is known). The PII contact is never placed in snapshots by callers.
pub fn support_audit_event(
    action: &str,
    actor: Option<UserId>,
    branch_id: Option<BranchId>,
    target_type: &str,
    target_id: impl ToString,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    let mut event = AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        target_type,
        target_id.to_string(),
        trace,
        occurred_at,
    );
    if let Some(branch_id) = branch_id {
        event = event.with_branch(branch_id);
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_support_domain::SlaPolicy;
    use time::macros::datetime;

    struct RecordingTransaction {
        case: SupportCase,
        receipt: Option<SupportCaseIdempotencyReceipt>,
        calls: Vec<&'static str>,
        receipt_inputs: Vec<(CaseScope, SupportTicketId, UserId, String, String)>,
        commits: Vec<SupportCaseCommit>,
    }

    impl SupportCaseLinkVerifier for RecordingTransaction {
        fn verify_work_order<'a>(
            &'a mut self,
            _scope: CaseScope,
            _work_order_id: WorkOrderId,
        ) -> SupportCaseFuture<'a, ()> {
            Box::pin(async move {
                self.calls.push("verify_work_order");
                Ok(())
            })
        }

        fn verify_evidence_object<'a>(
            &'a mut self,
            _scope: CaseScope,
            _evidence_object_id: EvidenceObjectId,
        ) -> SupportCaseFuture<'a, ()> {
            Box::pin(async move {
                self.calls.push("verify_evidence_object");
                Ok(())
            })
        }
    }

    impl SupportCaseUnitOfWork for RecordingTransaction {
        fn idempotency_receipt<'a>(
            &'a mut self,
            scope: CaseScope,
            case_id: SupportTicketId,
            actor: UserId,
            action: &'a str,
            key: &'a str,
        ) -> SupportCaseFuture<'a, Option<SupportCaseIdempotencyReceipt>> {
            Box::pin(async move {
                self.calls.push("receipt");
                self.receipt_inputs.push((
                    scope,
                    case_id,
                    actor,
                    action.to_owned(),
                    key.to_owned(),
                ));
                Ok(self.receipt.clone())
            })
        }

        fn load_case_for_update<'a>(
            &'a mut self,
            case_id: SupportTicketId,
        ) -> SupportCaseFuture<'a, SupportCase> {
            Box::pin(async move {
                self.calls.push("load");
                if case_id != self.case.id() {
                    return Err(KernelError::not_found("support case was not found"));
                }
                Ok(self.case.clone())
            })
        }

        fn commit<'a>(&'a mut self, commit: SupportCaseCommit) -> SupportCaseFuture<'a, ()> {
            Box::pin(async move {
                self.calls.push("commit");
                let receipt = match &commit {
                    SupportCaseCommit::Mutation(mutation) => {
                        self.case = mutation.case.clone();
                        SupportCaseIdempotencyReceipt {
                            fingerprint: mutation.fingerprint.clone(),
                            result: mutation.result.clone(),
                        }
                    }
                    SupportCaseCommit::ReceiptOnly(receipt) => SupportCaseIdempotencyReceipt {
                        fingerprint: receipt.fingerprint.clone(),
                        result: receipt.result.clone(),
                    },
                };
                self.receipt = Some(receipt);
                self.commits.push(commit);
                Ok(())
            })
        }
    }

    struct RecordingRepository {
        tx: RecordingTransaction,
        transaction_orgs: Vec<OrgId>,
    }

    impl SupportCaseRepository for RecordingRepository {
        type Transaction = RecordingTransaction;

        fn transaction<'a, T, F>(
            &'a mut self,
            org_id: OrgId,
            operation: F,
        ) -> SupportCaseFuture<'a, T>
        where
            T: Send + 'a,
            F: for<'tx> FnOnce(&'tx mut Self::Transaction) -> SupportCaseFuture<'tx, T> + Send + 'a,
        {
            self.transaction_orgs.push(org_id);
            Box::pin(async move { operation(&mut self.tx).await })
        }
    }

    fn run_ready<T>(future: impl Future<Output = T>) -> T {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(value) => value,
            std::task::Poll::Pending => panic!("recording test future must complete immediately"),
        }
    }

    fn fixture() -> (
        RecordingRepository,
        SupportCaseActorContext,
        SupportCaseCommandMetadata,
        Timestamp,
    ) {
        let now = datetime!(2026-07-24 09:00 UTC);
        let org_id = OrgId::new();
        let branch_id = BranchId::new();
        let actor = UserId::new();
        let case = SupportCase::open(
            SupportTicketId::new(),
            CaseScope::new(org_id, branch_id),
            TicketPriority::High,
            now,
            SlaPolicy::default(),
        )
        .unwrap();
        let context = SupportCaseActorContext {
            org_id,
            user_id: actor,
            branch_scope: BranchScope::single(branch_id),
        };
        let metadata = SupportCaseCommandMetadata {
            actor,
            expected_version: 0,
            idempotency_key: "support-case-idempotency-0001".to_owned(),
            fingerprint: "support-case-fingerprint-0001".to_owned(),
            trace: TraceContext::generate(),
        };
        (
            RecordingRepository {
                tx: RecordingTransaction {
                    case,
                    receipt: None,
                    calls: Vec::new(),
                    receipt_inputs: Vec::new(),
                    commits: Vec::new(),
                },
                transaction_orgs: Vec::new(),
            },
            context,
            metadata,
            now,
        )
    }

    #[test]
    fn customer_visible_audience_hides_internal_notes() {
        assert!(!CommentAudience::CustomerVisible.shows_internal_notes());
        assert!(CommentAudience::Internal.shows_internal_notes());
    }

    #[test]
    fn audit_event_without_branch_is_org_global() {
        let event = support_audit_event(
            "support.ticket.create_customer",
            None,
            None,
            "support_ticket",
            SupportTicketId::new(),
            TraceContext::generate(),
            Timestamp::now_utc(),
        )
        .unwrap();
        assert!(event.branch_id.is_none());
        assert!(event.actor.is_none());
    }

    #[test]
    fn audit_event_with_branch_carries_scope() {
        let branch = BranchId::new();
        let event = support_audit_event(
            "support.ticket.create_internal",
            Some(UserId::new()),
            Some(branch),
            "support_ticket",
            SupportTicketId::new(),
            TraceContext::generate(),
            Timestamp::now_utc(),
        )
        .unwrap();
        assert_eq!(event.branch_id, Some(branch));
    }

    #[test]
    fn case_command_metadata_binds_actor_scope_and_idempotency() {
        let actor = UserId::new();
        let branch = BranchId::new();
        let context = SupportCaseActorContext {
            org_id: console_kernel_core::OrgId::new(),
            user_id: actor,
            branch_scope: BranchScope::single(branch),
        };
        let metadata = SupportCaseCommandMetadata {
            actor,
            expected_version: 3,
            idempotency_key: "support-case-handoff-0001".to_owned(),
            fingerprint: "sha256:request-body".to_owned(),
            trace: TraceContext::generate(),
        };
        assert!(metadata.validate(&context, branch).is_ok());
        assert_eq!(
            support_case_idempotency(None, &metadata).unwrap(),
            SupportCaseIdempotency::Execute
        );
        assert_eq!(
            support_case_idempotency(Some("sha256:request-body"), &metadata).unwrap(),
            SupportCaseIdempotency::Replay
        );
        assert!(support_case_idempotency(Some("other"), &metadata).is_err());
        assert!(metadata.validate(&context, BranchId::new()).is_err());
        let wrong_actor = SupportCaseCommandMetadata {
            actor: UserId::new(),
            ..metadata.clone()
        };
        assert!(wrong_actor.validate(&context, branch).is_err());
    }

    #[test]
    fn case_outbox_intent_is_deterministic_and_contains_no_delivery_url() {
        let case_id = SupportTicketId::new();
        let history = CaseHistoryEntry {
            version: 4,
            event: CaseEvent::EvidenceBound {
                evidence_object_id: EvidenceObjectId::new(),
            },
            actor: UserId::new(),
            occurred_at: Timestamp::now_utc(),
        };
        let intent = SupportCaseOutboxIntent::from_history(case_id, &history);
        assert_eq!(intent.case_id, case_id);
        assert_eq!(intent.version, 4);
        assert_eq!(
            intent.dedupe_key,
            format!("support-case:{case_id}:4:support.case.evidence_bound")
        );
    }

    #[test]
    fn replay_lookup_follows_case_authorization_and_is_bound_to_canonical_scope_actor_and_case() {
        let (mut repository, context, metadata, now) = fixture();
        let case_id = repository.tx.case.id();
        repository.tx.receipt = Some(SupportCaseIdempotencyReceipt {
            fingerprint: metadata.fingerprint.clone(),
            result: SupportCaseMutationResult {
                case_id,
                version: 7,
                replayed: false,
            },
        });
        let result = run_ready(request_dispatch_handoff(
            &mut repository,
            &context,
            RequestDispatchHandoffCommand {
                case_id,
                work_order_id: WorkOrderId::new(),
                metadata: metadata.clone(),
                occurred_at: now,
            },
        ))
        .unwrap();
        assert!(result.replayed);
        assert_eq!(repository.tx.calls, ["load", "receipt"]);
        assert_eq!(
            repository.tx.receipt_inputs[0].0,
            repository.tx.case.scope()
        );
        assert_eq!(repository.tx.receipt_inputs[0].1, case_id);
        assert_eq!(repository.tx.receipt_inputs[0].2, context.user_id);
        assert_eq!(
            repository.tx.receipt_inputs[0].3,
            "support.case.dispatch_handoff.request"
        );
        assert_eq!(repository.tx.receipt_inputs[0].4, metadata.idempotency_key);
    }

    #[test]
    fn malformed_actor_is_rejected_before_opening_a_transaction() {
        let (mut repository, context, mut metadata, now) = fixture();
        metadata.actor = UserId::new();
        let case_id = repository.tx.case.id();
        let result = run_ready(request_dispatch_handoff(
            &mut repository,
            &context,
            RequestDispatchHandoffCommand {
                case_id,
                work_order_id: WorkOrderId::new(),
                metadata,
                occurred_at: now,
            },
        ));
        assert!(result.is_err());
        assert!(repository.tx.calls.is_empty());
    }

    #[test]
    fn wrong_case_cannot_replay_before_authorization() {
        let (mut repository, context, metadata, now) = fixture();
        let result = run_ready(request_dispatch_handoff(
            &mut repository,
            &context,
            RequestDispatchHandoffCommand {
                case_id: SupportTicketId::new(),
                work_order_id: WorkOrderId::new(),
                metadata,
                occurred_at: now,
            },
        ));
        assert!(result.is_err());
        assert_eq!(repository.tx.calls, ["load"]);
    }

    #[test]
    fn mismatched_receipt_fingerprint_conflicts_without_verification_or_persistence() {
        let (mut repository, context, metadata, now) = fixture();
        let case_id = repository.tx.case.id();
        repository.tx.receipt = Some(SupportCaseIdempotencyReceipt {
            fingerprint: "other-command-payload".to_owned(),
            result: SupportCaseMutationResult {
                case_id,
                version: 0,
                replayed: false,
            },
        });

        let result = run_ready(request_dispatch_handoff(
            &mut repository,
            &context,
            RequestDispatchHandoffCommand {
                case_id,
                work_order_id: WorkOrderId::new(),
                metadata,
                occurred_at: now,
            },
        ));

        assert!(result.is_err());
        assert_eq!(repository.tx.calls, ["load", "receipt"]);
        assert!(repository.tx.commits.is_empty());
    }

    #[test]
    fn first_semantic_noop_persists_only_receipt_then_replays_or_conflicts_before_verification() {
        let (mut repository, context, mut metadata, now) = fixture();
        let work_order_id = WorkOrderId::new();
        repository
            .tx
            .case
            .request_dispatch_handoff(0, context.user_id, work_order_id, now)
            .unwrap();
        metadata.expected_version = repository.tx.case.version();
        let case_id = repository.tx.case.id();
        let version_before = repository.tx.case.version();
        let history_len_before = repository.tx.case.history().len();

        let first = run_ready(request_dispatch_handoff(
            &mut repository,
            &context,
            RequestDispatchHandoffCommand {
                case_id,
                work_order_id,
                metadata: metadata.clone(),
                occurred_at: now,
            },
        ))
        .unwrap();
        assert!(!first.replayed);
        assert_eq!(first.version, version_before);
        assert_eq!(repository.tx.case.version(), version_before);
        assert_eq!(repository.tx.case.history().len(), history_len_before);
        assert_eq!(
            repository.tx.calls,
            ["load", "receipt", "verify_work_order", "commit"]
        );
        let SupportCaseCommit::ReceiptOnly(receipt) = &repository.tx.commits[0] else {
            panic!("semantic no-op must atomically persist only a receipt");
        };
        assert_eq!(receipt.scope, repository.tx.case.scope());
        assert_eq!(receipt.case_id, case_id);
        assert_eq!(receipt.result, first);
        assert_eq!(
            receipt.audit.action.as_str(),
            "support.case.dispatch_handoff.request_noop"
        );
        assert_eq!(receipt.audit.trace, metadata.trace);

        let replay = run_ready(request_dispatch_handoff(
            &mut repository,
            &context,
            RequestDispatchHandoffCommand {
                case_id,
                work_order_id,
                metadata: metadata.clone(),
                occurred_at: now,
            },
        ))
        .unwrap();
        assert!(replay.replayed);
        assert_eq!(repository.tx.commits.len(), 1);
        assert_eq!(
            repository.tx.calls,
            [
                "load",
                "receipt",
                "verify_work_order",
                "commit",
                "load",
                "receipt"
            ]
        );

        let conflicting = SupportCaseCommandMetadata {
            fingerprint: "support-case-different-fingerprint".to_owned(),
            ..metadata
        };
        let conflict = run_ready(request_dispatch_handoff(
            &mut repository,
            &context,
            RequestDispatchHandoffCommand {
                case_id,
                work_order_id,
                metadata: conflicting,
                occurred_at: now,
            },
        ));
        assert!(conflict.is_err());
        assert_eq!(repository.tx.commits.len(), 1);
        assert_eq!(
            repository.tx.calls,
            [
                "load",
                "receipt",
                "verify_work_order",
                "commit",
                "load",
                "receipt",
                "load",
                "receipt"
            ]
        );
    }

    #[test]
    fn stale_expected_version_is_rejected_without_persistence() {
        let (mut repository, context, mut metadata, now) = fixture();
        let case_id = repository.tx.case.id();
        metadata.expected_version = 1;

        let result = run_ready(request_dispatch_handoff(
            &mut repository,
            &context,
            RequestDispatchHandoffCommand {
                case_id,
                work_order_id: WorkOrderId::new(),
                metadata,
                occurred_at: now,
            },
        ));

        assert!(result.is_err());
        assert_eq!(
            repository.tx.calls,
            ["load", "receipt", "verify_work_order"]
        );
        assert!(repository.tx.commits.is_empty());
    }

    #[test]
    fn wrong_branch_cannot_replay_before_authorization() {
        let (mut repository, mut context, metadata, now) = fixture();
        context.branch_scope = BranchScope::single(BranchId::new());
        let case_id = repository.tx.case.id();
        let result = run_ready(request_dispatch_handoff(
            &mut repository,
            &context,
            RequestDispatchHandoffCommand {
                case_id,
                work_order_id: WorkOrderId::new(),
                metadata,
                occurred_at: now,
            },
        ));
        assert!(result.is_err());
        assert_eq!(repository.tx.calls, ["load"]);
    }

    #[test]
    fn dispatch_handoff_commits_matching_case_history_outbox_and_receipt() {
        let (mut repository, context, metadata, now) = fixture();
        let case_id = repository.tx.case.id();
        let work_order_id = WorkOrderId::new();
        run_ready(request_dispatch_handoff(
            &mut repository,
            &context,
            RequestDispatchHandoffCommand {
                case_id,
                work_order_id,
                metadata: metadata.clone(),
                occurred_at: now,
            },
        ))
        .unwrap();
        let SupportCaseCommit::Mutation(commit) = &repository.tx.commits[0] else {
            panic!("state-changing handoff must commit a mutation");
        };
        assert_eq!(
            repository.tx.calls,
            ["load", "receipt", "verify_work_order", "commit"]
        );
        assert_eq!(repository.transaction_orgs, [context.org_id]);
        assert_eq!(commit.case.id(), commit.outbox.case_id);
        assert_eq!(commit.outbox.event, commit.history.event);
        assert_eq!(commit.outbox.version, commit.history.version);
        assert_eq!(commit.idempotency_key, metadata.idempotency_key);
        assert_eq!(commit.result.case_id, case_id);
        assert_eq!(commit.expected_version, metadata.expected_version);
        assert_eq!(commit.audit.actor, Some(context.user_id));
        assert_eq!(commit.audit.org_id, Some(context.org_id));
        assert_eq!(
            commit.audit.branch_id,
            Some(repository.tx.case.scope().branch_id)
        );
        assert_eq!(commit.audit.trace, metadata.trace);
    }

    #[test]
    fn evidence_binding_commits_matching_case_history_outbox_and_receipt() {
        let (mut repository, context, metadata, now) = fixture();
        let case_id = repository.tx.case.id();
        let evidence_object_id = EvidenceObjectId::new();
        run_ready(bind_case_evidence(
            &mut repository,
            &context,
            BindCaseEvidenceCommand {
                case_id,
                evidence_object_id,
                metadata: metadata.clone(),
                occurred_at: now,
            },
        ))
        .unwrap();
        let SupportCaseCommit::Mutation(commit) = &repository.tx.commits[0] else {
            panic!("state-changing evidence bind must commit a mutation");
        };
        assert_eq!(
            repository.tx.calls,
            ["load", "receipt", "verify_evidence_object", "commit"]
        );
        assert_eq!(commit.case.id(), commit.outbox.case_id);
        assert_eq!(commit.outbox.event, commit.history.event);
        assert_eq!(commit.idempotency_key, metadata.idempotency_key);
        assert!(
            matches!(commit.history.event, CaseEvent::EvidenceBound { evidence_object_id: id } if id == evidence_object_id)
        );
    }

    #[test]
    fn handoff_resolution_commits_matching_case_history_outbox_and_receipt() {
        let (mut repository, context, mut metadata, now) = fixture();
        let work_order_id = WorkOrderId::new();
        repository
            .tx
            .case
            .request_dispatch_handoff(0, context.user_id, work_order_id, now)
            .unwrap();
        metadata.expected_version = repository.tx.case.version();
        let case_id = repository.tx.case.id();
        run_ready(resolve_dispatch_handoff(
            &mut repository,
            &context,
            ResolveDispatchHandoffCommand {
                case_id,
                status: DispatchHandoffStatus::Accepted,
                metadata: metadata.clone(),
                occurred_at: now,
            },
        ))
        .unwrap();
        let SupportCaseCommit::Mutation(commit) = &repository.tx.commits[0] else {
            panic!("handoff resolution must commit a mutation");
        };
        assert_eq!(repository.tx.calls, ["load", "receipt", "commit"]);
        assert_eq!(commit.case.id(), commit.outbox.case_id);
        assert_eq!(commit.outbox.event, commit.history.event);
        assert_eq!(commit.idempotency_key, metadata.idempotency_key);
        assert!(
            matches!(commit.history.event, CaseEvent::DispatchHandoffResolved { work_order_id: id, status: DispatchHandoffStatus::Accepted } if id == work_order_id)
        );
    }
}
