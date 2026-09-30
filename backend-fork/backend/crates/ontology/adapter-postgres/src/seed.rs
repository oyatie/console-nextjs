//! Governed console config object types, seeded THROUGH the ontology engine.
//!
//! The console's SLO settings (§4-26) and dashboard/table layouts (§19) are not
//! bespoke stores — they are ordinary `instance`-backed object types in the §18
//! registry, so they get lifecycle, revision staging (§3.9.0), fixity, RLS and
//! audit for free from the one engine. This module only builds the
//! [`CreateObjectTypeDraft`]s and submits one tenant-independent manifest to the
//! database-owned built-in catalog installer. PostgreSQL accepts only the
//! migration-allowlisted catalog version and digest, resolves stable-key links,
//! and publishes the complete catalog atomically. Human authoring remains on the
//! separate reviewed lifecycle path; bootstrap has no protection flag, GUC, or
//! fake-approval escape hatch.
//!
//! Each type ships a generic `create` action (`instance_revision` dispatch) whose
//! `edits` copy each declared property from a same-named param. That is the path
//! the console's `/actions/{key}/execute` uses to create an instance (there is no
//! direct POST /instances) and, with `instance_id` supplied, to stage a v+1
//! revision.

use std::collections::HashMap;

use console_kernel_core::{KernelError, TraceContext, UserId};
use console_ontology_domain::{
    ActionDispatch, BackingKind, LinkCardinality, ObjectTypeId, SchemaLifecycleState,
};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ActionTypeInput, CreateObjectTypeDraft, LinkTypeInput, ObjectTypeSummary, PgOntologyError,
    PgOntologyStore, PropertyDefInput,
};

pub const SUPPORT_SLO_SETTING_KEY: &str = "support_slo_setting";
pub const CONSOLE_VIEW_KEY: &str = "console_view";
pub const SLA_SETTING_KEY: &str = "sla_setting";
pub const HANDOVER_POLICY_KEY: &str = "handover_policy";
pub const SHIFT_TIMETABLE_KEY: &str = "shift_timetable";
pub const LABOR_REFUSAL_KEY: &str = "labor_refusal";
pub const REGULATION_PARAM_KEY: &str = "regulation_param";
pub const SITE_COVERAGE_KEY: &str = "site_coverage";
pub const PROFITABILITY_ANALYTIC_KEY: &str = "profitability_analytic";

// BE-semantic-backfill: register existing domain tables as `projected` object
// types (coverage-matrix gap lane #4 — "Register the existing domain tables
// as engine ontology types"). Each key below mirrors the real table's `code`
// prefix/domain name so the type is recognizable from the coverage matrix.
pub const WORK_ORDER_KEY: &str = "work_order";
pub const EMPLOYEE_KEY: &str = "employee";
pub const EQUIPMENT_KEY: &str = "equipment";
pub const CUSTOMER_KEY: &str = "customer";
pub const SITE_KEY: &str = "site";
pub const APPROVAL_KEY: &str = "approval";
pub const SUPPORT_TICKET_KEY: &str = "support_ticket";
pub const EVIDENCE_KEY: &str = "evidence";
pub const COMPLIANCE_OBLIGATION_KEY: &str = "compliance_obligation";
pub const COMPLIANCE_REGULATION_KEY: &str = "compliance_regulation";
pub const COMPLIANCE_FRAMEWORK_KEY: &str = "compliance_framework";
pub const LEAVE_REQUEST_KEY: &str = "leave_request";
pub const WORKFLOW_DEFINITION_KEY: &str = "workflow_definition";
pub const MESSENGER_THREAD_KEY: &str = "messenger_thread";
pub const MAIL_KEY: &str = "mail";

/// Versioned identity of the immutable built-in ontology catalog. Changing any
/// authoritative schema in [`builtin_catalog_manifest`] requires a new version
/// and a newly reviewed database allowlist digest.
pub const BUILTIN_CATALOG_VERSION: &str = "2026-07-19.1";

// C-chain (거래처 계약 → 직무/직위 → 채용 공고 → 직원): the client-contract-to-hire
// spine. Each is an `instance`-backed engine type; the forward links form the
// traversable chain contract → position → posting → employee.
pub const CONTRACT_KEY: &str = "contract";
pub const POSITION_KEY: &str = "position";
pub const POSTING_KEY: &str = "posting";

/// Stable key of the `posting → employee` link. The historical contract leaves
/// its target unresolved (`to_object_type_id = None`) until a separately
/// governed compatibility change binds it. The catalog installer preserves that
/// intentional null rather than inferring a target merely because the employee
/// projection is present in the same catalog.
pub const POSTING_EMPLOYEE_LINK_KEY: &str = "employee";

/// A required property backed by a stored field-type tag (§3c).
fn prop(key: &str, title: &str, field_type: &str, config: serde_json::Value) -> PropertyDefInput {
    PropertyDefInput {
        key: key.to_owned(),
        title: title.to_owned(),
        field_type: field_type.to_owned(),
        config,
        backing_column: None,
        required: true,
        in_property_policy: false,
    }
}

/// The generic `create` action: one edit per property, each pulled from a
/// same-named required param. Handles both create (no `instance_id`) and stage
/// v+1 (with `instance_id`) via the `instance_revision` writeback.
///
/// The equivalent auto-attach on publish (no-code gap ①) — giving a
/// user-authored `instance`-backed type a create action when it has none of its
/// own — is NOT this function: it is owned by the SQL of
/// `ontology_api.transition_object_type`
/// (`0165_ontology_object_type_key_revisions.sql`), and
/// [`crate::PgOntologyStore::transition_lifecycle`] only calls that routine.
pub(crate) fn create_action(props: &[PropertyDefInput]) -> ActionTypeInput {
    let params_schema: serde_json::Map<String, serde_json::Value> = props
        .iter()
        .map(|p| (p.key.clone(), json!({ "required": p.required })))
        .collect();
    let edits: Vec<serde_json::Value> = props
        .iter()
        .map(|p| json!({ "property": p.key, "param": p.key }))
        .collect();
    ActionTypeInput {
        stable_key: "create".to_owned(),
        title: "저장".to_owned(),
        params_schema: serde_json::Value::Object(params_schema),
        edits: serde_json::Value::Array(edits),
        submission_criteria: json!([]),
        side_effects: json!([]),
        dispatch: ActionDispatch::InstanceRevision,
        dispatch_target: None,
        // Authority gate only; team-scope gating (§19 팀 배포) is opened as a
        // governance approval by the caller, not encoded on the action here.
        control_points: json!(["authority"]),
    }
}

/// A read-projected property: not authored through an action (projected
/// types dispatch writes through their own domain use-case, §1a), so
/// `required` is always false here — the shape a real domain row happens to
/// have, not a create-time contract. `backing_column` mirrors `key` (every
/// projected property here is named after the column it reads).
fn projected_prop(
    key: &str,
    title: &str,
    field_type: &str,
    config: serde_json::Value,
) -> PropertyDefInput {
    PropertyDefInput {
        key: key.to_owned(),
        title: title.to_owned(),
        field_type: field_type.to_owned(),
        config,
        backing_column: Some(key.to_owned()),
        required: false,
        in_property_policy: false,
    }
}

/// A projected `choice` property whose choices are the raw stored enum
/// values (id == name). `ponytail:` skip bilingual choice labels for the
/// backfill; add Korean display names when a console screen needs them.
fn choice_prop(key: &str, title: &str, values: &[&str]) -> PropertyDefInput {
    let choices: Vec<serde_json::Value> =
        values.iter().map(|v| json!({"id": v, "name": v})).collect();
    projected_prop(key, title, "choice", json!({"choices": choices}))
}

/// A traversable link from this projected type to another registered type,
/// resolved by FK column (§2 traversal generalizes the existing equipment
/// timeline-graph). Declared as `one_many`: many `from` rows reference one
/// `to` row (the standard child→parent FK shape). `reverse` is the owning
/// type's own name — the "← arrow" the console relationship tab renders from
/// the target's side (docs/design/oyatie-console change-log 74), mirroring the
/// C-chain [`link`] helper.
fn fk_link(stable_key: &str, title: &str, reverse: &str, to: ObjectTypeId) -> LinkTypeInput {
    LinkTypeInput {
        stable_key: stable_key.to_owned(),
        title: title.to_owned(),
        reverse_title: Some(reverse.to_owned()),
        to_object_type_id: Some(to),
        cardinality: LinkCardinality::OneMany,
        traversable: true,
    }
}

/// Build a `projected` object-type draft: no owned instance store, no
/// actions (this lane is read-path only — write dispatch through the domain
/// use-case is a future charter, arch §9.3), `primary_key_property` is the
/// backing table's literal `id` column on every table registered here.
fn projected_draft(
    stable_key: &str,
    title: &str,
    backing_table: &str,
    title_property_key: &str,
    properties: Vec<PropertyDefInput>,
    links: Vec<LinkTypeInput>,
) -> CreateObjectTypeDraft {
    CreateObjectTypeDraft {
        stable_key: stable_key.to_owned(),
        title: title.to_owned(),
        title_property_key: Some(title_property_key.to_owned()),
        backing_kind: BackingKind::Projected,
        backing_table: Some(backing_table.to_owned()),
        primary_key_property: Some("id".to_owned()),
        actions: Vec::new(),
        properties,
        links,
        analytics: Vec::new(),
    }
}

/// CU- customer (`registry_customers`).
#[must_use]
pub fn customer_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        projected_prop("branch_id", "지사", "reference", json!({})),
        projected_prop("name", "고객명", "text", json!({})),
        projected_prop("created_at", "등록일", "timestamp", json!({})),
    ];
    projected_draft(
        CUSTOMER_KEY,
        "고객",
        "registry_customers",
        "name",
        properties,
        Vec::new(),
    )
}

/// SI- site (`registry_sites`), FK-linked to its customer.
#[must_use]
pub fn site_draft(customer_type_id: ObjectTypeId) -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        projected_prop("branch_id", "지사", "reference", json!({})),
        projected_prop("customer_id", "고객", "reference", json!({})),
        projected_prop("name", "현장명", "text", json!({})),
        projected_prop("created_at", "등록일", "timestamp", json!({})),
    ];
    let links = vec![fk_link("customer", "고객", "현장", customer_type_id)];
    projected_draft(
        SITE_KEY,
        "현장",
        "registry_sites",
        "name",
        properties,
        links,
    )
}

/// FL- equipment (`registry_equipment`), FK-linked to customer + site.
#[must_use]
pub fn equipment_draft(
    customer_type_id: ObjectTypeId,
    site_type_id: ObjectTypeId,
) -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        projected_prop("equipment_no", "장비번호", "text", json!({})),
        projected_prop("branch_id", "지사", "reference", json!({})),
        projected_prop("customer_id", "고객", "reference", json!({})),
        projected_prop("site_id", "현장", "reference", json!({})),
        choice_prop("status", "상태", &["임대", "예비", "폐기", "대체", "매각"]),
        projected_prop("manufacturer_code", "제조사코드", "text", json!({})),
        projected_prop("kind_code", "종류코드", "text", json!({})),
        projected_prop("specification", "규격", "text", json!({})),
        projected_prop("created_at", "등록일", "timestamp", json!({})),
    ];
    let links = vec![
        fk_link("customer", "고객", "장비", customer_type_id),
        fk_link("site", "현장", "장비", site_type_id),
    ];
    projected_draft(
        EQUIPMENT_KEY,
        "장비",
        "registry_equipment",
        "equipment_no",
        properties,
        links,
    )
}

/// HR- employee (`employees`).
#[must_use]
pub fn employee_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        projected_prop("company", "회사", "text", json!({})),
        projected_prop("name", "이름", "text", json!({})),
        projected_prop("source_key", "원본 키", "text", json!({})),
        projected_prop("created_at", "등록일", "timestamp", json!({})),
    ];
    projected_draft(
        EMPLOYEE_KEY,
        "직원",
        "employees",
        "name",
        properties,
        Vec::new(),
    )
}

/// WO- work order (`work_orders`), FK-linked to equipment/customer/site.
#[must_use]
pub fn work_order_draft(
    equipment_type_id: ObjectTypeId,
    customer_type_id: ObjectTypeId,
    site_type_id: ObjectTypeId,
) -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        projected_prop("request_no", "접수번호", "text", json!({})),
        projected_prop("branch_id", "지사", "reference", json!({})),
        projected_prop("equipment_id", "장비", "reference", json!({})),
        projected_prop("customer_id", "고객", "reference", json!({})),
        projected_prop("site_id", "현장", "reference", json!({})),
        choice_prop(
            "status",
            "상태",
            &[
                "RECEIVED",
                "UNASSIGNED",
                "ASSIGNED",
                "IN_PROGRESS",
                "REPORT_SUBMITTED",
                "ADMIN_REVIEW",
                "FINAL_COMPLETED",
                "REJECTED",
                "ON_HOLD",
                "DELAYED",
                "TEMPORARY_ACTION",
                "PART_WAITING",
                "EQUIPMENT_IN_USE",
                "REVISIT_REQUIRED",
                "ARCHIVED",
                "CANCELLED",
            ],
        ),
        choice_prop(
            "priority",
            "우선순위",
            &["P1", "P2", "P3", "OUTSOURCE", "UNSET"],
        ),
        projected_prop("symptom", "증상", "text", json!({})),
        projected_prop("target_due_at", "목표완료일", "timestamp", json!({})),
        projected_prop("created_at", "접수일", "timestamp", json!({})),
    ];
    let links = vec![
        fk_link("equipment", "장비", "작업지시", equipment_type_id),
        fk_link("customer", "고객", "작업지시", customer_type_id),
        fk_link("site", "현장", "작업지시", site_type_id),
    ];
    projected_draft(
        WORK_ORDER_KEY,
        "작업지시",
        "work_orders",
        "request_no",
        properties,
        links,
    )
}

/// AP- approval (`gov_approval_requests`) — the pending request a four-eyes
/// decision (`gov_approvals`) decides. No separate `approval_items` table
/// exists in the schema; the request row is the item.
#[must_use]
pub fn approval_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        projected_prop("request_ref", "대상", "reference", json!({})),
        projected_prop("kind", "종류", "text", json!({})),
        projected_prop("requested_by", "기안자", "reference", json!({})),
        projected_prop("created_at", "기안일", "timestamp", json!({})),
    ];
    projected_draft(
        APPROVAL_KEY,
        "결재",
        "gov_approval_requests",
        "kind",
        properties,
        Vec::new(),
    )
}

/// SUP- support ticket (`support_tickets`).
#[must_use]
pub fn support_ticket_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        choice_prop("origin", "채널", &["INTERNAL", "CUSTOMER"]),
        choice_prop(
            "category",
            "분류",
            &[
                "SYSTEM_BUG",
                "ACCESS_REQUEST",
                "OPERATIONAL",
                "EQUIPMENT_INQUIRY",
                "COMPLAINT",
                "OTHER",
            ],
        ),
        choice_prop("priority", "우선순위", &["LOW", "MEDIUM", "HIGH", "URGENT"]),
        choice_prop(
            "status",
            "상태",
            &["OPEN", "IN_PROGRESS", "ON_HOLD", "RESOLVED", "CLOSED"],
        ),
        projected_prop("title", "제목", "text", json!({})),
        projected_prop("assignee_user_id", "담당자", "reference", json!({})),
        projected_prop("due_at", "SLA 기한", "timestamp", json!({})),
        projected_prop("created_at", "접수일", "timestamp", json!({})),
    ];
    projected_draft(
        SUPPORT_TICKET_KEY,
        "지원 티켓",
        "support_tickets",
        "title",
        properties,
        Vec::new(),
    )
}

/// EV- evidence object (`docs_evidence_objects`).
#[must_use]
pub fn evidence_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        projected_prop("code", "코드", "text", json!({})),
        choice_prop(
            "source_type",
            "출처유형",
            &[
                "record_archive",
                "inbox_doc",
                "mail_attachment",
                "ingest_job",
                "work_order_evidence_media",
                "external_document",
            ],
        ),
        choice_prop(
            "classification",
            "분류등급",
            &["GENERAL", "INTERNAL", "SENSITIVE", "CONFIDENTIAL", "SECRET"],
        ),
        projected_prop("current_custody_stage", "보관단계", "text", json!({})),
        choice_prop("legal_hold_state", "법적보류", &["CLEAR", "ACTIVE"]),
        projected_prop("created_at", "등록일", "timestamp", json!({})),
    ];
    projected_draft(
        EVIDENCE_KEY,
        "증거",
        "docs_evidence_objects",
        "code",
        properties,
        Vec::new(),
    )
}

/// CP- compliance obligation (`compliance_obligations`), FK-linked to site
/// (nullable — org/branch-scoped obligations carry no site).
#[must_use]
pub fn compliance_obligation_draft(site_type_id: ObjectTypeId) -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        projected_prop("code", "코드", "text", json!({})),
        choice_prop(
            "obligation_type",
            "유형",
            &[
                "LEGAL",
                "REGULATORY",
                "CONTRACTUAL",
                "INTERNAL_POLICY",
                "CONTROL_REQUIREMENT",
            ],
        ),
        choice_prop(
            "scope_type",
            "범위",
            &["ORG", "BRANCH", "SITE", "TEAM", "ROLE"],
        ),
        projected_prop("site_id", "현장", "reference", json!({})),
        choice_prop(
            "severity",
            "심각도",
            &["INFO", "LOW", "MEDIUM", "HIGH", "CRITICAL"],
        ),
        choice_prop(
            "status",
            "상태",
            &["DRAFT", "ACTIVE", "WAIVED", "SUPERSEDED", "ARCHIVED"],
        ),
        projected_prop("created_at", "등록일", "timestamp", json!({})),
    ];
    let links = vec![fk_link("site", "현장", "준수 의무", site_type_id)];
    projected_draft(
        COMPLIANCE_OBLIGATION_KEY,
        "준수 의무",
        "compliance_obligations",
        "code",
        properties,
        links,
    )
}

/// RG- compliance regulation (`compliance_regulation_impacts`).
#[must_use]
pub fn compliance_regulation_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        projected_prop("code", "코드", "text", json!({})),
        projected_prop("jurisdiction", "관할", "text", json!({})),
        projected_prop("regulator", "규제기관", "text", json!({})),
        choice_prop(
            "risk_level",
            "위험도",
            &["INFO", "LOW", "MEDIUM", "HIGH", "CRITICAL"],
        ),
        choice_prop(
            "status",
            "상태",
            &["DRAFT", "ACTIVE", "SUPERSEDED", "ARCHIVED"],
        ),
        projected_prop("created_at", "등록일", "timestamp", json!({})),
    ];
    projected_draft(
        COMPLIANCE_REGULATION_KEY,
        "규정",
        "compliance_regulation_impacts",
        "code",
        properties,
        Vec::new(),
    )
}

/// FW- compliance framework (`compliance_frameworks`).
#[must_use]
pub fn compliance_framework_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        projected_prop("code", "코드", "text", json!({})),
        projected_prop("name", "명칭", "text", json!({})),
        choice_prop(
            "framework_kind",
            "유형",
            &[
                "LEGAL_BASELINE",
                "INTERNAL_CONTROL",
                "CUSTOMER_CONTROL",
                "SECURITY_STANDARD",
                "SAFETY_STANDARD",
                "AUDIT_PROGRAM",
            ],
        ),
        choice_prop(
            "status",
            "상태",
            &["DRAFT", "ACTIVE", "RETIRED", "ARCHIVED"],
        ),
        projected_prop("created_at", "등록일", "timestamp", json!({})),
    ];
    projected_draft(
        COMPLIANCE_FRAMEWORK_KEY,
        "표준 프레임워크",
        "compliance_frameworks",
        "code",
        properties,
        Vec::new(),
    )
}

/// 연차 leave request (`leave_requests`), FK-linked to the subject employee.
#[must_use]
pub fn leave_request_draft(employee_type_id: ObjectTypeId) -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        projected_prop("subject_employee_id", "대상 직원", "reference", json!({})),
        choice_prop("leave_type", "유형", &["annual", "half_day"]),
        choice_prop(
            "status",
            "상태",
            &["pending", "approved", "returned", "rejected"],
        ),
        projected_prop("start_date", "시작일", "date", json!({})),
        projected_prop("end_date", "종료일", "date", json!({})),
        projected_prop("reason", "사유", "text", json!({})),
        projected_prop("created_at", "신청일", "timestamp", json!({})),
    ];
    let links = vec![fk_link("employee", "직원", "휴가 신청", employee_type_id)];
    projected_draft(
        LEAVE_REQUEST_KEY,
        "휴가 신청",
        "leave_requests",
        "reason",
        properties,
        links,
    )
}

/// workflow definition (`workflow_definitions`, §M2 engine).
#[must_use]
pub fn workflow_definition_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        projected_prop("workflow_key", "키", "text", json!({})),
        projected_prop("display_name", "이름", "text", json!({})),
        projected_prop("object_type", "대상 타입", "text", json!({})),
        choice_prop("status", "상태", &["DRAFT", "ACTIVE", "PAUSED", "RETIRED"]),
        projected_prop("created_at", "등록일", "timestamp", json!({})),
    ];
    projected_draft(
        WORKFLOW_DEFINITION_KEY,
        "워크플로우 정의",
        "workflow_definitions",
        "display_name",
        properties,
        Vec::new(),
    )
}

/// messenger thread (`messenger_threads`), FK-linked to its work order
/// (nullable — team/DM/group threads carry no work order).
#[must_use]
pub fn messenger_thread_draft(work_order_type_id: ObjectTypeId) -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        choice_prop("kind", "종류", &["work_order", "team", "dm", "group"]),
        projected_prop("branch_id", "지사", "reference", json!({})),
        projected_prop("work_order_id", "작업지시", "reference", json!({})),
        projected_prop("title", "제목", "text", json!({})),
        projected_prop("created_at", "생성일", "timestamp", json!({})),
    ];
    let links = vec![fk_link(
        "work_order",
        "작업지시",
        "메신저 스레드",
        work_order_type_id,
    )];
    projected_draft(
        MESSENGER_THREAD_KEY,
        "메신저 스레드",
        "messenger_threads",
        "title",
        properties,
        links,
    )
}

/// mail (`email_messages`, webmail sync).
#[must_use]
pub fn mail_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        projected_prop("id", "ID", "reference", json!({})),
        choice_prop("direction", "방향", &["IN", "OUT"]),
        projected_prop("from_address", "발신자", "text", json!({})),
        projected_prop("subject", "제목", "text", json!({})),
        projected_prop("seen", "읽음", "boolean", json!({})),
        projected_prop("flagged", "중요", "boolean", json!({})),
        projected_prop("created_at", "수신일", "timestamp", json!({})),
    ];
    projected_draft(
        MAIL_KEY,
        "메일",
        "email_messages",
        "subject",
        properties,
        Vec::new(),
    )
}

/// §4-26 SLO setting: threshold/window/escalation per support ticket type.
#[must_use]
pub fn support_slo_setting_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        prop(
            "ticket_type",
            "티켓 유형",
            "choice",
            json!({"choices": [
                {"id": "incident", "name": "장애"},
                {"id": "request", "name": "요청"},
                {"id": "change", "name": "변경"}
            ]}),
        ),
        prop("threshold_minutes", "임계(분)", "integer", json!({})),
        prop(
            "window",
            "적용 시간",
            "choice",
            json!({"choices": [
                {"id": "business_hours", "name": "업무시간"},
                {"id": "calendar", "name": "24x7"}
            ]}),
        ),
        prop("escalation_target", "에스컬레이션 대상", "text", json!({})),
    ];
    CreateObjectTypeDraft {
        stable_key: SUPPORT_SLO_SETTING_KEY.to_owned(),
        title: "SLO 설정".to_owned(),
        title_property_key: Some("ticket_type".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        actions: vec![create_action(&properties)],
        properties,
        links: Vec::new(),
        analytics: Vec::new(),
    }
}

/// §19 console_view: a persisted dashboard/table layout doc. `scope` distinguishes
/// a personal layout (direct save) from a team layout (deployed via approval).
#[must_use]
pub fn console_view_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        prop("screen_key", "화면", "text", json!({})),
        prop("config", "레이아웃", "json", json!({})),
        prop(
            "scope",
            "범위",
            "choice",
            json!({"choices": [
                {"id": "personal", "name": "개인"},
                {"id": "team", "name": "팀"}
            ]}),
        ),
    ];
    CreateObjectTypeDraft {
        stable_key: CONSOLE_VIEW_KEY.to_owned(),
        title: "콘솔 뷰".to_owned(),
        title_property_key: Some("screen_key".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        actions: vec![create_action(&properties)],
        properties,
        links: Vec::new(),
        analytics: Vec::new(),
    }
}

/// SLA setting: contract/site service-level terms — distinct from the
/// per-ticket-type SLO above (§4-26).
#[must_use]
pub fn sla_setting_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        prop("contract_ref", "계약/현장 참조", "text", json!({})),
        prop(
            "tier",
            "등급",
            "choice",
            json!({"choices": [
                {"id": "standard", "name": "표준"},
                {"id": "premium", "name": "프리미엄"}
            ]}),
        ),
        prop("response_minutes", "응답(분)", "integer", json!({})),
        prop("resolution_minutes", "해결(분)", "integer", json!({})),
        prop("penalty_clause", "위약조항", "text", json!({})),
    ];
    CreateObjectTypeDraft {
        stable_key: SLA_SETTING_KEY.to_owned(),
        title: "SLA 설정".to_owned(),
        title_property_key: Some("contract_ref".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        actions: vec![create_action(&properties)],
        properties,
        links: Vec::new(),
        analytics: Vec::new(),
    }
}

/// HO- handover policy (인수인계): who acts automatically, when it escalates,
/// the minimum fit-for-duty staffing floor, and the department heads on point.
#[must_use]
pub fn handover_policy_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        prop("policy_name", "정책명", "text", json!({})),
        prop("auto_act", "자동조치", "boolean", json!({})),
        prop(
            "escalate",
            "에스컬레이션",
            "choice",
            json!({"choices": [
                {"id": "none", "name": "없음"},
                {"id": "supervisor", "name": "감독자"},
                {"id": "duty_manager", "name": "당직관리자"}
            ]}),
        ),
        prop("fit_floor", "최소인원 기준", "integer", json!({})),
        prop("dept_heads", "부서장", "text", json!({})),
    ];
    CreateObjectTypeDraft {
        stable_key: HANDOVER_POLICY_KEY.to_owned(),
        title: "인수인계 정책".to_owned(),
        title_property_key: Some("policy_name".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        actions: vec![create_action(&properties)],
        properties,
        links: Vec::new(),
        analytics: Vec::new(),
    }
}

/// 교대 shift timetable: named shift with a time-of-day window.
#[must_use]
pub fn shift_timetable_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        prop("shift_name", "교대명", "text", json!({})),
        prop("start_time", "시작시각", "text", json!({})),
        prop("end_time", "종료시각", "text", json!({})),
        prop("days_of_week", "적용요일", "text", json!({})),
    ];
    CreateObjectTypeDraft {
        stable_key: SHIFT_TIMETABLE_KEY.to_owned(),
        title: "교대 시간표".to_owned(),
        title_property_key: Some("shift_name".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        actions: vec![create_action(&properties)],
        properties,
        links: Vec::new(),
        analytics: Vec::new(),
    }
}

/// 노무수령거부 labor refusal: a legal-status record for a refusal to receive
/// an employee's labor.
#[must_use]
pub fn labor_refusal_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        prop("employee_ref", "대상 근로자", "text", json!({})),
        prop("refusal_date", "거부일자", "date", json!({})),
        prop("reason", "사유", "text", json!({})),
        prop(
            "status",
            "상태",
            "choice",
            json!({"choices": [
                {"id": "pending", "name": "대기"},
                {"id": "confirmed", "name": "확정"},
                {"id": "withdrawn", "name": "철회"}
            ]}),
        ),
    ];
    CreateObjectTypeDraft {
        stable_key: LABOR_REFUSAL_KEY.to_owned(),
        title: "노무수령거부".to_owned(),
        title_property_key: Some("employee_ref".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        actions: vec![create_action(&properties)],
        properties,
        links: Vec::new(),
        analytics: Vec::new(),
    }
}

/// RG- regulation parameter (최저임금, 주52h): an org-scoped statutory value
/// with an effective date and its impact.
#[must_use]
pub fn regulation_param_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        prop(
            "param_key",
            "파라미터",
            "choice",
            json!({"choices": [
                {"id": "min_wage", "name": "최저임금"},
                {"id": "max_weekly_hours", "name": "주52시간"}
            ]}),
        ),
        prop("value", "값", "decimal", json!({})),
        prop("effective_date", "시행일", "date", json!({})),
        prop("impact_scope", "영향범위", "text", json!({})),
        prop("impact_note", "영향메모", "text", json!({})),
    ];
    CreateObjectTypeDraft {
        stable_key: REGULATION_PARAM_KEY.to_owned(),
        title: "규제 파라미터".to_owned(),
        title_property_key: Some("param_key".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        actions: vec![create_action(&properties)],
        properties,
        links: Vec::new(),
        analytics: Vec::new(),
    }
}

/// 현장 site coverage: required vs. assigned headcount for a worksite as of a
/// given date.
#[must_use]
pub fn site_coverage_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        prop("site_ref", "현장", "text", json!({})),
        prop("required_headcount", "필요인원", "integer", json!({})),
        prop("assigned_headcount", "배치인원", "integer", json!({})),
        prop("coverage_date", "기준일", "date", json!({})),
    ];
    CreateObjectTypeDraft {
        stable_key: SITE_COVERAGE_KEY.to_owned(),
        title: "현장 커버리지".to_owned(),
        title_property_key: Some("site_ref".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        actions: vec![create_action(&properties)],
        properties,
        links: Vec::new(),
        analytics: Vec::new(),
    }
}

/// 수익성 profitability analytic: a contract's revenue/cost with the margin
/// formula that derives it.
#[must_use]
pub fn profitability_analytic_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        prop("contract_ref", "계약", "text", json!({})),
        prop("revenue", "매출", "decimal", json!({})),
        prop("cost", "원가", "decimal", json!({})),
        prop("margin_pct", "마진율", "decimal", json!({})),
        prop(
            "formula",
            "산식",
            "text",
            json!({"expression": "(revenue - cost) / revenue * 100"}),
        ),
    ];
    CreateObjectTypeDraft {
        stable_key: PROFITABILITY_ANALYTIC_KEY.to_owned(),
        title: "수익성 분석".to_owned(),
        title_property_key: Some("contract_ref".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        actions: vec![create_action(&properties)],
        properties,
        links: Vec::new(),
        analytics: Vec::new(),
    }
}

/// A forward link on the owning type. `to` is the target type's published
/// version id, or `None` when the target type is not yet registered (resolved by
/// `stable_key` later). Every C-chain link is traversable so the chain walks.
fn link(
    stable_key: &str,
    title: &str,
    reverse_title: &str,
    to: Option<ObjectTypeId>,
    cardinality: LinkCardinality,
) -> LinkTypeInput {
    LinkTypeInput {
        stable_key: stable_key.to_owned(),
        title: title.to_owned(),
        reverse_title: Some(reverse_title.to_owned()),
        to_object_type_id: to,
        cardinality,
        traversable: true,
    }
}

/// C- contract (거래처 계약): the client-contract head of the chain. Links forward
/// (one → many) to the positions it authorizes.
#[must_use]
pub fn contract_draft(position_type_id: ObjectTypeId) -> CreateObjectTypeDraft {
    let properties = vec![
        prop("client", "거래처", "text", json!({})),
        prop(
            "monthly_fee",
            "월 계약금",
            "money",
            json!({"currency": "KRW"}),
        ),
        prop("period", "기간", "daterange", json!({})),
        prop(
            "status",
            "상태",
            "lifecycle",
            json!({"states": [
                {"id": "draft", "name": "초안"},
                {"id": "active", "name": "활성"},
                {"id": "expired", "name": "만료"},
                {"id": "terminated", "name": "해지"}
            ]}),
        ),
        prop("margin", "마진", "percent", json!({})),
    ];
    CreateObjectTypeDraft {
        stable_key: CONTRACT_KEY.to_owned(),
        title: "계약".to_owned(),
        title_property_key: Some("client".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        actions: vec![create_action(&properties)],
        properties,
        links: vec![link(
            "positions",
            "직무",
            "계약",
            Some(position_type_id),
            LinkCardinality::OneMany,
        )],
        analytics: Vec::new(),
    }
}

/// Position (직무/직위): a site × 직무 × 직책 × TO opening the contract authorizes.
/// Links forward (one → many) to the postings raised to fill it.
#[must_use]
pub fn position_draft(posting_type_id: ObjectTypeId) -> CreateObjectTypeDraft {
    let properties = vec![
        prop(
            "worksite",
            "현장",
            "reference",
            json!({"target": "worksite"}),
        ),
        prop("job_function", "직무", "text", json!({})),
        prop("job_title", "직책", "text", json!({})),
        prop("headcount", "정원(TO)", "integer", json!({})),
    ];
    CreateObjectTypeDraft {
        stable_key: POSITION_KEY.to_owned(),
        title: "직무".to_owned(),
        title_property_key: Some("job_title".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        actions: vec![create_action(&properties)],
        properties,
        links: vec![link(
            "postings",
            "공고",
            "직무",
            Some(posting_type_id),
            LinkCardinality::OneMany,
        )],
        analytics: Vec::new(),
    }
}

/// Posting (채용 공고): an internal/external hiring notice for a position. Links
/// forward (one → many) to the employees it fills — the employee type is not yet
/// registered, so that link is authored unresolved (see [`POSTING_EMPLOYEE_LINK_KEY`]).
#[must_use]
pub fn posting_draft() -> CreateObjectTypeDraft {
    let properties = vec![
        prop(
            "scope",
            "공개 범위",
            "choice",
            json!({"choices": [
                {"id": "internal", "name": "내부"},
                {"id": "external", "name": "외부"}
            ]}),
        ),
        prop("fill_count", "충원", "integer", json!({})),
        prop("deadline", "마감", "date", json!({})),
    ];
    CreateObjectTypeDraft {
        stable_key: POSTING_KEY.to_owned(),
        title: "채용 공고".to_owned(),
        title_property_key: Some("scope".to_owned()),
        backing_kind: BackingKind::Instance,
        backing_table: None,
        primary_key_property: None,
        actions: vec![create_action(&properties)],
        properties,
        // Employee target is a projected ref resolved by the backfill lane; author
        // the link now (unresolved) so the chain shape is complete and stable.
        links: vec![link(
            POSTING_EMPLOYEE_LINK_KEY,
            "충원 직원",
            "공고",
            None,
            LinkCardinality::OneMany,
        )],
        analytics: Vec::new(),
    }
}

/// Provision the C-chain (contract → position → posting) from the immutable
/// built-in catalog for the armed org.
pub async fn seed_c_chain_object_types(
    store: &PgOntologyStore,
    actor: UserId,
    occurred_at: OffsetDateTime,
) -> Result<Vec<ObjectTypeSummary>, PgOntologyError> {
    install_builtin_catalog(
        store,
        actor,
        occurred_at,
        &[CONTRACT_KEY, POSITION_KEY, POSTING_KEY],
    )
    .await
}

/// BE-semantic-backfill: register the ~15 existing domain tables listed in
/// the coverage-matrix gap lane #4 as `projected` object types. The installer
/// resolves their logical stable-key links to tenant-local IDs atomically. Read
/// path only this lane — no
/// actions are attached, so these types cannot yet be created/edited through
/// the engine; the domain crates' own use-cases remain the sole writers
/// (arch §9.3: never a second writeback into a projected table).
pub async fn seed_projected_domain_object_types(
    store: &PgOntologyStore,
    actor: UserId,
    occurred_at: OffsetDateTime,
) -> Result<Vec<ObjectTypeSummary>, PgOntologyError> {
    install_builtin_catalog(store, actor, occurred_at, PROJECTED_DOMAIN_KEYS).await
}

const GOVERNED_CONFIG_KEYS: &[&str] = &[
    SUPPORT_SLO_SETTING_KEY,
    CONSOLE_VIEW_KEY,
    SLA_SETTING_KEY,
    HANDOVER_POLICY_KEY,
    SHIFT_TIMETABLE_KEY,
    LABOR_REFUSAL_KEY,
    REGULATION_PARAM_KEY,
    SITE_COVERAGE_KEY,
    PROFITABILITY_ANALYTIC_KEY,
];

const C_CHAIN_KEYS: &[&str] = &[CONTRACT_KEY, POSITION_KEY, POSTING_KEY];

const PROJECTED_DOMAIN_KEYS: &[&str] = &[
    CUSTOMER_KEY,
    SITE_KEY,
    EQUIPMENT_KEY,
    EMPLOYEE_KEY,
    WORK_ORDER_KEY,
    APPROVAL_KEY,
    SUPPORT_TICKET_KEY,
    EVIDENCE_KEY,
    COMPLIANCE_OBLIGATION_KEY,
    COMPLIANCE_REGULATION_KEY,
    COMPLIANCE_FRAMEWORK_KEY,
    LEAVE_REQUEST_KEY,
    WORKFLOW_DEFINITION_KEY,
    MESSENGER_THREAD_KEY,
    MAIL_KEY,
];

fn catalog_target(value: u128) -> ObjectTypeId {
    ObjectTypeId::from_uuid(Uuid::from_u128(value))
}

/// Build the complete tenant-independent catalog manifest. Placeholder target
/// ids exist only while invoking the existing strongly typed draft builders;
/// they are replaced by stable keys before the manifest crosses the adapter
/// boundary. Consequently no tenant-local UUID can enter the allowlisted digest.
#[doc(hidden)]
pub fn builtin_catalog_manifest() -> Result<serde_json::Value, PgOntologyError> {
    let posting_id = catalog_target(1);
    let position_id = catalog_target(2);
    let customer_id = catalog_target(3);
    let site_id = catalog_target(4);
    let equipment_id = catalog_target(5);
    let employee_id = catalog_target(6);
    let work_order_id = catalog_target(7);

    let target_keys = HashMap::from([
        (posting_id.to_string(), POSTING_KEY),
        (position_id.to_string(), POSITION_KEY),
        (customer_id.to_string(), CUSTOMER_KEY),
        (site_id.to_string(), SITE_KEY),
        (equipment_id.to_string(), EQUIPMENT_KEY),
        (employee_id.to_string(), EMPLOYEE_KEY),
        (work_order_id.to_string(), WORK_ORDER_KEY),
    ]);

    let drafts = vec![
        support_slo_setting_draft(),
        console_view_draft(),
        sla_setting_draft(),
        handover_policy_draft(),
        shift_timetable_draft(),
        labor_refusal_draft(),
        regulation_param_draft(),
        site_coverage_draft(),
        profitability_analytic_draft(),
        contract_draft(position_id),
        position_draft(posting_id),
        posting_draft(),
        customer_draft(),
        site_draft(customer_id),
        equipment_draft(customer_id, site_id),
        employee_draft(),
        work_order_draft(equipment_id, customer_id, site_id),
        approval_draft(),
        support_ticket_draft(),
        evidence_draft(),
        compliance_obligation_draft(site_id),
        compliance_regulation_draft(),
        compliance_framework_draft(),
        leave_request_draft(employee_id),
        workflow_definition_draft(),
        messenger_thread_draft(work_order_id),
        mail_draft(),
    ];

    let mut object_types = Vec::with_capacity(drafts.len());
    for draft in drafts {
        crate::validate_draft(&draft)?;
        let mut snapshot = serde_json::to_value(draft).map_err(|error| {
            KernelError::validation(format!("invalid built-in ontology snapshot: {error}"))
        })?;
        let links = snapshot
            .get_mut("links")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(|| KernelError::validation("built-in ontology links must be an array"))?;
        for link in links {
            let link = link.as_object_mut().ok_or_else(|| {
                KernelError::validation("built-in ontology link must be an object")
            })?;
            let target = link.remove("to_object_type_id").unwrap_or_default();
            let stable_key = match target.as_str() {
                Some(id) => target_keys.get(id).copied().ok_or_else(|| {
                    KernelError::validation("built-in ontology link target is not in the catalog")
                })?,
                None if target.is_null() => "",
                None => {
                    return Err(KernelError::validation(
                        "built-in ontology link target must be a UUID or null",
                    )
                    .into());
                }
            };
            link.insert(
                "to_stable_key".to_owned(),
                if stable_key.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(stable_key.to_owned())
                },
            );
        }
        object_types.push(snapshot);
    }

    Ok(json!({
        "catalog_version": BUILTIN_CATALOG_VERSION,
        "object_types": object_types,
    }))
}

/// Atomically install the exact digest-allowlisted catalog, then return the
/// requested published heads in caller-specified order. Exact retries are a
/// database-owned no-op; any drift or partial pre-existing catalog fails closed.
async fn install_builtin_catalog(
    store: &PgOntologyStore,
    actor: UserId,
    occurred_at: OffsetDateTime,
    requested_keys: &[&str],
) -> Result<Vec<ObjectTypeSummary>, PgOntologyError> {
    let manifest = builtin_catalog_manifest()?;
    let trace = TraceContext::generate();
    let expected_count = manifest
        .get("object_types")
        .and_then(serde_json::Value::as_array)
        .and_then(|object_types| i64::try_from(object_types.len()).ok())
        .ok_or_else(|| KernelError::validation("built-in ontology catalog is malformed"))?;
    let result = store
        .install_builtin_catalog(actor, BUILTIN_CATALOG_VERSION, manifest, trace, occurred_at)
        .await?;
    if result.object_type_count != expected_count {
        return Err(KernelError::validation(format!(
            "built-in ontology installer returned {} objects; expected {expected_count}",
            result.object_type_count
        ))
        .into());
    }

    let mut heads = store.list_object_types().await?;
    let mut selected = Vec::with_capacity(requested_keys.len());
    for key in requested_keys {
        let index = heads
            .iter()
            .position(|head| head.stable_key == *key)
            .ok_or_else(|| {
                KernelError::validation(format!(
                    "built-in ontology installer omitted published key `{key}`"
                ))
            })?;
        let head = heads.swap_remove(index);
        if head.lifecycle_state != SchemaLifecycleState::Published {
            return Err(KernelError::validation(format!(
                "built-in ontology key `{key}` is not published"
            ))
            .into());
        }
        selected.push(head);
    }
    Ok(selected)
}

/// Provision the standard complete built-in catalog for the org armed on the
/// current request context (`app.current_org`). The database verifies its
/// canonical JSONB digest against a migration-owned allowlist, installs all
/// definitions atomically, and treats an exact retry as a no-op.
pub async fn seed_governed_config_object_types(
    store: &PgOntologyStore,
    actor: UserId,
    occurred_at: OffsetDateTime,
) -> Result<Vec<ObjectTypeSummary>, PgOntologyError> {
    let mut requested = Vec::with_capacity(
        GOVERNED_CONFIG_KEYS.len() + C_CHAIN_KEYS.len() + PROJECTED_DOMAIN_KEYS.len(),
    );
    requested.extend_from_slice(GOVERNED_CONFIG_KEYS);
    requested.extend_from_slice(C_CHAIN_KEYS);
    requested.extend_from_slice(PROJECTED_DOMAIN_KEYS);
    install_builtin_catalog(store, actor, occurred_at, &requested).await
}
