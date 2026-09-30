#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Proof for the audited object-policy attach writer and for the single-instance
//! read paths that currently ignore the object policy entirely.
//!
//! Every request traverses the real ontology router, a signed-JWT request
//! context, and a genuine `console_rt` pool, so RLS, the transactional audit
//! path, and the enforced-policy residual are all part of the proof rather than
//! mocked away. A superuser or `BYPASSRLS` read would make every isolation
//! assertion here vacuous.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use console_governance_adapter_postgres::PgGovernanceStore;
use console_governance_rest::GovernanceRestState;
use console_kernel_core::{OrgId, TraceContext, UserId};
use console_ontology_adapter_postgres::PgOntologyStore;
use console_ontology_adapter_postgres::instances::{
    CreateInstance, PgInstanceStore, StageRevision,
};
use console_ontology_domain::{LinkTypeId, ObjectTypeId};
use console_ontology_rest::{ONTOLOGY_ROUTE_PATHS, OntologyRestState, router};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_authz::cedar_pbac::authoring::{
    Effect, NoCodeBlocks, SimEffect, SimRequest, SimResource, SimSubject,
};
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

/// The trace context every RAW definer probe below supplies as 0206's 7th and 8th
/// arguments. The audited entrypoint writes them straight onto the audit row
/// (`audit_events.trace_id CHAR(32)`, `span_id CHAR(16)`); the route supplies a
/// freshly generated pair instead, which is what the audit-row pin in the first
/// test measures the length of.
const PROBE_TRACE_ID: &str = "0123456789abcdef0123456789abcdef";
const PROBE_SPAN_ID: &str = "0123456789abcdef";

/// The audited writer this slice ships. Every path in this file is spelled as a
/// literal rather than imported from the crate, so the file pins the wire
/// contract a client depends on and not the name the crate happens to give it.
fn policies_path(stable_key: &str) -> String {
    format!("/api/v1/ontology/object-types/{stable_key}/policies")
}

// ---------------------------------------------------------------------------
// 1. The writer: an attached permit is what makes rows visible, and nothing else
// ---------------------------------------------------------------------------

/// Deny-by-default is the ground state and it survives this slice: a published
/// type with no attached policy serves nothing, and the ONLY thing that changes
/// that is an org-authored permit written through the audited HTTP writer.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_attached_permit_is_the_only_thing_that_makes_instances_visible(owner_pool: PgPool) {
    let fx = Fixture::build(&owner_pool, "policy-attach").await;
    let type_id = fx
        .publish("policyattach", instance_type_draft("policyattach"))
        .await;

    let _mine = fx
        .seed_instance(type_id, "visible-to-owner", "CODE-MINE")
        .await;
    let _theirs = fx
        .seed_instance_owned_by(type_id, "hidden-other-owner", "another-user", "CODE-THEIRS")
        .await;

    // RED control for the assertion below AND the deny-by-default assertion in
    // its own right: with no policy attached the list is empty, so a list that
    // later returns rows can only be the attached permit's doing.
    let before = fx
        .get(&format!("/api/v1/ontology/instances?type={type_id}"))
        .await;
    assert_eq!(before.status, StatusCode::OK, "{:?}", before.body);
    assert_eq!(
        before.body,
        json!([]),
        "an unattached type must serve nothing: deny-by-default is not negotiable"
    );

    let attached = fx
        .post(
            &policies_path("policyattach"),
            json!({ "effect": "permit", "conditions": [owner_is_subject()] }),
        )
        .await;
    assert_eq!(
        attached.status,
        StatusCode::CREATED,
        "attach must be created: {:?}",
        attached.body
    );

    let after = fx
        .get(&format!("/api/v1/ontology/instances?type={type_id}"))
        .await;
    assert_eq!(after.status, StatusCode::OK, "{:?}", after.body);
    assert_instance_titles(&after.body, &["visible-to-owner"]);
    assert!(
        !body_text(&after.body).contains("hidden-other-owner"),
        "the permit does not cover the other owner's row"
    );

    // A principal the policy does not permit gets nothing. Same org, same
    // feature grant, same route -- only the residual differs.
    let stranger = fx.other_principal("policy-attach-stranger").await;
    let strangers_view = fx
        .get_as(
            &format!("/api/v1/ontology/instances?type={type_id}"),
            &stranger,
        )
        .await;
    assert_eq!(
        strangers_view.status,
        StatusCode::OK,
        "{:?}",
        strangers_view.body
    );
    assert_eq!(strangers_view.body, json!([]));

    // A SECOND type, in the same org and the same request, with no attachment.
    // Asserted here rather than in its own test so a mis-seeded fixture cannot
    // fake deny-by-default: the permitted list above proves the fixture works.
    let bare_id = fx
        .publish("policybare", instance_type_draft("policybare"))
        .await;
    let bare_instance = fx.seed_instance(bare_id, "unpoliced", "CODE-BARE").await;
    let bare_list = fx
        .get(&format!("/api/v1/ontology/instances?type={bare_id}"))
        .await;
    assert_eq!(bare_list.body, json!([]), "no policy, no rows");
    let bare_by_id = fx
        .get(&format!("/api/v1/ontology/instances/{bare_instance}"))
        .await;
    assert_eq!(
        bare_by_id.status,
        StatusCode::NOT_FOUND,
        "a row the list hides must not be fetchable by id: {:?}",
        bare_by_id.body
    );

    // The audited evidence, read back as the genuine runtime role: `console_rt`
    // holds SELECT on the catalog and no INSERT (0150:117-118), so a row it can
    // read but not have written is exactly what the definer is for.
    let (status, effect, has_normalized, stable_key): (String, String, bool, String) = fx
        .as_runtime_role(|tx| {
            Box::pin(async move {
                sqlx::query_as(
                    "SELECT status, effect, normalized_row IS NOT NULL, stable_key \
                     FROM cedar_policy_catalog_entries",
                )
                .fetch_one(&mut **tx)
                .await
                .expect("exactly one catalog row must be readable as console_rt")
            })
        })
        .await;
    assert_eq!(status, "enforced");
    assert_eq!(effect, "permit");
    assert!(
        has_normalized,
        "an enforced row without a canonical normalized_row 500s every later read"
    );
    assert!(
        stable_key.contains('.'),
        "catalog stable_key must satisfy the 0150:11 dotted CHECK, got {stable_key:?}"
    );

    let attach_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'ontology.object_policy.attach'",
    )
    .bind(*fx.org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(attach_audits, 1, "the attach must be audited, once");

    // The audit row's CONTENT, not just its existence. 0206 moves the INSERT from
    // `insert_audit_event_tx` in the application into the definer, and every one
    // of these columns has to survive the move byte-identically: the count above
    // stays green against a row whose actor, target or org drifted, and a drifted
    // `actor` would instead throw 23503 on the users FK and surface as a 422.
    let audit: String = sqlx::query_scalar(
        "SELECT actor::TEXT || '|' || target_type || '|' || target_id || '|' \
                || org_id::TEXT || '|' || (before_snap IS NULL)::TEXT || '|' \
                || coalesce(after_snap->>'resource_type', '<no-normalized-row>') || '|' \
                || length(trim(trace_id))::TEXT || '|' || length(trim(span_id))::TEXT \
         FROM audit_events \
         WHERE org_id = $1 AND action = 'ontology.object_policy.attach'",
    )
    .bind(*fx.org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        audit,
        format!(
            "{}|ont_object_policies|{}|{}|true|policyattach|32|16",
            fx.actor.as_uuid(),
            type_id.as_uuid(),
            fx.org.as_uuid()
        ),
        "every column of the attach audit row, pinned. `actor` carries a composite \
         FK to users(id, org_id), so a drifted one throws 23503 and surfaces as a \
         422 rather than a missing audit row; `before_snap` must be NULL because \
         an attach creates; `after_snap` must be the canonical normalized row; and \
         trace/span must be the full 32/16 hex the char(32)/char(16) columns hold"
    );

    let attachments: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ont_object_policies WHERE org_id = $1 AND object_type_id = $2",
    )
    .bind(*fx.org.as_uuid())
    .bind(*type_id.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(attachments, 1);
}

// ---------------------------------------------------------------------------
// 2. Defect (a): a row the list hides is still readable by five other routes
// ---------------------------------------------------------------------------

/// The attachment here is seeded with raw SQL rather than through the writer on
/// purpose: this defect predates the writer and must be red against `main` with
/// no new route present at all. It is also the permanent guard for policies
/// attached by any other surface (the policy studio, a fixture, a migration).
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_row_hidden_from_the_list_is_refused_by_id_as_of_history_traverse_acting_and_resolve(
    owner_pool: PgPool,
) {
    let fx = Fixture::build(&owner_pool, "policy-unfiltered").await;
    let type_id = fx
        .publish("policyreads", linked_instance_type_draft("policyreads"))
        .await;
    let link_type = fx.link_type_id("policyreads").await;

    let mine = fx
        .seed_instance(type_id, "visible-to-owner", "CODE-VISIBLE-V1")
        .await;
    let hidden = fx
        .seed_instance_owned_by(type_id, "hidden-other-owner", "another-user", "CODE-HIDDEN")
        .await;
    fx.link(link_type, mine, hidden).await;

    // A SECOND revision on the permitted row, so `history` has a chain to keep
    // intact and `as_of` has a past to serve that differs from the head. Without
    // it both routes are satisfied by a gate that returns the head twice.
    let between = OffsetDateTime::now_utc();
    fx.revise(
        mine,
        json!({ "owner": fx.actor.to_string(), "code": "CODE-VISIBLE" }),
        between + Duration::seconds(60),
    )
    .await;

    attach_enforced_policy(
        &owner_pool,
        fx.org,
        type_id,
        "policy.owner_permit",
        "permit",
        json!({
            "effect": "permit",
            "action": "view",
            "resource_type": "policyreads",
            "conditions": [owner_is_subject()]
        }),
    )
    .await;

    // The probe's own proof that it is not mis-seeded: the residual demonstrably
    // filters, so every 200 below is a leak and not an empty fixture.
    let list = fx
        .get(&format!("/api/v1/ontology/instances?type={type_id}"))
        .await;
    assert_eq!(list.status, StatusCode::OK, "{:?}", list.body);
    assert_instance_titles(&list.body, &["visible-to-owner"]);

    // POSITIVE CONTROLS, one per gated route. Every refusal below is ALSO
    // satisfied by a gate that refuses everything, so without a positive control
    // on the same route the refusal proves nothing. Only `by id` had one.
    let permitted_by_id = fx.get(&format!("/api/v1/ontology/instances/{mine}")).await;
    assert_eq!(
        permitted_by_id.status,
        StatusCode::OK,
        "the permitted row must still be readable by id: {:?}",
        permitted_by_id.body
    );
    assert_eq!(
        permitted_by_id.body["revision"]["attributes"]["code"],
        json!("CODE-VISIBLE"),
        "by id must serve the HEAD revision: {:?}",
        permitted_by_id.body
    );

    // The as-of branch is gated on the head but must still serve the PAST. A
    // refactor that returned the head for every `as_of` would satisfy the
    // hidden-row refusal above and silently break time travel.
    let past = between
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let permitted_as_of = fx
        .get(&format!("/api/v1/ontology/instances/{mine}?as_of={past}"))
        .await;
    assert_eq!(
        permitted_as_of.status,
        StatusCode::OK,
        "{:?}",
        permitted_as_of.body
    );
    assert_eq!(
        permitted_as_of.body["revision"]["attributes"]["code"],
        json!("CODE-VISIBLE-V1"),
        "as-of must serve the historical revision, not the head it gated on: {:?}",
        permitted_as_of.body
    );

    // The chain comes back INTACT: `verify_chain` breaks on the first prev_hash
    // gap, so a per-revision security filter would masquerade as a tamper alarm.
    let permitted_history = fx
        .get(&format!("/api/v1/ontology/instances/{mine}/history"))
        .await;
    assert_eq!(
        permitted_history.status,
        StatusCode::OK,
        "{:?}",
        permitted_history.body
    );
    assert_eq!(
        permitted_history.body.as_array().map(Vec::len),
        Some(2),
        "both revisions must survive the gate: {:?}",
        permitted_history.body
    );

    let permitted_acting = fx
        .get(&format!("/api/v1/ontology/instances/{mine}/acting"))
        .await;
    assert_eq!(
        permitted_acting.status,
        StatusCode::OK,
        "the permitted row must still expose its acting rules: {:?}",
        permitted_acting.body
    );

    let permitted_resolve = fx.get("/api/v1/ontology/resolve?code=CODE-VISIBLE").await;
    assert_eq!(
        permitted_resolve.status,
        StatusCode::OK,
        "the permitted row must still resolve by code: {:?}",
        permitted_resolve.body
    );
    assert_eq!(permitted_resolve.body["id"], json!(mine.to_string()));

    // A nonexistent id is the same 404 as a denied one, from the same gate.
    let nowhere = fx
        .get(&format!("/api/v1/ontology/instances/{}", Uuid::new_v4()))
        .await;
    assert_eq!(nowhere.status, StatusCode::NOT_FOUND, "{:?}", nowhere.body);

    // Same STATUS is not enough on `/resolve`: it is the one gated route whose
    // own miss carries a different message than the gate's, so the BODY has to
    // be compared too. Object codes are human-meaningful and enumerable, so a
    // distinguishable refusal tells a caller which hidden rows exist.
    let unknown_code = fx
        .get("/api/v1/ontology/resolve?code=CODE-NOBODY-ISSUED")
        .await;
    let hidden_code = fx.get("/api/v1/ontology/resolve?code=CODE-HIDDEN").await;
    assert_eq!(
        unknown_code.status,
        StatusCode::NOT_FOUND,
        "{:?}",
        unknown_code.body
    );
    assert_eq!(
        hidden_code.body, unknown_code.body,
        "resolving a HIDDEN code must be byte-identical to resolving one that was never \
         issued; a distinguishable body is an existence oracle over the org's hidden rows"
    );

    let as_of = OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    for (label, uri) in [
        ("by id", format!("/api/v1/ontology/instances/{hidden}")),
        (
            "by as-of",
            format!("/api/v1/ontology/instances/{hidden}?as_of={as_of}"),
        ),
        (
            "by history",
            format!("/api/v1/ontology/instances/{hidden}/history"),
        ),
        (
            "by traverse",
            format!("/api/v1/ontology/instances/{hidden}/traverse"),
        ),
        (
            "by acting",
            format!("/api/v1/ontology/instances/{hidden}/acting"),
        ),
        (
            "by code",
            "/api/v1/ontology/resolve?code=CODE-HIDDEN".to_owned(),
        ),
    ] {
        let refused = fx.get(&uri).await;
        assert_eq!(
            refused.status,
            StatusCode::NOT_FOUND,
            "{label}: a policy-denied row must be 404, never 403 -- a 403 makes the \
             status code an existence oracle: {:?}",
            refused.body
        );
        assert!(
            !body_text(&refused.body).contains("hidden-other-owner"),
            "{label}: the refusal body discloses the hidden title"
        );
    }

    // Root-only gating measurably leaks: traversing from a PERMITTED root
    // returned both hidden neighbours' titles at depth 1. The hydrated node set
    // has to be filtered too, along with the edges that touch it.
    let neighbours = fx
        .get(&format!(
            "/api/v1/ontology/instances/{mine}/traverse?depth=1"
        ))
        .await;
    assert_eq!(neighbours.status, StatusCode::OK, "{:?}", neighbours.body);
    let graph = body_text(&neighbours.body);
    assert!(
        graph.contains(&mine.to_string()),
        "traverse must still serve its permitted root: {graph}"
    );
    assert!(
        !graph.contains("hidden-other-owner"),
        "traverse from a permitted root disclosed the hidden neighbour's title: {graph}"
    );
    assert!(
        !graph.contains(&hidden.to_string()),
        "traverse from a permitted root disclosed the hidden neighbour's id: {graph}"
    );
}

/// The traversal gate is not "hide the neighbour I linked to". Two properties it
/// claims and nothing exercised: a node is gated by the policies of ITS OWN
/// object type (`by_type`), and a node whose only path ran through a hidden one
/// is dropped even though the node itself is permitted -- otherwise its surviving
/// depth discloses the length of the hidden path.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_traversal_gates_each_node_by_its_own_type_and_drops_what_only_a_hidden_path_reached(
    owner_pool: PgPool,
) {
    let fx = Fixture::build(&owner_pool, "policy-graph").await;
    let web = fx
        .publish("policyweb", linked_instance_type_draft("policyweb"))
        .await;
    let link_type = fx.link_type_id("policyweb").await;
    let other = fx
        .publish("policyother", instance_type_draft("policyother"))
        .await;

    let root = fx.seed_instance(web, "graph-root", "G-ROOT").await;
    let gate = fx
        .seed_instance_owned_by(web, "hidden-gate", "another-user", "G-GATE")
        .await;
    // Owned by the CALLER, so the permit covers it: the only reason it may not
    // appear is that every path to it runs through the hidden gate.
    let behind = fx.seed_instance(web, "behind-the-gate", "G-BEHIND").await;
    // Also owned by the caller, but of a DIFFERENT object type, which has no
    // policy of its own yet.
    let cross = fx
        .seed_instance(other, "cross-type-neighbour", "G-CROSS")
        .await;
    fx.link(link_type, root, gate).await;
    fx.link(link_type, gate, behind).await;
    fx.link(link_type, root, cross).await;

    let attached = fx
        .post(
            &policies_path("policyweb"),
            json!({ "effect": "permit", "conditions": [owner_is_subject()] }),
        )
        .await;
    assert_eq!(attached.status, StatusCode::CREATED, "{:?}", attached.body);

    let traversed = fx
        .get(&format!(
            "/api/v1/ontology/instances/{root}/traverse?depth=3"
        ))
        .await;
    assert_eq!(traversed.status, StatusCode::OK, "{:?}", traversed.body);
    let graph = body_text(&traversed.body);
    assert!(
        graph.contains(&root.to_string()),
        "the permitted root must survive: {graph}"
    );
    assert!(
        !graph.contains(&gate.to_string()) && !graph.contains("hidden-gate"),
        "the hidden neighbour leaked: {graph}"
    );
    assert!(
        !graph.contains(&behind.to_string()) && !graph.contains("behind-the-gate"),
        "a node reachable ONLY through the hidden gate leaked, disclosing that the \
         hidden path exists and how long it is: {graph}"
    );
    assert!(
        !graph.contains(&cross.to_string()) && !graph.contains("cross-type-neighbour"),
        "a neighbour of an UNPOLICED object type leaked: deny-by-default is \
         per-type, and the root's permit says nothing about it: {graph}"
    );

    // Now give the OTHER type its own permit. If the gate resolved policies from
    // the root's type instead of each node's own, this changes nothing and the
    // assertion below fails -- which is what makes the exclusion above meaningful
    // rather than a blanket "drop everything unfamiliar".
    let attached_other = fx
        .post(
            &policies_path("policyother"),
            json!({ "effect": "permit", "conditions": [owner_is_subject()] }),
        )
        .await;
    assert_eq!(
        attached_other.status,
        StatusCode::CREATED,
        "{:?}",
        attached_other.body
    );
    let retraversed = fx
        .get(&format!(
            "/api/v1/ontology/instances/{root}/traverse?depth=3"
        ))
        .await;
    assert_eq!(retraversed.status, StatusCode::OK, "{:?}", retraversed.body);
    let graph = body_text(&retraversed.body);
    assert!(
        graph.contains(&cross.to_string()),
        "the cross-type neighbour must appear once ITS OWN type is policed: {graph}"
    );
    assert!(
        !graph.contains(&behind.to_string()),
        "the transitively-hidden node must stay hidden: {graph}"
    );
}

/// `PgInstanceStore::traverse` is a level-synchronous BFS, so `depth` is the
/// SHORTEST hop count from the root. The gate re-walks the surviving edges to
/// recompute it, and that re-walk has to be a BFS too.
///
/// The diamond below is the discriminator, and it needs NOTHING hidden: with
/// every node permitted the gated graph must be identical to the ungated one.
/// A stack-based re-walk (`frontier.pop()`) reaches `far` first through the long
/// arm, records 3, and the `Vacant` guard then refuses to lower it when the
/// short arm arrives -- reporting a node as further from the root than it is.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_fully_permitted_traversal_reports_shortest_hop_depths(owner_pool: PgPool) {
    let fx = Fixture::build(&owner_pool, "policy-depth").await;
    let type_id = fx
        .publish("policydepth", linked_instance_type_draft("policydepth"))
        .await;
    let link_type = fx.link_type_id("policydepth").await;

    // root -> short -> far          (2 hops, the true depth)
    // root -> long  -> mid -> far   (3 hops)
    let root = fx.seed_instance(type_id, "d-root", "D-ROOT").await;
    let short = fx.seed_instance(type_id, "d-short", "D-SHORT").await;
    let long = fx.seed_instance(type_id, "d-long", "D-LONG").await;
    let mid = fx.seed_instance(type_id, "d-mid", "D-MID").await;
    let far = fx.seed_instance(type_id, "d-far", "D-FAR").await;
    fx.link(link_type, root, short).await;
    fx.link(link_type, root, long).await;
    fx.link(link_type, long, mid).await;
    fx.link(link_type, mid, far).await;
    fx.link(link_type, short, far).await;

    let attached = fx
        .post(
            &policies_path("policydepth"),
            json!({ "effect": "permit", "conditions": [owner_is_subject()] }),
        )
        .await;
    assert_eq!(attached.status, StatusCode::CREATED, "{:?}", attached.body);

    let traversed = fx
        .get(&format!(
            "/api/v1/ontology/instances/{root}/traverse?depth=3"
        ))
        .await;
    assert_eq!(traversed.status, StatusCode::OK, "{:?}", traversed.body);
    let nodes = traversed.body["nodes"]
        .as_array()
        .expect("traverse returns a node array")
        .clone();
    let depth_of = |id: Uuid| -> u64 {
        nodes
            .iter()
            .find(|node| node["instance_id"] == json!(id.to_string()))
            .unwrap_or_else(|| panic!("{id} is missing from the permitted graph: {nodes:?}"))["depth"]
            .as_u64()
            .expect("depth is a number")
    };
    assert_eq!(depth_of(root), 0);
    assert_eq!(depth_of(short), 1);
    assert_eq!(depth_of(long), 1);
    assert_eq!(depth_of(mid), 2);
    assert_eq!(
        depth_of(far),
        2,
        "`far` is two hops away via `short`; a depth-first re-walk reports the \
         long arm's 3 and never lowers it: {nodes:?}"
    );

    // The documented node ordering (`instances.rs`: `sort_by_key((depth, id))`)
    // is part of the response contract, and recomputed depths can break it.
    let ordering: Vec<(u64, String)> = nodes
        .iter()
        .map(|node| {
            (
                node["depth"].as_u64().unwrap(),
                node["instance_id"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let mut sorted = ordering.clone();
    sorted.sort();
    assert_eq!(
        ordering, sorted,
        "nodes must stay ordered by (depth, id) after the gate recomputes depth"
    );
}

/// A `forbid` authored through the writer, and a SECOND policy on a type the
/// route already policed. Both are claimed in comments
/// (`rest/src/lib.rs:487-495`) and neither was exercised: the `Effect::Forbid`
/// arm, the effect that must agree across catalog row, attachment and the
/// `0170` trigger, and the fresh-discriminator `stable_key` that makes two
/// policies on one type legal under the `(org, key, status)` UNIQUE.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_forbid_attached_beside_a_permit_wins_and_two_policies_may_share_a_type(
    owner_pool: PgPool,
) {
    let fx = Fixture::build(&owner_pool, "policy-forbid").await;
    let type_id = fx
        .publish("policyforbid", instance_type_draft("policyforbid"))
        .await;
    let _mine = fx.seed_instance(type_id, "mine", "F-MINE").await;
    let _theirs = fx
        .seed_instance_owned_by(type_id, "theirs", "another-user", "F-THEIRS")
        .await;

    let permit = fx
        .post(
            &policies_path("policyforbid"),
            json!({ "effect": "permit", "conditions": [] }),
        )
        .await;
    assert_eq!(permit.status, StatusCode::CREATED, "{:?}", permit.body);
    let listed = fx
        .get(&format!("/api/v1/ontology/instances?type={type_id}"))
        .await;
    assert_instance_titles(&listed.body, &["theirs", "mine"]);

    // Same type, second policy, opposite effect. A UNIQUE collision on
    // stable_key would surface here as a 5xx.
    let forbid = fx
        .post(
            &policies_path("policyforbid"),
            json!({ "effect": "forbid", "conditions": [owner_is_subject()] }),
        )
        .await;
    assert_eq!(
        forbid.status,
        StatusCode::CREATED,
        "a second policy on the same type must be attachable: {:?}",
        forbid.body
    );

    let listed = fx
        .get(&format!("/api/v1/ontology/instances?type={type_id}"))
        .await;
    assert_instance_titles(&listed.body, &["theirs"]);

    // The forbid must reach the single-instance gate too, or the by-id route is
    // a bypass of the row the list just removed.
    let mine_by_id = fx.get(&format!("/api/v1/ontology/instances/{_mine}")).await;
    assert_eq!(
        mine_by_id.status,
        StatusCode::NOT_FOUND,
        "a forbidden row must be 404 by id as well: {:?}",
        mine_by_id.body
    );

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT c.effect, p.effect FROM cedar_policy_catalog_entries c \
         JOIN ont_object_policies p ON p.cedar_policy_id = c.id AND p.org_id = c.org_id \
         WHERE c.org_id = $1 ORDER BY c.effect",
    )
    .bind(*fx.org.as_uuid())
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            ("forbid".to_owned(), "forbid".to_owned()),
            ("permit".to_owned(), "permit".to_owned())
        ],
        "catalog effect and attachment effect must agree; 0170's trigger is the \
         last line of defence and it only fires if both are written"
    );

    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'ontology.object_policy.attach'",
    )
    .bind(*fx.org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(audits, 2, "both attaches must be audited");
}

// ---------------------------------------------------------------------------
// 2b. Defect (a), the WRITE half: preflight, execute and lifecycle
// ---------------------------------------------------------------------------

/// `prepare()` (`rest/src/lib.rs:1248`) loads the action target through an
/// UNGATED `self.instances.get_current(id)` (`:1271-1281`) and drops the row's
/// `revision.attributes` into the evaluation context, so BOTH action routes read
/// a row every gated read path refuses -- and `execute` then APPENDS A REVISION
/// to it. `commit_lifecycle` (`:1707-1717`) reads the same head ungated and then
/// transitions the row.
///
/// Mutation of a policy-hidden row is strictly worse than reading one, so the
/// zero-write assertions here are the point of the test, not decoration. They
/// are counted on `owner_pool` -- the `#[sqlx::test]` BYPASSRLS superuser -- on
/// purpose: a NEGATIVE "nothing was written" claim must be made by a reader that
/// cannot miss a row. Every ISOLATION assertion still goes through the router on
/// the genuine `console_rt` pool.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_row_hidden_from_the_list_is_refused_by_preflight_execute_and_lifecycle(
    owner_pool: PgPool,
) {
    let fx = Fixture::build(&owner_pool, "policy-writes").await;
    let type_id = fx
        .publish("policywrites", editable_instance_type_draft("policywrites"))
        .await;

    let mine = fx
        .seed_instance(type_id, "visible-to-owner", "CODE-W-VISIBLE")
        .await;
    let hidden = fx
        .seed_instance_owned_by(
            type_id,
            "hidden-other-owner",
            "another-user",
            "CODE-W-HIDDEN",
        )
        .await;

    attach_enforced_policy(
        &owner_pool,
        fx.org,
        type_id,
        "policy.write_owner_permit",
        "permit",
        json!({
            "effect": "permit",
            "action": "view",
            "resource_type": "policywrites",
            "conditions": [owner_is_subject()]
        }),
    )
    .await;

    // The probe's own proof that it is not mis-seeded: the residual demonstrably
    // filters, so every 200 below is a leak and not an empty fixture.
    let list = fx
        .get(&format!("/api/v1/ontology/instances?type={type_id}"))
        .await;
    assert_eq!(list.status, StatusCode::OK, "{:?}", list.body);
    assert_instance_titles(&list.body, &["visible-to-owner"]);

    // POSITIVE CONTROLS, one per route. Every refusal below is ALSO satisfied by
    // a gate that refuses everything, so without a positive control on the same
    // route the refusal proves nothing.
    // Preflight is asked WITHOUT `command_id`/`expected_revision`: it evaluates
    // neither, and the shipped request schema does not require them. A preflight
    // that refused this body would 422 the console dialog that renders the gate
    // report before a command id is minted.
    let permitted_preflight = fx
        .post(
            "/api/v1/ontology/actions/set_code/preflight",
            json!({
                "object_type_id": type_id.to_string(),
                "instance_id": mine,
                "params": { "code": "CODE-W-EDITED" }
            }),
        )
        .await;
    assert_eq!(
        permitted_preflight.status,
        StatusCode::OK,
        "the permitted row must still preflight: {:?}",
        permitted_preflight.body
    );

    let permitted_execute = fx
        .post(
            "/api/v1/ontology/actions/set_code/execute",
            json!({
                "object_type_id": type_id.to_string(),
                "instance_id": mine,
                "params": { "code": "CODE-W-EDITED" },
                "expected_revision": 1,
                "command_id": Uuid::new_v4()
            }),
        )
        .await;
    assert_eq!(
        permitted_execute.status,
        StatusCode::OK,
        "the permitted row must still be actionable: {:?}",
        permitted_execute.body
    );

    // The lifecycle edge is unconfigured for this type, so BOTH rows would be a
    // 403 today. The control is therefore "not 404": it proves the gate is not
    // simply refusing every row, while the hidden row below must become a 404.
    let permitted_lifecycle = fx
        .post(
            &format!("/api/v1/ontology/instances/{mine}/lifecycle"),
            json!({ "to_state": "active" }),
        )
        .await;
    assert_ne!(
        permitted_lifecycle.status,
        StatusCode::NOT_FOUND,
        "the permitted row must reach the lifecycle gate chain, not the visibility gate: {:?}",
        permitted_lifecycle.body
    );

    // --- the hidden row ----------------------------------------------------
    let revisions_before = fx.revision_count(hidden).await;
    let state_before = fx.lifecycle_state(hidden).await;
    assert_eq!(revisions_before, 1, "the hidden row starts at one revision");

    // All three requests are issued BEFORE the first assertion. Asserting inline
    // would stop at the first failure and leave the two write routes unexercised
    // -- and it is the write routes, not preflight, that carry the severe defect.
    let refused_preflight = fx
        .post(
            "/api/v1/ontology/actions/set_code/preflight",
            json!({
                "object_type_id": type_id.to_string(),
                "instance_id": hidden,
                "params": { "code": "CODE-W-PWNED" }
            }),
        )
        .await;
    let refused_execute = fx
        .post(
            "/api/v1/ontology/actions/set_code/execute",
            json!({
                "object_type_id": type_id.to_string(),
                "instance_id": hidden,
                "params": { "code": "CODE-W-PWNED" },
                "expected_revision": 1,
                "command_id": Uuid::new_v4()
            }),
        )
        .await;
    let refused_lifecycle = fx
        .post(
            &format!("/api/v1/ontology/instances/{hidden}/lifecycle"),
            json!({ "to_state": "active" }),
        )
        .await;

    // NOTHING was written. Asserted FIRST: the refusal has to happen before any
    // writeback opens, not merely be reported after one.
    assert_eq!(
        fx.revision_count(hidden).await,
        revisions_before,
        "execute appended a revision to a policy-hidden row: {:?}",
        refused_execute.body
    );
    assert_eq!(
        fx.lifecycle_state(hidden).await,
        state_before,
        "the lifecycle route transitioned a policy-hidden row: {:?}",
        refused_lifecycle.body
    );
    assert_eq!(
        fx.command_receipt_count().await,
        1,
        "only the permitted execute may leave a command receipt"
    );

    assert_eq!(
        refused_execute.status,
        StatusCode::NOT_FOUND,
        "executing an action against a policy-hidden row must be 404: {:?}",
        refused_execute.body
    );
    assert!(
        !body_text(&refused_execute.body).contains("hidden-other-owner"),
        "the refusal body discloses the hidden title: {:?}",
        refused_execute.body
    );
    assert_eq!(
        refused_lifecycle.status,
        StatusCode::NOT_FOUND,
        "a lifecycle transition on a policy-hidden row must be 404: a 403 naming the \
         from-state discloses both that the row exists and what state it is in: {:?}",
        refused_lifecycle.body
    );
    assert_eq!(
        refused_preflight.status,
        StatusCode::NOT_FOUND,
        "preflight on a policy-hidden row must be 404, never 403 -- and `criteria_ok` is a \
         boolean oracle over the row's attributes: {:?}",
        refused_preflight.body
    );
}

// ---------------------------------------------------------------------------
// 3. Refusals. Written now, because these are the ones that get dropped later.
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn the_attach_route_refuses_unauthorized_unrepresentable_and_malformed_policies(
    owner_pool: PgPool,
) {
    let fx = Fixture::build(&owner_pool, "policy-attach-refusals").await;
    let good_id = fx
        .publish("policygood", instance_type_draft("policygood"))
        .await;
    let _good = fx.seed_instance(good_id, "reachable", "CODE-GOOD").await;
    let bad_id = fx
        .publish("policydated", dated_type_draft("policydated"))
        .await;
    let _bad = fx.seed_instance(bad_id, "unreachable", "CODE-DATED").await;

    // In-test RED control: the working permit proves the route CAN create, so
    // every refusal below is a refusal and not a route that never works.
    let good = fx
        .post(
            &policies_path("policygood"),
            json!({ "effect": "permit", "conditions": [owner_is_subject()] }),
        )
        .await;
    assert_eq!(good.status, StatusCode::CREATED, "{:?}", good.body);
    let good_list = fx
        .get(&format!("/api/v1/ontology/instances?type={good_id}"))
        .await;
    assert_instance_titles(&good_list.body, &["reachable"]);

    let anonymous = fx
        .request(
            "POST",
            &policies_path("policygood"),
            None,
            json!({ "effect": "permit", "conditions": [] }),
        )
        .await;
    assert!(
        anonymous.status == StatusCode::UNAUTHORIZED || anonymous.status == StatusCode::FORBIDDEN,
        "an unauthenticated attach must be refused, got {}: {:?}",
        anonymous.status,
        anonymous.body
    );

    // Bite 5: `declared` maps only Boolean and Text, so a condition over a Date
    // property fails Cedar validation. Refuse it at attach with the validator's
    // own words instead of letting it land enforced and 500 every later read.
    let unrepresentable = fx
        .post(
            &policies_path("policydated"),
            json!({
                "effect": "permit",
                "conditions": [{
                    "attr": "due_date",
                    "op": "eq",
                    "value": { "kind": "literal", "value": "2026-01-01" }
                }]
            }),
        )
        .await;
    assert_eq!(
        unrepresentable.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a condition over a Date property must be refused at attach: {:?}",
        unrepresentable.body
    );
    assert!(
        body_text(&unrepresentable.body).contains("due_date"),
        "the refusal must carry the validator's own message: {:?}",
        unrepresentable.body
    );

    // A subject-attr reference outside the whitelist is a validation failure at
    // authoring (authoring.rs:310-316), not a silent deny.
    let unknown_subject = fx
        .post(
            &policies_path("policydated"),
            json!({
                "effect": "permit",
                "conditions": [{
                    "attr": "owner",
                    "op": "eq",
                    "value": { "kind": "subject_attr", "value": "not_a_subject_attr" }
                }]
            }),
        )
        .await;
    assert_eq!(
        unknown_subject.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        unknown_subject.body
    );

    // An attachment can never be revoked through any application path --
    // `ont_object_policies` is append-only (0154:90-99) and NO role holds UPDATE
    // on `cedar_policy_catalog_entries` (0150:118 revokes it, 0205:103 grants the
    // writer only SELECT/INSERT), so `status` can never leave `enforced`. An
    // oversized condition list is therefore permanent: every later read of the
    // type re-validates, re-renders, re-normalizes and lowers all N predicates,
    // on the list AND on all five single-instance paths AND once per node-type
    // group inside a traversal. Bound what gets PERSISTED.
    let oversized: Vec<Value> = (0..64).map(|_| owner_is_subject()).collect();
    let too_many = fx
        .post(
            &policies_path("policygood"),
            json!({ "effect": "permit", "conditions": oversized }),
        )
        .await;
    assert_eq!(
        too_many.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an unbounded condition list is a permanent amplification, not a big rule: {:?}",
        too_many.body
    );

    let malformed = fx
        .post(
            &policies_path("policygood"),
            json!({ "effect": "maybe", "conditions": [] }),
        )
        .await;
    assert!(
        malformed.status.is_client_error(),
        "a malformed effect must be refused by the route, got {}: {:?}",
        malformed.status,
        malformed.body
    );

    // A validly SIGNED token for a principal with no `users` row. `created_by`
    // FKs `(created_by, org_id) -> users(id, org_id)` on both tables written by
    // the definer (0150:41, and the same shape on ont_object_policies), so an
    // unresolvable principal reaches the database and comes back as a raw
    // constraint violation. Whatever the verdict, it must be a mapped client
    // error -- a 500 here tells a caller it found an unhandled server path.
    let ghost = issue_token(
        &SIGNING_KEY.with(SigningKey::clone),
        UserId::from_uuid(Uuid::new_v4()),
        fx.org,
    );
    let ghost_attach = fx
        .request(
            "POST",
            &policies_path("policygood"),
            Some(&ghost),
            json!({ "effect": "permit", "conditions": [] }),
        )
        .await;
    assert!(
        ghost_attach.status.is_client_error(),
        "an attach by a principal with no user row must be a mapped 4xx, got {}: {:?}",
        ghost_attach.status,
        ghost_attach.body
    );
    assert_eq!(
        ghost_attach.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the 23503 mapping (`rest/src/lib.rs:755-762`) is the whole reason this is not \
         a 500; `is_client_error` alone is also satisfied by a 400 from a body parser \
         that never reached the database: {:?}",
        ghost_attach.body
    );
    // `rest/src/lib.rs:761` keeps the message deliberately generic and leaves the
    // constraint name in the log. Nothing asserted that, so a later "let's include
    // the DB error, it helps debugging" would ship the schema to any caller holding
    // a token for a since-removed user.
    let ghost_text = body_text(&ghost_attach.body);
    for leaked in [
        "fkey",
        "cedar_policy_catalog_entries",
        "ont_object_policies",
        "created_by",
    ] {
        assert!(
            !ghost_text.contains(leaked),
            "the refusal discloses the schema through {leaked:?}; the constraint name \
             belongs in the log, not in the response body: {ghost_text}"
        );
    }

    let unknown_type = fx
        .post(
            &policies_path("policynosuchtype"),
            json!({ "effect": "permit", "conditions": [] }),
        )
        .await;
    assert_eq!(
        unknown_type.status,
        StatusCode::NOT_FOUND,
        "{:?}",
        unknown_type.body
    );

    // An unresolvable object type is a 404, NEVER an empty policy set: the two are
    // indistinguishable at the list endpoint (both serve no rows), so only this
    // assertion keeps `object_view_policies` from degrading into "unknown type =>
    // unpoliced" the day something upstream stops failing loudly.
    let unknown_list = fx
        .get(&format!(
            "/api/v1/ontology/instances?type={}",
            Uuid::new_v4()
        ))
        .await;
    assert_eq!(
        unknown_list.status,
        StatusCode::NOT_FOUND,
        "{:?}",
        unknown_list.body
    );

    // Nothing partial persisted: the only enforced row is the control's, and the
    // refused type is still invisible.
    let dated_list = fx
        .get(&format!("/api/v1/ontology/instances?type={bad_id}"))
        .await;
    assert_eq!(
        dated_list.body,
        json!([]),
        "a refused attach must leave no enforced row behind"
    );
    let catalog_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cedar_policy_catalog_entries WHERE org_id = $1")
            .bind(*fx.org.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        catalog_rows, 1,
        "only the control attach may have written a row"
    );
}

// ---------------------------------------------------------------------------
// 4. Defect (c): a dotted object-type key must be policyable, not merely loud
// ---------------------------------------------------------------------------

/// `0165:94` allows a dotted object-type `stable_key`; `authoring.rs:279` rejects
/// a dotted `resource_type`. Refusing the attach would make every dotted type
/// permanently un-policyable and therefore permanently invisible on all five read
/// paths -- a hole, not a fix. The assertion is positive on purpose: rows must
/// come back, so neither the loud 500 branch nor the silent empty-list branch can
/// pass it.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_dotted_object_type_key_is_policyable_end_to_end(owner_pool: PgPool) {
    let fx = Fixture::build(&owner_pool, "policy-dotted").await;
    let type_id = fx
        .publish("hr.employee", instance_type_draft("hr.employee"))
        .await;
    let _mine = fx
        .seed_instance(type_id, "dotted-and-visible", "CODE-DOTTED")
        .await;

    let attached = fx
        .post(
            &policies_path("hr.employee"),
            json!({ "effect": "permit", "conditions": [owner_is_subject()] }),
        )
        .await;
    assert_eq!(
        attached.status,
        StatusCode::CREATED,
        "a dotted type key must be attachable: {:?}",
        attached.body
    );

    let list = fx
        .get(&format!("/api/v1/ontology/instances?type={type_id}"))
        .await;
    assert_eq!(
        list.status,
        StatusCode::OK,
        "a dotted resource_type must not 500 the list: {:?}",
        list.body
    );
    assert_instance_titles(&list.body, &["dotted-and-visible"]);
}

// ---------------------------------------------------------------------------
// 4b. Gating a read must not lose the row when its type has been revised
// ---------------------------------------------------------------------------

/// Staging a revision behind a published head INSERTS A NEW `ont_object_types`
/// ROW with a fresh `gen_random_uuid()` and `schema_version = max + 1`
/// (`0165:884-900`), and nothing anywhere re-points `ont_instances.object_type_id`
/// at it. Existing rows therefore stay filed under the version that created them
/// — `object_type_lifecycle_over_http.rs:713-715` already pins `v1_id != v2_id`.
///
/// Both registry lookups the gate can reach are `DISTINCT ON (o.stable_key)`
/// (`adapter-postgres/src/lib.rs:581-589`, `:615-626`), so they resolve exactly
/// ONE version per key. A gate that resolves an instance's type by scanning that
/// list cannot resolve a superseded version id at all, and every read of every
/// pre-revision row becomes a permanent 404 the instant a v2 is published.
///
/// The fixture below reproduces the ELSE branch of `stage_object_type` directly
/// — same columns, same fresh id, same `max + 1` — because reaching it over HTTP
/// needs the full four-eyes publish dance for a fact this states in six lines.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_row_survives_a_revision_of_its_object_type(owner_pool: PgPool) {
    let fx = Fixture::build(&owner_pool, "policy-superseded").await;
    let v1 = fx
        .publish("superseded", instance_type_draft("superseded"))
        .await;
    let mine = fx.seed_instance(v1, "filed-under-v1", "CODE-V1").await;
    let attached = fx
        .post(
            &policies_path("superseded"),
            json!({ "effect": "permit", "conditions": [owner_is_subject()] }),
        )
        .await;
    assert_eq!(attached.status, StatusCode::CREATED, "{:?}", attached.body);

    // CONTROL: before the revision, every gated route serves the row. Without
    // this the assertions below would also pass against a fixture that never
    // worked at all.
    for uri in [
        format!("/api/v1/ontology/instances?type={v1}"),
        format!("/api/v1/ontology/instances/{mine}"),
        format!("/api/v1/ontology/instances/{mine}/history"),
        format!("/api/v1/ontology/instances/{mine}/traverse"),
        format!("/api/v1/ontology/instances/{mine}/acting"),
        "/api/v1/ontology/resolve?code=CODE-V1".to_owned(),
    ] {
        let res = fx.get(&uri).await;
        assert_eq!(
            res.status,
            StatusCode::OK,
            "control {uri} must be readable BEFORE the revision: {:?}",
            res.body
        );
    }

    // The real versioning path, over HTTP: publish v1, stage v2, publish v2.
    fx.revise_type_to_a_new_published_version("superseded", instance_type_draft_v2("superseded"))
        .await;

    // The instance is untouched, its attachment is untouched, and its policy
    // still permits its owner. Only the registry head moved.
    for uri in [
        format!("/api/v1/ontology/instances?type={v1}"),
        format!("/api/v1/ontology/instances/{mine}"),
        format!("/api/v1/ontology/instances/{mine}/history"),
        format!("/api/v1/ontology/instances/{mine}/traverse"),
        format!("/api/v1/ontology/instances/{mine}/acting"),
        "/api/v1/ontology/resolve?code=CODE-V1".to_owned(),
    ] {
        let res = fx.get(&uri).await;
        assert_eq!(
            res.status,
            StatusCode::OK,
            "{uri}: revising the type must not delete every row filed under the \
             previous version -- the gate has to resolve the instance's OWN \
             object-type version, not the registry head: {:?}",
            res.body
        );
    }
    let list = fx
        .get(&format!("/api/v1/ontology/instances?type={v1}"))
        .await;
    assert_instance_titles(&list.body, &["filed-under-v1"]);
}

// ---------------------------------------------------------------------------
// 5. Defect (b) + the definer-owner pin
// ---------------------------------------------------------------------------

/// The silent catastrophe this pins: a dropped `ALTER FUNCTION ... OWNER TO`
/// leaves the migration applier as owner -- `console_app` in production and the
/// sqlx superuser locally, both `BYPASSRLS`. The org floor would be gone and
/// every other test in this file would stay green.
///
/// TOTAL over the schema, like 0206's owner pin and like
/// `the_attach_schema_grants_exactly_the_audited_command_credential`, and it was
/// NOT: this query filtered `proname = 'attach_object_policy'`, so it pinned the
/// 8-argument entrypoint and said nothing about `attach_object_policy_rows` --
/// the routine that holds 0205's entire envelope and performs both policy
/// INSERTs. That body calls EIGHT unqualified `pg_catalog` functions
/// (`gen_random_uuid`, `replace`, `left`, `sha256`, `convert_to`, `encode`,
/// `jsonb_typeof`, `jsonb_array_length`; `0205` §4's INSERT and its shape
/// checks), so its `SET search_path = pg_catalog` is the only thing standing
/// between a SECURITY DEFINER owned by the writer role and a caller-controlled
/// resolution of those eight names. `ALTER FUNCTION ... RENAME` preserves
/// `proconfig`, which is why it holds today -- and a later
/// `CREATE OR REPLACE FUNCTION ont_policy_api.attach_object_policy_rows(...)`
/// that omitted the two SET clauses would pass every other test in this file.
/// Measured, not argued: with `ALTER FUNCTION ont_policy_api
/// .attach_object_policy_rows(...) RESET search_path` appended to 0206, this
/// assertion fails on `attach_object_policy_rows|true|console_ontology_writer|
/// false|true` and the other 26 tests in this file all pass. The per-name query
/// this replaced could not fail on it at all.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn the_attach_definer_is_owned_by_a_non_bypassrls_role_under_a_pinned_search_path(
    owner_pool: PgPool,
) {
    let attributes: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT p.proname || '|' || p.prosecdef::TEXT || '|' || r.rolname || '|'
               || (COALESCE(p.proconfig, '{}') @> ARRAY['search_path=pg_catalog'])::TEXT || '|'
               || (COALESCE(p.proconfig, '{}') @> ARRAY['row_security=on'])::TEXT
        FROM pg_proc p
        JOIN pg_namespace n ON n.oid = p.pronamespace
        JOIN pg_roles r ON r.oid = p.proowner
        WHERE n.nspname = 'ont_policy_api'
        ORDER BY p.proname
        "#,
    )
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        attributes,
        vec![
            "attach_object_policy|true|console_ontology_writer|true|true".to_owned(),
            "attach_object_policy_rows|true|console_ontology_writer|true|true".to_owned(),
        ],
        "every routine in ont_policy_api must be SECURITY DEFINER, owned by the \
         NOBYPASSRLS writer role, with search_path AND row_security pinned. A \
         routine here that resolves unqualified names through the CALLER's \
         search_path is a definer hijack, and the row-writer is the one with a \
         body full of unqualified pg_catalog calls"
    );

    // Total over the new schema, so a future routine added beside it cannot
    // regress without a baseline anyone has to remember to update.
    let unsafe_definers: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT p.proname
        FROM pg_proc p
        JOIN pg_namespace n ON n.oid = p.pronamespace
        JOIN pg_roles r ON r.oid = p.proowner
        WHERE n.nspname = 'ont_policy_api'
          AND p.prosecdef
          AND (r.rolsuper OR r.rolbypassrls)
        "#,
    )
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert!(unsafe_definers.is_empty(), "{unsafe_definers:?}");

    // Defect (b): 0170's trigger function reads the catalog UNQUALIFIED with no
    // `SET search_path`, so it inherits the definer's `pg_catalog` and raises
    // 42P01. CREATE OR REPLACE fixes it while preserving the 0170:24-26 binding.
    let trigger_config: Option<Vec<String>> = sqlx::query_scalar(
        "SELECT proconfig FROM pg_proc WHERE proname = 'enforce_ont_object_policy_effect_matches_catalog'",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert!(
        trigger_config
            .unwrap_or_default()
            .iter()
            .any(|entry| entry == "search_path=pg_catalog"),
        "the 0170 trigger function must pin search_path or every definer attach is 42P01"
    );
    let triggers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid \
         WHERE c.relname = 'ont_object_policies' AND NOT t.tgisinternal",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        triggers, 4,
        "a second CREATE TRIGGER would double-fire; REPLACE the function instead"
    );

    // THE WALL. The entire reason this migration adds a definer is that
    // `console_rt` has SELECT and no INSERT on the policy catalog
    // (`0150:117-118`). 0205 grants that INSERT to `console_ontology_writer`; if
    // it ever lands on `console_rt` instead, every other test in this file stays
    // green while the audited writer becomes optional.
    let fx = Fixture::build(&owner_pool, "policy-definer").await;
    let direct = fx
        .as_runtime_role(|tx| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO cedar_policy_catalog_entries \
                     (org_id, stable_key, title, natural_language_rule, effect, status, source, \
                      principal, action, resource, conditions, validation_status) \
                     VALUES ($1, 'wall.probe', 'Wall probe', 'wall probe', 'permit', 'draft', \
                      'no_code_draft', '{}'::jsonb, '{}'::jsonb, '{}'::jsonb, '[]'::jsonb, 'valid')",
                )
                .bind(OrgId::knl().as_uuid())
                .execute(&mut **tx)
                .await
                .err()
                .and_then(|error| {
                    error
                        .as_database_error()
                        .and_then(|db| db.code().map(|code| code.into_owned()))
                })
            })
        })
        .await;
    assert_eq!(
        direct.as_deref(),
        Some("42501"),
        "console_rt must still be denied a direct catalog INSERT"
    );

    // The org floor is unconditional and the definer may only narrow. Armed to a
    // different org than the row it writes, the INSERT must be refused by RLS.
    //
    // The payload is deliberately CANONICAL for `policyfloor`. Every in-definer
    // envelope check therefore passes and the INSERT is actually reached, so this
    // probe still proves what it says it proves: that the FLOOR, not a parameter
    // check, is what refuses a cross-org write. A malformed payload here would be
    // rejected by the envelope and the assertion below would silently stop
    // testing RLS at all.
    //
    // This is also why the definer must NOT filter its object-type lookup by
    // `p_org_id` and must NOT compare `p_org_id` to `app.current_org`: either one
    // pre-empts RLS on this exact path and makes the org floor untestable through
    // the only route that reaches it. RLS already scopes the lookup to the armed
    // org, so the extra check buys nothing and costs this proof.
    let type_id = fx
        .publish("policyfloor", instance_type_draft("policyfloor"))
        .await;
    let foreign_org = Uuid::from_u128(0x7777_7777_7777_7777_7777_7777_7777_7777);
    let refused = fx
        .as_command_role(move |tx| {
            Box::pin(async move {
                sqlx::query("SELECT set_config('app.current_org', $1, true)")
                    .bind(OrgId::knl().as_uuid().to_string())
                    .execute(&mut **tx)
                    .await
                    .unwrap();
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT ont_policy_api.attach_object_policy($1,$2,$3,$4,$5,$6,$7,$8)",
                )
                .bind(foreign_org)
                .bind(Uuid::new_v4())
                .bind(*type_id.as_uuid())
                .bind("permit")
                .bind(canonical_normalized_row("permit", "policyfloor", vec![]))
                .bind("ontology-runtime-filter-v1")
                .bind(PROBE_TRACE_ID)
                .bind(PROBE_SPAN_ID)
                .fetch_one(&mut **tx)
                .await
                .err()
                .map(|error| error.to_string())
            })
        })
        .await;
    let refused = refused.expect("a cross-org definer insert must not succeed");
    assert!(
        refused.contains("row-level security policy"),
        "the definer must be stopped by the RLS org floor, got: {refused}"
    );
    // NAMING THE TABLE, not just the mechanism. An audit-FIRST definer refuses
    // this same call with `... for table "audit_events"`, which satisfies the
    // table-agnostic assertion above while having moved the org-floor proof off
    // the policy catalog entirely. The audit INSERT must come LAST.
    assert!(
        refused.contains("cedar_policy_catalog_entries"),
        "the floor must refuse the POLICY CATALOG write; a refusal naming \
         audit_events instead means the audit row is written before the catalog \
         row and this probe no longer proves the catalog is org-scoped. Got: {refused}"
    );
}

// ---------------------------------------------------------------------------
// 5a. The definer is a security boundary in its own right, not only the route
// ---------------------------------------------------------------------------

/// `ont_policy_api.attach_object_policy` is EXECUTE-granted to a database login.
/// Anyone holding that credential can call it directly, skipping every check
/// `attach_object_policy` (`rest/src/lib.rs:500-557`) performs — a reviewer
/// minted an `enforced` catalog row carrying arbitrary generated Cedar text that
/// way. The route is therefore not the only boundary; this routine is one too,
/// and it stays one after 0206 moved EXECUTE to `console_ontology_cmd`: the
/// command credential is a database login the app holds, so a leak of it must not
/// buy a forged attachment either. Every forgery below is therefore probed as
/// `console_ontology_cmd`, the ONE role that can still reach the body.
///
/// Two shapes of fix, both asserted here:
///
///   * DELETION beats validation. `p_stable_key`, `p_title`,
///     `p_natural_language_rule` and `p_generated_policy_digest` are all derivable
///     from what is already supplied, so the definer generates them and drops the
///     parameters. `p_generated_policy_text` is dropped for the opposite reason:
///     it is derivable NOWHERE in SQL and boundable by no predicate, so the
///     definer stores NULL instead of a value nothing can re-check. You cannot
///     forge what you cannot supply, and there is no check left to get wrong.
///     Arity falls 11 -> 6.
///   * What CANNOT be deleted is bounded in the definer: the object type must
///     exist and be visible under the armed org, the normalized row's
///     `effect`/`action`/`resource_type` must agree with the attachment and the
///     type, and the condition list must respect the same 32 bound the route
///     applies (`rest/src/lib.rs:491`).
///
/// What deliberately stays route-only: the HTTP principal (`authorize_ontology`
/// has no SQL equivalent) and Cedar's strict validator verdict
/// (`validate_blocks_with` — re-implementing it in SQL is the divergence that
/// rots). Both are re-detected fail-closed on every read by
/// `load_enforced_object_policy_blocks` (`authz-rest/src/store.rs:569-586`);
/// remove that re-validation and this justification dies silently.
///
/// Every case runs as a genuine, non-superuser, non-BYPASSRLS login — both roles
/// asserted, not assumed — because a superuser or BYPASSRLS session makes each
/// refusal below vacuous.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn the_attach_definer_refuses_every_forgery_the_route_would_have_refused(owner_pool: PgPool) {
    let fx = Fixture::build(&owner_pool, "definer-hardening").await;
    let type_id = fx
        .publish("definerhard", instance_type_draft("definerhard"))
        .await;
    let type_uuid = *type_id.as_uuid();

    let role: (String, bool, bool) = fx
        .as_runtime_role(|tx| {
            Box::pin(async move {
                sqlx::query_as(
                    "SELECT current_user::TEXT, r.rolsuper, r.rolbypassrls \
                     FROM pg_roles r WHERE r.rolname = current_user",
                )
                .fetch_one(&mut **tx)
                .await
                .unwrap()
            })
        })
        .await;
    assert_eq!(
        role,
        ("console_rt".to_owned(), false, false),
        "every refusal below is vacuous unless the caller is the genuine, \
         non-superuser, non-BYPASSRLS runtime role"
    );

    // The definer probes below run as `console_ontology_cmd`, so its identity has
    // to be pinned too — a `SET ROLE` that silently failed would leave them
    // running as the sqlx superuser, where RLS does not apply and every envelope
    // refusal below could pass for the wrong reason.
    let command_role: (String, bool, bool) = fx
        .as_command_role(|tx| {
            Box::pin(async move {
                sqlx::query_as(
                    "SELECT current_user::TEXT, r.rolsuper, r.rolbypassrls \
                     FROM pg_roles r WHERE r.rolname = current_user",
                )
                .fetch_one(&mut **tx)
                .await
                .unwrap()
            })
        })
        .await;
    assert_eq!(
        command_role,
        ("console_ontology_cmd".to_owned(), false, false),
        "the command credential must be the genuine, non-superuser, \
         non-BYPASSRLS role or every definer probe below is vacuous"
    );

    // POSITIVE CONTROL, first. A hardened definer that refuses everything would
    // satisfy every negative case below while breaking the product.
    //
    // It also proves the DELETION half: the stored `stable_key` carries the
    // object type's own key and a fresh discriminator, `bundle_digest` is the
    // SHA-256 of the normalized row actually stored beside it, and no Cedar
    // source is stored at all. None of the three was supplied.
    // `forge_attach_persisted`, i.e. the SAME hand-crafted call as every forgery
    // below with the SAME real actor (`created_by` carries a composite FK to
    // `users(id, org_id)` — 0150:41 — so `Uuid::nil()` would assert only that
    // referential integrity works), but COMMITTING, because the row has to be
    // read back on a different connection.
    let accepted_id = fx
        .forge_attach_persisted(
            type_uuid,
            "permit",
            canonical_normalized_row("permit", "definerhard", vec![owner_is_subject()]),
        )
        .await
        .expect("a canonical attach must still be accepted");
    // Read back as `console_rt`, not as the command credential. The command role
    // is EXECUTE-only by construction (0165's shape: no table privileges at all,
    // pinned by the `command_audit_table_access` assertion in
    // `key_revision_migration_upgrade.rs`), so it cannot SELECT the catalog and
    // must not be granted the ability. `console_rt` holds SELECT and no INSERT
    // (0150:117-118), which makes this read the stronger statement anyway: a row
    // the runtime role can see and could never have written.
    let (accepted_key, digest_is_self_consistent, stored_text): (String, bool, Option<String>) = fx
        .as_runtime_role(move |tx| {
            Box::pin(async move {
                sqlx::query_as(
                    "SELECT stable_key, \
                            bundle_digest = 'sha256:' || encode(sha256(convert_to(normalized_row::text, 'UTF8')), 'hex'), \
                            generated_policy_text \
                     FROM cedar_policy_catalog_entries WHERE id = $1",
                )
                .bind(accepted_id)
                .fetch_one(&mut **tx)
                .await
                .unwrap()
            })
        })
        .await;
    assert!(
        accepted_key.starts_with("object_policy.definerhard."),
        "the definer must GENERATE the catalog stable_key from the object type it \
         resolved, not accept one: {accepted_key}"
    );
    assert!(
        digest_is_self_consistent,
        "bundle_digest must be derived from the row stored beside it; a supplied \
         digest is a stored false attestation nothing ever re-checks"
    );
    assert!(
        stored_text.is_none(),
        "the definer must not store Cedar source: it cannot derive it, cannot \
         bound it, and both readers of the column decide live authorizations \
         with it: {stored_text:?}"
    );

    // --- the forgeries -----------------------------------------------------
    // Each asserts its OWN refusal message. `is_err()` alone would pass today
    // against a definer that does not exist at this arity, which is exactly the
    // vacuous-green class this suite exists to avoid.

    let unknown_type = fx
        .forge_attach(
            Uuid::from_u128(0x0BAD_0BAD_0BAD_0BAD_0BAD_0BAD_0BAD_0BAD),
            "permit",
            canonical_normalized_row("permit", "definerhard", vec![]),
        )
        .await;
    assert_forged_attach_refused(
        &unknown_type,
        "unknown object type",
        "ont_object_policies.object_type_id has no FK (0154:29), so an unresolvable \
         or foreign object type must be refused by the definer's own RLS-scoped lookup",
    );

    let wrong_resource = fx
        .forge_attach(
            type_uuid,
            "permit",
            canonical_normalized_row("permit", "some.other.type", vec![]),
        )
        .await;
    assert_forged_attach_refused(
        &wrong_resource,
        "resource_type",
        "a row whose resource_type disagrees with the type it is attached to is \
         inert forever at HTTP 200 [] -- a silent failure no post-hoc test can see",
    );

    let wrong_action = fx
        .forge_attach(
            type_uuid,
            "permit",
            json!({
                "effect": "permit",
                "action": "edit",
                "resource_type": "definerhard",
                "conditions": []
            }),
        )
        .await;
    assert_forged_attach_refused(
        &wrong_action,
        "action",
        "the route pins the action to authoring::OBJECT_POLICY_ACTION; the definer \
         must too, or the attachment never matches applicable_object_policies",
    );

    let effect_disagreement = fx
        .forge_attach(
            type_uuid,
            "permit",
            canonical_normalized_row("forbid", "definerhard", vec![]),
        )
        .await;
    assert_forged_attach_refused(
        &effect_disagreement,
        "effect",
        "the 0170 trigger only compares catalog to attachment, and the definer \
         binds BOTH from p_effect, so a blocks/catalog disagreement passes it",
    );

    let too_many_conditions = fx
        .forge_attach(
            type_uuid,
            "permit",
            canonical_normalized_row(
                "permit",
                "definerhard",
                std::iter::repeat_with(owner_is_subject).take(33).collect(),
            ),
        )
        .await;
    assert_forged_attach_refused(
        &too_many_conditions,
        "conditions",
        "the route bounds this at 32 (rest/src/lib.rs:491) and both tables are \
         append-only, so an oversized list is permanent work charged to every read",
    );

    // `0154:105` granted `console_rt` INSERT on the attachment table outright, so
    // an attacker holding the runtime role could bind an existing enforced catalog
    // policy to any object type with one bare statement and the definer was not
    // needed at all. 0205's `REVOKE INSERT ON public.ont_object_policies FROM
    // console_rt` took it back; this asserts it stays taken back, because with it
    // every check in the definer is optional again.
    let bare_attachment = fx
        .as_runtime_role(move |tx| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO ont_object_policies (org_id, object_type_id, cedar_policy_id, effect) \
                     SELECT $1, $2, id, 'permit' FROM cedar_policy_catalog_entries LIMIT 1",
                )
                .bind(*OrgId::knl().as_uuid())
                .bind(type_uuid)
                .execute(&mut **tx)
                .await
                .err()
                .and_then(|error| {
                    error
                        .as_database_error()
                        .and_then(|db| db.code().map(|code| code.into_owned()))
                })
            })
        })
        .await;
    assert_eq!(
        bare_attachment.as_deref(),
        Some("42501"),
        "console_rt must hold no INSERT on ont_object_policies: with it, every \
         check in the definer is optional"
    );

    // The trap that makes all of the above green while none of it holds: the
    // signature change adds a NEW function and leaves the old 11-argument one
    // EXECUTE-granted beside it. The type list is repeated four times in 0205
    // (CREATE, ALTER OWNER, REVOKE, GRANT); missing one leaves the bypass open.
    //
    // `oidvectortypes`, not `pg_get_function_identity_arguments`: the latter
    // interpolates the PARAMETER NAMES, so this assertion would have been a
    // spelling contract on `p_org_id`/`p_created_by`/... rather than on the
    // overload set. The type list below is byte-identical to PostgreSQL's own
    // `function ... does not exist` rendering, which is what an attacker
    // probing for a surviving sibling would read.
    let executable: Vec<String> = sqlx::query_scalar(
        "SELECT oidvectortypes(p.proargtypes) \
         FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
         WHERE n.nspname = 'ont_policy_api' AND p.proname = 'attach_object_policy' \
           AND has_function_privilege('console_rt', p.oid, 'EXECUTE')",
    )
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    // THE ONE SANCTIONED INVERSION. This asserted, until 0206, that `console_rt`
    // could execute exactly one attach overload. That capability WAS the residual
    // 0205 escalated: the definer is not an audited boundary, so anything holding
    // runtime credentials attached a policy untraced. It is now zero overloads,
    // and the argument-list-repeated-four-times trap the old assertion guarded is
    // guarded instead by the ACL totality query below, which is total over the
    // schema rather than over one `proname`.
    assert_eq!(
        executable,
        Vec::<String>::new(),
        "console_rt must be able to execute NO attach overload; one surviving \
         overload is the whole 0206 topology bypassed with every test still green"
    );
}

// ---------------------------------------------------------------------------
// 5a-bis. 0206: the definer is reachable only by the audited command credential
// ---------------------------------------------------------------------------

/// The exact inversion of the exploit #525 proved by execution: a direct call to
/// the definer as `console_rt` persisted one enforced attachment with zero audit
/// rows. It is now refused at the PERMISSION layer, before the body runs.
///
/// BOTH the SQLSTATE and the message, because neither alone discriminates.
///
/// The SQLSTATE separates "never got in" from "got in and was stopped by the
/// body": `console_rt` reaching an envelope check raises `P0001` at
/// `exec_stmt_raise, pl_exec.c:3923` (measured), which is the exploit's
/// PRECONDITION, not its closure.
///
/// The message separates WHICH permission was missing, and an earlier version of
/// this comment argued AGAINST asserting it, on the grounds that `42501` /
/// `aclcheck_error` already proves the credential never got in. Executed against
/// a migrated database, that is false. Revoking `console_rt`'s USAGE on the
/// schema yields `42501: permission denied for schema ont_policy_api` from the
/// IDENTICAL internal raise site, `aclcheck_error, aclchk.c:2795` — the same code
/// and the same location as the function-level refusal this test exists to pin,
/// but a different topology from the one 0206 asserts, and one 0206 explicitly
/// declined. This probe stayed green against it until the message assertion below
/// existed.
///
/// Known ceiling: that substring also prefix-matches `attach_object_policy_rows`.
/// It cannot arise here, because the call names the 8-argument entrypoint by its
/// argument list; the sibling probe below carries the same ceiling.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn console_rt_cannot_execute_the_attach_definer_at_all(owner_pool: PgPool) {
    let fx = Fixture::build(&owner_pool, "rt-definer-revoked").await;
    let type_id = fx
        .publish("rtrevoked", instance_type_draft("rtrevoked"))
        .await;
    let type_uuid = *type_id.as_uuid();
    let org = *fx.org.as_uuid();
    let actor = *fx.actor.as_uuid();

    // A CANONICAL payload for a REAL, resolvable object type with a REAL actor.
    // Every envelope check inside the definer would pass, so nothing but the
    // grant topology can refuse this call. A malformed payload would be refused
    // by the body even with EXECUTE restored, and this test would pass against
    // the exploit it exists to close.
    let refusal: Option<(String, String)> = fx
        .as_runtime_role(move |tx| {
            Box::pin(async move {
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT ont_policy_api.attach_object_policy($1,$2,$3,$4,$5,$6,$7,$8)",
                )
                .bind(org)
                .bind(actor)
                .bind(type_uuid)
                .bind("permit")
                .bind(canonical_normalized_row(
                    "permit",
                    "rtrevoked",
                    vec![owner_is_subject()],
                ))
                .bind("ontology-runtime-filter-v1")
                .bind(PROBE_TRACE_ID)
                .bind(PROBE_SPAN_ID)
                .fetch_one(&mut **tx)
                .await
                .err()
                .map(refusal)
            })
        })
        .await;

    let (sqlstate, message) = refusal.expect(
        "console_rt executing the attach definer is #525's proven exploit: it \
         persisted an enforced attachment with ZERO audit rows. It must be refused.",
    );
    assert_eq!(
        sqlstate, "42501",
        "the refusal must come from the PERMISSION layer. P0001 means console_rt \
         reached the definer body and was stopped by an envelope check -- which is \
         the exploit's precondition, not its closure. Got {sqlstate}: {message}"
    );
    assert!(
        message.contains("permission denied for function attach_object_policy"),
        "the refusal must name the FUNCTION. `permission denied for schema \
         ont_policy_api` is ALSO 42501 and is raised from the same internal site \
         (`aclcheck_error, aclchk.c:2795`, measured both ways), so the code alone \
         reads green when console_rt has merely lost schema USAGE -- a topology \
         0206 declined, not the one it asserts. Got: {message}"
    );
}

/// The other half of the same topology: the credential that CAN attach must not be
/// able to skip the audit row.
///
/// 0206 renames 0205's body to `attach_object_policy_rows` and wraps it in an
/// 8-argument entrypoint that appends the audit row. That split is only worth
/// anything if the inner, unaudited routine stays owner-only — otherwise the one
/// credential that can attach at all can still attach untraced, which is the
/// residual this slice retires, moved one function to the left.
///
/// Its own test, not a tail assertion on the one above: behind a failing assert it
/// would never execute, and an unexecuted probe is not evidence.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn the_command_credential_cannot_skip_the_audit_row(owner_pool: PgPool) {
    let fx = Fixture::build(&owner_pool, "cmd-cannot-skip-audit").await;
    let type_id = fx
        .publish("skipaudit", instance_type_draft("skipaudit"))
        .await;
    let type_uuid = *type_id.as_uuid();
    let org = *fx.org.as_uuid();
    let actor = *fx.actor.as_uuid();

    let rows_refusal: Option<(String, String)> = fx
        .as_command_role(move |tx| {
            Box::pin(async move {
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT ont_policy_api.attach_object_policy_rows($1,$2,$3,$4,$5,$6)",
                )
                .bind(org)
                .bind(actor)
                .bind(type_uuid)
                .bind("permit")
                .bind(canonical_normalized_row(
                    "permit",
                    "skipaudit",
                    vec![owner_is_subject()],
                ))
                .bind("ontology-runtime-filter-v1")
                .fetch_one(&mut **tx)
                .await
                .err()
                .map(refusal)
            })
        })
        .await;
    let (rows_sqlstate, rows_message) = rows_refusal.expect(
        "console_ontology_cmd must reach the AUDITED entrypoint only. EXECUTE on \
         the inner row-writer would make the audit row skippable by the one \
         credential that can attach at all",
    );
    assert_eq!(rows_sqlstate, "42501", "got {rows_message}");
    // The MESSAGE, not just the SQLSTATE. Missing schema USAGE also raises 42501
    // (`permission denied for schema ont_policy_api`), so a bare code assertion
    // here passes today — before 0206 exists and before the routine it names has
    // even been created — which is the vacuous green this suite exists to refuse.
    assert!(
        rows_message.contains("function attach_object_policy_rows"),
        "the refusal must name the ROW-WRITER FUNCTION. A schema-level 42501 means \
         the command role lacks USAGE and this probe proved nothing about the \
         audit row being unskippable. Got: {rows_message}"
    );
}

/// The grant topology asserted against `pg_catalog`, TOTAL over the schema.
///
/// Per-role `has_function_privilege` answers only the question you thought to
/// ask: it cannot see a third grantee, and it reads `true` for a role holding
/// EXECUTE with no schema USAGE — under which every real call fails 42501 naming
/// the SCHEMA while the probe stays green. `aclexplode(proacl)` over the whole
/// schema plus an explicit `has_schema_privilege` pair closes both.
///
/// `COALESCE(proacl, acldefault(...))` closes a third, which `aclexplode(proacl)`
/// alone did not: a routine left at DEFAULT privileges has `proacl IS NULL`, which
/// means EXECUTE to PUBLIC — and `aclexplode(NULL)` returns ZERO rows, so the
/// vector below matched unchanged with two PUBLIC-executable routines added to the
/// schema (measured: `has_function_privilege('console_rt', ...)` read `t` for both
/// while this assertion stayed green). `acldefault` materialises the grant
/// PostgreSQL implies, so the implied grant is measured like any other. A
/// PROCEDURE is the same hole with an extra edge: `REVOKE ALL ON ALL FUNCTIONS IN
/// SCHEMA` does not cover `prokind = 'p'` at all.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn the_attach_schema_grants_exactly_the_audited_command_credential(owner_pool: PgPool) {
    let acl: Vec<String> = sqlx::query_scalar(
        "SELECT p.proname || '|' || a.grantee::regrole::text || '|' || a.privilege_type \
         FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace, \
              aclexplode(COALESCE(p.proacl, pg_catalog.acldefault('f', p.proowner))) a \
         WHERE n.nspname = 'ont_policy_api' \
         ORDER BY p.proname, a.grantee::regrole::text, a.privilege_type",
    )
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        acl,
        vec![
            "attach_object_policy|console_ontology_cmd|EXECUTE".to_owned(),
            "attach_object_policy|console_ontology_writer|EXECUTE".to_owned(),
            "attach_object_policy_rows|console_ontology_writer|EXECUTE".to_owned(),
        ],
        "the whole ont_policy_api ACL, so a fourth grantee, a surviving overload, \
         or a routine left at default privileges cannot hide behind a per-role \
         probe. console_rt must appear nowhere, and only the OWNER may call the \
         unaudited row-writer.\n\
         \n\
         A LATER MIGRATION ADDING ANY ROUTINE TO ont_policy_api FAILS HERE BY \
         DESIGN, including a PROCEDURE, which `REVOKE ALL ON ALL FUNCTIONS IN \
         SCHEMA` does not reach. The correct repair is to REVOKE the implied \
         PUBLIC grant on the new routine and add the exact row it should have. \
         Widening this vector to admit `-|EXECUTE` re-opens the hole the COALESCE \
         exists to close, and `ALTER DEFAULT PRIVILEGES` does not close it either \
         (measured twice on PG 18.4: pg_default_acl records nothing and the next \
         routine still lands with proacl NULL)."
    );

    let schema_usage: (bool, bool) = sqlx::query_as(
        "SELECT has_schema_privilege('console_ontology_cmd', 'ont_policy_api', 'USAGE'), \
                has_schema_privilege('console_rt', 'ont_policy_api', 'USAGE')",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert!(
        schema_usage.0,
        "0205's `GRANT USAGE ON SCHEMA ont_policy_api TO console_rt` grants schema \
         USAGE to that role only. Without USAGE for the command role, \
         has_function_privilege above still reads true while every attach fails \
         42501 naming the SCHEMA"
    );
    assert!(
        schema_usage.1,
        "console_rt keeps schema USAGE deliberately: it is what makes its refusal \
         read `permission denied for function attach_object_policy` instead of a \
         schema-level message, and USAGE alone grants nothing executable"
    );
}

/// A `PgCedarPolicyStore` with NO command pool must REFUSE to attach, typed, and
/// never fall back to its read pool.
///
/// This is the test that makes the other three trustworthy. The fallback that
/// breaks this slice is one expression — `self.command_pool.as_ref()
/// .unwrap_or(&self.pool)` — it compiles, it reads as defensive, and it restores
/// the exact capability 0206 removes. Every other test in this file wires both
/// pools, so every one of them stays green while the attach path runs on
/// `console_rt` again. Nothing but an unwired store can see the difference.
///
/// `command_pool()?` must short-circuit AFTER `validate_blocks_with` and BEFORE
/// any connection is taken, so the blocks below are deliberately VALID: an
/// invalid set would be refused by the validator and this test would pass without
/// the command-pool check existing at all.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_store_with_no_command_pool_refuses_to_attach_rather_than_using_the_read_pool(
    owner_pool: PgPool,
) {
    let fx = Fixture::build(&owner_pool, "no-cmd-pool").await;
    let type_id = fx
        .publish("nocmdpool", instance_type_draft("nocmdpool"))
        .await;

    // Built exactly like `app/src`'s policy-studio store and every draft-only
    // test fixture: `new(pool)` and nothing else.
    let store = console_platform_authz_rest::PgCedarPolicyStore::new(fx.runtime_pool.clone());
    let outcome = scope_org(fx.org, async {
        store
            .attach_object_policy(console_platform_authz_rest::AttachObjectPolicyCommand {
                actor: fx.actor,
                object_type_id: *type_id.as_uuid(),
                blocks: NoCodeBlocks {
                    effect: Effect::Permit,
                    action: "view".to_owned(),
                    resource_type: "nocmdpool".to_owned(),
                    conditions: vec![],
                },
                declared: vec![],
            })
            .await
    })
    .await;

    match outcome {
        Err(console_platform_authz_rest::PgCedarError::CommandUnavailable) => {}
        Ok(id) => panic!(
            "an unwired store ATTACHED policy {id}: the attach ran on the read \
             pool, which is the capability 0206 exists to remove"
        ),
        Err(other) => panic!(
            "an unwired store must fail with the typed CommandUnavailable, not \
             an opaque database error a deployment fault is indistinguishable \
             from: {other}"
        ),
    }

    // Nothing was written. A store that refused AFTER the definer call would
    // satisfy the match above and still have minted the rows.
    let written: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM ont_object_policies WHERE org_id = $1), \
                (SELECT COUNT(*) FROM audit_events WHERE org_id = $1 \
                   AND action = 'ontology.object_policy.attach')",
    )
    .bind(*fx.org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        written,
        (0, 0),
        "the refusal must precede every write: no attachment and no audit claim \
         that one happened"
    );
}

/// The credential the application now holds on the attach path must reach the
/// definer's tables ONLY through the definer.
///
/// EXECUTE-only is the entire premise of 0206: `console_ontology_cmd` cannot skip
/// the audit row because it cannot reach `attach_object_policy_rows`, and it
/// cannot forge one because it cannot INSERT into `audit_events`. Both claims are
/// stated in 0206's header and neither was asserted anywhere — the `console_rt`
/// twin of the probe below exists in
/// `the_attach_definer_refuses_every_forgery_the_route_would_have_refused`, but it
/// was never re-pointed at the role that inherited the capability.
///
/// A blanket `GRANT ... ON ALL TABLES IN SCHEMA public TO console_ontology_cmd` in
/// some later migration is all it takes: the audited definer becomes optional for
/// the one credential the attach route holds, and every other test in this file
/// stays green because they all attach through the route, which would keep
/// working.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn the_command_credential_holds_no_direct_write_on_the_tables_the_definer_writes(
    owner_pool: PgPool,
) {
    let fx = Fixture::build(&owner_pool, "cmd-no-table-write").await;
    fx.publish("cmdnowrite", instance_type_draft("cmdnowrite"))
        .await;
    let victim = fx
        .publish("cmdnowriteb", instance_type_draft("cmdnowriteb"))
        .await;
    let victim_uuid = *victim.as_uuid();
    let org = *fx.org.as_uuid();

    // A REAL enforced catalog row, authored through the audited route, so the bare
    // statement below is the genuine attack and not an FK violation wearing its
    // clothes: it binds an existing permit to a SECOND object type that never had
    // one, with no definer, no envelope checks and no audit row.
    let authored = fx
        .post(
            &policies_path("cmdnowrite"),
            json!({ "effect": "permit", "conditions": [owner_is_subject()] }),
        )
        .await;
    assert_eq!(authored.status, StatusCode::CREATED, "{:?}", authored.body);
    let policy_id: Uuid = authored.body["id"].as_str().unwrap().parse().unwrap();

    // EXECUTED, not inferred. `0154:105` granted `console_rt` this exact INSERT and
    // 0205's `REVOKE INSERT ON public.ont_object_policies FROM console_rt` took it
    // back; nothing ever asked the same question of the command role.
    //
    // `VALUES` and not `INSERT ... SELECT FROM cedar_policy_catalog_entries`, which
    // is how the `console_rt` twin of this probe is written and is WRONG here: the
    // command role holds no SELECT on the catalog either, so that form is refused
    // 42501 for the read and never reaches the INSERT privilege check at all. It
    // was written that way first and stayed green with `GRANT INSERT ON
    // ont_object_policies TO console_ontology_cmd` applied.
    let bare_attachment = fx
        .as_command_role(move |tx| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO ont_object_policies (org_id, object_type_id, cedar_policy_id, effect) \
                     VALUES ($1, $2, $3, 'permit')",
                )
                .bind(org)
                .bind(victim_uuid)
                .bind(policy_id)
                .execute(&mut **tx)
                .await
                .err()
                .map(refusal)
            })
        })
        .await;
    let (bare_sqlstate, bare_message) = bare_attachment.expect(
        "console_ontology_cmd must hold no INSERT on ont_object_policies: with it, \
         the audited definer is optional for the ONE credential the attach route \
         holds, which is the whole of 0206 undone",
    );
    assert_eq!(bare_sqlstate, "42501", "got {bare_message}");
    // THIS table's missing GRANT, not any other 42501. Measured with `GRANT INSERT
    // ON ont_object_policies TO console_ontology_cmd` applied, the same statement
    // raises `42501: permission denied for table cedar_policy_catalog_entries`,
    // from `enforce_ont_object_policy_effect_matches_catalog()` -- a BEFORE INSERT
    // ROW trigger with `prosecdef = false`, so it reads the catalog as the caller
    // and fires BEFORE RLS is evaluated. RLS is unreachable for THIS role; the
    // tenancy floor is not what refuses it. Same SQLSTATE, same internal raise site
    // (`aclcheck_error, aclchk.c:2795`), a different table -- so the code alone
    // reads green against exactly the grant this test exists to forbid.
    assert!(
        bare_message.contains("permission denied for table \"ont_object_policies\"")
            || bare_message.contains("permission denied for table ont_object_policies"),
        "the refusal must name ont_object_policies, the table whose INSERT is being \
         forbidden. A 42501 naming any other table means the command role got past \
         the write check on this one and was stopped somewhere downstream, which \
         proves nothing about the grant. Got: {bare_message}"
    );

    // TOTAL over the three tables the definer writes, so a privilege nobody
    // thought to execute cannot hide. Absence of the grant is a complete proof of
    // refusal here (the reverse direction — a grant present with no schema USAGE —
    // is what makes catalog probes unsafe, and does not apply to tables in
    // `public`, whose USAGE `console_ontology_cmd` holds).
    //
    // Reads are deliberately NOT asserted: SELECT buys no forgery, and pinning it
    // would fail a future migration that legitimately let the command role read.
    // The claim being pinned is 0206's, which is about writes.
    let writable: Vec<String> = sqlx::query_scalar(
        "SELECT t || ':' || p \
         FROM unnest(ARRAY['cedar_policy_catalog_entries','ont_object_policies','audit_events']) t, \
              unnest(ARRAY['INSERT','UPDATE','DELETE']) p \
         WHERE has_table_privilege('console_ontology_cmd', t, p) \
         ORDER BY t, p",
    )
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        writable,
        Vec::<String>::new(),
        "console_ontology_cmd must be EXECUTE-only. An INSERT on audit_events in \
         particular is a forgery channel 0165's protected_audit_writer_guard() \
         explicitly reasons about, in the ELSIF branch that raises \
         ontology_audit.command_required, and an INSERT on either policy table \
         bypasses the definer entirely"
    );
}

/// The audit row is written BY THE DEFINER, not by the application.
///
/// This is the difference between 0205 and 0206, and no test could see it: the
/// route-level audit assertions in
/// `an_attached_permit_is_the_only_thing_that_makes_instances_visible` stay green
/// whichever side writes the row, so restoring the application-side `with_audit`
/// and dropping the INSERT from the definer would pass the whole suite while
/// handing the audit row back to a caller who can choose not to write it. That
/// regression is precisely the residual 0205 escalated.
///
/// Nothing here goes over HTTP. The only thing that runs is one hand-crafted
/// `console_ontology_cmd` call to the definer, so the application is not in the
/// picture and the row can only have come from inside the routine.
///
/// It also pins 0206's two NEW parameters by VALUE. The route generates its own
/// pair, so the route-level assertion can only measure their LENGTH (32/16) — and
/// a definer that ignored arguments 7 and 8 and wrote its own constants would
/// satisfy that.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn the_definer_writes_the_audit_row_itself_carrying_the_supplied_trace(owner_pool: PgPool) {
    let fx = Fixture::build(&owner_pool, "definer-writes-audit").await;
    let type_id = fx
        .publish("defineraudit", instance_type_draft("defineraudit"))
        .await;
    let normalized_row =
        canonical_normalized_row("permit", "defineraudit", vec![owner_is_subject()]);

    let policy_id = fx
        .forge_attach_persisted(*type_id.as_uuid(), "permit", normalized_row.clone())
        .await
        .expect("a canonical attach through the audited entrypoint must be accepted");

    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT actor::TEXT || '|' || target_type || '|' || target_id || '|' \
                || org_id::TEXT || '|' || (before_snap IS NULL)::TEXT || '|' \
                || (after_snap = $2)::TEXT || '|' || trim(trace_id) || '|' || trim(span_id) \
         FROM audit_events \
         WHERE org_id = $1 AND action = 'ontology.object_policy.attach'",
    )
    .bind(*fx.org.as_uuid())
    .bind(&normalized_row)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        audited,
        vec![format!(
            "{}|ont_object_policies|{}|{}|true|true|{PROBE_TRACE_ID}|{PROBE_SPAN_ID}",
            fx.actor.as_uuid(),
            type_id.as_uuid(),
            fx.org.as_uuid()
        )],
        "one audit row, written inside the definer, for a call no application code \
         took part in. `fetch_all` and not `fetch_one`: a definer that wrote the \
         row twice is as wrong as one that wrote it never, and `fetch_one` would \
         report the first of two as success"
    );

    // The row it audits really exists, so the assertion above cannot be satisfied
    // by an audit claim about an attach that did not happen.
    let attached: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ont_object_policies \
         WHERE org_id = $1 AND object_type_id = $2 AND cedar_policy_id = $3",
    )
    .bind(*fx.org.as_uuid())
    .bind(type_id.as_uuid())
    .bind(policy_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        attached, 1,
        "the audited attachment must be the one on disk"
    );
}

/// An audit row that cannot be written must take the policy rows down with it.
///
/// "an attach and its audit claim commit or roll back together" is asserted in
/// prose three times — 0206's header, 0205's §4 header after this lane rewrote it,
/// and the ledger — and was executed nowhere. Every other probe in this file sees
/// the two together only when BOTH succeed, which is the one case that cannot
/// distinguish an atomic pair from an audit INSERT whose failure is swallowed.
///
/// The mutation that survives the entire suite is one plausible defensive edit to
/// 0206 §4 — wrap the audit INSERT in `BEGIN ... EXCEPTION WHEN OTHERS THEN NULL;
/// END` so an attach never fails on audit trouble — and it re-creates exactly the
/// residual this slice retires: a policy row that no audit event describes.
/// Measured with that mutation applied: the call below returns Ok — the definer
/// accepted an attach it could not audit — and this test is the ONLY one of the 27
/// in this file that fails. The other 26 never make the audit INSERT fail, so none
/// of them can tell a swallowed failure from an atomic pair.
///
/// The transaction is COMMITTED, not rolled back like `as_command_role` does: on a
/// rollback the counts are zero whatever the definer did, which is the vacuous
/// green this file exists to refuse. A NULL `p_trace_id` is the forcing input
/// because `audit_events.trace_id` is `CHAR(32) NOT NULL` (`0003:16`) and nothing
/// in the row-writer reads arguments 7 or 8 — so the policy INSERTs have already
/// happened when the audit INSERT raises, which is the ordering the claim is about.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_unwritable_audit_row_rolls_the_policy_rows_back_with_it(owner_pool: PgPool) {
    let fx = Fixture::build(&owner_pool, "audit-atomic").await;
    let type_id = fx
        .publish("auditatomic", instance_type_draft("auditatomic"))
        .await;

    // Inline, and neither `forge_attach_persisted` nor `armed_tx`: the NULL trace
    // below is the forcing input, and this probe's refusal must depend on nothing
    // but the statement written here. Measured: with `armed_tx` mutated to stop
    // arming the org, five probes in this file go red and this one stays GREEN on a
    // refusal it does not assert (`unknown object type`), because zero rows is also
    // what it expects. Five lines of duplication buys immunity to that.
    let mut tx = fx.command_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(fx.org.as_uuid().to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let outcome = sqlx::query_scalar::<_, Uuid>(
        "SELECT ont_policy_api.attach_object_policy($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(*fx.org.as_uuid())
    .bind(*fx.actor.as_uuid())
    .bind(*type_id.as_uuid())
    .bind("permit")
    .bind(canonical_normalized_row(
        "permit",
        "auditatomic",
        vec![owner_is_subject()],
    ))
    .bind("ontology-runtime-filter-v1")
    .bind(Option::<String>::None)
    .bind(PROBE_SPAN_ID)
    .fetch_one(&mut *tx)
    .await;
    let error = outcome.err().map(|error| error.to_string()).expect(
        "an attach whose audit row cannot be written must be REFUSED. Accepting \
             it is the retired residual restored: a policy attached with no audit \
             event naming who attached it",
    );
    // A COMMIT of an aborted transaction rolls back at the server, so this is a
    // no-op on the correct implementation and the persisting write on a mutated
    // one. Committing anyway is what makes the counts below evidence.
    let _ = tx.commit().await;

    let written: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM ont_object_policies \
                   WHERE org_id = $1 AND object_type_id = $2), \
                (SELECT COUNT(*) FROM audit_events WHERE org_id = $1 \
                   AND action = 'ontology.object_policy.attach')",
    )
    .bind(*fx.org.as_uuid())
    .bind(type_id.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        written,
        (0, 0),
        "the policy rows the row-writer already INSERTed must be gone: the audit \
         INSERT is the LAST statement in the definer, so this is the only ordering \
         where a swallowed audit failure leaves an untraced attachment. Refusal \
         was: {error}"
    );
}

/// The HTTP surface of the fail-closed path: an ontology registry with no command
/// pool must make the attach route 503, never 500 and never an attach.
///
/// `a_store_with_no_command_pool_refuses_to_attach_rather_than_using_the_read_pool`
/// pins the typed store error. This pins the two things above it that were
/// measured unexecuted by the whole suite: the
/// `PgCedarError::CommandUnavailable => SERVICE_UNAVAILABLE` arm of
/// `RestError::from_cedar` (`ontology/rest/src/lib.rs`), and the `None` arm of the
/// derivation in `OntologyRestState::new` — every other construction site in the
/// tests and in `backend/app/src` passes `.with_command_pool(...)`, so nothing
/// exercised the case where the registry has none.
///
/// The derivation is what makes the deployment coherent: the policy store's
/// command credential is taken from the registry rather than passed in, so a
/// composition root that configured no ontology command database gets a policy
/// store that fails closed too, instead of one silently attaching as `console_rt`.
///
/// 503 and not 500 because a missing command database is a deployment fault, and a
/// 500 is indistinguishable from the database being broken — which is the one
/// alarm that must not be ambiguous while an operator is deciding whether to roll
/// back.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn the_attach_route_is_service_unavailable_when_the_registry_has_no_command_pool(
    owner_pool: PgPool,
) {
    let fx = Fixture::build(&owner_pool, "route-no-cmd-pool").await;
    // Published through the WIRED fixture: a registry with no command pool cannot
    // publish either, and this test is about attach, not about publish.
    let type_id = fx
        .publish("routenocmd", instance_type_draft("routenocmd"))
        .await;

    // The same runtime pool, the same verifier, one difference: no command pool.
    // This is `backend/app/src/lib.rs`'s `DatabaseDependency::NotConfigured` arm.
    let auth = test_auth(fx.actor, fx.org);
    let unwired = router(OntologyRestState::new(
        PgOntologyStore::new(fx.runtime_pool.clone()),
        PgInstanceStore::new(fx.runtime_pool.clone()),
        PgGovernanceStore::new(fx.runtime_pool.clone()),
        Some(auth.verifier),
    ));
    let request = Request::builder()
        .method("POST")
        .uri(policies_path("routenocmd"))
        .header(header::AUTHORIZATION, format!("Bearer {}", auth.token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({ "effect": "permit", "conditions": [owner_is_subject()] }))
                .unwrap(),
        ))
        .unwrap();
    let response = unwired.oneshot(request).await.unwrap();
    let status = response.status();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
            .unwrap_or(Value::Null);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "an unwired registry must make attach 503, not 500 and not 201: {body:?}"
    );
    assert_eq!(
        body["error"]["code"], "ontology_command_unavailable",
        "the code is the machine-readable half a deployment alarm keys on. It is \
         deliberately the SAME code the registry's own CommandUnavailable maps to \
         (`RestError::from_ontology`): both mean one thing an operator acts on \
         identically -- ONTOLOGY_COMMAND_DATABASE_URL is missing: {body:?}"
    );

    // The refusal reached the store, not the router's own guards: a 503 produced by
    // some earlier check would satisfy the assertions above while the object type
    // was never even resolved.
    let resolved = fx.get("/api/v1/ontology/object-types/routenocmd").await;
    assert_eq!(
        resolved.status,
        StatusCode::OK,
        "the type the unwired route refused to attach to must be resolvable, or the \
         503 above proves nothing about the command pool: {:?}",
        resolved.body
    );
    assert_eq!(resolved.body["object_type"]["id"], json!(type_id.as_uuid()));

    let written: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM ont_object_policies WHERE org_id = $1), \
                (SELECT COUNT(*) FROM audit_events WHERE org_id = $1 \
                   AND action = 'ontology.object_policy.attach')",
    )
    .bind(*fx.org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        written,
        (0, 0),
        "a 503 must mean nothing happened: no attachment, and no audit row claiming \
         one did"
    );
}

/// The catalog row an attached object policy produces must never join the
/// org-wide enforced set behind `/policy/authorize`.
///
/// `p_generated_policy_text` is the one parameter the definer could neither
/// derive nor validate — re-rendering Cedar in SQL would be a second copy of
/// `generate_cedar_text_with` living in a migration, and bounding the text with
/// LIKE predicates does not bound it (a condition literal may legitimately
/// contain `;`). It is therefore no longer accepted at all: the definer stores
/// NULL, and `load_enforced_policies` (`authz-rest/src/store.rs:609`) filters
/// `generated_policy_text IS NOT NULL` before its `NOT EXISTS` clause is even
/// consulted. Both are asserted here, because the exclusion must survive
/// whichever of the two is reached first.
///
/// The type-scoped half of the same reach has its own probe below; excluding
/// only the org-wide set left it open.
///
/// No object-policy test can ever catch this: the object-policy read path never
/// reads `generated_policy_text`. It needs this probe or it needs none.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_attached_object_policy_never_joins_the_org_wide_enforced_set(owner_pool: PgPool) {
    let fx = Fixture::build(&owner_pool, "orgwide-narrowing").await;
    fx.publish("orgwide", instance_type_draft("orgwide")).await;

    // The row an attacker also reaches by calling the definer directly, authored
    // here through the real audited route so the probe is about the LOADER and
    // not about how the row was minted.
    let attached = fx
        .post(
            &policies_path("orgwide"),
            json!({ "effect": "permit", "conditions": [] }),
        )
        .await;
    assert_eq!(attached.status, StatusCode::CREATED, "{:?}", attached.body);
    let attached_id = attached.body["id"].as_str().unwrap().to_owned();

    // POSITIVE CONTROL: an enforced catalog policy with NO attachment is the
    // org-wide set's actual population and must survive the narrowing. Without
    // this, `load_enforced_policies` returning nothing at all would pass.
    let free_standing: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO cedar_policy_catalog_entries
            (org_id, stable_key, title, natural_language_rule, effect, status, source,
             principal, action, resource, conditions, policy_version, schema_version,
             bundle_digest, validation_status, normalized_row, generated_policy_text)
        VALUES ($1, 'orgwide.free_standing', 'Free standing', 'org-wide rule', 'permit',
                'enforced', 'no_code_draft', '{}'::jsonb, '{}'::jsonb, '{}'::jsonb,
                '[]'::jsonb, 1, 'ontology-runtime-filter-v1',
                'sha256:0000000000000000000000000000000000000000000000000000000000000000',
                'valid', $2, 'permit(principal, action, resource);')
        RETURNING id
        "#,
    )
    .bind(*fx.org.as_uuid())
    .bind(canonical_normalized_row("permit", "orgwide", vec![]))
    // rls-arming: fixture seeds the protected catalog as DB owner before the
    // genuine console_rt pool makes the loader's decision.
    .fetch_one(&owner_pool)
    .await
    .unwrap();

    let loaded = scope_org(fx.org, async {
        console_platform_authz_rest::PgCedarPolicyStore::new(fx.runtime_pool.clone())
            .load_enforced_policies()
            .await
    })
    .await
    .expect("the org-wide enforced set must load as console_rt");
    let ids = loaded.iter().map(|p| p.id.as_str()).collect::<Vec<_>>();

    assert!(
        ids.contains(&free_standing.to_string().as_str()),
        "an unattached enforced policy is the org-wide set: narrowing it away \
         would break /policy/authorize instead of hardening it, got {ids:?}"
    );
    assert!(
        !ids.contains(&attached_id.as_str()),
        "an object-policy attachment carries attacker-suppliable Cedar source \
         into every /policy/authorize, /policy/authorize/bulk and /policy/simulate \
         decision for the whole org; it must not reach the org-wide set, got {ids:?}"
    );
}

/// The org-wide exclusion above was only HALF the reach.
///
/// `load_enforced_policies` excludes attached rows, but the SAME endpoint —
/// `POST /policy/authorize` — takes an `object_type_id` and routes to
/// `authorize_object_row` (`authz-rest/src/lib.rs:354-359`), which reads
/// `OBJECT_POLICY_SELECT` (`authz-rest/src/store.rs:803-809`): the stored
/// `generated_policy_text` of every policy attached to that type, fed straight
/// into `authoring::simulate`. So the fix's own justification ("remove its
/// REACH") was not achieved by excluding the org-wide set alone — a definer call
/// could still choose the Cedar source that decides a live type-scoped
/// authorization.
///
/// The answer is the one `0205` already states for its four other parameters:
/// DELETION over validation. `p_generated_policy_text` is gone; the definer
/// stores NULL, and BOTH consumers already filter `generated_policy_text IS NOT
/// NULL`, so neither can be steered. The ontology residual — the only thing that
/// actually hides rows — never read the column at all.
///
/// This probe was RED before that change: `permit(principal, action ==
/// Action::"view", resource)` minted through the definer made `authorize_object_row`
/// return `Allow` for a principal that owns nothing.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_definer_minted_row_cannot_choose_the_cedar_that_decides_policy_authorize(
    owner_pool: PgPool,
) {
    let fx = Fixture::build(&owner_pool, "orgwide-typescoped").await;
    let type_id = fx
        .publish("typescoped", instance_type_draft("typescoped"))
        .await;

    // A hand-crafted definer call carrying permit-everything Cedar, exactly the
    // capability `console_ontology_cmd` holds after 0206 — a leak of that
    // credential still buys this, which is why the definer stays a boundary in its
    // own right. The normalized row is canonical, so every in-definer envelope
    // check passes and the row really is written.
    let forged = fx
        .forge_attach_persisted(
            *type_id.as_uuid(),
            "permit",
            canonical_normalized_row("permit", "typescoped", vec![owner_is_subject()]),
        )
        .await
        .expect("a canonical attach must still be accepted");

    // POSITIVE CONTROL: a hand-seeded attachment whose text was NOT minted
    // through the definer still decides its type. Without this, an
    // `authorize_object_row` that returned Deny unconditionally would pass.
    let honest_type = Uuid::new_v4();
    let honest: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO cedar_policy_catalog_entries
            (org_id, stable_key, title, natural_language_rule, effect, status, source,
             principal, action, resource, conditions, policy_version, schema_version,
             bundle_digest, validation_status, normalized_row, generated_policy_text)
        VALUES ($1, 'typescoped.honest', 'Honest', 'org-wide rule', 'permit',
                'enforced', 'no_code_draft', '{}'::jsonb, '{}'::jsonb, '{}'::jsonb,
                '[]'::jsonb, 1, 'ontology-runtime-filter-v1',
                'sha256:0000000000000000000000000000000000000000000000000000000000000000',
                'valid', $2, $3)
        RETURNING id
        "#,
    )
    .bind(*fx.org.as_uuid())
    .bind(canonical_normalized_row("permit", "typescoped", vec![]))
    .bind(ARBITRARY_POLICY_TEXT)
    // rls-arming: fixture seeds the protected catalog as DB owner before the
    // genuine console_rt pool makes the loader's decision.
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ont_object_policies (org_id, object_type_id, cedar_policy_id, effect) \
         VALUES ($1, $2, $3, 'permit')",
    )
    .bind(*fx.org.as_uuid())
    .bind(honest_type)
    .bind(honest)
    // rls-arming: same fixture seeding as above.
    .execute(&owner_pool)
    .await
    .unwrap();

    let store = console_platform_authz_rest::PgCedarPolicyStore::new(fx.runtime_pool.clone());
    let request = |resource_type: &str| SimRequest {
        subject: SimSubject {
            org: fx.org,
            // Owns nothing: only the permit-everything text can allow this.
            user_id: "nobody".to_owned(),
            roles: vec![],
            clearance_keys: vec![],
        },
        action: "view".to_owned(),
        resource: SimResource {
            org: fx.org,
            resource_type: resource_type.to_owned(),
            resource_id: Some("row-1".to_owned()),
            owner: Some("someone-else".to_owned()),
            branch: None,
            legal_hold: None,
        },
        purpose: None,
        field: None,
    };

    let control = scope_org(fx.org, async {
        store
            .authorize_object_row(honest_type, &request("typescoped"))
            .await
    })
    .await
    .expect("the type-scoped decision must load as console_rt");
    assert_eq!(
        control.effect,
        SimEffect::Allow,
        "a catalog row that carries real Cedar text must still decide its own \
         type, or the assertion below proves only that the endpoint is broken"
    );
    assert_eq!(control.determining_policies, vec![honest.to_string()]);

    let steered = scope_org(fx.org, async {
        store
            .authorize_object_row(*type_id.as_uuid(), &request("typescoped"))
            .await
    })
    .await
    .expect("the type-scoped decision must load as console_rt");
    assert_eq!(
        steered.effect,
        SimEffect::Deny,
        "a definer-minted row supplied the Cedar source that decided \
         POST /policy/authorize for its object type: {steered:?}"
    );
    assert!(
        steered.determining_policies.is_empty(),
        "no definer-minted policy may be a determining policy: {:?}",
        steered.determining_policies
    );

    // The column itself, so the assertion above cannot pass for the wrong reason
    // (an empty attachment join would also read Deny).
    let (text, digest_over_normalized_row): (Option<String>, bool) = sqlx::query_as(
        "SELECT generated_policy_text, \
                bundle_digest = 'sha256:' || encode(sha256(convert_to(normalized_row::text, 'UTF8')), 'hex') \
         FROM cedar_policy_catalog_entries WHERE id = $1",
    )
    .bind(forged)
    // rls-arming: ok, reads back the row the command-role definer call just wrote.
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert!(
        text.is_none(),
        "the definer must store no Cedar source at all; a stored one is a value \
         nothing can re-derive and nothing re-checks: {text:?}"
    );
    assert!(
        digest_over_normalized_row,
        "with no policy text, bundle_digest must attest the normalized row that \
         IS stored -- a digest over nothing is a false attestation"
    );
}

// ---------------------------------------------------------------------------
// 5b. The read path's re-validation is the LIVE CONSTRAINT, not a nicety
// ---------------------------------------------------------------------------

/// `0205:29-34` states plainly what makes the definer's residual capability — "a
/// row coherent in every checkable respect that Cedar's validator would
/// nonetheless reject" — acceptable: `load_enforced_object_policy_blocks`
/// (`authz-rest/src/store.rs:559-591`) re-runs the validator, the canonicality
/// comparison and the effect agreement on EVERY read and errors the whole load.
/// It also says, correctly, that deleting that re-validation "dies silently while
/// every test here stays green".
///
/// It was true. `cargo llvm-cov` over this file reported `rest/src/lib.rs:643-645`
/// — the loader-failure arm of `object_view_policies` — as the only uncovered
/// logic in the slice, together with `:2093`, the arm that stops a loader failure
/// from being rewritten into `resolve`'s own "no instance resolves that code".
/// Nothing made the loader fail, so the justification rested on unexecuted code.
///
/// Each forgery gets its OWN object type. Attached to one type the first error
/// masks the second and deleting either check alone leaves this green — a probe
/// that cannot say which arm is load-bearing is not evidence that both are.
///
/// The effect-agreement arm is deliberately absent: the definer refuses a
/// blocks/attachment effect disagreement, so it is unreachable through the only
/// credential that can attach. Seeding one as the owning superuser would test the
/// loader against a row no attacker can mint.
///
/// THIS IS THE RESIDUAL 0206 DOES NOT CLOSE, and the reason the on-every-read
/// re-validation stays. Both forgeries below are minted through the definer as
/// `console_ontology_cmd` and both SUCCEED: 0206 narrowed WHICH credential can
/// mint a validator-invalid or non-canonical row from `console_rt` to the command
/// role, and made the minting audited, but it did not make one unmintable. The
/// loader still has to refuse it on every read.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_forged_enforced_row_the_read_path_cannot_re_validate_fails_closed(owner_pool: PgPool) {
    let fx = Fixture::build(&owner_pool, "loader-revalidation").await;
    let honest = fx
        .publish("loaderhonest", instance_type_draft("loaderhonest"))
        .await;
    let invalid = fx
        .publish("loaderinvalid", instance_type_draft("loaderinvalid"))
        .await;
    let noncanon = fx
        .publish("loadernoncanon", instance_type_draft("loadernoncanon"))
        .await;
    fx.seed_instance(honest, "honest-and-visible", "CODE-HONEST")
        .await;
    let behind_invalid = fx
        .seed_instance(invalid, "behind-an-invalid-policy", "CODE-INVALID")
        .await;
    let behind_noncanon = fx
        .seed_instance(noncanon, "behind-a-noncanonical-policy", "CODE-NONCANON")
        .await;

    // POSITIVE CONTROL, first. Every refusal below is also satisfied by a read
    // path that has stopped serving anything at all.
    let attached = fx
        .post(
            &policies_path("loaderhonest"),
            json!({ "effect": "permit", "conditions": [owner_is_subject()] }),
        )
        .await;
    assert_eq!(attached.status, StatusCode::CREATED, "{:?}", attached.body);
    let honest_list = fx
        .get(&format!("/api/v1/ontology/instances?type={honest}"))
        .await;
    assert_instance_titles(&honest_list.body, &["honest-and-visible"]);

    // The residual capability 0205 names, actually exercised. Both rows are minted
    // as the genuine `console_ontology_cmd` through the definer and COMMITTED:
    // `forge_attach` rolls back, and a forgery that never persists cannot be read
    // back.
    //
    // The definer accepting these is the POINT, not a defect: its envelope checks
    // `resource_type`, `action`, `effect` and the condition-list shape, and all
    // four hold here. What it cannot hold is Cedar's verdict.
    let invalid_policy = fx
        .forge_attach_persisted(
            *invalid.as_uuid(),
            "permit",
            json!({
                "effect": "permit",
                "action": "view",
                "resource_type": "loaderinvalid",
                "conditions": [{
                    "attr": "not_a_declared_property",
                    "op": "eq",
                    "value": { "kind": "literal", "value": "anything" }
                }]
            }),
        )
        .await;
    assert!(
        invalid_policy.is_ok(),
        "the definer's SQL envelope cannot carry Cedar's verdict, and 0205:22-27 says \
         so. A refusal here means a second copy of the validator now lives in a \
         migration -- the divergence 0205 refused -- and this probe tests nothing: \
         {invalid_policy:?}"
    );

    // Canonicality: a VALID envelope plus one extra key. `NoCodeBlocks` declares no
    // `deny_unknown_fields`, so it deserializes and validates cleanly; only the
    // comparison against the re-normalized row can see the difference.
    let noncanon_policy = fx
        .forge_attach_persisted(
            *noncanon.as_uuid(),
            "permit",
            json!({
                "effect": "permit",
                "action": "view",
                "resource_type": "loadernoncanon",
                "conditions": [],
                "smuggled": "a key the re-normalized row will not carry"
            }),
        )
        .await;
    assert!(
        noncanon_policy.is_ok(),
        "an extra JSON key passes every scalar check the definer makes; if this is \
         refused the canonicality arm below is unreachable and proves nothing: \
         {noncanon_policy:?}"
    );

    for (arm, type_id, instance, code, title) in [
        (
            "validator",
            invalid,
            behind_invalid,
            "CODE-INVALID",
            "behind-an-invalid-policy",
        ),
        (
            "canonicality",
            noncanon,
            behind_noncanon,
            "CODE-NONCANON",
            "behind-a-noncanonical-policy",
        ),
    ] {
        let list = fx
            .get(&format!("/api/v1/ontology/instances?type={type_id}"))
            .await;
        assert!(
            list.status.is_server_error(),
            "{arm}: a policy set the read path cannot re-validate must fail the whole \
             load. Serving rows would be the optimistic unfiltered read the loader \
             exists to refuse; an empty 200 would be indistinguishable from a working \
             deny and would hide a broken policy engine forever. Got {}: {:?}",
            list.status,
            list.body
        );

        let by_id = fx
            .get(&format!("/api/v1/ontology/instances/{instance}"))
            .await;
        assert!(
            by_id.status.is_server_error(),
            "{arm}: reading by id must fail the same way the list does, got {}: {:?}",
            by_id.status,
            by_id.body
        );

        // `rest/src/lib.rs:2087-2088`: only a NotFound is rewritten into this
        // route's own miss, so a policy-loader failure still surfaces as the 500 it
        // is. Rewriting it would answer "no such code" for a code that resolves.
        let resolved = fx
            .get(&format!("/api/v1/ontology/resolve?code={code}"))
            .await;
        assert_ne!(
            resolved.status,
            StatusCode::NOT_FOUND,
            "{arm}: a broken policy load was reported as a resolvable-code miss; the \
             gate's 404 rewrite must cover NotFound only: {:?}",
            resolved.body
        );
        assert!(
            resolved.status.is_server_error(),
            "{arm}: resolve must surface the loader failure, got {}: {:?}",
            resolved.status,
            resolved.body
        );

        for (route, body) in [
            ("list", &list.body),
            ("by id", &by_id.body),
            ("resolve", &resolved.body),
        ] {
            let text = body_text(body);
            assert!(
                !text.contains(title) && !text.contains(&instance.to_string()),
                "{arm}: the {route} failure discloses the row it could not decide on: {text}"
            );
        }
    }

    // The failure is SCOPED to the type carrying the forged row. A read path that
    // 500s the whole tenant would satisfy every assertion above.
    let honest_after = fx
        .get(&format!("/api/v1/ontology/instances?type={honest}"))
        .await;
    assert_eq!(
        honest_after.status,
        StatusCode::OK,
        "an honestly authored type must survive a forgery attached to another type: {:?}",
        honest_after.body
    );
    assert_instance_titles(&honest_after.body, &["honest-and-visible"]);
}

// ---------------------------------------------------------------------------
// 5b-bis. The two arms of the SAME re-validation that executed nowhere
// ---------------------------------------------------------------------------
//
// MEASURED, not estimated. Region coverage over the whole green suite
// (`cargo llvm-cov report`, per-segment execution counts) puts two of the four
// fail-closed arms of `PgCedarPolicyStore::load_enforced_object_policy_blocks` at
// ZERO executions: the deserialization refusal and the effect-agreement
// comparison. The other two -- validator verdict and canonicality -- ran 3 times
// each, from `a_forged_enforced_row_the_read_path_cannot_re_validate_fails_closed`.
//
// Either uncovered arm could be deleted today with all 24 tests still green,
// which is precisely what 0205's header says must never be true of this
// re-validation ("Delete this re-validation and a forged row is served while
// every test here stays green"). That header claim was two-thirds accurate and is
// now fully so.
//
// WHY the existing forgery test cannot reach them, and rightly does not try: it
// mints every forgery through the definer as `console_ontology_cmd`, and 0206
// binds catalog effect, attachment effect and `normalized_row.effect` from ONE
// parameter it also cross-checks, so a definer-minted row can disagree with
// nothing. Neither arm is reachable by any credential.
//
// They are reachable by every OTHER surface that writes these two tables -- the
// policy studio, a fixture, a backfill migration. That is the case
// `attach_enforced_policy` exists for and the case
// `a_row_hidden_from_the_list_is_refused_by_id_as_of_history_traverse_acting_and_resolve`
// already names ("the permanent guard for policies attached by any other
// surface"). Two tests and not one loop: each arm is an independent claim, and an
// arm sitting behind another arm's failing assert never executes.

/// A `forbid` in the catalog and in the attachment whose stored blocks say
/// `permit` must fail every read closed.
///
/// 0170's trigger compares the ATTACHMENT to the CATALOG only, and 0205 says so
/// where it declines to re-check that pair inside the definer. Both are `forbid`
/// here, so the trigger is satisfied and the disagreement is invisible to the
/// database. The loader's comparison is the only thing that sees it.
///
/// The consequence of deleting that comparison is not a wrong error message: the
/// loader hands the gate `Effect::Permit` with an empty condition list, which
/// permits every row of the type. A forbid inverts into a blanket permit and the
/// rows it exists to hide are served, with the catalog still reading `forbid` to
/// anyone who audits the table.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_catalog_row_whose_blocks_disagree_with_its_effect_is_refused_on_every_read(
    owner_pool: PgPool,
) {
    let fx = Fixture::build(&owner_pool, "effect-drift").await;
    let honest = fx
        .publish("driftcontrol", instance_type_draft("driftcontrol"))
        .await;
    let drifted = fx
        .publish("effectdrift", instance_type_draft("effectdrift"))
        .await;
    fx.seed_instance(honest, "honest-and-visible", "CODE-DRIFT-OK")
        .await;
    let hidden = fx
        .seed_instance(drifted, "behind-an-inverted-forbid", "CODE-DRIFT-BAD")
        .await;

    // POSITIVE CONTROL, first and through the real route. Every 5xx below is also
    // produced by a read path that has stopped serving anything at all.
    let attached = fx
        .post(
            &policies_path("driftcontrol"),
            json!({ "effect": "permit", "conditions": [owner_is_subject()] }),
        )
        .await;
    assert_eq!(attached.status, StatusCode::CREATED, "{:?}", attached.body);
    let control = fx
        .get(&format!("/api/v1/ontology/instances?type={honest}"))
        .await;
    assert_instance_titles(&control.body, &["honest-and-visible"]);

    // `forbid` for the catalog row AND the attachment -- so 0170's trigger passes
    // -- with a normalized row that is canonical and validator-valid for `permit`.
    // Canonical and valid on purpose: the loader checks the validator verdict and
    // canonicality BEFORE the effect comparison, so a row that failed either would
    // be refused by an arm that already has coverage and this test would prove
    // nothing about the arm it names.
    attach_enforced_policy(
        &owner_pool,
        fx.org,
        drifted,
        "object_policy.effectdrift.inverted",
        "forbid",
        canonical_normalized_row("permit", "effectdrift", vec![]),
    )
    .await;

    let list = fx
        .get(&format!("/api/v1/ontology/instances?type={drifted}"))
        .await;
    assert!(
        list.status.is_server_error(),
        "a stored `permit` under a `forbid` catalog row and a `forbid` attachment \
         must fail the whole load. Serving rows here is the forbid inverted into a \
         blanket permit -- the loader would lower `Effect::Permit` with no \
         conditions while the catalog still reads `forbid` to anyone auditing the \
         table. An empty 200 would be indistinguishable from the forbid working. \
         Got {}: {:?}",
        list.status,
        list.body
    );

    // By id as well, so the refusal is the LOADER and not the list query: the two
    // routes share only `load_enforced_object_policy_blocks`.
    let by_id = fx
        .get(&format!("/api/v1/ontology/instances/{hidden}"))
        .await;
    assert!(
        by_id.status.is_server_error(),
        "reading by id must fail the same way the list does, got {}: {:?}",
        by_id.status,
        by_id.body
    );
    for (route, body) in [("list", &list.body), ("by id", &by_id.body)] {
        let text = body_text(body);
        assert!(
            !text.contains("behind-an-inverted-forbid") && !text.contains(&hidden.to_string()),
            "the {route} failure discloses the row it could not decide on: {text}"
        );
    }

    // SCOPED to the type carrying the drifted row. A loader that 500s the whole
    // tenant satisfies every assertion above.
    let control_after = fx
        .get(&format!("/api/v1/ontology/instances?type={honest}"))
        .await;
    assert_eq!(
        control_after.status,
        StatusCode::OK,
        "an honestly authored type must survive a drifted row on another type: {:?}",
        control_after.body
    );
    assert_instance_titles(&control_after.body, &["honest-and-visible"]);
}

/// A `normalized_row` that is not a `NoCodeBlocks` at all must fail every read
/// closed, never be skipped.
///
/// The arm is one `map_err(...)?`, and the mutation that kills it is the ordinary
/// defensive one: skip the row you cannot understand, or substitute a default. A
/// skipped `forbid` is a served row. The loader has to hard-error the whole load
/// instead, and no other test supplies a row it cannot deserialize -- every
/// forgery reachable through the definer is well-formed JSONB by construction,
/// because the definer's own checks read `normalized_row ->> 'effect'`,
/// `'action'`, `'resource_type'` and `jsonb_array_length(... -> 'conditions')`.
///
/// `conditions` as a STRING and not a missing key: a missing key would also be
/// refused by the definer's `jsonb_typeof(...) IS DISTINCT FROM 'array'` check,
/// while this row is one the shape checks of 0205 would pass and only serde
/// rejects -- so the arm under test is the deserialization arm and not a
/// re-statement of an envelope check.
///
/// The unreadable row is attached BESIDE an honest `permit` on the SAME type, and
/// that is what makes this a security test rather than a message test. Measured on
/// the two obvious defensive mutations:
///
///   * substituting a default block for the one that would not parse is caught
///     DOWNSTREAM by the canonicality arm, which already has coverage -- so this
///     test correctly stays green and the arm is not what saved it;
///   * SKIPPING the row it cannot read serves every row on the type, because the
///     honest permit beside it survives. That is the mutation this test exists to
///     fail on, and with only one policy on the type it would instead produce a
///     deny-by-default `200 []` -- a refusal by accident, which would have made
///     the red indistinguishable from a fail-closed loader.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_catalog_row_whose_normalized_row_is_unparseable_is_refused_on_every_read(
    owner_pool: PgPool,
) {
    let fx = Fixture::build(&owner_pool, "unparseable-row").await;
    let scoping = fx
        .publish("parsecontrol", instance_type_draft("parsecontrol"))
        .await;
    let unparseable = fx
        .publish("unparseable", instance_type_draft("unparseable"))
        .await;
    fx.seed_instance(scoping, "honest-and-visible", "CODE-PARSE-OK")
        .await;
    let hidden = fx
        .seed_instance(unparseable, "behind-an-unreadable-policy", "CODE-PARSE-BAD")
        .await;

    // Two honest permits through the real route: one on the type that will carry
    // the unreadable row, one on a second type that must survive it.
    for key in ["parsecontrol", "unparseable"] {
        let attached = fx
            .post(
                &policies_path(key),
                json!({ "effect": "permit", "conditions": [owner_is_subject()] }),
            )
            .await;
        assert_eq!(
            attached.status,
            StatusCode::CREATED,
            "{key}: {:?}",
            attached.body
        );
    }
    // POSITIVE CONTROL on the type under test, before the forgery lands: the row
    // IS served while the policy set is readable, so the 5xx below is the
    // unreadable row and not a fixture that never served anything.
    let control = fx
        .get(&format!("/api/v1/ontology/instances?type={unparseable}"))
        .await;
    assert_instance_titles(&control.body, &["behind-an-unreadable-policy"]);

    attach_enforced_policy(
        &owner_pool,
        fx.org,
        unparseable,
        "object_policy.unparseable.blocks",
        "permit",
        json!({
            "effect": "permit",
            "action": "view",
            "resource_type": "unparseable",
            "conditions": "not-an-array"
        }),
    )
    .await;

    let list = fx
        .get(&format!("/api/v1/ontology/instances?type={unparseable}"))
        .await;
    assert!(
        list.status.is_server_error(),
        "an enforced row the loader cannot deserialize must fail the WHOLE load. \
         Skipping it drops a policy that may be the `forbid` hiding these rows \
         while the honest permit beside it keeps serving them -- measured: with the \
         skip applied this route returns 200 carrying \
         `behind-an-unreadable-policy`. Got {}: {:?}",
        list.status,
        list.body
    );

    let by_id = fx
        .get(&format!("/api/v1/ontology/instances/{hidden}"))
        .await;
    assert!(
        by_id.status.is_server_error(),
        "reading by id must fail the same way the list does, got {}: {:?}",
        by_id.status,
        by_id.body
    );
    for (route, body) in [("list", &list.body), ("by id", &by_id.body)] {
        let text = body_text(body);
        assert!(
            !text.contains("behind-an-unreadable-policy") && !text.contains(&hidden.to_string()),
            "the {route} failure discloses the row it could not decide on: {text}"
        );
    }

    let control_after = fx
        .get(&format!("/api/v1/ontology/instances?type={scoping}"))
        .await;
    assert_eq!(
        control_after.status,
        StatusCode::OK,
        "an honestly authored type must survive an unreadable row on another type: {:?}",
        control_after.body
    );
    assert_instance_titles(&control_after.body, &["honest-and-visible"]);
}

// ---------------------------------------------------------------------------
// 5c. The condition bound, at its edge
// ---------------------------------------------------------------------------

/// The route bounds a persisted policy at 32 conditions (`rest/src/lib.rs:141`)
/// and `0205:224-227` mirrors the bound inside the definer. Only the far side of
/// each was ever exercised — the route at 64, the definer at 33 — so the edge
/// itself was invisible, and so was any drift between the two: a route bound of 31
/// makes a legal policy unauthorable and its type permanently invisible, while a
/// route bound of 33 turns a 422 the caller can act on into the definer's `RAISE`,
/// which the REST layer reports as an unhandled 500.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn thirty_two_conditions_attach_and_thirty_three_are_refused_by_the_route(
    owner_pool: PgPool,
) {
    let fx = Fixture::build(&owner_pool, "policy-bound").await;
    let type_id = fx
        .publish("policybound", instance_type_draft("policybound"))
        .await;
    fx.seed_instance(type_id, "bounded-and-visible", "CODE-BOUND")
        .await;

    let at_the_bound: Vec<Value> = std::iter::repeat_with(owner_is_subject).take(32).collect();
    let accepted = fx
        .post(
            &policies_path("policybound"),
            json!({ "effect": "permit", "conditions": at_the_bound }),
        )
        .await;
    assert_eq!(
        accepted.status,
        StatusCode::CREATED,
        "32 is the bound, not one past it: a route that refuses it makes a legal rule \
         unauthorable, and an unattached type is permanently invisible: {:?}",
        accepted.body
    );

    // The row must still be READABLE. The attach and the read derive `declared`
    // from the same function, but only a read proves the persisted row survives
    // the loader's re-validation at the bound.
    let list = fx
        .get(&format!("/api/v1/ontology/instances?type={type_id}"))
        .await;
    assert_eq!(list.status, StatusCode::OK, "{:?}", list.body);
    assert_instance_titles(&list.body, &["bounded-and-visible"]);

    let one_too_many: Vec<Value> = std::iter::repeat_with(owner_is_subject).take(33).collect();
    let refused = fx
        .post(
            &policies_path("policybound"),
            json!({ "effect": "permit", "conditions": one_too_many }),
        )
        .await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "33 must be refused BY THE ROUTE. A 500 here means the route let it through \
         and the definer's RAISE is what stopped it -- the two bounds have drifted: {:?}",
        refused.body
    );
    assert!(
        body_text(&refused.body).contains("32"),
        "the refusal must name the bound it applied: {:?}",
        refused.body
    );
}

// ---------------------------------------------------------------------------
// 6. Totality: no ontology route may quietly skip instance-visibility gating
// ---------------------------------------------------------------------------

/// Every route the crate publishes is classified here. A new `.route(` with no
/// entry fails this assertion, so "we forgot to gate the new read path" is a red
/// test rather than a silent leak.
enum InstanceVisibility {
    /// Serves no instance row.
    ///
    /// NOT LOAD-BEARING, and saying so is the point. Proving "this handler can
    /// never serve an instance row" is a negative claim over a handler body: no
    /// route-level test can establish it, and a source-text classifier fails by
    /// construction because the gate sits two frames below the handler
    /// (`action_preflight` -> `preflight_action` -> `prepare()`). A future route
    /// mislabelled here therefore ships green as far as THIS table is concerned.
    /// What covers that hole is not a stronger label but the sealed instance
    /// store in `rest/src/lib.rs`, which makes an ungated single-row read
    /// impossible to compile whatever it is labelled.
    NotInstanceBearing,
    /// Serves or MUTATES an instance row under the object-policy residual.
    ///
    /// Not a label. `method`, `uri` and `body` are a REAL request
    /// [`every_gated_route_refuses_a_policy_hidden_instance`] issues against a
    /// policy-hidden row, so an entry that claims gating it does not have fails
    /// by execution. `{id}`, `{type}`, `{code}` and `{command_id}` are
    /// substituted per request.
    Gated {
        method: &'static str,
        uri: &'static str,
        body: &'static str,
    },
}

const INSTANCE_ROUTE_CLASSIFICATION: &[(&str, InstanceVisibility)] = &[
    (
        "/api/v1/ontology/object-types",
        InstanceVisibility::NotInstanceBearing,
    ),
    (
        "/api/v1/ontology/object-types/{key}",
        InstanceVisibility::NotInstanceBearing,
    ),
    (
        "/api/v1/ontology/object-types/{key}/acting",
        InstanceVisibility::NotInstanceBearing,
    ),
    (
        "/api/v1/ontology/object-types/{key}/lifecycle",
        InstanceVisibility::NotInstanceBearing,
    ),
    (
        "/api/v1/ontology/object-types/{key}/policies",
        InstanceVisibility::NotInstanceBearing,
    ),
    (
        "/api/v1/ontology/instances",
        InstanceVisibility::Gated {
            method: "GET",
            uri: "/api/v1/ontology/instances?type={type}",
            body: "",
        },
    ),
    (
        "/api/v1/ontology/instances/aggregate",
        InstanceVisibility::Gated {
            method: "GET",
            uri: "/api/v1/ontology/instances/aggregate?type={type}&group_by=lifecycle_state",
            body: "",
        },
    ),
    (
        "/api/v1/ontology/instances/{id}",
        InstanceVisibility::Gated {
            method: "GET",
            uri: "/api/v1/ontology/instances/{id}",
            body: "",
        },
    ),
    (
        "/api/v1/ontology/instances/{id}/history",
        InstanceVisibility::Gated {
            method: "GET",
            uri: "/api/v1/ontology/instances/{id}/history",
            body: "",
        },
    ),
    (
        "/api/v1/ontology/instances/{id}/traverse",
        InstanceVisibility::Gated {
            method: "GET",
            uri: "/api/v1/ontology/instances/{id}/traverse",
            body: "",
        },
    ),
    (
        "/api/v1/ontology/instances/{id}/acting",
        InstanceVisibility::Gated {
            method: "GET",
            uri: "/api/v1/ontology/instances/{id}/acting",
            body: "",
        },
    ),
    (
        "/api/v1/ontology/instances/{id}/lifecycle",
        InstanceVisibility::Gated {
            method: "POST",
            uri: "/api/v1/ontology/instances/{id}/lifecycle",
            body: r#"{"to_state":"active"}"#,
        },
    ),
    (
        "/api/v1/ontology/resolve",
        InstanceVisibility::Gated {
            method: "GET",
            uri: "/api/v1/ontology/resolve?code={code}",
            body: "",
        },
    ),
    (
        "/api/v1/ontology/actions/{action_key}/preflight",
        InstanceVisibility::Gated {
            method: "POST",
            uri: "/api/v1/ontology/actions/set_code/preflight",
            // Preflight's own input contract: it evaluates neither `command_id`
            // nor `expected_revision`, so it must answer the gate chain without
            // them. A 422 here would mean the positive control never reached the
            // gate chain, which the `assert_ne!` below now catches.
            body: r#"{"object_type_id":"{type}","instance_id":"{id}","params":{"code":"CODE-SWEPT"}}"#,
        },
    ),
    (
        "/api/v1/ontology/actions/{action_key}/execute",
        InstanceVisibility::Gated {
            method: "POST",
            uri: "/api/v1/ontology/actions/set_code/execute",
            body: r#"{"object_type_id":"{type}","instance_id":"{id}","params":{"code":"CODE-SWEPT"},"expected_revision":1,"command_id":"{command_id}"}"#,
        },
    ),
];

#[test]
fn every_ontology_route_is_classified_for_instance_visibility() {
    for path in ONTOLOGY_ROUTE_PATHS {
        assert!(
            INSTANCE_ROUTE_CLASSIFICATION
                .iter()
                .any(|(candidate, _)| candidate == path),
            "unclassified route {path}: classify it before it ships, or it ships ungated"
        );
    }
    for (path, _) in INSTANCE_ROUTE_CLASSIFICATION {
        assert!(
            ONTOLOGY_ROUTE_PATHS.contains(path),
            "{path} is classified but is not a published route"
        );
    }
}

/// The classification above used to be inert: the variants were constructed and
/// matched nowhere, so a leaking route labelled `Gated` shipped green. This is
/// what makes the label cost something — every `Gated` entry is issued as a real
/// request against a policy-hidden instance, and every one of them must refuse.
///
/// The sweep is the TOTALITY check the per-route tests above are not: those pin
/// four routes in detail, this one pins that there are no OTHERS. Adding a route
/// forces an entry; an entry that claims `Gated` and is not fails here.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn every_gated_route_refuses_a_policy_hidden_instance(owner_pool: PgPool) {
    let fx = Fixture::build(&owner_pool, "policy-sweep").await;
    let type_id = fx
        .publish("policysweep", editable_instance_type_draft("policysweep"))
        .await;

    let mine = fx
        .seed_instance(type_id, "sweep-visible", "CODE-SWEEP-VISIBLE")
        .await;
    let hidden = fx
        .seed_instance_owned_by(type_id, "sweep-hidden", "another-user", "CODE-SWEEP-HIDDEN")
        .await;
    attach_enforced_policy(
        &owner_pool,
        fx.org,
        type_id,
        "policy.sweep_owner_permit",
        "permit",
        canonical_normalized_row("permit", "policysweep", vec![owner_is_subject()]),
    )
    .await;

    let render = |template: &str, instance: Uuid, code: &str| {
        template
            .replace("{id}", &instance.to_string())
            .replace("{type}", &type_id.to_string())
            .replace("{code}", code)
            .replace("{command_id}", &Uuid::new_v4().to_string())
    };

    // POSITIVE CONTROLS, all of them, FIRST. Every refusal below is also
    // satisfied by a gate that refuses everything, and a fixture that hid both
    // rows would make the whole sweep vacuous.
    for (path, visibility) in INSTANCE_ROUTE_CLASSIFICATION {
        let InstanceVisibility::Gated { method, uri, body } = visibility else {
            continue;
        };
        let res = fx
            .request(
                method,
                &render(uri, mine, "CODE-SWEEP-VISIBLE"),
                Some(&fx.token),
                render(body, mine, "CODE-SWEEP-VISIBLE")
                    .parse::<Value>()
                    .unwrap_or(Value::Null),
            )
            .await;
        assert_ne!(
            res.status,
            StatusCode::NOT_FOUND,
            "{path}: the PERMITTED row must reach this route, or its 404 below \
             proves nothing: {:?}",
            res.body
        );
        // A 422 means the request never reached the gate chain at all, so the
        // hidden row's 404 below would be indistinguishable from the same
        // rejection. The entry's body must stay a request this route accepts;
        // this is what fails when it drifts out of the route's input contract.
        assert_ne!(
            res.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{path}: the PERMITTED row was refused before the gate chain, so its \
             404 below proves nothing -- fix this entry's body: {:?}",
            res.body
        );
    }

    // --- the hidden row ----------------------------------------------------
    let revisions_before = fx.revision_count(hidden).await;
    let state_before = fx.lifecycle_state(hidden).await;

    for (path, visibility) in INSTANCE_ROUTE_CLASSIFICATION {
        let InstanceVisibility::Gated { method, uri, body } = visibility else {
            continue;
        };
        let res = fx
            .request(
                method,
                &render(uri, hidden, "CODE-SWEEP-HIDDEN"),
                Some(&fx.token),
                render(body, hidden, "CODE-SWEEP-HIDDEN")
                    .parse::<Value>()
                    .unwrap_or(Value::Null),
            )
            .await;
        let text = body_text(&res.body);
        assert!(
            !text.contains(&hidden.to_string()) && !text.contains("sweep-hidden"),
            "{path}: the response discloses the policy-hidden row: {text}"
        );
        // Whether the request NAMES the row, not whether the URI does: preflight
        // and execute carry `instance_id` in the BODY, and a URI-only test skips
        // exactly the two routes that mutate. Preflight leaks no id and no title,
        // so without this its `criteria_ok` boolean oracle over the hidden row's
        // attributes passes the disclosure check above unnoticed.
        if uri.contains("{id}") || body.contains("{id}") {
            assert_eq!(
                res.status,
                StatusCode::NOT_FOUND,
                "{path}: a row the residual denies must be 404 and never 403 -- a \
                 refusal that distinguishes hidden from missing is an existence \
                 oracle on the route being closed: {:?}",
                res.body
            );
        }
        if *method != "GET" {
            assert_eq!(
                fx.revision_count(hidden).await,
                revisions_before,
                "{path}: appended a revision to a policy-hidden row; refusal must \
                 precede every write, not be reported after one"
            );
            assert_eq!(
                fx.lifecycle_state(hidden).await,
                state_before,
                "{path}: transitioned a policy-hidden row"
            );
        }
    }

    // NOT SWEPT HERE: an unauthenticated caller. It was written, and removed
    // because it could not be proven red. Authentication on this crate is ONE
    // object — the `with_request_context` layer `router()` wraps every route in
    // (`rest/src/lib.rs:258`) — and that same layer is what scopes `CURRENT_ORG`,
    // so a router without it cannot serve an AUTHENTICATED request either: the
    // mutation that removes authentication takes the fixture down at `publish`
    // before any anonymous request is issued (measured). Eight per-route
    // assertions would have been eight readings of one indivisible object with no
    // failure mode of their own. Where it belongs is a middleware test in
    // `platform/request-context`, which has no `tests/` directory at all.
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

struct Fixture {
    org: OrgId,
    actor: UserId,
    token: String,
    owner_pool: PgPool,
    runtime_pool: PgPool,
    /// A `console_ontology_cmd` connection. After 0206 this is the ONLY
    /// credential that can reach `ont_policy_api.attach_object_policy`, so every
    /// definer probe below runs on it; `runtime_pool` keeps the `console_rt`
    /// probes that assert a REFUSAL.
    command_pool: PgPool,
    service: axum::Router,
    /// A DISTINCT principal, because publishing is four-eyes: the requester may
    /// not be the decider.
    approver_token: String,
}

impl Fixture {
    async fn build(owner_pool: &PgPool, tag: &str) -> Self {
        let org = OrgId::knl();
        let actor = seed_org_and_super_admin(owner_pool, *org.as_uuid(), tag).await;
        let auth = test_auth(actor, org);
        let runtime_pool = runtime_role_pool(owner_pool).await;
        let command_pool = command_role_pool(owner_pool).await;
        // Merged exactly as `build_router` merges them: publishing a type
        // consumes four-eyes evidence authored through the governance routes, so
        // a fixture that cannot reach them cannot reach a published type either.
        let service = router(OntologyRestState::new(
            PgOntologyStore::new(runtime_pool.clone()).with_command_pool(command_pool.clone()),
            PgInstanceStore::new(runtime_pool.clone()),
            PgGovernanceStore::new(runtime_pool.clone()),
            Some(auth.verifier.clone()),
        ))
        .merge(console_governance_rest::router(GovernanceRestState::new(
            PgGovernanceStore::new(runtime_pool.clone()),
            Some(auth.verifier),
        )));
        let approver =
            seed_org_and_super_admin(owner_pool, *org.as_uuid(), &format!("{tag}-approver")).await;
        Self {
            org,
            actor,
            token: auth.token,
            approver_token: issue_token(&SIGNING_KEY.with(SigningKey::clone), approver, org),
            owner_pool: owner_pool.clone(),
            runtime_pool,
            command_pool,
            service,
        }
    }

    /// A second signed-in principal in the SAME org: same route, same feature
    /// grant, different `subject.user_id`, so only the residual can differ.
    async fn other_principal(&self, tag: &str) -> String {
        let user = seed_org_and_super_admin(&self.owner_pool, *self.org.as_uuid(), tag).await;
        // The router holds one verifier, so the second token must be issued by
        // the same key. `test_auth` mints a fresh pair, which the router would
        // reject; re-issue instead.
        issue_token(&SIGNING_KEY.with(SigningKey::clone), user, self.org)
    }

    async fn publish(&self, stable_key: &str, draft: Value) -> ObjectTypeId {
        let created = self.post("/api/v1/ontology/object-types", draft).await;
        assert_eq!(
            created.status,
            StatusCode::CREATED,
            "publish {stable_key}: {:?}",
            created.body
        );
        ObjectTypeId::from_uuid(
            created.body["id"]
                .as_str()
                .expect("object-type create response must carry an id")
                .parse()
                .expect("object-type id must be a UUID"),
        )
    }

    /// Take `stable_key` through the REAL versioning path, entirely over HTTP:
    /// publish v1 (four eyes), stage a v2 behind it, publish v2 (four eyes
    /// again). `ontology_api.stage_object_type` inserts a NEW `ont_object_types`
    /// row with a fresh `gen_random_uuid()` and `schema_version = max + 1`
    /// (`0165:884-900`); nothing anywhere re-points `ont_instances.object_type_id`
    /// at it, so every row seeded before this call stays filed under v1.
    ///
    /// Hand-rolled SQL was tried first and is impossible by design: the DB
    /// refuses an unaudited version (`ontology_write.exactly_one_current_
    /// transaction_audit_required`, `0165:372-426`) and refuses the audit row to
    /// anyone but the command role (`ontology_audit.command_required`,
    /// `0165` `protected_audit_writer_guard`). Only the real path reaches this state.
    async fn revise_type_to_a_new_published_version(&self, stable_key: &str, draft: Value) {
        let detail = self
            .get(&format!("/api/v1/ontology/object-types/{stable_key}"))
            .await;
        assert_eq!(detail.status, StatusCode::OK, "{:?}", detail.body);
        let v1_id = detail.body["object_type"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let etag = self
            .publish_lifecycle(stable_key, &v1_id, &detail.etag())
            .await;

        let staged = self
            .send(
                "PUT",
                &format!("/api/v1/ontology/object-types/{stable_key}"),
                Some(&etag),
                draft,
            )
            .await;
        assert_eq!(
            staged.status,
            StatusCode::CREATED,
            "stage v2: {:?}",
            staged.body
        );
        let v2_id = staged.body["id"].as_str().unwrap().to_owned();
        assert_ne!(v1_id, v2_id, "a staged revision must be a NEW version row");
        assert_eq!(staged.body["schema_version"], json!(2));

        // `?version=2`: key-only addressing resolves the PUBLISHED head, i.e. v1.
        let reviewed = self
            .send(
                "POST",
                &format!("/api/v1/ontology/object-types/{stable_key}/lifecycle?version=2"),
                Some(&staged.etag()),
                json!({ "to_state": "review_pending" }),
            )
            .await;
        assert_eq!(
            reviewed.status,
            StatusCode::OK,
            "v2 review: {:?}",
            reviewed.body
        );
        self.approve_publish_of(
            &v2_id,
            reviewed.body["key_write_revision"].as_i64().unwrap(),
        )
        .await;
        let published = self
            .send(
                "POST",
                &format!("/api/v1/ontology/object-types/{stable_key}/lifecycle?version=2"),
                Some(&reviewed.etag()),
                json!({ "to_state": "published" }),
            )
            .await;
        assert_eq!(
            published.status,
            StatusCode::OK,
            "v2 publish: {:?}",
            published.body
        );
        assert_eq!(published.body["lifecycle_state"], json!("published"));
    }

    /// draft -> review_pending -> (four eyes) -> published. Returns the ETag the
    /// publish left behind.
    async fn publish_lifecycle(&self, stable_key: &str, type_id: &str, etag: &str) -> String {
        let uri = format!("/api/v1/ontology/object-types/{stable_key}/lifecycle");
        let reviewed = self
            .send(
                "POST",
                &uri,
                Some(etag),
                json!({ "to_state": "review_pending" }),
            )
            .await;
        assert_eq!(
            reviewed.status,
            StatusCode::OK,
            "review: {:?}",
            reviewed.body
        );
        self.approve_publish_of(
            type_id,
            reviewed.body["key_write_revision"].as_i64().unwrap(),
        )
        .await;
        let published = self
            .send(
                "POST",
                &uri,
                Some(&reviewed.etag()),
                json!({ "to_state": "published" }),
            )
            .await;
        assert_eq!(
            published.status,
            StatusCode::OK,
            "publish: {:?}",
            published.body
        );
        published.etag()
    }

    /// The four-eyes half: the SAME actor requests, a DISTINCT one decides.
    async fn approve_publish_of(&self, type_id: &str, key_revision: i64) {
        let request_ref = Uuid::new_v4();
        let requested = self
            .post(
                "/api/v1/governance/approvals",
                json!({
                    "request_ref": request_ref,
                    "kind": "ontology.schema.publish",
                    "target_ref": type_id,
                    "payload_summary": { "key_revision": key_revision }
                }),
            )
            .await;
        assert_eq!(
            requested.status,
            StatusCode::CREATED,
            "request: {:?}",
            requested.body
        );
        let decided = self
            .request(
                "POST",
                "/api/v1/governance/approvals/decide",
                Some(&self.approver_token),
                json!({
                    "request_ref": request_ref,
                    "kind": "ontology.schema.publish",
                    "requested_by": self.actor.as_uuid().to_string(),
                    "decision": "approved"
                }),
            )
            .await;
        assert_eq!(
            decided.status,
            StatusCode::CREATED,
            "decide: {:?}",
            decided.body
        );
    }

    async fn link_type_id(&self, stable_key: &str) -> LinkTypeId {
        let detail = self
            .get(&format!("/api/v1/ontology/object-types/{stable_key}"))
            .await;
        assert_eq!(detail.status, StatusCode::OK, "{:?}", detail.body);
        LinkTypeId::from_uuid(
            detail.body["links"][0]["id"]
                .as_str()
                .expect("the draft declared one link type")
                .parse()
                .unwrap(),
        )
    }

    /// Seeded owned by the fixture's own actor, i.e. by the principal every
    /// permit in this file is authored for.
    async fn seed_instance(&self, type_id: ObjectTypeId, title: &str, code: &str) -> Uuid {
        self.seed_instance_owned_by(type_id, title, &self.actor.to_string(), code)
            .await
    }

    async fn seed_instance_owned_by(
        &self,
        type_id: ObjectTypeId,
        title: &str,
        owner: &str,
        code: &str,
    ) -> Uuid {
        scope_org(self.org, async {
            PgInstanceStore::new(self.runtime_pool.clone())
                .create_instance(
                    self.actor,
                    CreateInstance {
                        object_type_id: type_id,
                        title: title.to_owned(),
                        attributes: json!({ "owner": owner, "code": code }),
                        valid_from: None,
                        action_type_id: None,
                        reason: Some("object-policy attach fixture".to_owned()),
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

    /// Append a second revision, so `history` has a chain and `as_of` has a past
    /// that differs from the head.
    async fn revise(&self, id: Uuid, attributes: Value, at: OffsetDateTime) {
        scope_org(self.org, async {
            PgInstanceStore::new(self.runtime_pool.clone())
                .stage_revision(
                    self.actor,
                    console_ontology_domain::InstanceId::from_uuid(id),
                    StageRevision {
                        attributes,
                        valid_from: None,
                        action_type_id: None,
                        reason: Some("object-policy attach fixture revision".to_owned()),
                    },
                    TraceContext::generate(),
                    at,
                )
                .await
                .expect("revise the fixture instance through console_rt");
        })
        .await;
    }

    /// Counted on the OWNER pool (the `#[sqlx::test]` BYPASSRLS superuser) on
    /// purpose: a "nothing was written" claim must be made by a reader that
    /// cannot miss a row. Never used for an isolation assertion.
    async fn revision_count(&self, instance_id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM ont_instance_revisions WHERE instance_id = $1")
            .bind(instance_id)
            .fetch_one(&self.owner_pool)
            .await
            .unwrap()
    }

    async fn lifecycle_state(&self, instance_id: Uuid) -> String {
        sqlx::query_scalar("SELECT lifecycle_state FROM ont_instances WHERE id = $1")
            .bind(instance_id)
            .fetch_one(&self.owner_pool)
            .await
            .unwrap()
    }

    async fn command_receipt_count(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM ont_action_command_receipts WHERE org_id = $1")
            .bind(*self.org.as_uuid())
            .fetch_one(&self.owner_pool)
            .await
            .unwrap()
    }

    async fn link(&self, link_type: LinkTypeId, from: Uuid, to: Uuid) {
        scope_org(self.org, async {
            PgInstanceStore::new(self.runtime_pool.clone())
                .create_link(
                    self.actor,
                    link_type,
                    console_ontology_domain::InstanceId::from_uuid(from),
                    console_ontology_domain::InstanceId::from_uuid(to),
                    None,
                    TraceContext::generate(),
                    OffsetDateTime::now_utc(),
                )
                .await
                .expect("link the fixture instances");
        })
        .await;
    }

    /// A transaction on `pool` with `app.current_org` armed the way a
    /// request-scoped connection arms it. `scope_org` alone only sets a task-local:
    /// without the GUC every RLS read returns zero rows and passes any "no rows"
    /// assertion while proving nothing.
    ///
    /// The disposition is the CALLER's: the role runners below roll back, and the
    /// two probes that have to read their own write back afterwards commit.
    async fn armed_tx(&self, pool: &PgPool) -> sqlx::Transaction<'static, sqlx::Postgres> {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(self.org.as_uuid().to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        tx
    }

    /// Runs a closure on a `console_rt` connection with the org armed, rolled back
    /// afterwards. See [`Self::armed_tx`] for why the GUC is not optional.
    async fn as_runtime_role<T, F>(&self, body: F) -> T
    where
        F: for<'c> FnOnce(
            &'c mut sqlx::Transaction<'static, sqlx::Postgres>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'c>>,
    {
        let mut tx = self.armed_tx(&self.runtime_pool).await;
        let out = body(&mut tx).await;
        tx.rollback().await.unwrap();
        out
    }

    /// [`Self::as_runtime_role`] on the `console_ontology_cmd` pool instead.
    ///
    /// After 0206 the command role is the only credential that may execute the
    /// attach definer, so every probe of the definer's OWN bounds has to run
    /// here. Same arming, same rollback — only the role differs, which is what
    /// keeps each re-pointed probe testing the envelope check it names rather
    /// than degenerating into a permission assertion.
    async fn as_command_role<T, F>(&self, body: F) -> T
    where
        F: for<'c> FnOnce(
            &'c mut sqlx::Transaction<'static, sqlx::Postgres>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'c>>,
    {
        let mut tx = self.armed_tx(&self.command_pool).await;
        let out = body(&mut tx).await;
        tx.rollback().await.unwrap();
        out
    }

    /// One hand-crafted `attach_object_policy` call as the genuine
    /// `console_ontology_cmd` role with the org armed — every envelope check the
    /// definer performs, probed by the ONE credential 0206 leaves able to reach
    /// it. The org is always the armed one and `created_by` is always this
    /// fixture's REAL actor, so only the parameter under test differs from a call
    /// the route itself would have made: a nil id would be refused by
    /// `created_by`'s composite FK (0150:41) whatever the definer did, which is a
    /// refusal that proves nothing about the hardening.
    ///
    /// This ran as `console_rt` before 0206 — that role now gets 42501 before the
    /// body, which is asserted on its own in
    /// `console_rt_cannot_execute_the_attach_definer_at_all`.
    async fn forge_attach(
        &self,
        object_type_id: Uuid,
        effect: &str,
        normalized_row: Value,
    ) -> Result<Uuid, String> {
        let mut tx = self.armed_tx(&self.command_pool).await;
        let forged = self
            .attach_via_definer(&mut tx, object_type_id, effect, normalized_row)
            .await;
        tx.rollback().await.unwrap();
        forged
    }

    /// [`Self::forge_attach`] that COMMITS on success.
    ///
    /// Rolling back is right for a probe that only reads the refusal and useless
    /// for one that has to read the forged row back over HTTP afterwards. Same
    /// role, same armed org, same statement — only the disposition differs.
    async fn forge_attach_persisted(
        &self,
        object_type_id: Uuid,
        effect: &str,
        normalized_row: Value,
    ) -> Result<Uuid, String> {
        let mut tx = self.armed_tx(&self.command_pool).await;
        let forged = self
            .attach_via_definer(&mut tx, object_type_id, effect, normalized_row)
            .await;
        if forged.is_ok() {
            tx.commit().await.unwrap();
        } else {
            tx.rollback().await.unwrap();
        }
        forged
    }

    /// The 8-argument audited entrypoint, with this fixture's org and actor and this
    /// file's probe trace pair. ONE statement, shared by the rolled-back and the
    /// committed probe above, so the two cannot drift apart in what they call —
    /// which is the same reason 0206 renames 0205's body instead of copying it.
    async fn attach_via_definer(
        &self,
        tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        object_type_id: Uuid,
        effect: &str,
        normalized_row: Value,
    ) -> Result<Uuid, String> {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT ont_policy_api.attach_object_policy($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(*self.org.as_uuid())
        .bind(*self.actor.as_uuid())
        .bind(object_type_id)
        .bind(effect)
        .bind(normalized_row)
        .bind("ontology-runtime-filter-v1")
        .bind(PROBE_TRACE_ID)
        .bind(PROBE_SPAN_ID)
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| error.to_string())
    }

    async fn get(&self, uri: &str) -> HttpResponse {
        self.request("GET", uri, Some(&self.token), Value::Null)
            .await
    }

    async fn get_as(&self, uri: &str, token: &str) -> HttpResponse {
        self.request("GET", uri, Some(token), Value::Null).await
    }

    async fn post(&self, uri: &str, body: Value) -> HttpResponse {
        self.request("POST", uri, Some(&self.token), body).await
    }

    /// An authenticated call carrying the optimistic-concurrency `If-Match` the
    /// object-type write routes require.
    async fn send(
        &self,
        method: &str,
        uri: &str,
        if_match: Option<&str>,
        body: Value,
    ) -> HttpResponse {
        self.request_inner(method, uri, Some(&self.token), if_match, body)
            .await
    }

    async fn request(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Value,
    ) -> HttpResponse {
        self.request_inner(method, uri, token, None, body).await
    }

    async fn request_inner(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        if_match: Option<&str>,
        body: Value,
    ) -> HttpResponse {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(if_match) = if_match {
            builder = builder.header(header::IF_MATCH, if_match);
        }
        if body != Value::Null {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        let body = if body == Value::Null {
            Body::empty()
        } else {
            Body::from(serde_json::to_vec(&body).unwrap())
        };
        let response = self
            .service
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        HttpResponse {
            status,
            etag,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }
}

struct HttpResponse {
    status: StatusCode,
    etag: Option<String>,
    body: Value,
}

impl HttpResponse {
    fn etag(&self) -> String {
        self.etag
            .clone()
            .unwrap_or_else(|| panic!("response carried no ETag: {} {:?}", self.status, self.body))
    }
}

struct TestAuth {
    token: String,
    verifier: JwtVerifier,
}

thread_local! {
    /// One key per test thread so a second principal's token verifies against
    /// the router's single verifier.
    static SIGNING_KEY: SigningKey = SigningKey::random(&mut OsRng);
}

/// The issuer's and the verifier's settings must be the SAME settings, or every
/// token this file mints is rejected by the router it is aimed at.
fn settings() -> JwtSettings {
    JwtSettings {
        issuer: TEST_ISSUER.to_owned(),
        audience: TEST_AUDIENCE.to_owned(),
        access_token_ttl: Duration::minutes(15),
    }
}

fn test_auth(user_id: UserId, org: OrgId) -> TestAuth {
    let signing_key = SIGNING_KEY.with(SigningKey::clone);
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let verifier = JwtVerifier::from_es256_public_pem(settings(), public_pem.as_bytes()).unwrap();
    TestAuth {
        token: issue_token(&signing_key, user_id, org),
        verifier,
    }
}

fn issue_token(signing_key: &SigningKey, user_id: UserId, org: OrgId) -> String {
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    JwtIssuer::from_es256_pem(settings(), private_pem.as_bytes(), public_pem.as_bytes())
        .unwrap()
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

// ---------------------------------------------------------------------------
// Drafts and blocks
// ---------------------------------------------------------------------------

fn owner_property() -> Value {
    json!({
        "key": "owner", "title": "Owner", "field_type": "text", "config": {},
        "backing_column": null, "required": true, "in_property_policy": false
    })
}

fn code_property() -> Value {
    json!({
        "key": "code", "title": "Code", "field_type": "text", "config": {},
        "backing_column": null, "required": false, "in_property_policy": false
    })
}

fn instance_type_draft(stable_key: &str) -> Value {
    json!({
        "stable_key": stable_key,
        "title": "Policy attach case",
        "backing_kind": "instance",
        "properties": [owner_property(), code_property()],
        "links": [],
        "actions": [],
        "analytics": []
    })
}

/// The SAME shape as [`instance_type_draft`], staged as schema_version 2. The
/// properties are unchanged on purpose: the only variable under test is the
/// object-type VERSION id an instance is filed under.
fn instance_type_draft_v2(stable_key: &str) -> Value {
    json!({
        "stable_key": stable_key,
        "title": "Policy attach case v2",
        "backing_kind": "instance",
        "properties": [owner_property(), code_property()],
        "links": [],
        "actions": [],
        "analytics": []
    })
}

/// The SAME shape as [`instance_type_draft`] plus ONE `instance_revision` action
/// that edits `code` from a param — the minimum a write path needs to reach a
/// policy-hidden row through `prepare()`.
fn editable_instance_type_draft(stable_key: &str) -> Value {
    json!({
        "stable_key": stable_key,
        "title": "Policy attach case with an action",
        "backing_kind": "instance",
        "properties": [owner_property(), code_property()],
        "links": [],
        "actions": [{
            "stable_key": "set_code",
            "title": "Set code",
            "params_schema": { "code": { "required": true } },
            "edits": [{ "property": "code", "param": "code" }],
            "submission_criteria": [],
            "side_effects": [],
            "dispatch": "instance_revision",
            "dispatch_target": null,
            "control_points": ["authority"]
        }],
        "analytics": []
    })
}

fn linked_instance_type_draft(stable_key: &str) -> Value {
    json!({
        "stable_key": stable_key,
        "title": "Policy attach case with a link",
        "backing_kind": "instance",
        "properties": [owner_property(), code_property()],
        "links": [{
            "stable_key": "related",
            "title": "Related",
            "reverse_title": null,
            "to_object_type_id": null,
            "cardinality": "many_many",
            "traversable": true
        }],
        "actions": [],
        "analytics": []
    })
}

/// A type whose only non-`owner` property is a Date: every condition over it is
/// unrepresentable in `declared` and must be refused at attach.
fn dated_type_draft(stable_key: &str) -> Value {
    json!({
        "stable_key": stable_key,
        "title": "Policy attach case with a date",
        "backing_kind": "instance",
        "properties": [owner_property(), code_property(), {
            "key": "due_date", "title": "Due date", "field_type": "date", "config": {},
            "backing_column": null, "required": false, "in_property_policy": false
        }],
        "links": [],
        "actions": [],
        "analytics": []
    })
}

fn owner_is_subject() -> Value {
    json!({
        "attr": "owner",
        "op": "eq",
        "value": { "kind": "subject_attr", "value": "user_id" }
    })
}

/// Cedar source the definer can neither derive nor re-validate: rendering it in
/// SQL would be a second copy of `generate_cedar_text_with` in a migration, which
/// is the divergence that rots. It is stored verbatim, which is the whole reason
/// the org-wide set has to stop reading attached rows.
const ARBITRARY_POLICY_TEXT: &str = "permit(principal, action == Action::\"view\", resource);";

/// The canonical `normalized_row` envelope the authoring validator emits
/// (`authz/src/cedar_pbac/authoring.rs:429-436`): exactly four keys, and the
/// three scalars must agree with the attachment and with the object type.
fn canonical_normalized_row(effect: &str, resource_type: &str, conditions: Vec<Value>) -> Value {
    json!({
        "effect": effect,
        "action": "view",
        "resource_type": resource_type,
        "conditions": conditions
    })
}

/// The SQLSTATE **and** the message of a refusal, because in this file neither
/// alone discriminates: `42501` is raised alike for a missing function grant, a
/// missing schema USAGE and a missing table grant, from the identical internal site
/// (`aclcheck_error, aclchk.c:2795`). Which one it was lives only in the message, so
/// every caller asserts on both — each with the measurement that forced it recorded
/// at the call site.
fn refusal(error: sqlx::Error) -> (String, String) {
    let code = error
        .as_database_error()
        .and_then(|db| db.code().map(|code| code.into_owned()))
        .unwrap_or_default();
    (code, error.to_string())
}

/// A refusal must name the violation it refused. `is_err()` alone is satisfied by
/// a definer that does not exist at this arity, by a typo, and by an unrelated
/// fault — the vacuous green this file exists to make impossible.
fn assert_forged_attach_refused(outcome: &Result<Uuid, String>, expected: &str, why: &str) {
    match outcome {
        Ok(id) => panic!(
            "the definer ACCEPTED a forgery the route would have rejected \
             (catalog id {id}); {why}"
        ),
        Err(error) => assert!(
            error.contains(expected),
            "the definer must refuse this naming `{expected}`; {why}. Got: {error}"
        ),
    }
}

/// Raw-SQL attachment, used ONLY by the defect-(a) test so that it is red against
/// `main` with no writer present, and so it also covers policies attached by the
/// policy studio rather than by this route.
async fn attach_enforced_policy(
    owner_pool: &PgPool,
    org: OrgId,
    type_id: ObjectTypeId,
    stable_key: &str,
    effect: &str,
    blocks: Value,
) {
    let policy_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO cedar_policy_catalog_entries
            (org_id, stable_key, title, natural_language_rule, effect, status, source,
             principal, action, resource, conditions, policy_version, schema_version,
             bundle_digest, validation_status, normalized_row, generated_policy_text)
        VALUES ($1, $2, $3, 'object-policy attach fixture', $4, 'enforced', 'imported_fixture',
                '{}'::jsonb, '{}'::jsonb, '{}'::jsonb, '[]'::jsonb, 1,
                'ontology-runtime-filter-v1',
                'sha256:0000000000000000000000000000000000000000000000000000000000000000',
                'valid', $5, 'permit(principal, action, resource);')
        RETURNING id
        "#,
    )
    .bind(*org.as_uuid())
    .bind(stable_key)
    .bind(format!("Policy {stable_key}"))
    .bind(effect)
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
    .bind(effect)
    // rls-arming: test fixture attaches the policy as DB owner before the
    // runtime-role request makes the read decision.
    .execute(owner_pool)
    .await
    .expect("attach enforced object policy");
}

fn assert_instance_titles(body: &Value, expected: &[&str]) {
    let titles = body
        .as_array()
        .unwrap_or_else(|| panic!("instance list must be an array, got {body}"))
        .iter()
        .map(|instance| instance["instance"]["title"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(titles, expected);
}

fn body_text(body: &Value) -> String {
    serde_json::to_string(body).expect("response body is serializable")
}
