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

// ─────────────────────────────────────────────────────────────────────────────
// Real implementation (feature = "browser")
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "browser")]
mod real {
    use super::{build_page_url, find_chrome};
    use crate::metrics::{BrowserResult, ErrorCategory, ErrorRecord, Protocol, RequestAttempt};
    use chromiumoxide::browser::{Browser, BrowserConfig};
    use chromiumoxide::cdp::browser_protocol::network::EventResponseReceived;
    use chrono::Utc;
    use futures::StreamExt;
    use std::collections::HashMap;
    use std::time::Instant;
    use uuid::Uuid;

    pub async fn run_browser_probe(
        run_id: Uuid,
        sequence_num: u32,
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
                    started_at,
                    "Chrome not found. Install Chrome/Chromium or set NETWORKER_CHROME_PATH.",
                    ErrorCategory::Config,
                );
            }
        };

        // 2. Build page URL
        let page_url = build_page_url(base_url, asset_sizes);
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

        let browser_config = match BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .arg("--headless=new")
            .arg("--no-sandbox")
            .arg("--disable-setuid-sandbox")
            .arg("--disable-gpu")
            .arg("--ignore-certificate-errors")
            .arg("--disable-dev-shm-usage")
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return browser_error(
                    run_id,
                    attempt_id,
                    sequence_num,
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
                    started_at,
                    "Page open timed out",
                    ErrorCategory::Timeout,
                );
            }
        };

        // 5. Subscribe to network response events
        let mut response_events = match page.event_listener::<EventResponseReceived>().await {
            Ok(e) => e,
            Err(e) => {
                handler_task.abort();
                return browser_error(
                    run_id,
                    attempt_id,
                    sequence_num,
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
            protocol: Protocol::Browser,
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

        let wrapper = std::env::temp_dir()
            .join(format!("networker-chrome-{}.sh", std::process::id()));

        // Single-quote the chrome path to handle spaces (e.g. macOS bundles).
        let escaped = chrome_path.display().to_string().replace('\'', "'\\''");
        let script = format!(
            "#!/bin/sh\nexec {runuser} -u {sudo_user} -- '{escaped}' \"$@\"\n"
        );

        if std::fs::write(&wrapper, &script).is_ok()
            && std::fs::set_permissions(
                &wrapper,
                std::fs::Permissions::from_mode(0o755),
            )
            .is_ok()
        {
            tracing::debug!(
                "Wrapping Chrome with runuser -u {sudo_user} (running as root)"
            );
            wrapper
        } else {
            chrome_path
        }
    }

    fn browser_error(
        run_id: Uuid,
        attempt_id: Uuid,
        sequence_num: u32,
        started_at: chrono::DateTime<Utc>,
        message: &str,
        category: ErrorCategory,
    ) -> RequestAttempt {
        RequestAttempt {
            attempt_id,
            run_id,
            protocol: Protocol::Browser,
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
            let result = run_browser_probe(uuid::Uuid::new_v4(), 0, &base, &[], 30_000, true).await;

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
            protocol: Protocol::Browser,
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
        assert!(url.contains("/browser-page"), "should rewrite to /browser-page");
        assert!(url.contains("assets=3"), "should include assets count");
        assert!(url.contains("bytes=10240"), "should include asset size as bytes=");
    }

    #[test]
    fn build_page_url_no_assets() {
        let base = url::Url::parse("http://localhost:8080/health").unwrap();
        let url = build_page_url(&base, &[]);
        assert!(url.contains("/browser-page"), "should rewrite to /browser-page");
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
        let attempt = run_browser_probe(uuid::Uuid::new_v4(), 0, &base, &[], 5_000, true).await;
        assert_eq!(attempt.protocol, crate::metrics::Protocol::Browser);
    }
}
