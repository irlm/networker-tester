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
// URL rewriting helpers
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
    use std::path::Path;
    use std::time::Instant;
    use uuid::Uuid;

    // ── Cert trust helpers ────────────────────────────────────────────────────

    /// DER-encoded certificate → PEM string (base64 with header/footer).
    fn der_to_pem(cert_der: &[u8]) -> String {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(cert_der);
        let lines = b64
            .as_bytes()
            .chunks(64)
            .map(|c| std::str::from_utf8(c).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");
        format!("-----BEGIN CERTIFICATE-----\n{lines}\n-----END CERTIFICATE-----\n")
    }

    /// SHA-256 hash of the DER cert bytes, hex-encoded without separators.
    /// Used by macOS `security delete-certificate -Z` for cleanup.
    fn compute_cert_sha256_hex(cert_der: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(cert_der);
        hash.iter().map(|b| format!("{b:02X}")).collect()
    }

    // ── CertTrustGuard ────────────────────────────────────────────────────────

    /// RAII guard that removes the temporarily-installed cert trust on drop.
    ///
    /// Ensures cleanup even if the browser probe panics or returns early.
    struct CertTrustGuard {
        /// macOS: SHA-256 fingerprint for `security delete-certificate -Z`.
        #[cfg(target_os = "macos")]
        sha256_hex: String,
        /// Linux: NSS db path for `certutil -D`.
        #[cfg(target_os = "linux")]
        nss_db_path: String,
        /// Linux: cert nickname for `certutil -D`.
        #[cfg(target_os = "linux")]
        cert_name: String,
        // Other platforms: guard is a no-op; keep the struct non-empty.
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        _unused: (),
    }

    impl Drop for CertTrustGuard {
        fn drop(&mut self) {
            #[cfg(target_os = "macos")]
            {
                tracing::debug!(
                    sha256 = %self.sha256_hex,
                    "browser3: removing temp cert from login keychain"
                );
                let _ = std::process::Command::new("security")
                    .args(["delete-certificate", "-Z", &self.sha256_hex])
                    .output();
            }
            #[cfg(target_os = "linux")]
            {
                tracing::debug!(
                    db = %self.nss_db_path,
                    name = %self.cert_name,
                    "browser3: removing temp cert from NSS db"
                );
                let _ = std::process::Command::new("certutil")
                    .args(["-D", "-d", &self.nss_db_path, "-n", &self.cert_name])
                    .output();
            }
        }
    }

    // ── install_cert_trust ────────────────────────────────────────────────────

    /// Install the server's leaf cert as a temporarily-trusted root so Chrome's
    /// QUIC stack accepts it at the **network-service level** (not just the page target).
    ///
    /// # Why this is needed
    ///
    /// Chrome silently discards Alt-Svc hints from connections where cert errors
    /// were overridden (e.g. via `--ignore-certificate-errors`).  This is a
    /// security policy: upgrading a broken connection to QUIC would allow a MITM
    /// to force protocol negotiation.
    ///
    /// With the cert *actually trusted* (no errors at all), Chrome processes Alt-Svc
    /// normally, schedules a background QUIC session, and the main navigation uses H3.
    ///
    /// # Platform behaviour
    ///
    /// **macOS**: adds the cert to the user's login keychain as a trusted root via
    /// `security add-trusted-cert`.  `CertTrustGuard::drop` removes it afterwards.
    ///
    /// **Linux**: creates an NSS certificate database in `profile_dir/Default/` and
    /// imports the cert as trusted via `certutil` (package `libnss3-tools`).
    /// Chrome on Linux reads this NSS db at startup, so the profile dir must be
    /// pre-created before `Browser::launch`.
    ///
    /// **Other platforms**: returns an error; the probe falls back to H2.
    async fn install_cert_trust(
        cert_der: &[u8],
        profile_dir: &Path,
    ) -> anyhow::Result<CertTrustGuard> {
        // Write PEM into the profile dir (cleaned up by ProfileDirGuard on exit).
        let pem = der_to_pem(cert_der);
        let pem_path = profile_dir.join("browser3-server-cert.pem");
        tokio::fs::write(&pem_path, pem.as_bytes()).await?;
        install_cert_trust_inner(cert_der, &pem_path, profile_dir).await
    }

    #[cfg(target_os = "macos")]
    async fn install_cert_trust_inner(
        cert_der: &[u8],
        pem_path: &Path,
        _profile_dir: &Path,
    ) -> anyhow::Result<CertTrustGuard> {
        let sha256_hex = compute_cert_sha256_hex(cert_der);
        let home = std::env::var("HOME").unwrap_or_default();
        let keychain = format!("{home}/Library/Keychains/login.keychain-db");
        let out = tokio::process::Command::new("security")
            .args([
                "add-trusted-cert",
                "-d",
                "-r",
                "trustRoot",
                "-k",
                &keychain,
                pem_path.to_str().unwrap_or_default(),
            ])
            .output()
            .await?;
        if !out.status.success() {
            anyhow::bail!(
                "security add-trusted-cert failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        tracing::info!(%sha256_hex, "browser3: server cert added to login keychain");
        Ok(CertTrustGuard { sha256_hex })
    }

    #[cfg(target_os = "linux")]
    async fn install_cert_trust_inner(
        _cert_der: &[u8],
        pem_path: &Path,
        profile_dir: &Path,
    ) -> anyhow::Result<CertTrustGuard> {
        // Verify certutil is available.
        let certutil_path = which_command("certutil").ok_or_else(|| {
            anyhow::anyhow!(
                "certutil not found; install libnss3-tools: \
                 apt install libnss3-tools  (or  dnf install nss-tools)"
            )
        })?;

        // NSS db lives inside the Chrome profile dir (Chrome reads it at startup).
        let nss_dir = profile_dir.join("Default");
        tokio::fs::create_dir_all(&nss_dir).await?;
        let db_path = format!("sql:{}", nss_dir.display());

        // Initialise NSS db (ignore error if already exists).
        let _ = tokio::process::Command::new(&certutil_path)
            .args(["-N", "-d", &db_path, "--empty-password"])
            .output()
            .await;

        // Import cert as a trusted CA.
        let cert_name = format!("networker-endpoint-{}", std::process::id());
        let out = tokio::process::Command::new(&certutil_path)
            .args([
                "-A",
                "-d",
                &db_path,
                "-t",
                "CT,,",
                "-n",
                &cert_name,
                "-i",
                pem_path.to_str().unwrap_or_default(),
            ])
            .output()
            .await?;
        if !out.status.success() {
            anyhow::bail!(
                "certutil -A failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        tracing::info!(db = %db_path, name = %cert_name, "browser3: cert imported into NSS db");
        Ok(CertTrustGuard {
            nss_db_path: db_path,
            cert_name,
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    async fn install_cert_trust_inner(
        _cert_der: &[u8],
        _pem_path: &Path,
        _profile_dir: &Path,
    ) -> anyhow::Result<CertTrustGuard> {
        anyhow::bail!("cert trust installation not supported on this platform (macOS/Linux only)")
    }

    /// Returns the full path to `cmd` if it exists anywhere in `$PATH`.
    #[cfg(target_os = "linux")]
    fn which_command(cmd: &str) -> Option<std::path::PathBuf> {
        std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path).find_map(|dir| {
                let p = dir.join(cmd);
                if p.exists() {
                    Some(p)
                } else {
                    None
                }
            })
        })
    }

    // ── Main probe ────────────────────────────────────────────────────────────

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

        // 1. Locate Chrome.
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
        // browser2/3 use the same HTTPS URL; protocol is forced via Chrome flags.
        let page_url = if matches!(protocol, Protocol::Browser1) {
            build_browser1_url(base_url, asset_sizes)
        } else {
            build_page_url(base_url, asset_sizes)
        };
        tracing::debug!(url = %page_url, "Browser probe target URL");

        // 3. Per-run user-data dir.
        //
        // Created EARLY (before Chrome launches) so the Linux NSS cert database
        // can be pre-populated in profile_dir/Default/ before Chrome reads it
        // at startup.  On macOS the keychain is queried dynamically, so order
        // relative to Chrome launch is less critical — but pre-creation is still
        // required to write the PEM file used by security(1).
        let profile_dir =
            std::env::temp_dir().join(format!("networker-chrome-profile-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&profile_dir); // pre-create for NSS db

        struct ProfileDirGuard(std::path::PathBuf);
        impl Drop for ProfileDirGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _profile_guard = ProfileDirGuard(profile_dir.clone());

        // 4. browser3: fetch the server cert and install it as a trusted root.
        //
        // This must happen BEFORE Browser::launch so that:
        //   (a) On Linux: the NSS db in profile_dir/Default/ exists at Chrome startup.
        //   (b) On macOS: the keychain entry is present when Chrome's TLS stack first
        //       queries it during the QUIC handshake.
        //
        // Without actual cert trust, Chrome silently discards Alt-Svc hints from
        // connections where cert errors were overridden (--ignore-certificate-errors).
        // With the cert trusted (no errors), Chrome processes Alt-Svc → schedules a
        // background QUIC session → main navigation uses H3.
        let _cert_trust_guard: Option<CertTrustGuard> = if matches!(protocol, Protocol::Browser3) {
            match fetch_cert_der(base_url).await {
                Some(cert_der) => match install_cert_trust(&cert_der, &profile_dir).await {
                    Ok(guard) => {
                        tracing::info!(
                            "browser3: server cert installed as trusted root; \
                                 QUIC/H3 should work"
                        );
                        Some(guard)
                    }
                    Err(e) => {
                        tracing::warn!(
                            "browser3: cert trust installation failed ({e}); \
                                 Chrome may fall back to H2"
                        );
                        None
                    }
                },
                None => {
                    tracing::warn!(
                        "browser3: could not fetch server cert; Chrome may fall back to H2"
                    );
                    None
                }
            }
        } else {
            None
        };

        // 5. Per-protocol Chrome flags.
        //   browser1 → http:// URL; no ALPN → definitively H1.1 (no flags needed)
        //   browser2 → --disable-quic           (force HTTP/2, prevent H3 upgrade)
        //   browser3 → --origin-to-force-quic-on (force QUIC alongside Alt-Svc)
        //   browser  → no extra flags (auto-negotiate; typically H2 on LAN)
        let mut extra_args: Vec<String> = Vec::new();
        match &protocol {
            Protocol::Browser1 => {
                // URL already rewritten to http:// — no extra Chrome flags needed.
            }
            Protocol::Browser2 => {
                extra_args.push("--disable-quic".into());
            }
            Protocol::Browser3 => {
                // Belt-and-suspenders: force QUIC alongside the Alt-Svc warmup.
                // On Chrome 132+ this flag may be a no-op, but the cert trust +
                // Alt-Svc path is the primary mechanism.
                let host = base_url.host_str().unwrap_or("");
                let port = base_url.port_or_known_default().unwrap_or(443);
                extra_args.push(format!("--origin-to-force-quic-on={host}:{port}"));
            }
            _ => {}
        }

        // 6. Root-user wrapper (snap Chrome restriction on Linux).
        #[cfg(unix)]
        let chrome_path = wrap_chrome_for_root(chrome_path);

        // 7. Launch browser.
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

        // Spawn the CDP message handler.
        let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

        // 8. Open a new page.
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

        // 9a. browser3: belt-and-suspenders CDP cert override (page-level).
        //     The cert is now trusted at the network-service level (step 4), so
        //     this is mainly for older Chrome where trust-store propagation is slow.
        if matches!(protocol, Protocol::Browser3) {
            match page
                .execute(SetIgnoreCertificateErrorsParams { ignore: true })
                .await
            {
                Ok(_) => tracing::debug!(
                    "browser3: CDP Security.setIgnoreCertificateErrors(true) applied"
                ),
                Err(e) => {
                    tracing::warn!("browser3: CDP cert-error override failed (non-fatal): {e}")
                }
            }
        }

        // 9b. browser3 warmup navigation to seed the Alt-Svc / QUIC pre-connect cache.
        //
        // Flow (with cert trusted):
        //   1. Warmup GET /health → server responds with Alt-Svc: h3=":PORT" over H2.
        //   2. Chrome stores the hint and starts a background QUIC session.
        //      Because the cert is *actually* trusted (no overridden errors), Chrome
        //      processes the Alt-Svc hint and the QUIC TLS succeeds.
        //   3. 1 s sleep → background QUIC session fully established.
        //   4. Main navigation uses the open QUIC session → H3.
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
            // Give Chrome time to complete the background QUIC handshake.
            tokio::time::sleep(std::time::Duration::from_millis(1_000)).await;
        }

        // 9c. Subscribe to network response events.
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

        // 10. Navigate and wait for load event.
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

        // 11. Extract performance timing via JS.
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

        // 12. Drain network events (500 ms after navigation).
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

        // Sort protocol counts by count descending.
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

    // ── Performance timing helpers ────────────────────────────────────────────

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

    // ── Root-user wrapper ─────────────────────────────────────────────────────

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

    // ── Error helper ──────────────────────────────────────────────────────────

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

    // ── Cert fetch ────────────────────────────────────────────────────────────

    /// Connect to the server via TLS and return the leaf certificate's DER bytes.
    ///
    /// All certificate errors are ignored (custom verifier that accepts anything),
    /// so this works even for self-signed certificates.
    ///
    /// Returns `None` on any TCP/TLS error.
    async fn fetch_cert_der(base_url: &url::Url) -> Option<Vec<u8>> {
        use std::sync::{Arc, Mutex};

        let host = base_url.host_str()?.to_string();
        let port = base_url.port_or_known_default()?;

        // Custom verifier: accept all certs, capture the leaf cert DER bytes.
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
        let server_name = rustls::pki_types::ServerName::try_from(host).ok()?;
        let _tls = connector.connect(server_name, tcp).await.ok()?;

        let cert_der = capturer.0.lock().unwrap().take();
        cert_der
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

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

        #[test]
        fn der_to_pem_has_header_and_footer() {
            let dummy = vec![0u8; 32];
            let pem = der_to_pem(&dummy);
            assert!(
                pem.starts_with("-----BEGIN CERTIFICATE-----\n"),
                "pem={pem}"
            );
            assert!(pem.ends_with("-----END CERTIFICATE-----\n"), "pem={pem}");
        }

        #[test]
        fn compute_cert_sha256_hex_is_64_uppercase_hex() {
            let dummy = vec![0u8; 32];
            let hex = compute_cert_sha256_hex(&dummy);
            assert_eq!(hex.len(), 64, "SHA-256 hex should be 64 chars: {hex}");
            assert!(
                hex.chars().all(|c| c.is_ascii_hexdigit()),
                "all hex digits: {hex}"
            );
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
