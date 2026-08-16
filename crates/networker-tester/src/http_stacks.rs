//! Canonical port layout + capabilities of the comparison HTTP stacks
//! (`shared/http-stacks.json`, embedded at compile time).
//!
//! One table for every place that used to hard-code "8444 → 8081": the
//! `pageload` / `throughput` / `browser1` HTTPS→HTTP rewrites and the
//! `--http-stacks` CLI table. Before this module each copy knew only
//! nginx + IIS, so `pageload` (forced HTTP/1.1) against a caddy / traefik /
//! haproxy / apache endpoint rewrote to the *TLS* port over plain HTTP and
//! fetched 0/N assets (lab finding, 2026-08), and `--http-stacks caddy`
//! carried ports the installer never used (8083/8446).
//!
//! The same JSON is guarded on the C# side (`ProxyHttpsPortTests`) and drives
//! `lab/validate.sh`'s per-stack HTTP/3 expectations.

use std::sync::OnceLock;

const MANIFEST: &str = include_str!("../../../shared/http-stacks.json");

/// One row of `shared/http-stacks.json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct StackPorts {
    /// Stack id as the installer / control plane name it (`nginx`, `caddy`, …;
    /// `endpoint` = the bare networker-endpoint).
    pub id: String,
    /// Plain-HTTP listener.
    pub http_port: u16,
    /// TLS listener (h1 + h2, and h3/QUIC on the same UDP port when `h3`).
    pub https_port: u16,
    /// Whether the installer configures HTTP/3 on this stack.
    pub h3: bool,
}

#[derive(serde::Deserialize)]
struct Manifest {
    stacks: Vec<StackPorts>,
}

/// All known stacks, in manifest order.
pub fn all() -> &'static [StackPorts] {
    static TABLE: OnceLock<Vec<StackPorts>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            serde_json::from_str::<Manifest>(MANIFEST)
                .expect("shared/http-stacks.json is embedded and must be valid")
                .stacks
        })
        .as_slice()
}

/// Look a stack up by (case-insensitive) id.
pub fn by_name(name: &str) -> Option<&'static StackPorts> {
    let n = name.trim().to_ascii_lowercase();
    all().iter().find(|s| s.id == n)
}

/// The plain-HTTP port paired with an HTTPS port (8443→8080, 8444→8081,
/// 8454→8091, …); `None` when the port is not a known stack listener.
pub fn http_port_for_https(https_port: u16) -> Option<u16> {
    all()
        .iter()
        .find(|s| s.https_port == https_port)
        .map(|s| s.http_port)
}

/// Rewrite an `https://host:PORT/...` URL to the paired plain-HTTP listener
/// (scheme `http`, port from [`http_port_for_https`]). Non-HTTPS URLs are
/// returned unchanged; `443` / no port → port 80 (omitted); an HTTPS port with
/// no known pairing keeps its port (and the caller may log that).
pub fn rewrite_to_http(base: &url::Url) -> url::Url {
    if base.scheme() != "https" {
        return base.clone();
    }
    let mut u = base.clone();
    let _ = u.set_scheme("http");
    let http_port: Option<u16> = match base.port_or_known_default() {
        Some(443) | None => None,
        Some(p) => Some(http_port_for_https(p).unwrap_or(p)),
    };
    let _ = u.set_port(http_port);
    u
}

/// Comma-separated stack ids for help/error text (excluding the bare endpoint).
pub fn proxy_stack_names() -> String {
    all()
        .iter()
        .filter(|s| s.id != "endpoint")
        .map(|s| s.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_has_every_installer_stack_with_distinct_ports() {
        let ids: Vec<&str> = all().iter().map(|s| s.id.as_str()).collect();
        for want in [
            "endpoint", "nginx", "iis", "caddy", "traefik", "haproxy", "apache",
        ] {
            assert!(ids.contains(&want), "missing stack {want}");
        }
        let mut https: Vec<u16> = all().iter().map(|s| s.https_port).collect();
        let mut http: Vec<u16> = all().iter().map(|s| s.http_port).collect();
        https.sort_unstable();
        http.sort_unstable();
        https.dedup();
        http.dedup();
        assert_eq!(https.len(), all().len(), "https ports must be unique");
        assert_eq!(http.len(), all().len(), "http ports must be unique");
    }

    #[test]
    fn https_to_http_pairs_match_the_installer_layout() {
        assert_eq!(http_port_for_https(8443), Some(8080)); // endpoint
        assert_eq!(http_port_for_https(8444), Some(8081)); // nginx
        assert_eq!(http_port_for_https(8445), Some(8082)); // iis
        assert_eq!(http_port_for_https(8454), Some(8091)); // caddy
        assert_eq!(http_port_for_https(8455), Some(8092)); // traefik
        assert_eq!(http_port_for_https(8456), Some(8093)); // haproxy
        assert_eq!(http_port_for_https(8457), Some(8094)); // apache
        assert_eq!(http_port_for_https(9999), None);
    }

    #[test]
    fn rewrite_to_http_covers_every_stack_and_leaves_http_alone() {
        for s in all() {
            let u =
                url::Url::parse(&format!("https://10.0.0.5:{}/asset?id=1", s.https_port)).unwrap();
            let r = rewrite_to_http(&u);
            assert_eq!(r.scheme(), "http");
            assert_eq!(r.port(), Some(s.http_port), "stack {}", s.id);
            assert_eq!(r.path(), "/asset");
            assert_eq!(r.query(), Some("id=1"));
        }
        let plain = url::Url::parse("http://h:8080/x").unwrap();
        assert_eq!(rewrite_to_http(&plain), plain);
        let default = url::Url::parse("https://example.com/x").unwrap();
        assert_eq!(rewrite_to_http(&default).port_or_known_default(), Some(80));
        let unknown = url::Url::parse("https://h:9443/x").unwrap();
        assert_eq!(rewrite_to_http(&unknown).port(), Some(9443));
    }

    #[test]
    fn h3_capability_matches_installer_configs() {
        assert!(by_name("nginx").unwrap().h3);
        assert!(by_name("caddy").unwrap().h3);
        assert!(!by_name("apache").unwrap().h3);
        assert!(!by_name("haproxy").unwrap().h3);
        assert!(!by_name("traefik").unwrap().h3);
        assert!(by_name("NGINX").is_some(), "lookup is case-insensitive");
        assert!(by_name("lighttpd").is_none());
    }
}
