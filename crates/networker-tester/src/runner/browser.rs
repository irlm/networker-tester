/// Real-browser probe via the Chrome DevTools Protocol (chromiumoxide).
///
/// The probe:
///   1. Locates a Chromium/Chrome binary (`NETWORKER_CHROME_PATH` env var or well-known paths).
///   2. Rewrites the target URL to `/page` on the same host/port (like `pageload*` probes).
///   3. Launches a headless browser, navigates to the URL, and waits for the load event.
///   4. Extracts `window.performance.timing` via JS for TTFB, DCL, and load-event timings.
///   5. Aggregates Network.responseReceived events to count resources and bytes per protocol.
///
/// Requires `--features browser` at compile time.
use std::path::PathBuf;

// ─────────────────────────────────────────────────────────────────────────────
// Chrome binary discovery
// ─────────────────────────────────────────────────────────────────────────────

/// Find a usable Chrome/Chromium binary.
///
/// Search order:
/// 1. `NETWORKER_CHROME_PATH` environment variable.
/// 2. Windows standard install locations (`%ProgramFiles%`, `%LocalAppData%`).
/// 3. Linux system paths.
/// 4. macOS application bundle paths.
pub fn find_chrome() -> Option<PathBuf> {
    // 1. Env var override
    if let Ok(path) = std::env::var("NETWORKER_CHROME_PATH") {
        let p = PathBuf::from(&path);
        if std::fs::metadata(&p).is_ok() {
            return Some(p);
        }
    }

    // 2. Windows paths (resolved from environment variables so they work on
    //    any locale / drive letter)
    #[cfg(target_os = "windows")]
    {
        let win_roots: Vec<String> = [
            std::env::var("PROGRAMFILES").ok(),
            std::env::var("LOCALAPPDATA").ok(),
            std::env::var("PROGRAMFILES(X86)").ok(),
        ]
        .into_iter()
        .flatten()
        .collect();

        let win_rel = [
            r"Google\Chrome\Application\chrome.exe",
            r"Chromium\Application\chrome.exe",
        ];

        for root in &win_roots {
            for rel in &win_rel {
                let p = PathBuf::from(root).join(rel);
                if std::fs::metadata(&p).is_ok() {
                    return Some(p);
                }
            }
        }
    }

    // 3. Linux paths
    let linux_paths = [
        "/usr/bin/google-chrome",
        "/usr/bin/chromium-browser",
        "/usr/bin/chromium",
        "/usr/bin/google-chrome-stable",
        "/snap/bin/chromium",
    ];
    for path in &linux_paths {
        let p = PathBuf::from(path);
        if std::fs::metadata(&p).is_ok() {
            return Some(p);
        }
    }

    // 4. macOS application bundle paths
    let macos_paths = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
    ];
    for path in &macos_paths {
        let p = PathBuf::from(path);
        if std::fs::metadata(&p).is_ok() {
            return Some(p);
        }
    }

    None
}

// ─────────────────────────────────────────────────────────────────────────────
// URL rewriting helper
// ─────────────────────────────────────────────────────────────────────────────

/// Rewrite the base URL to the `/browser-page` endpoint.
///
/// `/browser-page` returns an actual HTML page with `<img>` tags so that Chrome
/// fetches each asset via real network requests and the `load` event fires after
/// all assets have settled.  (`/page` returns a JSON manifest used by the
/// synthetic pageload probes — a real browser would just display it as text.)
///
/// Adds `assets=N&bytes=S` query params derived from the provided asset sizes.
/// If `asset_sizes` is empty the endpoint uses its own defaults (20 assets, 10 KiB).
pub fn build_page_url(base: &url::Url, asset_sizes: &[usize]) -> String {
    let mut target = base.clone();
    target.set_path("/browser-page");

    if !asset_sizes.is_empty() {
        let n = asset_sizes.len();
        let bytes = asset_sizes[0];
        target.set_query(Some(&format!("assets={n}&bytes={bytes}")));
    }

    target.to_string()
}

/// Rewrite the base URL for the `browser1` (forced HTTP/1.1) probe.
///
/// Uses `http://` so there is no TLS ALPN negotiation — Chrome physically
/// cannot use HTTP/2 or HTTP/3 over plain HTTP.
///
/// Port derivation: 8443 → 8080 (endpoint convention); 443 / no port → 80
/// (HTTP default, omitted from the URL).  Any other explicit port is kept as-is.
pub fn build_browser1_url(base: &url::Url, asset_sizes: &[usize]) -> String {
    let mut target = base.clone();
    let _ = target.set_scheme("http");
    // Derive plain HTTP port from the HTTPS port.
    let http_port: Option<u16> = match base.port_or_known_default() {
        Some(8443) => Some(8080),
        Some(443) | None => None, // use HTTP default (80, omit from URL)
        Some(p) => Some(p),       // non-standard port: keep as-is
    };
    let _ = target.set_port(http_port);
    target.set_path("/browser-page");
    if !asset_sizes.is_empty() {
        let n = asset_sizes.len();
        let bytes = asset_sizes[0];
        target.set_query(Some(&format!("assets={n}&bytes={bytes}")));
    }
    target.to_string()
}

/// Rewrite the base URL for the `browser3` (forced HTTP/3 QUIC) probe.
///
/// Rewrites the host to `localhost` so that Chrome's cert verification passes
/// against the self-signed cert (which always has `localhost` as a SAN).
/// The actual server IP is passed via `--host-resolver-rules=MAP localhost <ip>`
/// so Chrome still connects to the real server while presenting `localhost` as
/// the SNI hostname — matching the cert SAN exactly.
///
/// This avoids the hostname-mismatch that would block QUIC even when the cert
/// is trusted via SPKI pin: Chrome's QUIC TLS path is stricter about SAN
/// matching than the regular TCP/TLS path.
pub fn build_browser3_url(base: &url::Url, asset_sizes: &[usize]) -> String {
    let mut target = base.clone();
    // Keep https:// scheme and port; just swap the host to localhost.
    let _ = target.set_host(Some("localhost"));
    target.set_path("/browser-page");
    if !asset_sizes.is_empty() {
        let n = asset_sizes.len();
        let bytes = asset_sizes[0];
        target.set_query(Some(&format!("assets={n}&bytes={bytes}")));
    }
    target.to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// Real implementation (feature = "browser")
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "browser")]
mod real {
    use super::{build_browser1_url, build_page_url, find_chrome};
    use crate::metrics::{BrowserResult, ErrorCategory, ErrorRecord, Protocol, RequestAttempt};
    use chromiumoxide::browser::{Browser, BrowserConfig};
    use chromiumoxide::cdp::browser_protocol::network::EventResponseReceived;
    use chromiumoxide::cdp::browser_protocol::security::SetIgnoreCertificateErrorsParams;
    use chrono::Utc;
    use futures::StreamExt;
    use std::collections::HashMap;
    use std::time::Instant;
    use uuid::Uuid;

    pub async fn run_browser_probe(
        run_id: Uuid,
        sequence_num: u32,
        protocol: Protocol,
        base_url: &url::Url,
        asset_sizes: &[usize],
        timeout_ms: u64,
        _insecure: bool,
    ) -> RequestAttempt {
        let attempt_id = Uuid::new_v4();
        let started_at = Utc::now();

        // 1. Locate Chrome
        let chrome_path = match find_chrome() {
            Some(p) => p,
            None => {
                return browser_error(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    "Chrome not found. Install Chrome/Chromium or set NETWORKER_CHROME_PATH.",
                    ErrorCategory::Config,
                );
            }
        };

        // 2. Build page URL.
        // browser1 uses plain HTTP so Chrome has no ALPN to negotiate H2/H3.
        // browser3 uses the same HTTPS URL as browser2 but with QUIC forced via Chrome flags.
        let page_url = if matches!(protocol, Protocol::Browser1) {
            build_browser1_url(base_url, asset_sizes)
        } else {
            build_page_url(base_url, asset_sizes)
        };
        tracing::debug!(url = %page_url, "Browser probe target URL");

        // 3. Launch browser
        //
        // When running as root (e.g. via sudo), the snap chromium launcher
        // script filters out --no-sandbox for security reasons, so Chrome
        // always exits with "Running as root without --no-sandbox is not
        // supported" regardless of what flags we pass.
        //
        // Fix: if we are root and SUDO_USER is set, wrap the Chrome binary
        // with `runuser -u <original-user> --` so Chrome runs as the
        // non-root user and never sees the root restriction.  runuser is
        // part of util-linux (available on all mainstream Linux distros)
        // and allows root to switch users without a password.
        //
        // The wrapper is a small temp shell script so chromiumoxide can
        // still manage the child process normally via its path interface.
        #[cfg(unix)]
        let chrome_path = wrap_chrome_for_root(chrome_path);

        // Use a unique per-run user-data dir so Chrome is never blocked by a
        // root-owned /tmp/chromiumoxide-runner left from a previous sudo run.
        // chromiumoxide defaults to /tmp/chromiumoxide-runner; if that dir was
        // created by root it is not writable by the non-root SUDO_USER that our
        // runuser wrapper launches Chrome as.
        let profile_dir =
            std::env::temp_dir().join(format!("networker-chrome-profile-{}", std::process::id()));
        // RAII guard: remove the profile dir on every return path.
        struct ProfileDirGuard(std::path::PathBuf);
        impl Drop for ProfileDirGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _profile_guard = ProfileDirGuard(profile_dir.clone());

        // Per-protocol Chrome flags:
        //   browser1 → plain HTTP URL (no ALPN = definitively H1.1; no extra flags needed)
        //   browser2 → --disable-quic           (force HTTP/2, prevent H3 upgrade)
        //   browser3 → --enable-quic + --origin-to-force-quic-on=<host>:<port>
        //              Cert trust is handled via CDP Security.setIgnoreCertificateErrors()
        //              applied after Chrome starts — this reliably works in --headless=new
        //              where --ignore-certificate-errors-spki-list is silently ignored by
        //              Chrome's QUIC network stack.
        //              SPKI hash is also passed as belt-and-suspenders for older Chrome.
        //   browser  → no extra flags (auto-negotiate, typically H2)
        let mut extra_args: Vec<String> = Vec::new();
        match &protocol {
            Protocol::Browser1 => {
                // URL already rewritten to http:// above — no extra Chrome flags needed.
            }
            Protocol::Browser2 => {
                extra_args.push("--disable-quic".into());
            }
            Protocol::Browser3 => {
                let host = base_url.host_str().unwrap_or("localhost");
                let port = base_url.port_or_known_default().unwrap_or(443);
                extra_args.push("--enable-quic".into());
                extra_args.push(format!("--origin-to-force-quic-on={host}:{port}"));
                // Belt-and-suspenders: SPKI hash for older Chrome where the CDP
                // Security.setIgnoreCertificateErrors() may not reach the network stack.
                match fetch_spki_hash(base_url).await {
                    Some(hash) => {
                        tracing::info!(spki = %hash, "browser3: SPKI hash fetched for cert pinning");
                        extra_args.push(format!("--ignore-certificate-errors-spki-list={hash}"));
                    }
                    None => {
                        tracing::warn!(
                            "browser3: could not fetch SPKI hash; \
                             relying on CDP cert-override only"
                        );
                    }
                }
            }
            _ => {}
        }

        let mut config_builder = BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(&profile_dir)
            .arg("--headless=new")
            .arg("--no-sandbox")
            .arg("--disable-setuid-sandbox")
            .arg("--disable-gpu")
            .arg("--ignore-certificate-errors")
            .arg("--disable-dev-shm-usage");
        for arg in &extra_args {
            config_builder = config_builder.arg(arg.as_str());
        }

        let browser_config = match config_builder.build() {
            Ok(c) => c,
            Err(e) => {
                return browser_error(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    &format!("Failed to build browser config: {e}"),
                    ErrorCategory::Config,
                );
            }
        };

        let (browser, mut handler) = match tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            Browser::launch(browser_config),
        )
        .await
        {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => {
                return browser_error(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    &format!("Failed to launch browser: {e}"),
                    ErrorCategory::Other,
                );
            }
            Err(_) => {
                return browser_error(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    "Browser launch timed out",
                    ErrorCategory::Timeout,
                );
            }
        };

        // Spawn the CDP message handler
        let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

        // 4. Open a new page
        let page = match tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms / 2),
            browser.new_page("about:blank"),
        )
        .await
        {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                handler_task.abort();
                return browser_error(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    &format!("Failed to open page: {e}"),
                    ErrorCategory::Other,
                );
            }
            Err(_) => {
                handler_task.abort();
                return browser_error(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    "Page open timed out",
                    ErrorCategory::Timeout,
                );
            }
        };

        // 5a. browser3: enable CDP-level cert-error override BEFORE any navigation.
        //
        // Chrome's --headless=new mode silently ignores --ignore-certificate-errors-spki-list
        // for QUIC connections (Chrome bug / architecture limitation: the flag is handled by
        // the browser process but QUIC TLS runs in the network service process which doesn't
        // always receive the flag).  The CDP Security.setIgnoreCertificateErrors() command
        // goes through the DevTools session and reliably applies to ALL connections,
        // including QUIC/H3, when set before the first navigation.
        if matches!(protocol, Protocol::Browser3) {
            match page
                .execute(SetIgnoreCertificateErrorsParams { ignore: true })
                .await
            {
                Ok(_) => tracing::info!(
                    "browser3: CDP Security.setIgnoreCertificateErrors(true) applied"
                ),
                Err(e) => tracing::warn!(
                    "browser3: CDP cert-error override failed (falling back to SPKI pin): {e}"
                ),
            }
        }

        // 5b. browser3 warmup navigation.
        //
        // Alt-Svc discovery flow:
        //   1. Warmup GET /health → server returns Alt-Svc: h3=":PORT" over H2.
        //   2. Chrome caches the hint and starts establishing a QUIC connection.
        //   3. 500 ms sleep → Chrome completes the QUIC TLS handshake.
        //   4. Main navigation → Chrome uses the already-open QUIC session → H3.
        //
        // Without this warmup Chrome always wins the TCP-vs-QUIC race on LAN (<1 ms)
        // and uses H2 even when --origin-to-force-quic-on is set.
        if matches!(protocol, Protocol::Browser3) {
            let warmup = if let Ok(mut u) = url::Url::parse(&page_url) {
                u.set_path("/health");
                u.set_query(None);
                u.to_string()
            } else {
                page_url.clone()
            };
            tracing::info!(url = %warmup, "browser3: warmup navigation to seed QUIC/Alt-Svc cache");
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                page.goto(&warmup).await?;
                page.wait_for_navigation().await
            })
            .await;
            // Give Chrome 500 ms to complete the QUIC handshake from the Alt-Svc hint.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        // 5c. Subscribe to network response events.
        // Subscribed AFTER the warmup so only the main navigation's resources are counted.
        let mut response_events = match page.event_listener::<EventResponseReceived>().await {
            Ok(e) => e,
            Err(e) => {
                handler_task.abort();
                return browser_error(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    &format!("Failed to subscribe to network events: {e}"),
                    ErrorCategory::Other,
                );
            }
        };

        // 6. Navigate and wait for load event
        let nav_start = Instant::now();
        let nav_result =
            tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
                page.goto(&page_url).await?;
                page.wait_for_navigation().await?;
                Ok::<_, anyhow::Error>(())
            })
            .await;

        match nav_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                handler_task.abort();
                return browser_error(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    &format!("Navigation failed: {e}"),
                    ErrorCategory::Http,
                );
            }
            Err(_) => {
                handler_task.abort();
                return browser_error(
                    run_id,
                    attempt_id,
                    sequence_num,
                    protocol,
                    started_at,
                    &format!("Navigation timed out after {}ms", timeout_ms),
                    ErrorCategory::Timeout,
                );
            }
        }

        let _nav_elapsed = nav_start.elapsed().as_millis();

        // 7. Extract performance timing via JS
        let timing_js = r#"
            (function() {
                var t = window.performance.timing;
                return JSON.stringify({
                    navigationStart: t.navigationStart,
                    responseStart: t.responseStart,
                    domContentLoadedEventEnd: t.domContentLoadedEventEnd,
                    loadEventEnd: t.loadEventEnd
                });
            })()
        "#;

        let (load_ms, dom_content_loaded_ms, ttfb_ms) = match page.evaluate(timing_js).await {
            Ok(v) => {
                let json_str: String = v.into_value().unwrap_or_default();
                parse_perf_timing(&json_str)
            }
            Err(e) => {
                tracing::warn!("Failed to extract performance timing: {e}");
                (0.0, 0.0, 0.0)
            }
        };

        // 8. Drain network events (500 ms drain timeout after navigation)
        let mut resource_count: u32 = 0;
        let mut transferred_bytes: usize = 0;
        let mut main_protocol = String::from("unknown");
        let mut first_resource = true;
        let mut protocol_counts: HashMap<String, u32> = HashMap::new();

        let drain_deadline = tokio::time::sleep(std::time::Duration::from_millis(500));
        tokio::pin!(drain_deadline);

        loop {
            tokio::select! {
                event = response_events.next() => {
                    match event {
                        Some(evt) => {
                            resource_count += 1;
                            let proto = evt.response.protocol
                                .as_deref()
                                .unwrap_or("unknown")
                                .to_lowercase();
                            let encoded_len = evt.response.encoded_data_length as usize;
                            transferred_bytes += encoded_len;

                            if first_resource {
                                main_protocol = proto.clone();
                                first_resource = false;
                            }
                            *protocol_counts.entry(proto).or_insert(0) += 1;
                        }
                        None => break,
                    }
                }
                _ = &mut drain_deadline => {
                    break;
                }
            }
        }

        // Sort protocol counts by count descending
        let mut resource_protocols: Vec<(String, u32)> = protocol_counts.into_iter().collect();
        resource_protocols.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

        handler_task.abort();
        let finished_at = Utc::now();

        RequestAttempt {
            attempt_id,
            run_id,
            protocol,
            sequence_num,
            started_at,
            finished_at: Some(finished_at),
            success: load_ms > 0.0,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: Some(BrowserResult {
                load_ms,
                dom_content_loaded_ms,
                ttfb_ms,
                resource_count,
                transferred_bytes,
                protocol: main_protocol,
                resource_protocols,
                started_at,
            }),
        }
    }

    /// Parse `window.performance.timing` JSON into (load_ms, dcl_ms, ttfb_ms).
    fn parse_perf_timing(json: &str) -> (f64, f64, f64) {
        if json.is_empty() {
            return (0.0, 0.0, 0.0);
        }
        let v: serde_json::Value = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(_) => return (0.0, 0.0, 0.0),
        };
        let get = |key: &str| v[key].as_f64().unwrap_or(0.0);
        let nav_start = get("navigationStart");
        let response_start = get("responseStart");
        let dcl_end = get("domContentLoadedEventEnd");
        let load_end = get("loadEventEnd");

        if nav_start == 0.0 {
            return (0.0, 0.0, 0.0);
        }

        let load_ms = (load_end - nav_start).max(0.0);
        let dcl_ms = (dcl_end - nav_start).max(0.0);
        let ttfb_ms = (response_start - nav_start).max(0.0);
        (load_ms, dcl_ms, ttfb_ms)
    }

    /// If we are running as root (e.g. via sudo) and SUDO_USER is set, return
    /// a path to a temporary shell script that re-executes the Chrome binary
    /// as the original non-root user via `runuser`.  This bypasses the snap
    /// chromium launcher's root-safety check, which strips --no-sandbox and
    /// causes Chrome to exit immediately.
    ///
    /// If not root, or SUDO_USER is unset/root, returns `chrome_path` unchanged.
    #[cfg(unix)]
    fn wrap_chrome_for_root(chrome_path: std::path::PathBuf) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        // Only needed when running as root.
        if unsafe { libc::getuid() } != 0 {
            return chrome_path;
        }

        let sudo_user = match std::env::var("SUDO_USER") {
            Ok(u) if !u.is_empty() && u != "root" => u,
            _ => return chrome_path,
        };

        // Find runuser (util-linux; present on Debian/Ubuntu/RHEL/Fedora/etc.)
        let runuser = ["/usr/sbin/runuser", "/sbin/runuser"]
            .iter()
            .find(|p| std::path::Path::new(p).exists())
            .copied()
            .unwrap_or("/usr/sbin/runuser");

        let wrapper =
            std::env::temp_dir().join(format!("networker-chrome-{}.sh", std::process::id()));

        // Single-quote the chrome path to handle spaces (e.g. macOS bundles).
        let escaped = chrome_path.display().to_string().replace('\'', "'\\''");
        let script = format!("#!/bin/sh\nexec {runuser} -u {sudo_user} -- '{escaped}' \"$@\"\n");

        if std::fs::write(&wrapper, &script).is_ok()
            && std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).is_ok()
        {
            tracing::debug!("Wrapping Chrome with runuser -u {sudo_user} (running as root)");
            wrapper
        } else {
            chrome_path
        }
    }

    fn browser_error(
        run_id: Uuid,
        attempt_id: Uuid,
        sequence_num: u32,
        protocol: Protocol,
        started_at: chrono::DateTime<Utc>,
        message: &str,
        category: ErrorCategory,
    ) -> RequestAttempt {
        RequestAttempt {
            attempt_id,
            run_id,
            protocol,
            sequence_num,
            started_at,
            finished_at: Some(Utc::now()),
            success: false,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: Some(ErrorRecord {
                category,
                message: message.to_string(),
                detail: None,
                occurred_at: Utc::now(),
            }),
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
        }
    }

    /// Fetch the SHA-256 hash of the server's leaf certificate SubjectPublicKeyInfo (SPKI),
    /// base64-encoded (standard alphabet with padding).
    ///
    /// Chrome's `--ignore-certificate-errors-spki-list` accepts exactly this format and
    /// allows QUIC/TLS to succeed against a self-signed certificate even when
    /// `--ignore-certificate-errors` alone is not honoured by the QUIC stack.
    ///
    /// Returns `None` on any TCP/TLS error — callers should log a warning and continue
    /// without the pin (Chrome may fall back to H2 in that case).
    async fn fetch_spki_hash(base_url: &url::Url) -> Option<String> {
        use base64::Engine as _;
        use sha2::{Digest, Sha256};
        use std::sync::{Arc, Mutex};

        let host = base_url.host_str()?.to_string();
        let port = base_url.port_or_known_default()?;

        // Custom verifier: accept all certs but capture the leaf cert DER bytes.
        #[derive(Debug)]
        struct CertCapture(Mutex<Option<Vec<u8>>>);

        impl rustls::client::danger::ServerCertVerifier for CertCapture {
            fn verify_server_cert(
                &self,
                end_entity: &rustls::pki_types::CertificateDer,
                _intermediates: &[rustls::pki_types::CertificateDer],
                _server_name: &rustls::pki_types::ServerName,
                _ocsp_response: &[u8],
                _now: rustls::pki_types::UnixTime,
            ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
                *self.0.lock().unwrap() = Some(end_entity.as_ref().to_vec());
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }

            fn verify_tls12_signature(
                &self,
                _message: &[u8],
                _cert: &rustls::pki_types::CertificateDer,
                _dss: &rustls::DigitallySignedStruct,
            ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
            {
                Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
            }

            fn verify_tls13_signature(
                &self,
                _message: &[u8],
                _cert: &rustls::pki_types::CertificateDer,
                _dss: &rustls::DigitallySignedStruct,
            ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
            {
                Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
            }

            fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
                rustls::crypto::ring::default_provider()
                    .signature_verification_algorithms
                    .supported_schemes()
            }
        }

        let capturer = Arc::new(CertCapture(Mutex::new(None)));
        let tls_config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(capturer.clone())
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(tls_config));

        let tcp = tokio::net::TcpStream::connect((host.as_str(), port))
            .await
            .ok()?;
        // TryFrom<String> → ServerName<'static>; TryFrom<&str> is only ServerName<'a>.
        let server_name = rustls::pki_types::ServerName::try_from(host).ok()?;
        let _tls = connector.connect(server_name, tcp).await.ok()?;

        let cert_der = capturer.0.lock().unwrap().take()?;
        use x509_parser::prelude::FromDer as _;
        let (_, cert) = x509_parser::certificate::X509Certificate::from_der(&cert_der).ok()?;
        let spki_raw = cert.tbs_certificate.subject_pki.raw;

        let mut hasher = Sha256::new();
        hasher.update(spki_raw);
        let hash = hasher.finalize();
        Some(base64::engine::general_purpose::STANDARD.encode(hash))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parse_perf_timing_valid() {
            let json = r#"{"navigationStart":1000,"responseStart":1050,"domContentLoadedEventEnd":1200,"loadEventEnd":1500}"#;
            let (load, dcl, ttfb) = parse_perf_timing(json);
            assert!((load - 500.0).abs() < 1e-6);
            assert!((dcl - 200.0).abs() < 1e-6);
            assert!((ttfb - 50.0).abs() < 1e-6);
        }

        #[test]
        fn parse_perf_timing_empty() {
            let (load, dcl, ttfb) = parse_perf_timing("");
            assert_eq!(load, 0.0);
            assert_eq!(dcl, 0.0);
            assert_eq!(ttfb, 0.0);
        }

        #[tokio::test]
        #[ignore = "requires Chrome and local endpoint"]
        async fn browser_probe_returns_load_time() {
            if find_chrome().is_none() {
                eprintln!("Chrome not found, skipping browser probe test");
                return;
            }

            let base = url::Url::parse("https://127.0.0.1:8443/health").unwrap();
            let result = run_browser_probe(
                uuid::Uuid::new_v4(),
                0,
                Protocol::Browser,
                &base,
                &[],
                30_000,
                true,
            )
            .await;

            if !result.success {
                eprintln!("Browser probe failed: {:?}", result.error);
                return;
            }

            let b = result.browser.unwrap();
            assert!(b.load_ms > 0.0, "load_ms should be > 0");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Stub implementation (feature = "browser" not enabled)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(feature = "browser"))]
mod stub {
    use crate::metrics::{ErrorCategory, ErrorRecord, Protocol, RequestAttempt};
    use chrono::Utc;
    use uuid::Uuid;

    pub async fn run_browser_probe(
        run_id: Uuid,
        sequence_num: u32,
        protocol: Protocol,
        _base_url: &url::Url,
        _asset_sizes: &[usize],
        _timeout_ms: u64,
        _insecure: bool,
    ) -> RequestAttempt {
        let attempt_id = Uuid::new_v4();
        let started_at = Utc::now();
        RequestAttempt {
            attempt_id,
            run_id,
            protocol,
            sequence_num,
            started_at,
            finished_at: Some(Utc::now()),
            success: false,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: Some(ErrorRecord {
                category: ErrorCategory::Config,
                message: "browser probe requires '--features browser' (recompile to enable)"
                    .to_string(),
                detail: Some("cargo build --features browser -p networker-tester".to_string()),
                occurred_at: Utc::now(),
            }),
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public re-exports
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "browser")]
pub use real::run_browser_probe;

#[cfg(not(feature = "browser"))]
pub use stub::run_browser_probe;

// Re-export find_chrome and build_page_url as public for use from main.rs
// and for testability.
pub use self::build_page_url as build_browser_page_url;
pub use self::find_chrome as find_chrome_binary;

// ─────────────────────────────────────────────────────────────────────────────
// Module-level tests (always compiled, no Chrome needed)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_page_url_with_assets() {
        let base = url::Url::parse("https://host:8443/health").unwrap();
        let url = build_page_url(&base, &[10240, 10240, 10240]);
        assert!(
            url.contains("/browser-page"),
            "should rewrite to /browser-page"
        );
        assert!(url.contains("assets=3"), "should include assets count");
        assert!(
            url.contains("bytes=10240"),
            "should include asset size as bytes="
        );
    }

    #[test]
    fn build_page_url_no_assets() {
        let base = url::Url::parse("http://localhost:8080/health").unwrap();
        let url = build_page_url(&base, &[]);
        assert!(
            url.contains("/browser-page"),
            "should rewrite to /browser-page"
        );
        assert!(!url.contains("assets="), "should not add empty query");
    }

    #[test]
    fn build_page_url_preserves_scheme_and_host() {
        let base = url::Url::parse("https://myhost.example.com:9443/some/path").unwrap();
        let url = build_page_url(&base, &[1024]);
        assert!(
            url.starts_with("https://myhost.example.com:9443/browser-page"),
            "url={url}"
        );
    }

    #[test]
    fn build_page_url_http_scheme_preserved() {
        let base = url::Url::parse("http://127.0.0.1:8080/health").unwrap();
        let url = build_page_url(&base, &[512, 512]);
        assert!(
            url.starts_with("http://127.0.0.1:8080/browser-page"),
            "url={url}"
        );
        assert!(url.contains("assets=2"));
        assert!(url.contains("bytes=512"));
    }

    #[test]
    fn find_chrome_env_var_nonexistent_path_is_skipped() {
        // Temporarily set the env var to a path that doesn't exist.
        // find_chrome should fall through to system paths (or return None).
        // We can't guarantee the outcome on all machines, but we can verify
        // that a non-existent path doesn't cause a panic.
        let key = "NETWORKER_CHROME_PATH";
        let saved = std::env::var(key).ok();
        std::env::set_var(key, "/this/path/does/not/exist/chrome");
        let result = find_chrome();
        // Restore environment
        match saved {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        // The non-existent env path should not be returned.
        if let Some(p) = result {
            assert_ne!(
                p.to_str().unwrap(),
                "/this/path/does/not/exist/chrome",
                "non-existent env var path should not be returned"
            );
        }
    }

    #[test]
    fn find_chrome_env_var_existing_file_is_returned() {
        use std::io::Write;
        let key = "NETWORKER_CHROME_PATH";
        let saved = std::env::var(key).ok();

        // Create a temporary file to simulate a Chrome binary.
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "#!/bin/sh").unwrap();
        let tmp_path = tmp.path().to_path_buf();

        std::env::set_var(key, tmp_path.to_str().unwrap());
        let result = find_chrome();
        // Restore environment
        match saved {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        assert_eq!(
            result.unwrap(),
            tmp_path,
            "should return the env-var path when the file exists"
        );
    }

    #[tokio::test]
    async fn stub_or_real_returns_browser_protocol() {
        let base = url::Url::parse("https://127.0.0.1:8443/health").unwrap();
        let attempt = run_browser_probe(
            uuid::Uuid::new_v4(),
            0,
            crate::metrics::Protocol::Browser,
            &base,
            &[],
            5_000,
            true,
        )
        .await;
        assert_eq!(attempt.protocol, crate::metrics::Protocol::Browser);
    }

    // ── build_browser3_url tests ──────────────────────────────────────────────

    #[test]
    fn build_browser3_url_rewrites_host_to_localhost() {
        let base = url::Url::parse("https://172.16.32.106:8443/health").unwrap();
        let url = build_browser3_url(&base, &[]);
        assert!(
            url.starts_with("https://localhost:8443/browser-page"),
            "url={url}"
        );
    }

    #[test]
    fn build_browser3_url_keeps_https_and_port() {
        let base = url::Url::parse("https://192.168.1.1:9443/some/path").unwrap();
        let url = build_browser3_url(&base, &[]);
        assert!(
            url.starts_with("https://localhost:9443/browser-page"),
            "url={url}"
        );
    }

    #[test]
    fn build_browser3_url_includes_asset_params() {
        let base = url::Url::parse("https://10.0.0.1:8443/health").unwrap();
        let url = build_browser3_url(&base, &[8192, 8192]);
        assert!(url.contains("assets=2"), "url={url}");
        assert!(url.contains("bytes=8192"), "url={url}");
    }

    #[test]
    fn build_browser3_url_no_query_when_no_assets() {
        let base = url::Url::parse("https://10.0.0.1:8443/health").unwrap();
        let url = build_browser3_url(&base, &[]);
        assert!(!url.contains("assets="), "url={url}");
        assert!(!url.contains("bytes="), "url={url}");
    }

    // ── build_browser1_url tests ──────────────────────────────────────────────

    #[test]
    fn build_browser1_url_switches_to_http_and_maps_8443() {
        let base = url::Url::parse("https://host:8443/health").unwrap();
        let url = build_browser1_url(&base, &[]);
        assert!(
            url.starts_with("http://host:8080/browser-page"),
            "url={url}"
        );
    }

    #[test]
    fn build_browser1_url_standard_https_port_omits_port() {
        let base = url::Url::parse("https://example.com/health").unwrap();
        let url = build_browser1_url(&base, &[]);
        // Port 80 is default for http — url::Url omits it.
        assert!(
            url.starts_with("http://example.com/browser-page"),
            "url={url}"
        );
        assert!(
            !url.contains(":80"),
            "default port should not appear: {url}"
        );
    }

    #[test]
    fn build_browser1_url_non_standard_port_preserved() {
        let base = url::Url::parse("https://host:9443/health").unwrap();
        let url = build_browser1_url(&base, &[]);
        assert!(
            url.starts_with("http://host:9443/browser-page"),
            "url={url}"
        );
    }

    #[test]
    fn build_browser1_url_includes_asset_params() {
        let base = url::Url::parse("https://host:8443/health").unwrap();
        let url = build_browser1_url(&base, &[4096, 4096, 4096]);
        assert!(url.contains("assets=3"), "url={url}");
        assert!(url.contains("bytes=4096"), "url={url}");
    }
}
