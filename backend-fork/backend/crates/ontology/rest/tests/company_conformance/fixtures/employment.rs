//! LANE-EDITABLE — this type's `declare`, and its param bags.
//!
//! Target contract:
//!   * `job_position_id` and `org_unit_id` are `InstanceId` strings. The target
//!     does NOT accept them as mere attributes: it walks the engine's traversal
//!     surface and asserts the employment actually LINKS to that position
//!     (CC-04) and, after the transfer, to the NEW unit only (CC-11).
//!   * `base_salary` is a JSON integer. The target NEVER sends a pay total — it
//!     sums these and asserts the pay run DERIVED the same number (CC-10), so
//!     the two people must carry DIFFERENT non-zero salaries.
//!   * This same bag is reused for the TRANSFER (CC-06): a full param bag on
//!     every revision, because `apply_edits` nulls any property whose param is
//!     absent (`ontology/application/src/lib.rs:288,296`).

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

/// Base salary per person. Distinct on purpose: CC-10 runs the pay cycle twice
/// over different populations, so no constant can satisfy both totals.
pub fn base_salary(person: &str) -> i64 {
    match person {
        "김정비" => 3_600_000,
        _ => 4_200_000,
    }
}

pub fn hire(person: &str, job_position: InstanceId, org_unit: InstanceId) -> Value {
    json!({
        "person_name": person,
        "job_position_id": job_position.to_string(),
        "org_unit_id": org_unit.to_string(),
        "base_salary": base_salary(person),
    })
}

pub async fn declare(h: &Harness) {
    scope_org(h.org, async {
        let store = h.registry();
        let governance = PgGovernanceStore::new(h.runtime_pool.clone());

        let created = store
            .create_object_type(h.admin, draft(), TraceContext::generate(), AT)
            .await
            .expect("employment draft must be created");
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
            .expect("employment draft must enter review");

        // `reviewed`, never `created`: the key-revision ladder is 1 -> 2 -> 3 and the
        // publish gate compares `payload_summary.key_revision` against the CURRENT
        // revision (`0165_ontology_object_type_key_revisions.sql:1003`). `created`'s
        // 1 raises 42501 `publish_approval_required`, an off-by-one that reads like
        // a governance bug. The same 2 is the publish CAS.
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
            .expect("employment publication approval must be requested");
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
            .expect("a distinct reviewer must approve employment publication");

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
            .expect("reviewed and approved employment must publish");

        // `get_object_type` orders `(lifecycle_state = 'published') DESC` with NO
        // lifecycle filter, so a stuck draft still RESOLVES and would turn a green
        // the engine cannot act on.
        assert_eq!(
            published.lifecycle_state,
            SchemaLifecycleState::Published,
            "employment must reach Published — a draft still resolves"
        );
    })
    .await;
}

/// The first type in this fan-out to declare TWO link properties, which makes it
/// the case the link resolver's no-churn rule exists for. CC-06 transfers a person
/// between org units WITHOUT changing their position, and the resolver acts on the
/// difference: the `org_unit` edge closes and reopens, the `job_position` edge is
/// left untouched. An implementation that swept both would produce an identical
/// graph — `traverse` reads `valid_to IS NULL` — while writing a position change
/// into `ont_links` history that never happened.
///
/// Every property [`hire`] sends and nothing else; all `required: true`, since the
/// same full bag is re-sent on the transfer revision (`apply_edits` nulls any
/// property whose param is absent).
fn draft() -> CreateObjectTypeDraft {
    CreateObjectTypeDraft {
        stable_key: "employment".to_owned(),
        title: "재직".to_owned(),
        title_property_key: Some("person_name".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        properties: vec![
            PropertyDefInput {
                key: "person_name".to_owned(),
                title: "성명".to_owned(),
                field_type: "text".to_owned(),
                config: json!({}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
            PropertyDefInput {
                key: "job_position_id".to_owned(),
                title: "직무".to_owned(),
                field_type: "reference".to_owned(),
                config: json!({
                    "link": {"stable_key": "employment_position", "to_type": "job_position"}
                }),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
            PropertyDefInput {
                key: "org_unit_id".to_owned(),
                title: "소속 조직".to_owned(),
                field_type: "reference".to_owned(),
                config: json!({
                    "link": {"stable_key": "employment_org_unit", "to_type": "org_unit"}
                }),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
            PropertyDefInput {
                key: "base_salary".to_owned(),
                title: "기본급".to_owned(),
                // `integer`: CC-10 sums these with `.as_i64()` and asserts the pay
                // run DERIVED the same number. A `decimal` would satisfy the
                // round-trip and fail the derivation on a type mismatch.
                field_type: "integer".to_owned(),
                config: json!({}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
        ],
        // Two link types, one per reference property. Both leave
        // `to_object_type_id` unresolved: the id is per-VERSION, so pinning it
        // would go stale the next time the job_position or org_unit lane revised
        // its own type — silently, in a file this lane never opened. `0152:76`
        // documents NULL as "unresolved"; `config.link.to_type` carries the stable
        // key that does not rot.
        links: vec![
            LinkTypeInput {
                stable_key: "employment_position".to_owned(),
                title: "직무".to_owned(),
                reverse_title: Some("재직자".to_owned()),
                to_object_type_id: None,
                cardinality: LinkCardinality::OneMany,
                traversable: true,
            },
            LinkTypeInput {
                stable_key: "employment_org_unit".to_owned(),
                title: "소속 조직".to_owned(),
                reverse_title: Some("재직자".to_owned()),
                to_object_type_id: None,
                cardinality: LinkCardinality::OneMany,
                traversable: true,
            },
        ],
        // NONE. Publishing auto-attaches the generic `create` action only while no
        // `instance_revision`-dispatch action exists
        // (`0165_ontology_object_type_key_revisions.sql:1024-1029`); one
        // hand-authored action suppresses it and `execute_action("create", ..)`
        // then 404s, byte-similar to the unbuilt-type red.
        actions: Vec::new(),
        analytics: Vec::new(),
    }
}
