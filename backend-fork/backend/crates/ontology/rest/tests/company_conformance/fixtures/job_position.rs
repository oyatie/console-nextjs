//! LANE-EDITABLE — this type's `declare`, and its param bags.
//!
//! Target contract: the bag MUST carry `org_unit_id` (an `InstanceId` string) and
//! `headcount` (a JSON integer — the target asserts `.as_i64()`, so the property
//! must be declared `integer`).
//!
//! CC-03 also HOPS out of a position and requires its org unit to be reachable,
//! so `org_unit_id` carries a `config.link` binding exactly as `org_unit`'s own
//! `parent_org_unit_id` does. The only difference is that this link crosses
//! types: `to_type` is `org_unit`, not `job_position`. That is the whole cost of
//! a cross-type reference — the resolver is generic, so there is no engine change.

use crate::harness::{AT, Harness};
use console_governance_adapter_postgres::PgGovernanceStore;
use console_governance_application::{
    ApprovalDecision, CreateApprovalCommand, DecideApprovalCommand,
};
use console_kernel_core::TraceContext;
use console_ontology_adapter_postgres::{CreateObjectTypeDraft, LinkTypeInput, PropertyDefInput};
use console_ontology_domain::{BackingKind, InstanceId, LinkCardinality, SchemaLifecycleState};
use console_platform_request_context::scope_org;
use serde_json::{Value, json};
use uuid::Uuid;

pub fn position(title: &str, org_unit: InstanceId, headcount: i64) -> Value {
    json!({
        "job_title": title,
        "org_unit_id": org_unit.to_string(),
        "headcount": headcount,
    })
}

pub async fn declare(h: &Harness) {
    scope_org(h.org, async {
        let store = h.registry();
        let governance = PgGovernanceStore::new(h.runtime_pool.clone());

        let created = store
            .create_object_type(h.admin, draft(), TraceContext::generate(), AT)
            .await
            .expect("job_position draft must be created");
        let reviewed = store
            .transition_lifecycle(
                h.admin,
                created.id,
                created.write_precondition(),
                SchemaLifecycleState::ReviewPending,
                true,
                TraceContext::generate(),
                AT,
            )
            .await
            .expect("job_position draft must enter review");

        // `reviewed`, never `created`: the key-revision ladder is 1 -> 2 -> 3 and the
        // publish gate compares `payload_summary.key_revision` against the CURRENT
        // revision (`0165_ontology_object_type_key_revisions.sql:1003`). `created`'s
        // 1 raises 42501 `publish_approval_required`, which reads like a governance
        // bug but is an off-by-one. The same 2 is the publish CAS.
        //
        // Its own `request_ref`: an approval is single-use, consumed by the publish
        // that spends it, so a ref shared between lanes fails the SECOND publish.
        let request_ref = Uuid::new_v4();
        governance
            .create_approval(CreateApprovalCommand {
                requester: h.admin,
                request_ref,
                kind: "ontology.schema.publish".to_owned(),
                target_ref: Some(*created.id.as_uuid()),
                payload_summary: json!({"key_revision": reviewed.key_write_revision}),
                trace: TraceContext::generate(),
                occurred_at: AT,
            })
            .await
            .expect("job_position publication approval must be requested");
        governance
            .decide_approval(DecideApprovalCommand {
                approver: h.approver,
                request_ref,
                kind: "ontology.schema.publish".to_owned(),
                requested_by: h.admin,
                target_ref: Some(*created.id.as_uuid()),
                decision: ApprovalDecision::Approved,
                trace: TraceContext::generate(),
                occurred_at: AT,
            })
            .await
            .expect("a distinct reviewer must approve job_position publication");

        let published = store
            .transition_lifecycle(
                h.admin,
                created.id,
                reviewed.write_precondition(),
                SchemaLifecycleState::Published,
                true,
                TraceContext::generate(),
                AT,
            )
            .await
            .expect("reviewed and approved job_position must publish");

        // `get_object_type` orders `(lifecycle_state = 'published') DESC` with NO
        // lifecycle filter, so a stuck draft still RESOLVES and would turn a green
        // the engine cannot act on.
        assert_eq!(
            published.lifecycle_state,
            SchemaLifecycleState::Published,
            "job_position must reach Published — a draft still resolves"
        );
    })
    .await;
}

/// Every property [`position`] sends, and nothing else. `validate_params` rejects
/// an undeclared key and the target asserts each sent param round-trips verbatim.
///
/// All three are `required: true`: [`position`] always sends all three, and unlike
/// `org_unit`'s optional parent there is no position without an org unit.
fn draft() -> CreateObjectTypeDraft {
    CreateObjectTypeDraft {
        stable_key: "job_position".to_owned(),
        title: "직무".to_owned(),
        title_property_key: Some("job_title".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        properties: vec![
            PropertyDefInput {
                key: "job_title".to_owned(),
                title: "직무명".to_owned(),
                field_type: "text".to_owned(),
                config: json!({}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
            PropertyDefInput {
                key: "org_unit_id".to_owned(),
                title: "소속 조직".to_owned(),
                field_type: "reference".to_owned(),
                // The same declarative binding `org_unit` uses, pointing at a
                // DIFFERENT type. `to_type` is a `stable_key` string and never an
                // id: `ont_object_types` is one row per
                // `(org, stable_key, schema_version)`
                // (`0152_create_ontology_registry.sql:32`), so id equality would
                // break the moment `org_unit` reaches version N+1 — which a lane
                // editing its own type would cause without ever touching this file.
                config: json!({"link": {"stable_key": "position_org_unit", "to_type": "org_unit"}}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
            PropertyDefInput {
                key: "headcount".to_owned(),
                title: "정원".to_owned(),
                // `integer`, because CC-03 asserts `.as_i64()`. Declaring it `text`
                // would still round-trip the JSON number and fail only on that one
                // assertion.
                field_type: "integer".to_owned(),
                config: json!({}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
        ],
        // Cross-type, so `to_object_type_id` COULD be resolved here — `org_unit`
        // already exists by the time this runs. It is deliberately left None
        // anyway: the id is per-VERSION, so a resolved id would silently go stale
        // when `org_unit` is next revised, while `config.link.to_type` is a stable
        // key that does not. `0152:76` documents NULL as "unresolved".
        links: vec![LinkTypeInput {
            stable_key: "position_org_unit".to_owned(),
            title: "소속 조직".to_owned(),
            reverse_title: Some("직무".to_owned()),
            to_object_type_id: None,
            cardinality: LinkCardinality::OneMany,
            traversable: true,
        }],
        // NONE. Publishing auto-attaches the generic `create` action only while no
        // `instance_revision`-dispatch action exists
        // (`0165_ontology_object_type_key_revisions.sql:1024-1029`); one
        // hand-authored action suppresses it and `execute_action("create", ..)`
        // then 404s, byte-similar to the unbuilt-type red.
        actions: Vec::new(),
        analytics: Vec::new(),
    }
}
