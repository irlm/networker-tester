/// Throughput probes: download (GET /download?bytes=N) and upload (POST /upload with N-byte body).
///
/// These are thin wrappers around `run_probe` that:
///  1. Rewrite the URL to point at the appropriate endpoint route.
///  2. Set `payload_size` appropriately (0 for download, N for upload).
///  3. After the probe returns, patch the `HttpResult` with `payload_bytes`
///     and `throughput_mbps` using a direction-appropriate time window.
///
/// ## Why the time basis differs by direction
///
/// **Download** — data flows server → client *after* the server sends its first
/// response byte.  The correct window is the body-receive phase:
///   transfer_ms = total_duration_ms − ttfb_ms
///
/// **Upload** — data flows client → server *before* the server responds.
///
/// The preferred measurement source is the server's own drain timer, returned
/// in the `Server-Timing: recv;dur=X` response header when talking to
/// `networker-endpoint`.  This directly measures how long the server spent
/// reading the request body and is immune to client-side timing ambiguities.
///
/// When that header is absent (generic HTTP target), we fall back to the
/// client-side `total_duration_ms` (time from HTTP start to full response
/// receipt).  This is accurate only when the server drains the body **before**
/// sending its response — which our endpoint guarantees.  For servers that
/// respond before the body is fully received, `total_duration_ms` can be
/// near-zero (the kernel TCP send buffer absorbs the body and hyper returns
/// as soon as response headers arrive), leading to absurdly high throughput.
///
///   preferred: transfer_ms = server_timing.recv_body_ms   (server drain time)
///   fallback:  transfer_ms = total_duration_ms            (end-to-end client time)
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
        // Prefer the server's own drain timer (Server-Timing: recv;dur=X) — it
        // is accurate even when the server responds before the body is fully
        // received on the wire.  Fall back to client-side total_duration_ms.
        let server_recv_ms = attempt
            .server_timing
            .as_ref()
            .and_then(|st| st.recv_body_ms);
        attempt.http = Some(patch_upload_throughput(h, payload_bytes, server_recv_ms));
    }
    attempt
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Compute and attach download throughput to an `HttpResult`.
///
/// Transfer window: `total_duration_ms − ttfb_ms` (body-receive phase only).
fn patch_throughput(h: HttpResult, payload_bytes: usize) -> HttpResult {
    let transfer_ms = h.total_duration_ms - h.ttfb_ms;
    let throughput_mbps = mbps(payload_bytes, transfer_ms);
    HttpResult {
        payload_bytes,
        throughput_mbps,
        ..h
    }
}

/// Compute and attach upload throughput to an `HttpResult`.
///
/// Uses `server_recv_ms` (from `Server-Timing: recv;dur=X`) when provided —
/// the server's own timer for how long it spent draining the request body.
/// Falls back to `h.total_duration_ms` (reliable only when the server drains
/// before responding, which `networker-endpoint` guarantees).
fn patch_upload_throughput(
    h: HttpResult,
    payload_bytes: usize,
    server_recv_ms: Option<f64>,
) -> HttpResult {
    let transfer_ms = server_recv_ms.unwrap_or(h.total_duration_ms);
    let throughput_mbps = mbps(payload_bytes, transfer_ms);
    HttpResult {
        payload_bytes,
        throughput_mbps,
        ..h
    }
}

fn mbps(payload_bytes: usize, transfer_ms: f64) -> Option<f64> {
    if transfer_ms > 0.0 {
        Some(payload_bytes as f64 / transfer_ms * 1000.0 / (1024.0 * 1024.0))
    } else {
        None
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

    // ── Download ─────────────────────────────────────────────────────────────

    #[test]
    fn download_uses_body_receive_time() {
        // 1 MiB received in 1000ms body time (ttfb=10ms, total=1010ms) → 1.0 MB/s
        let h = make_http_result(10.0, 1010.0);
        let patched = patch_throughput(h, 1024 * 1024);
        let mbps = patched.throughput_mbps.expect("should have throughput");
        assert!((mbps - 1.0).abs() < 1e-9, "expected ~1.0 MB/s, got {mbps}");
    }

    #[test]
    fn download_throughput_none_when_transfer_ms_is_zero() {
        // download where ttfb == total → body_ms == 0 → no throughput
        let h = make_http_result(100.0, 100.0);
        let patched = patch_throughput(h, 65536);
        assert!(patched.throughput_mbps.is_none());
    }

    #[test]
    fn download_payload_bytes_set() {
        let h = make_http_result(5.0, 100.0);
        let patched = patch_throughput(h, 65536);
        assert_eq!(patched.payload_bytes, 65536);
    }

    // ── Upload: server recv_body_ms available ─────────────────────────────────

    #[test]
    fn upload_prefers_server_recv_ms_when_available() {
        // Server says it drained the body in 1000ms; client total is also 1000ms.
        // Result should use server's 1000ms.
        let h = make_http_result(0.5, 1000.0);
        let patched = patch_upload_throughput(h, 1024 * 1024, Some(1000.0));
        let mbps = patched.throughput_mbps.expect("should have throughput");
        assert!((mbps - 1.0).abs() < 1e-9, "expected ~1.0 MB/s, got {mbps}");
    }

    #[test]
    fn upload_server_recv_ms_prevents_absurd_throughput_on_fast_respond() {
        // Server responded before draining → client total_duration_ms ≈ 0.2ms
        // but server recv_body_ms = 9000ms (actual drain).
        // Without server timing, throughput would be ~5M MB/s; with it → ~113 MB/s.
        let h = make_http_result(0.2, 0.2); // fast client-side times
        let patched = patch_upload_throughput(h, 1024 * 1024 * 1024, Some(9000.0));
        let mbps = patched.throughput_mbps.expect("should have throughput");
        assert!(
            mbps < 10_000.0,
            "throughput must not be astronomically wrong: {mbps}"
        );
        assert!(mbps > 50.0, "throughput must be in a plausible range: {mbps}");
    }

    // ── Upload: no server timing, falls back to total_duration_ms ─────────────

    #[test]
    fn upload_falls_back_to_total_duration_without_server_timing() {
        // No server recv_body_ms; endpoint drains before respond → total ≈ 1000ms.
        let h = make_http_result(0.5, 1000.0);
        let patched = patch_upload_throughput(h, 1024 * 1024, None);
        let mbps = patched.throughput_mbps.expect("should have throughput");
        assert!((mbps - 1.0).abs() < 1e-9, "expected ~1.0 MB/s, got {mbps}");
    }

    #[test]
    fn upload_near_zero_ttfb_does_not_produce_absurd_throughput_with_server_timing() {
        // Regression guard: with server recv_body_ms, even if client ttfb ≈ 0,
        // the result uses the server's 9000ms drain time.
        let h = make_http_result(0.5, 9000.0);
        let patched = patch_upload_throughput(h, 1024 * 1024 * 1024, Some(9000.0));
        let mbps = patched.throughput_mbps.expect("should have throughput");
        assert!(
            mbps < 10_000.0,
            "throughput must not be astronomically wrong: {mbps}"
        );
        assert!(mbps > 50.0, "throughput must be in a plausible range: {mbps}");
    }

    #[test]
    fn upload_payload_bytes_set() {
        let h = make_http_result(5.0, 100.0);
        let patched = patch_upload_throughput(h, 65536, Some(100.0));
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
