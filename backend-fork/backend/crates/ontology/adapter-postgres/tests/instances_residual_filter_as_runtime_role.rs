#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proofs for the L-CEDAR-residual list-filter (arch §5d / decision D1),
//! exercised as the genuine non-owner `console_rt` role (NOSUPERUSER, NOBYPASSRLS,
//! FORCE RLS) — the only faithful exercise of the org_isolation floor. A BYPASSRLS
//! superuser would mask a residual that leaks across tenants.
//!
//! Proves the residual composes as `WHERE <RLS org floor> AND <residual>`:
//!   (a) permit-with-condition (`owner == principal.user_id`) shows ONLY matching
//!       rows — the row-filter is pushed to SQL, not a per-row loop;
//!   (b) no applicable permit ⇒ zero rows (deny-by-omission, `WHERE FALSE`);
//!   (c) `forbid` excludes a row even when a permit matches it (forbid wins);
//!   (d) an untranslatable term (subject attr the request doesn't carry) ⇒ zero
//!       rows — fail-closed, never a silent widen;
//!   (e) the RLS floor still holds: another org's row stays invisible even when a
//!       permit would otherwise match it.
//!
//! NOTE (migrations path): runs against the canonical
//! `../../platform/db/migrations` (the ship path). The earlier concurrent-lane
//! migration-number collision has been reconciled, so no deduplicated copy is
//! needed.

use console_ontology_adapter_postgres::instances::{
    AggregateGroupBy, CreateInstance, PgInstanceStore,
};
use console_ontology_adapter_postgres::{CreateObjectTypeDraft, PgOntologyStore, PropertyDefInput};
use console_ontology_domain::{BackingKind, ObjectTypeId};

use console_kernel_core::{OrgId, TraceContext, UserId};
use console_platform_authz::cedar_pbac::authoring::Effect;
use console_platform_authz::cedar_pbac::residual::{
    ObjectPolicy, Predicate, PredicateValue, ResidualOp, SqlValue, SubjectAttrs,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x4444_4444_4444_4444_4444_4444_4444_4444);

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

async fn command_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_ontology_cmd")
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn seed_org_and_user(owner_pool: &PgPool, org: Uuid, tag: &str) -> UserId {
    let slug = format!("org-{}", &org.simple().to_string()[..12]);
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
        .bind(org)
        .bind(slug)
        .bind(format!("Org {tag}"))
        .execute(owner_pool)
        .await
        .unwrap();
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Admin {tag}"))
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

/// Publish an `instance`-backed object type with a required `owner` text property
/// and an optional `flagged` boolean property.
async fn seed_object_type(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    key: &str,
) -> ObjectTypeId {
    console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(owner_pool.clone())
            .with_command_pool(command_role_pool(owner_pool).await);
        let draft = CreateObjectTypeDraft {
            stable_key: key.to_owned(),
            title: "케이스".to_owned(),
            title_property_key: Some("title".to_owned()),
            backing_kind: BackingKind::Instance,
            backing_table: None,
            primary_key_property: None,
            properties: vec![
                PropertyDefInput {
                    key: "owner".to_owned(),
                    title: "담당자".to_owned(),
                    field_type: "text".to_owned(),
                    config: serde_json::json!({}),
                    backing_column: None,
                    required: true,
                    in_property_policy: false,
                },
                PropertyDefInput {
                    key: "flagged".to_owned(),
                    title: "보류".to_owned(),
                    field_type: "boolean".to_owned(),
                    config: serde_json::json!({}),
                    backing_column: None,
                    required: false,
                    in_property_policy: false,
                },
            ],
            links: Vec::new(),
            actions: Vec::new(),
            analytics: Vec::new(),
        };
        store
            .create_object_type(
                actor,
                draft,
                TraceContext::generate(),
                datetime!(2026-07-09 12:00 UTC),
            )
            .await
            .expect("create object type")
            .id
    })
    .await
}

fn instance(object_type_id: ObjectTypeId, owner: &str, flagged: Option<bool>) -> CreateInstance {
    let attributes = match flagged {
        Some(flag) => serde_json::json!({ "owner": owner, "flagged": flag }),
        None => serde_json::json!({ "owner": owner }),
    };
    CreateInstance {
        object_type_id,
        title: format!("case-{owner}"),
        attributes,
        valid_from: None,
        action_type_id: None,
        reason: Some("seed".to_owned()),
    }
}

fn owner_permit() -> ObjectPolicy {
    ObjectPolicy {
        effect: Effect::Permit,
        predicates: vec![Predicate {
            field: "owner".to_owned(),
            op: ResidualOp::Eq,
            value: PredicateValue::SubjectAttr("user_id".to_owned()),
        }],
    }
}

fn flagged_forbid() -> ObjectPolicy {
    ObjectPolicy {
        effect: Effect::Forbid,
        predicates: vec![Predicate {
            field: "flagged".to_owned(),
            op: ResidualOp::Eq,
            value: PredicateValue::Literal(SqlValue::Bool(true)),
        }],
    }
}

fn subject(user_id: &str) -> SubjectAttrs {
    SubjectAttrs::default().with_scalar("user_id", user_id)
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn residual_permit_forbid_and_untranslatable(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "a").await;
    let type_id = seed_object_type(&owner_pool, org, actor, "case.inst").await;
    let at = datetime!(2026-07-09 12:00 UTC);

    console_platform_request_context::scope_org(org, async {
        let store = PgInstanceStore::new(rt.clone());
        // alice owns two (one flagged), bob owns one.
        store
            .create_instance(
                actor,
                instance(type_id, "alice", None),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap();
        store
            .create_instance(
                actor,
                instance(type_id, "alice", Some(true)),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap();
        store
            .create_instance(
                actor,
                instance(type_id, "bob", None),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap();

        // Baseline: unfiltered list sees all three (RLS floor only).
        assert_eq!(store.list_instances(type_id).await.unwrap().len(), 3);

        // (a) permit `owner == user_id[alice]` ⇒ alice's TWO rows, not bob's.
        let alice = subject("alice");
        let permitted = store
            .list_instances_filtered(type_id, &alice, &[owner_permit()])
            .await
            .unwrap();
        assert_eq!(permitted.len(), 2, "only alice-owned rows are visible");
        assert!(
            permitted
                .iter()
                .all(|s| s.revision.attributes["owner"] == "alice")
        );

        // (b) no applicable permit ⇒ zero rows (deny-by-omission).
        let denied = store
            .list_instances_filtered(type_id, &alice, &[])
            .await
            .unwrap();
        assert!(denied.is_empty(), "no permit ⇒ WHERE FALSE ⇒ zero rows");

        // (c) forbid `flagged == true` excludes alice's flagged row even though the
        // permit matches it ⇒ only the ONE unflagged alice row survives.
        let with_forbid = store
            .list_instances_filtered(type_id, &alice, &[owner_permit(), flagged_forbid()])
            .await
            .unwrap();
        assert_eq!(with_forbid.len(), 1, "forbid excludes the flagged row");
        // The survivor is the un-flagged alice row: it carries no `flagged` key at
        // all, so `attributes["flagged"]` is JSON null — the absent-attribute NULL
        // path the COALESCE composition is built to survive (a naive `NOT NULL`
        // would have wrongly dropped it).
        assert_ne!(
            with_forbid[0].revision.attributes["flagged"],
            serde_json::Value::Bool(true),
            "the surviving row is the un-flagged one"
        );

        // (d) untranslatable term (subject attr the request lacks) ⇒ zero rows.
        let untranslatable = ObjectPolicy {
            effect: Effect::Permit,
            predicates: vec![Predicate {
                field: "owner".to_owned(),
                op: ResidualOp::Eq,
                value: PredicateValue::SubjectAttr("missing_attr".to_owned()),
            }],
        };
        let out = store
            .list_instances_filtered(type_id, &alice, &[untranslatable])
            .await
            .unwrap();
        assert!(
            out.is_empty(),
            "untranslatable term ⇒ fail-closed zero rows"
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn residual_cannot_widen_past_the_rls_floor(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);
    let actor_a = seed_org_and_user(&owner_pool, *org_a.as_uuid(), "a").await;
    let actor_b = seed_org_and_user(&owner_pool, *org_b.as_uuid(), "b").await;
    let type_a = seed_object_type(&owner_pool, org_a, actor_a, "case.a").await;
    let type_b = seed_object_type(&owner_pool, org_b, actor_b, "case.b").await;
    let at = datetime!(2026-07-09 12:00 UTC);

    // Both orgs have an alice-owned instance.
    console_platform_request_context::scope_org(org_a, async {
        PgInstanceStore::new(rt.clone())
            .create_instance(
                actor_a,
                instance(type_a, "alice", None),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap();
    })
    .await;
    console_platform_request_context::scope_org(org_b, async {
        PgInstanceStore::new(rt.clone())
            .create_instance(
                actor_b,
                instance(type_b, "alice", None),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap();
    })
    .await;

    // (e) Under org-A's GUC, a permit that matches alice everywhere still cannot
    // reveal org-B's row — RLS is the hard floor the residual only narrows.
    console_platform_request_context::scope_org(org_a, async {
        let store = PgInstanceStore::new(rt.clone());
        let alice = subject("alice");
        let a_rows = store
            .list_instances_filtered(type_a, &alice, &[owner_permit()])
            .await
            .unwrap();
        assert_eq!(a_rows.len(), 1, "A sees its own alice row");
        // B's type queried under A's GUC yields nothing (cross-tenant invisible).
        let b_rows = store
            .list_instances_filtered(type_b, &alice, &[owner_permit()])
            .await
            .unwrap();
        assert!(
            b_rows.is_empty(),
            "residual cannot widen past the RLS floor"
        );
    })
    .await;

    // Fail-closed with no org armed: even a matching permit yields an error/empty.
    let store = PgInstanceStore::new(rt.clone());
    assert!(
        store
            .list_instances_filtered(type_a, &subject("alice"), &[owner_permit()])
            .await
            .is_err(),
        "list must fail closed without an armed org"
    );
}

// ---------------------------------------------------------------------------
// Aggregate path (ADR-0030 §7.5 / console-09c): same residual as list, COUNT
// without materialising rows.
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn aggregate_instances_subject_counts_match_list_and_deny_all_as_runtime_role(
    owner_pool: PgPool,
) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "agg").await;
    let type_id = seed_object_type(&owner_pool, org, actor, "case.agg").await;
    let at = datetime!(2026-07-09 12:00 UTC);

    console_platform_request_context::scope_org(org, async {
        let store = PgInstanceStore::new(rt.clone());
        // alice: 2, bob: 1 — same seed shape as the residual list proofs.
        store
            .create_instance(
                actor,
                instance(type_id, "alice", None),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap();
        store
            .create_instance(
                actor,
                instance(type_id, "alice", Some(false)),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap();
        store
            .create_instance(
                actor,
                instance(type_id, "bob", None),
                TraceContext::generate(),
                at,
            )
            .await
            .unwrap();

        let alice = subject("alice");
        let bob = subject("bob");
        let permit = [owner_permit()];

        // (a) two subjects with different policies → different counts.
        let alice_agg = store
            .aggregate_instances(
                type_id,
                AggregateGroupBy::Attribute("owner".into()),
                &alice,
                &permit,
            )
            .await
            .unwrap();
        let bob_agg = store
            .aggregate_instances(
                type_id,
                AggregateGroupBy::Attribute("owner".into()),
                &bob,
                &permit,
            )
            .await
            .unwrap();
        let alice_total: i64 = alice_agg.iter().map(|b| b.count).sum();
        let bob_total: i64 = bob_agg.iter().map(|b| b.count).sum();
        assert_eq!(alice_total, 2, "alice sees only her rows");
        assert_eq!(bob_total, 1, "bob sees only his row");
        assert_ne!(alice_total, bob_total);

        // (b) counts equal list-path row count for the same subject (anti-drift).
        let alice_list = store
            .list_instances_filtered(type_id, &alice, &permit)
            .await
            .unwrap();
        assert_eq!(alice_total as usize, alice_list.len());
        let bob_list = store
            .list_instances_filtered(type_id, &bob, &permit)
            .await
            .unwrap();
        assert_eq!(bob_total as usize, bob_list.len());

        // lifecycle_state group-by also matches list length (all draft on create).
        let by_lifecycle = store
            .aggregate_instances(type_id, AggregateGroupBy::LifecycleState, &alice, &permit)
            .await
            .unwrap();
        let life_total: i64 = by_lifecycle.iter().map(|b| b.count).sum();
        assert_eq!(life_total as usize, alice_list.len());

        // (c) deny-all (no permit) → zero, not everything.
        let denied = store
            .aggregate_instances(
                type_id,
                AggregateGroupBy::Attribute("owner".into()),
                &alice,
                &[],
            )
            .await
            .unwrap();
        assert!(
            denied.is_empty() || denied.iter().all(|b| b.count == 0),
            "deny-all must not return the unfiltered universe: {denied:?}"
        );
        let denied_total: i64 = denied.iter().map(|b| b.count).sum();
        assert_eq!(denied_total, 0, "deny-all ⇒ zero rows counted");

        // Injection fail-closed: free-form JSON path must not reach SQL text.
        let poisoned = store
            .aggregate_instances(
                type_id,
                AggregateGroupBy::Attribute("owner'; drop table ont_instances;--".into()),
                &alice,
                &permit,
            )
            .await;
        assert!(
            poisoned.is_err(),
            "unsafe attribute group key must fail closed"
        );
    })
    .await;
}
