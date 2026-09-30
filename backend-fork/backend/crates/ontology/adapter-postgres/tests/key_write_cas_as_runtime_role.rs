#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_kernel_core::{ErrorKind, OrgId, TraceContext, UserId};
use console_ontology_adapter_postgres::seed::{BUILTIN_CATALOG_VERSION, builtin_catalog_manifest};
use console_ontology_adapter_postgres::{
    CreateObjectTypeDraft, ObjectTypeSummary, PgOntologyError, PgOntologyStore, PropertyDefInput,
};
use console_ontology_domain::{BackingKind, SchemaLifecycleState};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use time::macros::datetime;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x3333_3333_3333_3333_3333_3333_3333_3333);

async fn role_pool(owner_pool: &PgPool, role: &'static str) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _meta| {
            Box::pin(async move {
                match role {
                    "console_rt" => sqlx::query("SET ROLE console_rt").execute(conn).await?,
                    "console_ontology_cmd" => {
                        sqlx::query("SET ROLE console_ontology_cmd")
                            .execute(conn)
                            .await?
                    }
                    _ => unreachable!("test role must be allowlisted"),
                };
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    role_pool(owner_pool, "console_rt").await
}

async fn command_role_pool(owner_pool: &PgPool) -> PgPool {
    role_pool(owner_pool, "console_ontology_cmd").await
}

async fn seed_org_and_user(owner_pool: &PgPool, org: Uuid, tag: &str) -> UserId {
    let slug = format!("cas-{}", &org.simple().to_string()[..12]);
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(slug)
    .bind(format!("CAS {tag}"))
    .execute(owner_pool)
    .await
    .unwrap();
    let user = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user.as_uuid())
        .bind(format!("CAS {tag}"))
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    user
}

fn draft(key: &str, title: &str) -> CreateObjectTypeDraft {
    CreateObjectTypeDraft {
        stable_key: key.to_owned(),
        title: title.to_owned(),
        title_property_key: None,
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        properties: vec![PropertyDefInput {
            key: "name".to_owned(),
            title: "Name".to_owned(),
            field_type: "text".to_owned(),
            config: serde_json::json!({}),
            backing_column: None,
            required: true,
            in_property_policy: false,
        }],
        links: Vec::new(),
        actions: Vec::new(),
        analytics: Vec::new(),
    }
}

async fn create(
    store: &PgOntologyStore,
    actor: UserId,
    key: &str,
    title: &str,
) -> ObjectTypeSummary {
    store
        .create_object_type(
            actor,
            draft(key, title),
            TraceContext::generate(),
            datetime!(2026-07-19 12:00 UTC),
        )
        .await
        .unwrap()
}

async fn ontology_bootstrap_mutation_count(owner_pool: &PgPool, org: Uuid) -> i64 {
    sqlx::query_scalar(
        r#"
        SELECT
          (SELECT COUNT(*) FROM ont_object_types WHERE org_id=$1)
          + (SELECT COUNT(*) FROM ont_object_type_key_revisions WHERE org_id=$1)
          + (SELECT COUNT(*) FROM ont_property_defs WHERE org_id=$1)
          + (SELECT COUNT(*) FROM ont_link_types WHERE org_id=$1)
          + (SELECT COUNT(*) FROM ont_action_types WHERE org_id=$1)
          + (SELECT COUNT(*) FROM ont_analytics WHERE org_id=$1)
          + (SELECT COUNT(*) FROM ont_builtin_catalog_installs WHERE org_id=$1)
          + (SELECT COUNT(*) FROM audit_events WHERE org_id=$1)
        "#,
    )
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

async fn ontology_bootstrap_snapshot(owner_pool: &PgPool, org: Uuid) -> serde_json::Value {
    sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
          'object_types', (SELECT COUNT(*) FROM ont_object_types WHERE org_id=$1),
          'key_revisions', (SELECT COUNT(*) FROM ont_object_type_key_revisions WHERE org_id=$1),
          'properties', (SELECT COUNT(*) FROM ont_property_defs WHERE org_id=$1),
          'links', (SELECT COUNT(*) FROM ont_link_types WHERE org_id=$1),
          'actions', (SELECT COUNT(*) FROM ont_action_types WHERE org_id=$1),
          'analytics', (SELECT COUNT(*) FROM ont_analytics WHERE org_id=$1),
          'markers', (SELECT COUNT(*) FROM ont_builtin_catalog_installs WHERE org_id=$1),
          'audits', (SELECT COUNT(*) FROM audit_events WHERE org_id=$1)
        )
        "#,
    )
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn key_revision_is_tenant_scoped_and_advances_once_for_stage_and_publish(owner_pool: PgPool) {
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);
    let actor_a = seed_org_and_user(&owner_pool, *org_a.as_uuid(), "a").await;
    let actor_b = seed_org_and_user(&owner_pool, ORG_B, "b").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let cmd_pool = command_role_pool(&owner_pool).await;
    let key = "cas.shared_key";

    let a_v1 = console_platform_request_context::scope_org(org_a, async {
        create(
            &PgOntologyStore::new(rt_pool.clone()).with_command_pool(cmd_pool.clone()),
            actor_a,
            key,
            "A v1",
        )
        .await
    })
    .await;
    let b_v1 = console_platform_request_context::scope_org(org_b, async {
        create(
            &PgOntologyStore::new(rt_pool.clone()).with_command_pool(cmd_pool.clone()),
            actor_b,
            key,
            "B v1",
        )
        .await
    })
    .await;

    assert_eq!(a_v1.key_write_revision, 1);
    assert_eq!(b_v1.key_write_revision, 1);
    assert_ne!(a_v1.key_write_etag, b_v1.key_write_etag);
    assert!(a_v1.key_write_etag.starts_with('"'));
    assert!(a_v1.key_write_etag.ends_with('"'));
    assert!(!a_v1.key_write_etag.starts_with("W/"));

    let stale_cross_tenant = console_platform_request_context::scope_org(org_a, async {
        PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(cmd_pool.clone())
            .stage_revision(
                actor_a,
                key,
                b_v1.write_precondition(),
                draft(key, "must not write"),
                TraceContext::generate(),
                datetime!(2026-07-19 12:01 UTC),
            )
            .await
    })
    .await;
    assert!(matches!(
        stale_cross_tenant,
        Err(PgOntologyError::PreconditionFailed { ref current })
            if current.revision == 1 && current.etag == a_v1.key_write_etag
    ));

    let a_review_v1 = console_platform_request_context::scope_org(org_a, async {
        PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(cmd_pool.clone())
            .transition_lifecycle(
                actor_a,
                a_v1.id,
                a_v1.write_precondition(),
                SchemaLifecycleState::ReviewPending,
                true,
                TraceContext::generate(),
                datetime!(2026-07-19 12:02 UTC),
            )
            .await
            .unwrap()
    })
    .await;
    let approver_a = seed_org_and_user(&owner_pool, *org_a.as_uuid(), "approver-a").await;
    let approval_ref_v1 = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO gov_approval_requests (org_id, request_ref, kind, requested_by, payload_summary, target_ref) VALUES ($1, $2, 'ontology.schema.publish', $3, jsonb_build_object('key_revision', $4::bigint), $5)",
    )
    .bind(*org_a.as_uuid())
    .bind(approval_ref_v1)
    .bind(*actor_a.as_uuid())
    .bind(a_review_v1.key_write_revision)
    .bind(*a_v1.id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO gov_approvals (org_id, request_ref, kind, requested_by, approver_id, decision, target_ref) VALUES ($1, $2, 'ontology.schema.publish', $3, $4, 'approved', $5)",
    )
    .bind(*org_a.as_uuid())
    .bind(approval_ref_v1)
    .bind(*actor_a.as_uuid())
    .bind(*approver_a.as_uuid())
    .bind(*a_v1.id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    let a_published_v1 = console_platform_request_context::scope_org(org_a, async {
        PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(cmd_pool.clone())
            .transition_lifecycle(
                actor_a,
                a_v1.id,
                a_review_v1.write_precondition(),
                SchemaLifecycleState::Published,
                true,
                TraceContext::generate(),
                datetime!(2026-07-19 12:02:01 UTC),
            )
            .await
            .unwrap()
    })
    .await;
    assert_eq!(a_published_v1.key_write_revision, 3);

    let a_v2 = console_platform_request_context::scope_org(org_a, async {
        PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(cmd_pool.clone())
            .stage_revision(
                actor_a,
                key,
                a_published_v1.write_precondition(),
                draft(key, "A v2"),
                TraceContext::generate(),
                datetime!(2026-07-19 12:03 UTC),
            )
            .await
            .unwrap()
    })
    .await;
    assert_eq!(a_v2.schema_version, 2);
    assert_eq!(a_v2.key_write_revision, 4);

    let a_review_v2 = console_platform_request_context::scope_org(org_a, async {
        PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(cmd_pool.clone())
            .transition_lifecycle(
                actor_a,
                a_v2.id,
                a_v2.write_precondition(),
                SchemaLifecycleState::ReviewPending,
                true,
                TraceContext::generate(),
                datetime!(2026-07-19 12:04 UTC),
            )
            .await
            .unwrap()
    })
    .await;
    let approval_ref_v2 = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO gov_approval_requests (org_id, request_ref, kind, requested_by, payload_summary, target_ref) VALUES ($1, $2, 'ontology.schema.publish', $3, jsonb_build_object('key_revision', $4::bigint), $5)",
    )
    .bind(*org_a.as_uuid())
    .bind(approval_ref_v2)
    .bind(*actor_a.as_uuid())
    .bind(a_review_v2.key_write_revision)
    .bind(*a_v2.id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO gov_approvals (org_id, request_ref, kind, requested_by, approver_id, decision, target_ref) VALUES ($1, $2, 'ontology.schema.publish', $3, $4, 'approved', $5)",
    )
    .bind(*org_a.as_uuid())
    .bind(approval_ref_v2)
    .bind(*actor_a.as_uuid())
    .bind(*approver_a.as_uuid())
    .bind(*a_v2.id.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    let a_published_v2 = console_platform_request_context::scope_org(org_a, async {
        PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(cmd_pool.clone())
            .transition_lifecycle(
                actor_a,
                a_v2.id,
                a_review_v2.write_precondition(),
                SchemaLifecycleState::Published,
                true,
                TraceContext::generate(),
                datetime!(2026-07-19 12:04:01 UTC),
            )
            .await
            .unwrap()
    })
    .await;
    assert_eq!(a_published_v2.key_write_revision, 6);

    let rows = sqlx::query(
        "SELECT schema_version, lifecycle_state FROM ont_object_types WHERE org_id = $1 AND stable_key = $2 ORDER BY schema_version",
    )
    .bind(*org_a.as_uuid())
    .bind(key)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].try_get::<String, _>("lifecycle_state").unwrap(),
        "superseded"
    );
    assert_eq!(
        rows[1].try_get::<String, _>("lifecycle_state").unwrap(),
        "published"
    );
    let revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM ont_object_type_key_revisions WHERE org_id = $1 AND stable_key = $2",
    )
    .bind(*org_a.as_uuid())
    .bind(key)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        revision, 6,
        "each review and publish transition increments the key exactly once"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn same_base_stage_has_one_winner_and_one_zero_mutation_precondition_loser(
    owner_pool: PgPool,
) {
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "race").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let cmd_pool = command_role_pool(&owner_pool).await;
    let created = console_platform_request_context::scope_org(org, async {
        create(
            &PgOntologyStore::new(rt_pool.clone()).with_command_pool(cmd_pool.clone()),
            actor,
            "cas.race",
            "base",
        )
        .await
    })
    .await;
    let expected = created.write_precondition();

    let first_pool = rt_pool.clone();
    let first_cmd_pool = cmd_pool.clone();
    let first = tokio::spawn(console_platform_request_context::scope_org(
        org,
        async move {
            PgOntologyStore::new(first_pool)
                .with_command_pool(first_cmd_pool)
                .stage_revision(
                    actor,
                    "cas.race",
                    expected,
                    draft("cas.race", "first"),
                    TraceContext::generate(),
                    datetime!(2026-07-19 12:10 UTC),
                )
                .await
        },
    ));
    let second_pool = rt_pool.clone();
    let second_cmd_pool = cmd_pool.clone();
    let second = tokio::spawn(console_platform_request_context::scope_org(
        org,
        async move {
            PgOntologyStore::new(second_pool)
                .with_command_pool(second_cmd_pool)
                .stage_revision(
                    actor,
                    "cas.race",
                    expected,
                    draft("cas.race", "second"),
                    TraceContext::generate(),
                    datetime!(2026-07-19 12:10 UTC),
                )
                .await
        },
    ));
    let outcomes = [first.await.unwrap(), second.await.unwrap()];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, Err(PgOntologyError::PreconditionFailed { .. })))
            .count(),
        1
    );
    let winner = outcomes
        .iter()
        .find_map(|result| result.as_ref().ok())
        .unwrap();
    assert_eq!(winner.key_write_revision, 2);
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'ontology.object_type.stage_revision'",
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(audits, 1, "the precondition loser emits no audit");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn statement_failure_after_cas_rolls_back_revision_content_and_audit(owner_pool: PgPool) {
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "rollback").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let cmd_pool = command_role_pool(&owner_pool).await;
    let created = console_platform_request_context::scope_org(org, async {
        create(
            &PgOntologyStore::new(rt_pool.clone()).with_command_pool(cmd_pool.clone()),
            actor,
            "cas.rollback",
            "before",
        )
        .await
    })
    .await;

    sqlx::raw_sql(
        r#"
        CREATE FUNCTION test_reject_cas_content_update() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.stable_key = 'cas.rollback' AND NEW.title = 'rollback me' THEN
                RAISE EXCEPTION 'hostile post-CAS failure';
            END IF;
            RETURN NEW;
        END
        $$;
        CREATE TRIGGER test_reject_cas_content_update
        BEFORE UPDATE ON ont_object_types
        FOR EACH ROW EXECUTE FUNCTION test_reject_cas_content_update();
        "#,
    )
    .execute(&owner_pool)
    .await
    .unwrap();

    let failure = console_platform_request_context::scope_org(org, async {
        PgOntologyStore::new(rt_pool)
            .with_command_pool(cmd_pool.clone())
            .stage_revision(
                actor,
                "cas.rollback",
                created.write_precondition(),
                draft("cas.rollback", "rollback me"),
                TraceContext::generate(),
                datetime!(2026-07-19 12:20 UTC),
            )
            .await
    })
    .await;
    assert!(matches!(failure, Err(PgOntologyError::Db(_))));

    let row = sqlx::query(
        "SELECT k.revision, o.title FROM ont_object_type_key_revisions k JOIN ont_object_types o USING (org_id, stable_key) WHERE k.org_id = $1 AND k.stable_key = 'cas.rollback'",
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(row.try_get::<i64, _>("revision").unwrap(), 1);
    assert_eq!(row.try_get::<String, _>("title").unwrap(), "before");
    let stage_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'ontology.object_type.stage_revision'",
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(stage_audits, 0);
}

#[test]
fn stale_write_is_not_a_generic_conflict() {
    let error = PgOntologyError::PreconditionFailed {
        current: console_ontology_adapter_postgres::ObjectTypeWriteVersion {
            etag: "\"ont-object-type-key:00000000000000000000000000000001:r9\"".to_owned(),
            revision: 9,
        },
    };
    assert!(!matches!(
        error,
        PgOntologyError::Domain(ref kernel) if kernel.kind == ErrorKind::Conflict
    ));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn runtime_role_cannot_forge_or_delete_key_validator(owner_pool: PgPool) {
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "privileges").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let cmd_pool = command_role_pool(&owner_pool).await;
    let created = console_platform_request_context::scope_org(org, async {
        create(
            &PgOntologyStore::new(rt_pool.clone()).with_command_pool(cmd_pool.clone()),
            actor,
            "cas.privileges",
            "protected",
        )
        .await
    })
    .await;

    let forged_validator = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ffff);
    let mut forge_conn = rt_pool.acquire().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(org.as_uuid().to_string())
        .execute(&mut *forge_conn)
        .await
        .unwrap();
    let forge = sqlx::query(
        r#"
        INSERT INTO ont_object_type_key_revisions (
            org_id, stable_key, validator_id, revision, created_at, updated_at
        ) VALUES ($1, 'cas.forged', $2, 999, now(), now())
        "#,
    )
    .bind(*org.as_uuid())
    .bind(forged_validator)
    .execute(&mut *forge_conn)
    .await;
    assert!(
        forge.is_err(),
        "console_rt must not supply server-owned validator or revision columns"
    );
    drop(forge_conn);

    let before_versions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ont_object_types WHERE org_id = $1 AND stable_key = 'cas.privileges'",
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();

    let mut delete_conn = rt_pool.acquire().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(org.as_uuid().to_string())
        .execute(&mut *delete_conn)
        .await
        .unwrap();
    let delete = sqlx::query(
        "DELETE FROM ont_object_type_key_revisions WHERE org_id = $1 AND stable_key = 'cas.privileges'",
    )
    .bind(*org.as_uuid())
    .execute(&mut *delete_conn)
    .await;
    assert!(delete.is_err(), "console_rt must not delete key validators");
    drop(delete_conn);

    let preserved = sqlx::query(
        r#"
        SELECT k.validator_id, k.revision,
               (SELECT COUNT(*) FROM ont_object_types o
                WHERE o.org_id = k.org_id AND o.stable_key = k.stable_key) AS version_count
        FROM ont_object_type_key_revisions k
        WHERE k.org_id = $1 AND k.stable_key = 'cas.privileges'
        "#,
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        preserved.try_get::<Uuid, _>("validator_id").unwrap(),
        created.write_precondition().validator_id
    );
    assert_eq!(preserved.try_get::<i64, _>("revision").unwrap(), 1);
    assert_eq!(
        preserved.try_get::<i64, _>("version_count").unwrap(),
        before_versions
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn runtime_role_has_one_validated_audited_object_type_write_surface(owner_pool: PgPool) {
    let org = OrgId::knl();
    let actor = seed_org_and_user(&owner_pool, *org.as_uuid(), "sql-boundary").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let cmd_pool = command_role_pool(&owner_pool).await;

    let unavailable = console_platform_request_context::scope_org(org, async {
        PgOntologyStore::new(rt_pool.clone())
            .create_object_type(
                actor,
                draft("cas.no_command_pool", "must fail closed"),
                TraceContext::generate(),
                datetime!(2026-07-19 12:00 UTC),
            )
            .await
    })
    .await;
    assert!(matches!(
        unavailable,
        Err(PgOntologyError::CommandUnavailable)
    ));

    let mut incomplete = cmd_pool.acquire().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(org.as_uuid().to_string())
        .execute(&mut *incomplete)
        .await
        .unwrap();
    let incomplete_snapshot = serde_json::json!({
        "stable_key": "cas.incomplete",
        "title": "Incomplete",
        "title_property_key": null,
        "backing_kind": "instance",
        "backing_table": null,
        "primary_key_property": null,
        "properties": [],
        "links": [],
        "actions": []
    });
    let incomplete_error =
        sqlx::query("SELECT * FROM ontology_api.create_object_type($1,$2,$3,$4,$5)")
            .bind(*org.as_uuid())
            .bind(incomplete_snapshot)
            .bind(*actor.as_uuid())
            .bind("0123456789abcdef0123456789abcdef")
            .bind("0123456789abcdef")
            .execute(&mut *incomplete)
            .await
            .expect_err("missing required snapshot arrays must fail closed");
    assert_eq!(
        incomplete_error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("22023")
    );
    drop(incomplete);
    let incomplete_mutations: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM ont_object_types WHERE org_id=$1 AND stable_key='cas.incomplete') + (SELECT COUNT(*) FROM audit_events WHERE org_id=$1 AND action='ontology.object_type.create')",
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(incomplete_mutations, 0);

    let created = console_platform_request_context::scope_org(org, async {
        create(
            &PgOntologyStore::new(rt_pool.clone()).with_command_pool(cmd_pool.clone()),
            actor,
            "cas.sql_boundary",
            "before",
        )
        .await
    })
    .await;

    let direct_statements = [
        "UPDATE ont_object_types SET title = 'bypassed' WHERE id = $1",
        "UPDATE ont_object_type_key_revisions SET revision = revision + 1 WHERE org_id = $2",
        "INSERT INTO ont_property_defs (org_id, object_type_id, key, title, type) VALUES ($2, $1, 'bypass', 'Bypass', 'text')",
        "INSERT INTO ont_link_types (org_id, object_type_id, stable_key, title, cardinality) VALUES ($2, $1, 'bypass', 'Bypass', 'one_one')",
        "INSERT INTO ont_action_types (org_id, object_type_id, stable_key, title, dispatch) VALUES ($2, $1, 'bypass', 'Bypass', 'instance_revision')",
        "INSERT INTO ont_analytics (org_id, object_type_id, key, title) VALUES ($2, $1, 'bypass', 'Bypass')",
    ];
    for statement in direct_statements {
        let mut conn = rt_pool.acquire().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, false)")
            .bind(org.as_uuid().to_string())
            .execute(&mut *conn)
            .await
            .unwrap();
        let error = sqlx::query(statement)
            .bind(*created.id.as_uuid())
            .bind(*org.as_uuid())
            .execute(&mut *conn)
            .await
            .expect_err("console_rt direct parent, token, and child DML must be denied");
        assert_eq!(
            error
                .as_database_error()
                .and_then(|value| value.code())
                .as_deref(),
            if statement.starts_with("UPDATE") {
                Some("42501")
            } else {
                Some("23514")
            },
            "legacy-shaped privileges must still reject content edits and child appends without a same-transaction audit"
        );
    }

    for (table, retained) in [
        ("ont_object_types", &["INSERT", "UPDATE"][..]),
        ("ont_object_type_key_revisions", &[][..]),
        ("ont_property_defs", &["INSERT"][..]),
        ("ont_link_types", &["INSERT"][..]),
        ("ont_action_types", &["INSERT"][..]),
        ("ont_analytics", &["INSERT"][..]),
    ] {
        for privilege in ["INSERT", "UPDATE", "DELETE", "TRUNCATE"] {
            let granted: bool =
                sqlx::query_scalar("SELECT has_table_privilege('console_rt', $1, $2)")
                    .bind(table)
                    .bind(privilege)
                    .fetch_one(&owner_pool)
                    .await
                    .unwrap();
            assert_eq!(
                granted,
                retained.contains(&privilege),
                "console_rt {table}/{privilege} must match the narrow blue/green compatibility ACL"
            );
        }
    }

    for action in [
        "ontology.object_type.create",
        "ontology.object_type.stage_revision",
        "ontology.object_type.transition",
        "ontology.object_type.builtin_install",
    ] {
        let mut audit_forge = rt_pool.acquire().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, false)")
            .bind(org.as_uuid().to_string())
            .execute(&mut *audit_forge)
            .await
            .unwrap();
        let forge_error = sqlx::query(
            "INSERT INTO audit_events (id,actor,action,target_type,target_id,trace_id,span_id,occurred_at,org_id) VALUES (gen_random_uuid(),$1,$2,'ont_object_types',$3,'0123456789abcdef0123456789abcdef','0123456789abcdef',statement_timestamp(),$4)",
        )
        .bind(*actor.as_uuid())
        .bind(action)
        .bind(created.id.as_uuid().to_string())
        .bind(*org.as_uuid())
        .execute(&mut *audit_forge)
        .await
        .expect_err("console_rt must not forge protected ontology audit actions");
        let forge_database_error = forge_error.as_database_error().unwrap();
        assert_eq!(forge_database_error.code().as_deref(), Some("42501"));
        assert_eq!(
            forge_database_error.message(),
            "ontology_audit.command_required"
        );
    }

    let snapshot = serde_json::to_value(draft("cas.sql_boundary", "guarded")).unwrap();
    let trace = TraceContext::generate();
    let mut forbidden = rt_pool.acquire().await.unwrap();
    let runtime_execute =
        sqlx::query("SELECT * FROM ontology_api.stage_object_type($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(*org.as_uuid())
            .bind("cas.sql_boundary")
            .bind(created.write_precondition().validator_id)
            .bind(created.key_write_revision)
            .bind(&snapshot)
            .bind(*actor.as_uuid())
            .bind(trace.trace_id())
            .bind(trace.span_id())
            .execute(&mut *forbidden)
            .await
            .expect_err("general console_rt must not execute ontology commands");
    assert_eq!(
        runtime_execute
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("42501")
    );
    drop(forbidden);

    let mut guarded = cmd_pool.acquire().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(org.as_uuid().to_string())
        .execute(&mut *guarded)
        .await
        .unwrap();
    let row = sqlx::query(
        "SELECT object_type_id, key_write_revision FROM ontology_api.stage_object_type($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(*org.as_uuid())
    .bind("cas.sql_boundary")
    .bind(created.write_precondition().validator_id)
    .bind(created.key_write_revision)
    .bind(&snapshot)
    .bind(*actor.as_uuid())
    .bind(trace.trace_id())
    .bind(trace.span_id())
    .fetch_one(&mut *guarded)
    .await
    .unwrap();
    assert_eq!(row.get::<i64, _>("key_write_revision"), 2);

    let stale = sqlx::query(
        "SELECT object_type_id FROM ontology_api.stage_object_type($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(*org.as_uuid())
    .bind("cas.sql_boundary")
    .bind(created.write_precondition().validator_id)
    .bind(created.key_write_revision)
    .bind(&snapshot)
    .bind(*actor.as_uuid())
    .bind(trace.trace_id())
    .bind(trace.span_id())
    .fetch_optional(&mut *guarded)
    .await
    .unwrap();
    assert!(stale.is_none());
    drop(guarded);

    let state = sqlx::query(
        "SELECT o.title, k.revision, (SELECT COUNT(*) FROM audit_events e WHERE e.org_id=o.org_id AND e.target_id=o.id::text) AS audits FROM ont_object_types o JOIN ont_object_type_key_revisions k USING(org_id,stable_key) WHERE o.id=$1",
    )
    .bind(*created.id.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(state.get::<String, _>("title"), "guarded");
    assert_eq!(state.get::<i64, _>("revision"), 2);
    assert_eq!(
        state.get::<i64, _>("audits"),
        2,
        "create plus one successful raw stage; stale call audits nothing"
    );

    let reviewed = console_platform_request_context::scope_org(org, async {
        PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(cmd_pool.clone())
            .transition_lifecycle(
                actor,
                created.id,
                console_ontology_adapter_postgres::ObjectTypeWritePrecondition {
                    validator_id: created.write_precondition().validator_id,
                    revision: 2,
                },
                SchemaLifecycleState::ReviewPending,
                false,
                TraceContext::generate(),
                datetime!(2026-07-19 12:32 UTC),
            )
            .await
            .unwrap()
    })
    .await;
    let publish_without_approval = console_platform_request_context::scope_org(org, async {
        PgOntologyStore::new(rt_pool.clone())
            .with_command_pool(cmd_pool.clone())
            .transition_lifecycle(
                actor,
                reviewed.id,
                reviewed.write_precondition(),
                SchemaLifecycleState::Published,
                false,
                TraceContext::generate(),
                datetime!(2026-07-19 12:33 UTC),
            )
            .await
    })
    .await;
    assert!(matches!(
        publish_without_approval,
        Err(PgOntologyError::Domain(ref kernel)) if kernel.kind == ErrorKind::Forbidden
    ));
    let final_revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM ont_object_type_key_revisions WHERE org_id=$1 AND stable_key='cas.sql_boundary'",
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        final_revision, 3,
        "failed protected publish mutates and audits nothing"
    );

    let signatures: Vec<String> = sqlx::query_scalar(
        "SELECT pg_get_function_arguments(p.oid) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='ontology_api' AND p.proname IN ('stage_object_type','transition_object_type')",
    )
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert!(signatures.iter().all(
        |signature| !signature.contains("schema_version") && !signature.contains("protection")
    ));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn builtin_catalog_install_is_allowlisted_atomic_idempotent_and_race_safe(
    owner_pool: PgPool,
) {
    let install_org_uuid = Uuid::from_u128(0x5555_5555_5555_5555_5555_5555_5555_5555);
    let drift_org_uuid = Uuid::from_u128(0x6666_6666_6666_6666_6666_6666_6666_6666);
    let race_org_uuid = Uuid::from_u128(0x7777_7777_7777_7777_7777_7777_7777_7777);
    let physical_org_uuid = Uuid::from_u128(0x8888_8888_8888_8888_8888_8888_8888_8888);
    let nonempty_org_uuid = Uuid::from_u128(0x9999_9999_9999_9999_9999_9999_9999_9999);
    let install_org = OrgId::from_uuid(install_org_uuid);
    let drift_org = OrgId::from_uuid(drift_org_uuid);
    let physical_org = OrgId::from_uuid(physical_org_uuid);
    let nonempty_org = OrgId::from_uuid(nonempty_org_uuid);
    let install_actor = seed_org_and_user(&owner_pool, install_org_uuid, "catalog-install").await;
    let drift_actor = seed_org_and_user(&owner_pool, drift_org_uuid, "catalog-drift").await;
    let race_actor = seed_org_and_user(&owner_pool, race_org_uuid, "catalog-race").await;
    let physical_actor =
        seed_org_and_user(&owner_pool, physical_org_uuid, "catalog-physical-id").await;
    let nonempty_actor =
        seed_org_and_user(&owner_pool, nonempty_org_uuid, "catalog-nonempty").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let cmd_pool = command_role_pool(&owner_pool).await;
    let store = PgOntologyStore::new(rt_pool.clone()).with_command_pool(cmd_pool.clone());
    let manifest = builtin_catalog_manifest().unwrap();
    let object_types = manifest["object_types"].as_array().unwrap();
    assert_eq!(object_types.len(), 27);
    let logical_link_count = object_types
        .iter()
        .flat_map(|object_type| object_type["links"].as_array().unwrap())
        .filter(|link| {
            link.get("to_stable_key")
                .and_then(serde_json::Value::as_str)
                .is_some()
        })
        .count() as i64;
    assert!(
        object_types
            .iter()
            .flat_map(|object_type| object_type["links"].as_array().unwrap())
            .all(|link| link.get("to_object_type_id").is_none())
    );

    let mut runtime_install = rt_pool.acquire().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(install_org_uuid.to_string())
        .execute(&mut *runtime_install)
        .await
        .unwrap();
    let runtime_install_error =
        sqlx::query("SELECT * FROM ontology_api.install_builtin_catalog($1,$2,$3,$4,$5,$6)")
            .bind(install_org_uuid)
            .bind(BUILTIN_CATALOG_VERSION)
            .bind(&manifest)
            .bind(*install_actor.as_uuid())
            .bind("0123456789abcdef0123456789abcdef")
            .bind("0123456789abcdef")
            .execute(&mut *runtime_install)
            .await
            .expect_err("the general runtime credential must not execute catalog install");
    assert_eq!(
        runtime_install_error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("42501")
    );
    drop(runtime_install);

    let installed = console_platform_request_context::scope_org(install_org, async {
        store
            .install_builtin_catalog(
                install_actor,
                BUILTIN_CATALOG_VERSION,
                manifest.clone(),
                TraceContext::generate(),
                datetime!(2026-07-19 13:00 UTC),
            )
            .await
            .unwrap()
    })
    .await;
    assert!(installed.installed);
    assert_eq!(installed.object_type_count, 27);
    let installed_snapshot = ontology_bootstrap_snapshot(&owner_pool, install_org_uuid).await;

    let retry = console_platform_request_context::scope_org(install_org, async {
        store
            .install_builtin_catalog(
                install_actor,
                BUILTIN_CATALOG_VERSION,
                manifest.clone(),
                TraceContext::generate(),
                datetime!(2026-07-19 13:01 UTC),
            )
            .await
            .unwrap()
    })
    .await;
    assert!(!retry.installed);
    assert_eq!(retry.object_type_count, object_types.len() as i64);
    assert_eq!(
        ontology_bootstrap_snapshot(&owner_pool, install_org_uuid).await,
        installed_snapshot,
        "an exact retry must be row- and audit-mutation-free across the full catalog footprint"
    );

    let install_counts = sqlx::query(
        r#"
        SELECT
          (SELECT COUNT(*) FROM ont_object_types WHERE org_id=$1) AS object_types,
          (SELECT COUNT(*) FROM ont_object_types WHERE org_id=$1 AND lifecycle_state='published') AS published,
          (SELECT COUNT(*) FROM audit_events WHERE org_id=$1 AND action='ontology.object_type.builtin_install') AS audits,
          (SELECT COUNT(*) FROM ont_link_types WHERE org_id=$1 AND to_object_type_id IS NOT NULL) AS resolved_links,
          (SELECT COUNT(*) FROM ont_link_types l JOIN ont_object_types target ON target.id=l.to_object_type_id WHERE l.org_id=$1 AND target.org_id<>$1) AS cross_tenant_links
        "#,
    )
    .bind(install_org_uuid)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(install_counts.get::<i64, _>("object_types"), 27);
    assert_eq!(install_counts.get::<i64, _>("published"), 27);
    assert_eq!(
        install_counts.get::<i64, _>("audits"),
        27,
        "bootstrap emits exactly one audit per installed object type"
    );
    assert_eq!(
        install_counts.get::<i64, _>("resolved_links"),
        logical_link_count
    );
    assert_eq!(install_counts.get::<i64, _>("cross_tenant_links"), 0);

    let mut drifted_manifest = manifest.clone();
    drifted_manifest["object_types"][0]["title"] = serde_json::json!("altered");
    let drift_error = console_platform_request_context::scope_org(drift_org, async {
        store
            .install_builtin_catalog(
                drift_actor,
                BUILTIN_CATALOG_VERSION,
                drifted_manifest,
                TraceContext::generate(),
                datetime!(2026-07-19 13:02 UTC),
            )
            .await
    })
    .await;
    assert!(matches!(drift_error, Err(PgOntologyError::Db(_))));
    assert_eq!(
        ontology_bootstrap_mutation_count(&owner_pool, drift_org_uuid).await,
        0,
        "wrong digest must leave no object, child, marker, or audit residue"
    );

    let mut physical_manifest = manifest.clone();
    let physical_catalog_version = "test-physical-id";
    physical_manifest["catalog_version"] = serde_json::json!(physical_catalog_version);
    let physical_link = physical_manifest["object_types"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .flat_map(|object_type| object_type["links"].as_array_mut().unwrap())
        .next()
        .expect("built-in catalog must exercise at least one logical link");
    physical_link["to_object_type_id"] = serde_json::json!("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
    sqlx::query(
        "INSERT INTO ont_builtin_catalog_allowlist(catalog_version,manifest_digest) VALUES ($1,digest(convert_to($2::jsonb::text,'UTF8'),'sha256'))",
    )
    .bind(physical_catalog_version)
    .bind(&physical_manifest)
    .execute(&owner_pool)
    .await
    .unwrap();
    let physical_error = console_platform_request_context::scope_org(physical_org, async {
        store
            .install_builtin_catalog(
                physical_actor,
                physical_catalog_version,
                physical_manifest,
                TraceContext::generate(),
                datetime!(2026-07-19 13:02:30 UTC),
            )
            .await
    })
    .await;
    let physical_error = physical_error.expect_err("physical link IDs must be rejected");
    assert!(matches!(&physical_error, PgOntologyError::Db(_)));
    assert!(
        physical_error
            .to_string()
            .contains("ontology_builtin.physical_link_id_forbidden")
    );
    assert_eq!(
        ontology_bootstrap_mutation_count(&owner_pool, physical_org_uuid).await,
        0,
        "physical link IDs must leave no object, child, marker, or audit residue"
    );

    console_platform_request_context::scope_org(nonempty_org, async {
        create(&store, nonempty_actor, "cas.preexisting", "Preexisting").await
    })
    .await;
    let nonempty_baseline = ontology_bootstrap_mutation_count(&owner_pool, nonempty_org_uuid).await;
    let nonempty_error = console_platform_request_context::scope_org(nonempty_org, async {
        store
            .install_builtin_catalog(
                nonempty_actor,
                BUILTIN_CATALOG_VERSION,
                manifest.clone(),
                TraceContext::generate(),
                datetime!(2026-07-19 13:02:45 UTC),
            )
            .await
    })
    .await;
    assert!(matches!(nonempty_error, Err(PgOntologyError::Db(_))));
    assert_eq!(
        ontology_bootstrap_mutation_count(&owner_pool, nonempty_org_uuid).await,
        nonempty_baseline,
        "non-empty-org rejection must add no catalog object, child, marker, or audit"
    );

    // Hold the shared bootstrap advisory lock in an uncommitted ordinary create.
    // Without the common lock, the installer would observe an empty org and
    // commit its marker/catalog beside this invisible custom row.
    let mut create_tx = cmd_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(race_org_uuid.to_string())
        .execute(&mut *create_tx)
        .await
        .unwrap();
    sqlx::query("SELECT * FROM ontology_api.create_object_type($1,$2,$3,$4,$5)")
        .bind(race_org_uuid)
        .bind(serde_json::to_value(draft("cas.raced_custom", "Raced custom")).unwrap())
        .bind(*race_actor.as_uuid())
        .bind("0123456789abcdef0123456789abcdef")
        .bind("0123456789abcdef")
        .execute(&mut *create_tx)
        .await
        .unwrap();
    let mut install_conn = cmd_pool.acquire().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(race_org_uuid.to_string())
        .execute(&mut *install_conn)
        .await
        .unwrap();
    let install_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *install_conn)
        .await
        .unwrap();
    let raced_manifest = manifest.clone();
    let install_task = tokio::spawn(async move {
        sqlx::query("SELECT * FROM ontology_api.install_builtin_catalog($1,$2,$3,$4,$5,$6)")
            .bind(race_org_uuid)
            .bind(BUILTIN_CATALOG_VERSION)
            .bind(raced_manifest)
            .bind(*race_actor.as_uuid())
            .bind("3123456789abcdef0123456789abcdef")
            .bind("3123456789abcdef")
            .execute(&mut *install_conn)
            .await
    });
    let mut observed_advisory_wait = false;
    for _ in 0..100 {
        observed_advisory_wait = sqlx::query_scalar(
            "SELECT COALESCE((SELECT wait_event_type='Lock' AND wait_event='advisory' FROM pg_stat_activity WHERE pid=$1),false)",
        )
        .bind(install_pid)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        if observed_advisory_wait {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        observed_advisory_wait,
        "installer must be observably blocked on the ordinary create's org advisory lock"
    );
    create_tx.commit().await.unwrap();
    let install_error = install_task
        .await
        .unwrap()
        .expect_err("installer must recheck and reject the now non-empty org");
    assert!(
        install_error
            .as_database_error()
            .is_some_and(|error| error.message() == "ontology_builtin.empty_org_required")
    );
    let race_counts = sqlx::query(
        r#"
        SELECT
          (SELECT COUNT(*) FROM ont_object_types WHERE org_id=$1) AS object_types,
          (SELECT COUNT(*) FROM ont_builtin_catalog_installs WHERE org_id=$1) AS markers,
          (SELECT COUNT(*) FROM audit_events WHERE org_id=$1 AND action='ontology.object_type.create') AS create_audits,
          (SELECT COUNT(*) FROM audit_events WHERE org_id=$1 AND action='ontology.object_type.builtin_install') AS install_audits
        "#,
    )
    .bind(race_org_uuid)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(race_counts.get::<i64, _>("object_types"), 1);
    assert_eq!(race_counts.get::<i64, _>("markers"), 0);
    assert_eq!(race_counts.get::<i64, _>("create_audits"), 1);
    assert_eq!(race_counts.get::<i64, _>("install_audits"), 0);
}
