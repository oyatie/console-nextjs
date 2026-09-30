//! Ontology registry domain.
//!
//! Pure types + rules for the §18 registry: the schema-lifecycle FSM (§3a), the
//! object-type backing kind, link cardinality, action dispatch, and the field
//! discriminated-union tag whose reader degrades on unknown types (§3c). No I/O,
//! no serde_json — persistence + wire concerns live in the adapter crate.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;

/// Defines a UUID-backed ID newtype (local copy of the kernel-core idiom; the
/// kernel macro is not exported, and this lane keeps its ids self-contained).
macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
            serde::Serialize, serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(uuid::Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4())
            }
            #[must_use]
            pub const fn from_uuid(value: uuid::Uuid) -> Self {
                Self(value)
            }
            #[must_use]
            pub const fn as_uuid(&self) -> &uuid::Uuid {
                &self.0
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

typed_id!(
    /// One VERSION snapshot of an object-type schema (one row per
    /// `(org, stable_key, schema_version)`); children reference this id.
    ObjectTypeId
);
typed_id!(PropertyDefId);
typed_id!(LinkTypeId);
typed_id!(ActionTypeId);
typed_id!(AnalyticId);

typed_id!(
    /// One user-authored object instance (`OT-…` type). Head row pointing at its
    /// latest revision; the fixity-chained revisions carry the effective-dated
    /// state (§1b).
    InstanceId
);
typed_id!(
    /// One append-only, fixity-stamped revision of an instance's attribute bag.
    InstanceRevisionId
);
typed_id!(
    /// One effective-dated edge between two instances (§2 traversal walks these).
    InstanceLinkId
);

// ---------------------------------------------------------------------------
// Schema lifecycle FSM (§3a): draft → review_pending → published → superseded
//                             → retired. Direct draft→published is forbidden
// once protection is on; edits then must pass through review (four-eyes).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaLifecycleState {
    Draft,
    ReviewPending,
    Published,
    Superseded,
    Retired,
}

impl SchemaLifecycleState {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::ReviewPending => "review_pending",
            Self::Published => "published",
            Self::Superseded => "superseded",
            Self::Retired => "retired",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "draft" => Ok(Self::Draft),
            "review_pending" => Ok(Self::ReviewPending),
            "published" => Ok(Self::Published),
            "superseded" => Ok(Self::Superseded),
            "retired" => Ok(Self::Retired),
            other => Err(KernelError::validation(format!(
                "unknown schema lifecycle state {other:?}"
            ))),
        }
    }
}

/// Validate a schema-lifecycle transition. `protection_enabled` gates the
/// direct draft→published shortcut: when protection is on, a schema change must
/// pass through `review_pending` (proposal + four-eyes) before it can publish.
///
/// `published → superseded` is the internal step taken when a newer version of
/// the same key publishes; it is a legal transition here so the adapter can
/// retire the prior head atomically.
pub fn validate_schema_transition(
    from: SchemaLifecycleState,
    to: SchemaLifecycleState,
    protection_enabled: bool,
) -> Result<SchemaLifecycleState, KernelError> {
    use SchemaLifecycleState::{Draft, Published, Retired, ReviewPending, Superseded};
    let allowed = match (from, to) {
        (Draft, ReviewPending) => true,
        // Direct-to-published is only allowed when protection is OFF.
        (Draft, Published) => !protection_enabled,
        (ReviewPending, Published) => true,
        // Reviewer sends the proposal back for edits.
        (ReviewPending, Draft) => true,
        // A newer version publishing supersedes the current head.
        (Published, Superseded) => true,
        // Retire a live or superseded type (terminal).
        (Published | Superseded, Retired) => true,
        _ => false,
    };
    if allowed {
        Ok(to)
    } else {
        Err(KernelError::conflict(format!(
            "illegal schema lifecycle transition {} -> {}",
            from.as_db_str(),
            to.as_db_str()
        )))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackingKind {
    /// Projects an existing domain table (WO / employee / equipment / …).
    Projected,
    /// User-authored type with an owned effective-dated instance store.
    Instance,
}

impl BackingKind {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Projected => "projected",
            Self::Instance => "instance",
        }
    }
    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "projected" => Ok(Self::Projected),
            "instance" => Ok(Self::Instance),
            other => Err(KernelError::validation(format!(
                "unknown backing kind {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkCardinality {
    OneOne,
    OneMany,
    ManyMany,
}

impl LinkCardinality {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::OneOne => "one_one",
            Self::OneMany => "one_many",
            Self::ManyMany => "many_many",
        }
    }
    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "one_one" => Ok(Self::OneOne),
            "one_many" => Ok(Self::OneMany),
            "many_many" => Ok(Self::ManyMany),
            other => Err(KernelError::validation(format!(
                "unknown link cardinality {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionDispatch {
    /// Route the writeback through a domain crate's existing use-case.
    ProjectedUsecase,
    /// Append a revision to the owned instance store.
    InstanceRevision,
}

impl ActionDispatch {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::ProjectedUsecase => "projected_usecase",
            Self::InstanceRevision => "instance_revision",
        }
    }
    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "projected_usecase" => Ok(Self::ProjectedUsecase),
            "instance_revision" => Ok(Self::InstanceRevision),
            other => Err(KernelError::validation(format!(
                "unknown action dispatch {other:?}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Instance lifecycle FSM (§3b): draft → active → (locked?) → archived → disposed.
// Archive is reversible; dispose is terminal (no hard delete anywhere, §9.8).
// Per-object-type FSM config (gov_lifecycle_transitions) is L-GOV's concern; this
// is the built-in default transition table the instance store validates against.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceLifecycleState {
    Draft,
    Active,
    Locked,
    Archived,
    Disposed,
}

impl InstanceLifecycleState {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Locked => "locked",
            Self::Archived => "archived",
            Self::Disposed => "disposed",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "draft" => Ok(Self::Draft),
            "active" => Ok(Self::Active),
            "locked" => Ok(Self::Locked),
            "archived" => Ok(Self::Archived),
            "disposed" => Ok(Self::Disposed),
            other => Err(KernelError::validation(format!(
                "unknown instance lifecycle state {other:?}"
            ))),
        }
    }
}

/// Validate an instance lifecycle transition against the built-in FSM (§3b).
/// `disposed` is terminal; `archived → active` is the reversible restore.
pub fn validate_instance_transition(
    from: InstanceLifecycleState,
    to: InstanceLifecycleState,
) -> Result<InstanceLifecycleState, KernelError> {
    use InstanceLifecycleState::{Active, Archived, Disposed, Draft, Locked};
    let allowed = match (from, to) {
        (Draft, Active) => true,
        // Discard a never-activated draft.
        (Draft, Archived) => true,
        (Active, Locked | Archived) => true,
        // Unlock back to active, or archive a locked instance.
        (Locked, Active | Archived) => true,
        // Archive is reversible (restore), or terminally disposed.
        (Archived, Active | Disposed) => true,
        _ => false,
    };
    if allowed {
        Ok(to)
    } else {
        Err(KernelError::conflict(format!(
            "illegal instance lifecycle transition {} -> {}",
            from.as_db_str(),
            to.as_db_str()
        )))
    }
}

// ---------------------------------------------------------------------------
// Field discriminated-union tag (§3c). New field types ship with zero
// migration: the tag is stored as free text, and the READER degrades any
// unrecognized tag to `Unknown` instead of failing — so an older binary never
// crashes on a type a newer authoring surface introduced.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FieldKind {
    Text,
    Integer,
    Decimal,
    Boolean,
    Date,
    Timestamp,
    Choice,
    MultiChoice,
    Reference,
    Attachment,
    GeoPoint,
    Json,
    /// Any tag this build does not recognize. Carries the raw string so it can
    /// still be round-tripped, listed, and stored — never a hard error.
    Unknown(String),
}

impl FieldKind {
    /// Parse a stored `type` tag. Unrecognized tags degrade to
    /// [`FieldKind::Unknown`] (forward-compatible; never fails).
    #[must_use]
    pub fn parse(tag: &str) -> Self {
        match tag {
            "text" => Self::Text,
            "integer" => Self::Integer,
            "decimal" => Self::Decimal,
            "boolean" => Self::Boolean,
            "date" => Self::Date,
            "timestamp" => Self::Timestamp,
            "choice" => Self::Choice,
            "multi_choice" => Self::MultiChoice,
            "reference" => Self::Reference,
            "attachment" => Self::Attachment,
            "geo_point" => Self::GeoPoint,
            "json" => Self::Json,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// The canonical stored tag. `Unknown` echoes the raw string it carried.
    #[must_use]
    pub fn as_tag(&self) -> &str {
        match self {
            Self::Text => "text",
            Self::Integer => "integer",
            Self::Decimal => "decimal",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::Timestamp => "timestamp",
            Self::Choice => "choice",
            Self::MultiChoice => "multi_choice",
            Self::Reference => "reference",
            Self::Attachment => "attachment",
            Self::GeoPoint => "geo_point",
            Self::Json => "json",
            Self::Unknown(raw) => raw,
        }
    }

    /// Whether this build recognizes the tag. `false` for [`FieldKind::Unknown`].
    #[must_use]
    pub const fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SchemaLifecycleState::{Draft, Published, Retired, ReviewPending, Superseded};

    #[test]
    fn protection_forbids_direct_draft_to_published() {
        assert!(validate_schema_transition(Draft, Published, true).is_err());
        // The reviewed path is always open.
        assert!(validate_schema_transition(Draft, ReviewPending, true).is_ok());
        assert!(validate_schema_transition(ReviewPending, Published, true).is_ok());
    }

    #[test]
    fn without_protection_draft_may_publish_directly() {
        assert_eq!(
            validate_schema_transition(Draft, Published, false).unwrap(),
            Published
        );
    }

    #[test]
    fn supersede_and_retire_are_legal_terminal_moves() {
        assert!(validate_schema_transition(Published, Superseded, true).is_ok());
        assert!(validate_schema_transition(Published, Retired, true).is_ok());
        assert!(validate_schema_transition(Superseded, Retired, true).is_ok());
    }

    #[test]
    fn illegal_jumps_are_rejected() {
        assert!(validate_schema_transition(Retired, Published, false).is_err());
        assert!(validate_schema_transition(Superseded, Published, false).is_err());
        assert!(validate_schema_transition(Published, Draft, false).is_err());
    }

    #[test]
    fn instance_fsm_allows_the_forward_path_and_reversible_archive() {
        use InstanceLifecycleState::{Active, Archived, Disposed, Draft, Locked};
        assert!(validate_instance_transition(Draft, Active).is_ok());
        assert!(validate_instance_transition(Active, Locked).is_ok());
        assert!(validate_instance_transition(Locked, Active).is_ok());
        assert!(validate_instance_transition(Active, Archived).is_ok());
        // Archive is reversible.
        assert!(validate_instance_transition(Archived, Active).is_ok());
        // Dispose is terminal.
        assert!(validate_instance_transition(Archived, Disposed).is_ok());
    }

    #[test]
    fn instance_fsm_rejects_illegal_and_post_dispose_moves() {
        use InstanceLifecycleState::{Active, Disposed, Draft, Locked};
        // Can't jump draft straight to disposed, or resurrect a disposed instance.
        assert!(validate_instance_transition(Draft, Disposed).is_err());
        assert!(validate_instance_transition(Disposed, Active).is_err());
        // Locked can't be disposed without archiving first.
        assert!(validate_instance_transition(Locked, Disposed).is_err());
    }

    #[test]
    fn instance_lifecycle_state_roundtrips() {
        for st in [
            InstanceLifecycleState::Draft,
            InstanceLifecycleState::Active,
            InstanceLifecycleState::Locked,
            InstanceLifecycleState::Archived,
            InstanceLifecycleState::Disposed,
        ] {
            assert_eq!(
                InstanceLifecycleState::from_db_str(st.as_db_str()).unwrap(),
                st
            );
        }
    }

    #[test]
    fn field_kind_degrades_unknown_tag_without_panicking() {
        // A tag a newer authoring surface introduced.
        let kind = FieldKind::parse("quantum_spinor");
        assert_eq!(kind, FieldKind::Unknown("quantum_spinor".to_owned()));
        assert!(!kind.is_known());
        // Round-trips the raw tag so storage/listing still work.
        assert_eq!(kind.as_tag(), "quantum_spinor");
        // Known tags parse and echo back.
        assert!(FieldKind::parse("choice").is_known());
        assert_eq!(FieldKind::parse("choice").as_tag(), "choice");
    }

    #[test]
    fn field_kind_known_tags_roundtrip_and_are_known() {
        for tag in [
            "text",
            "integer",
            "decimal",
            "boolean",
            "date",
            "timestamp",
            "choice",
            "multi_choice",
            "reference",
            "attachment",
            "geo_point",
            "json",
        ] {
            let kind = FieldKind::parse(tag);
            assert!(kind.is_known(), "tag {tag} should be known");
            assert_eq!(kind.as_tag(), tag);
            assert_eq!(FieldKind::parse(kind.as_tag()), kind);
        }
    }

    #[test]
    fn instance_fsm_allows_discarding_a_never_activated_draft() {
        use InstanceLifecycleState::{Archived, Draft};
        assert!(validate_instance_transition(Draft, Archived).is_ok());
    }

    // --- W0-ONT-1: schema/registry wire + reviewer send-back ---

    #[test]
    fn schema_lifecycle_state_roundtrips() {
        for st in [Draft, ReviewPending, Published, Superseded, Retired] {
            assert_eq!(
                SchemaLifecycleState::from_db_str(st.as_db_str()).unwrap(),
                st
            );
        }
    }

    #[test]
    fn backing_kind_link_cardinality_and_action_dispatch_roundtrip() {
        for kind in [BackingKind::Projected, BackingKind::Instance] {
            assert_eq!(BackingKind::from_db_str(kind.as_db_str()).unwrap(), kind);
        }
        for card in [
            LinkCardinality::OneOne,
            LinkCardinality::OneMany,
            LinkCardinality::ManyMany,
        ] {
            assert_eq!(
                LinkCardinality::from_db_str(card.as_db_str()).unwrap(),
                card
            );
        }
        for dispatch in [
            ActionDispatch::ProjectedUsecase,
            ActionDispatch::InstanceRevision,
        ] {
            assert_eq!(
                ActionDispatch::from_db_str(dispatch.as_db_str()).unwrap(),
                dispatch
            );
        }
    }

    #[test]
    fn registry_enum_wire_tags_reject_unknown() {
        // Closed sets (DB CHECK + domain). Unlike FieldKind display tags, these
        // never degrade to Unknown — unknown wire is a hard validation error.
        assert!(SchemaLifecycleState::from_db_str("quantum_draft").is_err());
        assert!(SchemaLifecycleState::from_db_str("").is_err());
        assert!(SchemaLifecycleState::from_db_str("Published").is_err()); // case-sensitive
        assert!(BackingKind::from_db_str("hybrid").is_err());
        assert!(LinkCardinality::from_db_str("zero_many").is_err());
        assert!(ActionDispatch::from_db_str("direct_sql").is_err());
        assert!(InstanceLifecycleState::from_db_str("soft_deleted").is_err());
    }

    #[test]
    fn schema_reviewer_may_return_proposal_to_draft() {
        // Four-eyes send-back: reviewer rejects proposal → author edits again.
        assert_eq!(
            validate_schema_transition(ReviewPending, Draft, true).unwrap(),
            Draft
        );
        // Protection flag only gates draft→published; send-back stays open either way.
        assert_eq!(
            validate_schema_transition(ReviewPending, Draft, false).unwrap(),
            Draft
        );
    }
}
