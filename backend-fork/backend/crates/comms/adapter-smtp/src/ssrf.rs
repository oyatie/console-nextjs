//! SSRF guard for outbound webmail connections.
//!
//! Before ANY outbound connect (test-connection and send) the admin-configured
//! host is resolved ONCE via [`hickory_resolver`]; every resolved IP is checked
//! against a denylist (RFC1918, loopback, link-local incl. the `169.254.169.254`
//! cloud-metadata endpoint, unique-local IPv6 `fc00::/7`, CGNAT `100.64/10`, and
//! more). The connection then dials the PINNED resolved IP rather than
//! re-resolving the hostname — closing the DNS-rebinding (TOCTOU) window where a
//! second lookup could return a private address after the check passed.
//!
//! IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) AND the deprecated
//! IPv4-compatible form (`::a.b.c.d`, e.g. `::7f00:1` = `::127.0.0.1`) are
//! un-mapped to their IPv4 form BEFORE the denylist check, so an attacker cannot
//! smuggle a private v4 address through either v6 wrapper.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ipnet::{Ipv4Net, Ipv6Net};

/// Why an outbound host was refused. Carries a stable, non-secret `code` so the
/// REST layer can surface a fixed message without leaking the host or the
/// resolved address.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SsrfError {
    /// The host is empty or syntactically unusable.
    #[error("invalid host")]
    InvalidHost,
    /// DNS resolution returned no address (or the lookup failed).
    #[error("host did not resolve")]
    Unresolvable,
    /// A resolved IP is in a denied range (private / loopback / link-local / …).
    #[error("host resolves to a disallowed address")]
    DisallowedAddress,
}

impl SsrfError {
    /// A stable token for the REST/transport error surface.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidHost => "invalid_host",
            Self::Unresolvable => "host_unresolvable",
            Self::DisallowedAddress => "host_not_allowed",
        }
    }
}

/// IPv4 ranges that an outbound mail host may NEVER resolve into.
fn denied_v4() -> &'static [Ipv4Net] {
    use std::sync::OnceLock;
    static NETS: OnceLock<Vec<Ipv4Net>> = OnceLock::new();
    NETS.get_or_init(|| {
        [
            "0.0.0.0/8",       // "this host" / unspecified
            "10.0.0.0/8",      // RFC1918 private
            "100.64.0.0/10",   // RFC6598 CGNAT
            "127.0.0.0/8",     // loopback
            "169.254.0.0/16",  // link-local (incl. 169.254.169.254 metadata)
            "172.16.0.0/12",   // RFC1918 private
            "192.0.0.0/24",    // IETF protocol assignments
            "192.0.2.0/24",    // TEST-NET-1
            "192.168.0.0/16",  // RFC1918 private
            "198.18.0.0/15",   // benchmarking
            "198.51.100.0/24", // TEST-NET-2
            "203.0.113.0/24",  // TEST-NET-3
            "224.0.0.0/4",     // multicast
            "240.0.0.0/4",     // reserved (incl. 255.255.255.255 broadcast)
        ]
        .iter()
        // The literals above are all valid CIDRs; `filter_map(.ok())` keeps this
        // panic-free (a typo would silently drop a range, which a unit test
        // below guards against by asserting representative denials).
        .filter_map(|s| s.parse().ok())
        .collect()
    })
}

/// IPv6 ranges that an outbound mail host may NEVER resolve into.
fn denied_v6() -> &'static [Ipv6Net] {
    use std::sync::OnceLock;
    static NETS: OnceLock<Vec<Ipv6Net>> = OnceLock::new();
    NETS.get_or_init(|| {
        [
            "::1/128",       // loopback
            "::/128",        // unspecified
            "fc00::/7",      // unique-local (ULA)
            "fe80::/10",     // link-local
            "ff00::/8",      // multicast
            "2001:db8::/32", // documentation
            "64:ff9b::/96",  // NAT64 (could embed a private v4)
        ]
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect()
    })
}

/// Normalize an address before the denylist check: un-map BOTH the IPv4-mapped
/// (`::ffff:a.b.c.d`) and the deprecated IPv4-compatible (`::a.b.c.d`) IPv6 forms
/// to their IPv4 form so a private v4 can't hide in either v6 wrapper.
///
/// `Ipv6Addr::to_ipv4()` covers both forms (it un-maps anything in `::/96` or
/// `::ffff:0:0/96`); it also turns `::1`/`::` into `0.0.0.1`/`0.0.0.0`, but those
/// fall inside the v4 `0.0.0.0/8` denylist, so loopback/unspecified stay denied.
#[must_use]
fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        other => other,
    }
}

/// Returns `true` when `ip` (after normalization) falls in a denied range.
#[must_use]
pub fn is_denied(ip: IpAddr) -> bool {
    match normalize(ip) {
        IpAddr::V4(v4) => is_denied_v4(v4),
        IpAddr::V6(v6) => is_denied_v6(v6),
    }
}

fn is_denied_v4(v4: Ipv4Addr) -> bool {
    if v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_broadcast() {
        return true;
    }
    denied_v4().iter().any(|net| net.contains(&v4))
}

fn is_denied_v6(v6: Ipv6Addr) -> bool {
    if v6.is_loopback() || v6.is_unspecified() {
        return true;
    }
    denied_v6().iter().any(|net| net.contains(&v6))
}

/// Validate a slice of resolved addresses and return the FIRST allowed one to
/// pin for the connect. If ANY resolved address is denied, the whole host is
/// rejected (a host that resolves to a mix of public and private addresses is
/// treated as hostile — there is no safe subset to dial).
pub fn pick_pinned_ip(addresses: &[IpAddr]) -> Result<IpAddr, SsrfError> {
    if addresses.is_empty() {
        return Err(SsrfError::Unresolvable);
    }
    if addresses.iter().copied().any(is_denied) {
        return Err(SsrfError::DisallowedAddress);
    }
    Ok(addresses[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn denies_cloud_metadata_endpoint() {
        assert!(is_denied(ip("169.254.169.254")));
    }

    #[test]
    fn denies_loopback_and_rfc1918() {
        assert!(is_denied(ip("127.0.0.1")));
        assert!(is_denied(ip("10.1.2.3")));
        assert!(is_denied(ip("172.16.5.5")));
        assert!(is_denied(ip("192.168.0.1")));
    }

    #[test]
    fn denies_cgnat_and_reserved() {
        assert!(is_denied(ip("100.64.0.1")));
        assert!(is_denied(ip("0.0.0.0")));
        assert!(is_denied(ip("255.255.255.255")));
    }

    #[test]
    fn denies_ipv6_loopback_ula_linklocal() {
        assert!(is_denied(ip("::1")));
        assert!(is_denied(ip("fc00::1")));
        assert!(is_denied(ip("fd12:3456::1")));
        assert!(is_denied(ip("fe80::1")));
    }

    #[test]
    fn denies_ipv4_mapped_ipv6_metadata() {
        // ::ffff:169.254.169.254 must be un-mapped and then denied.
        assert!(is_denied(ip("::ffff:169.254.169.254")));
        assert!(is_denied(ip("::ffff:10.0.0.1")));
        assert!(is_denied(ip("::ffff:127.0.0.1")));
    }

    #[test]
    fn denies_ipv4_compatible_ipv6() {
        // The DEPRECATED IPv4-compatible form ::a.b.c.d (e.g. ::127.0.0.1, which
        // can also be written ::7f00:1) must be un-mapped via to_ipv4() and then
        // subjected to the v4 denylist — both wrappers of 127.0.0.1 are denied.
        assert!(is_denied(ip("::ffff:127.0.0.1")));
        assert!(is_denied(ip("::127.0.0.1")));
        assert!(is_denied(ip("::7f00:1"))); // == ::127.0.0.1
        assert!(is_denied(ip("::ffff:169.254.169.254")));
        assert!(is_denied(ip("::169.254.169.254"))); // compat-form metadata
        assert!(is_denied(ip("::10.0.0.1"))); // compat-form RFC1918
    }

    #[test]
    fn allows_public_addresses() {
        assert!(!is_denied(ip("8.8.8.8")));
        assert!(!is_denied(ip("1.1.1.1")));
        assert!(!is_denied(ip("93.184.216.34"))); // example.com
        assert!(!is_denied(ip("2606:4700:4700::1111"))); // cloudflare v6
    }

    #[test]
    fn pick_pinned_rejects_when_any_address_is_private() {
        let mixed = [ip("8.8.8.8"), ip("10.0.0.1")];
        assert_eq!(pick_pinned_ip(&mixed), Err(SsrfError::DisallowedAddress));
    }

    #[test]
    fn pick_pinned_returns_first_public_address() {
        let all_public = [ip("8.8.8.8"), ip("1.1.1.1")];
        assert_eq!(pick_pinned_ip(&all_public).unwrap(), ip("8.8.8.8"));
    }

    #[test]
    fn pick_pinned_rejects_empty() {
        assert_eq!(pick_pinned_ip(&[]), Err(SsrfError::Unresolvable));
    }
}
