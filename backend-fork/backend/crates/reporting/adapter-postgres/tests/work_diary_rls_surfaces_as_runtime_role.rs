#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for work-diary update/confirm.
//!
//! `work_diary_draft_can_be_generated_edited_confirmed_and_exported` (in
//! excel_exports.rs) drives the same update/confirm path but on the plain
//! `#[sqlx::test]` pool — a BYPASSRLS superuser that ignores `app.current_org`
//! entirely, so it cannot tell an armed GUC from an unarmed one. This test
//! proves the same path AS the genuine non-owner runtime role `console_rt`
//! (NOSUPERUSER, NOBYPASSRLS, FORCE RLS): SEED as the owner, GENERATE/UPDATE/
//! CONFIRM as `console_rt` under the armed GUC.

use console_kernel_core::{BranchScope, OrgId, TraceContext};
use console_platform_test_support::{runtime_role_pool, seed_branch, seed_user};
use console_reporting_adapter_postgres::PgReportingRepository;
use console_reporting_application::{
    WorkDiaryConfirmCommand, WorkDiaryDraftPort, WorkDiaryQuery, WorkDiaryUpdateCommand,
};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime, macros::date};

const DIARY_DATE: time::Date = date!(2026 - 06 - 12);

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn work_diary_update_and_confirm_succeed_as_runtime_role(owner_pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let branch = seed_branch(&owner_pool, "Region", "Branch").await;
        let actor = seed_user(&owner_pool, "Admin", "ADMIN", branch).await;

        let rt_pool = runtime_role_pool(&owner_pool).await;
        let repo = PgReportingRepository::new(rt_pool);
        let branch_scope = BranchScope::single(branch);

        let generated = repo
            .get_or_generate_work_diary(WorkDiaryQuery {
                actor,
                date: DIARY_DATE,
                branch_scope: branch_scope.clone(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .expect("generate must succeed as console_rt under the armed GUC");
        assert_eq!(generated.status.as_str(), "DRAFT");

        let updated = repo
            .update_work_diary(WorkDiaryUpdateCommand {
                actor,
                date: DIARY_DATE,
                branch_scope: branch_scope.clone(),
                body: generated.body,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc() + Duration::seconds(1),
            })
            .await
            .expect("update must succeed as console_rt under the armed GUC — an unarmed GUC would RLS-deny the UPDATE and this would fail closed as `work diary draft was not editable`");
        assert_eq!(updated.status.as_str(), "DRAFT");

        let confirmed = repo
            .confirm_work_diary(WorkDiaryConfirmCommand {
                actor,
                date: DIARY_DATE,
                branch_scope,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc() + Duration::seconds(2),
            })
            .await
            .expect("confirm must succeed as console_rt under the armed GUC — an unarmed GUC would RLS-deny the UPDATE and this would fail closed as `work diary is already confirmed`/not-found");
        assert_eq!(confirmed.status.as_str(), "CONFIRMED");
    })
    .await;
}
