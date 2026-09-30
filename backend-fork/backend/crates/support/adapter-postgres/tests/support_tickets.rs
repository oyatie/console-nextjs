//! DB-backed tests for the support-ticket Postgres adapter.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_kernel_core::{
    BranchId, BranchScope, ErrorKind, OrgId, SupportTicketId, TraceContext, UserId,
};
use console_support_adapter_postgres::PgSupportStore;
use console_support_application::{
    AddCommentCommand, AssignTicketCommand, CommentAudience, CreateCustomerIntakeCommand,
    CreateInternalTicketCommand, ListTicketsQuery, TicketNotificationKind, TransitionTicketCommand,
};
use console_support_domain::{TicketCategory, TicketOrigin, TicketPriority, TicketStatus};
use sqlx::{PgPool, Row};
use time::macros::datetime;

// ---------------------------------------------------------------------------
// create_internal_ticket: branch-scoped, audited, SLA derived from priority
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_internal_ticket_is_branch_scoped_audited_and_sla_derived(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch = seed_branch(&pool).await;
        let actor = seed_user(&pool, "Staff", branch).await;
        let store = PgSupportStore::new(pool.clone());
        let now = datetime!(2026-06-13 09:00 UTC);

        let summary = store
            .create_internal_ticket(CreateInternalTicketCommand {
                actor,
                branch_id: branch,
                category: TicketCategory::SystemBug,
                priority: TicketPriority::High,
                title: "Login is broken".to_owned(),
                body: "Cannot sign in".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();

        assert_eq!(summary.origin, TicketOrigin::Internal);
        assert_eq!(summary.status, TicketStatus::Open);
        assert_eq!(summary.branch_id, Some(branch));
        assert_eq!(summary.requester_user_id, Some(actor));
        // HIGH -> SLA 1 day.
        assert_eq!(summary.due_at, Some(now + time::Duration::days(1)));

        let actions = audit_actions_for_target(&pool, summary.id).await;
        assert!(actions.contains(&"support.ticket.create_internal".to_owned()));
    })
    .await;
}

// ---------------------------------------------------------------------------
// create_customer_intake: branch-less, audited, no PII in audit
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_customer_intake_is_branchless_and_audited_without_pii(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let store = PgSupportStore::new(pool.clone());
        let now = datetime!(2026-06-13 09:00 UTC);

        let summary = store
            .create_customer_intake(CreateCustomerIntakeCommand {
                category: TicketCategory::Complaint,
                priority: TicketPriority::Urgent,
                title: "Bad service".to_owned(),
                body: "The forklift was late".to_owned(),
                requester_name: "Hong Gildong".to_owned(),
                requester_contact: "010-1234-5678".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();

        assert_eq!(summary.origin, TicketOrigin::Customer);
        assert_eq!(summary.branch_id, None);
        assert_eq!(summary.requester_user_id, None);
        assert_eq!(summary.requester_name.as_deref(), Some("Hong Gildong"));
        // URGENT -> SLA 4 hours.
        assert_eq!(summary.due_at, Some(now + time::Duration::hours(4)));

        // Audited.
        let actions = audit_actions_for_target(&pool, summary.id).await;
        assert!(actions.contains(&"support.ticket.create_customer".to_owned()));

        // The PII contact must never appear in any audit snapshot.
        let snapshots: Vec<String> = sqlx::query_scalar(
            "SELECT COALESCE(after_snap::text, '') FROM audit_events WHERE target_id = $1",
        )
        .bind(summary.id.to_string())
        .fetch_all(&pool)
        .await
        .unwrap();
        for snapshot in snapshots {
            assert!(
                !snapshot.contains("010-1234-5678"),
                "PII contact leaked into audit snapshot"
            );
            assert!(!snapshot.contains("Hong Gildong"));
        }
    })
    .await;
}

// ---------------------------------------------------------------------------
// assign_ticket: audited + notification enqueued for the new assignee
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn assign_ticket_audits_and_enqueues_assignee_notification(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch = seed_branch(&pool).await;
        let requester = seed_user(&pool, "Requester", branch).await;
        let assignee = seed_user(&pool, "Assignee", branch).await;
        let store = PgSupportStore::new(pool.clone());
        let now = datetime!(2026-06-13 09:00 UTC);

        let ticket = store
            .create_internal_ticket(CreateInternalTicketCommand {
                actor: requester,
                branch_id: branch,
                category: TicketCategory::Operational,
                priority: TicketPriority::Medium,
                title: "Need help".to_owned(),
                body: "Details".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();

        let (summary, notifications) = store
            .assign_ticket(AssignTicketCommand {
                actor: requester,
                ticket_id: ticket.id,
                assignee_user_id: assignee,
                branch_id: None,
                trace: TraceContext::generate(),
                occurred_at: now + time::Duration::minutes(1),
            })
            .await
            .unwrap();

        assert_eq!(summary.assignee_user_id, Some(assignee));
        // The same-org LEFT JOIN resolves the assignee's display name on the
        // returned summary (no raw-UUID leak to the client).
        assert_eq!(summary.assignee_name.as_deref(), Some("Assignee"));
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].recipient, assignee);
        assert_eq!(notifications[0].kind, TicketNotificationKind::Assigned);

        let actions = audit_actions_for_target(&pool, ticket.id).await;
        assert!(actions.contains(&"support.ticket.assign".to_owned()));

        // The display-name JOIN also resolves through the list path, and an
        // unassigned ticket yields NULL there (not a UUID).
        let listed = store
            .list_tickets(ListTicketsQuery {
                branch_scope: BranchScope::single(branch),
                status: None,
                priority: None,
                category: None,
                origin: None,
                assignee_user_id: None,
                include_untriaged: false,
                limit: None,
                site_id: None,
                cursor: None,
            })
            .await
            .unwrap();
        let listed_ticket = listed
            .items
            .iter()
            .find(|t| t.id == ticket.id)
            .expect("assigned ticket is in the list");
        assert_eq!(listed_ticket.assignee_name.as_deref(), Some("Assignee"));
    })
    .await;
}

// ---------------------------------------------------------------------------
// assign_ticket triages a branch-less customer ticket into a branch
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn assign_triages_untriaged_customer_ticket_into_branch(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch = seed_branch(&pool).await;
        let assignee = seed_user(&pool, "Triager", branch).await;
        let store = PgSupportStore::new(pool.clone());
        let now = datetime!(2026-06-13 09:00 UTC);

        let ticket = store
            .create_customer_intake(CreateCustomerIntakeCommand {
                category: TicketCategory::EquipmentInquiry,
                priority: TicketPriority::Low,
                title: "Question".to_owned(),
                body: "How tall is the mast".to_owned(),
                requester_name: "Customer".to_owned(),
                requester_contact: "customer@example.com".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();
        assert_eq!(ticket.branch_id, None);

        // Triage without branch_id is rejected.
        let err = store
            .assign_ticket(AssignTicketCommand {
                actor: assignee,
                ticket_id: ticket.id,
                assignee_user_id: assignee,
                branch_id: None,
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Validation);

        // Triage with branch_id assigns the branch.
        let (summary, _) = store
            .assign_ticket(AssignTicketCommand {
                actor: assignee,
                ticket_id: ticket.id,
                assignee_user_id: assignee,
                branch_id: Some(branch),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();
        assert_eq!(summary.branch_id, Some(branch));
        assert_eq!(summary.assignee_user_id, Some(assignee));
    })
    .await;
}

// ---------------------------------------------------------------------------
// transition_status: valid transition audited; invalid transition rejected
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn transition_status_enforces_fsm_and_audits(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch = seed_branch(&pool).await;
        let requester = seed_user(&pool, "Requester", branch).await;
        let assignee = seed_user(&pool, "Assignee", branch).await;
        let store = PgSupportStore::new(pool.clone());
        let now = datetime!(2026-06-13 09:00 UTC);

        let ticket = store
            .create_internal_ticket(CreateInternalTicketCommand {
                actor: requester,
                branch_id: branch,
                category: TicketCategory::Other,
                priority: TicketPriority::Low,
                title: "Track me".to_owned(),
                body: "Body".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();
        store
            .assign_ticket(AssignTicketCommand {
                actor: requester,
                ticket_id: ticket.id,
                assignee_user_id: assignee,
                branch_id: None,
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();

        // OPEN -> RESOLVED is invalid.
        let err = store
            .transition_status(TransitionTicketCommand {
                actor: assignee,
                ticket_id: ticket.id,
                to_status: TicketStatus::Resolved,
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidTransition);

        // OPEN -> IN_PROGRESS is valid; notifies assignee + internal requester.
        let (summary, notifications) = store
            .transition_status(TransitionTicketCommand {
                actor: assignee,
                ticket_id: ticket.id,
                to_status: TicketStatus::InProgress,
                trace: TraceContext::generate(),
                occurred_at: now + time::Duration::minutes(1),
            })
            .await
            .unwrap();
        assert_eq!(summary.status, TicketStatus::InProgress);
        let recipients: Vec<UserId> = notifications.iter().map(|n| n.recipient).collect();
        assert!(recipients.contains(&assignee));
        assert!(recipients.contains(&requester));

        // IN_PROGRESS -> RESOLVED stamps resolved_at.
        let (resolved, _) = store
            .transition_status(TransitionTicketCommand {
                actor: assignee,
                ticket_id: ticket.id,
                to_status: TicketStatus::Resolved,
                trace: TraceContext::generate(),
                occurred_at: now + time::Duration::minutes(2),
            })
            .await
            .unwrap();
        assert_eq!(resolved.status, TicketStatus::Resolved);
        assert!(resolved.resolved_at.is_some());

        let actions = audit_actions_for_target(&pool, ticket.id).await;
        assert_eq!(
            actions
                .iter()
                .filter(|a| a.as_str() == "support.ticket.transition")
                .count(),
            2
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// add_comment: audited; internal note suppresses requester notification
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn add_comment_audits_and_respects_internal_note_visibility(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch = seed_branch(&pool).await;
        let requester = seed_user(&pool, "Requester", branch).await;
        let assignee = seed_user(&pool, "Assignee", branch).await;
        let store = PgSupportStore::new(pool.clone());
        let now = datetime!(2026-06-13 09:00 UTC);

        let ticket = store
            .create_internal_ticket(CreateInternalTicketCommand {
                actor: requester,
                branch_id: branch,
                category: TicketCategory::AccessRequest,
                priority: TicketPriority::Medium,
                title: "Grant access".to_owned(),
                body: "Please".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();
        store
            .assign_ticket(AssignTicketCommand {
                actor: requester,
                ticket_id: ticket.id,
                assignee_user_id: assignee,
                branch_id: None,
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();

        // Internal note by the assignee: no notifications.
        let (note, note_notifications) = store
            .add_comment(AddCommentCommand {
                actor: assignee,
                ticket_id: ticket.id,
                body: "internal triage note".to_owned(),
                is_internal_note: true,
                trace: TraceContext::generate(),
                occurred_at: now + time::Duration::minutes(1),
            })
            .await
            .unwrap();
        assert!(note.is_internal_note);
        // The same-org LEFT JOIN resolves the comment author's display name.
        assert_eq!(note.author_name.as_deref(), Some("Assignee"));
        assert!(note_notifications.is_empty());

        // Customer-visible reply by the assignee: notifies the requester.
        let (reply, reply_notifications) = store
            .add_comment(AddCommentCommand {
                actor: assignee,
                ticket_id: ticket.id,
                body: "we are on it".to_owned(),
                is_internal_note: false,
                trace: TraceContext::generate(),
                occurred_at: now + time::Duration::minutes(2),
            })
            .await
            .unwrap();
        assert!(!reply.is_internal_note);
        let reply_recipients: Vec<UserId> =
            reply_notifications.iter().map(|n| n.recipient).collect();
        assert!(reply_recipients.contains(&requester));
        // Author (assignee) is not notified of their own comment.
        assert!(!reply_recipients.contains(&assignee));

        // Staff view returns both comments; customer-visible view drops the note.
        let staff = store
            .get_ticket(
                ticket.id,
                &BranchScope::single(branch),
                CommentAudience::Internal,
            )
            .await
            .unwrap();
        assert_eq!(staff.comments.len(), 2);
        let customer = store
            .get_ticket(
                ticket.id,
                &BranchScope::single(branch),
                CommentAudience::CustomerVisible,
            )
            .await
            .unwrap();
        assert_eq!(customer.comments.len(), 1);
        assert!(customer.comments.iter().all(|c| !c.is_internal_note));

        let actions = audit_actions_for_target_type(&pool, "support_ticket_comment").await;
        assert!(actions.contains(&"support.ticket.comment".to_owned()));
    })
    .await;
}

// ---------------------------------------------------------------------------
// list_tickets: branch scope (two branches) + filters
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn list_tickets_respects_branch_scope_and_filters(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch_a = seed_branch(&pool).await;
        let branch_b = seed_branch(&pool).await;
        let staff_a = seed_user(&pool, "Staff A", branch_a).await;
        let staff_b = seed_user(&pool, "Staff B", branch_b).await;
        let store = PgSupportStore::new(pool.clone());
        let now = datetime!(2026-06-13 09:00 UTC);

        let a_high = store
            .create_internal_ticket(CreateInternalTicketCommand {
                actor: staff_a,
                branch_id: branch_a,
                category: TicketCategory::SystemBug,
                priority: TicketPriority::High,
                title: "A high".to_owned(),
                body: "x".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();
        store
            .create_internal_ticket(CreateInternalTicketCommand {
                actor: staff_a,
                branch_id: branch_a,
                category: TicketCategory::Operational,
                priority: TicketPriority::Low,
                title: "A low".to_owned(),
                body: "x".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();
        store
            .create_internal_ticket(CreateInternalTicketCommand {
                actor: staff_b,
                branch_id: branch_b,
                category: TicketCategory::SystemBug,
                priority: TicketPriority::High,
                title: "B high".to_owned(),
                body: "x".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();

        // Branch A staff only see branch A's two tickets.
        let scope_a = BranchScope::single(branch_a);
        let a_all = store
            .list_tickets(ListTicketsQuery {
                branch_scope: scope_a.clone(),
                status: None,
                priority: None,
                category: None,
                origin: None,
                assignee_user_id: None,
                include_untriaged: false,
                limit: None,
                site_id: None,
                cursor: None,
            })
            .await
            .unwrap();
        assert_eq!(a_all.total, 2);
        assert_eq!(a_all.items.len(), 2);
        assert!(a_all.items.iter().all(|t| t.branch_id == Some(branch_a)));

        // Filter by priority within branch A.
        let a_high_only = store
            .list_tickets(ListTicketsQuery {
                branch_scope: scope_a,
                status: None,
                priority: Some(TicketPriority::High),
                category: None,
                origin: None,
                assignee_user_id: None,
                include_untriaged: false,
                limit: None,
                site_id: None,
                cursor: None,
            })
            .await
            .unwrap();
        assert_eq!(a_high_only.total, 1);
        assert_eq!(a_high_only.items.len(), 1);
        assert_eq!(a_high_only.items[0].id, a_high.id);

        // Cross-branch (All) sees all three.
        let all = store
            .list_tickets(ListTicketsQuery {
                branch_scope: BranchScope::All,
                status: None,
                priority: None,
                category: None,
                origin: None,
                assignee_user_id: None,
                include_untriaged: false,
                limit: None,
                site_id: None,
                cursor: None,
            })
            .await
            .unwrap();
        assert_eq!(all.total, 3);
        assert_eq!(all.items.len(), 3);
    })
    .await;
}

// ---------------------------------------------------------------------------
// list_tickets: untriaged customer intake only visible cross-branch
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn untriaged_intake_is_only_visible_cross_branch(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch = seed_branch(&pool).await;
        let store = PgSupportStore::new(pool.clone());
        let now = datetime!(2026-06-13 09:00 UTC);

        store
            .create_customer_intake(CreateCustomerIntakeCommand {
                category: TicketCategory::Complaint,
                priority: TicketPriority::Medium,
                title: "intake".to_owned(),
                body: "x".to_owned(),
                requester_name: "Cust".to_owned(),
                requester_contact: "c@example.com".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();

        // Branch-scoped staff cannot see the untriaged ticket even with the flag.
        let scoped = store
            .list_tickets(ListTicketsQuery {
                branch_scope: BranchScope::single(branch),
                status: None,
                priority: None,
                category: None,
                origin: None,
                assignee_user_id: None,
                include_untriaged: true,
                limit: None,
                site_id: None,
                cursor: None,
            })
            .await
            .unwrap();
        assert_eq!(scoped.total, 0);
        assert!(scoped.items.is_empty());

        // Cross-branch with the flag sees it.
        let cross = store
            .list_tickets(ListTicketsQuery {
                branch_scope: BranchScope::All,
                status: None,
                priority: None,
                category: None,
                origin: Some(TicketOrigin::Customer),
                assignee_user_id: None,
                include_untriaged: true,
                limit: None,
                site_id: None,
                cursor: None,
            })
            .await
            .unwrap();
        assert_eq!(cross.total, 1);
        assert_eq!(cross.items.len(), 1);
        assert_eq!(cross.items[0].branch_id, None);
    })
    .await;
}

// ---------------------------------------------------------------------------
// rate-limit counter: increments past the cap (REST converts >cap to 429)
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn rate_limit_counter_increments_and_exceeds_cap(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let store = PgSupportStore::new(pool.clone());
        let window = datetime!(2026-06-13 09:00 UTC);
        let cap: i64 = 5;

        let mut last = 0;
        for _ in 0..(cap + 1) {
            last = store
                .increment_rate_bucket("ip:203.0.113.7", "support_intake", window)
                .await
                .unwrap();
        }
        // After cap+1 attempts the count exceeds the cap, which the REST limiter
        // maps to HTTP 429.
        assert!(last > cap, "expected {last} > {cap}");

        // A different bucket key is independent.
        let other = store
            .increment_rate_bucket("ip:198.51.100.9", "support_intake", window)
            .await
            .unwrap();
        assert_eq!(other, 1);
    })
    .await;
}

// ---------------------------------------------------------------------------
// get_ticket: not found outside branch scope
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn get_ticket_is_not_found_outside_branch_scope(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch = seed_branch(&pool).await;
        let other_branch = seed_branch(&pool).await;
        let staff = seed_user(&pool, "Staff", branch).await;
        let store = PgSupportStore::new(pool.clone());
        let now = datetime!(2026-06-13 09:00 UTC);

        let ticket = store
            .create_internal_ticket(CreateInternalTicketCommand {
                actor: staff,
                branch_id: branch,
                category: TicketCategory::Other,
                priority: TicketPriority::Low,
                title: "secret".to_owned(),
                body: "x".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap();

        let err = store
            .get_ticket(
                ticket.id,
                &BranchScope::single(other_branch),
                CommentAudience::Internal,
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);

        // Unknown id is also not found.
        let err = store
            .get_ticket(
                SupportTicketId::new(),
                &BranchScope::All,
                CommentAudience::Internal,
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    })
    .await;
}

// ---------------------------------------------------------------------------
// list_tickets: hard server-side cap + keyset cursor pagination
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn list_tickets_caps_and_pages_by_keyset_cursor(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch = seed_branch(&pool).await;
        let staff = seed_user(&pool, "Staff", branch).await;
        let store = PgSupportStore::new(pool.clone());
        let base = datetime!(2026-06-13 09:00 UTC);

        // Five tickets with strictly increasing created_at so the (created_at DESC,
        // id DESC) order is deterministic.
        for i in 0..5 {
            store
                .create_internal_ticket(CreateInternalTicketCommand {
                    actor: staff,
                    branch_id: branch,
                    category: TicketCategory::SystemBug,
                    priority: TicketPriority::High,
                    title: format!("ticket {i}"),
                    body: "x".to_owned(),
                    trace: TraceContext::generate(),
                    occurred_at: base + time::Duration::minutes(i),
                })
                .await
                .unwrap();
        }

        let query = |limit: Option<i64>, cursor: Option<SupportTicketId>| ListTicketsQuery {
            branch_scope: BranchScope::single(branch),
            status: None,
            priority: None,
            category: None,
            origin: None,
            assignee_user_id: None,
            include_untriaged: false,
            limit,
            cursor,
            site_id: None,
        };

        // First page of 2: the two newest tickets. The unpaged total is the same
        // (5) on every page, and `next_cursor` points at the next page.
        let page1 = store.list_tickets(query(Some(2), None)).await.unwrap();
        assert_eq!(page1.items.len(), 2);
        assert_eq!(page1.total, 5);
        let cursor = page1.next_cursor.expect("more pages exist after page1");
        // The cursor is the last KEPT id (look-ahead row is dropped, not exposed).
        assert_eq!(cursor, page1.items[1].id);
        // Newest first.
        assert!(page1.items[0].created_at >= page1.items[1].created_at);

        // Next page after page1's cursor: two more, all strictly older.
        let page2 = store
            .list_tickets(query(Some(2), Some(cursor)))
            .await
            .unwrap();
        assert_eq!(page2.items.len(), 2);
        assert_eq!(page2.total, 5);
        assert!(page2.items[0].created_at <= page1.items[1].created_at);
        // No overlap between pages.
        for ticket in &page2.items {
            assert!(!page1.items.iter().any(|p| p.id == ticket.id));
        }

        // The final page (1 remaining) reports no further cursor.
        let cursor2 = page2.next_cursor.expect("one more page after page2");
        let page3 = store
            .list_tickets(query(Some(2), Some(cursor2)))
            .await
            .unwrap();
        assert_eq!(page3.items.len(), 1);
        assert_eq!(page3.total, 5);
        assert!(page3.next_cursor.is_none(), "last page has no next_cursor");

        // A None limit must still bound the fetch (default 50), never unbounded.
        let defaulted = store.list_tickets(query(None, None)).await.unwrap();
        assert_eq!(defaulted.items.len(), 5);
        assert_eq!(defaulted.total, 5);
        // The whole set fits in one page, so there is no further cursor.
        assert!(defaulted.next_cursor.is_none());

        // An over-large limit is clamped, not honored verbatim.
        let clamped = store.list_tickets(query(Some(10_000), None)).await.unwrap();
        assert_eq!(clamped.items.len(), 5);
        assert_eq!(clamped.total, 5);
    })
    .await;
}

// ---------------------------------------------------------------------------
// create paths reject over-length free-text fields with a validation error
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn customer_intake_rejects_over_length_fields(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let store = PgSupportStore::new(pool.clone());
        let now = datetime!(2026-06-13 09:00 UTC);

        let err = store
            .create_customer_intake(CreateCustomerIntakeCommand {
                category: TicketCategory::Complaint,
                priority: TicketPriority::Medium,
                title: "t".to_owned(),
                // Body over the 8000-char cap.
                body: "x".repeat(8001),
                requester_name: "Cust".to_owned(),
                requester_contact: "c@example.com".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Validation);
    })
    .await;
}

// ---------------------------------------------------------------------------
// Seeding helpers
// ---------------------------------------------------------------------------

async fn seed_branch(pool: &PgPool) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Support Region {}", uuid::Uuid::new_v4()))
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind("Support Branch")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(pool: &PgPool, name: &str, branch_id: BranchId) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, phone, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(name)
        .bind(format!("010{}", &user_id.to_string()[..8]))
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    user_id
}

async fn audit_actions_for_target(pool: &PgPool, target_id: SupportTicketId) -> Vec<String> {
    sqlx::query("SELECT action FROM audit_events WHERE target_id = $1 ORDER BY occurred_at")
        .bind(target_id.to_string())
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<String, _>("action"))
        .collect()
}

async fn audit_actions_for_target_type(pool: &PgPool, target_type: &str) -> Vec<String> {
    sqlx::query("SELECT action FROM audit_events WHERE target_type = $1 ORDER BY occurred_at")
        .bind(target_type)
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<String, _>("action"))
        .collect()
}
