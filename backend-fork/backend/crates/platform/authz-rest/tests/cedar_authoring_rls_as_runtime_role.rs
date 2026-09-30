#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Cedar Policy Studio store, exercised as the genuine non-owner role `console_rt`.
//!
//! Why `console_rt` and not the default `#[sqlx::test]` pool: that pool connects as a
//! BYPASSRLS superuser and would see every tenant's rows regardless of
//! `app.current_org`, green-lighting a broken isolation policy. We SEED as the
//! owner (catalog INSERT is revoked from `console_rt`) and RUN every authoring
//! mutation / point decision as `console_rt` under an armed org.
//!
//! Proves:
//!   (a) a no-code draft can never create an enforced/shadow row — it lands as
//!       `review_status = 'draft'`, and the `0103` CHECK rejects a direct
//!       enforced-status draft;
//!   (b) `simulate` returns Allow/Deny WITH determining-policy diagnostics;
//!   (c) `authorize` denies by omission (object type with no attached policy);
//!   (d) an object policy HIDES a cross-principal row (owner sees it, others do not);
//!   (e) `forbid` always wins over a matching permit (legal-hold guardrail).

use console_kernel_core::{OrgId, UserId};
use console_platform_authz::cedar_pbac::authoring::{
    self, AuthoredPolicy, Condition, ConditionOp, ConditionValue, Effect, NoCodeBlocks, SimEffect,
    SimRequest, SimResource, SimSubject,
};
use console_platform_authz_rest::{CreateDraftCommand, PgCedarPolicyStore};
use console_platform_request_context::scope_org;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0xA000_0000_0000_0000_0000_0000_0000_0001);

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

async fn seed_org(pool: &PgPool, org_id: Uuid, slug: &str) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(org_id)
        .bind(slug)
        .bind(format!("Org {slug}"))
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_user(pool: &PgPool, org_id: Uuid, name: &str) -> UserId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(name)
    .bind(["SUPER_ADMIN"].as_slice())
    .bind(org_id)
    .fetch_one(pool)
    .await
    .unwrap();
    UserId::from_uuid(id)
}

/// Seed a catalog entry as the OWNER (console_rt has no catalog INSERT). A `draft`
/// status entry needs no policy_version/bundle_digest, and the attach path reads
/// its `generated_policy_text` regardless of status.
async fn seed_catalog_entry(
    pool: &PgPool,
    org_id: Uuid,
    stable_key: &str,
    effect: &str,
    generated_text: &str,
) -> Uuid {
    sqlx::query_scalar(
        r#"
        INSERT INTO cedar_policy_catalog_entries
            (org_id, stable_key, title, natural_language_rule, effect, status, source,
             principal, action, resource, conditions, validation_status, generated_policy_text)
        VALUES ($1, $2, $3, 'authored in test', $4, 'draft', 'no_code_draft',
                '{}'::jsonb, '{}'::jsonb, '{}'::jsonb, '[]'::jsonb, 'valid', $5)
        RETURNING id
        "#,
    )
    .bind(org_id)
    .bind(stable_key)
    .bind(format!("Policy {stable_key}"))
    .bind(effect)
    .bind(generated_text)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn attach_object_policy(
    pool: &PgPool,
    org_id: Uuid,
    object_type_id: Uuid,
    cedar_policy_id: Uuid,
    effect: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO ont_object_policies (org_id, object_type_id, cedar_policy_id, effect)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(org_id)
    .bind(object_type_id)
    .bind(cedar_policy_id)
    .bind(effect)
    .execute(pool)
    .await
    .unwrap();
}

fn owner_view_permit() -> NoCodeBlocks {
    NoCodeBlocks {
        effect: Effect::Permit,
        action: "view".to_owned(),
        resource_type: "work_order".to_owned(),
        conditions: vec![Condition {
            attr: "owner".to_owned(),
            op: ConditionOp::Eq,
            value: ConditionValue::SubjectAttr("user_id".to_owned()),
        }],
    }
}

fn legal_hold_forbid() -> NoCodeBlocks {
    NoCodeBlocks {
        effect: Effect::Forbid,
        action: "view".to_owned(),
        resource_type: "work_order".to_owned(),
        conditions: vec![Condition {
            attr: "legal_hold".to_owned(),
            op: ConditionOp::Eq,
            value: ConditionValue::Bool(true),
        }],
    }
}

fn subject(user_id: &str) -> SimSubject {
    SimSubject {
        org: OrgId::from_uuid(ORG_A),
        user_id: user_id.to_owned(),
        roles: vec![],
        clearance_keys: vec![],
    }
}

fn row(owner: &str, legal_hold: Option<bool>) -> SimResource {
    SimResource {
        org: OrgId::from_uuid(ORG_A),
        resource_type: "work_order".to_owned(),
        resource_id: Some("wo-1".to_owned()),
        owner: Some(owner.to_owned()),
        branch: None,
        legal_hold,
    }
}

fn view_request(user_id: &str, resource: SimResource) -> SimRequest {
    SimRequest {
        subject: subject(user_id),
        action: "view".to_owned(),
        resource,
        purpose: None,
        field: None,
    }
}

// (a) A no-code draft is always a reviewable draft, never an enforced/shadow row.
#[sqlx::test(migrations = "../db/migrations")]
async fn draft_cannot_create_enforced_row(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let author = seed_user(&pool, ORG_A, "Author").await;
    let store = PgCedarPolicyStore::new(runtime_role_pool(&pool).await);

    let draft = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .create_draft(CreateDraftCommand {
                actor: author,
                draft_key: "ont.owner_view".to_owned(),
                title: "Owner view".to_owned(),
                author_note: None,
                blocks: owner_view_permit(),
            })
            .await
    })
    .await
    .unwrap();
    assert_eq!(draft.review_status, "draft", "a new draft must be 'draft'");
    assert_eq!(draft.validation_status, "valid");

    // The 0103 CHECK is the backstop: a draft whose normalized_row claims an
    // enforced status is rejected at the DB, even by a direct insert.
    let rt = store.pool().clone();
    let mut tx = rt.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let direct = sqlx::query(
        r#"
        INSERT INTO cedar_policy_drafts
            (org_id, draft_key, title, blocks, normalized_row, generated_policy_text,
             generated_policy_digest, validation_status, review_status, created_by, updated_by)
        VALUES ($1, 'ont.sneaky', 'Sneaky', '{}'::jsonb,
                '{"status":"enforced"}'::jsonb, 'permit(principal,action,resource);',
                'sha256:0000000000000000000000000000000000000000000000000000000000000000',
                'valid', 'draft', $2, $2)
        "#,
    )
    .bind(ORG_A)
    .bind(author.as_uuid())
    .execute(tx.as_mut())
    .await;
    assert!(
        direct.is_err(),
        "a draft with normalized_row.status='enforced' must be rejected by the CHECK"
    );
}

// (b) simulate returns Allow/Deny with determining-policy diagnostics.
#[sqlx::test(migrations = "../db/migrations")]
async fn simulate_reports_allow_deny_with_diagnostics(_pool: PgPool) {
    let text = authoring::generate_cedar_text(&owner_view_permit());
    let policies = [AuthoredPolicy::new("owner_view", text)];

    let allow = authoring::simulate(&policies, &view_request("alice", row("alice", None)));
    assert_eq!(allow.effect, SimEffect::Allow);
    assert_eq!(allow.determining_policies, vec!["owner_view".to_owned()]);
    assert!(allow.errors.is_empty());

    let deny = authoring::simulate(&policies, &view_request("bob", row("alice", None)));
    assert_eq!(deny.effect, SimEffect::Deny);
    assert!(
        deny.determining_policies.is_empty(),
        "deny-by-omission: no policy matched"
    );
}

// (c) authorize denies by omission when nothing is attached to the object type.
#[sqlx::test(migrations = "../db/migrations")]
async fn authorize_denies_by_omission(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let store = PgCedarPolicyStore::new(runtime_role_pool(&pool).await);
    let object_type_id = Uuid::new_v4();

    let outcome = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .authorize_object_row(object_type_id, &view_request("alice", row("alice", None)))
            .await
    })
    .await
    .unwrap();
    assert_eq!(outcome.effect, SimEffect::Deny, "no attached policy ⇒ deny");
}

// (d) an object policy hides a cross-principal row.
#[sqlx::test(migrations = "../db/migrations")]
async fn object_policy_hides_cross_principal_row(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let store = PgCedarPolicyStore::new(runtime_role_pool(&pool).await);
    let object_type_id = Uuid::new_v4();

    let permit = authoring::generate_cedar_text(&owner_view_permit());
    let cedar_id = seed_catalog_entry(&pool, ORG_A, "ont.owner_view", "permit", &permit).await;
    attach_object_policy(&pool, ORG_A, object_type_id, cedar_id, "permit").await;

    // Alice owns the row ⇒ visible.
    let owner_ok = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .authorize_object_row(object_type_id, &view_request("alice", row("alice", None)))
            .await
    })
    .await
    .unwrap();
    assert_eq!(owner_ok.effect, SimEffect::Allow);
    assert_eq!(owner_ok.determining_policies, vec![cedar_id.to_string()]);

    // Bob does not own it ⇒ hidden (deny-by-omission).
    let stranger = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .authorize_object_row(object_type_id, &view_request("bob", row("alice", None)))
            .await
    })
    .await
    .unwrap();
    assert_eq!(
        stranger.effect,
        SimEffect::Deny,
        "a non-owner must not see the row"
    );
}

// (e) forbid always wins over a matching permit (legal-hold guardrail).
#[sqlx::test(migrations = "../db/migrations")]
async fn forbid_always_wins(pool: PgPool) {
    seed_org(&pool, ORG_A, "org-alpha").await;
    let store = PgCedarPolicyStore::new(runtime_role_pool(&pool).await);
    let object_type_id = Uuid::new_v4();

    let permit = authoring::generate_cedar_text(&owner_view_permit());
    let forbid = authoring::generate_cedar_text(&legal_hold_forbid());
    let permit_id = seed_catalog_entry(&pool, ORG_A, "ont.owner_view", "permit", &permit).await;
    let forbid_id = seed_catalog_entry(&pool, ORG_A, "ont.legal_hold", "forbid", &forbid).await;
    attach_object_policy(&pool, ORG_A, object_type_id, permit_id, "permit").await;
    attach_object_policy(&pool, ORG_A, object_type_id, forbid_id, "forbid").await;

    // Alice owns the row but it is under legal hold ⇒ forbid wins ⇒ hidden.
    let held = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .authorize_object_row(
                object_type_id,
                &view_request("alice", row("alice", Some(true))),
            )
            .await
    })
    .await
    .unwrap();
    assert_eq!(
        held.effect,
        SimEffect::Deny,
        "forbid must win over the owner permit"
    );

    // Same owner/row without the hold is visible again.
    let free = scope_org(OrgId::from_uuid(ORG_A), async {
        store
            .authorize_object_row(object_type_id, &view_request("alice", row("alice", None)))
            .await
    })
    .await
    .unwrap();
    assert_eq!(
        free.effect,
        SimEffect::Allow,
        "no hold ⇒ owner permit applies"
    );
}
