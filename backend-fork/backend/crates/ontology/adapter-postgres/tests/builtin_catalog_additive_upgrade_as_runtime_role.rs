#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proofs for the additive built-in-catalog upgrade path (migration
//! 0204). Before it, `install_builtin_catalog` fail-closed on any tenant that
//! had already installed a catalog version, so a 28th built-in object type
//! could never reach a seeded environment.
//!
//! Everything the tenant can observe is read back as the genuine non-owner
//! `console_rt` role under FORCE RLS with `app.current_org` armed — the only
//! faithful exercise of the isolation these tables claim.
//!
//! Proves:
//!   (a) upgrading an already-seeded tenant installs ONLY the keys it lacks —
//!       every pre-existing key is byte-identical afterwards, same row id, same
//!       schema_version, same key-revision sidecar;
//!   (b) a newly installed type's logical link resolves against a type that
//!       arrived with an EARLIER catalog version, and never leaves the tenant;
//!   (c) re-applying the upgrade, and re-applying the older version after it,
//!       are both total no-ops — row, marker, and audit;
//!   (d) a tenant with no catalog at all still receives the upgraded catalog
//!       whole, and neither tenant can see the other's registry;
//!   (e) a retained key that contradicts the manifest's projection contract
//!       fails the whole install closed, with zero residue;
//!   (f) a version that adds NO key — one that only edits an existing type — is
//!       recorded as applied and changes nothing else. That is the additive
//!       path's sharpest limitation, so it is pinned by a test rather than left
//!       to a prose caveat, and it is the only case where the marker append and
//!       its audit row are the whole transaction.

use console_kernel_core::{OrgId, TraceContext, UserId};
use console_ontology_adapter_postgres::seed::{
    BUILTIN_CATALOG_VERSION, CUSTOMER_KEY, builtin_catalog_manifest,
};
use console_ontology_adapter_postgres::{
    CreateObjectTypeDraft, LinkTypeInput, PgOntologyError, PgOntologyStore, PropertyDefInput,
};
use console_ontology_domain::{BackingKind, LinkCardinality, SchemaLifecycleState};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

/// Deliberately not `deal`: this key exists only to exercise the upgrade
/// machinery, and must never be mistaken for a shipped built-in.
const PROBE_KEY: &str = "catalog_upgrade_probe";
const UPGRADE_VERSION: &str = "test-additive-upgrade.1";
const CONFLICT_VERSION: &str = "test-additive-conflict.1";
const EDIT_ONLY_VERSION: &str = "test-additive-edit-only.1";
const SEEDED_CATALOG_SIZE: i64 = 27;

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

async fn seed_org_and_user(owner_pool: &PgPool, org: Uuid, tag: &str) -> UserId {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("cat-{}", &org.simple().to_string()[..12]))
    .bind(format!("Catalog {tag}"))
    .execute(owner_pool)
    .await
    .unwrap();
    let user = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user.as_uuid())
        .bind(format!("Catalog {tag}"))
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    user
}

/// The extra object type the upgrade adds. `Projected` is used only by the
/// conflict probe, where the same key contradicts an already-installed
/// `Instance` type.
fn probe_draft(backing_kind: BackingKind) -> CreateObjectTypeDraft {
    let projected = matches!(backing_kind, BackingKind::Projected);
    CreateObjectTypeDraft {
        stable_key: PROBE_KEY.to_owned(),
        title: "카탈로그 증분 프로브".to_owned(),
        title_property_key: Some("name".to_owned()),
        backing_kind,
        backing_table: projected.then(|| "registry_customers".to_owned()),
        primary_key_property: projected.then(|| "id".to_owned()),
        properties: vec![PropertyDefInput {
            key: "name".to_owned(),
            title: "이름".to_owned(),
            field_type: "text".to_owned(),
            config: serde_json::json!({}),
            backing_column: projected.then(|| "name".to_owned()),
            required: !projected,
            in_property_policy: false,
        }],
        // Targets a type that a seeded tenant already has from the EARLIER
        // catalog version — the resolution case the pre-0204 installer never
        // had to handle.
        links: vec![LinkTypeInput {
            stable_key: "customer".to_owned(),
            title: "고객".to_owned(),
            reverse_title: Some("프로브".to_owned()),
            to_object_type_id: None,
            cardinality: LinkCardinality::OneMany,
            traversable: true,
        }],
        actions: Vec::new(),
        analytics: Vec::new(),
    }
}

/// The shipped manifest plus the probe, under a new catalog version. Mirrors
/// `builtin_catalog_manifest`'s boundary rewrite: only logical stable keys
/// cross into the database, never a physical UUID.
fn upgraded_manifest(catalog_version: &str, backing_kind: BackingKind) -> serde_json::Value {
    let mut manifest = builtin_catalog_manifest().unwrap();
    let mut snapshot = serde_json::to_value(probe_draft(backing_kind)).unwrap();
    for link in snapshot["links"].as_array_mut().unwrap() {
        let link = link.as_object_mut().unwrap();
        link.remove("to_object_type_id");
        link.insert(
            "to_stable_key".to_owned(),
            serde_json::Value::String(CUSTOMER_KEY.to_owned()),
        );
    }
    manifest["catalog_version"] = serde_json::Value::String(catalog_version.to_owned());
    manifest["object_types"]
        .as_array_mut()
        .unwrap()
        .push(snapshot);
    manifest
}

/// The shipped manifest under a new catalog version, with one existing type's
/// title edited and NO new key. A real catalog version shaped like this — "add a
/// property to `customer`" — is the case the additive installer cannot carry.
fn edit_only_manifest(catalog_version: &str) -> serde_json::Value {
    let mut manifest = builtin_catalog_manifest().unwrap();
    manifest["catalog_version"] = serde_json::Value::String(catalog_version.to_owned());
    manifest["object_types"][0]["title"] = serde_json::Value::String("편집된 제목".to_owned());
    manifest
}

fn manifest_keys(manifest: &serde_json::Value) -> Vec<String> {
    manifest["object_types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|object_type| object_type["stable_key"].as_str().unwrap().to_owned())
        .collect()
}

/// Pin a test catalog version to the exact canonical digest the installer will
/// recompute — the same digest chain a migration uses for a shipped version.
async fn allowlist(owner_pool: &PgPool, catalog_version: &str, manifest: &serde_json::Value) {
    sqlx::query(
        "INSERT INTO ont_builtin_catalog_allowlist(catalog_version, manifest_digest)
         VALUES ($1, digest(convert_to($2::jsonb::text,'UTF8'),'sha256'))",
    )
    .bind(catalog_version)
    .bind(manifest)
    .execute(owner_pool)
    .await
    .unwrap();
}

async fn install(
    store: &PgOntologyStore,
    org: OrgId,
    actor: UserId,
    catalog_version: &str,
    manifest: &serde_json::Value,
    occurred_at: OffsetDateTime,
) -> Result<(bool, i64), PgOntologyError> {
    let manifest = manifest.clone();
    let catalog_version = catalog_version.to_owned();
    console_platform_request_context::scope_org(org, async move {
        store
            .install_builtin_catalog(
                actor,
                &catalog_version,
                manifest,
                TraceContext::generate(),
                occurred_at,
            )
            .await
            .map(|result| (result.installed, result.object_type_count))
    })
    .await
}

/// Every registry row the named keys own, read as `console_rt`. Row ids and
/// timestamps are included on purpose: an upgrade that recreated or restaged a
/// retained key would change them.
const FINGERPRINT_SQL: &str = r#"
SELECT COALESCE(jsonb_agg(entry ORDER BY sort_key, sort_version), '[]'::jsonb)
FROM (
  SELECT o.stable_key AS sort_key, o.schema_version AS sort_version,
    jsonb_build_object(
      'object_type', to_jsonb(o),
      'key_revision', (SELECT to_jsonb(k) FROM ont_object_type_key_revisions k
                        WHERE k.org_id = o.org_id AND k.stable_key = o.stable_key),
      'properties', (SELECT COALESCE(jsonb_agg(to_jsonb(d) ORDER BY d.key), '[]'::jsonb)
                       FROM ont_property_defs d WHERE d.org_id = o.org_id AND d.object_type_id = o.id),
      'links', (SELECT COALESCE(jsonb_agg(to_jsonb(l) ORDER BY l.stable_key), '[]'::jsonb)
                  FROM ont_link_types l WHERE l.org_id = o.org_id AND l.object_type_id = o.id),
      'actions', (SELECT COALESCE(jsonb_agg(to_jsonb(a) ORDER BY a.stable_key), '[]'::jsonb)
                    FROM ont_action_types a WHERE a.org_id = o.org_id AND a.object_type_id = o.id),
      'analytics', (SELECT COALESCE(jsonb_agg(to_jsonb(n) ORDER BY n.key), '[]'::jsonb)
                      FROM ont_analytics n WHERE n.org_id = o.org_id AND n.object_type_id = o.id)
    ) AS entry
  FROM ont_object_types o
  WHERE o.stable_key = ANY($1)
) s
"#;

/// A pooled `console_rt` connection with this tenant's `app.current_org` armed —
/// the only context in which the FORCE RLS policies are actually exercised.
async fn armed_runtime_conn(
    rt_pool: &PgPool,
    org: Uuid,
) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let mut conn = rt_pool.acquire().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(org.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
    conn
}

async fn catalog_fingerprint(rt_pool: &PgPool, org: Uuid, keys: &[String]) -> serde_json::Value {
    let mut conn = armed_runtime_conn(rt_pool, org).await;
    sqlx::query_scalar(FINGERPRINT_SQL)
        .bind(keys.to_vec())
        .fetch_one(&mut *conn)
        .await
        .unwrap()
}

/// Marker history plus the whole audit footprint, read as owner so an
/// idempotent re-run can be proven mutation-free beyond the RLS surface.
async fn install_footprint(owner_pool: &PgPool, org: Uuid) -> serde_json::Value {
    sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
          'markers', (SELECT COALESCE(jsonb_agg(jsonb_build_object(
                          'catalog_version', i.catalog_version,
                          'manifest_digest', encode(i.manifest_digest,'hex'))
                        ORDER BY i.catalog_version), '[]'::jsonb)
                        FROM ont_builtin_catalog_installs i WHERE i.org_id=$1),
          'audits', (SELECT COALESCE(jsonb_agg(jsonb_build_object(
                          'action', e.action, 'target_type', e.target_type, 'after', e.after_snap)
                        ORDER BY e.action, e.after_snap::TEXT), '[]'::jsonb)
                        FROM audit_events e WHERE e.org_id=$1)
        )
        "#,
    )
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

/// The digest chain the additive path leans on, asserted end to end: the
/// manifest Rust builds today must hash to exactly the row a migration pinned
/// for `BUILTIN_CATALOG_VERSION`. Editing any draft without bumping the version
/// and allowlisting the new digest turns every install into
/// `ontology_builtin.manifest_not_allowlisted` at runtime; this fails first, in
/// CI, and prints the digest the new migration row needs.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn shipped_catalog_digest_matches_its_migration_allowlist_row(owner_pool: PgPool) {
    let manifest = builtin_catalog_manifest().unwrap();
    let (computed, allowlisted): (String, Option<String>) = sqlx::query_as(
        "SELECT encode(digest(convert_to($1::jsonb::text,'UTF8'),'sha256'),'hex'),
                (SELECT encode(a.manifest_digest,'hex') FROM ont_builtin_catalog_allowlist a
                  WHERE a.catalog_version = $2)",
    )
    .bind(&manifest)
    .bind(BUILTIN_CATALOG_VERSION)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        allowlisted.as_deref(),
        Some(computed.as_str()),
        "catalog {BUILTIN_CATALOG_VERSION} is not allowlisted at its own canonical digest. \
         A manifest change needs a NEW BUILTIN_CATALOG_VERSION plus a migration inserting \
         ('<new-version>', decode('{computed}','hex')) into ont_builtin_catalog_allowlist."
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn additive_upgrade_adds_only_new_keys_and_is_idempotent_as_runtime_role(owner_pool: PgPool) {
    let org_uuid = Uuid::from_u128(0x1a1a_1a1a_1a1a_1a1a_1a1a_1a1a_1a1a_1a1a);
    let org = OrgId::from_uuid(org_uuid);
    let actor = seed_org_and_user(&owner_pool, org_uuid, "seeded").await;
    let rt_pool = role_pool(&owner_pool, "console_rt").await;
    let cmd_pool = role_pool(&owner_pool, "console_ontology_cmd").await;
    let store = PgOntologyStore::new(rt_pool.clone()).with_command_pool(cmd_pool);

    let base = builtin_catalog_manifest().unwrap();
    let base_keys = manifest_keys(&base);
    assert_eq!(base_keys.len() as i64, SEEDED_CATALOG_SIZE);
    assert!(!base_keys.iter().any(|key| key == PROBE_KEY));

    let upgrade = upgraded_manifest(UPGRADE_VERSION, BackingKind::Instance);
    allowlist(&owner_pool, UPGRADE_VERSION, &upgrade).await;

    let (installed, count) = install(
        &store,
        org,
        actor,
        BUILTIN_CATALOG_VERSION,
        &base,
        datetime!(2026-07-25 09:00 UTC),
    )
    .await
    .unwrap();
    assert!(installed);
    assert_eq!(count, SEEDED_CATALOG_SIZE);
    let seeded_fingerprint = catalog_fingerprint(&rt_pool, org_uuid, &base_keys).await;
    assert_eq!(
        seeded_fingerprint.as_array().unwrap().len() as i64,
        SEEDED_CATALOG_SIZE
    );

    // Pre-0204 this call raised ontology_builtin.different_catalog_already_installed.
    let (upgraded, upgraded_count) = install(
        &store,
        org,
        actor,
        UPGRADE_VERSION,
        &upgrade,
        datetime!(2026-07-25 09:05 UTC),
    )
    .await
    .unwrap();
    assert!(upgraded);
    assert_eq!(upgraded_count, SEEDED_CATALOG_SIZE + 1);

    assert_eq!(
        catalog_fingerprint(&rt_pool, org_uuid, &base_keys).await,
        seeded_fingerprint,
        "an additive upgrade must not change one byte of an already-installed key"
    );

    let probe = console_platform_request_context::scope_org(org, async {
        store.get_object_type(PROBE_KEY, None).await.unwrap()
    })
    .await;
    assert_eq!(
        probe.object_type.lifecycle_state,
        SchemaLifecycleState::Published
    );
    assert_eq!(probe.object_type.schema_version, 1);

    // (b) the new type's link binds to the customer type installed by the
    // EARLIER catalog version, inside this tenant.
    let mut probe_conn = armed_runtime_conn(&rt_pool, org_uuid).await;
    let link_resolves: bool = sqlx::query_scalar(
        r#"
        SELECT (SELECT l.to_object_type_id
                  FROM ont_link_types l
                  JOIN ont_object_types p ON p.id = l.object_type_id
                 WHERE p.stable_key = $1 AND l.stable_key = 'customer')
             = (SELECT c.id FROM ont_object_types c
                 WHERE c.stable_key = $2 AND c.lifecycle_state = 'published')
        "#,
    )
    .bind(PROBE_KEY)
    .bind(CUSTOMER_KEY)
    .fetch_one(&mut *probe_conn)
    .await
    .unwrap();
    drop(probe_conn);
    assert!(
        link_resolves,
        "a newly installed type must bind its logical link to this tenant's existing published head"
    );

    let cross_tenant_links: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ont_link_types l
           JOIN ont_object_types target ON target.id = l.to_object_type_id
          WHERE l.org_id = $1 AND target.org_id <> $1",
    )
    .bind(org_uuid)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(cross_tenant_links, 0);

    // The upgrade audits exactly one new type plus one catalog-level record
    // naming what it installed and what it deliberately retained.
    let audit = sqlx::query_scalar::<_, serde_json::Value>(
        r#"
        SELECT jsonb_build_object(
          'builtin_install', (SELECT COUNT(*) FROM audit_events
                               WHERE org_id=$1 AND action='ontology.object_type.builtin_install'),
          'probe_install', (SELECT COUNT(*) FROM audit_events
                             WHERE org_id=$1 AND action='ontology.object_type.builtin_install'
                               AND after_snap->>'stable_key'=$2),
          'catalog_install', (SELECT COUNT(*) FROM audit_events
                               WHERE org_id=$1 AND action='ontology.builtin_catalog.install'),
          'upgrade_after', (SELECT after_snap FROM audit_events
                             WHERE org_id=$1 AND action='ontology.builtin_catalog.upgrade')
        )
        "#,
    )
    .bind(org_uuid)
    .bind(PROBE_KEY)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(audit["builtin_install"], SEEDED_CATALOG_SIZE + 1);
    assert_eq!(audit["probe_install"], 1);
    assert_eq!(audit["catalog_install"], 1);
    assert_eq!(audit["upgrade_after"]["catalog_version"], UPGRADE_VERSION);
    assert_eq!(
        audit["upgrade_after"]["installed_keys"],
        serde_json::json!([PROBE_KEY])
    );
    assert_eq!(
        audit["upgrade_after"]["retained_keys"]
            .as_array()
            .unwrap()
            .len() as i64,
        SEEDED_CATALOG_SIZE
    );

    // (c) both directions of re-application are total no-ops.
    let mut all_keys = base_keys.clone();
    all_keys.push(PROBE_KEY.to_owned());
    let settled_registry = catalog_fingerprint(&rt_pool, org_uuid, &all_keys).await;
    let settled_footprint = install_footprint(&owner_pool, org_uuid).await;

    let (replayed, replayed_count) = install(
        &store,
        org,
        actor,
        UPGRADE_VERSION,
        &upgrade,
        datetime!(2026-07-25 09:10 UTC),
    )
    .await
    .unwrap();
    assert!(!replayed);
    assert_eq!(replayed_count, SEEDED_CATALOG_SIZE + 1);

    let (older, older_count) = install(
        &store,
        org,
        actor,
        BUILTIN_CATALOG_VERSION,
        &base,
        datetime!(2026-07-25 09:15 UTC),
    )
    .await
    .unwrap();
    assert!(!older);
    assert_eq!(older_count, SEEDED_CATALOG_SIZE);

    assert_eq!(
        catalog_fingerprint(&rt_pool, org_uuid, &all_keys).await,
        settled_registry,
        "replaying either recorded catalog version must leave every registry row untouched"
    );
    assert_eq!(
        install_footprint(&owner_pool, org_uuid).await,
        settled_footprint,
        "replaying either recorded catalog version must add no marker and no audit"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn upgraded_catalog_installs_whole_on_a_fresh_tenant_and_stays_isolated_as_runtime_role(
    owner_pool: PgPool,
) {
    let fresh_uuid = Uuid::from_u128(0x2b2b_2b2b_2b2b_2b2b_2b2b_2b2b_2b2b_2b2b);
    let legacy_uuid = Uuid::from_u128(0x3c3c_3c3c_3c3c_3c3c_3c3c_3c3c_3c3c_3c3c);
    let fresh = OrgId::from_uuid(fresh_uuid);
    let legacy = OrgId::from_uuid(legacy_uuid);
    let fresh_actor = seed_org_and_user(&owner_pool, fresh_uuid, "fresh").await;
    let legacy_actor = seed_org_and_user(&owner_pool, legacy_uuid, "legacy").await;
    let rt_pool = role_pool(&owner_pool, "console_rt").await;
    let cmd_pool = role_pool(&owner_pool, "console_ontology_cmd").await;
    let store = PgOntologyStore::new(rt_pool.clone()).with_command_pool(cmd_pool);

    let base = builtin_catalog_manifest().unwrap();
    let upgrade = upgraded_manifest(UPGRADE_VERSION, BackingKind::Instance);
    allowlist(&owner_pool, UPGRADE_VERSION, &upgrade).await;

    let (installed, count) = install(
        &store,
        fresh,
        fresh_actor,
        UPGRADE_VERSION,
        &upgrade,
        datetime!(2026-07-25 10:00 UTC),
    )
    .await
    .unwrap();
    assert!(installed);
    assert_eq!(count, SEEDED_CATALOG_SIZE + 1);

    let (legacy_installed, legacy_count) = install(
        &store,
        legacy,
        legacy_actor,
        BUILTIN_CATALOG_VERSION,
        &base,
        datetime!(2026-07-25 10:05 UTC),
    )
    .await
    .unwrap();
    assert!(legacy_installed);
    assert_eq!(legacy_count, SEEDED_CATALOG_SIZE);

    let mut all_keys = manifest_keys(&base);
    all_keys.push(PROBE_KEY.to_owned());

    let fresh_view = catalog_fingerprint(&rt_pool, fresh_uuid, &all_keys).await;
    let legacy_view = catalog_fingerprint(&rt_pool, legacy_uuid, &all_keys).await;
    assert_eq!(
        fresh_view.as_array().unwrap().len() as i64,
        SEEDED_CATALOG_SIZE + 1
    );
    assert_eq!(
        legacy_view.as_array().unwrap().len() as i64,
        SEEDED_CATALOG_SIZE,
        "a tenant that never received the upgraded version must not see its key"
    );
    assert!(
        !legacy_view
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["object_type"]["stable_key"] == PROBE_KEY)
    );

    let fresh_orgs: Vec<Uuid> = fresh_view
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            entry["object_type"]["org_id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    assert!(fresh_orgs.iter().all(|seen| *seen == fresh_uuid));

    let cross_tenant_links: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ont_link_types l
           JOIN ont_object_types target ON target.id = l.to_object_type_id
          WHERE target.org_id <> l.org_id",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(cross_tenant_links, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn retained_key_contradicting_the_projection_contract_fails_closed_as_runtime_role(
    owner_pool: PgPool,
) {
    let org_uuid = Uuid::from_u128(0x4d4d_4d4d_4d4d_4d4d_4d4d_4d4d_4d4d_4d4d);
    let org = OrgId::from_uuid(org_uuid);
    let actor = seed_org_and_user(&owner_pool, org_uuid, "conflict").await;
    let rt_pool = role_pool(&owner_pool, "console_rt").await;
    let cmd_pool = role_pool(&owner_pool, "console_ontology_cmd").await;
    let store = PgOntologyStore::new(rt_pool.clone()).with_command_pool(cmd_pool);

    let base = builtin_catalog_manifest().unwrap();
    let upgrade = upgraded_manifest(UPGRADE_VERSION, BackingKind::Instance);
    let conflicting = upgraded_manifest(CONFLICT_VERSION, BackingKind::Projected);
    allowlist(&owner_pool, UPGRADE_VERSION, &upgrade).await;
    allowlist(&owner_pool, CONFLICT_VERSION, &conflicting).await;

    install(
        &store,
        org,
        actor,
        BUILTIN_CATALOG_VERSION,
        &base,
        datetime!(2026-07-25 11:00 UTC),
    )
    .await
    .unwrap();
    install(
        &store,
        org,
        actor,
        UPGRADE_VERSION,
        &upgrade,
        datetime!(2026-07-25 11:05 UTC),
    )
    .await
    .unwrap();

    let mut all_keys = manifest_keys(&base);
    all_keys.push(PROBE_KEY.to_owned());
    let settled_registry = catalog_fingerprint(&rt_pool, org_uuid, &all_keys).await;
    let settled_footprint = install_footprint(&owner_pool, org_uuid).await;

    let error = install(
        &store,
        org,
        actor,
        CONFLICT_VERSION,
        &conflicting,
        datetime!(2026-07-25 11:10 UTC),
    )
    .await
    .expect_err("a retained key may not contradict the manifest projection contract");
    assert!(matches!(&error, PgOntologyError::Db(_)));
    assert!(
        error
            .to_string()
            .contains("ontology_builtin.existing_key_projection_conflict"),
        "unexpected error: {error}"
    );

    assert_eq!(
        catalog_fingerprint(&rt_pool, org_uuid, &all_keys).await,
        settled_registry,
        "a rejected upgrade must leave no registry residue"
    );
    assert_eq!(
        install_footprint(&owner_pool, org_uuid).await,
        settled_footprint,
        "a rejected upgrade must leave no marker or audit residue"
    );
}

/// (f) The additive installer creates keys; it never revises one. A catalog
/// version whose only change is to an already-installed type therefore lands as
/// a marker and an audit row and NOTHING else — the tenant is recorded as being
/// on that version while its registry still holds the previous definitions.
///
/// This is a deliberate limit, not a defect, but it is the one that would be
/// easiest to discover in production instead of here: the install reports
/// success, so a caller has no signal that its edit was dropped. Pinning it
/// means a future lane that decides to carry edits has to change this test on
/// purpose. It is also the only transaction in which the marker append and its
/// audit row are the entire state change, which is exactly why that append is
/// audited at all.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn edit_only_catalog_version_is_recorded_and_changes_nothing_else_as_runtime_role(
    owner_pool: PgPool,
) {
    let org_uuid = Uuid::from_u128(0x5e5e_5e5e_5e5e_5e5e_5e5e_5e5e_5e5e_5e5e);
    let org = OrgId::from_uuid(org_uuid);
    let actor = seed_org_and_user(&owner_pool, org_uuid, "edit-only").await;
    let rt_pool = role_pool(&owner_pool, "console_rt").await;
    let cmd_pool = role_pool(&owner_pool, "console_ontology_cmd").await;
    let store = PgOntologyStore::new(rt_pool.clone()).with_command_pool(cmd_pool);

    let base = builtin_catalog_manifest().unwrap();
    let base_keys = manifest_keys(&base);
    let edit_only = edit_only_manifest(EDIT_ONLY_VERSION);
    assert_eq!(
        manifest_keys(&edit_only),
        base_keys,
        "the edit-only version must introduce no key"
    );
    assert_ne!(
        edit_only["object_types"][0]["title"], base["object_types"][0]["title"],
        "the edit-only version must actually differ from the installed one"
    );
    allowlist(&owner_pool, EDIT_ONLY_VERSION, &edit_only).await;

    install(
        &store,
        org,
        actor,
        BUILTIN_CATALOG_VERSION,
        &base,
        datetime!(2026-07-25 12:00 UTC),
    )
    .await
    .unwrap();
    let seeded_fingerprint = catalog_fingerprint(&rt_pool, org_uuid, &base_keys).await;

    let (installed, count) = install(
        &store,
        org,
        actor,
        EDIT_ONLY_VERSION,
        &edit_only,
        datetime!(2026-07-25 12:05 UTC),
    )
    .await
    .unwrap();
    assert!(
        installed,
        "an unrecorded version must take the upgrade path"
    );
    assert_eq!(count, SEEDED_CATALOG_SIZE);

    assert_eq!(
        catalog_fingerprint(&rt_pool, org_uuid, &base_keys).await,
        seeded_fingerprint,
        "an edit-only version must not revise a retained key - the edit is dropped, silently"
    );

    let outcome = sqlx::query_scalar::<_, serde_json::Value>(
        r#"
        SELECT jsonb_build_object(
          'markers', (SELECT jsonb_agg(i.catalog_version ORDER BY i.catalog_version)
                        FROM ont_builtin_catalog_installs i WHERE i.org_id=$1),
          'builtin_install', (SELECT COUNT(*) FROM audit_events
                               WHERE org_id=$1 AND action='ontology.object_type.builtin_install'),
          'upgrade_after', (SELECT after_snap FROM audit_events
                             WHERE org_id=$1 AND action='ontology.builtin_catalog.upgrade')
        )
        "#,
    )
    .bind(org_uuid)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        outcome["markers"],
        serde_json::json!([BUILTIN_CATALOG_VERSION, EDIT_ONLY_VERSION]),
        "the tenant is recorded as being on a version its registry does not reflect"
    );
    assert_eq!(
        outcome["builtin_install"], SEEDED_CATALOG_SIZE,
        "no key was created, so no per-type audit row was added"
    );
    assert_eq!(
        outcome["upgrade_after"]["catalog_version"],
        EDIT_ONLY_VERSION
    );
    assert_eq!(
        outcome["upgrade_after"]["installed_keys"],
        serde_json::json!([]),
        "the audit must say plainly that this upgrade installed nothing"
    );
    assert_eq!(
        outcome["upgrade_after"]["retained_keys"]
            .as_array()
            .unwrap()
            .len() as i64,
        SEEDED_CATALOG_SIZE
    );
}
