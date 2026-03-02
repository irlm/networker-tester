/// Throughput probes: download/webdownload (GET /download?bytes=N) and
/// upload/webupload (POST /upload with N-byte body).
///
/// All four probes rewrite the target URL to `/download` or `/upload` so they
/// work correctly regardless of what path the `--target` flag points at.
/// The `web` variants differ only in their protocol label, enabling
/// side-by-side comparison in reports.
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
/// The transfer window is `max(server_recv_ms, ttfb_ms)`.
///
/// ## Why `ttfb_ms` is the right client-side stopwatch
///
/// `ttfb_ms` is measured in `http.rs` as follows:
/// ```text
/// t_sent = Instant::now()       // just before send_request()
/// send_request(req).await       // writes headers + full body; blocks whenever
///                               // the kernel TCP send buffer fills up
/// ttfb_ms = t_sent.elapsed()    // fires when server sends response headers
/// ```
/// Because `networker-endpoint` only sends its response **after** draining the
/// entire request body, the stopwatch spans: write headers → write body
/// (blocking on the wire) → server drain → response RTT.  At Gigabit speed
/// with a same-machine connection this is ~9 s for 1 GiB — the actual
/// wire transfer time.
///
/// `total_duration_ms` additionally includes the time to read the server's JSON
/// response body (~0.7 ms for 74 bytes).  That is download time, not upload
/// time, so it is a noisier denominator even though the difference is tiny.
///
/// ## Why `max` is still needed
///
/// Old-style endpoints respond **before** draining the request body.  In that
/// case `ttfb_ms ≈ 0 ms` (response headers arrive instantly) while
/// `server_recv_ms` (from `Server-Timing: recv;dur=X`) is the accurate value.
/// Taking `max` of both covers every scenario:
///
///   our endpoint (drain-before-respond):  ttfb ≈ 9134ms, recv ≈ 0.124ms → ttfb wins   ✓
///   old-style (respond-before-drain):     ttfb ≈ 0.2ms,  recv ≈ 9000ms  → recv wins   ✓
///   generic (no Server-Timing header):    ttfb ≈ correct, recv = None   → ttfb used   ✓
///
///   transfer_ms = max(server_timing.recv_body_ms, ttfb_ms)
use crate::metrics::{HttpResult, Protocol, RequestAttempt};
use crate::runner::http::{run_probe, RunConfig};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Goodput helper
// ─────────────────────────────────────────────────────────────────────────────

/// Sum of the connection-setup phases (DNS + TCP + TLS) for an attempt.
/// Used as the overhead component of goodput: goodput = payload / (overhead + http_total).
fn compute_overhead_ms(attempt: &RequestAttempt) -> f64 {
    let dns = attempt.dns.as_ref().map(|d| d.duration_ms).unwrap_or(0.0);
    let tcp = attempt
        .tcp
        .as_ref()
        .map(|t| t.connect_duration_ms)
        .unwrap_or(0.0);
    let tls = attempt
        .tls
        .as_ref()
        .map(|t| t.handshake_duration_ms)
        .unwrap_or(0.0);
    dns + tcp + tls
}

// ─────────────────────────────────────────────────────────────────────────────
// WebDownload probe
// ─────────────────────────────────────────────────────────────────────────────

/// GET `/download?bytes=N` on the target host and measure response body throughput.
///
/// Rewrites the URL path to `/download` and sets `bytes=<N>` — identical
/// URL construction to `run_download_probe`.  The protocol label in the result
/// is `webdownload` so the two modes can be compared side-by-side in reports.
pub async fn run_webdownload_probe(
    run_id: Uuid,
    sequence_num: u32,
    payload_bytes: usize,
    cfg: &ThroughputConfig,
) -> RequestAttempt {
    let mut target = cfg.base_url.clone();
    target.set_path("/download");
    target.set_query(Some(&format!("bytes={payload_bytes}")));

    let probe_cfg = RunConfig {
        payload_size: 0, // GET — body comes from server
        ..cfg.run_cfg.clone()
    };

    let mut attempt = run_probe(
        run_id,
        sequence_num,
        Protocol::WebDownload,
        &target,
        &probe_cfg,
    )
    .await;

    if let Some(h) = attempt.http.clone() {
        let body_size = h.body_size_bytes;
        let mut patched = patch_throughput(h, body_size);
        let overhead_ms = compute_overhead_ms(&attempt);
        patched.goodput_mbps = mbps(
            patched.payload_bytes,
            overhead_ms + patched.total_duration_ms,
        );
        attempt.http = Some(patched);
    }
    attempt
}

// ─────────────────────────────────────────────────────────────────────────────
// WebUpload probe
// ─────────────────────────────────────────────────────────────────────────────

/// POST a `payload_bytes`-byte body to `/upload` on the target host and
/// measure upload throughput.
///
/// Rewrites the URL path to `/upload` — identical URL construction to
/// `run_upload_probe`.  The protocol label in the result is `webupload` so
/// the two modes can be compared side-by-side in reports.
pub async fn run_webupload_probe(
    run_id: Uuid,
    sequence_num: u32,
    payload_bytes: usize,
    cfg: &ThroughputConfig,
) -> RequestAttempt {
    let mut target = cfg.base_url.clone();
    target.set_path("/upload");
    target.set_query(None);

    let probe_cfg = RunConfig {
        payload_size: payload_bytes,
        ..cfg.run_cfg.clone()
    };

    let mut attempt = run_probe(
        run_id,
        sequence_num,
        Protocol::WebUpload,
        &target,
        &probe_cfg,
    )
    .await;

    if let Some(h) = attempt.http.clone() {
        let server_recv_ms = attempt
            .server_timing
            .as_ref()
            .and_then(|st| st.recv_body_ms);
        let mut patched = patch_webupload_throughput(h, payload_bytes, server_recv_ms);
        let overhead_ms = compute_overhead_ms(&attempt);
        patched.goodput_mbps = mbps(
            patched.payload_bytes,
            overhead_ms + patched.total_duration_ms,
        );
        attempt.http = Some(patched);
    }
    attempt
}

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
        let mut patched = patch_throughput(h, payload_bytes);
        let overhead_ms = compute_overhead_ms(&attempt);
        patched.goodput_mbps = mbps(
            patched.payload_bytes,
            overhead_ms + patched.total_duration_ms,
        );
        attempt.http = Some(patched);
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
        let mut patched = patch_upload_throughput(h, payload_bytes, server_recv_ms);
        let overhead_ms = compute_overhead_ms(&attempt);
        patched.goodput_mbps = mbps(
            patched.payload_bytes,
            overhead_ms + patched.total_duration_ms,
        );
        attempt.http = Some(patched);
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
/// Uses `max(server_recv_ms, h.ttfb_ms)` as the transfer window —
/// see module-level documentation for why `ttfb_ms` is the right stopwatch
/// and why `max` is needed.  When `server_recv_ms` is absent, falls back to
/// `h.ttfb_ms` alone.
fn patch_upload_throughput(
    h: HttpResult,
    payload_bytes: usize,
    server_recv_ms: Option<f64>,
) -> HttpResult {
    let transfer_ms = match server_recv_ms {
        Some(srv) => srv.max(h.ttfb_ms),
        None => h.ttfb_ms,
    };
    let throughput_mbps = mbps(payload_bytes, transfer_ms);
    HttpResult {
        payload_bytes,
        throughput_mbps,
        ..h
    }
}

/// Compute and attach upload throughput to a `webupload` `HttpResult`.
///
/// Unlike `patch_upload_throughput`, the fallback when `server_recv_ms` is
/// absent uses `total_duration_ms` rather than `ttfb_ms`.  For generic targets
/// that respond *before* draining the request body, `ttfb_ms` can be
/// near-zero (the server ignored the body), which would produce absurdly high
/// throughput figures.  `total_duration_ms` has the same flaw in that edge
/// case, but using it is more consistent with "how long the whole request
/// took" semantics.
///
/// A physical cap of **100,000 MB/s** (≈ 800 Gbps) is applied regardless of
/// denominator: anything above that is physically impossible on today's
/// hardware and indicates that the server responded without draining the body.
/// In that case `throughput_mbps` is set to `None` rather than a nonsensical
/// value.
fn patch_webupload_throughput(
    h: HttpResult,
    payload_bytes: usize,
    server_recv_ms: Option<f64>,
) -> HttpResult {
    let transfer_ms = match server_recv_ms {
        Some(srv) => srv.max(h.ttfb_ms),
        None => h.total_duration_ms,
    };
    // Cap: 100,000 MB/s ≈ 800 Gbps — exceeds any real network link.
    // Values above this mean the server replied before reading the body.
    const MAX_PLAUSIBLE_MBPS: f64 = 100_000.0;
    let throughput_mbps = mbps(payload_bytes, transfer_ms).filter(|&v| v <= MAX_PLAUSIBLE_MBPS);
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

    // ─────────────────────────────────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────────────────────────────────

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
            goodput_mbps: None,
            cpu_time_ms: None,
            csw_voluntary: None,
            csw_involuntary: None,
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // mbps — unit conversion
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn mbps_one_mib_in_one_second() {
        // 1 MiB / 1000ms * 1000 / 1MiB = 1.0 MB/s
        let result = mbps(1024 * 1024, 1000.0).expect("should produce a value");
        assert!((result - 1.0).abs() < 1e-9, "expected 1.0, got {result}");
    }

    #[test]
    fn mbps_ten_mib_in_one_second() {
        let result = mbps(10 * 1024 * 1024, 1000.0).expect("should produce a value");
        assert!((result - 10.0).abs() < 1e-9, "expected 10.0, got {result}");
    }

    #[test]
    fn mbps_one_gib_in_1024ms_is_1000_mbs() {
        // 1 GiB = 1024 MiB; in 1024ms → 1024 MiB/s = 1000 MB/s (exact)
        // 1073741824 / 1024 * 1000 / 1048576 = 1000.0
        let result = mbps(1024 * 1024 * 1024, 1024.0).expect("should produce a value");
        assert!(
            (result - 1000.0).abs() < 1e-6,
            "expected 1000.0, got {result}"
        );
    }

    #[test]
    fn mbps_zero_payload_is_zero() {
        // Zero bytes transferred is valid (e.g. empty body); rate = 0.
        let result = mbps(0, 1000.0).expect("should produce Some(0)");
        assert_eq!(result, 0.0);
    }

    #[test]
    fn mbps_zero_transfer_ms_returns_none() {
        // Division by zero must be guarded.
        assert!(mbps(1024 * 1024, 0.0).is_none());
    }

    #[test]
    fn mbps_negative_transfer_ms_returns_none() {
        // Negative window (e.g. ttfb > total) must not produce a result.
        assert!(mbps(1024 * 1024, -5.0).is_none());
    }

    #[test]
    fn mbps_single_byte_in_one_second() {
        // 1 byte in 1000ms → 1000 / 1048576 B/ms = ~9.54e-4 MB/s
        let expected = 1.0_f64 / 1000.0 * 1000.0 / (1024.0 * 1024.0);
        let result = mbps(1, 1000.0).expect("should produce a value");
        assert!(
            (result - expected).abs() < 1e-15,
            "expected {expected}, got {result}"
        );
    }

    #[test]
    fn mbps_very_slow_transfer() {
        // 1 KiB in 1 hour (~3600000ms) → tiny but non-zero
        let result = mbps(1024, 3_600_000.0).expect("should produce a value");
        assert!(result > 0.0);
        assert!(result < 0.001, "should be very slow: {result}");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // patch_throughput — download
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn download_uses_body_receive_time() {
        // 1 MiB in 1000ms body window (ttfb=10ms, total=1010ms) → 1.0 MB/s
        let h = make_http_result(10.0, 1010.0);
        let patched = patch_throughput(h, 1024 * 1024);
        let result = patched.throughput_mbps.expect("should have throughput");
        assert!(
            (result - 1.0).abs() < 1e-9,
            "expected 1.0 MB/s, got {result}"
        );
    }

    #[test]
    fn download_excludes_ttfb_from_window() {
        // 4 MiB in 4000ms body window (ttfb=1000ms, total=5000ms) → 1.0 MB/s
        let h = make_http_result(1000.0, 5000.0);
        let patched = patch_throughput(h, 4 * 1024 * 1024);
        let result = patched.throughput_mbps.expect("should have throughput");
        assert!(
            (result - 1.0).abs() < 1e-9,
            "expected 1.0 MB/s, got {result}"
        );
    }

    #[test]
    fn download_throughput_none_when_ttfb_equals_total() {
        // ttfb == total → body window = 0ms → no meaningful rate
        let h = make_http_result(100.0, 100.0);
        assert!(patch_throughput(h, 65536).throughput_mbps.is_none());
    }

    #[test]
    fn download_throughput_none_when_ttfb_exceeds_total() {
        // Malformed timing; must not produce a result.
        let h = make_http_result(200.0, 100.0);
        assert!(patch_throughput(h, 65536).throughput_mbps.is_none());
    }

    #[test]
    fn download_sets_payload_bytes() {
        let h = make_http_result(5.0, 100.0);
        assert_eq!(patch_throughput(h, 65536).payload_bytes, 65536);
    }

    #[test]
    fn download_preserves_other_http_fields() {
        // Patching must not alter unrelated timing fields.
        let h = make_http_result(12.5, 512.5);
        let patched = patch_throughput(h, 1024);
        assert!((patched.ttfb_ms - 12.5).abs() < 1e-9);
        assert!((patched.total_duration_ms - 512.5).abs() < 1e-9);
        assert_eq!(patched.status_code, 200);
        assert_eq!(patched.negotiated_version, "HTTP/1.1");
    }

    #[test]
    fn download_zero_payload_gives_zero_throughput() {
        // 0-byte body is valid; rate = 0.
        let h = make_http_result(10.0, 1010.0);
        let result = patch_throughput(h, 0)
            .throughput_mbps
            .expect("should be Some(0)");
        assert_eq!(result, 0.0);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // patch_upload_throughput — upload, server timing present
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn upload_uses_server_recv_ms_when_available() {
        // 1 MiB drained by server in 1000ms → 1.0 MB/s
        let h = make_http_result(0.5, 1000.0);
        let patched = patch_upload_throughput(h, 1024 * 1024, Some(1000.0));
        let result = patched.throughput_mbps.expect("should have throughput");
        assert!(
            (result - 1.0).abs() < 1e-9,
            "expected 1.0 MB/s, got {result}"
        );
    }

    #[test]
    fn upload_ttfb_wins_when_larger_than_server_recv() {
        // Our endpoint (drain-before-respond): ttfb captures full upload time.
        // server_recv_ms (500ms) < ttfb_ms (1000ms) → max picks ttfb.
        // 1 MiB / 1000ms = 1.0 MB/s.
        let h = make_http_result(1000.0, 1000.7);
        let patched = patch_upload_throughput(h, 1024 * 1024, Some(500.0));
        let result = patched.throughput_mbps.expect("should have throughput");
        assert!(
            (result - 1.0).abs() < 1e-9,
            "expected 1.0 MB/s, got {result}"
        );
    }

    #[test]
    fn upload_server_recv_ms_wins_when_larger_than_ttfb() {
        // Old-style endpoint responds before draining:
        //   ttfb_ms ≈ 0.2ms (server responded instantly; upload still in flight)
        //   server recv_body_ms = 9000ms → max picks server → ~113 MB/s  ✓
        let h = make_http_result(0.2, 0.2);
        let patched = patch_upload_throughput(h, 1024 * 1024 * 1024, Some(9000.0));
        let result = patched.throughput_mbps.expect("should have throughput");
        assert!(
            result < 10_000.0,
            "must not be astronomically wrong: {result}"
        );
        assert!(result > 50.0, "must be in plausible range: {result}");
    }

    #[test]
    fn upload_same_machine_kernel_buffer_case() {
        // Same-machine Gigabit (e.g. 172.16.32.106 → 172.16.32.106):
        //   - Data travels through the NIC driver at Gigabit speed (~9134ms for 1 GiB)
        //   - hyper's send_request blocks until server responds → ttfb_ms ≈ 9134ms
        //   - By the time the axum handler runs the kernel buffer is already full
        //     → server_recv_ms ≈ 0.124ms (memory-copy speed — NOT wire speed)
        //   - max(0.124, 9134.7) = 9134.7ms → ~112 MB/s  ✓
        let h = make_http_result(9134.7, 9134.7); // ttfb ≈ total for uploads
        let patched = patch_upload_throughput(h, 1024 * 1024 * 1024, Some(0.124));
        let result = patched.throughput_mbps.expect("should have throughput");
        // Should be ~112 MB/s, not 8 million MB/s.
        assert!(
            result < 10_000.0,
            "must not be astronomically wrong: {result}"
        );
        assert!(result > 50.0, "must be in plausible range: {result}");
        // More precisely: 1 GiB / 9134.7ms → ~112 MB/s
        let expected = 1024.0_f64 * 1024.0 * 1024.0 / 9134.7 * 1000.0 / (1024.0 * 1024.0);
        assert!(
            (result - expected).abs() < 1.0,
            "expected ~{expected:.1} MB/s, got {result:.1} MB/s"
        );
    }

    #[test]
    fn upload_server_recv_ms_zero_falls_back_to_ttfb() {
        // server_recv_ms = 0 is invalid; max(0, ttfb=1000ms) = 1000ms → 1.0 MB/s.
        // In practice ttfb ≈ total for uploads (response body is tiny).
        let h = make_http_result(1000.0, 1000.7);
        let result = patch_upload_throughput(h, 1024 * 1024, Some(0.0))
            .throughput_mbps
            .expect("should fall back to ttfb_ms");
        assert!(
            (result - 1.0).abs() < 1e-9,
            "expected 1.0 MB/s, got {result}"
        );
    }

    #[test]
    fn upload_server_recv_ms_negative_falls_back_to_ttfb() {
        // server_recv_ms < 0 is invalid; max(-10, ttfb=1000ms) = 1000ms → 1.0 MB/s.
        let h = make_http_result(1000.0, 1000.7);
        let result = patch_upload_throughput(h, 1024 * 1024, Some(-10.0))
            .throughput_mbps
            .expect("should fall back to ttfb_ms");
        assert!(
            (result - 1.0).abs() < 1e-9,
            "expected 1.0 MB/s, got {result}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // patch_upload_throughput — upload, no server timing (fallback)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn upload_falls_back_to_ttfb_without_server_timing() {
        // No Server-Timing header (generic endpoint); ttfb_ms is the stopwatch.
        // ttfb ≈ total for uploads (response body is tiny).
        let h = make_http_result(1000.0, 1000.7);
        let patched = patch_upload_throughput(h, 1024 * 1024, None);
        let result = patched.throughput_mbps.expect("should have throughput");
        assert!(
            (result - 1.0).abs() < 1e-9,
            "expected 1.0 MB/s, got {result}"
        );
    }

    #[test]
    fn upload_fallback_none_when_ttfb_is_zero() {
        // ttfb = 0ms with no server timing → undefined rate.
        let h = make_http_result(0.0, 100.0);
        assert!(patch_upload_throughput(h, 65536, None)
            .throughput_mbps
            .is_none());
    }

    // ─────────────────────────────────────────────────────────────────────────
    // patch_upload_throughput — field preservation
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn upload_sets_payload_bytes() {
        let h = make_http_result(5.0, 100.0);
        assert_eq!(
            patch_upload_throughput(h, 65536, Some(100.0)).payload_bytes,
            65536
        );
    }

    #[test]
    fn upload_preserves_other_http_fields() {
        let h = make_http_result(12.5, 512.5);
        let patched = patch_upload_throughput(h, 1024, Some(100.0));
        assert!((patched.ttfb_ms - 12.5).abs() < 1e-9);
        assert!((patched.total_duration_ms - 512.5).abs() < 1e-9);
        assert_eq!(patched.status_code, 200);
        assert_eq!(patched.negotiated_version, "HTTP/1.1");
    }

    #[test]
    fn upload_zero_payload_gives_zero_throughput() {
        let h = make_http_result(0.5, 1000.0);
        let result = patch_upload_throughput(h, 0, Some(1000.0))
            .throughput_mbps
            .expect("should be Some(0)");
        assert_eq!(result, 0.0);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // patch_webupload_throughput
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn webupload_server_recv_ms_wins_when_present() {
        // Same semantics as upload when server timing is available.
        let h = make_http_result(0.5, 1000.0);
        let patched = patch_webupload_throughput(h, 1024 * 1024, Some(2000.0));
        // max(2000.0, 0.5) = 2000 ms → 1 MiB / 2 s = 0.5 MB/s
        let thr = patched.throughput_mbps.expect("should compute throughput");
        assert!((thr - 0.5).abs() < 1e-6, "expected 0.5, got {thr}");
    }

    #[test]
    fn webupload_falls_back_to_total_duration_without_server_timing() {
        // No server_recv_ms: use total_duration_ms (4000 ms) not ttfb_ms (0.5 ms).
        let h = make_http_result(0.5, 4000.0);
        let patched = patch_webupload_throughput(h, 1024 * 1024, None);
        // 1 MiB / 4000 ms * 1000 / 1 MiB = 0.25 MB/s
        let thr = patched.throughput_mbps.expect("should compute throughput");
        assert!((thr - 0.25).abs() < 1e-6, "expected 0.25, got {thr}");
    }

    #[test]
    fn webupload_absurd_throughput_suppressed() {
        // Server responded in 0.4 ms without draining 512 MiB body.
        let payload = 512 * 1024 * 1024; // 512 MiB
        let h = make_http_result(0.4, 0.4);
        let patched = patch_webupload_throughput(h, payload, None);
        // Would be ~1.3M MB/s — must be capped to None.
        assert!(
            patched.throughput_mbps.is_none(),
            "expected None for implausible throughput, got {:?}",
            patched.throughput_mbps
        );
    }

    #[test]
    fn webupload_plausible_throughput_shown() {
        // 512 MiB upload in 4832 ms ≈ 106 MB/s — well within the cap.
        let payload = 512 * 1024 * 1024;
        let h = make_http_result(4832.0, 4833.0);
        let patched = patch_webupload_throughput(h, payload, None);
        let thr = patched.throughput_mbps.expect("should compute throughput");
        assert!(thr > 100.0 && thr < 200.0, "expected ~106 MB/s, got {thr}");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Integration tests (require live endpoint)
    // ─────────────────────────────────────────────────────────────────────────

    async fn endpoint_available(addr: &str) -> bool {
        tokio::net::TcpStream::connect(addr).await.is_ok()
    }

    #[tokio::test]
    #[ignore = "requires local networker-endpoint on :8080"]
    async fn download_probe_returns_throughput() {
        if !endpoint_available("127.0.0.1:8080").await {
            eprintln!("Skipping download_probe_returns_throughput: no endpoint on :8080");
            return;
        }
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
        if !endpoint_available("127.0.0.1:8080").await {
            eprintln!("Skipping upload_probe_returns_throughput: no endpoint on :8080");
            return;
        }
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
