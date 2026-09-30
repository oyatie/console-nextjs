//! Effective-dated grants — ADR-0032 §1, §3 and §6.
//!
//! A 발령 (appointment) has a date; a grant row until now did not. The interval
//! goes on the grant and only on the grant, and it is **half-open**:
//! `[valid_from, valid_to)`. `valid_from` is inside, `valid_to` is outside. That
//! is not a stylistic choice — it is what makes "this 발령 ends the instant the
//! next one begins" expressible without the two claiming a shared instant, and
//! it matches the predicate `PgInstanceStore::get_as_of` already ships.
//!
//! §6 is the load-bearing clause here: crossing `valid_from` or `valid_to` is
//! **not a write**, so no `bump_subject_version_tx` fires and no monotone
//! freshness counter can notice it. An interval therefore must never be cited as
//! a reason to skip a per-request evaluation, and expiry is enforced by the read
//! predicate rather than by a status-flipping job. [`GrantValidity::contains`]
//! is consulted at the instant the decision is made
//! ([`crate::authorize_scoped`]) and the result is never carried across it.

use console_kernel_core::{KernelError, Timestamp};

/// A grant's half-open validity interval `[valid_from, valid_to)`.
///
/// `valid_to == None` is an open-ended grant. BOTH ends `None` is
/// [`GrantValidity::always`] — the pre-ADR-0032 shape, a grant whose row carries
/// no interval at all, which is how every grant resolved today looks.
///
/// # `{None, None}` is never produced by OMISSION
///
/// [`Self::always`] is "effective at every instant". Anything that can produce
/// that value WITHOUT saying so is a fail-open, because the shape a truncated,
/// version-skewed or hand-built payload degrades to is the empty one:
///
/// * There is no `Default`. A derived one produced `{None, None}` from nothing
///   at all, so `GrantValidity::default()` — and any `..Default::default()`
///   struct update reaching this field — was an unconditionally live grant that
///   never named an interval.
/// * [`Deserialize`](serde::Deserialize) is written by hand, below. The derived
///   one wrote these private fields directly, skipping [`Self::half_open`]: an
///   inverted or empty interval that the constructor rejects arrived through it
///   as a live grant, and — because `serde` fills a missing `Option` field with
///   `None` rather than failing — `{}` deserialized into `{None, None}`.
///
/// So there are exactly two entrances, and each one is a statement: calling
/// [`Self::always`], or a payload whose two bounds are BOTH present and BOTH
/// null — the form [`Serialize`](serde::Serialize) emits for it. Every partial
/// payload is still refused.
///
/// # A defaulted validity does not compile
///
/// THE CONTROL. The named constructor is a real, callable path, and this is the
/// value the failing example below wants:
///
/// ```
/// use console_platform_authz::GrantValidity;
///
/// let always: GrantValidity = GrantValidity::always();
/// ```
///
/// The same binding taken from `Default` instead, with nothing else changed:
///
/// ```compile_fail,E0277
/// use console_platform_authz::GrantValidity;
///
/// let always: GrantValidity = Default::default();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct GrantValidity {
    valid_from: Option<Timestamp>,
    valid_to: Option<Timestamp>,
}

/// The wire form goes through [`GrantValidity::half_open`], so a payload cannot
/// assert an interval the constructor would have refused.
///
/// BOTH bounds must be PRESENT. `serde` fills a missing `Option` field with
/// `None` instead of failing, and each absence widens a grant on its own: a
/// dropped `valid_from` reads as "effective since forever" and a dropped
/// `valid_to` turns a bounded 발령 into one that never expires. A truncated
/// payload must deny, so each bound is nested one level deeper: the outer
/// `Option` is "the field was there at all" (`serde` cannot express that on a
/// bare `Option` — its missing-field shim answers `deserialize_option` with
/// `None`, so absent and explicit-null are otherwise the same value), and the
/// inner one is the null the payload really did carry.
///
/// # An explicitly null PAIR is `always()`, and it is the only null that widens
///
/// It is the value [`GrantValidity::always`] serialises to, and the row the
/// still-unwritten `valid_from`/`valid_to` columns produce when a 발령 records
/// no interval. Refusing it — while [`Serialize`](serde::Serialize) kept
/// emitting it — made this type unable to read back its own output, an
/// invariant that reads as a guard and is a latent decode failure on the first
/// payload that carries one.
///
/// It is not the fail-open shape, because it is not a shape anything degrades
/// TO: a truncated, version-skewed or hand-built payload loses fields, and
/// every missing field is still a refusal. Saying `always` takes both bounds,
/// spelled out and null. One bound null and the other present stays a refusal —
/// that is a half-lost interval, and the widening this guards.
impl<'de> serde::Deserialize<'de> for GrantValidity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        fn present<'de, D>(deserializer: D) -> Result<Option<Option<Timestamp>>, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            Option::<Timestamp>::deserialize(deserializer).map(Some)
        }

        #[derive(serde::Deserialize)]
        struct Wire {
            #[serde(default, deserialize_with = "present")]
            valid_from: Option<Option<Timestamp>>,
            #[serde(default, deserialize_with = "present")]
            valid_to: Option<Option<Timestamp>>,
        }

        let wire = Wire::deserialize(deserializer)?;
        let valid_from = wire
            .valid_from
            .ok_or_else(|| serde::de::Error::missing_field("valid_from"))?;
        let valid_to = wire
            .valid_to
            .ok_or_else(|| serde::de::Error::missing_field("valid_to"))?;

        match (valid_from, valid_to) {
            (None, None) => Ok(Self::always()),
            (None, Some(_)) => Err(serde::de::Error::custom(
                "a grant with valid_to and no valid_from is a half-lost interval, not an \
                 always-effective grant",
            )),
            (Some(valid_from), valid_to) => Self::half_open(valid_from, valid_to)
                .map_err(|err| serde::de::Error::custom(err.message)),
        }
    }
}

impl GrantValidity {
    /// No interval recorded. Effective at every instant, which is exactly what
    /// every grant resolved from today's `user_role_assignments` means.
    #[must_use]
    pub const fn always() -> Self {
        Self {
            valid_from: None,
            valid_to: None,
        }
    }

    /// `[valid_from, valid_to)`. `None` upper bound means open-ended.
    ///
    /// # Errors
    ///
    /// Rejects `valid_to <= valid_from`. `[t, t)` contains no instant and an
    /// inverted pair contains none either, so both describe a grant that can
    /// never be effective — a data-entry error rather than a policy, and one a
    /// half-open reading would otherwise swallow silently.
    pub fn half_open(
        valid_from: Timestamp,
        valid_to: Option<Timestamp>,
    ) -> Result<Self, KernelError> {
        if valid_to.is_some_and(|end| end <= valid_from) {
            return Err(KernelError::validation(
                "grant validity must be a non-empty half-open interval [valid_from, valid_to)",
            ));
        }
        Ok(Self {
            valid_from: Some(valid_from),
            valid_to,
        })
    }

    /// Whether `at` falls inside `[valid_from, valid_to)`.
    #[must_use]
    pub fn contains(self, at: Timestamp) -> bool {
        self.valid_from.is_none_or(|from| from <= at) && self.valid_to.is_none_or(|to| at < to)
    }

    #[must_use]
    pub const fn valid_from(self) -> Option<Timestamp> {
        self.valid_from
    }

    #[must_use]
    pub const fn valid_to(self) -> Option<Timestamp> {
        self.valid_to
    }

    /// Two half-open intervals share at least one instant.
    ///
    /// `[a, b)` and `[b, c)` do NOT overlap; that is the property the whole
    /// half-open reading exists to provide.
    fn overlaps(self, other: Self) -> bool {
        starts_before_end(self.valid_from, other.valid_to)
            && starts_before_end(other.valid_from, self.valid_to)
    }
}

/// An absent bound is unbounded: `None` start is `-∞`, `None` end is `+∞`.
fn starts_before_end(from: Option<Timestamp>, to: Option<Timestamp>) -> bool {
    match (from, to) {
        (Some(from), Some(to)) => from < to,
        _ => true,
    }
}

/// ADR-0032 §1: `user_role_assignments`' `UNIQUE (org_id, user_id, role_id)`
/// becomes a per-`(org_id, user_id, role_id)` NON-OVERLAP constraint.
///
/// The grouping key is the ROLE. Two different roles covering the same instant
/// is the ordinary case — a user holds several — and rejecting that would be an
/// outage, not a guard. Two rows for the SAME role covering the same instant is
/// the ambiguity: at that instant, two records claim to say when this one
/// authority began.
///
/// The database constraint is the real enforcement and it is not written yet
/// (see this crate's lane notes). This is the same predicate at the trust
/// boundary, so the resolver refuses an ambiguous set rather than folding over
/// it, whichever of the two lands first.
///
/// # Errors
///
/// [`console_kernel_core::ErrorKind::Conflict`] naming the offending role when
/// two of its assignment intervals share any instant.
pub fn enforce_assignment_non_overlap(
    assignments: &[(uuid::Uuid, GrantValidity)],
) -> Result<(), KernelError> {
    // ponytail: O(n²) over one user's role assignments — a handful of rows.
    // Sort-by-(role, valid_from) and compare neighbours if that ever stops being
    // a handful.
    for (index, (role_id, validity)) in assignments.iter().enumerate() {
        for (other_role, other_validity) in assignments.iter().skip(index + 1) {
            if role_id == other_role && validity.overlaps(*other_validity) {
                return Err(KernelError::conflict(format!(
                    "role {role_id} has overlapping effective-dated assignments"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_kernel_core::{ErrorKind, Timestamp};

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_unix_timestamp(secs).unwrap()
    }

    fn role_a() -> uuid::Uuid {
        uuid::Uuid::from_u128(0xa)
    }

    fn role_b() -> uuid::Uuid {
        uuid::Uuid::from_u128(0xb)
    }

    /// THE interval-end test. A test that probes only the middle proves nothing:
    /// the whole risk of a half-open interval is which end is closed.
    #[test]
    fn valid_from_is_inside_the_interval_and_valid_to_is_outside() {
        let validity = GrantValidity::half_open(ts(100), Some(ts(200))).unwrap();

        assert!(!validity.contains(ts(99)), "one second before valid_from");
        assert!(validity.contains(ts(100)), "valid_from itself is INSIDE");
        assert!(validity.contains(ts(199)), "one second before valid_to");
        assert!(!validity.contains(ts(200)), "valid_to itself is OUTSIDE");
        assert!(!validity.contains(ts(201)), "one second after valid_to");
    }

    #[test]
    fn an_open_ended_interval_closes_only_its_lower_end() {
        let validity = GrantValidity::half_open(ts(100), None).unwrap();

        assert!(!validity.contains(ts(99)));
        assert!(validity.contains(ts(100)));
        assert!(validity.contains(ts(4_000_000_000)));
    }

    /// `[t, t)` contains no instant at all, and `[b, a)` with `a < b` is inverted.
    /// A grant that can never be effective is a data-entry error, not a policy.
    #[test]
    fn an_empty_or_inverted_interval_is_rejected() {
        let empty = GrantValidity::half_open(ts(200), Some(ts(200))).unwrap_err();
        assert_eq!(empty.kind, ErrorKind::Validation);

        let inverted = GrantValidity::half_open(ts(200), Some(ts(100))).unwrap_err();
        assert_eq!(inverted.kind, ErrorKind::Validation);
    }

    #[test]
    fn a_grant_with_no_recorded_interval_is_always_effective() {
        let always = GrantValidity::always();

        assert!(always.contains(ts(0)));
        assert!(always.contains(ts(4_000_000_000)));
    }

    /// The half-open payoff: a 발령 that ends the instant the next one begins is
    /// the NORMAL shape, and a closed-interval model would reject it.
    #[test]
    fn touching_half_open_intervals_for_one_role_do_not_overlap() {
        let rows = [
            (
                role_a(),
                GrantValidity::half_open(ts(100), Some(ts(200))).unwrap(),
            ),
            (
                role_a(),
                GrantValidity::half_open(ts(200), Some(ts(300))).unwrap(),
            ),
        ];

        assert!(enforce_assignment_non_overlap(&rows).is_ok());
    }

    /// One second of shared time is an overlap.
    #[test]
    fn intervals_sharing_a_single_instant_for_one_role_are_rejected() {
        let rows = [
            (
                role_a(),
                GrantValidity::half_open(ts(100), Some(ts(201))).unwrap(),
            ),
            (
                role_a(),
                GrantValidity::half_open(ts(200), Some(ts(300))).unwrap(),
            ),
        ];

        let err = enforce_assignment_non_overlap(&rows).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
    }

    #[test]
    fn two_open_ended_intervals_for_one_role_are_rejected() {
        let rows = [
            (role_a(), GrantValidity::half_open(ts(100), None).unwrap()),
            (role_a(), GrantValidity::half_open(ts(900), None).unwrap()),
        ];

        assert!(enforce_assignment_non_overlap(&rows).is_err());
    }

    #[test]
    fn two_interval_less_rows_for_one_role_are_rejected() {
        let rows = [
            (role_a(), GrantValidity::always()),
            (role_a(), GrantValidity::always()),
        ];

        assert!(enforce_assignment_non_overlap(&rows).is_err());
    }

    /// The constraint is per `(org_id, user_id, role_id)`. Two DIFFERENT roles
    /// covering the same instant is the ordinary case — rejecting it would be an
    /// outage, not a guard.
    #[test]
    fn the_same_interval_on_two_roles_is_not_an_overlap() {
        let rows = [
            (
                role_a(),
                GrantValidity::half_open(ts(100), Some(ts(200))).unwrap(),
            ),
            (
                role_b(),
                GrantValidity::half_open(ts(100), Some(ts(200))).unwrap(),
            ),
        ];

        assert!(enforce_assignment_non_overlap(&rows).is_ok());
    }

    /// Order of presentation must not decide the verdict.
    #[test]
    fn overlap_detection_does_not_depend_on_input_order() {
        let early = (
            role_a(),
            GrantValidity::half_open(ts(100), Some(ts(300))).unwrap(),
        );
        let late = (
            role_a(),
            GrantValidity::half_open(ts(200), Some(ts(400))).unwrap(),
        );

        assert!(enforce_assignment_non_overlap(&[early, late]).is_err());
        assert!(enforce_assignment_non_overlap(&[late, early]).is_err());
    }

    #[test]
    fn an_empty_assignment_set_is_not_an_overlap() {
        assert!(enforce_assignment_non_overlap(&[]).is_ok());
    }

    /// The wire is a trust boundary. `serde`'s derived `Deserialize` writes the
    /// private fields directly, so it is a second entrance that skips
    /// [`GrantValidity::half_open`] entirely — and an interval that constructor
    /// rejects as never-effective would arrive through it as a live grant.
    ///
    /// Built by serialising real `Timestamp`s rather than hand-writing their
    /// wire form, so this pins the validation and not the date format.
    #[test]
    fn an_inverted_interval_is_rejected_on_the_wire_too() {
        let inverted = serde_json::json!({
            "valid_from": serde_json::to_value(ts(200)).unwrap(),
            "valid_to": serde_json::to_value(ts(100)).unwrap(),
        });

        serde_json::from_value::<GrantValidity>(inverted)
            .expect_err("an inverted interval must not deserialize");
    }

    #[test]
    fn an_empty_interval_is_rejected_on_the_wire_too() {
        let empty = serde_json::json!({
            "valid_from": serde_json::to_value(ts(200)).unwrap(),
            "valid_to": serde_json::to_value(ts(200)).unwrap(),
        });

        serde_json::from_value::<GrantValidity>(empty)
            .expect_err("an empty interval must not deserialize");
    }

    /// `{None, None}` is what this type's own documentation calls "effective at
    /// every instant". No constructor produces it from a PARTIAL interval and
    /// no wire form may either: one bound null beside a real one is half an
    /// interval, and reading half an interval as an unconditionally live grant
    /// is the definition of failing open.
    #[test]
    fn a_half_null_interval_does_not_deserialize_into_an_always_effective_grant() {
        let half_null = serde_json::json!({
            "valid_from": null,
            "valid_to": serde_json::to_value(ts(100)).unwrap(),
        });

        serde_json::from_value::<GrantValidity>(half_null.clone())
            .expect_err(&format!("{half_null} must not deserialize"));

        // THE CONTROL: the SAME payload with the lower bound restored is an
        // ordinary interval, so the refusal above is the lost bound and not the
        // shape of the fixture.
        serde_json::from_value::<GrantValidity>(serde_json::json!({
            "valid_from": serde_json::to_value(ts(50)).unwrap(),
            "valid_to": serde_json::to_value(ts(100)).unwrap(),
        }))
        .expect("an interval with both bounds must deserialize");
    }

    /// A dated grant carries an interval; that is the entire reason it is not an
    /// ordinary grant. So a payload missing either bound is a refusal, never a
    /// silently-filled default.
    #[test]
    fn an_absent_bound_is_a_refusal_not_a_default() {
        for truncated in [
            serde_json::json!({}),
            serde_json::json!({ "valid_from": serde_json::to_value(ts(100)).unwrap() }),
            serde_json::json!({ "valid_to": serde_json::to_value(ts(100)).unwrap() }),
            // The one that pairs an absence with the null PAIR that IS
            // `always()`: read the missing bound as the null it is beside and
            // half a truncated payload becomes an unconditionally live grant.
            serde_json::json!({ "valid_to": null }),
            serde_json::json!({ "valid_from": null }),
        ] {
            serde_json::from_value::<GrantValidity>(truncated.clone())
                .expect_err(&format!("{truncated} must not deserialize"));
        }
    }

    /// EVERY value this type can hold must survive the round trip, or the tests
    /// above would be satisfied by a `Deserialize` that rejects everything —
    /// and a value the crate's own `Serialize` emits but its `Deserialize`
    /// refuses is a payload that cannot be read back at all.
    ///
    /// The three shapes are the whole domain: `always()`, a closed interval and
    /// an open-ended one. Scoping this to the two that already survived is what
    /// let the asymmetry sit here green.
    #[test]
    fn every_constructible_validity_round_trips_through_serde() {
        for validity in [
            GrantValidity::always(),
            GrantValidity::half_open(ts(100), Some(ts(200))).unwrap(),
            GrantValidity::half_open(ts(100), None).unwrap(),
        ] {
            let wire = serde_json::to_value(validity).unwrap();

            assert_eq!(
                serde_json::from_value::<GrantValidity>(wire.clone()).unwrap(),
                validity,
                "{wire} is what this type serialised; it must read back"
            );
        }
    }
}
