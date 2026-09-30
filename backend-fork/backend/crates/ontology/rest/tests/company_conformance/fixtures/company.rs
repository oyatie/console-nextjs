//! LANE-EDITABLE — this type's `declare`, and its param bags.
//!
//! Target contract: the bag MUST carry `legal_name` (the target uses it as the
//! instance title and asserts `head.title == legal_name`).
//!
//! Transliterated from `org_unit`, the reference. The only differences are the
//! stable key, the property list, and the absence of links — `company` is the
//! root of the hierarchy and points at nothing. Neither type contributes a line
//! of per-type code to the engine, which is the property the fan-out rests on:
//! landing this lane edits this file and no other.

use crate::harness::{AT, Harness};
use console_governance_adapter_postgres::PgGovernanceStore;
use console_governance_application::{
    ApprovalDecision, CreateApprovalCommand, DecideApprovalCommand,
};
use console_kernel_core::TraceContext;
use console_ontology_adapter_postgres::{CreateObjectTypeDraft, PropertyDefInput};
use console_ontology_domain::{BackingKind, SchemaLifecycleState};
use console_platform_request_context::scope_org;
use serde_json::{Value, json};
use uuid::Uuid;

pub fn found(legal_name: &str) -> Value {
    json!({
        "legal_name": legal_name,
        "registration_number": "123-45-67890",
        "founded_on": "2026-07-10",
    })
}

pub async fn declare(h: &Harness) {
    scope_org(h.org, async {
        let store = h.registry();
        let governance = PgGovernanceStore::new(h.runtime_pool.clone());

        let created = store
            .create_object_type(h.admin, draft(), TraceContext::generate(), AT)
            .await
            .expect("company draft must be created");
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
            .expect("company draft must enter review");

        // `reviewed`, never `created`: the key-revision ladder is 1 -> 2 -> 3, and
        // the publish gate compares `payload_summary.key_revision` against the
        // CURRENT revision (`0165_ontology_object_type_key_revisions.sql:1003`).
        // `created`'s 1 raises 42501 `publish_approval_required`, which reads like
        // a governance bug but is an off-by-one. The same 2 is the publish CAS.
        //
        // Its own `request_ref`: an approval is single-use, consumed by the publish
        // that spends it, so a ref shared between lanes fails the SECOND publish
        // rather than this one — a failure that would land in someone else's lane.
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
            .expect("company publication approval must be requested");
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
            .expect("a distinct reviewer must approve company publication");

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
            .expect("reviewed and approved company must publish");

        // `get_object_type` orders `(lifecycle_state = 'published') DESC` with NO
        // lifecycle filter, so a stuck draft still RESOLVES and would turn a green
        // the engine cannot act on.
        assert_eq!(
            published.lifecycle_state,
            SchemaLifecycleState::Published,
            "company must reach Published — a draft still resolves"
        );
    })
    .await;
}

/// Every property [`found`] sends, and nothing else. `validate_params` rejects an
/// undeclared key and the target asserts each sent param round-trips verbatim, so
/// the declaration and the bag are two halves of one contract.
///
/// All three are `required: true` because [`found`] always sends all three.
/// `org_unit`'s `parent_org_unit_id` is optional only because its ROOT genuinely
/// has no parent; nothing here is ever absent, and a needlessly optional param
/// would let a future omission through as a silent JSON null.
fn draft() -> CreateObjectTypeDraft {
    CreateObjectTypeDraft {
        stable_key: "company".to_owned(),
        title: "법인".to_owned(),
        title_property_key: Some("legal_name".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        properties: vec![
            PropertyDefInput {
                key: "legal_name".to_owned(),
                title: "법인명".to_owned(),
                field_type: "text".to_owned(),
                config: json!({}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
            PropertyDefInput {
                key: "registration_number".to_owned(),
                title: "사업자등록번호".to_owned(),
                field_type: "text".to_owned(),
                config: json!({}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
            PropertyDefInput {
                key: "founded_on".to_owned(),
                title: "설립일".to_owned(),
                // `date`, not `text`. `check_field_shape` accepts a string for
                // both (`instances.rs:1448-1454`), so the bag is unchanged either
                // way — but the declared kind is what a reader, an exporter, and
                // any future validator see, and this is a date.
                field_type: "date".to_owned(),
                config: json!({}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
        ],
        // NONE. `company` is the root: `org_unit` points AT it, never the reverse.
        // An unreferenced link type here would be inert at best, and if a property
        // ever bound to it, would invert the direction `hop()` walks.
        links: Vec::new(),
        // NONE. Publishing auto-attaches the generic `create` action only while no
        // `instance_revision`-dispatch action exists
        // (`0165_ontology_object_type_key_revisions.sql:1024-1029`); one
        // hand-authored action suppresses it and `execute_action("create", ..)`
        // then 404s, byte-similar to the unbuilt-type red.
        actions: Vec::new(),
        analytics: Vec::new(),
    }
}
