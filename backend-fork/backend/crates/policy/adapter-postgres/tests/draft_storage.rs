#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_kernel_core::{OrgId, TraceContext, UserId};
use console_platform_request_context::scope_org;
use console_policy_adapter_postgres::PgPolicyStore;
use console_policy_application::{CedarPolicyDraftSaveCommand, CedarPolicyDraftSaveMode};
use console_policy_domain::{
    CedarActionSelector, CedarPolicyEffect, CedarPrincipalKind, CedarPrincipalSelector,
    CedarResourceScope, CedarResourceSelector,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn seed_org_and_user(owner_pool: &PgPool, org: OrgId) -> UserId {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
        .bind(*org.as_uuid())
        // organizations_slug_check caps slug at 40 chars; the hyphenated UUID
        // form ("policy-" + 36) is 43 and only passed when another test seeded
        // this org first (ON CONFLICT masks the bad insert). Use the 32-hex
        // simple form: "policy-" + 32 = 39, valid + unique per org.
        .bind(format!("policy-{}", org.as_uuid().simple()))
        .bind(format!("Policy Org {org}"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id, is_active) VALUES ($1, $2, $3, $4, true)")
        .bind(*user_id.as_uuid())
        .bind(format!("Policy User {user_id}"))
        .bind(vec!["ADMIN".to_owned()])
        .bind(*org.as_uuid())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    user_id
}

fn draft_command(actor: UserId) -> CedarPolicyDraftSaveCommand {
    CedarPolicyDraftSaveCommand {
        actor,
        title: "팀장 소속팀 근태 열람".to_owned(),
        author_note: Some("Policy canvas staging draft".to_owned()),
        principal: CedarPrincipalSelector::new(
            CedarPrincipalKind::JobFunction,
            Some("team_lead".to_owned()),
            None,
            "직책 · 팀장",
        )
        .unwrap(),
        action: CedarActionSelector::new("attendance_read", "근태 열람").unwrap(),
        resource: CedarResourceSelector::new(
            "attendance_record",
            None,
            CedarResourceScope::Team,
            "소속 팀원 근태",
        )
        .unwrap(),
        effect: CedarPolicyEffect::Permit,
        conditions: Vec::new(),
        save_mode: CedarPolicyDraftSaveMode::Draft,
        trace: TraceContext::generate(),
        occurred_at: datetime!(2026-07-09 12:00 UTC),
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn draft_save_is_audited_tenant_isolated_and_not_enforced(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222));
    let actor_a = seed_org_and_user(&owner_pool, org_a).await;
    seed_org_and_user(&owner_pool, org_b).await;

    let store = PgPolicyStore::new(rt_pool.clone());
    let saved = scope_org(org_a, store.save_draft(draft_command(actor_a)))
        .await
        .expect("draft save as console_rt under org A");

    assert_eq!(saved.enforcement_effect(), "none");
    assert_eq!(saved.draft.review_status.as_db_str(), "draft");
    assert_eq!(saved.draft.catalog_row.status.as_db_str(), "draft");

    let org_a_rows = scope_org(org_a, store.list_catalog_rows(Default::default()))
        .await
        .expect("catalog read org A");
    assert_eq!(org_a_rows.items.len(), 1);
    assert_eq!(
        org_a_rows.items[0].natural_language_rule,
        "직책 · 팀장 can perform 근태 열람 on 소속 팀원 근태."
    );
    assert_eq!(org_a_rows.items[0].effect.as_db_str(), "permit");
    assert_eq!(org_a_rows.items[0].status.as_db_str(), "draft");

    let org_b_rows = scope_org(org_b, store.list_catalog_rows(Default::default()))
        .await
        .expect("catalog read org B");
    assert!(
        org_b_rows.items.is_empty(),
        "org B must not see org A's draft"
    );

    let (audit_count, policy_version_count): (i64, i64) = {
        let mut tx = owner_pool.begin().await.unwrap();
        sqlx::query("SET LOCAL row_security = off")
            .execute(&mut *tx)
            .await
            .unwrap();
        let row = sqlx::query_as::<_, (i64, i64)>(
            "SELECT \
             (SELECT COUNT(*) FROM audit_events WHERE action = 'policy.cedar_draft.create' AND org_id = $1), \
             (SELECT COUNT(*) FROM policy_versions WHERE org_id = $1)",
        )
        .bind(*org_a.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        row
    };
    assert_eq!(audit_count, 1, "draft save must append an audit event");
    assert_eq!(
        policy_version_count, 0,
        "draft save must not bump live policy_versions"
    );
}
