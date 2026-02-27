/// Throughput probes: download (GET /download?bytes=N) and upload (POST /upload with N-byte body).
///
/// These are thin wrappers around `run_probe` that:
///  1. Rewrite the URL to point at the appropriate endpoint route.
///  2. Set `payload_size` appropriately (0 for download, N for upload).
///  3. After the probe returns, patch the `HttpResult` with `payload_bytes`
///     and `throughput_mbps` derived from body transfer time.
///
/// Throughput formula (MB/s):
///   body_ms  = total_duration_ms − ttfb_ms   (time spent transferring the body)
///   mbps     = payload_bytes / body_ms * 1000 / (1024 * 1024)
use crate::metrics::{HttpResult, Protocol, RequestAttempt};
use crate::runner::http::{run_probe, RunConfig};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ThroughputConfig {
    pub run_cfg: RunConfig,
    pub base_url: url::Url,
}

// ─────────────────────────────────────────────────────────────────────────────
// Download probe
// ─────────────────────────────────────────────────────────────────────────────

/// GET /download?bytes={payload_bytes} and measure how fast the body arrives.
pub async fn run_download_probe(
    run_id: Uuid,
    sequence_num: u32,
    payload_bytes: usize,
    cfg: &ThroughputConfig,
) -> RequestAttempt {
    let mut target = cfg.base_url.clone();
    target.set_path("/download");
    target.set_query(Some(&format!("bytes={payload_bytes}")));

    let probe_cfg = RunConfig {
        payload_size: 0, // GET request
        ..cfg.run_cfg.clone()
    };

    let mut attempt = run_probe(
        run_id,
        sequence_num,
        Protocol::Download,
        &target,
        &probe_cfg,
    )
    .await;

    if let Some(h) = attempt.http.clone() {
        attempt.http = Some(patch_throughput(h, payload_bytes));
    }
    attempt
}

// ─────────────────────────────────────────────────────────────────────────────
// Upload probe
// ─────────────────────────────────────────────────────────────────────────────

/// POST /upload with a {payload_bytes}-byte zero-filled body and measure upload speed.
pub async fn run_upload_probe(
    run_id: Uuid,
    sequence_num: u32,
    payload_bytes: usize,
    cfg: &ThroughputConfig,
) -> RequestAttempt {
    let mut target = cfg.base_url.clone();
    target.set_path("/upload");
    target.set_query(None);

    let probe_cfg = RunConfig {
        payload_size: payload_bytes, // POST body
        ..cfg.run_cfg.clone()
    };

    let mut attempt = run_probe(run_id, sequence_num, Protocol::Upload, &target, &probe_cfg).await;

    if let Some(h) = attempt.http.clone() {
        attempt.http = Some(patch_throughput(h, payload_bytes));
    }
    attempt
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn patch_throughput(h: HttpResult, payload_bytes: usize) -> HttpResult {
    let body_ms = h.total_duration_ms - h.ttfb_ms;
    let throughput_mbps = if body_ms > 0.0 {
        Some(payload_bytes as f64 / body_ms * 1000.0 / (1024.0 * 1024.0))
    } else {
        None
    };
    HttpResult {
        payload_bytes,
        throughput_mbps,
        ..h
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::HttpResult;
    use chrono::Utc;

    fn make_http_result(ttfb_ms: f64, total_ms: f64) -> HttpResult {
        HttpResult {
            negotiated_version: "HTTP/1.1".into(),
            status_code: 200,
            headers_size_bytes: 0,
            body_size_bytes: 0,
            ttfb_ms,
            total_duration_ms: total_ms,
            redirect_count: 0,
            started_at: Utc::now(),
            response_headers: vec![],
            payload_bytes: 0,
            throughput_mbps: None,
        }
    }

    #[test]
    fn throughput_formula_is_correct() {
        // 1 MiB transferred in 1000ms body time → 1.0 MB/s
        let h = make_http_result(10.0, 1010.0); // ttfb=10ms, total=1010ms → body=1000ms
        let patched = patch_throughput(h, 1024 * 1024);
        let mbps = patched.throughput_mbps.expect("should have throughput");
        assert!((mbps - 1.0).abs() < 1e-9, "expected ~1.0 MB/s, got {mbps}");
    }

    #[test]
    fn throughput_none_when_body_ms_is_zero() {
        // ttfb == total → body_ms == 0 → no throughput
        let h = make_http_result(100.0, 100.0);
        let patched = patch_throughput(h, 65536);
        assert!(patched.throughput_mbps.is_none());
    }

    #[test]
    fn throughput_payload_bytes_set() {
        let h = make_http_result(5.0, 100.0);
        let patched = patch_throughput(h, 65536);
        assert_eq!(patched.payload_bytes, 65536);
    }

    #[tokio::test]
    #[ignore = "requires local networker-endpoint on :8080"]
    async fn download_probe_returns_throughput() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let base = url::Url::parse("http://127.0.0.1:8080/health").unwrap();
        let cfg = ThroughputConfig {
            run_cfg: RunConfig {
                dns_enabled: false,
                timeout_ms: 10_000,
                ..Default::default()
            },
            base_url: base,
        };
        let attempt = run_download_probe(Uuid::new_v4(), 0, 65536, &cfg).await;
        assert!(attempt.success, "probe failed: {:?}", attempt.error);
        let h = attempt.http.expect("http result missing");
        assert_eq!(h.payload_bytes, 65536);
        assert!(h.throughput_mbps.is_some(), "throughput should be measured");
    }

    #[tokio::test]
    #[ignore = "requires local networker-endpoint on :8080"]
    async fn upload_probe_returns_throughput() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let base = url::Url::parse("http://127.0.0.1:8080/health").unwrap();
        let cfg = ThroughputConfig {
            run_cfg: RunConfig {
                dns_enabled: false,
                timeout_ms: 10_000,
                ..Default::default()
            },
            base_url: base,
        };
        let attempt = run_upload_probe(Uuid::new_v4(), 0, 65536, &cfg).await;
        assert!(attempt.success, "probe failed: {:?}", attempt.error);
        let h = attempt.http.expect("http result missing");
        assert_eq!(h.payload_bytes, 65536);
        assert!(h.throughput_mbps.is_some(), "throughput should be measured");
    }
}
