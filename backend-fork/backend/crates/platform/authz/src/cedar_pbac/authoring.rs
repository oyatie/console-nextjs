//! Cedar policy authoring + point-decision evaluation (Policy Studio spine).
//!
//! This module is DEPENDENCY-LIGHT on purpose: it holds only pure domain logic
//! (no DB, no request-context) so `crates/platform/authz` never needs to depend
//! on `request-context` — which itself depends on `authz` — and the crate DAG
//! stays acyclic. Persistence + REST live in the thin `authz-rest` crate.
//!
//! Three responsibilities, all fail-closed:
//!  * **No-code authoring.** [`NoCodeBlocks`] → [`normalize`] (canonical row) →
//!    [`generate_cedar_text`] (real Cedar `permit`/`forbid`), then
//!    [`validate_blocks`] strict-validates the generated policy against
//!    [`AUTHORING_SCHEMA`] by reusing the already-audited
//!    [`engine::compile_bundle_from_sources`]. A draft that does not typecheck is
//!    marked `Invalid` and can never be submitted.
//!  * **Draft lifecycle.** [`submit_draft`] (validation must be `Valid`) and
//!    [`review_draft`] (four-eyes: reviewer ≠ author) move a draft
//!    `draft → review_pending → approved_for_promotion | rejected`. Nothing here
//!    can produce a `shadow`/`enforced` row — promotion is a separate gated lane.
//!  * **Point decision.** [`simulate`] is the single evaluator behind both the
//!    `/policy/simulate` (what-if) and `/policy/authorize` (live) surfaces:
//!    deny-by-omission default, `forbid` always wins, unknown action ⇒ deny, and
//!    the whole Cedar call is `catch_unwind`-isolated exactly like [`engine`].
//!
//! [`engine`]: super::engine

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::str::FromStr;

use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid, Policy,
    PolicyId, PolicySet, Request, RestrictedExpression, Schema,
};

use super::engine::compile_bundle_from_sources;
use console_kernel_core::{KernelError, OrgId};

/// Schema identity for the generic ontology object/property authoring bundle.
/// Bump when [`AUTHORING_SCHEMA`] changes so validated digests never collide.
pub const AUTHORING_SCHEMA_VERSION: &str = "2026-07-ontology-authoring-v1";

/// Cedar schema every authored object/property policy validates against.
///
/// Entities carry only SERVER-loaded material (a UI/JWT projection can't
/// fabricate `roles`/`clearance_keys`/`owner`). Actions are the fixed set the
/// ontology surfaces the authoring UI targets: `view`/`edit` are row (object)
/// policies, `read_field` is the property (field) policy. An authored policy for
/// any other action fails strict validation and is rejected fail-closed.
///
/// `ponytail:` fixed 3-action schema — object/property policies are the only
/// authoring surface today. Declare more actions here (zero code change) when a
/// real policy needs them.
pub const AUTHORING_SCHEMA: &str = r#"entity Subject = {
  "org": String,
  "user_id": String,
  "roles": Set<String>,
  "clearance_keys": Set<String>
};

entity Resource = {
  "org": String,
  "resource_type": String,
  "resource_id"?: String,
  "owner"?: String,
  "branch"?: String,
  "legal_hold"?: Bool
};

action view appliesTo {
  principal: [Subject],
  resource: [Resource],
  context: { "purpose"?: String }
};

action edit appliesTo {
  principal: [Subject],
  resource: [Resource],
  context: { "purpose"?: String }
};

action read_field appliesTo {
  principal: [Subject],
  resource: [Resource],
  context: { "field"?: String }
};

action "console:configure" appliesTo {
  principal: [Subject],
  resource: [Resource],
  context: { "purpose"?: String }
};

action "console:deploy" appliesTo {
  principal: [Subject],
  resource: [Resource],
  context: { "purpose"?: String }
};
"#;

/// The row-visibility action for an object policy.
pub const OBJECT_POLICY_ACTION: &str = "view";
/// The field-visibility action for a property policy.
pub const PROPERTY_POLICY_ACTION: &str = "read_field";

// ---------------------------------------------------------------------------
// No-code authoring: blocks → normalized row → generated Cedar text
// ---------------------------------------------------------------------------

/// Whether an authored policy grants or denies. `forbid` always wins in Cedar,
/// so it is the shape for tenant-isolation / legal-hold guardrails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Permit,
    Forbid,
}

impl Effect {
    /// Database and canonical-wire representation of this effect.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.keyword()
    }

    const fn keyword(self) -> &'static str {
        match self {
            Self::Permit => "permit",
            Self::Forbid => "forbid",
        }
    }
}

/// Comparison used by one authored condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOp {
    /// `attr == value`
    Eq,
    /// `attr != value`
    Ne,
    /// set-membership: `principal.<set>.contains("value")`
    Contains,
}

/// The right-hand side of a condition.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ConditionValue {
    /// A string literal, rendered quoted (`"..."`).
    Literal(String),
    /// A reference to a subject attribute, rendered `principal.<attr>` unquoted.
    /// Only whitelisted subject attrs are allowed (see [`SUBJECT_ATTRS`]).
    SubjectAttr(String),
    /// A boolean literal, rendered `true`/`false`.
    Bool(bool),
}

/// One AND-ed condition on the policy's `when` clause. `attr` is always a
/// `resource.<attr>` (or a `principal.<set>` for [`ConditionOp::Contains`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Condition {
    /// Left-hand attribute name (bare, e.g. `owner`, `resource_type`, `roles`).
    pub attr: String,
    pub op: ConditionOp,
    pub value: ConditionValue,
}

/// The no-code authoring blocks the UI builds. Deliberately narrow: an authored
/// policy is one effect, one action, one resource-type, and a set of AND-ed
/// conditions. `OR`/nested groups are a future widening (§5d grammar note).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NoCodeBlocks {
    pub effect: Effect,
    /// Must be one of the [`AUTHORING_SCHEMA`] actions or validation fails closed.
    pub action: String,
    pub resource_type: String,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// Whitelisted resource attributes that may appear on the left of a condition.
const RESOURCE_ATTRS: &[&str] = &["resource_type", "owner", "branch", "legal_hold"];
/// The subset of [`RESOURCE_ATTRS`] declared optional (`?`) in the schema. Cedar
/// strict validation forbids reading an optional attribute without a `has` guard,
/// so a condition on one of these emits `resource has <attr> && …`.
const OPTIONAL_RESOURCE_ATTRS: &[&str] = &["owner", "branch", "legal_hold"];

/// A property an object type declares, admitted into policies scoped to that type.
///
/// The execution layer has always supported this: `residual::lower` binds the
/// attribute key as a value and reads the instance's `attributes` JSONB, with no
/// whitelist. Only the authoring validator was still limited to the four generic
/// `Resource` attributes, so a policy over a declared property could be *executed*
/// but never *authored* — and an enforced row carrying one failed the read path
/// closed. Widening happens through the schema, not around it: each declared
/// property is spliced into the `Resource` entity so Cedar strict validation still
/// proves every attribute read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredAttr {
    pub key: String,
    /// `true` renders Cedar `Bool`, `false` renders `String`.
    pub boolean: bool,
}

impl DeclaredAttr {
    const fn cedar_type(&self) -> &'static str {
        if self.boolean { "Bool" } else { "String" }
    }
}

/// The exact text declared properties are spliced after. A unit test asserts it
/// still occurs exactly once in [`AUTHORING_SCHEMA`], so editing the schema can
/// never silently disable object-scoped policies.
const SCHEMA_SPLICE_ANCHOR: &str = "\n  \"legal_hold\"?: Bool\n};";

/// [`AUTHORING_SCHEMA`] widened with an object type's declared properties, each
/// optional so an instance missing the attribute fails the `when` closed.
fn object_authoring_schema(declared: &[DeclaredAttr]) -> String {
    if declared.is_empty() {
        return AUTHORING_SCHEMA.to_owned();
    }
    let extra: String = declared
        .iter()
        .map(|attr| format!(",\n  {}?: {}", quote(&attr.key), attr.cedar_type()))
        .collect();
    AUTHORING_SCHEMA.replacen(
        SCHEMA_SPLICE_ANCHOR,
        &format!("\n  \"legal_hold\"?: Bool{extra}\n}};"),
        1,
    )
}

fn declares(declared: &[DeclaredAttr], attr: &str) -> bool {
    declared.iter().any(|candidate| candidate.key == attr)
}
/// Whitelisted subject attributes referenceable on the right of a condition or
/// as the set for `contains`.
const SUBJECT_ATTRS: &[&str] = &["org", "user_id", "roles", "clearance_keys"];
/// Actions the authoring schema declares.
// `console:configure` (internal-employee console configuration) and
// `console:deploy` (elevated / 민감정보+ clearance deployment) are console-authority
// verbs (design change-log 86). They are authorizable but carry NO seeded permit,
// so deny-by-omission denies external/candidate principals (configure) and any
// principal lacking the clearance (deploy) fail-closed until an org authors a
// scoped permit through the no-code builder.
const AUTHORING_ACTIONS: &[&str] = &[
    "view",
    "edit",
    "read_field",
    "console:configure",
    "console:deploy",
];

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A resource-type stable key, mirroring the object-type CHECK
/// `^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$` (`0152:20`, `0165:94`).
///
/// Dots matter: an object type may legitimately be keyed `hr.employee`, and the
/// runtime matches an attached policy by `block.resource_type == stable_key`.
/// Refusing a dot here would make every dotted object type permanently
/// un-policyable and therefore — once the single-instance reads are gated —
/// permanently invisible on every read path. That is a hole, not a fix.
///
/// This is not a widening of the injection boundary: `resource_type` is emitted
/// only through [`quote`] as a JSON-escaped string literal, where a dot is inert.
/// On every non-dot axis this predicate is strictly TIGHTER than [`is_ident`],
/// which it is spelled in terms of so the two cannot drift: each segment is an
/// ident that additionally has to START with a letter, so `1abc`, `_x` and `123`
/// were accepted before and are refused now. An empty segment — `""`, `.a`,
/// `a.`, `a..b` — is refused by `is_ident`.
fn is_stable_key(s: &str) -> bool {
    s.split('.')
        .all(|segment| is_ident(segment) && segment.starts_with(|c: char| c.is_ascii_lowercase()))
}

/// Validate no-code blocks against the closed attribute/action whitelist and the
/// resource-type stable-key shape. This is the boundary that keeps generated
/// Cedar text un-injectable: every emitted token comes from a whitelist or a
/// shape-checked literal, never raw pass-through.
fn check_blocks_with(blocks: &NoCodeBlocks, declared: &[DeclaredAttr]) -> Result<(), KernelError> {
    for attr in declared {
        if !is_ident(&attr.key) {
            return Err(KernelError::validation(format!(
                "declared property {:?} is not a [a-z0-9_] key",
                attr.key
            )));
        }
    }
    if !AUTHORING_ACTIONS.contains(&blocks.action.as_str()) {
        return Err(KernelError::validation(format!(
            "unknown authoring action {:?}: expected one of {AUTHORING_ACTIONS:?}",
            blocks.action
        )));
    }
    if !is_stable_key(&blocks.resource_type) {
        return Err(KernelError::validation(
            "resource_type must be a dot-separated [a-z][a-z0-9_]* stable key",
        ));
    }
    for cond in &blocks.conditions {
        match cond.op {
            ConditionOp::Contains => {
                if !SUBJECT_ATTRS.contains(&cond.attr.as_str()) {
                    return Err(KernelError::validation(format!(
                        "`contains` requires a subject set attribute, got {:?}",
                        cond.attr
                    )));
                }
                if !matches!(cond.value, ConditionValue::Literal(_)) {
                    return Err(KernelError::validation(
                        "`contains` requires a string-literal value",
                    ));
                }
            }
            ConditionOp::Eq | ConditionOp::Ne => {
                if !RESOURCE_ATTRS.contains(&cond.attr.as_str()) && !declares(declared, &cond.attr)
                {
                    return Err(KernelError::validation(format!(
                        "condition attribute {:?} is neither a whitelisted resource attribute \
                         nor a property declared by this object type",
                        cond.attr
                    )));
                }
            }
        }
        if let ConditionValue::SubjectAttr(attr) = &cond.value
            && !SUBJECT_ATTRS.contains(&attr.as_str())
        {
            return Err(KernelError::validation(format!(
                "subject-attr reference {attr:?} is not whitelisted"
            )));
        }
        // Literals may contain arbitrary text; they are always emitted through
        // Cedar's JSON string escaping below, never concatenated raw.
    }
    Ok(())
}

/// Escape a string literal for embedding in Cedar source. Cedar uses the same
/// escapes as JSON for the characters that matter here, so `serde_json` gives a
/// correct, injection-safe quoted literal.
fn quote(s: &str) -> String {
    serde_json::Value::String(s.to_owned()).to_string()
}

fn render_condition(cond: &Condition, declared: &[DeclaredAttr]) -> String {
    match cond.op {
        ConditionOp::Contains => {
            // check_blocks rejects a non-literal `contains`; if generation runs on
            // already-invalid blocks (so the draft still gets storable, non-empty
            // text), emit a deliberately-mismatched literal — the draft's
            // validation_status is `invalid` regardless, so it can never submit.
            let lit = match &cond.value {
                ConditionValue::Literal(lit) => lit.clone(),
                ConditionValue::SubjectAttr(attr) => attr.clone(),
                ConditionValue::Bool(b) => b.to_string(),
            };
            format!("principal.{}.contains({})", cond.attr, quote(&lit))
        }
        ConditionOp::Eq | ConditionOp::Ne => {
            let op = if cond.op == ConditionOp::Eq {
                "=="
            } else {
                "!="
            };
            let rhs = match &cond.value {
                ConditionValue::Literal(lit) => quote(lit),
                ConditionValue::SubjectAttr(attr) => format!("principal.{attr}"),
                ConditionValue::Bool(b) => b.to_string(),
            };
            // Guard optional attrs so Cedar strict validation can prove the read
            // is safe; an absent attr makes the whole `when` false (fail-closed).
            if OPTIONAL_RESOURCE_ATTRS.contains(&cond.attr.as_str())
                || declares(declared, &cond.attr)
            {
                format!(
                    "resource has {attr} && resource.{attr} {op} {rhs}",
                    attr = cond.attr
                )
            } else {
                format!("resource.{} {op} {rhs}", cond.attr)
            }
        }
    }
}

/// Generate the real Cedar policy text for `blocks`.
///
/// The `resource_type` guard is always emitted so a policy only applies to its
/// authored object type; authored conditions are AND-ed after it.
#[must_use]
pub fn generate_cedar_text(blocks: &NoCodeBlocks) -> String {
    generate_cedar_text_with(blocks, &[])
}

/// [`generate_cedar_text`] for a policy scoped to an object type that declares
/// `declared`; those properties render with a `has` guard, like the optional
/// generic attributes.
#[must_use]
pub fn generate_cedar_text_with(blocks: &NoCodeBlocks, declared: &[DeclaredAttr]) -> String {
    let mut clauses = vec![format!(
        "resource.resource_type == {}",
        quote(&blocks.resource_type)
    )];
    clauses.extend(
        blocks
            .conditions
            .iter()
            .map(|cond| render_condition(cond, declared)),
    );
    format!(
        "{}(\n  principal,\n  action == Action::{},\n  resource\n)\nwhen {{ {} }};\n",
        blocks.effect.keyword(),
        quote(&blocks.action),
        clauses.join(" && ")
    )
}

/// Canonical, order-stable JSON projection of the blocks — the `normalized_row`
/// persisted alongside the draft. Deliberately NOT the raw `blocks` JSON so the
/// stored row is stable regardless of client key ordering.
#[must_use]
pub fn normalize(blocks: &NoCodeBlocks) -> serde_json::Value {
    serde_json::json!({
        "effect": blocks.effect,
        "action": blocks.action,
        "resource_type": blocks.resource_type,
        "conditions": blocks.conditions,
    })
}

/// Result of validating a draft's generated policy against [`AUTHORING_SCHEMA`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DraftValidation {
    pub generated_policy_text: String,
    pub normalized_row: serde_json::Value,
    pub valid: bool,
    pub errors: Vec<String>,
}

/// Normalize + generate + strict-validate no-code blocks in one call.
///
/// Reuses [`engine::compile_bundle_from_sources`], so a draft is `valid` iff the
/// generated policy parses AND passes Cedar strict validation with zero warnings
/// — the same fail-closed bar the shadow lane uses. Malformed blocks (unknown
/// action, non-whitelisted attr) short-circuit to `invalid` before Cedar runs.
///
/// [`engine::compile_bundle_from_sources`]: super::engine::compile_bundle_from_sources
#[must_use]
pub fn validate_blocks(org_id: OrgId, blocks: &NoCodeBlocks) -> DraftValidation {
    validate_blocks_with(org_id, blocks, &[])
}

/// [`validate_blocks`] for a policy scoped to an object type, admitting that
/// type's declared properties in addition to the generic `Resource` attributes.
#[must_use]
pub fn validate_blocks_with(
    org_id: OrgId,
    blocks: &NoCodeBlocks,
    declared: &[DeclaredAttr],
) -> DraftValidation {
    let normalized_row = normalize(blocks);
    // Always generate non-empty text so the draft row is storable (the drafts
    // table CHECKs char_length > 0); `valid` is the authoritative signal.
    let generated_policy_text = generate_cedar_text_with(blocks, declared);
    if let Err(err) = check_blocks_with(blocks, declared) {
        return DraftValidation {
            generated_policy_text,
            normalized_row,
            valid: false,
            errors: vec![err.message],
        };
    }
    match compile_bundle_from_sources(
        org_id,
        1,
        AUTHORING_SCHEMA_VERSION,
        &object_authoring_schema(declared),
        &generated_policy_text,
    ) {
        Ok(_) => DraftValidation {
            generated_policy_text,
            normalized_row,
            valid: true,
            errors: Vec::new(),
        },
        Err(err) => DraftValidation {
            generated_policy_text,
            normalized_row,
            valid: false,
            errors: vec![err.message],
        },
    }
}

// ---------------------------------------------------------------------------
// Draft review lifecycle (four-eyes)
// ---------------------------------------------------------------------------

/// Draft review state, mirroring the `cedar_policy_drafts.review_status` CHECK.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Draft,
    ReviewPending,
    Rejected,
    ApprovedForPromotion,
}

impl ReviewStatus {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::ReviewPending => "review_pending",
            Self::Rejected => "rejected",
            Self::ApprovedForPromotion => "approved_for_promotion",
        }
    }

    pub fn from_db_str(raw: &str) -> Result<Self, KernelError> {
        match raw {
            "draft" => Ok(Self::Draft),
            "review_pending" => Ok(Self::ReviewPending),
            "rejected" => Ok(Self::Rejected),
            "approved_for_promotion" => Ok(Self::ApprovedForPromotion),
            _ => Err(KernelError::validation(format!(
                "unknown draft review_status {raw:?}"
            ))),
        }
    }
}

/// A four-eyes review decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approve,
    Reject,
}

/// Compute the review status after `submit`. A draft may be submitted for review
/// only from `draft`/`rejected` and only when its last validation was `valid`
/// (mirrors the `review_status <> 'review_pending' OR validation = 'valid'` CHECK).
pub fn submit_draft(
    current: ReviewStatus,
    validation_valid: bool,
) -> Result<ReviewStatus, KernelError> {
    if !validation_valid {
        return Err(KernelError::validation(
            "a draft must validate before it can be submitted for review",
        ));
    }
    match current {
        ReviewStatus::Draft | ReviewStatus::Rejected => Ok(ReviewStatus::ReviewPending),
        ReviewStatus::ReviewPending => {
            Err(KernelError::conflict("draft is already pending review"))
        }
        ReviewStatus::ApprovedForPromotion => Err(KernelError::conflict(
            "draft is already approved for promotion",
        )),
    }
}

/// Compute the review status after a four-eyes decision. The reviewer MUST differ
/// from the author (self-review blocked), and the draft must be `review_pending`.
/// Approval yields `approved_for_promotion` — NOT an enforced/shadow row;
/// promotion is a separate gated lane.
pub fn review_draft(
    current: ReviewStatus,
    decision: ReviewDecision,
    author: console_kernel_core::UserId,
    reviewer: console_kernel_core::UserId,
) -> Result<ReviewStatus, KernelError> {
    if author == reviewer {
        return Err(KernelError::forbidden(
            "self-review is not allowed: reviewer must differ from the draft author",
        ));
    }
    if current != ReviewStatus::ReviewPending {
        return Err(KernelError::conflict(
            "only a review_pending draft can be reviewed",
        ));
    }
    Ok(match decision {
        ReviewDecision::Approve => ReviewStatus::ApprovedForPromotion,
        ReviewDecision::Reject => ReviewStatus::Rejected,
    })
}

// ---------------------------------------------------------------------------
// Point-decision evaluation (simulate / authorize)
// ---------------------------------------------------------------------------

/// Server-loaded subject for a point decision.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SimSubject {
    pub org: OrgId,
    pub user_id: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub clearance_keys: Vec<String>,
}

/// Server-loaded resource (an ontology row) for a point decision.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SimResource {
    pub org: OrgId,
    pub resource_type: String,
    #[serde(default)]
    pub resource_id: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub legal_hold: Option<bool>,
}

/// One point-decision request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SimRequest {
    pub subject: SimSubject,
    pub action: String,
    pub resource: SimResource,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub field: Option<String>,
}

/// Final effect of a point decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimEffect {
    Allow,
    Deny,
}

impl SimEffect {
    #[must_use]
    pub const fn is_allow(self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// Auditable outcome of [`simulate`]: the decision, the policy ids that
/// determined it (Cedar `reason`), and any evaluation-error diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SimulationOutcome {
    pub effect: SimEffect,
    /// Policy ids Cedar reports as determining the decision (empty on a
    /// deny-by-omission, since no policy matched).
    pub determining_policies: Vec<String>,
    /// Cedar evaluation-error strings; a non-empty list is itself a fail-closed
    /// signal (the decision is reported as it was computed, never widened).
    pub errors: Vec<String>,
    pub reason: String,
}

impl SimulationOutcome {
    fn deny(reason: impl Into<String>) -> Self {
        Self {
            effect: SimEffect::Deny,
            determining_policies: Vec::new(),
            errors: Vec::new(),
            reason: reason.into(),
        }
    }
}

/// One authored policy: a stable id + its Cedar source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoredPolicy {
    pub id: String,
    pub text: String,
}

impl AuthoredPolicy {
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
        }
    }
}

/// Evaluate `request` against the authored `policies` — the single evaluator
/// behind both `/policy/simulate` (what-if policy set) and `/policy/authorize`
/// (the org's live enforced set).
///
/// Fail-closed like [`engine::evaluate`]: any parse/build error becomes a
/// recorded Deny, an unknown action is a Deny (deny-by-omission), and the whole
/// Cedar call is `catch_unwind`-isolated so a Cedar panic can never escape as an
/// exception. Deny-by-omission is the default and `forbid` always wins, both by
/// Cedar's own authorization semantics.
///
/// [`engine::evaluate`]: super::engine::evaluate
#[must_use]
pub fn simulate(policies: &[AuthoredPolicy], request: &SimRequest) -> SimulationOutcome {
    match std::panic::catch_unwind(AssertUnwindSafe(|| simulate_inner(policies, request))) {
        Ok(outcome) => outcome,
        Err(_) => SimulationOutcome::deny("cedar simulate panicked"),
    }
}

fn simulate_inner(policies: &[AuthoredPolicy], request: &SimRequest) -> SimulationOutcome {
    if !AUTHORING_ACTIONS.contains(&request.action.as_str()) {
        // Unknown action is not a declared schema action ⇒ deny-by-omission.
        return SimulationOutcome::deny(format!(
            "action {:?} is not authorizable; deny-by-omission",
            request.action
        ));
    }

    let schema = match Schema::from_str(AUTHORING_SCHEMA) {
        Ok(schema) => schema,
        Err(err) => {
            return SimulationOutcome::deny(format!("authoring schema parse failed: {err}"));
        }
    };

    let mut policy_set = PolicySet::new();
    for authored in policies {
        let policy_id = PolicyId::from_str(&authored.id)
            .unwrap_or_else(|_| PolicyId::from_str("p").expect("literal id is valid"));
        let policy = match Policy::parse(Some(policy_id), &authored.text) {
            Ok(policy) => policy,
            Err(err) => {
                // A malformed stored policy fails the WHOLE decision closed: we
                // never silently drop it and evaluate a narrower (more-permissive
                // for forbids) set.
                return SimulationOutcome::deny(format!(
                    "authored policy {:?} failed to parse: {err}",
                    authored.id
                ));
            }
        };
        if let Err(err) = policy_set.add(policy) {
            return SimulationOutcome::deny(format!(
                "authored policy {:?} could not be added: {err}",
                authored.id
            ));
        }
    }

    let entities = match build_entities(request, &schema) {
        Ok(entities) => entities,
        Err(reason) => return SimulationOutcome::deny(reason),
    };
    let cedar_request = match build_request(request, &schema) {
        Ok(cedar_request) => cedar_request,
        Err(reason) => return SimulationOutcome::deny(reason),
    };

    let response = Authorizer::new().is_authorized(&cedar_request, &policy_set, &entities);
    let determining_policies = response
        .diagnostics()
        .reason()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let errors = response
        .diagnostics()
        .errors()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    match response.decision() {
        Decision::Allow => SimulationOutcome {
            effect: SimEffect::Allow,
            reason: "allow: a permit matched and no forbid did".to_owned(),
            determining_policies,
            errors,
        },
        Decision::Deny => {
            let reason = if determining_policies.is_empty() {
                "deny-by-omission: no permit matched".to_owned()
            } else {
                "deny: a forbid matched (forbid always wins)".to_owned()
            };
            SimulationOutcome {
                effect: SimEffect::Deny,
                reason,
                determining_policies,
                errors,
            }
        }
    }
}

fn entity_uid(type_name: &str, id: &str) -> Result<EntityUid, String> {
    let parsed = EntityTypeName::from_str(type_name)
        .map_err(|err| format!("entity type {type_name:?} invalid: {err}"))?;
    Ok(EntityUid::from_type_name_and_id(parsed, EntityId::new(id)))
}

fn build_entities(request: &SimRequest, schema: &Schema) -> Result<Entities, String> {
    let subject_uid = entity_uid("Subject", &request.subject.user_id)?;
    let subject_attrs = HashMap::from([
        (
            "org".to_owned(),
            RestrictedExpression::new_string(request.subject.org.to_string()),
        ),
        (
            "user_id".to_owned(),
            RestrictedExpression::new_string(request.subject.user_id.clone()),
        ),
        (
            "roles".to_owned(),
            RestrictedExpression::new_set(
                request
                    .subject
                    .roles
                    .iter()
                    .cloned()
                    .map(RestrictedExpression::new_string),
            ),
        ),
        (
            "clearance_keys".to_owned(),
            RestrictedExpression::new_set(
                request
                    .subject
                    .clearance_keys
                    .iter()
                    .cloned()
                    .map(RestrictedExpression::new_string),
            ),
        ),
    ]);
    let subject = Entity::new(subject_uid, subject_attrs, HashSet::new())
        .map_err(|err| format!("subject entity failed: {err}"))?;

    let resource_eid = request
        .resource
        .resource_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| request.resource.resource_type.clone());
    let resource_uid = entity_uid("Resource", &resource_eid)?;
    let mut resource_attrs = HashMap::from([
        (
            "org".to_owned(),
            RestrictedExpression::new_string(request.resource.org.to_string()),
        ),
        (
            "resource_type".to_owned(),
            RestrictedExpression::new_string(request.resource.resource_type.clone()),
        ),
    ]);
    if let Some(resource_id) = &request.resource.resource_id {
        resource_attrs.insert(
            "resource_id".to_owned(),
            RestrictedExpression::new_string(resource_id.clone()),
        );
    }
    if let Some(owner) = &request.resource.owner {
        resource_attrs.insert(
            "owner".to_owned(),
            RestrictedExpression::new_string(owner.clone()),
        );
    }
    if let Some(branch) = &request.resource.branch {
        resource_attrs.insert(
            "branch".to_owned(),
            RestrictedExpression::new_string(branch.clone()),
        );
    }
    if let Some(legal_hold) = request.resource.legal_hold {
        resource_attrs.insert(
            "legal_hold".to_owned(),
            RestrictedExpression::new_bool(legal_hold),
        );
    }
    let resource = Entity::new(resource_uid, resource_attrs, HashSet::new())
        .map_err(|err| format!("resource entity failed: {err}"))?;

    Entities::from_entities([subject, resource], Some(schema))
        .map_err(|err| format!("entities failed: {err}"))
}

fn build_request(request: &SimRequest, schema: &Schema) -> Result<Request, String> {
    let subject_uid = entity_uid("Subject", &request.subject.user_id)?;
    let action_uid = entity_uid("Action", &request.action)?;
    let resource_eid = request
        .resource
        .resource_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| request.resource.resource_type.clone());
    let resource_uid = entity_uid("Resource", &resource_eid)?;

    let mut context_pairs: Vec<(String, RestrictedExpression)> = Vec::new();
    if request.action == PROPERTY_POLICY_ACTION {
        if let Some(field) = &request.field {
            context_pairs.push((
                "field".to_owned(),
                RestrictedExpression::new_string(field.clone()),
            ));
        }
    } else if let Some(purpose) = &request.purpose {
        context_pairs.push((
            "purpose".to_owned(),
            RestrictedExpression::new_string(purpose.clone()),
        ));
    }
    let context =
        Context::from_pairs(context_pairs).map_err(|err| format!("context failed: {err}"))?;

    Request::new(subject_uid, action_uid, resource_uid, context, Some(schema))
        .map_err(|err| format!("request failed: {err}"))
}

/// Object-policy row visibility: is this row visible to this subject under the
/// authored object policies? A row is hidden unless a `view` permit matches and
/// no `forbid` does (deny-by-omission).
#[must_use]
pub fn object_row_visible(
    policies: &[AuthoredPolicy],
    subject: SimSubject,
    resource: SimResource,
) -> bool {
    simulate(
        policies,
        &SimRequest {
            subject,
            action: OBJECT_POLICY_ACTION.to_owned(),
            resource,
            purpose: None,
            field: None,
        },
    )
    .effect
    .is_allow()
}

/// Property-policy field visibility: is `field` readable by this subject on this
/// row? A field is nulled unless a `read_field` permit matches and no forbid does.
#[must_use]
pub fn property_field_visible(
    policies: &[AuthoredPolicy],
    subject: SimSubject,
    resource: SimResource,
    field: impl Into<String>,
) -> bool {
    simulate(
        policies,
        &SimRequest {
            subject,
            action: PROPERTY_POLICY_ACTION.to_owned(),
            resource,
            purpose: None,
            field: Some(field.into()),
        },
    )
    .effect
    .is_allow()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use console_kernel_core::UserId;

    fn owner_view_permit(resource_type: &str) -> NoCodeBlocks {
        NoCodeBlocks {
            effect: Effect::Permit,
            action: "view".to_owned(),
            resource_type: resource_type.to_owned(),
            conditions: vec![Condition {
                attr: "owner".to_owned(),
                op: ConditionOp::Eq,
                value: ConditionValue::SubjectAttr("user_id".to_owned()),
            }],
        }
    }

    fn subject(user_id: &str) -> SimSubject {
        SimSubject {
            org: OrgId::knl(),
            user_id: user_id.to_owned(),
            roles: vec![],
            clearance_keys: vec![],
        }
    }

    fn row(owner: &str) -> SimResource {
        SimResource {
            org: OrgId::knl(),
            resource_type: "work_order".to_owned(),
            resource_id: Some("wo-1".to_owned()),
            owner: Some(owner.to_owned()),
            branch: None,
            legal_hold: None,
        }
    }

    fn flagged_forbid(resource_type: &str) -> NoCodeBlocks {
        NoCodeBlocks {
            effect: Effect::Forbid,
            action: "view".to_owned(),
            resource_type: resource_type.to_owned(),
            conditions: vec![Condition {
                attr: "flagged".to_owned(),
                op: ConditionOp::Eq,
                value: ConditionValue::Bool(true),
            }],
        }
    }

    fn flagged_declared() -> Vec<DeclaredAttr> {
        vec![DeclaredAttr {
            key: "flagged".to_owned(),
            boolean: true,
        }]
    }

    #[test]
    fn declared_object_property_authors_and_strict_validates() {
        let v = validate_blocks_with(
            OrgId::knl(),
            &flagged_forbid("policycase"),
            &flagged_declared(),
        );
        assert!(
            v.valid,
            "a policy over a declared object property must author: {:?}",
            v.errors
        );
        // Optional in the spliced schema, so an instance without the attribute
        // fails the `when` closed rather than erroring.
        assert!(
            v.generated_policy_text
                .contains("resource has flagged && resource.flagged == true"),
            "declared properties must carry a has-guard: {}",
            v.generated_policy_text
        );
    }

    #[test]
    fn undeclared_object_property_is_still_rejected() {
        let v = validate_blocks_with(OrgId::knl(), &flagged_forbid("policycase"), &[]);
        assert!(!v.valid, "an undeclared property must not author");
        assert!(
            v.errors.iter().any(|e| e.contains("flagged")),
            "error must name the offending attribute: {:?}",
            v.errors
        );
    }

    #[test]
    fn declared_property_type_mismatch_fails_cedar_validation() {
        // Declared Bool, compared against a string literal: the spliced schema
        // makes this decidable, so strict validation rejects it.
        let mut blocks = flagged_forbid("policycase");
        blocks.conditions[0].value = ConditionValue::Literal("yes".to_owned());
        let v = validate_blocks_with(OrgId::knl(), &blocks, &flagged_declared());
        assert!(
            !v.valid,
            "Bool property vs string literal must not validate"
        );
    }

    #[test]
    fn schema_splice_anchor_is_exact() {
        assert_eq!(
            AUTHORING_SCHEMA.matches(SCHEMA_SPLICE_ANCHOR).count(),
            1,
            "editing AUTHORING_SCHEMA must not silently disable object-scoped policies"
        );
        assert_eq!(
            object_authoring_schema(&[]),
            AUTHORING_SCHEMA,
            "no declared properties must leave the schema byte-identical"
        );
        assert!(
            object_authoring_schema(&flagged_declared()).contains("\"flagged\"?: Bool"),
            "declared properties must reach the Resource entity"
        );
    }

    #[test]
    fn owner_permit_generates_and_validates() {
        let v = validate_blocks(OrgId::knl(), &owner_view_permit("work_order"));
        assert!(
            v.valid,
            "owner-view permit must strict-validate: {:?}",
            v.errors
        );
        assert!(v.generated_policy_text.contains("permit("));
        assert!(
            v.generated_policy_text
                .contains("resource.owner == principal.user_id")
        );
    }

    #[test]
    fn unknown_action_is_invalid() {
        let mut blocks = owner_view_permit("work_order");
        blocks.action = "delete_everything".to_owned();
        let v = validate_blocks(OrgId::knl(), &blocks);
        assert!(
            !v.valid,
            "an undeclared action must be rejected fail-closed"
        );
    }

    #[test]
    fn non_whitelisted_attr_is_invalid() {
        let mut blocks = owner_view_permit("work_order");
        blocks.conditions[0].attr = "secret_backdoor".to_owned();
        let v = validate_blocks(OrgId::knl(), &blocks);
        assert!(!v.valid, "a non-whitelisted attribute must be rejected");
    }

    #[test]
    fn authorize_denies_by_omission_with_no_policies() {
        let out = simulate(
            &[],
            &SimRequest {
                subject: subject("alice"),
                action: "view".to_owned(),
                resource: row("alice"),
                purpose: None,
                field: None,
            },
        );
        assert_eq!(out.effect, SimEffect::Deny);
        assert!(out.determining_policies.is_empty());
    }

    #[test]
    fn object_policy_hides_cross_principal_row() {
        let text = generate_cedar_text(&owner_view_permit("work_order"));
        let policies = [AuthoredPolicy::new("owner_view", text)];
        // Alice owns the row ⇒ visible.
        assert!(object_row_visible(
            &policies,
            subject("alice"),
            row("alice")
        ));
        // Bob does not own it ⇒ hidden (deny-by-omission).
        assert!(!object_row_visible(&policies, subject("bob"), row("alice")));
    }

    #[test]
    fn forbid_always_wins_over_permit() {
        let permit = generate_cedar_text(&owner_view_permit("work_order"));
        let forbid = generate_cedar_text(&NoCodeBlocks {
            effect: Effect::Forbid,
            action: "view".to_owned(),
            resource_type: "work_order".to_owned(),
            conditions: vec![Condition {
                attr: "legal_hold".to_owned(),
                op: ConditionOp::Eq,
                value: ConditionValue::Bool(true),
            }],
        });
        let policies = [
            AuthoredPolicy::new("owner_view", permit),
            AuthoredPolicy::new("legal_hold_forbid", forbid),
        ];
        // Alice owns the row, but it is under legal hold: forbid wins ⇒ hidden.
        let mut held = row("alice");
        held.legal_hold = Some(true);
        let out = simulate(
            &policies,
            &SimRequest {
                subject: subject("alice"),
                action: "view".to_owned(),
                resource: held,
                purpose: None,
                field: None,
            },
        );
        assert_eq!(out.effect, SimEffect::Deny, "forbid must win: {out:?}");
        // Same subject/row without the hold is visible again.
        assert!(object_row_visible(
            &policies,
            subject("alice"),
            row("alice")
        ));
    }

    #[test]
    fn simulate_reports_determining_policy_and_no_errors() {
        let text = generate_cedar_text(&owner_view_permit("work_order"));
        let policies = [AuthoredPolicy::new("owner_view", text)];
        let out = simulate(
            &policies,
            &SimRequest {
                subject: subject("alice"),
                action: "view".to_owned(),
                resource: row("alice"),
                purpose: None,
                field: None,
            },
        );
        assert_eq!(out.effect, SimEffect::Allow);
        assert_eq!(out.determining_policies, vec!["owner_view".to_owned()]);
        assert!(out.errors.is_empty(), "clean allow has no diagnostics");
    }

    #[test]
    fn submit_requires_valid_and_review_blocks_self() {
        assert!(submit_draft(ReviewStatus::Draft, false).is_err());
        assert_eq!(
            submit_draft(ReviewStatus::Draft, true).unwrap(),
            ReviewStatus::ReviewPending
        );
        let author = UserId::new();
        let reviewer = UserId::new();
        assert!(
            review_draft(
                ReviewStatus::ReviewPending,
                ReviewDecision::Approve,
                author,
                author
            )
            .is_err(),
            "self-review must be blocked"
        );
        assert_eq!(
            review_draft(
                ReviewStatus::ReviewPending,
                ReviewDecision::Approve,
                author,
                reviewer
            )
            .unwrap(),
            ReviewStatus::ApprovedForPromotion
        );
    }

    /// Defect (c): an object type may be keyed `hr.employee` (`0152:20`), and the
    /// runtime matches an attached policy by `block.resource_type == stable_key`.
    /// A dotted key must therefore validate, while every shape the object-type
    /// CHECK refuses must still be refused here.
    #[test]
    fn resource_type_accepts_dotted_stable_keys_and_still_refuses_everything_else() {
        for accepted in ["work_order", "hr.employee", "a.b.c", "a1_2.b3"] {
            let validation = validate_blocks(OrgId::knl(), &owner_view_permit(accepted));
            assert!(
                validation.valid,
                "{accepted:?} must be policyable: {:?}",
                validation.errors
            );
        }
        for refused in [
            "", ".a", "a.", "a..b", "1abc", "_x", "A", "a b", "a\"b", "a-b", "a.B", "a.1b",
        ] {
            let validation = validate_blocks(OrgId::knl(), &owner_view_permit(refused));
            assert!(!validation.valid, "{refused:?} must be refused");
            assert!(
                validation
                    .errors
                    .iter()
                    .any(|error| error.contains("stable key")),
                "{refused:?} must be refused BY THE KEY SHAPE, not incidentally: {:?}",
                validation.errors
            );
        }
    }
}
