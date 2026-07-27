/// Real-browser probe via the Chrome DevTools Protocol (chromiumoxide).
///
/// The probe:
///   1. Locates a Chromium/Chrome binary (`NETWORKER_CHROME_PATH` env var or well-known paths).
///   2. Rewrites the target URL to `/page` on the same host/port (like `pageload*` probes).
///   3. Launches a headless browser, injects a buffered PerformanceObserver
///      (Core Web Vitals: LCP / CLS / FCP / longtasks) before navigation,
///      navigates to the URL, and waits for the load event.
///   4. Extracts `window.performance.timing` via JS for TTFB, DCL, and load-event
///      timings, then collects the buffered vitals after a settle delay.
///   5. Correlates Network.requestWillBeSent / responseReceived / loadingFinished
///      events into a per-request waterfall with real wire bytes (encodedDataLength).
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

    // 3. Linux paths (system + user-local)
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
    // User-local install (no-sudo fallback from installer)
    if let Ok(home) = std::env::var("HOME") {
        let local_paths = [
            format!("{home}/.local/bin/google-chrome"),
            format!("{home}/.local/bin/chromium"),
            format!("{home}/.local/google-chrome/google-chrome"),
        ];
        for path in &local_paths {
            let p = PathBuf::from(path);
            if std::fs::metadata(&p).is_ok() {
                return Some(p);
            }
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

/// Compute per-asset byte size for browser probes.
///
/// Browser probes use `/browser-page?assets=N&bytes=B` where all N assets are
/// the same size B. When presets provide varied sizes, we use the average so
/// that the total page weight matches the preset target.
fn browser_asset_bytes(asset_sizes: &[usize]) -> usize {
    if asset_sizes.is_empty() {
        return 0;
    }
    let total: usize = asset_sizes.iter().sum();
    total / asset_sizes.len()
}

/// Rewrite the base URL to the `/browser-page` endpoint.
///
/// `/browser-page` returns an actual HTML page with `<img>` tags so that Chrome
/// fetches each asset via real network requests and the `load` event fires after
/// all assets have settled.  (`/page` returns a JSON manifest used by the
/// synthetic pageload probes — a real browser would just display it as text.)
///
/// Adds `assets=N&bytes=S` query params derived from the provided asset sizes.
/// Uses average asset size so total page weight matches the preset target.
/// If `asset_sizes` is empty the endpoint uses its own server-side defaults.
pub fn build_page_url(base: &url::Url, asset_sizes: &[usize]) -> String {
    let mut target = base.clone();
    target.set_path("/browser-page");

    if !asset_sizes.is_empty() {
        let n = asset_sizes.len();
        let bytes = browser_asset_bytes(asset_sizes);
        target.set_query(Some(&format!("assets={n}&bytes={bytes}")));
    }

    target.to_string()
}

/// Rewrite the base URL for the `browser1` (forced HTTP/1.1) probe.
///
/// Uses `http://` so there is no TLS ALPN negotiation — Chrome physically
/// cannot use HTTP/2 or HTTP/3 over plain HTTP.
///
/// Port derivation: 8443 → 8080 (endpoint), 8444 → 8081 (nginx),
/// 8445 → 8082 (IIS); 443 / no port → 80 (HTTP default, omitted from URL).
/// Any other explicit port is kept as-is.
pub fn build_browser_http1_url(base: &url::Url, asset_sizes: &[usize]) -> String {
    let mut target = base.clone();
    let _ = target.set_scheme("http");
    // Derive plain HTTP port from the HTTPS port.
    let http_port: Option<u16> = match base.port_or_known_default() {
        Some(8443) => Some(8080), // endpoint
        Some(8444) => Some(8081), // nginx stack
        Some(8445) => Some(8082), // IIS stack
        Some(443) | None => None, // use HTTP default (80, omit from URL)
        Some(p) => Some(p),       // non-standard port: keep as-is
    };
    let _ = target.set_port(http_port);
    target.set_path("/browser-page");
    if !asset_sizes.is_empty() {
        let n = asset_sizes.len();
        let bytes = browser_asset_bytes(asset_sizes);
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
pub fn build_browser_http3_url(base: &url::Url, asset_sizes: &[usize]) -> String {
    let mut target = base.clone();
    // Keep https:// scheme and port; just swap the host to localhost.
    let _ = target.set_host(Some("localhost"));
    target.set_path("/browser-page");
    if !asset_sizes.is_empty() {
        let n = asset_sizes.len();
        let bytes = browser_asset_bytes(asset_sizes);
        target.set_query(Some(&format!("assets={n}&bytes={bytes}")));
    }
    target.to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// Core Web Vitals helpers (pure — compiled and tested without the feature)
// ─────────────────────────────────────────────────────────────────────────────

/// Init script injected via `Page.addScriptToEvaluateOnNewDocument` BEFORE
/// navigation, so buffered `PerformanceObserver`s exist from document start
/// (avoids the observer/lifecycle race).
///
/// Definitions implemented here:
///   * LCP  — last `largest-contentful-paint` entry's startTime (candidates
///     are re-emitted as larger elements paint; the last one wins).
///   * FCP  — the `paint` entry named `first-contentful-paint`.
///   * CLS  — session-window rule (web.dev/articles/cls): layout-shift
///     entries without `hadRecentInput` are grouped into sessions; a NEW
///     session starts when the gap since the previous entry is ≥ 1 s or the
///     session would span > 5 s; CLS = max session value (NOT the naive sum
///     of all shifts). `cls` is initialised to 0 as soon as the observer
///     registers — zero observed shifts is a real 0.0, distinct from `null`
///     (observer unavailable).
///   * long tasks — raw (start, duration) pairs; TBT is computed on the Rust
///     side (see [`parse_vitals`]) so the FCP cutoff can be applied.
pub const VITALS_INIT_JS: &str = r#"
(() => {
  const v = {
    lcp: null,
    cls: null,
    fcp: null,
    longTasks: [],
    longTasksSupported: false,
    errors: [],
  };
  window.__networkerVitals = v;
  const observe = (type, cb) => {
    try {
      const po = new PerformanceObserver((list) => {
        try { cb(list.getEntries()); } catch (e) { v.errors.push(type + ': ' + e); }
      });
      po.observe({ type: type, buffered: true });
      return true;
    } catch (e) {
      v.errors.push(type + ': ' + e);
      return false;
    }
  };
  observe('largest-contentful-paint', (entries) => {
    for (const e of entries) { v.lcp = e.startTime; }
  });
  observe('paint', (entries) => {
    for (const e of entries) {
      if (e.name === 'first-contentful-paint' && v.fcp === null) { v.fcp = e.startTime; }
    }
  });
  let sVal = 0, sFirst = 0, sLast = 0, clsMax = 0;
  const clsOk = observe('layout-shift', (entries) => {
    for (const e of entries) {
      if (e.hadRecentInput) { continue; }
      if (sVal > 0 && e.startTime - sLast < 1000 && e.startTime - sFirst < 5000) {
        sVal += e.value;
      } else {
        sVal = e.value;
        sFirst = e.startTime;
      }
      sLast = e.startTime;
      if (sVal > clsMax) { clsMax = sVal; }
      v.cls = clsMax;
    }
  });
  if (clsOk) { v.cls = 0; }
  v.longTasksSupported = observe('longtask', (entries) => {
    for (const e of entries) { v.longTasks.push({ start: e.startTime, dur: e.duration }); }
  });
})();
"#;

/// Collector evaluated AFTER the load event + settle delay: returns the
/// buffered vitals as a JSON string ("" when the init script never ran).
pub const VITALS_COLLECT_JS: &str = r#"
(() => {
  const v = window.__networkerVitals;
  return v ? JSON.stringify(v) : "";
})()
"#;

/// Core Web Vitals parsed from the collector's JSON.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ParsedVitals {
    pub lcp_ms: Option<f64>,
    pub cls: Option<f64>,
    pub fcp_ms: Option<f64>,
    pub tbt_ms: Option<f64>,
}

/// Parse the `window.__networkerVitals` JSON and derive TBT.
///
/// TBT (lab definition used here): Σ max(0, duration − 50 ms) over longtask
/// entries whose startTime ≥ FCP (when FCP is known; otherwise all tasks),
/// within the collection window (navigation start → load event + settle
/// delay — a TTI proxy: headless lab pages receive no input, so the classic
/// FCP→TTI window is approximated by the load window).
///
/// Every metric is `None` — never 0-as-missing — when the page produced no
/// entries / the observer failed; CLS and TBT are `Some(0.0)` when their
/// observers registered but observed nothing (a real measurement of zero).
pub fn parse_vitals(json: &str) -> ParsedVitals {
    if json.is_empty() {
        return ParsedVitals::default();
    }
    let v: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return ParsedVitals::default(),
    };
    let fcp = v["fcp"].as_f64();
    let tbt = if v["longTasksSupported"].as_bool() == Some(true) {
        let mut sum = 0.0_f64;
        if let Some(tasks) = v["longTasks"].as_array() {
            for t in tasks {
                let start = t["start"].as_f64().unwrap_or(0.0);
                let dur = t["dur"].as_f64().unwrap_or(0.0);
                if fcp.is_none_or(|f| start >= f) {
                    sum += (dur - 50.0).max(0.0);
                }
            }
        }
        Some(sum)
    } else {
        None
    };
    ParsedVitals {
        lcp_ms: v["lcp"].as_f64(),
        cls: v["cls"].as_f64(),
        fcp_ms: fcp,
        tbt_ms: tbt,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Waterfall helpers (pure — compiled and tested without the feature)
// ─────────────────────────────────────────────────────────────────────────────

/// Truncate a URL to at most 160 chars (`…` suffix), on a char boundary.
pub fn truncate_url(url: &str) -> String {
    const MAX: usize = 160;
    if url.chars().count() <= MAX {
        return url.to_string();
    }
    let mut s: String = url.chars().take(MAX - 1).collect();
    s.push('…');
    s
}

/// Duration of one ResourceTiming phase. CDP reports −1 for phases that did
/// not occur (e.g. no DNS on a reused connection) → `None`.
pub fn timing_phase_ms(start: f64, end: f64) -> Option<f64> {
    if start >= 0.0 && end >= 0.0 && end >= start {
        Some(end - start)
    } else {
        None
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Real implementation (feature = "browser")
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "browser")]
mod real {
    use super::{
        build_browser_http1_url, build_page_url, find_chrome, parse_vitals, timing_phase_ms,
        truncate_url, VITALS_COLLECT_JS, VITALS_INIT_JS,
    };
    use crate::metrics::{
        BrowserRequest, BrowserRequestTiming, BrowserResult, ErrorCategory, ErrorRecord, Protocol,
        RequestAttempt, BROWSER_WATERFALL_CAP,
    };
    use chromiumoxide::browser::{Browser, BrowserConfig};
    use chromiumoxide::cdp::browser_protocol::network::{
        EventDataReceived, EventLoadingFinished, EventRequestWillBeSent, EventResponseReceived,
    };
    use chromiumoxide::cdp::browser_protocol::security::SetIgnoreCertificateErrorsParams;
    use chrono::Utc;
    use futures::StreamExt;
    use std::collections::HashMap;

    use std::time::Instant;
    use uuid::Uuid;

    // ── Cert helpers ──────────────────────────────────────────────────────────

    /// SHA-256 hash of the certificate's SubjectPublicKeyInfo (SPKI) DER bytes,
    /// Base64-encoded.
    ///
    /// This is the format expected by Chrome's `--ignore-certificate-errors-spki-list`
    /// flag.  Unlike `--ignore-certificate-errors`, the SPKI list is **honored by
    /// Chrome's QUIC cert verifier**, making it the only reliable cross-platform way
    /// to accept self-signed certs over QUIC/H3.
    ///
    /// The hash is of the raw DER bytes of the SubjectPublicKeyInfo field
    /// (the SEQUENCE containing AlgorithmIdentifier + BIT STRING key), not of the
    /// full certificate — matching Chrome's internal computation.
    fn compute_spki_sha256_base64(cert_der: &[u8]) -> Option<String> {
        use base64::Engine as _;
        use sha2::{Digest, Sha256};
        use x509_parser::prelude::*;

        let (_, cert) = X509Certificate::from_der(cert_der).ok()?;
        // `raw` is the DER-encoded SubjectPublicKeyInfo SEQUENCE bytes.
        let spki_der: &[u8] = cert.public_key().raw;
        let hash = Sha256::digest(spki_der);
        Some(base64::engine::general_purpose::STANDARD.encode(hash))
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
            build_browser_http1_url(base_url, asset_sizes)
        } else {
            build_page_url(base_url, asset_sizes)
        };
        tracing::debug!(url = %page_url, "Browser probe target URL");

        // 3. Per-run user-data dir.
        //
        // Each run gets an isolated profile directory so that there is no state
        // leakage between runs (cached connections, HSTS, QUIC session tickets, etc.).
        // The directory is created before Chrome launches and cleaned up by
        // ProfileDirGuard on drop.
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

        // 4. browser3: compute the SPKI hash of the server certificate.
        //
        // Chrome's `--ignore-certificate-errors-spki-list` flag IS honored by
        // Chrome's QUIC TLS cert verifier (unlike `--ignore-certificate-errors`).
        // When the SPKI hash is provided, Chrome accepts the self-signed cert over
        // QUIC/H3 without triggering cert errors, which also means:
        //   (a) Alt-Svc hints are processed normally (not silently discarded).
        //   (b) Background QUIC sessions are established correctly.
        //
        // This is the only reliable cross-platform approach — platform cert stores
        // (NSS db on Linux, Keychain on macOS, Windows Root) are unreliable for
        // Chrome's QUIC TLS path because Chrome 127+ uses its own Root Store by
        // default and the QUIC verifier ignores `--ignore-certificate-errors`.
        let spki_hash: Option<String> = if matches!(protocol, Protocol::Browser3) {
            match fetch_cert_der(base_url).await {
                Some(cert_der) => {
                    let hash = compute_spki_sha256_base64(&cert_der);
                    if hash.is_some() {
                        tracing::info!("browser3: SPKI hash computed; QUIC cert pinning active");
                    } else {
                        tracing::warn!(
                            "browser3: could not compute SPKI hash; Chrome may fall back to H2"
                        );
                    }
                    hash
                }
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
                // NOTE: chromiumoxide arg() converts &str to Arg by treating the
                // whole string as the key (WITHOUT "--" prefix).  chromiumoxide then
                // prepends "--" when building the command line.  Never pass "--flag"
                // with the dashes — that produces "----flag" which Chrome ignores.
                extra_args.push("disable-quic".into());
            }
            Protocol::Browser3 => {
                // Provide the SPKI hash so Chrome's QUIC TLS verifier accepts the
                // self-signed cert, then force QUIC via --origin-to-force-quic-on
                // so Chrome uses QUIC even before Alt-Svc is cached.
                //
                // `--ignore-certificate-errors-spki-list` IS honored by Chrome's
                // QUIC cert verifier (unlike `--ignore-certificate-errors`).
                //
                // Only add these flags when the SPKI hash is available.  Without it,
                // QUIC would fail with ERR_QUIC_PROTOCOL_ERROR instead of gracefully
                // falling back to H2.
                if let Some(hash) = &spki_hash {
                    let host = base_url.host_str().unwrap_or("");
                    let port = base_url.port_or_known_default().unwrap_or(443);
                    // Format "key=value" without "--" prefix; chromiumoxide prepends "--".
                    extra_args.push(format!("ignore-certificate-errors-spki-list={hash}"));
                    extra_args.push(format!("origin-to-force-quic-on={host}:{port}"));
                }
            }
            _ => {}
        }

        // 6. Root-user wrapper (snap Chrome restriction on Linux).
        #[cfg(unix)]
        let chrome_path = wrap_chrome_for_root(chrome_path);

        // 7. Launch browser.
        //
        // For browser3 with SPKI pinning: override chromiumoxide's DEFAULT_ARGS to
        // enable background networking (required for QUIC Alt-Svc probes).
        //
        // Chromiumoxide's DEFAULT_ARGS include `--disable-background-networking`
        // which disables ALL background network activity including QUIC speculative
        // pre-connections and Alt-Svc background probe sessions.  This was the root
        // cause of browser3 always showing H2 — the Alt-Svc warmup received
        // `h3=":PORT"` but Chrome never scheduled the background QUIC session.
        //
        // For browser1/browser2/browser the defaults are fine: those modes use
        // `--ignore-certificate-errors` which already disables QUIC anyway.
        // IMPORTANT: chromiumoxide's BrowserConfig::arg() converts &str to Arg by
        // using the ENTIRE string as the HashMap key (without "--" prefix).  The
        // launcher then prepends "--" to form the final flag.  Passing a string that
        // already begins with "--" produces "----flag" which Chrome silently ignores.
        //
        // Correct usage:
        //   config_builder.arg("disable-gpu")          → passes --disable-gpu       ✓
        //   config_builder.arg("headless=new")         → passes --headless=new      ✓
        //   config_builder.arg("--disable-gpu")        → passes ----disable-gpu     ✗
        //
        // For headless/sandbox, use chromiumoxide's typed methods instead of raw
        // arg strings — they call Arg::key() internally with the correct bare name.

        let cert_trusted = spki_hash.is_some();
        let needs_quic_networking = matches!(protocol, Protocol::Browser3) && cert_trusted;

        let mut config_builder = BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(&profile_dir)
            .no_sandbox() // --no-sandbox --disable-setuid-sandbox
            .new_headless_mode(); // --headless=new --hide-scrollbars --mute-audio

        if needs_quic_networking {
            // Disable chromiumoxide's DEFAULT_ARGS so we can exclude the two that
            // break QUIC for browser3:
            //   --disable-background-networking  → blocks QUIC Alt-Svc probes
            //   --use-mock-keychain              → prevents macOS Keychain cert trust
            // All other DEFAULT_ARGS are re-added manually below.
            config_builder = config_builder
                .disable_default_args()
                // Re-add all DEFAULT_ARGS except --disable-background-networking
                // and --use-mock-keychain.  Note: bare names, no "--" prefix.
                .arg("enable-features=NetworkService,NetworkServiceInProcess")
                .arg("disable-background-timer-throttling")
                .arg("disable-backgrounding-occluded-windows")
                .arg("disable-breakpad")
                .arg("disable-client-side-phishing-detection")
                .arg("disable-component-extensions-with-background-pages")
                .arg("disable-default-apps")
                .arg("disable-dev-shm-usage")
                .arg("disable-features=TranslateUI")
                .arg("disable-hang-monitor")
                .arg("disable-ipc-flooding-protection")
                .arg("disable-popup-blocking")
                .arg("disable-prompt-on-repost")
                .arg("disable-renderer-backgrounding")
                .arg("disable-sync")
                .arg("force-color-profile=srgb")
                .arg("metrics-recording-only")
                .arg("no-first-run")
                .arg("enable-automation") // required: enables Chrome DevTools Protocol
                .arg("password-store=basic")
                .arg("enable-blink-features=IdleDetection")
                .arg("lang=en_US");
        }

        // --disable-gpu is not in DEFAULT_ARGS or chromiumoxide's launch(); add explicitly.
        config_builder = config_builder.arg("disable-gpu");

        if !cert_trusted {
            // When no SPKI hash is available (browser1/2/browser modes, or browser3
            // where the server cert could not be fetched), fall back to ignoring cert
            // errors globally so the probe can still navigate.
            // Note: for browser3 with SPKI hash, --ignore-certificate-errors-spki-list
            // is already in extra_args and no blanket override is needed.
            config_builder = config_builder.arg("ignore-certificate-errors");
        }
        for arg in &extra_args {
            // extra_args entries are already formatted without "--" prefix.
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

        let (mut browser, mut handler) = match tokio::time::timeout(
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

        // 8b. Inject the Core Web Vitals observer script BEFORE any navigation
        // (CDP Page.addScriptToEvaluateOnNewDocument) so buffered
        // PerformanceObservers exist from document start. Runs on every new
        // document — for browser3 the warmup document's values are discarded
        // because the main navigation replaces the window. Non-fatal on
        // failure: vitals simply report None.
        let vitals_injected = match page.evaluate_on_new_document(VITALS_INIT_JS).await {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!("CWV init script injection failed (vitals will be None): {e}");
                false
            }
        };

        // 9a. browser3: CDP cert override — only when SPKI pinning is unavailable.
        //
        // `Security.setIgnoreCertificateErrors(true)` is equivalent to
        // `--ignore-certificate-errors` at the page level: it causes Chrome to
        // discard Alt-Svc hints from connections with overridden cert errors,
        // preventing QUIC from being scheduled.  When SPKI pinning is active
        // (cert_trusted=true), Chrome sees clean cert connections → Alt-Svc is
        // processed normally → QUIC/H3 succeeds.  Do NOT set this override then.
        if matches!(protocol, Protocol::Browser3) && !cert_trusted {
            match page
                .execute(SetIgnoreCertificateErrorsParams { ignore: true })
                .await
            {
                Ok(_) => tracing::debug!(
                    "browser3: CDP Security.setIgnoreCertificateErrors(true) applied (fallback)"
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

        // 9c. Subscribe to the CDP Network event stream.
        // Subscribed AFTER the warmup so only the main navigation's resources
        // are counted. Three listeners, correlated by requestId:
        //   * RequestWillBeSent → url/method/start time (waterfall skeleton)
        //   * ResponseReceived  → status/mime/protocol/cache flags/ResourceTiming,
        //     plus the legacy declared `content-length` sum (`transferred_bytes`)
        //   * LoadingFinished   → `encodedDataLength` = REAL wire bytes
        //     (headers + compressed body as transferred) + end time
        let mut request_events = match page.event_listener::<EventRequestWillBeSent>().await {
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
        let mut finished_events = match page.event_listener::<EventLoadingFinished>().await {
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
        let mut data_events = match page.event_listener::<EventDataReceived>().await {
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

        // 12. Drain the Network event streams (500 ms after navigation — this
        // doubles as the CWV settle delay before the collector runs).
        //
        // Collects resource count, per-protocol breakdown, the per-request
        // waterfall, and two byte figures:
        //   * `transferred_bytes` — sum of declared `content-length` response
        //     headers (kept as-is, Wave-T honest label: NOT wire bytes — it
        //     excludes headers, counts 0 for chunked responses, reflects the
        //     declared possibly-compressed size).
        //   * `wire_bytes_total` — real wire bytes: per request,
        //     max(`loadingFinished.encodedDataLength`, Σ `dataReceived`
        //     chunk `encodedDataLength`). The dataReceived accumulation
        //     matters because `loadingFinished.encodedDataLength` was
        //     observed to under-report on fast multiplexed (H2) transfers —
        //     it can reflect only the bytes accounted at finish time.
        //
        // NOTE on the drain loop shape: `tokio::select!` polls branches in
        // RANDOM order, so events for one request can be processed out of
        // arrival order across the four streams (e.g. responseReceived before
        // requestWillBeSent). All branches therefore go through `entry_for`,
        // which creates the map entry and records insertion order exactly
        // once per requestId.
        //
        // All events should already be queued when we reach this point because the
        // page load event guarantees all resources are complete before
        // wait_for_navigation() returns.
        let mut resource_count: u32 = 0;
        let mut transferred_bytes: usize = 0;
        let mut main_protocol = String::from("unknown");
        let mut first_resource = true;
        let mut protocol_counts: HashMap<String, u32> = HashMap::new();

        /// Per-request correlation state (keyed by CDP requestId).
        #[derive(Default)]
        struct PendingReq {
            url: String,
            method: String,
            /// requestWillBeSent MonotonicTime (seconds).
            start_ts: Option<f64>,
            status: Option<u16>,
            mime_type: Option<String>,
            protocol: Option<String>,
            from_disk_cache: bool,
            from_service_worker: bool,
            timing: Option<chromiumoxide::cdp::browser_protocol::network::ResourceTiming>,
            /// loadingFinished encodedDataLength.
            finished_bytes: Option<u64>,
            /// Σ dataReceived chunk encodedDataLength (wire bytes).
            data_bytes: u64,
            /// loadingFinished MonotonicTime (seconds).
            end_ts: Option<f64>,
        }
        impl PendingReq {
            /// Final wire bytes: the larger of the loadingFinished total and
            /// the dataReceived chunk sum (see drain-loop comment). `None`
            /// only when neither event carried any byte accounting.
            fn wire_bytes(&self) -> Option<u64> {
                match self.finished_bytes {
                    Some(f) => Some(f.max(self.data_bytes)),
                    None if self.data_bytes > 0 => Some(self.data_bytes),
                    None => None,
                }
            }
        }
        let mut pending: HashMap<String, PendingReq> = HashMap::new();
        let mut request_order: Vec<String> = Vec::new();
        /// Get-or-create the correlation entry, recording first-seen order
        /// exactly once per requestId (dedup across the four streams).
        fn entry_for<'m>(
            pending: &'m mut HashMap<String, PendingReq>,
            order: &mut Vec<String>,
            id: &str,
        ) -> &'m mut PendingReq {
            pending.entry(id.to_string()).or_insert_with(|| {
                order.push(id.to_string());
                PendingReq::default()
            })
        }
        // Time origin for the waterfall: the main document request's
        // requestWillBeSent timestamp (first request seen after subscribing).
        let mut nav_origin_ts: Option<f64> = None;

        let drain_deadline = tokio::time::sleep(std::time::Duration::from_millis(500));
        tokio::pin!(drain_deadline);

        loop {
            tokio::select! {
                event = request_events.next() => {
                    match event {
                        Some(evt) => {
                            let ts = *evt.timestamp.inner();
                            nav_origin_ts.get_or_insert(ts);
                            let id = evt.request_id.inner().clone();
                            let entry = entry_for(&mut pending, &mut request_order, &id);
                            entry.url = truncate_url(&evt.request.url);
                            entry.method = evt.request.method.clone();
                            entry.start_ts = Some(ts);
                        }
                        None => break,
                    }
                }
                event = response_events.next() => {
                    match event {
                        Some(evt) => {
                            resource_count += 1;
                            let proto = evt.response.protocol
                                .as_deref()
                                .unwrap_or("unknown")
                                .to_lowercase();
                            if first_resource {
                                main_protocol = proto.clone();
                                first_resource = false;
                            }
                            *protocol_counts.entry(proto.clone()).or_insert(0) += 1;
                            // Sum content-length headers for accurate byte accounting.
                            // Headers exposes inner() → &serde_json::Value.
                            // Header names are lower-case for H2/H3 but may be
                            // title-case for H1.1.
                            let h = evt.response.headers.inner();
                            let cl: usize = h["content-length"].as_str()
                                .or_else(|| h["Content-Length"].as_str())
                                .and_then(|s| s.parse::<usize>().ok())
                                .unwrap_or(0);
                            transferred_bytes += cl;
                            // Waterfall detail.
                            let id = evt.request_id.inner().clone();
                            let entry = entry_for(&mut pending, &mut request_order, &id);
                            if entry.url.is_empty() {
                                // requestWillBeSent not processed yet (random
                                // select! order) — take the URL from the response.
                                entry.url = truncate_url(&evt.response.url);
                            }
                            entry.status = u16::try_from(evt.response.status).ok();
                            let mime = evt.response.mime_type
                                .split(';').next().unwrap_or_default().trim();
                            if !mime.is_empty() {
                                entry.mime_type = Some(mime.to_string());
                            }
                            entry.protocol = Some(proto);
                            entry.from_disk_cache =
                                evt.response.from_disk_cache.unwrap_or(false);
                            entry.from_service_worker =
                                evt.response.from_service_worker.unwrap_or(false);
                            entry.timing = evt.response.timing.clone();
                        }
                        None => break,
                    }
                }
                event = data_events.next() => {
                    match event {
                        Some(evt) => {
                            let chunk = evt.encoded_data_length.max(0) as u64;
                            let id = evt.request_id.inner().clone();
                            let entry = entry_for(&mut pending, &mut request_order, &id);
                            entry.data_bytes += chunk;
                        }
                        None => break,
                    }
                }
                event = finished_events.next() => {
                    match event {
                        Some(evt) => {
                            let bytes = evt.encoded_data_length.max(0.0) as u64;
                            let id = evt.request_id.inner().clone();
                            let entry = entry_for(&mut pending, &mut request_order, &id);
                            entry.finished_bytes = Some(bytes);
                            entry.end_ts = Some(*evt.timestamp.inner());
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

        // 13. Collect Core Web Vitals (after load event + the 500 ms settle
        // above). None (not 0) when the observer never ran or produced nothing.
        let vitals = if vitals_injected {
            match page.evaluate(VITALS_COLLECT_JS).await {
                Ok(v) => {
                    let json: String = v.into_value().unwrap_or_default();
                    tracing::debug!(raw = %json, "CWV collector payload");
                    parse_vitals(&json)
                }
                Err(e) => {
                    tracing::warn!("CWV collector failed (vitals will be None): {e}");
                    super::ParsedVitals::default()
                }
            }
        } else {
            super::ParsedVitals::default()
        };

        // 14. Build the waterfall (insertion order, capped).
        let waterfall_truncated = request_order.len() > BROWSER_WATERFALL_CAP;
        let waterfall: Vec<BrowserRequest> = request_order
            .iter()
            .take(BROWSER_WATERFALL_CAP)
            .filter_map(|id| pending.get(id))
            .map(|p| {
                let rel_ms = |ts: Option<f64>| -> Option<f64> {
                    match (ts, nav_origin_ts) {
                        (Some(t), Some(t0)) => Some(((t - t0) * 1000.0).max(0.0)),
                        _ => None,
                    }
                };
                let timing = p.timing.as_ref().map(|t| {
                    // Content download: receiveHeadersEnd (ms relative to
                    // requestTime, same monotonic base as event timestamps)
                    // → loadingFinished.
                    let receive_ms = match (p.end_ts, t.receive_headers_end) {
                        (Some(end), rhe) if rhe >= 0.0 && t.request_time > 0.0 => {
                            Some(((end - (t.request_time + rhe / 1000.0)) * 1000.0).max(0.0))
                        }
                        _ => None,
                    };
                    BrowserRequestTiming {
                        dns_ms: timing_phase_ms(t.dns_start, t.dns_end),
                        connect_ms: timing_phase_ms(t.connect_start, t.connect_end),
                        ssl_ms: timing_phase_ms(t.ssl_start, t.ssl_end),
                        send_ms: timing_phase_ms(t.send_start, t.send_end),
                        wait_ms: timing_phase_ms(t.send_end, t.receive_headers_end),
                        receive_ms,
                    }
                });
                BrowserRequest {
                    url: p.url.clone(),
                    method: if p.method.is_empty() {
                        "GET".to_string()
                    } else {
                        p.method.clone()
                    },
                    status: p.status,
                    mime_type: p.mime_type.clone(),
                    protocol: p.protocol.clone(),
                    wire_bytes: p.wire_bytes(),
                    start_ms: rel_ms(p.start_ts),
                    end_ms: rel_ms(p.end_ts),
                    from_disk_cache: p.from_disk_cache,
                    from_service_worker: p.from_service_worker,
                    timing,
                }
            })
            .collect();
        // Total wire bytes across ALL requests (including any beyond the
        // waterfall cap). None — not 0 — when no request carried byte
        // accounting (e.g. the event streams produced nothing).
        let wire_bytes_total = {
            let mut sum: u64 = 0;
            let mut any = false;
            for p in pending.values() {
                if let Some(w) = p.wire_bytes() {
                    sum += w;
                    any = true;
                }
            }
            any.then_some(sum)
        };

        // Close the browser gracefully before aborting the handler so chromiumoxide
        // does not warn "Browser was not closed manually".
        let _ = browser.close().await;
        handler_task.abort();
        let finished_at = Utc::now();

        RequestAttempt {
            phase: None,
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
                lcp_ms: vitals.lcp_ms,
                cls: vitals.cls,
                fcp_ms: vitals.fcp_ms,
                tbt_ms: vitals.tbt_ms,
                wire_bytes_total,
                waterfall,
                waterfall_truncated,
            }),
            http_stack: None,
            rpm: None,
            ping: None,
            path: None,
            dualstack: None,
            websocket: None,
            pmtud: None,
            responsiveness: None,
            stamp: None,
            mthroughput: None,
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
            phase: None,
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
            http_stack: None,
            rpm: None,
            ping: None,
            path: None,
            dualstack: None,
            websocket: None,
            pmtud: None,
            responsiveness: None,
            stamp: None,
            mthroughput: None,
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
        fn parse_perf_timing_malformed_json() {
            let (load, dcl, ttfb) = parse_perf_timing("{invalid json");
            assert_eq!(load, 0.0);
            assert_eq!(dcl, 0.0);
            assert_eq!(ttfb, 0.0);
        }

        #[test]
        fn parse_perf_timing_missing_fields() {
            // Only navigationStart present — other fields default to 0.0.
            let json = r#"{"navigationStart":1000}"#;
            let (load, dcl, ttfb) = parse_perf_timing(json);
            // load = (0.0 - 1000.0).max(0.0) = 0.0
            assert_eq!(load, 0.0);
            assert_eq!(dcl, 0.0);
            assert_eq!(ttfb, 0.0);
        }

        #[test]
        fn parse_perf_timing_zero_navigation_start() {
            let json = r#"{"navigationStart":0,"responseStart":50,"loadEventEnd":500}"#;
            let (load, dcl, ttfb) = parse_perf_timing(json);
            // navigationStart == 0 → early return (0, 0, 0)
            assert_eq!(load, 0.0);
            assert_eq!(dcl, 0.0);
            assert_eq!(ttfb, 0.0);
        }

        #[test]
        fn parse_perf_timing_negative_timings_clamped() {
            // loadEventEnd < navigationStart → negative → clamped to 0.0
            let json = r#"{"navigationStart":2000,"responseStart":1000,"domContentLoadedEventEnd":1500,"loadEventEnd":1800}"#;
            let (load, dcl, ttfb) = parse_perf_timing(json);
            assert_eq!(load, 0.0); // 1800 - 2000 = -200 → 0.0
            assert_eq!(dcl, 0.0); // 1500 - 2000 = -500 → 0.0
            assert_eq!(ttfb, 0.0); // 1000 - 2000 = -1000 → 0.0
        }

        #[test]
        fn parse_perf_timing_string_values_ignored() {
            // String values instead of numbers → as_f64() returns None → 0.0.
            let json = r#"{"navigationStart":"1000","responseStart":"1050"}"#;
            let (load, dcl, ttfb) = parse_perf_timing(json);
            // navigationStart parses as 0.0 → early return
            assert_eq!(load, 0.0);
            assert_eq!(dcl, 0.0);
            assert_eq!(ttfb, 0.0);
        }

        #[test]
        fn parse_perf_timing_null_values() {
            let json = r#"{"navigationStart":null,"responseStart":null}"#;
            let (load, dcl, ttfb) = parse_perf_timing(json);
            assert_eq!(load, 0.0);
            assert_eq!(dcl, 0.0);
            assert_eq!(ttfb, 0.0);
        }

        #[test]
        fn compute_spki_sha256_base64_rejects_invalid_der() {
            // Non-DER bytes should return None without panicking.
            let result = compute_spki_sha256_base64(&[0u8; 32]);
            assert!(result.is_none(), "invalid DER should return None");
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
            phase: None,
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
            http_stack: None,
            rpm: None,
            ping: None,
            path: None,
            dualstack: None,
            websocket: None,
            pmtud: None,
            responsiveness: None,
            stamp: None,
            mthroughput: None,
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
    use std::sync::{Mutex, OnceLock};

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

    fn with_chrome_env_lock<T>(f: impl FnOnce() -> T) -> T {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("chrome env lock poisoned");
        f()
    }

    #[test]
    fn find_chrome_env_var_nonexistent_path_is_skipped() {
        with_chrome_env_lock(|| {
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
        });
    }

    #[test]
    fn find_chrome_env_var_existing_file_is_returned() {
        with_chrome_env_lock(|| {
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
        });
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

    // ── build_browser_http3_url tests ──────────────────────────────────────────────

    #[test]
    fn build_browser_http3_url_rewrites_host_to_localhost() {
        let base = url::Url::parse("https://172.16.32.106:8443/health").unwrap();
        let url = build_browser_http3_url(&base, &[]);
        assert!(
            url.starts_with("https://localhost:8443/browser-page"),
            "url={url}"
        );
    }

    #[test]
    fn build_browser_http3_url_keeps_https_and_port() {
        let base = url::Url::parse("https://192.168.1.1:9443/some/path").unwrap();
        let url = build_browser_http3_url(&base, &[]);
        assert!(
            url.starts_with("https://localhost:9443/browser-page"),
            "url={url}"
        );
    }

    #[test]
    fn build_browser_http3_url_includes_asset_params() {
        let base = url::Url::parse("https://10.0.0.1:8443/health").unwrap();
        let url = build_browser_http3_url(&base, &[8192, 8192]);
        assert!(url.contains("assets=2"), "url={url}");
        assert!(url.contains("bytes=8192"), "url={url}");
    }

    #[test]
    fn build_browser_http3_url_no_query_when_no_assets() {
        let base = url::Url::parse("https://10.0.0.1:8443/health").unwrap();
        let url = build_browser_http3_url(&base, &[]);
        assert!(!url.contains("assets="), "url={url}");
        assert!(!url.contains("bytes="), "url={url}");
    }

    // ── build_browser_http1_url tests ──────────────────────────────────────────────

    #[test]
    fn build_browser_http1_url_switches_to_http_and_maps_8443() {
        let base = url::Url::parse("https://host:8443/health").unwrap();
        let url = build_browser_http1_url(&base, &[]);
        assert!(
            url.starts_with("http://host:8080/browser-page"),
            "url={url}"
        );
    }

    #[test]
    fn build_browser_http1_url_standard_https_port_omits_port() {
        let base = url::Url::parse("https://example.com/health").unwrap();
        let url = build_browser_http1_url(&base, &[]);
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
    fn build_browser_http1_url_nginx_8444_maps_to_8081() {
        let base = url::Url::parse("https://host:8444/health").unwrap();
        let url = build_browser_http1_url(&base, &[]);
        assert!(
            url.starts_with("http://host:8081/browser-page"),
            "url={url}"
        );
    }

    #[test]
    fn build_browser_http1_url_iis_8445_maps_to_8082() {
        let base = url::Url::parse("https://host:8445/health").unwrap();
        let url = build_browser_http1_url(&base, &[]);
        assert!(
            url.starts_with("http://host:8082/browser-page"),
            "url={url}"
        );
    }

    #[test]
    fn build_browser_http1_url_non_standard_port_preserved() {
        let base = url::Url::parse("https://host:9443/health").unwrap();
        let url = build_browser_http1_url(&base, &[]);
        assert!(
            url.starts_with("http://host:9443/browser-page"),
            "url={url}"
        );
    }

    #[test]
    fn build_browser_http1_url_includes_asset_params() {
        let base = url::Url::parse("https://host:8443/health").unwrap();
        let url = build_browser_http1_url(&base, &[4096, 4096, 4096]);
        assert!(url.contains("assets=3"), "url={url}");
        assert!(url.contains("bytes=4096"), "url={url}");
    }

    // ── browser_asset_bytes tests ────────────────────────────────────────────

    #[test]
    fn browser_asset_bytes_uniform() {
        assert_eq!(
            super::browser_asset_bytes(&[10_240, 10_240, 10_240]),
            10_240
        );
    }

    #[test]
    fn browser_asset_bytes_varied_uses_average() {
        // Simulates a preset with varied sizes — average preserves total weight.
        let sizes = vec![1_024, 5_120, 51_200, 153_600];
        let avg = super::browser_asset_bytes(&sizes);
        let total_original: usize = sizes.iter().sum();
        let total_avg = avg * sizes.len();
        // Average-based total should be close to original (integer division rounding)
        assert!(
            (total_original as i64 - total_avg as i64).unsigned_abs() < sizes.len() as u64,
            "avg={avg}, total_original={total_original}, total_avg={total_avg}"
        );
    }

    #[test]
    fn browser_asset_bytes_empty() {
        assert_eq!(super::browser_asset_bytes(&[]), 0);
    }

    // ── Core Web Vitals parsing tests ────────────────────────────────────────

    #[test]
    fn parse_vitals_empty_and_malformed_yield_all_none() {
        for json in ["", "{not json", "null", "42"] {
            let v = parse_vitals(json);
            assert_eq!(v, ParsedVitals::default(), "input={json:?}");
        }
    }

    #[test]
    fn parse_vitals_full_payload() {
        let json = r#"{
            "lcp": 321.5, "cls": 0.042, "fcp": 120.25,
            "longTasksSupported": true,
            "longTasks": [
                {"start": 100.0, "dur": 80.0},
                {"start": 200.0, "dur": 120.0},
                {"start": 50.0,  "dur": 300.0}
            ]
        }"#;
        let v = parse_vitals(json);
        assert_eq!(v.lcp_ms, Some(321.5));
        assert_eq!(v.cls, Some(0.042));
        assert_eq!(v.fcp_ms, Some(120.25));
        // FCP = 120.25 → the tasks starting at 50 and 100 are excluded
        // (pre-FCP); only the task at 200 counts: TBT = 120 − 50 = 70.
        assert_eq!(v.tbt_ms, Some(70.0));
    }

    #[test]
    fn parse_vitals_tbt_without_fcp_sums_all_tasks() {
        let json = r#"{
            "lcp": null, "cls": null, "fcp": null,
            "longTasksSupported": true,
            "longTasks": [{"start": 10.0, "dur": 90.0}, {"start": 20.0, "dur": 30.0}]
        }"#;
        let v = parse_vitals(json);
        assert_eq!(v.fcp_ms, None);
        // No FCP cutoff → all tasks; sub-50ms task contributes 0.
        assert_eq!(v.tbt_ms, Some(40.0));
    }

    #[test]
    fn parse_vitals_tbt_none_when_longtask_unsupported() {
        let json = r#"{"lcp": 100.0, "cls": 0.0, "fcp": 50.0, "longTasksSupported": false, "longTasks": []}"#;
        let v = parse_vitals(json);
        assert_eq!(v.tbt_ms, None, "unsupported observer must be None, not 0");
        // CLS 0.0 is a real value (observer registered, zero shifts).
        assert_eq!(v.cls, Some(0.0));
    }

    #[test]
    fn parse_vitals_zero_tasks_is_zero_tbt_not_none() {
        let json = r#"{"lcp": 100.0, "cls": 0.0, "fcp": 50.0, "longTasksSupported": true, "longTasks": []}"#;
        let v = parse_vitals(json);
        assert_eq!(v.tbt_ms, Some(0.0), "observer worked, no tasks → real 0");
    }

    #[test]
    fn parse_vitals_null_metrics_stay_none() {
        let json = r#"{"lcp": null, "cls": null, "fcp": null, "longTasksSupported": false}"#;
        let v = parse_vitals(json);
        assert_eq!(v, ParsedVitals::default());
    }

    // ── Waterfall helper tests ───────────────────────────────────────────────

    #[test]
    fn truncate_url_short_unchanged() {
        assert_eq!(truncate_url("https://a/b"), "https://a/b");
    }

    #[test]
    fn truncate_url_long_gets_ellipsis_on_char_boundary() {
        let long: String = "https://example.com/"
            .chars()
            .chain("é".chars().cycle().take(400))
            .collect();
        let t = truncate_url(&long);
        assert_eq!(t.chars().count(), 160);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn timing_phase_ms_maps_negative_sentinel_to_none() {
        assert_eq!(timing_phase_ms(-1.0, 5.0), None);
        assert_eq!(timing_phase_ms(3.0, -1.0), None);
        assert_eq!(timing_phase_ms(5.0, 3.0), None, "end before start");
        assert_eq!(timing_phase_ms(3.0, 5.0), Some(2.0));
        assert_eq!(timing_phase_ms(0.0, 0.0), Some(0.0));
    }

    #[test]
    fn build_page_url_varied_sizes_uses_average() {
        let base = url::Url::parse("https://host:8443/health").unwrap();
        // 4 assets: 1KB, 5KB, 50KB, 150KB → avg = ~51.5KB
        let sizes = vec![1_024, 5_120, 51_200, 153_600];
        let url = build_page_url(&base, &sizes);
        assert!(url.contains("assets=4"), "url={url}");
        let avg = (1_024 + 5_120 + 51_200 + 153_600) / 4;
        assert!(
            url.contains(&format!("bytes={avg}")),
            "expected bytes={avg}, url={url}"
        );
    }
}
