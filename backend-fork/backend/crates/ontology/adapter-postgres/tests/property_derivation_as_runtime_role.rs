#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proofs for the declarative property derivation.
//!
//! A property whose `ont_property_defs.config` carries
//! `{"derive": {"op": "sum", "over": .., "of": .., "to_type": ..}}` is COMPUTED
//! at revision writeback from the referenced instances, before the bag is
//! validated and fixity-hashed. `pay_run.gross_total` is the first user, but
//! nothing here names it: the mechanism is the sibling of the property -> link
//! binding and the same five types inherit it.
//!
//! WHY THIS FILE EXISTS AT ALL. The company-conformance suite's CC-10 is the
//! only scenario that exercises derivation, and it is structurally blind to the
//! decision that matters most: CC-06 re-sends the identical `hire()` bag when it
//! transfers, so the employment's `base_salary` is the same in v1 and v2 and a
//! CURRENT-HEAD read produces the same number as the effective-dated read. An
//! implementation that reads the head passes CC-10 and is wrong — it would let
//! an unrelated later raise change a total already sealed into the hash chain.
//! Only a fixture whose salary CHANGES across the boundary separates them, which
//! is what `derivation_reads_the_referent_effective_at_the_revision_instant`
//! does. CC-10 is equally blind to every refusal below, to the second revision
//! writer, and to the fixity boundary.
//!
//! Everything is asserted as the genuine non-owner `console_rt` role under FORCE
//! RLS with `app.current_org` armed, including the forensic read-backs: a
//! BYPASSRLS superuser read would prove nothing about what a tenant observes.
//!
//! Proves:
//!   (a) a type declaring no `derive` is untouched — its stored bag is exactly
//!       what the caller sent, with no key invented;
//!   (b) the sum is real arithmetic over the DECLARED population: two referents
//!       and one referent give two different totals, so no constant satisfies
//!       both (the CC-10 contract, reproduced where it can actually run);
//!   (c) an empty population derives 0, not a refusal;
//!   (d) a caller-sent value is OVERWRITTEN — `params_schema` lists the derived
//!       property, so a client can send one, and only overwriting makes the
//!       value derived rather than defaulted;
//!   (e) `stage_revision_in_tx` derives too, not only `create_instance_in_tx`;
//!   (f) the derived value is inside the FIXITY hash: `verify_chain` recomputes
//!       from the stored attributes and reports no break, which it could not do
//!       if the value were written after `canonical_revision`;
//!   (g) referents are read AS OF the writing revision's `valid_from`, never at
//!       the head — the assertion CC-10 cannot make;
//!   (h) every failure is refused in Rust as a `KernelError`, never dropped as a
//!       silently smaller total: a missing referent, one with no revision
//!       effective at the instant, a wrong-typed one, a non-integer term, an
//!       overflow, an unimplemented `op`, a malformed declaration and a
//!       malformed id list;
//!   (i) the refusals are TOTAL — the whole revision aborts, leaving nothing.

use console_kernel_core::{OrgId, TraceContext, UserId};
use console_ontology_adapter_postgres::instances::{
    CreateInstance, PgInstanceStore, StageRevision, verify_chain,
};
use console_ontology_adapter_postgres::{
    CreateObjectTypeDraft, PgOntologyError, PgOntologyStore, PropertyDefInput,
};
use console_ontology_domain::{BackingKind, InstanceId, ObjectTypeId};
use console_platform_db::with_org_conn;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

const EMPLOYMENT: &str = "deriv_employment";
const PAY_RUN: &str = "deriv_pay_run";

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

async fn seed_org_and_user(owner_pool: &PgPool, org: Uuid) -> UserId {
    let slug = format!("org-{}", &org.simple().to_string()[..12]);
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(slug)
    .bind("Org derive")
    // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
    .execute(owner_pool)
    .await
    .unwrap();
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind("Admin derive")
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org)
        // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

fn prop(key: &str, field_type: &str, required: bool, config: Value) -> PropertyDefInput {
    PropertyDefInput {
        key: key.to_owned(),
        title: key.to_owned(),
        field_type: field_type.to_owned(),
        config,
        backing_column: None,
        required,
        in_property_policy: false,
    }
}

/// `gross_total` is declared `required: false` for a reason the engine enforces:
/// the publish auto-attaches a `create` action whose `params_schema` mirrors
/// `required` (`0165:1030`), and `validate_params` would reject the bag with
/// "required param 'gross_total' is missing" before anything could derive.
fn derived_total(over: &str, of: &str, to_type: &str, op: &str) -> PropertyDefInput {
    prop(
        "total",
        "integer",
        false,
        json!({"derive": {"op": op, "over": over, "of": of, "to_type": to_type}}),
    )
}

async fn seed_type(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    stable_key: &str,
    title_property_key: &str,
    properties: Vec<PropertyDefInput>,
) -> ObjectTypeId {
    let cmd = command_role_pool(owner_pool).await;
    console_platform_request_context::scope_org(org, async {
        PgOntologyStore::new(owner_pool.clone())
            .with_command_pool(cmd)
            .create_object_type(
                actor,
                CreateObjectTypeDraft {
                    stable_key: stable_key.to_owned(),
                    title: stable_key.to_owned(),
                    title_property_key: Some(title_property_key.to_owned()),
                    backing_kind: BackingKind::Instance,
                    backing_table: None,
                    primary_key_property: None,
                    properties,
                    links: Vec::new(),
                    actions: Vec::new(),
                    analytics: Vec::new(),
                },
                TraceContext::generate(),
                T0,
            )
            .await
            .unwrap_or_else(|e| panic!("create type {stable_key}: {e:?}"))
            .id
    })
    .await
}

/// The two types under test: an `employment` carrying an integer salary, and a
/// `pay_run` whose total is DERIVED from the employments its bag names.
async fn seed_pair(owner_pool: &PgPool, org: OrgId, actor: UserId) -> (ObjectTypeId, ObjectTypeId) {
    let employment = seed_type(
        owner_pool,
        org,
        actor,
        EMPLOYMENT,
        "name",
        vec![
            prop("name", "text", true, json!({})),
            prop("base_salary", "integer", true, json!({})),
        ],
    )
    .await;
    let pay_run = seed_type(
        owner_pool,
        org,
        actor,
        PAY_RUN,
        "ids",
        vec![
            // `multi_choice`, never `json`: `check_field_shape` is `is_array()`
            // for the former and `true` for everything for the latter, so this is
            // the only declaration that refuses a non-array before the resolver
            // sees it. `reference` is impossible — it requires `is_string()`.
            prop("ids", "multi_choice", true, json!({})),
            derived_total("ids", "base_salary", EMPLOYMENT, "sum"),
        ],
    )
    .await;
    (employment, pay_run)
}

fn hire(type_id: ObjectTypeId, name: &str, salary: i64, at: OffsetDateTime) -> CreateInstance {
    CreateInstance {
        object_type_id: type_id,
        title: name.to_owned(),
        attributes: json!({"name": name, "base_salary": salary}),
        valid_from: Some(at),
        action_type_id: None,
        reason: Some("hire".to_owned()),
    }
}

/// The pay-run bag, shaped exactly as the conformance fixture shapes it: the ids
/// and nothing else. `sent_total` models a CLIENT that sends the derived value
/// anyway — `params_schema` lists it, so nothing upstream stops them.
fn cycle(
    type_id: ObjectTypeId,
    members: &[InstanceId],
    at: OffsetDateTime,
    sent_total: Option<i64>,
) -> CreateInstance {
    let mut attributes = json!({
        "ids": members.iter().map(ToString::to_string).collect::<Vec<_>>(),
    });
    if let Some(total) = sent_total {
        attributes["total"] = json!(total);
    }
    CreateInstance {
        object_type_id: type_id,
        title: "2026-07".to_owned(),
        attributes,
        valid_from: Some(at),
        action_type_id: None,
        reason: Some("cycle".to_owned()),
    }
}

/// Read a stored attribute back as `console_rt`, after commit, at `at`.
async fn stored(rt: &PgPool, org: OrgId, id: InstanceId, key: &str, at: OffsetDateTime) -> Value {
    console_platform_request_context::scope_org(org, async {
        PgInstanceStore::new(rt.clone())
            .get_as_of(id, at)
            .await
            .expect("read back as the runtime role")
            .revision
            .attributes
            .get(key)
            .cloned()
            .unwrap_or(Value::Null)
    })
    .await
}

async fn total(rt: &PgPool, org: OrgId, id: InstanceId, at: OffsetDateTime) -> i64 {
    let value = stored(rt, org, id, "total", at).await;
    value.as_i64().unwrap_or_else(|| {
        panic!("gross_total must be a JSON integer (CC-10 reads it with as_i64), got {value:?}")
    })
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn derives_the_declared_sum_and_seals_it_into_the_chain(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid()).await;
    let (employment, pay_run) = seed_pair(&owner_pool, org, actor).await;

    console_platform_request_context::scope_org(org, async {
        let store = PgInstanceStore::new(rt.clone());
        let trace = TraceContext::generate;

        let e1 = store
            .create_instance(actor, hire(employment, "김정비", 250, T0), trace(), T0)
            .await
            .expect("hire e1")
            .instance
            .id;
        let e2 = store
            .create_instance(actor, hire(employment, "박정비", 150, T0), trace(), T0)
            .await
            .expect("hire e2")
            .instance
            .id;

        // (a) a type declaring no `derive` is untouched: exactly the keys sent,
        // no key invented.
        let bag = store
            .get_as_of(e1, T0)
            .await
            .expect("read e1")
            .revision
            .attributes;
        assert_eq!(
            bag,
            json!({"name": "김정비", "base_salary": 250}),
            "a type with no derived property must store exactly what was sent"
        );

        // (b) REAL ARITHMETIC over the declared population. Two populations, two
        // different correct answers — no constant satisfies both.
        let everyone = store
            .create_instance(actor, cycle(pay_run, &[e1, e2], T1, None), trace(), T1)
            .await
            .expect("run over everyone")
            .instance
            .id;
        let just_one = store
            .create_instance(actor, cycle(pay_run, &[e1], T1, None), trace(), T1)
            .await
            .expect("run over one")
            .instance
            .id;
        assert_eq!(total(&rt, org, everyone, T1).await, 400);
        assert_eq!(total(&rt, org, just_one, T1).await, 250);
        assert_ne!(
            total(&rt, org, everyone, T1).await,
            total(&rt, org, just_one, T1).await,
            "different populations must produce different totals, or a constant would pass"
        );

        // (c) an empty population derives 0 — a pay run over nobody grosses zero.
        let nobody = store
            .create_instance(actor, cycle(pay_run, &[], T1, None), trace(), T1)
            .await
            .expect("run over nobody")
            .instance
            .id;
        assert_eq!(total(&rt, org, nobody, T1).await, 0);

        // (d) a caller-sent value is OVERWRITTEN. `params_schema` lists the
        // derived property, so a client can send one; filling only an absent key
        // would also never fire at all, because the auto-attached `create` action
        // inserts an explicit null for every declared property.
        let spoofed = store
            .create_instance(
                actor,
                cycle(pay_run, &[e1, e2], T1, Some(999_999)),
                trace(),
                T1,
            )
            .await
            .expect("run with a spoofed total")
            .instance
            .id;
        assert_eq!(
            total(&rt, org, spoofed, T1).await,
            400,
            "a caller-sent gross_total must be overwritten by the derived value"
        );

        // (e) the SECOND revision writer derives too. A resolver wired only into
        // `create_instance_in_tx` keeps the spoofed value on every edit, and CC-10
        // (which only creates) would never notice.
        store
            .stage_revision(
                actor,
                spoofed,
                StageRevision {
                    attributes: json!({
                        "ids": [e2.to_string()],
                        "total": 999_999,
                    }),
                    valid_from: Some(T2),
                    action_type_id: None,
                    reason: Some("re-run".to_owned()),
                },
                trace(),
                T2,
            )
            .await
            .expect("stage a re-run");
        assert_eq!(
            total(&rt, org, spoofed, T2).await,
            150,
            "stage_revision must derive as well as create"
        );
        assert_eq!(
            total(&rt, org, spoofed, T1).await,
            400,
            "and must not rewrite the superseded revision"
        );

        // (f) the derived value is inside the FIXITY hash. `verify_chain`
        // recomputes every row_hash from the STORED attributes; a value written
        // after `canonical_revision` would make this report the revision as the
        // first break, indistinguishable from tamper.
        //
        // The property keys here are `ids` and `total` for a reason that is NOT
        // about derivation and IS a live defect: `canonical_revision`
        // (`src/instances.rs`) claims "no `preserve_order` feature in the
        // workspace → BTreeMap", and that premise is false — `cargo tree -p
        // serde_json -e features` shows `indexmap`, so `serde_json::Map` is
        // INSERTION-ordered. Postgres returns jsonb object keys in (length,
        // bytes) order, so any bag whose write order differs from that order
        // re-serializes differently on read-back and `verify_chain` reports a
        // break with nothing tampered. `ids`(3) then `total`(5) coincide under
        // both orders, which is what makes this assertion a test of the
        // derivation rather than of that bug. `pay_run`'s real keys
        // (`employment_ids`, `gross_total`, `period_end`, `period_start`) do NOT
        // coincide — see the escalation.
        let history = store.history(spoofed).await.expect("history");
        assert_eq!(history.len(), 2, "create + stage");
        assert_eq!(
            verify_chain(&history),
            None,
            "the derived value must be hashed, not decoration: {history:#?}"
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn derivation_reads_the_referent_effective_at_the_revision_instant(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid()).await;
    let (employment, pay_run) = seed_pair(&owner_pool, org, actor).await;

    console_platform_request_context::scope_org(org, async {
        let store = PgInstanceStore::new(rt.clone());
        let trace = TraceContext::generate;

        // (g) THE ASSERTION CC-10 CANNOT MAKE. CC-06 re-sends the identical hire
        // bag when it transfers, so v1 and v2 carry the SAME base_salary and a
        // head read is indistinguishable from an as-of read. A raise is the only
        // shape that separates them.
        let e1 = store
            .create_instance(actor, hire(employment, "김정비", 250, T0), trace(), T0)
            .await
            .expect("hire e1")
            .instance
            .id;
        store
            .stage_revision(
                actor,
                e1,
                StageRevision {
                    attributes: json!({"name": "김정비", "base_salary": 900}),
                    valid_from: Some(T2),
                    action_type_id: None,
                    reason: Some("raise".to_owned()),
                },
                trace(),
                T2,
            )
            .await
            .expect("raise e1 at T2");

        let before = store
            .create_instance(actor, cycle(pay_run, &[e1], T1, None), trace(), T3)
            .await
            .expect("pay run effective before the raise")
            .instance
            .id;
        let after = store
            .create_instance(actor, cycle(pay_run, &[e1], T3, None), trace(), T3)
            .await
            .expect("pay run effective after the raise")
            .instance
            .id;

        assert_eq!(
            total(&rt, org, before, T1).await,
            250,
            "a pay run effective BEFORE the raise must total the salary in force then; 900 means \
             the resolver read the current head and the number is a function of when the row was \
             written, not of the period it covers"
        );
        assert_eq!(
            total(&rt, org, after, T3).await,
            900,
            "and a pay run effective after it must total the new salary"
        );

        // The half-open interval `[valid_from, valid_to)` at the exact boundary:
        // v1 closes at T2 and v2 opens at T2, so T2 selects v2 — exactly one row,
        // which is what makes the single set read unambiguous.
        let at_boundary = store
            .create_instance(actor, cycle(pay_run, &[e1], T2, None), trace(), T3)
            .await
            .expect("pay run effective at the boundary")
            .instance
            .id;
        assert_eq!(
            total(&rt, org, at_boundary, T2).await,
            900,
            "at exactly valid_from the NEW revision is in force"
        );

        // Both writers read as-of, not only create: the wall clock is T3 for all
        // three runs above, so an `occurred_at` read would have returned 900 every
        // time.
        store
            .stage_revision(
                actor,
                before,
                StageRevision {
                    attributes: json!({"ids": [e1.to_string()]}),
                    valid_from: Some(T1 + time::Duration::hours(1)),
                    action_type_id: None,
                    reason: Some("correct the run".to_owned()),
                },
                trace(),
                T3,
            )
            .await
            .expect("re-stage the early run");
        assert_eq!(
            total(&rt, org, before, T1 + time::Duration::hours(2)).await,
            250,
            "a corrected run still dated before the raise must still total the old salary"
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn bad_declarations_and_referents_are_refused_in_rust(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid()).await;
    let (employment, pay_run) = seed_pair(&owner_pool, org, actor).await;

    // Three MIS-DECLARED types. Each must be refused by name at writeback rather
    // than silently computing something: a `_ => sum` default arm or a skipped
    // property surfaces as "the mechanism is broken", not "the declaration has a
    // typo".
    let bad_op = seed_type(
        &owner_pool,
        org,
        actor,
        "deriv_bad_op",
        "ids",
        vec![
            prop("ids", "multi_choice", true, json!({})),
            derived_total("ids", "base_salary", EMPLOYMENT, "product"),
        ],
    )
    .await;
    let bad_shape = seed_type(
        &owner_pool,
        org,
        actor,
        "deriv_bad_shape",
        "ids",
        vec![
            prop("ids", "multi_choice", true, json!({})),
            prop(
                "total",
                "integer",
                false,
                json!({"derive": {"op": "sum", "over": "ids"}}),
            ),
        ],
    )
    .await;
    let bad_of = seed_type(
        &owner_pool,
        org,
        actor,
        "deriv_bad_of",
        "ids",
        vec![
            prop("ids", "multi_choice", true, json!({})),
            // `name` is text on the referent: a term that is not an integer.
            derived_total("ids", "name", EMPLOYMENT, "sum"),
        ],
    )
    .await;
    // `over` naming a property this type does not declare. It is the ONE
    // declaration field a bag read cannot police on its own: an absent key is
    // byte-identical to a declared-but-empty one, so without a `props` check the
    // resolver derives 0 and seals it. `op`, `of` and `to_type` typos all fail
    // closed by construction; this one failed OPEN.
    let bad_over = seed_type(
        &owner_pool,
        org,
        actor,
        "deriv_bad_over",
        "ids",
        vec![
            prop("ids", "multi_choice", true, json!({})),
            derived_total("idz", "base_salary", EMPLOYMENT, "sum"),
        ],
    )
    .await;

    console_platform_request_context::scope_org(org, async {
        let store = PgInstanceStore::new(rt.clone());
        let trace = TraceContext::generate;

        let e1 = store
            .create_instance(actor, hire(employment, "김정비", 250, T0), trace(), T0)
            .await
            .expect("hire e1")
            .instance
            .id;
        // Hired only at T3: it exists, but has no revision effective at T1.
        let late = store
            .create_instance(actor, hire(employment, "늦은", 100, T3), trace(), T3)
            .await
            .expect("hire late")
            .instance
            .id;

        let refused = |input: CreateInstance, at: OffsetDateTime| {
            let store = &store;
            async move {
                let err = store
                    .create_instance(actor, input, TraceContext::generate(), at)
                    .await
                    .expect_err("this create must be refused");
                format!("{err:?}")
            }
        };

        // (h) a referent that does not exist. `not_found`, never a 23503 that
        // would surface as an unmappable 500.
        let dangling = InstanceId::new();
        let rendered = refused(cycle(pay_run, &[dangling], T1, None), T1).await;
        assert!(
            rendered.contains("no revision effective"),
            "expected a not_found naming the missing referent, got {rendered}"
        );

        // A referent that EXISTS but has no revision effective at the run's
        // instant. Dropping its term instead would store a smaller, entirely
        // plausible total — the worst available failure mode.
        let rendered = refused(cycle(pay_run, &[e1, late], T1, None), T1).await;
        assert!(
            rendered.contains("no revision effective") && rendered.contains(&late.to_string()),
            "expected a not_found naming the not-yet-effective referent, got {rendered}"
        );

        // A wrong-TYPED referent: the check compares `stable_key`, never the
        // per-version object_type_id.
        let a_run = store
            .create_instance(actor, cycle(pay_run, &[e1], T1, None), trace(), T1)
            .await
            .expect("a real pay run")
            .instance
            .id;
        let rendered = refused(cycle(pay_run, &[a_run], T1, None), T1).await;
        assert!(
            rendered.contains("must reference a 'deriv_employment'"),
            "expected a validation naming both types, got {rendered}"
        );

        // A term that is not an integer: never summed as zero.
        let rendered = refused(cycle(bad_of, &[e1], T1, None), T1).await;
        assert!(
            rendered.contains("does not carry") && rendered.contains("as an integer"),
            "expected a validation naming the non-integer term, got {rendered}"
        );

        // i64 overflow: `checked_add`, never a bare `+` that panics in debug and
        // wraps in release, inside the writeback transaction.
        let rich = store
            .create_instance(actor, hire(employment, "부자", i64::MAX, T0), trace(), T0)
            .await
            .expect("hire rich")
            .instance
            .id;
        let rendered = refused(cycle(pay_run, &[rich, e1], T1, None), T1).await;
        assert!(
            rendered.contains("overflowed"),
            "expected a validation, not a panic, on overflow: {rendered}"
        );

        // An unimplemented `op`.
        let rendered = refused(cycle(bad_op, &[e1], T1, None), T1).await;
        assert!(
            rendered.contains("derivation op 'product'"),
            "expected a validation naming the unimplemented op, got {rendered}"
        );

        // A malformed declaration.
        let rendered = refused(cycle(bad_shape, &[e1], T1, None), T1).await;
        assert!(
            rendered.contains("without string"),
            "expected a validation naming the malformed declaration, got {rendered}"
        );

        // An `over` that names no declared property. RED here is not an error
        // message but a SUCCESSFUL create carrying `total: 0` — a completely
        // plausible payroll that satisfies every round-trip, shape and fixity
        // assertion in this file. One transposed letter, hash-sealed as truth.
        let rendered = refused(cycle(bad_over, &[e1], T1, None), T1).await;
        assert!(
            rendered.contains("which this object type does not declare"),
            "expected a validation naming the undeclared `over`, got {rendered}"
        );

        // A malformed id list. `multi_choice` already refuses a non-array via
        // `check_field_shape`, so the resolver's own arm is what catches a
        // non-string INSIDE the array.
        let rendered = refused(
            CreateInstance {
                object_type_id: pay_run,
                title: "2026-07".to_owned(),
                attributes: json!({"ids": [7]}),
                valid_from: Some(T1),
                action_type_id: None,
                reason: None,
            },
            T1,
        )
        .await;
        assert!(
            rendered.contains("must hold instance id strings"),
            "expected a validation on a non-string member, got {rendered}"
        );

        // (i) the refusals are TOTAL. Exactly one pay run exists — the single
        // successful create above — so no aborted attempt left a revision behind.
        let runs: i64 = with_org_conn::<_, i64, PgOntologyError>(&rt, org, |tx| {
            Box::pin(async move {
                sqlx::query_scalar(
                    "SELECT count(*) FROM ont_instances i JOIN ont_object_types t \
                     ON t.id = i.object_type_id WHERE t.stable_key <> $1",
                )
                .bind(EMPLOYMENT)
                .fetch_one(tx.as_mut())
                .await
                .map_err(Into::into)
            })
        })
        .await
        .unwrap();
        assert_eq!(
            runs, 1,
            "a refused derivation must not leave an instance behind"
        );
    })
    .await;
}
