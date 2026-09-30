//! List-filtering residual (arch §5d, decision D1): lower OUR OWN no-code
//! condition grammar to a parameterized SQL `WHERE` fragment.
//!
//! We do NOT use Cedar's experimental `is_authorized_partial`. Instead, for a
//! list endpoint, the caller collects the object-policies applicable to
//! `(subject, action, object_type)` from the catalog — permit/forbid rows plus
//! their AND-ed field conditions — and lowers them here into a fragment that
//! composes as `WHERE <RLS org floor (already armed)> AND <residual>`. The
//! residual can only NARROW, never widen: the RLS tenant floor is unconditional
//! and independent (arch §6).
//!
//! Fail-closed, by construction:
//!  * **deny-by-omission** — zero applicable permit ⇒ [`ResidualFilter::deny_all`]
//!    (`WHERE FALSE`, no rows). Every list starts denied.
//!  * **`forbid` always wins** — each forbid lowers to an `AND NOT (…)` clause a
//!    permit can never out-permit; an unconditional forbid collapses the whole
//!    filter to `FALSE`.
//!  * **no silent drop** — ANY untranslatable term (unknown projected column,
//!    a subject attribute the request doesn't carry, an op/type combination the
//!    grammar can't lower) collapses the WHOLE filter to `FALSE`. A term we can't
//!    prove safe is NEVER dropped and NEVER interpolated.
//!
//! Values are bound, never string-formatted (`sqlx` binds only). Instance field
//! keys are bound into the JSON path (`attributes ->> $k`) so no key is
//! interpolated either; only projected column identifiers are spliced, and those
//! pass the [`crate::is_safe_column`] gate first.
//!
//! `ponytail:` narrow row-predicate grammar (field · op · literal|subject-attr,
//! AND within a policy, OR across permits). Nested boolean groups and
//! subject-only `contains` gating are a later widening — add them when a real
//! authored policy needs list-filtering with one.

use std::collections::BTreeMap;

use super::authoring::Effect;

/// A value bound into the query — never string-interpolated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlValue {
    Text(String),
    Int(i64),
    Bool(bool),
    /// Membership set for `∈`, bound as a Postgres `text[]`.
    TextArray(Vec<String>),
}

/// Comparison operator a condition lowers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidualOp {
    /// `=`
    Eq,
    /// `≠`
    Ne,
    /// `≥` (numeric only)
    Ge,
    /// `≤` (numeric only)
    Le,
    /// `∈` — field value is in a set (literal set or subject set attribute).
    In,
}

/// The right-hand side of a condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredicateValue {
    /// A literal, bound directly.
    Literal(SqlValue),
    /// A literal set for `∈`, bound as `text[]`.
    LiteralSet(Vec<String>),
    /// A reference to a subject attribute, resolved at lower-time to the
    /// request's concrete value(s). A scalar attr (`user_id`) resolves to a bound
    /// literal; a set attr (`roles`) resolves to a bound `text[]` (valid only for
    /// `∈`). A reference the subject does not carry is untranslatable ⇒ deny-all.
    SubjectAttr(String),
}

/// One AND-ed condition on an object policy: `<field> <op> <value>`. `field`
/// names an authored property key; how it maps to SQL is the [`LoweringTarget`]'s
/// job (a real column for projected types, `attributes ->> key` for instances).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Predicate {
    pub field: String,
    pub op: ResidualOp,
    pub value: PredicateValue,
}

/// One applicable object policy: its effect plus the AND-ed conditions of its
/// `when` clause. An empty condition list is the unconditional policy (permit ⇒
/// everything visible; forbid ⇒ nothing visible).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPolicy {
    pub effect: Effect,
    pub predicates: Vec<Predicate>,
}

/// The request's server-loaded subject attributes, used to resolve
/// [`PredicateValue::SubjectAttr`] references to concrete binds. Scalars are
/// single-valued attrs (`user_id`, `org`, `branch`); sets are multi-valued
/// (`roles`, `clearance_keys`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubjectAttrs {
    pub scalars: BTreeMap<String, String>,
    pub sets: BTreeMap<String, Vec<String>>,
}

impl SubjectAttrs {
    #[must_use]
    pub fn with_scalar(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.scalars.insert(key.into(), value.into());
        self
    }

    #[must_use]
    pub fn with_set(mut self, key: impl Into<String>, values: Vec<String>) -> Self {
        self.sets.insert(key.into(), values);
        self
    }
}

/// How a `field` maps to a SQL column reference.
#[derive(Debug, Clone, Copy)]
pub enum LoweringTarget<'a> {
    /// Projected type: `field` → a real column via this map. A field absent from
    /// the map, or a column that fails the identifier gate, is untranslatable.
    Projected {
        columns: &'a BTreeMap<String, String>,
    },
    /// Instance type: `field` → `<attributes_column> ->> $field` (key bound), the
    /// text value cast per the compared literal's type. `attributes_column` is a
    /// caller-supplied, gate-checked column reference (e.g. `r.attributes`).
    Instance { attributes_column: &'a str },
}

/// The lowered filter: a boolean SQL fragment plus its ordered bind values. The
/// fragment references placeholders `$first_bind ..` passed to [`lower`]; the
/// caller binds `binds` in order at those positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResidualFilter {
    pub where_sql: String,
    pub binds: Vec<SqlValue>,
}

impl ResidualFilter {
    /// The fail-closed filter: matches no rows, binds nothing.
    #[must_use]
    pub fn deny_all() -> Self {
        Self {
            where_sql: "FALSE".to_owned(),
            binds: Vec::new(),
        }
    }

    /// Whether this filter denies everything (the fail-closed sentinel).
    #[must_use]
    pub fn is_deny_all(&self) -> bool {
        self.where_sql == "FALSE"
    }
}

/// Lower the applicable object policies for one list request into a SQL `WHERE`
/// fragment. `first_bind` is the first `$N` placeholder the fragment may use
/// (e.g. `2` when the caller already binds `$1`).
///
/// The `(subject, action, object_type)` triple selects which policies apply —
/// that collection is the caller's query against the catalog; here `policies` is
/// the already-collected applicable set and `subject` supplies the concrete
/// attribute values referenced by conditions.
#[must_use]
pub fn lower(
    target: LoweringTarget<'_>,
    subject: &SubjectAttrs,
    policies: &[ObjectPolicy],
    first_bind: usize,
) -> ResidualFilter {
    // Gate the instance attributes column once; an unsafe reference is fatal.
    if let LoweringTarget::Instance { attributes_column } = target
        && !crate::is_safe_column(attributes_column)
    {
        return ResidualFilter::deny_all();
    }

    let mut emitter = Emitter {
        next: first_bind,
        binds: Vec::new(),
    };
    let mut permits: Vec<String> = Vec::new();
    let mut forbids: Vec<String> = Vec::new();

    for policy in policies {
        let clause = match lower_policy(target, subject, policy, &mut emitter) {
            Ok(clause) => clause,
            // Untranslatable term anywhere ⇒ the WHOLE filter denies. Never drop.
            Err(Untranslatable) => return ResidualFilter::deny_all(),
        };
        match policy.effect {
            Effect::Permit => permits.push(clause),
            Effect::Forbid => forbids.push(clause),
        }
    }

    // Deny-by-omission: no applicable permit ⇒ nothing is visible.
    if permits.is_empty() {
        return ResidualFilter::deny_all();
    }

    // COALESCE guards SQL three-valued logic (a condition on an absent JSON
    // attribute evaluates to NULL): an undetermined permit denies (fail-closed),
    // and an undetermined forbid does NOT exclude — a forbid only fires when its
    // clause is definitively TRUE. Without this, a `NOT NULL` would silently drop
    // a row the forbid was never meant to touch.
    let mut sql = format!("COALESCE(({}), FALSE)", permits.join(" OR "));
    for forbid in forbids {
        // forbid always wins: an AND-NOT clause a permit can never out-permit.
        sql = format!("{sql} AND NOT COALESCE({forbid}, FALSE)");
    }

    ResidualFilter {
        where_sql: format!("({sql})"),
        binds: emitter.binds,
    }
}

/// Marker: a term could not be proven safe to lower.
struct Untranslatable;

struct Emitter {
    next: usize,
    binds: Vec<SqlValue>,
}

impl Emitter {
    /// Bind `value`, returning its `$N` placeholder index.
    fn bind(&mut self, value: SqlValue) -> usize {
        let index = self.next;
        self.next += 1;
        self.binds.push(value);
        index
    }
}

fn lower_policy(
    target: LoweringTarget<'_>,
    subject: &SubjectAttrs,
    policy: &ObjectPolicy,
    emitter: &mut Emitter,
) -> Result<String, Untranslatable> {
    if policy.predicates.is_empty() {
        // Unconditional: permit ⇒ TRUE (all rows); forbid ⇒ NOT TRUE ⇒ FALSE.
        return Ok("TRUE".to_owned());
    }
    let parts = policy
        .predicates
        .iter()
        .map(|predicate| lower_predicate(target, subject, predicate, emitter))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("({})", parts.join(" AND ")))
}

fn lower_predicate(
    target: LoweringTarget<'_>,
    subject: &SubjectAttrs,
    predicate: &Predicate,
    emitter: &mut Emitter,
) -> Result<String, Untranslatable> {
    match predicate.op {
        ResidualOp::In => {
            let set = resolve_set(subject, &predicate.value)?;
            // JSON `->>` is always text; membership compares text ∈ text[].
            let lhs = field_lhs(target, &predicate.field, None, emitter)?;
            let value_idx = emitter.bind(SqlValue::TextArray(set));
            Ok(format!("{lhs} = ANY(${value_idx})"))
        }
        ResidualOp::Eq | ResidualOp::Ne | ResidualOp::Ge | ResidualOp::Le => {
            let scalar = resolve_scalar(subject, &predicate.value)?;
            let op = match predicate.op {
                ResidualOp::Eq => "=",
                ResidualOp::Ne => "<>",
                ResidualOp::Ge => ">=",
                ResidualOp::Le => "<=",
                ResidualOp::In => unreachable!("handled above"),
            };
            // `≥`/`≤` are numeric only; a bool compares only for equality.
            match (&scalar, predicate.op) {
                (SqlValue::Int(_), _) => {}
                (SqlValue::Bool(_), ResidualOp::Eq | ResidualOp::Ne) => {}
                (SqlValue::Text(_), ResidualOp::Eq | ResidualOp::Ne) => {}
                _ => return Err(Untranslatable),
            }
            let cast = match &scalar {
                SqlValue::Int(_) => Some("numeric"),
                SqlValue::Bool(_) => Some("boolean"),
                SqlValue::Text(_) => None,
                SqlValue::TextArray(_) => return Err(Untranslatable),
            };
            let lhs = field_lhs(target, &predicate.field, cast, emitter)?;
            let value_idx = emitter.bind(scalar);
            Ok(format!("{lhs} {op} ${value_idx}"))
        }
    }
}

/// The SQL for the field on the left of a condition. For instance types the field
/// key is bound into the JSON path (never interpolated); for projected types the
/// mapped column is gate-checked and spliced.
fn field_lhs(
    target: LoweringTarget<'_>,
    field: &str,
    cast: Option<&str>,
    emitter: &mut Emitter,
) -> Result<String, Untranslatable> {
    match target {
        LoweringTarget::Instance { attributes_column } => {
            let key_idx = emitter.bind(SqlValue::Text(field.to_owned()));
            let base = format!("({attributes_column} ->> ${key_idx})");
            Ok(match cast {
                Some(cast) => format!("{base}::{cast}"),
                None => base,
            })
        }
        LoweringTarget::Projected { columns } => {
            let column = columns.get(field).ok_or(Untranslatable)?;
            if !crate::is_safe_column(column) {
                return Err(Untranslatable);
            }
            // Projected columns are already typed; no cast needed.
            Ok(column.clone())
        }
    }
}

fn resolve_scalar(
    subject: &SubjectAttrs,
    value: &PredicateValue,
) -> Result<SqlValue, Untranslatable> {
    match value {
        PredicateValue::Literal(literal) => match literal {
            SqlValue::TextArray(_) => Err(Untranslatable),
            other => Ok(other.clone()),
        },
        PredicateValue::LiteralSet(_) => Err(Untranslatable),
        PredicateValue::SubjectAttr(name) => match subject.scalars.get(name) {
            Some(text) => Ok(SqlValue::Text(text.clone())),
            // A set attr where a scalar is required, or an absent attr: fail closed.
            None => Err(Untranslatable),
        },
    }
}

fn resolve_set(
    subject: &SubjectAttrs,
    value: &PredicateValue,
) -> Result<Vec<String>, Untranslatable> {
    match value {
        PredicateValue::LiteralSet(values) => Ok(values.clone()),
        PredicateValue::SubjectAttr(name) => subject.sets.get(name).cloned().ok_or(Untranslatable),
        PredicateValue::Literal(_) => Err(Untranslatable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ATTR: &str = "r.attributes";

    fn instance() -> LoweringTarget<'static> {
        LoweringTarget::Instance {
            attributes_column: ATTR,
        }
    }

    fn eq(field: &str, value: PredicateValue) -> Predicate {
        Predicate {
            field: field.to_owned(),
            op: ResidualOp::Eq,
            value,
        }
    }

    fn permit(predicates: Vec<Predicate>) -> ObjectPolicy {
        ObjectPolicy {
            effect: Effect::Permit,
            predicates,
        }
    }

    fn forbid(predicates: Vec<Predicate>) -> ObjectPolicy {
        ObjectPolicy {
            effect: Effect::Forbid,
            predicates,
        }
    }

    #[test]
    fn owner_scoping_lowers_subject_attr_to_a_bound_literal() {
        let subject = SubjectAttrs::default().with_scalar("user_id", "alice");
        let policies = [permit(vec![eq(
            "owner",
            PredicateValue::SubjectAttr("user_id".to_owned()),
        )])];
        let filter = lower(instance(), &subject, &policies, 2);
        // key bound at $2, value at $3 — nothing interpolated.
        assert_eq!(
            filter.where_sql,
            "(COALESCE((((r.attributes ->> $2) = $3)), FALSE))"
        );
        assert_eq!(
            filter.binds,
            vec![
                SqlValue::Text("owner".to_owned()),
                SqlValue::Text("alice".to_owned())
            ]
        );
    }

    #[test]
    fn no_permit_denies_by_omission() {
        // Only a forbid, or nothing at all ⇒ WHERE FALSE.
        let filter = lower(instance(), &SubjectAttrs::default(), &[], 2);
        assert!(filter.is_deny_all());
        let only_forbid = [forbid(vec![])];
        let filter = lower(instance(), &SubjectAttrs::default(), &only_forbid, 2);
        assert!(filter.is_deny_all());
    }

    #[test]
    fn forbid_is_and_not_and_can_never_be_out_permitted() {
        let subject = SubjectAttrs::default().with_scalar("user_id", "alice");
        let policies = [
            permit(vec![eq(
                "owner",
                PredicateValue::SubjectAttr("user_id".to_owned()),
            )]),
            forbid(vec![eq(
                "legal_hold",
                PredicateValue::Literal(SqlValue::Bool(true)),
            )]),
        ];
        let filter = lower(instance(), &subject, &policies, 2);
        assert!(
            filter.where_sql.contains("AND NOT"),
            "forbid must lower to AND NOT: {}",
            filter.where_sql
        );
        // The forbid's bool literal is bound (::boolean cast), not formatted.
        assert!(filter.where_sql.contains("::boolean"));
        assert!(filter.binds.contains(&SqlValue::Bool(true)));
    }

    #[test]
    fn unconditional_forbid_collapses_whole_filter_to_false() {
        let policies = [permit(vec![]), forbid(vec![])];
        let filter = lower(instance(), &SubjectAttrs::default(), &policies, 2);
        // (TRUE) AND NOT TRUE  ⇒ never matches. The SQL is still boolean-false.
        assert_eq!(
            filter.where_sql,
            "(COALESCE((TRUE), FALSE) AND NOT COALESCE(TRUE, FALSE))"
        );
    }

    #[test]
    fn untranslatable_term_collapses_whole_filter_never_drops() {
        // A subject attr the request does not carry ⇒ deny-all (not a dropped term
        // that would silently widen).
        let policies = [permit(vec![eq(
            "owner",
            PredicateValue::SubjectAttr("does_not_exist".to_owned()),
        )])];
        let filter = lower(instance(), &SubjectAttrs::default(), &policies, 2);
        assert!(filter.is_deny_all());
        assert!(filter.binds.is_empty());
    }

    #[test]
    fn ge_requires_numeric_and_casts_on_instance() {
        let policies = [permit(vec![Predicate {
            field: "severity".to_owned(),
            op: ResidualOp::Ge,
            value: PredicateValue::Literal(SqlValue::Int(3)),
        }])];
        let filter = lower(instance(), &SubjectAttrs::default(), &policies, 2);
        assert_eq!(
            filter.where_sql,
            "(COALESCE((((r.attributes ->> $2)::numeric >= $3)), FALSE))"
        );
        assert_eq!(filter.binds[1], SqlValue::Int(3));

        // ≥ on a text literal is out of grammar ⇒ deny-all.
        let bad = [permit(vec![Predicate {
            field: "owner".to_owned(),
            op: ResidualOp::Ge,
            value: PredicateValue::Literal(SqlValue::Text("x".to_owned())),
        }])];
        assert!(lower(instance(), &SubjectAttrs::default(), &bad, 2).is_deny_all());
    }

    #[test]
    fn membership_lowers_to_any_over_a_bound_array() {
        let subject = SubjectAttrs::default().with_set(
            "allowed_branches",
            vec!["seoul".to_owned(), "busan".to_owned()],
        );
        let policies = [permit(vec![Predicate {
            field: "branch".to_owned(),
            op: ResidualOp::In,
            value: PredicateValue::SubjectAttr("allowed_branches".to_owned()),
        }])];
        let filter = lower(instance(), &subject, &policies, 2);
        assert_eq!(
            filter.where_sql,
            "(COALESCE((((r.attributes ->> $2) = ANY($3))), FALSE))"
        );
        assert_eq!(
            filter.binds[1],
            SqlValue::TextArray(vec!["seoul".to_owned(), "busan".to_owned()])
        );
    }

    #[test]
    fn projected_target_maps_field_to_a_gate_checked_column() {
        let mut columns = BTreeMap::new();
        columns.insert("owner".to_owned(), "owner_user_id".to_owned());
        let subject = SubjectAttrs::default().with_scalar("user_id", "alice");
        let policies = [permit(vec![eq(
            "owner",
            PredicateValue::SubjectAttr("user_id".to_owned()),
        )])];
        let filter = lower(
            LoweringTarget::Projected { columns: &columns },
            &subject,
            &policies,
            2,
        );
        assert_eq!(
            filter.where_sql,
            "(COALESCE(((owner_user_id = $2)), FALSE))"
        );
        assert_eq!(filter.binds, vec![SqlValue::Text("alice".to_owned())]);

        // A field with no column mapping ⇒ deny-all (unknown ⇒ fail closed).
        let unmapped = [permit(vec![eq(
            "secret",
            PredicateValue::Literal(SqlValue::Text("x".to_owned())),
        )])];
        assert!(
            lower(
                LoweringTarget::Projected { columns: &columns },
                &subject,
                &unmapped,
                2
            )
            .is_deny_all()
        );
    }

    #[test]
    fn unsafe_attributes_column_denies_all() {
        let target = LoweringTarget::Instance {
            attributes_column: "r.attributes; DROP TABLE users",
        };
        let policies = [permit(vec![])];
        assert!(lower(target, &SubjectAttrs::default(), &policies, 2).is_deny_all());
    }
}
