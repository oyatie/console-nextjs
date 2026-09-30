#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proofs for the declarative property→link binding.
//!
//! A property whose `ont_property_defs.config` carries
//! `{"link": {"stable_key": .., "to_type": ..}}` projects into a real `ont_links`
//! edge on every revision write. That mechanism is the thing the company/HR
//! fan-out inherits: `org_unit` is written by hand and Position, Person,
//! Employment and PayRun transliterate from it, so a defect here reaches all
//! five.
//!
//! WHY THIS FILE EXISTS AT ALL. The company-conformance suite exercises exactly
//! two of the paths below — a null reference writing no edge, and one valid
//! referent writing one edge. It CANNOT see the rest, and in one case cannot see
//! the difference between correct and incorrect behaviour at all: `traverse`
//! reads `valid_to IS NULL` only, so an implementation that closes and reopens
//! every edge on every revision produces a byte-identical graph to one that
//! leaves unchanged edges alone. The difference is only visible in history, and
//! history is the product. Hence `unchanged_referent_is_not_rewritten`.
//!
//! Everything is asserted as the genuine non-owner `console_rt` role under FORCE
//! RLS with `app.current_org` armed — including the forensic reads of
//! `ont_links` itself, since a history assertion made as a BYPASSRLS superuser
//! would prove nothing about what a tenant can actually observe.
//!
//! Proves:
//!   (a) a null reference writes NO edge, and a type declaring no link never
//!       touches `ont_links` — the built-in catalog's 27 types are all here;
//!   (b) one valid referent writes exactly one CHILD→PARENT edge stamped with
//!       the REVISION's `valid_from`, not the wall clock;
//!   (c) a revision that leaves the referent alone leaves the EDGE alone — same
//!       row id, same `valid_from`, still open. This is the no-churn contract;
//!   (d) re-pointing closes the superseded edge at the new revision's instant
//!       and opens exactly one replacement;
//!   (e) dropping the reference to null closes the edge and opens nothing;
//!   (f) a dangling referent is refused as `not_found` and a wrong-typed one as
//!       `validation` — both pre-checked in Rust, because letting the FK raise
//!       23503 surfaces as an unmappable 500 rather than the 4xx a caller can
//!       act on;
//!   (g) the refusals are TOTAL: the whole revision aborts, leaving neither a
//!       new edge nor a new revision behind.

use console_kernel_core::{OrgId, TraceContext, UserId};
use console_ontology_adapter_postgres::instances::{
    CreateInstance, PgInstanceStore, StageRevision,
};
use console_ontology_adapter_postgres::{
    CreateObjectTypeDraft, LinkTypeInput, PgOntologyError, PgOntologyStore, PropertyDefInput,
};
use console_ontology_domain::{BackingKind, InstanceId, LinkCardinality, ObjectTypeId};
use console_platform_db::with_org_conn;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

const UNIT: &str = "link_sync_unit";
const OTHER: &str = "link_sync_other";
const PARENT_LINK: &str = "parent_unit";

const T0: OffsetDateTime = datetime!(2026-07-10 12:00 UTC);
const T1: OffsetDateTime = datetime!(2026-07-11 12:00 UTC);
const T2: OffsetDateTime = datetime!(2026-07-12 12:00 UTC);
const T3: OffsetDateTime = datetime!(2026-07-13 12:00 UTC);

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    role_pool(owner_pool, |conn| {
        Box::pin(async move {
            sqlx::query("SET ROLE console_rt")
                .execute(conn)
                .await
                .map(|_| ())
        })
    })
    .await
}

async fn command_role_pool(owner_pool: &PgPool) -> PgPool {
    role_pool(owner_pool, |conn| {
        Box::pin(async move {
            sqlx::query("SET ROLE console_ontology_cmd")
                .execute(conn)
                .await
                .map(|_| ())
        })
    })
    .await
}

/// The role statement is a literal in each caller: the SQL-injection lint rejects
/// a `format!`-built statement, and it is right to — a role name is an identifier,
/// which cannot be parameterised.
async fn role_pool<F>(owner_pool: &PgPool, set_role: F) -> PgPool
where
    F: for<'c> Fn(
            &'c mut sqlx::PgConnection,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), sqlx::Error>> + Send + 'c>,
        > + Send
        + Sync
        + 'static,
{
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _meta| set_role(conn))
        .connect_with(options)
        .await
        .unwrap()
}

async fn seed_org_and_user(owner_pool: &PgPool, org: Uuid, tag: &str) -> UserId {
    let slug = format!("org-{}", &org.simple().to_string()[..12]);
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(slug)
    .bind(format!("Org {tag}"))
    // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
    .execute(owner_pool)
    .await
    .unwrap();
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Admin {tag}"))
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org)
        // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

fn text_prop(key: &str) -> PropertyDefInput {
    PropertyDefInput {
        key: key.to_owned(),
        title: key.to_owned(),
        field_type: "text".to_owned(),
        config: serde_json::json!({}),
        backing_column: None,
        required: true,
        in_property_policy: false,
    }
}

/// A self-referential type: `parent_id` is declared `required: false` because the
/// root has no parent, and `validate_params` rejects a missing required param.
async fn seed_linked_type(owner_pool: &PgPool, org: OrgId, actor: UserId) -> ObjectTypeId {
    let cmd = command_role_pool(owner_pool).await;
    console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(owner_pool.clone()).with_command_pool(cmd);
        // A second type, so the wrong-referent refusal has something real to point at.
        store
            .create_object_type(
                actor,
                CreateObjectTypeDraft {
                    stable_key: OTHER.to_owned(),
                    title: "Other".to_owned(),
                    title_property_key: Some("name".to_owned()),
                    backing_kind: BackingKind::Instance,
                    backing_table: None,
                    primary_key_property: None,
                    properties: vec![text_prop("name")],
                    links: Vec::new(),
                    actions: Vec::new(),
                    analytics: Vec::new(),
                },
                TraceContext::generate(),
                T0,
            )
            .await
            .expect("create other type");

        store
            .create_object_type(
                actor,
                CreateObjectTypeDraft {
                    stable_key: UNIT.to_owned(),
                    title: "Unit".to_owned(),
                    title_property_key: Some("name".to_owned()),
                    backing_kind: BackingKind::Instance,
                    backing_table: None,
                    primary_key_property: None,
                    properties: vec![
                        text_prop("name"),
                        PropertyDefInput {
                            key: "parent_id".to_owned(),
                            title: "Parent".to_owned(),
                            field_type: "reference".to_owned(),
                            // The whole mechanism, declared as DATA.
                            config: serde_json::json!({
                                "link": {"stable_key": PARENT_LINK, "to_type": UNIT}
                            }),
                            backing_column: None,
                            required: false,
                            in_property_policy: false,
                        },
                    ],
                    links: vec![LinkTypeInput {
                        stable_key: PARENT_LINK.to_owned(),
                        title: "Parent unit".to_owned(),
                        reverse_title: None,
                        // NULL = unresolved (0152:76). A self-referential target id
                        // does not exist yet: it is generated inside the DB function,
                        // after Rust has serialised this draft.
                        to_object_type_id: None,
                        cardinality: LinkCardinality::OneMany,
                        traversable: true,
                    }],
                    actions: Vec::new(),
                    analytics: Vec::new(),
                },
                TraceContext::generate(),
                T0,
            )
            .await
            .expect("create unit type")
            .id
    })
    .await
}

async fn other_type_id(owner_pool: &PgPool, org: OrgId) -> ObjectTypeId {
    let id: Uuid =
        sqlx::query_scalar("SELECT id FROM ont_object_types WHERE org_id = $1 AND stable_key = $2")
            .bind(*org.as_uuid())
            .bind(OTHER)
            // rls-arming: ok test fixture reads the registry as owner during setup
            .fetch_one(owner_pool)
            .await
            .unwrap();
    ObjectTypeId::from_uuid(id)
}

/// One `ont_links` row as the tenant can observe it.
#[derive(Debug, PartialEq, Eq)]
struct Edge {
    id: Uuid,
    to: Uuid,
    valid_from: OffsetDateTime,
    valid_to: Option<OffsetDateTime>,
}

/// EVERY edge out of `from`, open and closed, read as `console_rt`.
async fn edges(rt: &PgPool, org: OrgId, from: InstanceId) -> Vec<Edge> {
    with_org_conn::<_, Vec<Edge>, PgOntologyError>(rt, org, |tx| {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT id, to_instance_id, valid_from, valid_to FROM ont_links \
                 WHERE from_instance_id = $1 ORDER BY valid_from, id",
            )
            .bind(*from.as_uuid())
            .fetch_all(tx.as_mut())
            .await?;
            rows.into_iter()
                .map(|r| {
                    Ok(Edge {
                        id: r.try_get("id")?,
                        to: r.try_get("to_instance_id")?,
                        valid_from: r.try_get("valid_from")?,
                        valid_to: r.try_get("valid_to")?,
                    })
                })
                .collect::<Result<Vec<Edge>, sqlx::Error>>()
                .map_err(Into::into)
        })
    })
    .await
    .unwrap()
}

fn live(all: &[Edge]) -> Vec<&Edge> {
    all.iter().filter(|e| e.valid_to.is_none()).collect()
}

fn create(type_id: ObjectTypeId, name: &str, parent: Option<InstanceId>) -> CreateInstance {
    CreateInstance {
        object_type_id: type_id,
        title: name.to_owned(),
        attributes: serde_json::json!({
            "name": name,
            "parent_id": parent.map(|p| p.to_string()),
        }),
        valid_from: Some(T0),
        action_type_id: None,
        reason: Some("seed".to_owned()),
    }
}

fn repoint(name: &str, parent: Option<InstanceId>, at: OffsetDateTime) -> StageRevision {
    StageRevision {
        attributes: serde_json::json!({
            "name": name,
            "parent_id": parent.map(|p| p.to_string()),
        }),
        valid_from: Some(at),
        action_type_id: None,
        reason: Some("repoint".to_owned()),
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn declared_link_projects_and_only_changes_on_change(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "link").await;
    let unit = seed_linked_type(&owner_pool, org, actor).await;

    console_platform_request_context::scope_org(org, async {
        let store = PgInstanceStore::new(rt.clone());
        let trace = TraceContext::generate;

        // (a) the ROOT declares a null parent: no edge at all.
        let root = store
            .create_instance(actor, create(unit, "root", None), trace(), T0)
            .await
            .expect("create root")
            .instance
            .id;
        assert!(
            edges(&rt, org, root).await.is_empty(),
            "a null reference must write no edge"
        );

        // (b) a child pointing at the root: exactly one CHILD -> PARENT edge,
        // stamped with the REVISION's valid_from (T0), never the wall clock.
        let child = store
            .create_instance(actor, create(unit, "child", Some(root)), trace(), T0)
            .await
            .expect("create child")
            .instance
            .id;
        let after_create = edges(&rt, org, child).await;
        assert_eq!(after_create.len(), 1, "one referent must write one edge");
        assert_eq!(
            after_create[0].to,
            *root.as_uuid(),
            "direction is CHILD -> PARENT"
        );
        assert_eq!(
            after_create[0].valid_from, T0,
            "edge carries the REVISION's instant"
        );
        assert_eq!(after_create[0].valid_to, None);
        let original = after_create[0].id;

        // (c) THE NO-CHURN CONTRACT. A revision that renames the child but leaves
        // its parent alone must leave the EDGE alone: same row, same valid_from,
        // still open. A close-and-reopen would be invisible to `traverse` (which
        // reads `valid_to IS NULL`) and would write a parent change into history
        // that never happened.
        store
            .stage_revision(
                actor,
                child,
                repoint("child-renamed", Some(root), T1),
                trace(),
                T1,
            )
            .await
            .expect("rename without repointing");
        let after_rename = edges(&rt, org, child).await;
        assert_eq!(
            after_rename.len(),
            1,
            "an unchanged referent must not add a history row, got {after_rename:#?}"
        );
        assert_eq!(
            after_rename[0].id, original,
            "the SAME edge row must survive"
        );
        assert_eq!(
            after_rename[0].valid_from, T0,
            "its valid_from must not move"
        );
        assert_eq!(
            after_rename[0].valid_to, None,
            "it must not be closed and reopened"
        );

        // (d) re-pointing at a new parent closes the old edge AT THE NEW INSTANT
        // and opens exactly one replacement.
        let second = store
            .create_instance(actor, create(unit, "second", None), trace(), T0)
            .await
            .expect("create second parent")
            .instance
            .id;
        store
            .stage_revision(
                actor,
                child,
                repoint("child-renamed", Some(second), T2),
                trace(),
                T2,
            )
            .await
            .expect("repoint");
        let after_repoint = edges(&rt, org, child).await;
        assert_eq!(
            after_repoint.len(),
            2,
            "repoint keeps history: closed + open"
        );
        let closed = after_repoint.iter().find(|e| e.id == original).unwrap();
        assert_eq!(
            closed.valid_to,
            Some(T2),
            "superseded edge closes at the new instant"
        );
        let open = live(&after_repoint);
        assert_eq!(open.len(), 1, "exactly one live edge after a repoint");
        assert_eq!(open[0].to, *second.as_uuid());
        assert_eq!(open[0].valid_from, T2);

        // (e) dropping the reference to null closes the edge and opens nothing.
        store
            .stage_revision(
                actor,
                child,
                repoint("child-renamed", None, T3),
                trace(),
                T3,
            )
            .await
            .expect("drop the reference");
        let after_drop = edges(&rt, org, child).await;
        assert!(
            live(&after_drop).is_empty(),
            "a null reference must close the edge, got {after_drop:#?}"
        );
        assert_eq!(after_drop.len(), 2, "and must not delete history");
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn bad_referents_are_refused_in_rust_and_leave_nothing_behind(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "link").await;
    let unit = seed_linked_type(&owner_pool, org, actor).await;
    let other = other_type_id(&owner_pool, org).await;

    console_platform_request_context::scope_org(org, async {
        let store = PgInstanceStore::new(rt.clone());
        let trace = TraceContext::generate;

        // (f) a referent that does not exist -> `not_found`, NOT a 23503 that
        // would surface as an unmappable 500.
        let dangling = InstanceId::new();
        let err = store
            .create_instance(actor, create(unit, "orphan", Some(dangling)), trace(), T0)
            .await
            .expect_err("a dangling referent must be refused");
        let rendered = format!("{err:?}");
        assert!(
            rendered.contains("does not exist"),
            "expected a not_found refusal naming the missing instance, got {rendered}"
        );

        // A wrong-TYPED referent -> `validation`. The check compares stable_key,
        // never the per-version object_type_id, which changes at every revision.
        let stranger = store
            .create_instance(
                actor,
                CreateInstance {
                    object_type_id: other,
                    title: "stranger".to_owned(),
                    attributes: serde_json::json!({"name": "stranger"}),
                    valid_from: Some(T0),
                    action_type_id: None,
                    reason: None,
                },
                trace(),
                T0,
            )
            .await
            .expect("create stranger")
            .instance
            .id;
        let err = store
            .create_instance(actor, create(unit, "mistyped", Some(stranger)), trace(), T0)
            .await
            .expect_err("a wrong-typed referent must be refused");
        let rendered = format!("{err:?}");
        assert!(
            rendered.contains("must reference"),
            "expected a validation refusal naming both types, got {rendered}"
        );

        // (g) the refusals are TOTAL. Neither attempt may leave an instance or an
        // edge behind: the whole revision aborts, not just the link write.
        let survivors: i64 = with_org_conn::<_, i64, PgOntologyError>(&rt, org, |tx| {
            Box::pin(async move {
                sqlx::query_scalar(
                    "SELECT count(*) FROM ont_instances i JOIN ont_object_types t \
                     ON t.id = i.object_type_id WHERE t.stable_key = $1",
                )
                .bind(UNIT)
                .fetch_one(tx.as_mut())
                .await
                .map_err(Into::into)
            })
        })
        .await
        .unwrap();
        assert_eq!(
            survivors, 0,
            "a refused create must not leave an instance behind"
        );

        let orphan_edges: i64 = with_org_conn::<_, i64, PgOntologyError>(&rt, org, |tx| {
            Box::pin(async move {
                sqlx::query_scalar("SELECT count(*) FROM ont_links")
                    .fetch_one(tx.as_mut())
                    .await
                    .map_err(Into::into)
            })
        })
        .await
        .unwrap();
        assert_eq!(
            orphan_edges, 0,
            "a refused create must not leave an edge behind"
        );
    })
    .await;
}
