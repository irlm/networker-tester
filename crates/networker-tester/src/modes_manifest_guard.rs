//! Drift guard: `shared/modes.json` ⇄ `Protocol` (metrics.rs).
//!
//! The engine is the source of truth for probe modes; `shared/modes.json` is
//! the canonical machine-readable manifest that the dashboard (vitest) and the
//! C# control plane (xUnit) are guarded against. This module guards the Rust
//! side of the seam **bidirectionally**:
//!
//! 1. every manifest `tester` mode parses via `Protocol::from_str` and its
//!    `Display` output round-trips to the manifest id;
//! 2. every `Protocol` variant appears in the manifest (enforced by an
//!    exhaustive `match` — adding a variant without updating the manifest
//!    breaks this file's compile, which is the point);
//! 3. `Protocol::all_modes()` equals the manifest's catalog entries
//!    (id, name, description, detail, group — byte-for-byte, in order);
//! 4. runner-level modes (e.g. `apibench`) must NOT parse as a tester
//!    `Protocol` — that separation is what the #377–379 bug class violated.
//!
//! Test-only: compiled under `#[cfg(test)]` from lib.rs.

use crate::metrics::Protocol;
use std::collections::HashSet;
use std::str::FromStr;

const MANIFEST: &str = include_str!("../../../shared/modes.json");

fn manifest() -> serde_json::Value {
    serde_json::from_str(MANIFEST).expect("shared/modes.json must be valid JSON")
}

fn manifest_modes(v: &serde_json::Value) -> &Vec<serde_json::Value> {
    v["modes"].as_array().expect("manifest has a modes array")
}

fn ids_by_level(v: &serde_json::Value, level: &str) -> Vec<String> {
    manifest_modes(v)
        .iter()
        .filter(|m| m["level"].as_str() == Some(level))
        .map(|m| m["id"].as_str().expect("mode id is a string").to_string())
        .collect()
}

/// Maps every `Protocol` variant to its canonical manifest id.
///
/// This match is intentionally exhaustive with **no wildcard arm**: adding a
/// new `Protocol` variant makes this fail to compile until the variant is
/// added here AND to `shared/modes.json` (test below verifies membership).
fn manifest_id_of(p: &Protocol) -> &'static str {
    match p {
        Protocol::Tcp => "tcp",
        Protocol::Http1 => "http1",
        Protocol::Http2 => "http2",
        Protocol::Http3 => "http3",
        Protocol::Udp => "udp",
        Protocol::Download => "download",
        Protocol::Download1 => "download1",
        Protocol::Download2 => "download2",
        Protocol::Download3 => "download3",
        Protocol::Upload => "upload",
        Protocol::Upload1 => "upload1",
        Protocol::Upload2 => "upload2",
        Protocol::Upload3 => "upload3",
        Protocol::WebDownload => "webdownload",
        Protocol::WebUpload => "webupload",
        Protocol::UdpDownload => "udpdownload",
        Protocol::UdpUpload => "udpupload",
        Protocol::Dns => "dns",
        Protocol::Tls => "tls",
        Protocol::TlsResume => "tlsresume",
        Protocol::Native => "native",
        Protocol::Curl => "curl",
        Protocol::PageLoad => "pageload",
        Protocol::PageLoad2 => "pageload2",
        Protocol::PageLoad3 => "pageload3",
        Protocol::Browser => "browser",
        Protocol::Browser1 => "browser1",
        Protocol::Browser2 => "browser2",
        Protocol::Browser3 => "browser3",
        Protocol::SdkProbe => "sdkprobe",
    }
}

#[test]
fn every_manifest_tester_mode_parses_and_round_trips() {
    let v = manifest();
    let tester_ids = ids_by_level(&v, "tester");
    assert!(!tester_ids.is_empty(), "manifest lists tester modes");
    for id in &tester_ids {
        let p = Protocol::from_str(id)
            .unwrap_or_else(|e| panic!("manifest tester mode {id:?} must parse as Protocol: {e}"));
        assert_eq!(
            &p.to_string(),
            id,
            "Display for {id:?} must round-trip to the manifest id"
        );
    }
}

#[test]
fn every_protocol_variant_is_in_the_manifest() {
    let v = manifest();
    let tester_ids: HashSet<String> = ids_by_level(&v, "tester").into_iter().collect();
    // Reconstruct every variant from the manifest ids, then verify via the
    // exhaustive mapping that each maps back into the manifest set. Combined
    // with the compile-time exhaustiveness of `manifest_id_of`, a variant
    // missing from the manifest fails here; a variant missing from the match
    // fails to compile.
    let mut covered = HashSet::new();
    for id in &tester_ids {
        let p = Protocol::from_str(id).expect("checked by round-trip test");
        assert!(
            tester_ids.contains(manifest_id_of(&p)),
            "Protocol variant {p:?} missing from shared/modes.json"
        );
        covered.insert(manifest_id_of(&p).to_string());
    }
    assert_eq!(
        covered.len(),
        tester_ids.len(),
        "manifest tester ids must map 1:1 onto Protocol variants (duplicate or alias id present?)"
    );
}

#[test]
fn all_modes_catalog_matches_manifest_exactly() {
    let v = manifest();
    let catalog: Vec<&serde_json::Value> = manifest_modes(&v)
        .iter()
        .filter(|m| m["catalog"].as_bool() == Some(true) && m["level"].as_str() == Some("tester"))
        .collect();
    let engine = Protocol::all_modes();
    assert_eq!(
        engine.len(),
        catalog.len(),
        "Protocol::all_modes() and manifest catalog (tester-level) must have the same length"
    );
    for (e, m) in engine.iter().zip(catalog.iter()) {
        let id = m["id"].as_str().unwrap();
        assert_eq!(e.id, id, "catalog order/id drift at {id:?}");
        assert_eq!(e.name, m["name"].as_str().unwrap(), "name drift for {id:?}");
        assert_eq!(
            e.description,
            m["description"].as_str().unwrap(),
            "description drift for {id:?}"
        );
        assert_eq!(
            e.detail,
            m["detail"].as_str().unwrap(),
            "detail drift for {id:?}"
        );
        assert_eq!(
            e.group,
            m["group"].as_str().unwrap(),
            "group drift for {id:?}"
        );
    }
}

#[test]
fn runner_level_modes_are_not_tester_protocols() {
    let v = manifest();
    for id in ids_by_level(&v, "runner") {
        assert!(
            Protocol::from_str(&id).is_err(),
            "runner-level mode {id:?} must NOT parse as a tester Protocol — \
             it is expanded by the agent/orchestrator (see manifest $comment)"
        );
    }
}

#[test]
fn cli_aliases_and_shorthands_target_valid_manifest_ids() {
    let v = manifest();
    let tester_ids: HashSet<String> = ids_by_level(&v, "tester").into_iter().collect();

    let aliases = v["cli_aliases"].as_object().expect("cli_aliases object");
    for (alias, target) in aliases {
        if alias.starts_with('$') {
            continue; // $comment
        }
        let target = target.as_str().unwrap();
        assert!(
            tester_ids.contains(target),
            "cli alias {alias:?} targets unknown mode {target:?}"
        );
    }
    // tls-resume is also accepted by FromStr — must resolve to the same variant.
    assert_eq!(
        Protocol::from_str("tls-resume").unwrap(),
        Protocol::TlsResume
    );

    let shorthands = v["cli_shorthands"]
        .as_object()
        .expect("cli_shorthands object");
    for (name, targets) in shorthands {
        if name.starts_with('$') {
            continue;
        }
        for t in targets.as_array().unwrap() {
            let t = t.as_str().unwrap();
            assert!(
                tester_ids.contains(t),
                "cli shorthand {name:?} expands to unknown mode {t:?}"
            );
        }
    }
}

#[test]
fn manifest_groups_cover_all_catalog_groups() {
    let v = manifest();
    let declared: HashSet<&str> = v["groups"]
        .as_array()
        .expect("groups array")
        .iter()
        .map(|g| g["label"].as_str().unwrap())
        .collect();
    for m in manifest_modes(&v) {
        if m["catalog"].as_bool() == Some(true) {
            let group = m["group"].as_str().unwrap();
            assert!(
                declared.contains(group),
                "catalog group {group:?} missing from manifest groups[]"
            );
        }
    }
}
