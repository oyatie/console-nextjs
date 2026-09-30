//! LANE-EDITABLE — this type's `declare`, and its param bags.
//!
//! Target contract: the bag MUST carry `parent_org_unit_id`, set to the parent's
//! `InstanceId` string, or JSON null for the root. The type's
//! `parent_org_unit_id` property therefore MUST be declared `required: false` —
//! `validate_params` rejects a missing required param, and the root has no
//! parent.

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

pub fn unit(name: &str, parent: Option<InstanceId>) -> Value {
    json!({
        "name": name,
        "parent_org_unit_id": parent.map(|id| id.to_string()),
    })
}

/// Author and publish `org_unit` — the 4-step publish, run inside
/// `scope_org(h.org, ..)` against `h.registry()` (which carries the command pool;
/// without it every registry mutation is `CommandUnavailable`). `declare_all`
/// runs at the tail of `Harness::bootstrap`, OUTSIDE bootstrap's own scope block.
///
/// Four-eyes is enforced three ways, so `h.admin` requests and `h.approver`
/// decides: `gov_approvals CHECK (approver_id <> requested_by)`
/// (`0153_create_governance.sql:74`), `decide_approval`'s own check, and the
/// publish gate's `ga.requested_by = p_actor`. Approvals are single-use (a
/// consumption row is inserted), so the `request_ref` is fresh per lane.
pub async fn declare(h: &Harness) {
    scope_org(h.org, async {
        let store = h.registry();
        let governance = PgGovernanceStore::new(h.runtime_pool.clone());

        let created = store
            .create_object_type(h.admin, draft(), TraceContext::generate(), AT)
            .await
            .expect("org_unit draft must be created");
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
            .expect("org_unit draft must enter review");

        // `reviewed`, never `created`: the key-revision ladder is 1 -> 2 -> 3, and
        // the publish gate compares `payload_summary.key_revision` against the
        // CURRENT revision (`0165_ontology_object_type_key_revisions.sql:1003`).
        // `created`'s 1 raises 42501 `publish_approval_required`, which reads like
        // a governance bug but is an off-by-one. The same 2 is the publish CAS.
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
            .expect("org_unit publication approval must be requested");
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
            .expect("a distinct reviewer must approve org_unit publication");

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
            .expect("reviewed and approved org_unit must publish");

        // `get_object_type` orders `(lifecycle_state = 'published') DESC` with NO
        // lifecycle filter, so a stuck draft still RESOLVES and would turn a green
        // the engine cannot act on.
        assert_eq!(
            published.lifecycle_state,
            SchemaLifecycleState::Published,
            "org_unit must reach Published — a draft still resolves"
        );
    })
    .await;
}

/// The reference declaration the other four lanes transliterate.
///
/// The property -> link binding is DECLARATIVE, carried in
/// `ont_property_defs.config` and resolved generically at revision writeback
/// (`adapter-postgres/src/instances.rs` `sync_property_links_tx`). No per-type
/// code exists anywhere in the engine, so landing a lane never edits a shared
/// file. `to_type` is a `stable_key` STRING and never an id: `ont_object_types`
/// is one row per `(org, stable_key, schema_version)`
/// (`0152_create_ontology_registry.sql:32`), so id equality breaks the moment a
/// type reaches version N+1.
fn draft() -> CreateObjectTypeDraft {
    CreateObjectTypeDraft {
        stable_key: "org_unit".to_owned(),
        title: "조직 단위".to_owned(),
        title_property_key: Some("name".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        properties: vec![
            PropertyDefInput {
                key: "name".to_owned(),
                title: "조직명".to_owned(),
                field_type: "text".to_owned(),
                config: json!({}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
            PropertyDefInput {
                key: "parent_org_unit_id".to_owned(),
                title: "상위 조직".to_owned(),
                field_type: "reference".to_owned(),
                config: json!({"link": {"stable_key": "parent_org_unit", "to_type": "org_unit"}}),
                backing_column: None,
                // The root has no parent and sends explicit JSON null;
                // `validate_params` rejects a null for a REQUIRED param.
                required: false,
                in_property_policy: false,
            },
        ],
        // Self-referential, so `to_object_type_id` MUST be None: the object type's
        // version id is generated inside `ontology_api.create_object_type`
        // (`0165_ontology_object_type_key_revisions.sql:773`), after the Rust
        // caller has already serialised this snapshot. `0152:76` documents NULL as
        // "unresolved".
        links: vec![LinkTypeInput {
            stable_key: "parent_org_unit".to_owned(),
            title: "상위 조직".to_owned(),
            reverse_title: Some("하위 조직".to_owned()),
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
