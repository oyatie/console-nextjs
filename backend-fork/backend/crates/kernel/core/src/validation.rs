//! Shared, pure input validation for primitive value bounds.
//!
//! Domain crates own their rich newtypes (EquipmentNo, Ton, …); this module
//! holds only the small, cross-cutting numeric/text bounds that several layers
//! need to agree on. Keeping them here (kernel, no async / no sqlx) lets the
//! REST handler, the application command, and the DB CHECK constraints all
//! reference one source of truth for the same limit.
//!
//! Geographic coordinates are the first such bound: migration 0039 adds
//! latitude/longitude CHECKs to `registry_sites`, and the PATCH /sites handler
//! validates the same WGS84 ranges in the domain so a bad value is rejected with
//! a 422 before a transaction ever opens, rather than surfacing as a DB error.

use crate::error::KernelError;

/// WGS84 latitude bounds (degrees). Matches the
/// `registry_sites_latitude_range` CHECK in migration 0039.
pub const LATITUDE_MIN: f64 = -90.0;
pub const LATITUDE_MAX: f64 = 90.0;

/// WGS84 longitude bounds (degrees). Matches the
/// `registry_sites_longitude_range` CHECK in migration 0039.
pub const LONGITUDE_MIN: f64 = -180.0;
pub const LONGITUDE_MAX: f64 = 180.0;

/// Reject a latitude outside the WGS84 range, or a non-finite value (NaN/∞,
/// which no CHECK can store meaningfully). Returns the value unchanged on
/// success so callers can use it inline.
///
/// # Errors
/// Returns `KernelError::validation` when `value` is not finite or falls outside
/// `[-90, 90]`.
pub fn validate_latitude(value: f64) -> Result<f64, KernelError> {
    if !value.is_finite() {
        return Err(KernelError::validation("latitude must be a finite number"));
    }
    if !(LATITUDE_MIN..=LATITUDE_MAX).contains(&value) {
        return Err(KernelError::validation(format!(
            "latitude must be between {LATITUDE_MIN} and {LATITUDE_MAX}"
        )));
    }
    Ok(value)
}

/// Reject a longitude outside the WGS84 range, or a non-finite value.
///
/// # Errors
/// Returns `KernelError::validation` when `value` is not finite or falls outside
/// `[-180, 180]`.
pub fn validate_longitude(value: f64) -> Result<f64, KernelError> {
    if !value.is_finite() {
        return Err(KernelError::validation("longitude must be a finite number"));
    }
    if !(LONGITUDE_MIN..=LONGITUDE_MAX).contains(&value) {
        return Err(KernelError::validation(format!(
            "longitude must be between {LONGITUDE_MIN} and {LONGITUDE_MAX}"
        )));
    }
    Ok(value)
}

/// Validate an optional latitude/longitude pair for a site coordinate write.
///
/// A pin needs BOTH coordinates or NEITHER (mirrors the
/// `registry_sites_lat_lon_paired` CHECK): supplying exactly one is a
/// validation error, since a half-located site can neither be pinned nor cleanly
/// listed as ungeocoded. When both are present, each is range-checked.
///
/// # Errors
/// Returns `KernelError::validation` when exactly one of `latitude`/`longitude`
/// is `Some`, or when either value is out of range / non-finite.
pub fn validate_coordinate_pair(
    latitude: Option<f64>,
    longitude: Option<f64>,
) -> Result<(), KernelError> {
    match (latitude, longitude) {
        (Some(lat), Some(lon)) => {
            validate_latitude(lat)?;
            validate_longitude(lon)?;
            Ok(())
        }
        (None, None) => Ok(()),
        _ => Err(KernelError::validation(
            "latitude and longitude must be provided together",
        )),
    }
}

/// Max code points for a site representative-contact name (담당자명). Matches the
/// `registry_sites_contact_name_max_chars` CHECK in migration 0040.
pub const CONTACT_NAME_MAX_CHARS: usize = 100;
/// Max code points for a site contact phone (연락처). Matches the
/// `registry_sites_contact_phone_max_chars` CHECK in migration 0040.
pub const CONTACT_PHONE_MAX_CHARS: usize = 40;
/// Max code points for a site contact email. Matches the
/// `registry_sites_contact_email_max_chars` CHECK in migration 0040.
pub const CONTACT_EMAIL_MAX_CHARS: usize = 320;

/// Max code points for a customer or site name (고객명 / 현장명). Matches the
/// `registry_customers_name_max_chars` / `registry_sites_name_max_chars` CHECKs
/// in migration 0047. The base `name <> ''` CHECK (migration 0007) already
/// rejects an empty name; this only bounds the upper length so an over-long
/// value is a 422 at the edge rather than a raw DB CHECK error.
pub const CUSTOMER_SITE_NAME_MAX_CHARS: usize = 200;

/// Max code points for a site street address. Matches the address CHECK in
/// migration 0039 (`char_length(address) <= 500`).
pub const ADDRESS_MAX_CHARS: usize = 500;
/// Max code points for a site province (시/도). Matches migration 0039.
pub const PROVINCE_MAX_CHARS: usize = 100;
/// Max code points for a site city (시/군/구). Matches migration 0039.
pub const CITY_MAX_CHARS: usize = 100;
/// Max code points for a site postal code (우편번호). Matches migration 0039.
pub const POSTAL_CODE_MAX_CHARS: usize = 20;

/// Reject NUL and text longer than `max_chars` Unicode code points (counted via
/// `chars()`, matching the DB CHECK's `char_length`). PostgreSQL text cannot
/// store NUL. `field` names the offending field; NUL-free empty or short text passes.
///
/// # Errors
/// Returns `KernelError::validation` for NUL or a value exceeding `max_chars`.
pub fn validate_bounded_text(
    value: &str,
    max_chars: usize,
    field: &str,
) -> Result<(), KernelError> {
    if value.contains('\0') {
        return Err(KernelError::validation(format!(
            "{field} must not contain NUL"
        )));
    }
    if value.chars().count() > max_chars {
        return Err(KernelError::validation(format!(
            "{field} must be at most {max_chars} characters"
        )));
    }
    Ok(())
}

/// Default geofence radius, in metres, for arrival/departure detection at a
/// customer site that has no per-site override (`registry_sites.geofence_radius_m`
/// NULL). A forklift customer yard is larger than a building footprint but small
/// enough that a passing GPS sample should not trigger a false arrival; on-site
/// GPS accuracy is typically 5–30 m, so 300 m clears that jitter with margin and
/// comfortably covers the larger customer yards seen in the field.
pub const DEFAULT_GEOFENCE_RADIUS_M: f64 = 300.0;

/// Great-circle (haversine) distance in **fractional metres** between two WGS84
/// coordinates, using the mean Earth radius (6 371 000 m). This is the single
/// kernel home for the geofence/dispatch distance primitive so domain crates
/// (which may not depend on one another) share it without duplication.
///
/// Use this unrounded form for a threshold compare such as
/// `haversine_meters_f64(..) <= radius_m`: rounding to whole metres first (see
/// [`haversine_meters`]) would push the boundary out by up to ~0.5 m, which is
/// material for a small geofence radius. Inputs are assumed already
/// range-validated (see [`validate_coordinate_pair`]); pure and infallible.
#[must_use]
pub fn haversine_meters_f64(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let radius_meters = 6_371_000.0_f64;
    let phi1 = lat1.to_radians();
    let phi2 = lat2.to_radians();
    let delta_lat = (lat2 - lat1).to_radians();
    let delta_lon = (lon2 - lon1).to_radians();
    let a =
        (delta_lat / 2.0).sin().powi(2) + phi1.cos() * phi2.cos() * (delta_lon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    radius_meters * c
}

/// Great-circle distance rounded to the nearest **whole metre**. Presentation /
/// display form (e.g. "1 234 m away"); for an inside/outside decision prefer the
/// unrounded [`haversine_meters_f64`].
#[must_use]
pub fn haversine_meters(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> i64 {
    haversine_meters_f64(lat1, lon1, lat2, lon2).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_in_range_coordinates() {
        // Seoul City Hall — a real coordinate within both ranges.
        assert!(validate_latitude(37.5665).is_ok());
        assert!(validate_longitude(126.9780).is_ok());
        assert!(validate_latitude(LATITUDE_MIN).is_ok());
        assert!(validate_latitude(LATITUDE_MAX).is_ok());
        assert!(validate_longitude(LONGITUDE_MIN).is_ok());
        assert!(validate_longitude(LONGITUDE_MAX).is_ok());
    }

    #[test]
    fn rejects_out_of_range_coordinates() {
        assert!(validate_latitude(90.0001).is_err());
        assert!(validate_latitude(-90.0001).is_err());
        assert!(validate_longitude(180.0001).is_err());
        assert!(validate_longitude(-180.0001).is_err());
    }

    #[test]
    fn rejects_non_finite_coordinates() {
        assert!(validate_latitude(f64::NAN).is_err());
        assert!(validate_latitude(f64::INFINITY).is_err());
        assert!(validate_longitude(f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn coordinate_pair_must_be_complete() {
        assert!(validate_coordinate_pair(Some(37.5), Some(127.0)).is_ok());
        assert!(validate_coordinate_pair(None, None).is_ok());
        assert!(validate_coordinate_pair(Some(37.5), None).is_err());
        assert!(validate_coordinate_pair(None, Some(127.0)).is_err());
        // A complete pair with one out-of-range value still fails.
        assert!(validate_coordinate_pair(Some(999.0), Some(127.0)).is_err());
    }

    #[test]
    fn bounded_text_counts_code_points_not_bytes() {
        assert!(validate_bounded_text("홍길동", CONTACT_NAME_MAX_CHARS, "contact_name").is_ok());
        assert!(validate_bounded_text("", CONTACT_PHONE_MAX_CHARS, "contact_phone").is_ok());
        // Exactly the bound (100 Hangul chars = 300 bytes) passes a 100-char limit.
        let exactly: String = "가".repeat(CONTACT_NAME_MAX_CHARS);
        assert!(validate_bounded_text(&exactly, CONTACT_NAME_MAX_CHARS, "contact_name").is_ok());
        let too_long: String = "가".repeat(CONTACT_NAME_MAX_CHARS + 1);
        assert!(validate_bounded_text(&too_long, CONTACT_NAME_MAX_CHARS, "contact_name").is_err());
        assert!(validate_bounded_text("A\0B", CONTACT_NAME_MAX_CHARS, "contact_name").is_err());
    }

    #[test]
    fn customer_site_name_bound_matches_db_check() {
        // The migration-0047 CHECK caps the name at 200 code points; the shared
        // bound must agree so an over-long name is a 422 at the edge, not a 500.
        let exactly: String = "현".repeat(CUSTOMER_SITE_NAME_MAX_CHARS);
        assert!(validate_bounded_text(&exactly, CUSTOMER_SITE_NAME_MAX_CHARS, "name").is_ok());
        let too_long: String = "현".repeat(CUSTOMER_SITE_NAME_MAX_CHARS + 1);
        assert!(validate_bounded_text(&too_long, CUSTOMER_SITE_NAME_MAX_CHARS, "name").is_err());
    }

    #[test]
    fn haversine_meters_is_correct() {
        // Identical points are zero metres apart.
        assert_eq!(haversine_meters(37.5665, 126.9780, 37.5665, 126.9780), 0);
        // One degree of latitude is ~111.19 km on the mean-radius sphere.
        let one_degree_lat = haversine_meters(0.0, 0.0, 1.0, 0.0);
        assert!(
            (111_100..=111_300).contains(&one_degree_lat),
            "1° latitude ≈ {one_degree_lat} m"
        );
        // Distance is symmetric.
        assert_eq!(
            haversine_meters(37.5665, 126.9780, 35.1796, 129.0756),
            haversine_meters(35.1796, 129.0756, 37.5665, 126.9780),
        );
    }
}
