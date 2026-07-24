//! Offline GeoIP / ASN enrichment from user-supplied MaxMind databases.
//!
//! Policy (owner decision, measurement-gap item #12): **offline MaxMind only**.
//! The user supplies GeoLite2/GeoIP2 `.mmdb` files (free account at
//! maxmind.com); we never download, bundle, or call a runtime geolocation API.
//! An absent or unreadable database silently disables enrichment — it is never
//! an error, and never affects measurement.
//!
//! Databases are opened once per process (the reader holds the whole file in
//! memory; GeoLite2-City is ~60 MB, ASN ~10 MB) and shared across targets.

use std::net::IpAddr;

use maxminddb::{geoip2, Reader};
use tracing::{debug, info};

use crate::baseline::classify_ip;
use crate::metrics::{GeoInfo, NetworkType};

/// Holds the (optional) City and ASN database readers for one process.
pub struct GeoIpResolver {
    city: Option<Reader<Vec<u8>>>,
    asn: Option<Reader<Vec<u8>>>,
}

impl GeoIpResolver {
    /// Open the configured databases. Missing/unreadable paths log one debug
    /// line each and disable that half of the enrichment — never an error.
    pub fn open(city_db: Option<&str>, asn_db: Option<&str>) -> Self {
        Self {
            city: open_reader(city_db, "city"),
            asn: open_reader(asn_db, "asn"),
        }
    }

    /// A resolver with no databases (enrichment disabled).
    pub fn disabled() -> Self {
        Self {
            city: None,
            asn: None,
        }
    }

    /// True when at least one database is loaded.
    pub fn is_enabled(&self) -> bool {
        self.city.is_some() || self.asn.is_some()
    }

    /// Look up one IP across both databases. Returns `None` when no database
    /// is loaded or neither database has a record for the address.
    pub fn lookup(&self, ip: IpAddr) -> Option<GeoInfo> {
        let mut geo = GeoInfo::default();
        let mut db_date = None;

        if let Some(ref reader) = self.city {
            if let Ok(result) = reader.lookup(ip) {
                if let Ok(Some(city)) = result.decode::<geoip2::City>() {
                    geo.country = city.country.iso_code.map(str::to_string);
                    geo.city = city.city.names.english.map(str::to_string);
                }
            }
            db_date = build_epoch_date(reader.metadata().build_epoch);
        }

        if let Some(ref reader) = self.asn {
            if let Ok(result) = reader.lookup(ip) {
                if let Ok(Some(asn)) = result.decode::<geoip2::Asn>() {
                    geo.asn = asn.autonomous_system_number;
                    geo.as_org = asn.autonomous_system_organization.map(str::to_string);
                }
            }
            if db_date.is_none() {
                db_date = build_epoch_date(reader.metadata().build_epoch);
            }
        }

        if geo == GeoInfo::default() {
            return None;
        }
        geo.db_date = db_date;
        Some(geo)
    }

    /// Enrich the client side: only when the local egress interface toward
    /// `target_ip` carries a public address (VMs with a public NIC, servers
    /// with routable addresses). Behind NAT the egress IP is RFC1918/CGNAT and
    /// enrichment is skipped — we deliberately do NOT call external
    /// what's-my-ip services to discover the NAT'd public address.
    pub fn lookup_client_egress(&self, target_ip: IpAddr) -> Option<GeoInfo> {
        if !self.is_enabled() {
            return None;
        }
        let egress = egress_local_ip(target_ip)?;
        if !matches!(classify_ip(&egress), NetworkType::Internet) {
            debug!(egress = %egress, "client egress IP is not public; skipping client geo enrichment");
            return None;
        }
        self.lookup(egress)
    }
}

fn open_reader(path: Option<&str>, kind: &str) -> Option<Reader<Vec<u8>>> {
    let path = path?;
    match Reader::open_readfile(path) {
        Ok(reader) => {
            info!(
                path,
                kind,
                db_type = %reader.metadata().database_type,
                build_date = build_epoch_date(reader.metadata().build_epoch).as_deref().unwrap_or("?"),
                "GeoIP database loaded"
            );
            Some(reader)
        }
        Err(e) => {
            debug!(path, kind, error = %e, "GeoIP database unavailable; geo enrichment disabled for this database");
            None
        }
    }
}

/// Format an mmdb build epoch (Unix seconds) as `YYYY-MM-DD`.
fn build_epoch_date(epoch: u64) -> Option<String> {
    let ts = i64::try_from(epoch).ok()?;
    chrono::DateTime::from_timestamp(ts, 0).map(|dt| dt.format("%Y-%m-%d").to_string())
}

/// Local IP the OS routes toward `target_ip` (UDP connect trick — resolves the
/// egress interface from the routing table; no packet is ever sent).
pub fn egress_local_ip(target_ip: IpAddr) -> Option<IpAddr> {
    let bind_addr = match target_ip {
        IpAddr::V4(_) => "0.0.0.0:0",
        IpAddr::V6(_) => "[::]:0",
    };
    let socket = std::net::UdpSocket::bind(bind_addr).ok()?;
    socket.connect((target_ip, 80)).ok()?;
    socket.local_addr().ok().map(|addr| addr.ip())
}

/// Resolve a hostname (or parse an IP literal) to its first address, honoring
/// the tester's address-family restrictions. Synchronous, same approach as
/// `baseline::classify_target`.
pub fn resolve_first_ip(host: &str, port: u16, ipv4_only: bool, ipv6_only: bool) -> Option<IpAddr> {
    let family_ok = |ip: &IpAddr| {
        if ipv4_only {
            ip.is_ipv4()
        } else if ipv6_only {
            ip.is_ipv6()
        } else {
            true
        }
    };
    if let Ok(ip) = host.parse::<IpAddr>() {
        return family_ok(&ip).then_some(ip);
    }
    use std::net::ToSocketAddrs;
    (host, port)
        .to_socket_addrs()
        .ok()?
        .map(|addr| addr.ip())
        .find(family_ok)
}

#[cfg(test)]
mod tests {
    //! Real-lookup tests against the official MaxMind test databases.
    //!
    //! Fixture attribution: `tests/fixtures/geoip/GeoIP2-City-Test.mmdb` and
    //! `tests/fixtures/geoip/GeoLite2-ASN-Test.mmdb` are vendored unmodified
    //! from https://github.com/maxmind/MaxMind-DB (test-data/), which is
    //! Copyright (c) 2013-2026 MaxMind, Inc. and dual-licensed under the
    //! Apache License 2.0 or the MIT License, at your option.
    use super::*;

    fn fixture(name: &str) -> String {
        format!("{}/tests/fixtures/geoip/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    fn full_resolver() -> GeoIpResolver {
        GeoIpResolver::open(
            Some(&fixture("GeoIP2-City-Test.mmdb")),
            Some(&fixture("GeoLite2-ASN-Test.mmdb")),
        )
    }

    #[test]
    fn city_lookup_maps_country_city_and_db_date() {
        let resolver = full_resolver();
        assert!(resolver.is_enabled());
        // 89.160.20.128 is the canonical Linköping, SE record in the City test DB.
        let geo = resolver
            .lookup("89.160.20.128".parse().unwrap())
            .expect("test DB has a record for 89.160.20.128");
        assert_eq!(geo.country.as_deref(), Some("SE"));
        assert_eq!(geo.city.as_deref(), Some("Linköping"));
        let db_date = geo.db_date.expect("db_date present");
        assert_eq!(db_date.len(), 10, "YYYY-MM-DD, got {db_date}");
    }

    #[test]
    fn asn_lookup_maps_asn_and_org() {
        let resolver = full_resolver();
        // 1.128.0.0 is the canonical AS1221 Telstra record in the ASN test DB.
        let geo = resolver
            .lookup("1.128.0.0".parse().unwrap())
            .expect("test DB has a record for 1.128.0.0");
        assert_eq!(geo.asn, Some(1221));
        assert_eq!(geo.as_org.as_deref(), Some("Telstra Pty Ltd"));
        // Not present in the City test DB → geo half stays None.
        assert_eq!(geo.country, None);
    }

    #[test]
    fn unknown_ip_returns_none_not_empty_struct() {
        let resolver = full_resolver();
        // TEST-NET-3 — present in neither test database.
        assert_eq!(resolver.lookup("203.0.113.7".parse().unwrap()), None);
    }

    #[test]
    fn missing_databases_disable_enrichment_silently() {
        let resolver = GeoIpResolver::open(Some("/nonexistent/city.mmdb"), None);
        assert!(!resolver.is_enabled());
        assert_eq!(resolver.lookup("89.160.20.128".parse().unwrap()), None);

        let disabled = GeoIpResolver::disabled();
        assert!(!disabled.is_enabled());
        assert_eq!(
            disabled.lookup_client_egress("8.8.8.8".parse().unwrap()),
            None
        );
    }

    #[test]
    fn client_egress_never_enriched_for_private_paths() {
        let resolver = full_resolver();
        // Egress toward loopback is loopback → must be skipped even though the
        // databases are loaded.
        assert_eq!(
            resolver.lookup_client_egress("127.0.0.1".parse().unwrap()),
            None
        );
    }

    #[test]
    fn resolve_first_ip_handles_literals_and_family_filters() {
        assert_eq!(
            resolve_first_ip("192.0.2.1", 443, false, false),
            Some("192.0.2.1".parse().unwrap())
        );
        // IPv4 literal rejected under --ipv6-only.
        assert_eq!(resolve_first_ip("192.0.2.1", 443, false, true), None);
        assert_eq!(
            resolve_first_ip("2001:db8::1", 443, false, false),
            Some("2001:db8::1".parse().unwrap())
        );
        // IPv6 literal rejected under --ipv4-only.
        assert_eq!(resolve_first_ip("2001:db8::1", 443, true, false), None);
        let localhost = resolve_first_ip("localhost", 80, true, false);
        assert_eq!(localhost, Some("127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn label_formats_compactly() {
        let geo = GeoInfo {
            country: Some("SE".into()),
            city: Some("Linköping".into()),
            asn: Some(1221),
            as_org: Some("Telstra Pty Ltd".into()),
            db_date: Some("2026-01-05".into()),
        };
        assert_eq!(geo.label(), "SE · Linköping · AS1221 Telstra Pty Ltd");
        assert_eq!(GeoInfo::default().label(), "—");
        let asn_only = GeoInfo {
            asn: Some(13335),
            as_org: Some("Cloudflare, Inc.".into()),
            ..GeoInfo::default()
        };
        assert_eq!(asn_only.label(), "AS13335 Cloudflare, Inc.");
    }

    #[test]
    fn build_epoch_date_is_iso_day() {
        assert_eq!(build_epoch_date(0).as_deref(), Some("1970-01-01"));
        assert_eq!(
            build_epoch_date(1_767_225_600).as_deref(),
            Some("2026-01-01")
        );
    }
}
