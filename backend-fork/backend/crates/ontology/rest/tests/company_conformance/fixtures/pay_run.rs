//! LANE-EDITABLE — this type's `declare`, and its param bags.
//!
//! Target contract: the bag carries `employment_ids` (a JSON array of
//! `InstanceId` strings — declare the property `multi_choice` or `json`, since
//! `check_field_shape` requires an array for `multi_choice` and accepts anything
//! for `json`) plus the period bounds.
//!
//! IT MUST NOT CARRY THE PAY TOTAL. `gross_total` is DERIVED from the referenced
//! employments' `base_salary`; the target asserts the computed value and REJECTS
//! a bag that sends it (CC-10). Sending it is the one edit that would turn a
//! derivation assertion back into a round-trip assertion — exactly the vacuity
//! this suite exists to prevent.

use crate::harness::{AT, Harness};
use console_governance_adapter_postgres::PgGovernanceStore;
use console_governance_application::{
    ApprovalDecision, CreateApprovalCommand, DecideApprovalCommand,
};
use console_kernel_core::TraceContext;
use console_ontology_adapter_postgres::{CreateObjectTypeDraft, PropertyDefInput};
use console_ontology_domain::{BackingKind, InstanceId, SchemaLifecycleState};
use console_platform_request_context::scope_org;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

pub fn cycle(
    employments: &[InstanceId],
    period_start: OffsetDateTime,
    period_end: OffsetDateTime,
) -> Value {
    json!({
        "employment_ids": employments.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "period_start": period_start.format(&Rfc3339).unwrap_or_default(),
        "period_end": period_end.format(&Rfc3339).unwrap_or_default(),
    })
}

/// Author and publish `pay_run` — the 4-step publish, transliterated from
/// `org_unit::declare` with a fresh `request_ref` (approvals are single-use).
///
/// `reviewed`, never `created`: the publish gate compares
/// `payload_summary.key_revision` against the CURRENT revision
/// (`0165_ontology_object_type_key_revisions.sql:1003`), and `created`'s 1 raises
/// 42501 `publish_approval_required`, which reads like a governance bug but is an
/// off-by-one.
pub async fn declare(h: &Harness) {
    scope_org(h.org, async {
        let store = h.registry();
        let governance = PgGovernanceStore::new(h.runtime_pool.clone());

        let created = store
            .create_object_type(h.admin, draft(), TraceContext::generate(), AT)
            .await
            .expect("pay_run draft must be created");
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
            .expect("pay_run draft must enter review");

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
            .expect("pay_run publication approval must be requested");
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
            .expect("a distinct reviewer must approve pay_run publication");

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
            .expect("reviewed and approved pay_run must publish");

        assert_eq!(
            published.lifecycle_state,
            SchemaLifecycleState::Published,
            "pay_run must reach Published — a draft still resolves"
        );
    })
    .await;
}

/// `gross_total` is the whole point of this type: it is DECLARED here and
/// COMPUTED by the generic resolver in `adapter-postgres/src/instances.rs`
/// (`resolve_derived_attributes_tx`), the sibling of the property -> link
/// binding. No engine code names `pay_run`; the arithmetic is data this
/// declaration carries.
fn draft() -> CreateObjectTypeDraft {
    CreateObjectTypeDraft {
        stable_key: "pay_run".to_owned(),
        title: "급여 지급".to_owned(),
        title_property_key: None,
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        properties: vec![
            // `multi_choice`, never `json` or `reference`: `check_field_shape`
            // requires `is_string()` for a reference (the bag sends an array) and
            // accepts literally anything for `json`, so `multi_choice`
            // (`is_array()`) is the only declaration that validates the shape.
            PropertyDefInput {
                key: "employment_ids".to_owned(),
                title: "대상 재직".to_owned(),
                field_type: "multi_choice".to_owned(),
                // NO `link`. CC-10 asserts the ids round-trip and the total
                // computes; it never traverses OUT of a pay run, and a link would
                // need a `LinkTypeInput` plus N `ont_links` rows nothing reads.
                // The referential guarantee CC-10 does need — each id exists and
                // is an `employment` — is delivered by the derivation's own
                // `to_type` check, which it must make anyway for the sum to mean
                // anything.
                config: json!({}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
            PropertyDefInput {
                key: "period_start".to_owned(),
                title: "기간 시작".to_owned(),
                field_type: "timestamp".to_owned(),
                config: json!({}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
            PropertyDefInput {
                key: "period_end".to_owned(),
                title: "기간 종료".to_owned(),
                field_type: "timestamp".to_owned(),
                config: json!({}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            },
            PropertyDefInput {
                key: "gross_total".to_owned(),
                title: "총 지급액".to_owned(),
                // `integer`, so `check_field_shape` validates the resolver's own
                // output and CC-10's `as_i64()` succeeds — a float sum would fail
                // that assertion even with correct arithmetic.
                field_type: "integer".to_owned(),
                config: json!({
                    "derive": {
                        "op": "sum",
                        "over": "employment_ids",
                        "of": "base_salary",
                        "to_type": "employment",
                    }
                }),
                backing_column: None,
                // MUST be false. The publish auto-attaches a `create` action whose
                // `params_schema` mirrors `required` (`0165:1030`), and
                // `validate_params` would reject the bag with "required param
                // 'gross_total' is missing" before anything could derive — the
                // single most likely way to ship a type that 400s on every create.
                required: false,
                in_property_policy: false,
            },
        ],
        links: Vec::new(),
        // NONE: a hand-authored action suppresses the auto-attached generic
        // `create` (`0165:1024-1029`).
        actions: Vec::new(),
        analytics: Vec::new(),
    }
}
