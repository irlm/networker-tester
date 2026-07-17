//! API-SPEC.md §7 parity: the four canonical requests against the shared
//! benchmark dataset must hash (canonical JSON → SHA-256) to the dataset's
//! `expected_checksums`. This pins the canonical (family C) implementation to
//! the frozen contract — every other language is validated against the same
//! four hashes by `benchmarks/validate/run-validation.sh`.
//!
//! Runs as its own integration-test process so it can set `BENCH_DATA_PATH`
//! before the endpoint's dataset cache initializes (the lib unit tests run
//! with the PRNG fallback, as before).

use axum::body::{to_bytes, Body};
use http::Request;
use networker_endpoint::{build_router, AppState, SystemMeta};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

fn dataset_path() -> String {
    format!(
        "{}/../../benchmarks/reference-apis/shared/bench-data.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn app() -> axum::Router {
    // Point the endpoint's dataset cache at the shared file before the router
    // (which eagerly resolves it) is built — regardless of which test in this
    // process runs first.
    std::env::set_var("BENCH_DATA_PATH", dataset_path());
    build_router(AppState {
        h3_port: None,
        http_port: 8080,
        https_port: 8443,
        udp_port: 9999,
        udp_throughput_port: 9998,
        started_at: std::time::Instant::now(),
        system_meta: SystemMeta::collect(),
    })
}

/// Canonical JSON (API-SPEC.md §7): sorted keys, no whitespace, shortest
/// round-trip floats. serde_json without `preserve_order` stores objects in a
/// BTreeMap, so parse + to_string yields exactly that form.
fn canon_hash(bytes: &[u8]) -> String {
    let v: serde_json::Value = serde_json::from_slice(bytes).expect("response must be JSON");
    let canon = serde_json::to_string(&v).unwrap();
    hex::encode(Sha256::digest(canon.as_bytes()))
}

#[tokio::test]
async fn canonical_responses_match_dataset_checksums() {
    let path = dataset_path();
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("shared dataset missing at {path}: {e}"));
    let dataset: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let expected = &dataset["expected_checksums"];

    for (key, uri) in [
        ("users_page1", "/api/users?page=1&sort=name&order=asc"),
        ("aggregate_default", "/api/aggregate"),
        ("search_network_top10", "/api/search?q=network&limit=10"),
    ] {
        let resp = app()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{uri}");
        let body = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
        assert_eq!(
            canon_hash(&body),
            expected[key].as_str().unwrap(),
            "{key} checksum mismatch — the endpoint diverged from API-SPEC.md §7 \
             (or the dataset was regenerated without updating the algorithms)"
        );
    }

    // transform_input0: POST /api/transform with the dataset's first input.
    let t0 = serde_json::to_string(&dataset["transform_inputs"][0]).unwrap();
    let resp = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/transform")
                .header("content-type", "application/json")
                .body(Body::from(t0))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(
        canon_hash(&body),
        expected["transform_input0"].as_str().unwrap(),
        "transform_input0 checksum mismatch"
    );
}

#[tokio::test]
async fn download_path_form_meets_orchestrator_contract() {
    // GET /download/{size} → exactly N bytes of 0x42 (API-SPEC.md §5.2); this
    // is the request shape deployer::validate_api actually sends.
    for size in [1024usize, 65536] {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri(format!("/download/{size}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 2 * 1024 * 1024).await.unwrap();
        assert_eq!(body.len(), size);
        assert!(body.iter().all(|&b| b == 0x42));
    }
}
