#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! HTTP composition proof for object-type key CAS.
//!
//! Every request traverses the real ontology router, signed-JWT request context,
//! and a genuine `console_rt` pool so RLS and the transactional audit path are part
//! of the proof rather than mocked away.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use console_governance_adapter_postgres::PgGovernanceStore;
use console_kernel_core::{OrgId, TraceContext, UserId};
use console_ontology_adapter_postgres::PgOntologyStore;
use console_ontology_adapter_postgres::instances::{CreateInstance, PgInstanceStore};
use console_ontology_rest::{OntologyRestState, router};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_request_context::scope_org;
use console_platform_test_support::{runtime_role_pool, seed_org_and_super_admin};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const KEY: &str = "cas.http.proof";
const POLICY_KEY: &str = "policycase";
const ORG_B: Uuid = Uuid::from_u128(0x5555_5555_5555_5555_5555_5555_5555_5555);
const MIGRATION_0169: &str =
    include_str!("../../../platform/db/migrations/0169_add_normalized_catalog_policy_blocks.sql");
const MIGRATION_0170: &str = include_str!(
    "../../../platform/db/migrations/0170_harden_object_policy_attachment_and_blockers.sql"
);
const MIGRATION_0171: &str = include_str!(
    "../../../platform/db/migrations/0171_validate_catalog_normalization_blocker_fk.sql"
);

struct TestAuth {
    token: String,
    verifier: JwtVerifier,
}

struct HttpResponse {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
}

#[derive(Debug, PartialEq, Eq)]
struct StoredState {
    title: String,
    object_type_rows: i64,
    key_revision: i64,
    stage_audits: i64,
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

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn object_type_cas_is_enforced_by_the_real_router(owner_pool: PgPool) {
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "ontology-cas-http").await;
    let auth = test_auth(actor, org);
    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let service = router(OntologyRestState::new(
        PgOntologyStore::new(runtime_pool.clone())
            .with_command_pool(command_role_pool(&owner_pool).await),
        PgInstanceStore::new(runtime_pool.clone()),
        PgGovernanceStore::new(runtime_pool),
        Some(auth.verifier),
    ));

    let created = request_json(
        service.clone(),
        "POST",
        "/api/v1/ontology/object-types",
        &auth.token,
        None,
        draft("Initial title"),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);

    let get = request_json(
        service.clone(),
        "GET",
        &format!("/api/v1/ontology/object-types/{KEY}"),
        &auth.token,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(get.status, StatusCode::OK, "{:?}", get.body);
    let first_etag = get
        .headers
        .get(header::ETAG)
        .expect("GET must return ETag")
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        first_etag.starts_with("\"ont-object-type-key:") && first_etag.ends_with(":r1\""),
        "GET returned a non-strong or unexpected validator: {first_etag}"
    );

    let winner = request_json(
        service.clone(),
        "PUT",
        &format!("/api/v1/ontology/object-types/{KEY}"),
        &auth.token,
        Some(&[first_etag.as_str()]),
        draft("Winning title"),
    )
    .await;
    assert_eq!(winner.status, StatusCode::CREATED, "{:?}", winner.body);
    let current_etag = winner
        .headers
        .get(header::ETAG)
        .expect("successful PUT must return the advanced ETag")
        .to_str()
        .unwrap()
        .to_owned();
    assert_ne!(current_etag, first_etag);
    assert!(current_etag.ends_with(":r2\""), "{current_etag}");

    let after_winner = stored_state(&owner_pool, org).await;
    assert_eq!(after_winner.title, "Winning title");
    assert_eq!(after_winner.object_type_rows, 1);
    assert_eq!(after_winner.key_revision, 2);
    assert_eq!(after_winner.stage_audits, 1);

    let loser = request_json(
        service.clone(),
        "PUT",
        &format!("/api/v1/ontology/object-types/{KEY}"),
        &auth.token,
        Some(&[first_etag.as_str()]),
        draft("Losing stale title"),
    )
    .await;
    assert_eq!(loser.status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        loser.headers.get(header::ETAG).unwrap().to_str().unwrap(),
        current_etag
    );
    assert_eq!(
        loser.headers.get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    assert_eq!(
        loser.body["error"]["current_key_write_revision"].as_i64(),
        Some(2)
    );
    assert_eq!(
        stored_state(&owner_pool, org).await,
        after_winner,
        "the stale loser must change no content, revision, row, or audit count"
    );

    let missing = request_json(
        service.clone(),
        "PUT",
        &format!("/api/v1/ontology/object-types/{KEY}"),
        &auth.token,
        None,
        draft("Missing precondition"),
    )
    .await;
    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED);

    for malformed in [
        vec![format!("W/{current_etag}")],
        vec![format!("{current_etag}, {current_etag}")],
        vec![current_etag.clone(), current_etag.clone()],
        vec!["not-an-etag".to_owned()],
    ] {
        let values: Vec<&str> = malformed.iter().map(String::as_str).collect();
        let rejected = request_json(
            service.clone(),
            "PUT",
            &format!("/api/v1/ontology/object-types/{KEY}"),
            &auth.token,
            Some(&values),
            draft("Malformed precondition"),
        )
        .await;
        assert_eq!(
            rejected.status,
            StatusCode::BAD_REQUEST,
            "validator {malformed:?} was not rejected: {:?}",
            rejected.body
        );
    }
    assert_eq!(stored_state(&owner_pool, org).await, after_winner);
}

/// The list endpoint must compose the caller's request context, the RLS tenant
/// floor, and the enforced object-policy residual. This test deliberately uses
/// the HTTP router rather than invoking the adapter primitive directly.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn instance_list_composes_enforced_permit_forbid_and_tenant_scope(owner_pool: PgPool) {
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "ontology-list-http").await;
    let auth = test_auth(actor, org);
    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let command_pool = command_role_pool(&owner_pool).await;
    let service = router(OntologyRestState::new(
        PgOntologyStore::new(runtime_pool.clone()).with_command_pool(command_pool),
        PgInstanceStore::new(runtime_pool.clone()),
        PgGovernanceStore::new(runtime_pool.clone()),
        Some(auth.verifier),
    ));

    let created = request_json(
        service.clone(),
        "POST",
        "/api/v1/ontology/object-types",
        &auth.token,
        None,
        policy_draft(POLICY_KEY),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let type_id = created_object_type_id(&created.body);

    let visible_id = seed_instance(
        &runtime_pool,
        org,
        actor,
        type_id,
        "visible-to-owner",
        json!({ "owner": actor.to_string(), "flagged": false }),
    )
    .await;
    let _forbidden_id = seed_instance(
        &runtime_pool,
        org,
        actor,
        type_id,
        "hidden-by-forbid",
        json!({ "owner": actor.to_string(), "flagged": true }),
    )
    .await;
    let _other_owner_id = seed_instance(
        &runtime_pool,
        org,
        actor,
        type_id,
        "hidden-other-owner",
        json!({ "owner": "another-user", "flagged": false }),
    )
    .await;

    attach_enforced_policy(
        &owner_pool,
        org,
        type_id,
        "policy.owner_permit",
        "permit",
        owner_permit_blocks(POLICY_KEY),
    )
    .await;
    attach_enforced_policy(
        &owner_pool,
        org,
        type_id,
        "policy.flagged_forbid",
        "forbid",
        flagged_forbid_blocks(POLICY_KEY),
    )
    .await;

    let permitted = request_json(
        service.clone(),
        "GET",
        &format!("/api/v1/ontology/instances?type={type_id}"),
        &auth.token,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(permitted.status, StatusCode::OK, "{:?}", permitted.body);
    assert_instance_titles(&permitted.body, &["visible-to-owner"]);
    assert_exact_instance_ids(&permitted.body, &[visible_id]);

    // A second org's type and instance are created through the real runtime role.
    // Supplying its UUID through an Org-A JWT must yield a not-found response,
    // rather than a list response that can disclose rows or counts.
    let org_b = OrgId::from_uuid(ORG_B);
    let actor_b = seed_org_and_super_admin(&owner_pool, ORG_B, "ontology-list-http-b").await;
    let type_b = seed_object_type(&owner_pool, org_b, actor_b, "otherpolicycase").await;
    let _cross_tenant_id = seed_instance(
        &runtime_pool,
        org_b,
        actor_b,
        type_b,
        "cross-tenant-secret",
        json!({ "owner": actor_b.to_string(), "flagged": false }),
    )
    .await;
    let cross_tenant = request_json(
        service.clone(),
        "GET",
        &format!("/api/v1/ontology/instances?type={type_b}"),
        &auth.token,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(cross_tenant.status, StatusCode::NOT_FOUND);
    assert!(!body_text(&cross_tenant.body).contains("cross-tenant-secret"));

    // An unconditional forbid must win over the matching permit and return an
    // empty array (never a count-bearing response).
    attach_enforced_policy(
        &owner_pool,
        org,
        type_id,
        "policy.global_forbid",
        "forbid",
        json!({
            "effect": "forbid",
            "action": "view",
            "resource_type": POLICY_KEY,
            "conditions": []
        }),
    )
    .await;
    let denied = request_json(
        service,
        "GET",
        &format!("/api/v1/ontology/instances?type={type_id}"),
        &auth.token,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(denied.status, StatusCode::OK, "{:?}", denied.body);
    assert_eq!(denied.body, json!([]));
    assert!(!body_text(&denied.body).contains("visible-to-owner"));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn instance_list_fails_closed_for_unsupported_or_malformed_enforced_policy(
    owner_pool: PgPool,
) {
    let org = OrgId::knl();
    let actor =
        seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "ontology-list-invalid").await;
    let auth = test_auth(actor, org);
    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let service = router(OntologyRestState::new(
        PgOntologyStore::new(runtime_pool.clone())
            .with_command_pool(command_role_pool(&owner_pool).await),
        PgInstanceStore::new(runtime_pool.clone()),
        PgGovernanceStore::new(runtime_pool.clone()),
        Some(auth.verifier),
    ));
    let created = request_json(
        service.clone(),
        "POST",
        "/api/v1/ontology/object-types",
        &auth.token,
        None,
        policy_draft(POLICY_KEY),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let type_id = created_object_type_id(&created.body);
    let _protected_id = seed_instance(
        &runtime_pool,
        org,
        actor,
        type_id,
        "must-not-leak",
        json!({ "owner": actor.to_string(), "flagged": false }),
    )
    .await;
    attach_enforced_policy(
        &owner_pool,
        org,
        type_id,
        "policy.base_permit",
        "permit",
        owner_permit_blocks(POLICY_KEY),
    )
    .await;
    attach_enforced_policy(
        &owner_pool,
        org,
        type_id,
        "policy.unsupported_contains",
        "forbid",
        json!({
            "effect": "forbid",
            "action": "view",
            "resource_type": POLICY_KEY,
            "conditions": [{
                "attr": "roles",
                "op": "contains",
                "value": { "kind": "literal", "value": "SUPER_ADMIN" }
            }]
        }),
    )
    .await;
    let unsupported = request_json(
        service.clone(),
        "GET",
        &format!("/api/v1/ontology/instances?type={type_id}"),
        &auth.token,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(unsupported.status, StatusCode::OK, "{:?}", unsupported.body);
    assert_eq!(unsupported.body, json!([]));

    // Corrupt persisted JSON is not silently skipped: the route returns its
    // opaque internal error before it can issue an unfiltered instance query.
    attach_enforced_policy(
        &owner_pool,
        org,
        type_id,
        "policy.malformed",
        "permit",
        json!({ "not": "a NoCodeBlocks row" }),
    )
    .await;
    let malformed = request_json(
        service.clone(),
        "GET",
        &format!("/api/v1/ontology/instances?type={type_id}"),
        &auth.token,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(malformed.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(malformed.body["error"]["code"], "internal");
    assert_eq!(
        malformed.body["error"]["message"],
        "unable to evaluate object visibility policy"
    );
    assert!(!body_text(&malformed.body).contains("must-not-leak"));

    // Every live row is revalidated, including metadata and canonical form. Each
    // independent router request has a matching base permit, so a failure here
    // proves an invalid sibling cannot be silently ignored to widen visibility.
    for (key, effect, validation_status, blocks) in [
        (
            "policycanonical",
            "permit",
            "valid",
            json!({
                "effect": "permit", "action": "view", "resource_type": "policycanonical",
                "conditions": [], "unexpected": "noncanonical"
            }),
        ),
        (
            "policystatus",
            "permit",
            "invalid",
            json!({
                "effect": "permit", "action": "view", "resource_type": "policystatus", "conditions": []
            }),
        ),
        (
            "policyaction",
            "permit",
            "valid",
            json!({
                "effect": "permit", "action": "delete_everything", "resource_type": "policyaction", "conditions": []
            }),
        ),
        (
            "policyresource",
            "permit",
            "valid",
            json!({
                "effect": "permit", "action": "view", "resource_type": "Bad-Key", "conditions": []
            }),
        ),
        (
            "policyattribute",
            "permit",
            "valid",
            json!({
                "effect": "permit", "action": "view", "resource_type": "policyattribute",
                "conditions": [{
                    "attr": "unapproved_attribute", "op": "eq",
                    "value": { "kind": "literal", "value": "x" }
                }]
            }),
        ),
    ] {
        let rejected = policy_validation_failure_response(
            service.clone(),
            &owner_pool,
            &runtime_pool,
            org,
            actor,
            &auth.token,
            key,
            effect,
            validation_status,
            blocks,
        )
        .await;
        assert_eq!(rejected.status, StatusCode::INTERNAL_SERVER_ERROR, "{key}");
        assert_eq!(rejected.body["error"]["code"], "internal", "{key}");
        assert_eq!(
            rejected.body["error"]["message"], "unable to evaluate object visibility policy",
            "{key}"
        );
        assert!(
            !body_text(&rejected.body).contains("must-not-leak"),
            "{key}"
        );
    }

    let (effect_mismatch, mismatch_instance_id) = policy_attachment_effect_mismatch_response(
        service,
        &owner_pool,
        &runtime_pool,
        org,
        actor,
        &auth.token,
    )
    .await;
    assert_eq!(effect_mismatch.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(effect_mismatch.body["error"]["code"], "internal");
    assert_eq!(
        effect_mismatch.body["error"]["message"],
        "unable to evaluate object visibility policy"
    );
    assert!(!body_text(&effect_mismatch.body).contains("effect-mismatch-secret"));
    assert!(!body_text(&effect_mismatch.body).contains(&mismatch_instance_id.to_string()));
    assert!(effect_mismatch.body.get("instance").is_none());
    assert!(effect_mismatch.body.get("instances").is_none());
}

async fn policy_attachment_effect_mismatch_response(
    service: axum::Router,
    owner_pool: &PgPool,
    runtime_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    token: &str,
) -> (HttpResponse, Uuid) {
    let key = "policyeffectmismatch";
    let created = request_json(
        service.clone(),
        "POST",
        "/api/v1/ontology/object-types",
        token,
        None,
        policy_draft(key),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let type_id = created_object_type_id(&created.body);
    let instance_id = seed_instance(
        runtime_pool,
        org,
        actor,
        type_id,
        "effect-mismatch-secret",
        json!({ "owner": actor.to_string(), "flagged": false }),
    )
    .await;

    // A database corruption/recovery fixture bypasses only the new write-time
    // trigger. The real HTTP path must still fail closed before lowering it.
    sqlx::query(
        "ALTER TABLE ont_object_policies DISABLE TRIGGER trg_ont_object_policies_effect_matches_catalog",
    )
    .execute(owner_pool)
    .await
    .expect("disable invariant trigger for corrupt-row fixture");
    attach_enforced_policy_with_attachment_effect(
        owner_pool,
        org,
        type_id,
        "policy.effect_mismatch",
        "permit",
        "forbid",
        "valid",
        owner_permit_blocks(key),
    )
    .await;
    sqlx::query(
        "ALTER TABLE ont_object_policies ENABLE TRIGGER trg_ont_object_policies_effect_matches_catalog",
    )
    .execute(owner_pool)
    .await
    .expect("restore invariant trigger after corrupt-row fixture");

    let response = request_json(
        service,
        "GET",
        &format!("/api/v1/ontology/instances?type={type_id}"),
        token,
        None,
        Value::Null,
    )
    .await;
    (response, instance_id)
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn migration_0169_stages_existing_live_rows_without_inventing_normalized_policy_evidence(
    owner_pool: PgPool,
) {
    // Rebuild the exact pre-0169 catalog shape inside the fully migrated fixture,
    // then execute the shipped migration verbatim. This protects production
    // upgrades, not merely fresh-schema behavior.
    sqlx::raw_sql(
        r#"
        DROP TABLE cedar_policy_catalog_normalization_blockers;
        ALTER TABLE cedar_policy_catalog_entries DROP COLUMN normalized_row;
        "#,
    )
    .execute(&owner_pool)
    .await
    .expect("restore pre-0169 catalog shape");

    let org = OrgId::knl();
    seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "migration-0169").await;
    let resembles_no_code_shape: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO cedar_policy_catalog_entries
            (org_id, stable_key, title, natural_language_rule, effect, status, source,
             principal, action, resource, conditions, policy_version, schema_version,
             bundle_digest, validation_status, generated_policy_text)
        VALUES ($1, 'policy.legacy_reconstructable', 'Legacy reconstructable', 'legacy row',
                'permit', 'enforced', 'no_code_draft', '{}'::jsonb,
                '{"action_key":"view"}'::jsonb,
                '{"resource_type":"work_order"}'::jsonb, '[]'::jsonb,
                1, 'legacy-v1',
                'sha256:0000000000000000000000000000000000000000000000000000000000000000',
                'valid', 'permit(principal, action, resource);')
        RETURNING id
        "#,
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    let unreconstructable: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO cedar_policy_catalog_entries
            (org_id, stable_key, title, natural_language_rule, effect, status, source,
             principal, action, resource, conditions, policy_version, schema_version,
             bundle_digest, validation_status, generated_policy_text)
        VALUES ($1, 'policy.legacy_unreconstructable', 'Legacy unreconstructable', 'legacy row',
                'permit', 'shadow', 'imported_fixture', '{}'::jsonb, '{}'::jsonb,
                '{}'::jsonb, '[]'::jsonb, 1, 'legacy-v1',
                'sha256:0000000000000000000000000000000000000000000000000000000000000000',
                'valid', 'permit(principal, action, resource);')
        RETURNING id
        "#,
    )
    .bind(*org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();

    sqlx::raw_sql(MIGRATION_0169)
        .execute(&owner_pool)
        .await
        .expect("0169 must upgrade pre-existing enforced and shadow rows safely");

    let reconstructed: Option<Value> =
        sqlx::query_scalar("SELECT normalized_row FROM cedar_policy_catalog_entries WHERE id = $1")
            .bind(resembles_no_code_shape)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert!(
        reconstructed.is_none(),
        "legacy selector JSON is not canonical-policy evidence and must not be inferred"
    );
    let retained_status: String =
        sqlx::query_scalar("SELECT status FROM cedar_policy_catalog_entries WHERE id = $1")
            .bind(resembles_no_code_shape)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(retained_status, "enforced");
    let shadow_status: String =
        sqlx::query_scalar("SELECT status FROM cedar_policy_catalog_entries WHERE id = $1")
            .bind(unreconstructable)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(shadow_status, "shadow");
    let blocker: (String, String) = sqlx::query_as(
        "SELECT prior_status, reason FROM cedar_policy_catalog_normalization_blockers WHERE catalog_entry_id = $1",
    )
    .bind(resembles_no_code_shape)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(blocker.0, "enforced");
    assert!(blocker.1.contains("missing a reviewed canonical"));
    let blocker_statuses: Vec<String> = sqlx::query_scalar(
        "SELECT prior_status FROM cedar_policy_catalog_normalization_blockers WHERE catalog_entry_id IN ($1, $2) ORDER BY prior_status",
    )
    .bind(resembles_no_code_shape)
    .bind(unreconstructable)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(blocker_statuses, vec!["enforced", "shadow"]);
    let validated: bool = sqlx::query_scalar(
        "SELECT convalidated FROM pg_constraint WHERE conname = 'cedar_policy_catalog_enforced_normalized_row'",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert!(
        !validated,
        "0169 must defer enforcement until reviewed backfill completes"
    );
}

#[allow(clippy::too_many_arguments)]
async fn policy_validation_failure_response(
    service: axum::Router,
    owner_pool: &PgPool,
    runtime_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    token: &str,
    key: &str,
    effect: &str,
    validation_status: &str,
    blocks: Value,
) -> HttpResponse {
    let created = request_json(
        service.clone(),
        "POST",
        "/api/v1/ontology/object-types",
        token,
        None,
        policy_draft(key),
    )
    .await;
    assert_eq!(
        created.status,
        StatusCode::CREATED,
        "{key}: {:?}",
        created.body
    );
    let type_id = created_object_type_id(&created.body);
    let title = format!("{key}-must-not-leak");
    let _instance_id = seed_instance(
        runtime_pool,
        org,
        actor,
        type_id,
        &title,
        json!({ "owner": actor.to_string(), "flagged": false }),
    )
    .await;
    attach_enforced_policy(
        owner_pool,
        org,
        type_id,
        &format!("{key}.base"),
        "permit",
        owner_permit_blocks(key),
    )
    .await;
    attach_enforced_policy_with_validation(
        owner_pool,
        org,
        type_id,
        &format!("{key}.invalid"),
        effect,
        validation_status,
        blocks,
    )
    .await;
    request_json(
        service,
        "GET",
        &format!("/api/v1/ontology/instances?type={type_id}"),
        token,
        None,
        Value::Null,
    )
    .await
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn blocker_queue_is_tenant_scoped_cascades_and_attachment_effects_are_write_checked(
    owner_pool: PgPool,
) {
    let org_a = OrgId::knl();
    seed_org_and_super_admin(&owner_pool, *org_a.as_uuid(), "blockers-a").await;
    let org_b = OrgId::from_uuid(ORG_B);
    seed_org_and_super_admin(&owner_pool, ORG_B, "blockers-b").await;
    let catalog_a = seed_catalog_entry(&owner_pool, org_a, "policy.blocker_a").await;
    let catalog_b = seed_catalog_entry(&owner_pool, org_b, "policy.blocker_b").await;
    sqlx::query(
        "INSERT INTO cedar_policy_catalog_normalization_blockers (org_id, catalog_entry_id, prior_status, reason) VALUES ($1, $2, 'enforced', 'fixture')",
    )
    .bind(*org_a.as_uuid())
    .bind(catalog_a)
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO cedar_policy_catalog_normalization_blockers (org_id, catalog_entry_id, prior_status, reason) VALUES ($1, $2, 'shadow', 'fixture')",
    )
    .bind(*org_b.as_uuid())
    .bind(catalog_b)
    .execute(&owner_pool)
    .await
    .unwrap();

    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let visible_to_a = blockers_visible_to(&runtime_pool, org_a).await;
    assert_eq!(visible_to_a, vec![catalog_a]);
    let visible_to_b = blockers_visible_to(&runtime_pool, org_b).await;
    assert_eq!(visible_to_b, vec![catalog_b]);

    let cross_org_blocker = sqlx::query(
        "INSERT INTO cedar_policy_catalog_normalization_blockers (org_id, catalog_entry_id, prior_status, reason) VALUES ($1, $2, 'enforced', 'cross-org fixture')",
    )
    .bind(*org_b.as_uuid())
    .bind(catalog_a)
    .execute(&owner_pool)
    .await;
    assert!(
        cross_org_blocker.is_err(),
        "a blocker cannot reference another tenant's catalog entry"
    );

    let mismatch = sqlx::query(
        "INSERT INTO ont_object_policies (org_id, object_type_id, cedar_policy_id, effect) VALUES ($1, $2, $3, 'forbid')",
    )
    .bind(*org_a.as_uuid())
    .bind(Uuid::new_v4())
    .bind(catalog_a)
    .execute(&owner_pool)
    .await;
    assert!(
        mismatch.is_err(),
        "attachment effect mismatch must be rejected"
    );

    sqlx::query("DELETE FROM cedar_policy_catalog_entries WHERE id = $1 AND org_id = $2")
        .bind(catalog_a)
        .bind(*org_a.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
    let orphan_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cedar_policy_catalog_normalization_blockers WHERE catalog_entry_id = $1")
            .bind(catalog_a)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(orphan_count, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn migration_0170_stages_blocker_fk_and_0171_validates_only_after_preflight(
    owner_pool: PgPool,
) {
    sqlx::raw_sql(
        r#"
        DROP TRIGGER trg_ont_object_policies_effect_matches_catalog ON ont_object_policies;
        DROP FUNCTION enforce_ont_object_policy_effect_matches_catalog();
        DROP TRIGGER trg_cedar_policy_catalog_normalization_blockers_org_immutable
            ON cedar_policy_catalog_normalization_blockers;
        DROP POLICY org_isolation ON cedar_policy_catalog_normalization_blockers;
        ALTER TABLE cedar_policy_catalog_normalization_blockers NO FORCE ROW LEVEL SECURITY;
        ALTER TABLE cedar_policy_catalog_normalization_blockers DISABLE ROW LEVEL SECURITY;
        REVOKE SELECT ON cedar_policy_catalog_normalization_blockers FROM console_rt;
        ALTER TABLE cedar_policy_catalog_normalization_blockers
            DROP CONSTRAINT fk_cedar_policy_catalog_normalization_blockers_catalog;
        "#,
    )
    .execute(&owner_pool)
    .await
    .expect("restore pre-0170 hardening shape");

    let org = OrgId::knl();
    seed_org_and_super_admin(&owner_pool, *org.as_uuid(), "migration-0170").await;
    let orphan_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO cedar_policy_catalog_normalization_blockers (org_id, catalog_entry_id, prior_status, reason) VALUES ($1, $2, 'enforced', 'preflight fixture')",
    )
    .bind(*org.as_uuid())
    .bind(orphan_id)
    .execute(&owner_pool)
    .await
    .unwrap();

    sqlx::raw_sql(MIGRATION_0170)
        .execute(&owner_pool)
        .await
        .expect("0170 must install an unvalidated FK without scanning legacy rows");
    let staged: bool = sqlx::query_scalar(
        "SELECT convalidated FROM pg_constraint WHERE conname = 'fk_cedar_policy_catalog_normalization_blockers_catalog'",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert!(!staged, "0170 FK must remain NOT VALID during the deploy");
    let premature_validation = sqlx::raw_sql(MIGRATION_0171).execute(&owner_pool).await;
    assert!(
        premature_validation.is_err(),
        "0171 preflight must reject orphan blockers rather than validating"
    );

    sqlx::query(
        "DELETE FROM cedar_policy_catalog_normalization_blockers WHERE catalog_entry_id = $1",
    )
    .bind(orphan_id)
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::raw_sql(MIGRATION_0171)
        .execute(&owner_pool)
        .await
        .expect("0171 must validate once preflight is clean");
    let validated: bool = sqlx::query_scalar(
        "SELECT convalidated FROM pg_constraint WHERE conname = 'fk_cedar_policy_catalog_normalization_blockers_catalog'",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert!(validated);
}

fn policy_draft(key: &str) -> Value {
    json!({
        "stable_key": key,
        "title": "Governed policy case",
        "backing_kind": "instance",
        "properties": [
            {
                "key": "owner",
                "title": "Owner",
                "field_type": "text",
                "config": {},
                "backing_column": null,
                "required": true,
                "in_property_policy": false
            },
            {
                "key": "flagged",
                "title": "Flagged",
                "field_type": "boolean",
                "config": {},
                "backing_column": null,
                "required": false,
                "in_property_policy": false
            }
        ],
        "links": [],
        "actions": [],
        "analytics": []
    })
}

fn created_object_type_id(body: &Value) -> console_ontology_domain::ObjectTypeId {
    let id = body["id"]
        .as_str()
        .expect("object-type create response must contain an id")
        .parse::<Uuid>()
        .expect("object-type id must be a UUID");
    console_ontology_domain::ObjectTypeId::from_uuid(id)
}

/// Reads the blocker queue as `console_rt` with `app.current_org` armed the way a
/// request-scoped connection arms it. The table has no Rust read path, so there
/// is no adapter to arm the GUC here: `scope_org` alone only sets a task-local,
/// and a raw pooled query would leave `current_setting('app.current_org')` empty
/// and read zero rows for every tenant — passing a "no cross-tenant rows"
/// assertion while proving nothing.
async fn blockers_visible_to(runtime_pool: &PgPool, org: OrgId) -> Vec<Uuid> {
    let mut tx = runtime_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let visible = sqlx::query_scalar(
        "SELECT catalog_entry_id FROM cedar_policy_catalog_normalization_blockers",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    visible
}

async fn seed_object_type(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    stable_key: &str,
) -> console_ontology_domain::ObjectTypeId {
    scope_org(org, async {
        PgOntologyStore::new(owner_pool.clone())
            .with_command_pool(command_role_pool(owner_pool).await)
            .create_object_type(
                actor,
                console_ontology_adapter_postgres::CreateObjectTypeDraft {
                    stable_key: stable_key.to_owned(),
                    title: "Other tenant case".to_owned(),
                    title_property_key: None,
                    backing_kind: console_ontology_domain::BackingKind::Instance,
                    backing_table: None,
                    primary_key_property: None,
                    // Mirrors `policy_draft`: the cross-tenant instance seeded into this
                    // type carries `owner`/`flagged`, and instance writes reject attributes
                    // the object type does not declare.
                    properties: vec![
                        console_ontology_adapter_postgres::PropertyDefInput {
                            key: "owner".to_owned(),
                            title: "Owner".to_owned(),
                            field_type: "text".to_owned(),
                            config: json!({}),
                            backing_column: None,
                            required: true,
                            in_property_policy: false,
                        },
                        console_ontology_adapter_postgres::PropertyDefInput {
                            key: "flagged".to_owned(),
                            title: "Flagged".to_owned(),
                            field_type: "boolean".to_owned(),
                            config: json!({}),
                            backing_column: None,
                            required: false,
                            in_property_policy: false,
                        },
                    ],
                    links: Vec::new(),
                    actions: Vec::new(),
                    analytics: Vec::new(),
                },
                TraceContext::generate(),
                OffsetDateTime::now_utc(),
            )
            .await
            .expect("seed other tenant object type")
            .id
    })
    .await
}

async fn seed_instance(
    runtime_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    type_id: console_ontology_domain::ObjectTypeId,
    title: &str,
    attributes: Value,
) -> Uuid {
    scope_org(org, async {
        PgInstanceStore::new(runtime_pool.clone())
            .create_instance(
                actor,
                CreateInstance {
                    object_type_id: type_id,
                    title: title.to_owned(),
                    attributes,
                    valid_from: None,
                    action_type_id: None,
                    reason: Some("runtime composition fixture".to_owned()),
                },
                TraceContext::generate(),
                OffsetDateTime::now_utc(),
            )
            .await
            .expect("seed instance through console_rt")
    })
    .await
    .instance
    .id
    .as_uuid()
    .to_owned()
}

async fn seed_catalog_entry(owner_pool: &PgPool, org: OrgId, stable_key: &str) -> Uuid {
    sqlx::query_scalar(
        r#"
        INSERT INTO cedar_policy_catalog_entries
            (org_id, stable_key, title, natural_language_rule, effect, status, source,
             principal, action, resource, conditions, validation_status, generated_policy_text)
        VALUES ($1, $2, $3, 'fixture', 'permit', 'draft', 'no_code_draft',
                '{}'::jsonb, '{}'::jsonb, '{}'::jsonb, '[]'::jsonb,
                'valid', 'permit(principal, action, resource);')
        RETURNING id
        "#,
    )
    .bind(*org.as_uuid())
    .bind(stable_key)
    .bind(format!("Policy {stable_key}"))
    .fetch_one(owner_pool)
    .await
    .expect("seed catalog fixture")
}

async fn attach_enforced_policy(
    owner_pool: &PgPool,
    org: OrgId,
    type_id: console_ontology_domain::ObjectTypeId,
    stable_key: &str,
    effect: &str,
    blocks: Value,
) {
    attach_enforced_policy_with_attachment_effect(
        owner_pool, org, type_id, stable_key, effect, effect, "valid", blocks,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn attach_enforced_policy_with_attachment_effect(
    owner_pool: &PgPool,
    org: OrgId,
    type_id: console_ontology_domain::ObjectTypeId,
    stable_key: &str,
    catalog_effect: &str,
    attachment_effect: &str,
    validation_status: &str,
    blocks: Value,
) {
    let policy_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO cedar_policy_catalog_entries
            (org_id, stable_key, title, natural_language_rule, effect, status, source,
             principal, action, resource, conditions, policy_version, schema_version,
             bundle_digest, validation_status, normalized_row, generated_policy_text)
        VALUES ($1, $2, $3, 'runtime composition fixture', $4, 'enforced', 'imported_fixture',
                '{}'::jsonb, '{}'::jsonb, '{}'::jsonb, '[]'::jsonb, 1,
                'ontology-runtime-filter-v1',
                'sha256:0000000000000000000000000000000000000000000000000000000000000000',
                $5, $6, 'permit(principal, action, resource);')
        RETURNING id
        "#,
    )
    .bind(*org.as_uuid())
    .bind(stable_key)
    .bind(format!("Policy {stable_key}"))
    .bind(catalog_effect)
    .bind(validation_status)
    .bind(blocks)
    // rls-arming: test fixture writes the protected catalog as DB owner before
    // the route exercises its genuine console_rt read role.
    .fetch_one(owner_pool)
    .await
    .expect("seed enforced policy catalog entry");
    sqlx::query(
        "INSERT INTO ont_object_policies (org_id, object_type_id, cedar_policy_id, effect) VALUES ($1, $2, $3, $4)",
    )
    .bind(*org.as_uuid())
    .bind(*type_id.as_uuid())
    .bind(policy_id)
    .bind(attachment_effect)
    // rls-arming: test fixture attaches the policy as DB owner before the
    // runtime-role request makes the read decision.
    .execute(owner_pool)
    .await
    .expect("attach enforced object policy");
}

async fn attach_enforced_policy_with_validation(
    owner_pool: &PgPool,
    org: OrgId,
    type_id: console_ontology_domain::ObjectTypeId,
    stable_key: &str,
    effect: &str,
    validation_status: &str,
    blocks: Value,
) {
    attach_enforced_policy_with_attachment_effect(
        owner_pool,
        org,
        type_id,
        stable_key,
        effect,
        effect,
        validation_status,
        blocks,
    )
    .await;
}

fn owner_permit_blocks(resource_type: &str) -> Value {
    json!({
        "effect": "permit",
        "action": "view",
        "resource_type": resource_type,
        "conditions": [{
            "attr": "owner",
            "op": "eq",
            "value": { "kind": "subject_attr", "value": "user_id" }
        }]
    })
}

fn flagged_forbid_blocks(resource_type: &str) -> Value {
    json!({
        "effect": "forbid",
        "action": "view",
        "resource_type": resource_type,
        "conditions": [{
            "attr": "flagged",
            "op": "eq",
            "value": { "kind": "bool", "value": true }
        }]
    })
}

fn assert_instance_titles(body: &Value, expected: &[&str]) {
    let titles = body
        .as_array()
        .expect("instance list must be an array")
        .iter()
        .map(|instance| instance["instance"]["title"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(titles, expected);
}

fn assert_exact_instance_ids(body: &Value, expected: &[Uuid]) {
    let ids = body
        .as_array()
        .expect("instance list must be an array")
        .iter()
        .map(|instance| {
            instance["instance"]["id"]
                .as_str()
                .expect("instance id must be serialized as a UUID")
                .parse::<Uuid>()
                .expect("instance id must parse as a UUID")
        })
        .collect::<Vec<_>>();
    assert_eq!(ids, expected);
}

fn body_text(body: &Value) -> String {
    serde_json::to_string(body).expect("response body is serializable")
}

fn draft(title: &str) -> Value {
    json!({
        "stable_key": KEY,
        "title": title,
        "backing_kind": "instance",
        "properties": [],
        "links": [],
        "actions": [],
        "analytics": []
    })
}

fn test_auth(user_id: UserId, org: OrgId) -> TestAuth {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let settings = JwtSettings {
        issuer: TEST_ISSUER.to_owned(),
        audience: TEST_AUDIENCE.to_owned(),
        access_token_ttl: Duration::minutes(15),
    };
    let verifier =
        JwtVerifier::from_es256_public_pem(settings.clone(), public_pem.as_bytes()).unwrap();
    let issuer =
        JwtIssuer::from_es256_pem(settings, private_pem.as_bytes(), public_pem.as_bytes()).unwrap();
    let token = issuer
        .issue_access_token(AccessTokenInput {
            subject: user_id,
            org_id: org,
            roles: vec!["SUPER_ADMIN".to_owned()],
            branches: Vec::new(),
            platform: false,
            view_as: false,
            read_only: false,
            display_name: None,
            feature_grants: Vec::new(),
            authz_subject_version: 0,
            authz_policy_version: 0,
            session_generation: 0,
            issued_at: OffsetDateTime::now_utc(),
        })
        .unwrap();
    TestAuth { token, verifier }
}

async fn request_json(
    service: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    if_match: Option<&[&str]>,
    body: Value,
) -> HttpResponse {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if body != Value::Null {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(values) = if_match {
        for value in values {
            builder = builder.header(header::IF_MATCH, *value);
        }
    }
    let body = if body == Value::Null {
        Body::empty()
    } else {
        Body::from(serde_json::to_vec(&body).unwrap())
    };
    let response = service.oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    HttpResponse {
        status,
        headers,
        body,
    }
}

async fn stored_state(owner_pool: &PgPool, org: OrgId) -> StoredState {
    let title: String = sqlx::query_scalar(
        "SELECT title FROM ont_object_types WHERE org_id = $1 AND stable_key = $2",
    )
    .bind(*org.as_uuid())
    .bind(KEY)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let object_type_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ont_object_types WHERE org_id = $1 AND stable_key = $2",
    )
    .bind(*org.as_uuid())
    .bind(KEY)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let key_revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM ont_object_type_key_revisions WHERE org_id = $1 AND stable_key = $2",
    )
    .bind(*org.as_uuid())
    .bind(KEY)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let stage_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'ontology.object_type.stage_revision'",
    )
    .bind(*org.as_uuid())
    .fetch_one(owner_pool)
    .await
    .unwrap();
    StoredState {
        title,
        object_type_rows,
        key_revision,
        stage_audits,
    }
}
