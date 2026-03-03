# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---

## [0.12.55] – 2026-03-03 — fix browser probe: use JS Performance API for transferred_bytes

### Fixed
- **`transferred_bytes` browser1 (HTTP/1.1) vastly under-reported**: `Network.loadingFinished.encodedDataLength`
  in Chrome's CDP is unreliable for HTTP/1.1 connections — values varied from 38 % to 55 % of expected
  bytes across runs (79–113 KiB for 20 × 10 KiB assets, vs the correct ~200 KiB).  H2/H3 were
  consistent (the same multiplexed connection makes the accounting deterministic), but H1.1 uses
  multiple serial connections where Chrome's internal byte counter drifts.
- **Fix**: Replaced the `EventLoadingFinished` CDP subscription with a single JavaScript evaluation
  of `performance.getEntriesByType('resource')` + `getEntriesByType('navigation')[0]`, summing
  `encodedBodySize` for every resource.  This value is computed post-load by the browser, is
  consistent across HTTP/1.1, H2, and H3, and does not depend on CDP event timing or ordering.
  `EventResponseReceived` is still used for resource count and per-protocol breakdown (unchanged).

---

## [0.12.54] – 2026-03-03 — installer: sync SCRIPT_VERSION to 0.12.54

### Changed
- `install.sh` and `install.ps1`: bump `SCRIPT_VERSION` from 0.12.48 → 0.12.54 so the installer
  banner matches the binary version being installed.

---

## [0.12.53] – 2026-03-03 — fix browser probe: use EventLoadingFinished for accurate transferred_bytes

### Fixed
- **`transferred_bytes` in browser probes**: Switched from `EventResponseReceived.response.encoded_data_length`
  to `EventLoadingFinished.encoded_data_length` for byte accounting.
  `Network.responseReceived` fires when headers arrive — its `encodedDataLength` is unreliable for
  multiplexed protocols (H2/H3) because it captures connection-level bytes at an arbitrary point
  mid-transfer, producing bimodal values (e.g. browser2 alternating between ~1 KiB and ~86 KiB
  across runs). `Network.loadingFinished` fires after the full response body is received and gives
  an accurate per-request byte count for all protocols.
  Protocol and resource-count accounting still uses `EventResponseReceived` (unchanged).

---

## [0.12.52] – 2026-03-03 — HTML report: browser comparison table, SVG charts, analysis, favicon fix

### Added
- **Browser Protocol Comparison table**: New aggregated summary table showing browser1/2/3 side-by-side
  (Avg TTFB, DCL, Load, p50, Min, Max, Avg Resources, Avg Bytes) — mirrors the existing pageload comparison table.
- **SVG bar charts** (self-contained, no CDN): Three horizontal bar charts embedded in the HTML report:
  1. "Page Load Time — All Protocols": merged browser + pageload avg load times, slowest→fastest
  2. "Browser TTFB / DCL / Load Breakdown": interleaved bars per browser mode with TTFB/DCL/Load breakdown
  3. "Throughput by Protocol (MB/s)": shown only when download/upload data is present
- **Analysis observations**: Auto-generated bullet list including fastest browser mode, H3 vs H2
  improvement percentage, real-browser vs synthetic overhead, and resource protocol breakdown.

### Fixed
- **Favicon 404**: Added `<link rel="icon" href="data:,">` to `<head>` in both `/browser-page` HTML
  endpoints (HTTP/2 `routes.rs` and HTTP/3 `http3_server.rs`). Chrome no longer auto-requests
  `/favicon.ico`, eliminating 404 noise from endpoint server debug logs.

---

## [0.12.51] – 2026-03-03 — fix browser probe: close browser gracefully to suppress chromiumoxide warning

### Fixed
- **`browser` probe**: Call `browser.close().await` before aborting the CDP handler task.
  Without this, chromiumoxide logs `WARN: Browser was not closed manually, it will be
  killed automatically in the background` on every probe run.

---

## [0.12.50] – 2026-03-03 — fix browser3: SPKI hash pinning replaces unreliable platform cert stores

### Fixed
- **`browser3` QUIC cert trust (cross-platform)**: Replaced the entire platform cert-store
  approach (NSS db on Linux, macOS Keychain, Windows Root store) with
  `--ignore-certificate-errors-spki-list=<spki_hash>`.  Chrome's QUIC TLS cert verifier
  **does** honour this flag (unlike `--ignore-certificate-errors`), making it the only
  reliable way to accept self-signed certs over QUIC/H3 on all platforms.
- **SPKI hash computation**: Added `compute_spki_sha256_base64()` which fetches the server
  cert via a custom-verifier TLS handshake, extracts the DER-encoded SubjectPublicKeyInfo
  bytes via `x509-parser`, and Base64-encodes their SHA-256 hash — exactly the format
  Chrome's SPKI list expects.
- Removed dead code: `der_to_pem`, `CertTrustGuard`, `install_cert_trust`,
  `install_cert_trust_inner` (all platform variants), `which_command`,
  `compute_cert_sha256_hex`.  The new SPKI approach is ~15 lines vs ~290 lines of
  platform-specific cert management.

---

## [0.12.49] – 2026-03-03 — fix browser3 Linux cert trust: import into ~/.pki/nssdb

### Fixed
- **`browser3` Linux cert trust**: `install_cert_trust_inner` was importing the server
  cert into `profile_dir/Default/` (the Chrome profile's data directory).  Chrome does
  **not** read TLS certificates from the profile directory — it reads `~/.pki/nssdb` when
  `--use-system-cert-store` is set.  Changed the NSS db path to `~/.pki/nssdb`; certutil
  creates the directory if absent.  The cert is still removed by `CertTrustGuard::drop`
  on probe completion.

---

## [0.12.48] – 2026-03-03 — installer: certutil shown in plan and confirmed before install

### Changed
- **Installer (`install.sh`)**: `certutil` (NSS tools) now appears in the Installation Plan
  when Chrome is available on Linux and `certutil` is not yet installed.  Shown in both the
  release-download and source-build plan sections.
- **Installer (`install.sh`)**: `step_ensure_certutil` now asks `Y/n` before installing
  (`ask_yn`) instead of silently running `apt-get install`.  Skipping prints a warning that
  browser3 will fall back to H2.
- **Installer (`install.sh`)**: `CERTUTIL_AVAILABLE` flag detected in `discover_system`
  (via `command -v certutil`) so the plan display knows whether to show the step.

---

## [0.12.47] – 2026-03-03 — installer: fix SYS_OS case check in step_ensure_certutil

### Fixed
- **Installer (`install.sh`)**: `step_ensure_certutil` compared `"$SYS_OS" == "linux"`
  (lowercase) but `SYS_OS` is set from `uname -s` which returns `"Linux"` (capital L).
  The guard returned immediately on every Linux system, so certutil was never installed.
  Fixed to `"Linux"`.

---

## [0.12.46] – 2026-03-03 — installer: certutil check runs for release downloads too

### Fixed
- **Installer (`install.sh`)**: `step_ensure_certutil()` was placed inside the `else`
  (source-build) branch of `main()`, so it was never reached when using the default
  release-binary download path.  Moved the call outside the `if/else` so it runs
  regardless of install method, whenever Chrome is available.

---

## [0.12.45] – 2026-03-03 — installer: ensure certutil for existing Chrome installs

### Fixed
- **Installer (`install.sh`)**: `libnss3-tools` / `nss-tools` was only installed inside
  `step_install_chrome()`, which is skipped when Chrome is already present.  Added a new
  `step_ensure_certutil()` function that runs whenever Chrome is available (pre-existing
  **or** freshly installed), checks for `certutil` with `command -v`, and installs the
  appropriate NSS package if missing.  This is a no-op on macOS (uses `security(1)`) and
  when `certutil` is already installed.
- **Installer (`install.ps1`, `install.sh`)**: bump `$ScriptVersion` / `SCRIPT_VERSION` to
  `0.12.45`.

---

## [0.12.44] – 2026-03-03 — installer: libnss3-tools; Windows browser3 cert trust

### Added
- **Installer (`install.sh`)**: automatically installs the NSS `certutil` helper tool
  alongside Chromium for all supported Linux package managers — `libnss3-tools` (apt-get),
  `nss-tools` (dnf/apk), `nss` (pacman), `mozilla-nss-tools` (zypper).  `certutil` is
  required for `browser3` QUIC cert trust on Linux; without it the probe silently falls
  back to H2.
- **`browser3` cert trust on Windows**: uses PowerShell to add the server's self-signed cert
  to `CurrentUser\Root` temporarily (no extra package installation required — Windows
  Certificate Store and PowerShell 5.1+ are built-in).  The cert is removed automatically
  via `CertTrustGuard::drop` after the probe completes.
- **Installer (`install.ps1`)**: bump `$ScriptVersion` to match binary version `0.12.43`
  (was stale at `0.12.38`).

---

## [0.12.43] – 2026-03-03 — browser3: fix ERR_QUIC_PROTOCOL_ERROR when cert trust fails on Linux

### Fixed
- **`browser3` fails with `ERR_QUIC_PROTOCOL_ERROR`** when `certutil` is not installed on Linux
  (or cert trust installation fails for any reason).  Root cause: `--origin-to-force-quic-on`
  was unconditionally added, causing Chrome to attempt QUIC even in the cert-trust-failed
  fallback path.  QUIC TLS does **not** honour `--ignore-certificate-errors`, so the handshake
  fails outright instead of falling back gracefully to H2.
  Fix: gate `--origin-to-force-quic-on` on cert trust success (`_cert_trust_guard.is_some()`).
  When cert trust fails, browser3 falls back to H2 (same as pre-v0.12.41 behaviour) rather
  than erroring.

---

## [0.12.42] – 2026-03-03 — fix dead_code warning on Linux

### Fixed
- Suppressed `dead_code` compiler warning for `compute_cert_sha256_hex` on Linux CI builds.
  The function is macOS-only (used by `security delete-certificate -Z`); gated with
  `#[cfg(target_os = "macos")]`.

---

## [0.12.41] – 2026-03-03 — browser3: fix Chrome arg double-dash bug (root cause of H3 regression)

### Fixed
- **`browser3` always reporting `proto=h2` despite all cert-trust fixes** — definitive root cause
  found: chromiumoxide's `BrowserConfig::arg(&str)` treats the entire string as the arg *key*
  and prepends `"--"` at launch time.  All our calls used `"--flag=value"` format, producing
  `"----flag=value"` which Chrome silently ignores.  Affected flags:
  - `--origin-to-force-quic-on` (the QUIC-forcing flag) — **was always ignored**
  - `--use-system-cert-store` (Chrome Root Store bypass)
  - `--ignore-certificate-errors`
  - `--disable-quic` (browser2)
  - All manually re-added DEFAULT_ARGS in the `disable_default_args()` block
  Fix: strip `--` prefix from all raw arg strings; use chromiumoxide's typed
  `.no_sandbox()` / `.new_headless_mode()` helpers instead of manual arg strings for
  sandbox/headless so chromiumoxide's Arg machinery produces correct `--` flags.
- **Chrome 127+ Chrome Root Store** — even with correct arg format, Chrome 127+ ignores
  `security add-trusted-cert` because it defaults to its own Chrome Root Store (not the
  macOS trust store).  Added `--use-system-cert-store` (now correctly passed) to force Chrome
  to use the platform verifier which reads `login.keychain-db`.

### Result
All three protocols are now consistently correct across 3 runs:
- `browser1` → `http/1.1×22` ✓
- `browser2` → `h2×22` ✓
- `browser3` → `h3×21` ✓ (was always `h2` before this fix)

---

## [0.12.40] – 2026-03-03 — browser3: fix QUIC cert trust (CA:TRUE) and remove --ignore-certificate-errors when cert is trusted

### Fixed
- **`browser3` QUIC TLS still failing with `CERTIFICATE_VERIFY_FAILED`** even after v0.12.39
  installed the server cert in the macOS Keychain.
  Root cause (definitive, from Chrome net-log analysis):
  1. Chrome's cert verifier (BoringSSL) requires trust anchors to have `basicConstraints: CA:TRUE`
     per RFC 5280 §4.2.1.9.  The endpoint used `rcgen::generate_simple_self_signed()` which
     produces a plain leaf cert with **no** `basicConstraints` extension.  Adding a non-CA leaf
     cert to the macOS Keychain as `trustRoot` is silently ignored by Chrome's QUIC verifier —
     it accepted the Keychain entry for display but refused to use the cert as a chain anchor,
     producing `CERTIFICATE_VERIFY_FAILED`.
  2. `--ignore-certificate-errors` was still added to all browser modes including browser3.
     Chrome discards `Alt-Svc` hints from connections with overridden cert errors, so QUIC was
     never scheduled even though cert trust installation logged success.
  Fix:
  - **Endpoint** (`networker-endpoint`): switched cert generation from
    `generate_simple_self_signed()` to `CertificateParams::new()` with
    `is_ca = IsCa::Ca(BasicConstraints::Unconstrained)`.  The server cert now carries
    `basicConstraints: CA:TRUE` and can serve as a proper trust anchor.
  - **Tester** (`networker-tester`, `runner/browser.rs`): `--ignore-certificate-errors` is now
    **omitted** for browser3 when cert trust installation succeeded (`CertTrustGuard` is `Some`).
    Chrome sees no cert errors → `Alt-Svc` hint is processed → QUIC session scheduled →
    main navigation uses H3.  Falls back to adding the flag (H2 only) if cert trust failed.
  - Removed the `--log-net-log` diagnostic flag that was left in browser3 in v0.12.39.

---

## [0.12.39] – 2026-03-03 — browser3: install server cert as trusted root before Chrome launch to enable QUIC/H3

### Fixed
- **`browser3` still shows `proto=h2` with zero QUIC packets** in all previous attempts
  (v0.12.35–v0.12.38).
  Root cause (definitive): Chrome silently discards `Alt-Svc` hints from connections where
  certificate errors were *overridden* via `--ignore-certificate-errors`.  This is a deliberate
  Chrome security policy: upgrading a "broken" (cert-error) connection to QUIC would allow a
  MITM to force protocol negotiation.  Even with the CDP `Security.setIgnoreCertificateErrors`
  command and the SPKI-list flag, Chrome never scheduled a background QUIC probe because the
  initial H2 warmup connection was flagged as having an overridden cert error.
  Fix: **install the server's leaf certificate as a temporarily-trusted root before Chrome
  launches**, so Chrome sees a fully-authenticated connection, processes the `Alt-Svc` hint,
  schedules a background QUIC session, and the main navigation uses H3.
  - **macOS**: `security add-trusted-cert -d -r trustRoot -k login.keychain-db <cert.pem>`
    before Chrome launch; `security delete-certificate -Z <SHA-256>` in RAII `Drop` guard.
  - **Linux**: `certutil -A -d sql:<profile>/Default/ -t CT,, -n <name> -i <cert.pem>` into
    the Chrome user-data-dir NSS database (populated before `Browser::launch` so Chrome reads
    it at startup); cert name uses PID for uniqueness; cleanup in RAII `Drop` guard.
  - Falls back gracefully (logs a warning, browser3 may show H2) if `certutil` is absent
    (`apt install libnss3-tools`), or if the macOS keychain is locked.
- Replaced `fetch_spki_hash` + `--ignore-certificate-errors-spki-list` (did not work in
  `--headless=new` mode) with `fetch_cert_der` used by the cert trust path.
- Re-enabled `--origin-to-force-quic-on=<host>:<port>` as belt-and-suspenders alongside
  the Alt-Svc warmup (cert now trusted, so the forced probe succeeds).
- Increased browser3 post-warmup sleep from 500 ms to 1 s for more reliable QUIC session
  establishment on higher-latency links.
- Added two new unit tests: `der_to_pem_has_header_and_footer`,
  `compute_cert_sha256_hex_is_64_uppercase_hex`.

---

## [0.12.38] – 2026-03-03 — browser3: remove forced-QUIC flags to fix Alt-Svc H3 negotiation

### Fixed
- **`browser3` still shows `proto=h2`** after CDP cert override (v0.12.37).
  Root cause: `--origin-to-force-quic-on=<ip>:<port>` causes Chrome's network stack to probe
  the origin with a raw QUIC connection during initialization — **before** the CDP
  `Security.setIgnoreCertificateErrors(true)` command is applied.  This early probe fails the
  TLS handshake (cert not yet trusted) and Chrome permanently marks the origin as "QUIC broken"
  for the duration of the session.  Subsequent Alt-Svc hints from the warmup navigation are
  then silently discarded for that broken origin.  Confirmed via server logs: Chrome sent zero
  QUIC packets to the server in all v0.12.37 runs.
  Fix: **remove `--origin-to-force-quic-on` and `--enable-quic`** from browser3's Chrome flags.
  QUIC is Chrome's default; without the forced-probe flag, no early QUIC attempt is made.
  H3 is triggered naturally via the server's `Alt-Svc: h3=":PORT"` warmup response: Chrome
  schedules a background QUIC session, the CDP cert override (already active) makes the TLS
  handshake succeed, and the main navigation uses the established QUIC session → H3.

---

## [0.12.37] – 2026-03-03 — browser3: CDP Security.setIgnoreCertificateErrors to fix QUIC cert trust in --headless=new

### Fixed
- **`browser3` failed with `net::ERR_CONNECTION_REFUSED`** (v0.12.36 regression): the localhost
  URL approach was broken because Chrome internally hardcodes `localhost` → `127.0.0.1` and
  silently ignores `--host-resolver-rules` for that hostname.  Reverted to using the actual
  server IP in the URL; removed `--host-resolver-rules`, `--allow-insecure-localhost`.
- **`browser3` root cause identified**: Chrome's `--headless=new` mode (Chrome 112+) does NOT
  apply `--ignore-certificate-errors-spki-list` to QUIC/H3 TLS connections.  The flag is
  handled in the browser process but QUIC TLS runs in the network service process which does
  not receive it in headless mode.  Fix: send `Security.setIgnoreCertificateErrors(true)` via
  CDP immediately after page creation.  The DevTools Protocol command applies to ALL
  connections made by that page, including QUIC, reliably in `--headless=new`.
  `--ignore-certificate-errors-spki-list` is kept as belt-and-suspenders for older Chrome.

### Changed
- browser3: `INFO` log `CDP Security.setIgnoreCertificateErrors(true) applied` for visibility.

---

## [0.12.36] – 2026-03-02 — browser3: localhost URL rewrite + host-resolver-rules to force H3

### Fixed
- **`browser3` still showed `proto=h2`** even after Alt-Svc + warmup navigation (v0.12.35).
  Root cause: Chrome's QUIC TLS path is stricter about SAN matching than the TCP/TLS path.
  When connecting to `172.16.32.106:8443`, Chrome's QUIC stack rejects the handshake if the
  cert has no SAN matching that exact IP — even with `--ignore-certificate-errors-spki-list`
  pinning the cert.  (`--ignore-certificate-errors` alone does not bypass QUIC TLS failures.)
  Fix: browser3 now **rewrites the navigation URL to `https://localhost:<port>`** and adds:
  - `--host-resolver-rules=MAP localhost <actual_ip>` — Chrome routes `localhost` DNS to the
    real server IP, so the connection reaches the endpoint.
  - `--allow-insecure-localhost` — Chrome trusts self-signed certs on localhost without
    requiring a SPKI pin (the cert always has `localhost` as a SAN).
  - `--origin-to-force-quic-on=localhost:<port>` — QUIC is forced on the localhost origin.
  - `--ignore-certificate-errors-spki-list=<hash>` — belt-and-suspenders SPKI pin still
    computed and passed.
  - `fetch_spki_hash()` log level promoted from `DEBUG` to `INFO` for visibility.
  - Warmup sleep increased from 200 ms to 500 ms to give Chrome more time to establish the
    QUIC session from the Alt-Svc hint.

---

## [0.12.35] – 2026-03-02 — browser3: Alt-Svc + warmup navigation to reliably force H3

### Fixed
- **`browser3` always showed `proto=h2`** despite `--origin-to-force-quic-on` and SPKI hash
  pinning.  Root cause: on a LAN, Chrome's TCP connection is established in <1 ms and wins
  the TCP-vs-QUIC race before the QUIC handshake completes.
  Two changes together fix this:
  1. **Endpoint now serves `Alt-Svc: h3=":PORT"; ma=86400`** on every HTTPS response
     (Chrome ignores Alt-Svc from plain-HTTP origins).  Chrome caches the hint and opens a
     QUIC connection in the background after the first response.
  2. **browser3 probe does a warmup navigation** to `/health` before the measured page
     load: the warmup receives the Alt-Svc header, Chrome establishes the QUIC session
     (200 ms sleep), and the main navigation uses the already-open QUIC connection → H3.
     Network events are subscribed **after** the warmup so only the main navigation's
     resources are counted.
- **Self-signed cert now includes the server's primary LAN IP** (e.g. `172.16.32.106`) as
  an IP SAN (rcgen auto-detects IP strings).  Detected via the UDP-socket routing trick —
  no packets sent.  Eliminates the hostname-mismatch cert error for clients connecting via
  LAN IP, reducing reliance on SPKI-pin bypass.

### Changed
- `build_router()` → `build_router(h3_port: Option<u16>)` in `networker-endpoint`.
  Callers pass `Some(https_port)` when the `http3` feature is compiled in, `None` otherwise.

---

## [0.12.34] – 2026-03-02 — browser3: SPKI hash pinning for QUIC cert trust; browser1: plain-HTTP URL

### Fixed
- **`browser3` (HTTP/3 QUIC)** probe was always showing `proto=h2` because Chrome's QUIC
  TLS stack does not honour `--ignore-certificate-errors` for self-signed certs, causing
  fallback to H2.  Fix: before launching Chrome, the probe now performs a plain TLS
  connection to the target, extracts the leaf certificate's SubjectPublicKeyInfo (SPKI)
  DER bytes, SHA-256-hashes them, and passes the base64 hash to Chrome via
  `--ignore-certificate-errors-spki-list=<hash>`.  Chrome trusts the cert over QUIC and
  successfully negotiates H3.  If the SPKI fetch fails a warning is logged and Chrome
  may still fall back to H2.
- **`browser1` (HTTP/1.1)** probe was always showing `proto=h2` because the
  `--disable-http2` Chrome flag is silently ignored in Chrome ≥ 136's network service
  subprocess.  Fix: the probe now rewrites the target URL to `http://` (plain HTTP) and
  auto-derives the HTTP port (8443 → 8080, 443/default → 80, other ports unchanged).
  Plain HTTP has no TLS ALPN negotiation, so Chrome physically cannot negotiate H2 or H3.

### Added
- `fetch_spki_hash()` helper inside `mod real` — extracts and hashes the server's SPKI
  for Chrome cert-pinning (uses `sha2 0.10` + `base64 0.22`, both already optional deps
  under the `browser` feature)
- `build_browser1_url()` public helper — rewrites a URL to `http://` with port mapping
- Unit tests for `build_browser1_url` (4 new tests: port mapping, param encoding,
  non-standard port preserved, standard port omitted)

---

## [0.12.33] – 2026-03-02 — pageload/browser as all-3 shortcuts; pageload1 alias; script version sync

### Changed
- **`pageload` mode** now expands to `pageload1 + pageload2 + pageload3` automatically —
  a single `--modes pageload` runs all three HTTP versions for a full comparison
- **`browser` mode** now expands to `browser1 + browser2 + browser3` automatically —
  a single `--modes browser` runs all three forced-protocol variants
- Deduplication: if a mode appears multiple times after expansion it is only run once
- `SCRIPT_VERSION` in `install.sh` and `install.ps1` now always matches the binary
  workspace version (previously drifted; was 0.12.25/0.12.24, now 0.12.33)

### Added
- **`pageload1`** mode — explicit alias for the HTTP/1.1 pageload probe (equivalent to
  the old `pageload` single-mode behaviour); consistent with `pageload2` / `pageload3`

---

## [0.12.32] – 2026-03-02 — Add browser1/browser2/browser3 forced-protocol probe modes

### Added
- **`browser1`** probe mode — real headless Chromium forced to HTTP/1.1
  via `--disable-http2`
- **`browser2`** probe mode — real headless Chromium forced to HTTP/2
  via `--disable-quic` (prevents H3 upgrade); requires HTTPS
- **`browser3`** probe mode — real headless Chromium forced to HTTP/3 QUIC
  via `--enable-quic --origin-to-force-quic-on=<host>:<port>`; requires HTTPS
- All three appear in the Protocol Comparison table alongside
  `pageload`/`pageload2`/`pageload3`
- HTML "Browser Results" table gains a **Mode** column so all four browser
  variants are shown together with their mode labels
- Terminal log line now shows `[browser1]`/`[browser2]`/`[browser3]` instead of
  hardcoded `[browser]`

---

## [0.12.31] – 2026-03-02 — Fix browser probe root: unique per-run user-data-dir

### Fixed
- **`runner/browser.rs`**: after the `runuser` fix (v0.12.30), Chrome now
  runs as `SUDO_USER` but was still failing with
  `readlink(/tmp/chromiumoxide-runner/SingletonLock): Permission denied` —
  the `/tmp/chromiumoxide-runner` directory was left over from a previous
  root run and is owned by root, so the non-root user Chrome now runs as
  cannot write to it
- Fix: compute a unique per-run profile directory
  (`/tmp/networker-chrome-profile-<pid>`) and pass it to chromiumoxide via
  `BrowserConfig::builder().user_data_dir()`, bypassing the default
  `/tmp/chromiumoxide-runner` path; Chrome (running as `SUDO_USER`) creates
  this fresh directory itself in world-writable `/tmp`
- RAII guard (`ProfileDirGuard`) cleans up the profile directory on all
  return paths so `/tmp` is not littered with leftover profiles

---

## [0.12.30] – 2026-03-02 — Fix browser probe as root: launch Chrome via runuser as SUDO_USER

### Fixed
- **`runner/browser.rs`**: snap chromium's internal launcher script
  (`chromium.launcher`) strips `--no-sandbox` when the process UID is 0,
  causing Chrome to always exit with
  `Running as root without --no-sandbox is not supported`
  regardless of flags passed — previous workarounds (`XDG_RUNTIME_DIR`,
  `--disable-setuid-sandbox`, pre-creating `/run/user/0`) could not
  overcome this launcher-level filter
- Fix: when running as root and `SUDO_USER` is set (i.e. invoked via
  `sudo`), generate a temporary wrapper script that re-executes the
  Chrome binary as the original non-root user via
  `runuser -u <SUDO_USER> -- <chrome> "$@"`; Chrome then runs as a
  normal user and the snap root check never triggers
- `runuser` (util-linux) is used without a password when called by root
  and is available on all mainstream Linux distributions

---

## [0.12.29] – 2026-03-02 — Fix browser probe root/snap: pre-create /run/user/0

### Fixed
- **`runner/browser.rs`**: `XDG_RUNTIME_DIR=/tmp` (v0.12.28) did not prevent
  the `mkdir: cannot create directory '/run/user/0': Permission denied` error
  because snap-confine hardcodes `/run/user/<uid>` and ignores `XDG_RUNTIME_DIR`
- Fix: pre-create `/run/user/<uid>` with mode `0700` before launching Chrome
  (only when the directory does not already exist); as root we have permission
  to create it, so snap-confine's subsequent `mkdir` succeeds (directory already
  exists) and Chrome proceeds to accept `--no-sandbox`

---

## [0.12.28] – 2026-03-02 — Fix browser probe when running as root (sudo)

### Fixed
- **`runner/browser.rs`**: browser probe failed with
  `Running as root without --no-sandbox is not supported` when
  `networker-tester` was invoked via `sudo`
- Root cause: snap Chromium tries to create `/run/user/0` (XDG runtime dir
  for root) which doesn't exist; this caused the snap wrapper to fail before
  Chrome could read the `--no-sandbox` flag
- Set `XDG_RUNTIME_DIR=/tmp` at launch time when the variable is not already
  set (Unix only); this allows the snap setup to proceed
- Added `--disable-setuid-sandbox` alongside `--no-sandbox` for belt-and-braces
  compatibility with Chrome's setuid sandbox check when running as root

---

## [0.12.27] – 2026-03-02 — Sync Cargo workspace version to match CHANGELOG/tag version

### Changed
- Cargo workspace `version` bumped from `0.12.13` to `0.12.27` to re-align
  `networker-tester --version` / `networker-endpoint --version` with the
  CHANGELOG and git tag versioning (they had drifted apart over several
  installer-only releases that did not bump the Cargo version)

---

## [0.12.26] – 2026-03-02 — Upgrade chromiumoxide 0.7 → 0.9 to fix CDP crash on Chrome 145+

### Fixed
- **`runner/browser.rs`**: browser probe crashed with
  `data did not match any variant of untagged enum Message` on Ubuntu 22.04
  with Chromium 145 (snap); root cause was chromiumoxide 0.7 failing to
  deserialize new CDP message types introduced in newer Chrome versions,
  which killed the WebSocket handler and caused all subsequent page operations
  to fail
- Upgraded `chromiumoxide` dependency from `0.7` to `0.9` (latest: 0.9.1);
  the `tokio-runtime` feature flag was removed in 0.9 (tokio is now the
  default runtime), so the dependency entry is simplified accordingly

---

## [0.12.25] – 2026-03-02 — Fix Chrome prompt skipped when git is installed

### Fixed
- **`install.sh`**: `PKG_MGR` was only populated inside the `git not installed` else-branch,
  so on machines where git is already present the package manager was never detected;
  this caused the Chrome Y/N prompt to be silently skipped and the plan to show
  `(not installed — https://www.google.com/chrome/)` instead of asking
- Package manager detection now always runs, independent of git availability

---

## [0.12.24] – 2026-03-02 — Installer skips Chrome prompt when only installing endpoint

### Fixed
- **`install.sh`** / **`install.ps1`**: Chrome detection, plan display, and install prompt
  are now skipped entirely when only `networker-endpoint` is being installed
  (`install.sh endpoint`); Chrome/browser probe is only relevant to `networker-tester`

---

## [0.12.23] – 2026-03-02 — Installer always asks before installing Chrome

### Fixed
- **`install.sh`** / **`install.ps1`**: Chrome/Chromium was installed silently whenever
  a package manager was available and the user chose "1 — Proceed with default installation";
  the user was only asked in the "2 — Customize" path
- Now the installer always prompts `[Y/n]` for Chrome when it is not found, regardless
  of whether the user chose Proceed or Customize

---

## [0.12.22] – 2026-03-02 — Fix browser probe: add `/browser-page` HTML endpoint

### Fixed
- **`networker-endpoint` `/browser-page` route** (HTTP/1.1, HTTP/2, HTTP/3): new endpoint
  returns a real HTML page with `<img>` tags for each asset; the browser's `load` event
  fires only after all image requests settle, giving accurate end-to-end page load timing
- **`runner/browser.rs` `build_page_url()`**: previously pointed to `/page` which returns
  a JSON manifest — a real browser displays the JSON as text and never fetches the listed
  assets (`res=2 bytes=178`); now points to `/browser-page`
- **`build_page_url()`**: fixed query param name `size=` → `bytes=` to match the endpoint's
  `PageParams` struct (was silently ignored, defaulting to 10 KiB regardless)

---

## [0.12.21] – 2026-03-02 — Fix browser probe Chrome detection on Windows

### Fixed
- **`runner/browser.rs` `find_chrome()`**: added Windows path detection; previously only
  Linux and macOS paths were searched, so Chrome installed via the installer was never
  found at runtime on Windows
- Now checks `%PROGRAMFILES%\Google\Chrome\Application\chrome.exe`,
  `%LOCALAPPDATA%\Google\Chrome\Application\chrome.exe`, and
  `%PROGRAMFILES(X86)%\...` before falling through to Linux/macOS paths

---

## [0.12.20] – 2026-03-02 — Fix: `--features browser` applied only to networker-tester

### Fixed
- **`install.sh`** / **`install.ps1`**: `--features browser` was incorrectly passed to
  `networker-endpoint` as well as `networker-tester`; the `browser` feature does not
  exist in the endpoint crate, causing `cargo install` to fail with
  "the package 'networker-endpoint' does not contain this feature: browser"
- Fixed by guarding the flag: only added when the binary being installed is
  `networker-tester`

---

## [0.12.19] – 2026-03-02 — Installer detects Chrome; browser feature follows Chrome availability

### Changed
- **`browser` feature removed from `default`** in `Cargo.toml`; it was briefly added in
  v0.12.18 but is now driven by the installer instead (same pattern as git/MSVC detection)
- **`release.yml`**: pre-built release binaries are always built with `--features browser`
  (Chrome is a runtime dep only; CI build machines need no Chrome installed)
- **`install.sh`** (SCRIPT_VERSION 0.12.19): detects Chrome/Chromium at startup via
  `NETWORKER_CHROME_PATH` env var or standard macOS/Linux paths; shows in System
  Information table; offers to install via the system package manager if absent;
  user can toggle in the Customize flow; adds `--features browser` to `cargo install`
  only when Chrome is present after the install step
- **`install.ps1`** (SCRIPT_VERSION 0.12.19): same Chrome detection and offer via
  `winget install Google.Chrome`; Chrome status shown in System Information table;
  `--features browser` added to `cargo install` only when Chrome is available

---

## [0.12.18] – 2026-03-02 — Make `browser` probe a default feature

### Changed
- **`browser` feature is now included in `default`** alongside `http3`; previously it
  had to be explicitly opt-in at compile time (`--features browser`), which meant the
  install script and pre-built binaries did not include it
- The probe still fails gracefully with a clear message if Chrome/Chromium is not
  installed at runtime; no behavioural change for users without Chrome

---

## [0.12.17] – 2026-03-02 — Expand README: all probe modes + page-load comparison guide

### Changed
- **`README.md`**: Probe Modes table now lists all 18 probe modes (previously only 9),
  including `dns`, `tls`, `native`, `curl`, `webdownload`, `webupload`, `udpdownload`,
  `udpupload`, `pageload2`, `pageload3`, and `browser`
- **`README.md`**: new "Page-Load Protocol Comparison (H1 / H2 / H3 / Browser)" section
  shows the exact command for comparing protocol performance across all four page-load
  probes against a local endpoint
- **`README.md`**: updated installer description and removed stale Known Limitations row

---

## [0.12.16] – 2026-03-02 — Fix MSVC install: wait for background VS installation

### Fixed
- **`install.ps1`**: the VS bootstrapper winget downloads (~4 MB) launches the real
  ~2 GB installation as a background process; the previous version called `.Trim()`
  on `$null` (vswhere output before VS had registered) causing a script crash
- **`install.ps1`**: added `--wait` to the VS installer override so it blocks until
  fully complete; added a polling loop (15 s intervals, 15 min timeout) as a
  belt-and-suspenders fallback in case `--wait` is insufficient
- **`install.ps1`**: all `vswhere` output is cast via `[string]` before null/whitespace
  checks so the script never crashes on empty output
- **`install.ps1`**: wrapped `vcvars64.bat` environment loading in `try/catch`; on
  failure, shows a clear "reopen terminal and re-run" message instead of crashing

---

## [0.12.15] – 2026-03-02 — Detect missing build tools before cargo install

### Added
- **`install.ps1`**: detects Visual C++ Build Tools via `vswhere.exe` before
  attempting `cargo install`; if absent and `winget` is available, auto-offers
  `winget install Microsoft.VisualStudio.2022.BuildTools` (~2-3 GB, VC++ workload);
  after install, sources `vcvars64.bat` so `link.exe` is on PATH for the current
  session without reopening the terminal
- **`install.ps1`**: VC++ build tools status shown in System Information table
  (`installed ✓` or `not installed`)
- **`install.ps1`**: customize flow lets user toggle the VC++ install step; shows
  the exact `winget` command to run manually if user opts out
- **`install.ps1`**: pre-flight warning in cargo install step if MSVC still absent —
  prints the exact `winget` one-liner to fix it
- **`install.sh`**: pre-flight check in `step_cargo_install` warns when no C linker
  (`cc`/`gcc`/`clang`) is found and prints the correct install command
  (`xcode-select --install` on macOS; `build-essential`/`gcc`/`base-devel` on Linux)

---

## [0.12.14] – 2026-03-02 — Optional git installation in both installers

### Added
- **`install.sh`**: detects installed package manager (`brew`, `apt-get`, `dnf`,
  `pacman`, `zypper`, `apk`) and offers to install git automatically when git is
  absent in source mode; new `detect_pkg_manager()` helper
- **`install.ps1`**: detects `winget` and offers to install `Git.Git` automatically
  when git is absent in source mode; refreshes `$env:PATH` after install so git
  is usable without reopening a terminal
- Both installers: git version shown in System Information table; installation plan
  includes a numbered "Install git" step when git is absent + package manager found
- Customize flow in both installers lets the user toggle the git install step

### Fixed
- **`install.sh`**: `CARGO_NET_GIT_FETCH_WITH_CLI=true` is now only set when `git`
  is found on PATH (mirrors the PR #84 fix already applied to `install.ps1`)

---

## [0.12.13] – 2026-03-02 — Fix Windows source-mode install when git is absent

### Fixed
- **`install.ps1`**: `CARGO_NET_GIT_FETCH_WITH_CLI=true` is now only set when
  `git.exe` is found on PATH. Previously, setting it unconditionally caused
  `cargo install` to fail with "program not found" on machines that have SSH
  (and Rust) but not Git for Windows. Without the flag, cargo falls back to its
  built-in libgit2, which reads SSH keys from `%USERPROFILE%\.ssh\` directly.
- **`install.ps1`**: `cargo install` exit code is now checked; a non-zero exit
  immediately prints a clear error (with a Git-for-Windows hint if git was absent)
  and exits — previously the installer reported success even when cargo failed.

---

## [0.12.12] – 2026-03-02 — Binary release downloads + cross-compile CI

### Added
- **Release workflow now builds and uploads native binaries** for 4 platforms:
  `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`,
  `x86_64-pc-windows-msvc`. Each release asset is a `.tar.gz` (Unix) or `.zip`
  (Windows) containing the single binary. Archives are attached to every GitHub
  release going forward.
- **Both installers now auto-detect install mode**:
  - **release mode** (default when available): if `gh` CLI is installed and
    `gh auth login` has been run, and the current platform has a release binary,
    the installer downloads and extracts the pre-built binary via
    `gh release download --latest` (~10 s total)
  - **source mode** (fallback): compiles from source via `cargo install` as before
    (~5-10 min); chosen automatically when `gh` is not available or
    `--from-source` / `-FromSource` flag is passed
- **`install.sh`**: new `--from-source` flag; system info shows `gh CLI: authenticated ✓`
  when release mode is available; plan section shows install method + target triple;
  customize flow offers install method choice
- **`install.ps1`**: new `-FromSource` switch; same UX improvements as bash
- Release mode installs to `~/.cargo/bin/` (Unix) / `%USERPROFILE%\.cargo\bin\`
  (Windows) — same path as source mode, identical PATH instructions

---

## [0.12.11] – 2026-03-02 — Rustup-style interactive installer

### Changed
- **`install.sh`** rewritten with a rustup-style interactive UX:
  - Banner with version, system info table (OS, arch, shell, Rust version, install path)
  - Numbered installation plan displayed before any action is taken
  - Three-option prompt: `1) Proceed` / `2) Customize` / `3) Cancel`
  - Customize flow lets the user toggle each step individually (SSH check, Rust install,
    component selection: tester / endpoint / both) then shows a revised plan before executing
  - Step headers (`Step N: …`) printed during execution so progress is always visible
  - Completion summary with PATH notice and quick-start commands
  - New CLI flags: `-y`/`--yes` (non-interactive), `--skip-ssh-check`, `--skip-rust`,
    `-h`/`--help`; positional `tester|endpoint|both` (default: `both`)
- **`install.ps1`** rewritten with the equivalent rustup-style UX for Windows:
  - Same banner → system info → plan → 1/2/3 prompt → customize → step execution → summary
  - PS 5.1 compatible throughout (no `??` operator, no `?.`); `$script:` prefix for
    cross-function state mutation
  - ARM64 vs x64 rustup-init URL auto-detected via `$env:PROCESSOR_ARCHITECTURE`
  - `$ErrorActionPreference` guarded around SSH call (GitHub always exits 1 by design)
  - Validates `-Component` value even when script is piped through `iex`
  - Equivalent flags: `-Yes`, `-SkipSshCheck`, `-SkipRust`, `-Help`

---

## [0.12.10] – 2026-03-02 — Coverage phase 5: tls/curl/pageload/html unit tests

### Added
- **`runner/tls.rs`** +2 tests: `make_failed` constructor — verifies `Protocol::Tls`, error
  message/detail/category, `retry_count=0`, `finished_at` present, `tls/dns/tcp` all None.
- **`runner/curl.rs`** +2 tests: `make_failed` constructor — verifies `Protocol::Curl`, error
  fields, no-detail variant.
- **`runner/pageload.rs`** +5 tests: `parse_server_timing_simple` — no headers → None;
  version header → Some with version; clock_skew formula (`diff_ms - ttfb/2`, verified to
  within 5ms tolerance); invalid RFC3339 timestamp → `server_timestamp`/`clock_skew` both None;
  server-timing header alone → Some.
- **`output/html.rs`** +10 tests: `append_proto_row` — all-success → `"ok"` CSS class; partial
  failure → `"warn"` CSS class; TTFB/total averages computed correctly; no HTTP result → em
  dash. `append_attempt_row` — HTTP 200 success shows status + `"ok"`; failed attempt shows
  `row-err` class + error message + detail in title; UDP echo shows `rtt_avg_ms` + `loss%`;
  UDP throughput shows `transfer_ms` + `throughput_mbps`; no-result attempt shows multiple em
  dashes + `OK` span; Download attempt shows `throughput_mbps` + formatted payload size.

### Coverage
- Total: 70.28% → **71.78%** lines (+1.5 pp); 74.48% → **75.40%** functions

---

## [0.12.9] – 2026-03-02 — Coverage phase 4: http.rs + udp.rs unit tests + integration flakiness fix

### Added
- **30 new unit tests in `runner/http.rs`**: `is_no_proxy` edge cases (empty string,
  case-insensitive, whitespace trimming, empty entries, non-suffix match); `parse_server_timing_header`
  edge cases (unknown name ignored, invalid `dur=` ignored, `dur=` among multiple attrs);
  `parse_cert_fields` (invalid DER returns None, empty bytes, valid cert subject/issuer/expiry via
  rcgen, expiry is in future); `pick_ip` fallback cases (no IPv4 available → first, `prefer_v4=false`
  → first regardless); `failed_attempt` constructor (all fields verified); `parse_server_timing`
  with HeaderMap (no headers → None, server-timing only, `x-networker-server-version`, request-id);
  `build_request` method/header selection (GET for empty payload, POST with content-length and
  upload-bytes headers, host + request-id headers set correctly).
- **3 new unit tests in `runner/udp.rs`**: `udp_failed` constructor correctness; loopback echo
  server with `probe_rtts_ms` all-Some verification; existing test reuse without duplication.

### Fixed
- **Integration test flakiness** (`free_udp_port()`): UDP server binds `0.0.0.0:{port}` but the
  previous `free_port()` used a TCP listener (different port namespace). New `free_udp_port()` now
  binds `0.0.0.0:0` — the same address family as the server — so the port is guaranteed free for a
  subsequent `0.0.0.0:{port}` bind, eliminating "UDP throughput server did not bind within 10s" panics.

### Coverage
- `runner/http.rs`: 68% → 78.22% lines, 75% → 86.67% functions
- `runner/udp.rs`: 60% → 84.68% lines, 78% → 88.89% functions
- Total: 67.46% → 70.28% lines, 74.92% → 74.48% functions (line improvement of +2.8 pp)

---

## [0.12.8] – 2026-03-02 — Upload size verification via response header

### Added
- **`X-Networker-Upload-Bytes: N`** request header: the client now declares the
  intended upload size on every POST so the server knows what to expect.
- **`X-Networker-Received-Bytes: N`** response header: the endpoint echoes the
  actual number of bytes drained from the request body, enabling end-to-end
  verification without parsing the JSON body.
- `verify_upload()` in `throughput.rs`: after every `upload` / `webupload` probe,
  the received byte count is compared against the declared payload size. A mismatch
  marks the attempt `success = false` with a clear `ErrorCategory::Http` message
  ("sent N bytes but server received M bytes"). Absent header (third-party or older
  endpoint) is silently skipped — no false failures.
- 6 new unit tests covering the verification logic: match passes, mismatch fails with
  descriptive message, absent header is skipped, already-failed attempt is not
  overwritten, header name matching is case-insensitive.
- 1 new endpoint unit test: `upload_returns_received_bytes_header`.

---

## [0.12.7] – 2026-03-02 — Streaming upload body; large-payload timeout scaling

### Fixed
- **Upload probes** (`upload`, `webupload`) no longer allocate the entire upload payload in
  RAM at once. The body is now streamed in 256 KiB chunks from a static zero buffer, so a
  5 GiB upload uses ~256 KiB of memory instead of ~5 GB.
- **Timeout auto-scaling**: both upload probes extend the request timeout if the payload
  cannot complete at an assumed minimum speed of ~100 MB/s within the user-specified timeout.
  A 5 GiB upload now gets ~60 s instead of the default 30 s, preventing spurious timeouts
  on large but healthy uploads. Users on slower links can still set `--timeout` explicitly.
- `content-length` header is now set on streaming upload requests so HTTP/1.1 uses
  fixed-length framing rather than chunked transfer encoding.

### Added
- 9 new unit tests: `upload_body_exact_byte_count`, `upload_body_all_zeros`,
  `upload_body_small_payload`, `upload_body_zero_bytes_yields_nothing` (streaming body
  correctness); `timeout_unchanged_for_small_payloads`, `timeout_unchanged_for_one_gib`,
  `timeout_extended_for_five_gib`, `timeout_never_below_base`,
  `timeout_zero_payload_returns_overhead_only` (timeout scaling logic).

---

## [0.12.6] – 2026-03-02 — Integration tests for pageload H1/H2/H3

### Added
- Integration tests for `run_pageload_probe` (HTTP/1.1), `run_pageload2_probe` (HTTP/2),
  and `run_pageload3_probe` (HTTP/3 over QUIC) — all using the in-process endpoint on random
  ports, no external dependencies required
- `Endpoint::wait_for_quic()` helper in the integration test fixture: waits 300 ms after
  TCP-HTTPS readiness to let the QUIC server bind its UDP port
- `pageload.rs` line coverage: 11% → 74%; function coverage: 28% → 84%
- Overall (lib + integration): lines 47.9% → 57.4%, functions 54% → 65.8%

---

## [0.12.5] – 2026-03-02 — Test coverage phase 2

### Added
- **`runner/curl.rs`** — unit tests for `parse_write_out`, `secs_to_ms`, and
  `error_category_for_exit` (all pure-logic, no live curl needed): 11 new tests
- **`metrics.rs`** — tests for `primary_metric_label` (Dns / Tls / Browser),
  `primary_metric_value` (with and without sub-results), `attempt_payload_bytes`
  (http, udp_throughput, zero-payload filter), `TestRun::protocols_tested`
  (deduplication), and `RequestAttempt::total_duration_ms`: 18 new tests
- **`runner/pageload.rs`** — tests for all 6 named presets (tiny / small / medium /
  large / default / mixed), mixed-asset composition, case-insensitive matching,
  and unknown-preset error messaging: 8 new tests
- **`cli.rs`** — tests for empty-modes validation, verbose flag → log-level debug,
  log-level overriding verbose, `parsed_modes` filtering invalid strings, `--page-preset`
  tiny resolution, invalid preset fallback, `parse_size` gigabyte suffix, and
  `load_config` error paths: 8 new tests
- **`output/html.rs`** — tests for CSS `<link>` injection, error section, Throughput
  Results section (Download attempt), TLS Details section, Page Load section, Browser
  Results section, `escape_html` quotes/ampersand: 8 new tests
- **`output/excel.rs`** — test exercising the Throughput sheet (Download + Upload
  attempts) that was previously uncovered: 1 new test
- **`runner/browser.rs`** — tests for `find_chrome` with non-existent env-var path
  (skip), existing tempfile path (return), and `build_page_url` scheme/host preservation:
  3 new tests

---

## [0.12.4] – 2026-03-02 — README: fix Windows installer examples

### Fixed
- **README.md** — Windows PowerShell section now shows self-contained one-liners for both
  `tester` and `endpoint`: the endpoint example previously assumed the script was already
  downloaded locally (`.\install.ps1 -Component endpoint`); replaced with an
  `Invoke-WebRequest` + `&` pattern that downloads from the Gist URL before running.
  Added PS 5.1 / PS 7+ compatibility note.

---

## [0.12.3] – 2026-03-02 — Fix install.ps1 NativeCommandError on SSH probe

### Fixed
- **`install.ps1`** — `ssh -T git@github.com` always exits with code 1 (GitHub design).
  With `$ErrorActionPreference = "Stop"` set globally, PowerShell 5.1 throws
  `NativeCommandError` before the authentication string can be checked.
  Fixed by temporarily lowering preference to `"Continue"` around the SSH call and
  restoring it immediately after.

---

## [0.12.2] – 2026-03-02 — Fix Gist sync to include install.ps1

### Fixed
- **`sync-gist.yml`** — workflow only watched `install.sh` changes and only uploaded `install.sh`
  to the Gist; `install.ps1` was never synced even when it changed. Updated path trigger and
  payload to include both `install.sh` and `install.ps1`.

---

## [0.12.1] – 2026-03-01 — Fix install.ps1 compatibility with Windows PowerShell 5.1

### Fixed
- **`install.ps1`** — replaced `?.Source` (null-conditional member access, requires PS 7.1+)
  with a PS 5.1-compatible `if ($cmd) { $cmd.Source }` pattern. The script declared
  `#Requires -Version 5.1` but used syntax only available in PowerShell 7.1+, causing a
  `ParseException: UnexpectedToken` when run via `irm … | iex` on Windows with the default
  Windows PowerShell 5.1.

---

## [0.12.0] – 2026-03-01 — Real-browser probe (`browser` mode via chromiumoxide)

### Added
- **`browser` probe mode** (`--features browser`) — drives a real headless Chromium instance
  via the Chrome DevTools Protocol (chromiumoxide 0.7) to measure actual page-load performance
  that no synthetic probe can replicate.
- **Metrics captured**: load time (navigation start → load event), DOMContentLoaded (ms),
  TTFB (ms), total resource count, total transferred bytes, negotiated protocol for the main
  document, and per-protocol resource counts (e.g. `h2×18 h3×2`).
- URL is rewritten to `/page` endpoint for a fair comparison with `pageload`/`pageload2`/
  `pageload3` probes.
- Self-skips with a `success: false` `RequestAttempt` if no Chrome/Chromium binary is found;
  binary search order: `NETWORKER_CHROME_PATH` env var → common Linux paths → macOS app bundles.
- `--features browser` is opt-in (not part of `default`); the stub build always compiles and
  returns a clear error message.
- HTML report **"Browser Results"** section with a per-attempt table (Protocol, TTFB, DCL,
  Load, Resources, Bytes, per-protocol counts).
- Terminal summary: `Protocol Comparison` table now includes a `browser` row.
- New `BrowserResult` struct in `metrics.rs`; `Protocol::Browser` variant in the `Protocol`
  enum; `RequestAttempt.browser: Option<BrowserResult>` field (backwards-compatible via
  `#[serde(default, skip_serializing_if = "Option::is_none")]`).

---

## [0.11.7] – 2026-03-01 — Fix ServerTimingResult schema (type mismatch in FK)

### Fixed
- **`sql/06_ServerTiming.sql`** — `ServerId` and `AttemptId` columns were declared
  as `UNIQUEIDENTIFIER`, but `RequestAttempt.AttemptId` (the FK target) is
  `NVARCHAR(36)`. SQL Server rejected the `CREATE TABLE` silently (sqlcmd
  doesn't abort on DDL errors), leaving `dbo.ServerTimingResult` absent from the
  database. Both columns are now `NVARCHAR(36)` to match the schema convention
  used by all other tables and the Rust insert code (`uuid.to_string()`).
  The `DEFAULT NEWSEQUENTIALID()` on `ServerId` is also removed — the insert
  always provides an explicit UUID value.

---

## [0.11.6] – 2026-03-01 — Improve sql.rs coverage

### Added
- **`sql_full_round_trip` test** — exercises all 7 sub-result insert helpers
  (`insert_dns_result`, `insert_tcp_result`, `insert_tls_result`,
  `insert_http_result`, `insert_udp_result`, `insert_error`,
  `insert_server_timing_result`) by inserting a fully-populated `RequestAttempt`
  with every sub-result field set; expected to push `sql.rs` line coverage from
  ~39% to ~85%+.

### Changed
- `sql_insert_round_trip` refactored to use shared `make_run` / `bare_attempt` /
  `sql_conn` helpers so the new comprehensive test reuses the same scaffolding.

---

## [0.11.5] – 2026-03-01 — Fix 6 test failures with --include-ignored in coverage CI

### Fixed
- **`validate_save_to_sql_without_conn_string_fails`** — test now self-skips when
  `NETWORKER_SQL_CONN` is set in the environment; clap picks up the env var automatically,
  so validation correctly passes in that environment (breaking the assertion).
- **`sql_insert_round_trip`** — coverage CI schema creation now runs all 7 SQL migration
  files (01–02 base + 04–07 migrations); previously missing migrations left `RequestAttempt`
  without the `RetryCount` column, causing the INSERT to panic.
- **`http1_probe_succeeds` / `http2_probe_negotiates_h2`** — tests now probe
  `127.0.0.1:8080` / `127.0.0.1:8443` with a TCP connect before running; they self-skip
  with an `eprintln!` message when no endpoint is listening, avoiding false failures in CI.
- **`download_probe_returns_throughput` / `upload_probe_returns_throughput`** — same
  self-skip pattern: TCP connect to `127.0.0.1:8080` before running the probe.
- **`sql-integration` CI job** — also updated to run all 7 SQL migration files (same fix
  as coverage job).

---

## [0.11.4] – 2026-03-01 — Coverage phase 1: SQL Docker, DNS tests, Excel tests

### Added
- **SQL Server in coverage CI** — MSSQL 2022 Docker service added to the coverage
  job; schema migrations run automatically; `NETWORKER_SQL_CONN` env var set so the
  existing `#[ignore]` SQL round-trip test executes under `cargo-llvm-cov`.
- **Excel unit tests** — `save_writes_xlsx_file` exercises all 10 worksheet writers
  with a fully-populated `TestRun` (HTTP, TCP, TLS, UDP, throughput, UDP throughput,
  server timing, page-load, errors); `save_empty_run_does_not_panic` covers the
  zero-attempt edge case.
- **`main.rs` excluded from coverage** — `--ignore-filename-regex 'main\.rs'` added
  to all `cargo llvm-cov report` invocations so the binary entry point does not
  drag down the overall percentage.

### Changed
- DNS tests `resolves_localhost` and `ipv4_only_filter` are no longer `#[ignore]` —
  loopback resolution works in all environments including CI.
- Coverage lib-test step now passes `--include-ignored` to pick up `#[ignore]` tests
  that self-skip when their service is unavailable (SQL, curl).

---

## [0.11.3] – 2026-03-01 — Coverage report as GitHub Actions artifact

### Changed
- CI coverage job now uploads an HTML report + `lcov.info` as a downloadable
  GitHub Actions artifact (`coverage-report`, 30-day retention) instead of
  pushing to Codecov. The coverage summary is also printed directly in the
  CI log (`cargo llvm-cov report --summary-only`).

---

## [0.11.2] – 2026-03-01 — Fix --all-features compile error in native probe

### Fixed
- `runner/native.rs`: second `HttpResult` literal was missing `goodput_mbps`,
  `cpu_time_ms`, `csw_voluntary`, and `csw_involuntary` — fields added in v0.11.0.
  Only exposed by `--all-features` builds (e.g. `cargo-llvm-cov`); the default CI
  build does not enable `native` and did not catch this. (#58)

---

## [0.11.1] – 2026-03-01 — HTTP/3 QUIC endpoint; --insecure for http3 probe

### Added
- **HTTP/3 QUIC server in `networker-endpoint`** — Quinn-based QUIC listener on UDP
  8443 (same port as HTTPS), serving `/health`, `/download`, `/upload`, `/page`, `/asset`
  with full `Server-Timing` (proc/recv/csw-v/csw-i) and `X-Networker-*` headers.
  `http3` is now a default feature of both crates; no extra flags needed.
- **`--insecure` and `--ca-bundle` for `http3` probe** — previously the h3 client always
  used webpki roots and ignored these flags. Now uses the same `build_tls_config()` path
  as HTTP/1.1 and HTTP/2, so `--insecure` works with the self-signed endpoint cert.

### Changed
- `networker-endpoint/Cargo.toml`: `[features] default = ["http3"]`
- `/info` endpoint: `"http3": true` and `"protocols": ["HTTP/1.1","HTTP/2","HTTP/3"]`
  when compiled with the http3 feature (now the default).

---

## [0.11.0] – 2026-02-28 — CPU cost, goodput, context switches & TTFB visibility

### Added
- **CPU time on all HTTP probes** (`http1`, `http2`, `http3`) — `HttpResult.cpu_time_ms`
  captures process CPU (user + system) consumed per probe using `cpu-time::ProcessTime`.
  Enables a fair H1 vs H2 vs H3 comparison; QUIC/HTTP3 is expected to show the highest
  CPU cost due to in-process TLS encryption.
- **Goodput metric** — `HttpResult.goodput_mbps` = payload_bytes / full end-to-end delivery
  time (DNS + TCP + TLS + total HTTP ms). Penalises connection-setup overhead, giving a
  more complete picture than throughput alone (which only measures the body-transfer phase).
  Set for all four throughput probe types: `download`, `upload`, `webdownload`, `webupload`.
- **Client-side context switches** — `HttpResult.csw_voluntary` and `csw_involuntary`
  capture the `getrusage(RUSAGE_SELF)` delta (`ru_nvcsw`, `ru_nivcsw`) over the full probe
  duration (Unix only; `None` on Windows).
- **Server-side context switches** — `networker-endpoint` now appends
  `csw-v;dur=N, csw-i;dur=N` to the existing `Server-Timing` header on `/download` and
  `/upload` responses. `ServerTimingResult.srv_csw_voluntary` /
  `srv_csw_involuntary` expose these values in the tester's metrics.
- **TTFB + TLS visibility in throughput terminal output** — `log_attempt()` for
  `download`, `upload`, `webdownload`, `webupload` probes now shows:
  `TLS:Xms` (when applicable), `TTFB:Xms`, `Goodput:X MB/s`, `CPU:Xms`,
  `CSW:Xv/Xi` (client), `sCSW:Xv/Xi` (server).
- **New HTML Throughput Results columns**: Goodput (MB/s), CPU (ms),
  Client CSW (v/i), Server CSW (v/i) alongside the existing TTFB and Total columns.

### Internal
- `parse_server_timing_header()` refactored to return a named `ParsedServerTiming` struct
  (replacing the previous 3-tuple) to accommodate the two new `csw-v`/`csw-i` fields.

---

## [0.10.0] – 2026-02-28 — H1.1 keep-alive fix, TLS cost visibility, named presets, CPU measurement

### Added
- **`pageload` H1.1 keep-alive pool** — corrected a fundamental accuracy bug where each
  asset opened a brand-new TCP+TLS connection. The rewritten probe opens `k = min(6, n)`
  persistent TCP connections (one TLS handshake each for HTTPS) and distributes assets
  across them round-robin, so each connection reuses its TCP/TLS handshake for all its
  assigned assets — exactly how a real browser behaves. This eliminates the previous
  inflation of TLS setup cost and makes the H1.1 vs H2 vs H3 comparison accurate.
- **TLS cost fields on `PageLoadResult`** — four new fields report the cost of TLS
  establishment per page-load variant:
  - `tls_setup_ms`: sum of all TLS handshake durations (H1.1: k handshakes; H2/H3: 1).
  - `tls_overhead_ratio`: fraction of `total_ms` spent in TLS (0.0–1.0).
  - `per_connection_tls_ms`: per-connection handshake durations (length = `connections_opened`).
  - `cpu_time_ms`: process CPU time consumed during the probe (highest for HTTP/3 due to
    QUIC userspace encryption).
- **Named `--page-preset` flag** — selects a predefined asset mix, overriding
  `--page-assets` and `--page-asset-size`:

  | Preset    | Assets | Size per asset | Total    |
  |-----------|--------|---------------|----------|
  | `tiny`    | 100    | 1 KB          | ~100 KB  |
  | `small`   | 50     | 5 KB          | ~250 KB  |
  | `default` | 20     | 10 KB         | ~200 KB  |
  | `medium`  | 10     | 100 KB        | ~1 MB    |
  | `large`   | 5      | 1 MB          | ~5 MB    |
  | `mixed`   | 30     | varied        | ~820 KB  |

  The `mixed` preset (1×200KB + 4×50KB + 10×20KB + 15×5KB) approximates a real-world
  web page with a large hero image, medium assets, and many small scripts/styles.
- **Per-asset sizes in `PageLoadConfig`** — `asset_sizes: Vec<usize>` replaces the old
  uniform `asset_count`/`asset_size` pair. Each element specifies the byte count for
  one asset, enabling varied payloads (used by presets and future per-asset control).
- **Extended Protocol Comparison table** — both the terminal output and the HTML report
  now include `TLS Setup (ms)`, `TLS Overhead %`, and `CPU (ms)` columns, making the
  cost structure of each protocol variant immediately visible.

### Changed
- `PageLoadConfig.asset_count` / `asset_size` → `asset_sizes: Vec<usize>` and
  `preset_name: Option<String>`. Consumers must pass `asset_sizes` (a `Vec`).
- `ResolvedConfig.page_assets` / `page_asset_size` → `page_asset_sizes: Vec<usize>` and
  `page_preset_name: Option<String>`.
- Workspace version bumped to `0.10.0` (MINOR — new fields, new flag, keep-alive fix).

---

## [0.9.0] – 2026-02-28 — HTTP/3 page-load probe

### Added
- **`pageload3` probe mode** — fetches the same N assets as `pageload`/`pageload2` but
  multiplexed over a single QUIC/HTTP/3 connection (`connections_opened = 1`).
  All N asset streams are opened sequentially (fast HEADERS frames) then all responses
  are received concurrently. Requires `--features http3` and an HTTPS target.
  Completes the three-protocol page-load comparison: HTTP/1.1 (≤6 conns) vs
  HTTP/2 (1 TLS conn) vs HTTP/3 (1 QUIC conn), motivated by
  "Does QUIC Make the Web Faster?" (Biswal & Gnawali, IEEE GLOBECOM 2016).
- **`--insecure` support for `pageload3`** — reuses `build_tls_config` from `http.rs`
  (same `NoCertVerifier` + custom CA bundle path), overriding ALPN to `h3`.
- **ALPN warning extended** — startup `[WARN]` now also fires for `pageload3` mode
  against a plain `http://` target.
- **Protocol Comparison table extended** — terminal and HTML report now include a
  `pageload3` row alongside `pageload` and `pageload2`.

### Background
Reference [5] cited in "Does QUIC Make the Web Faster?" for the finding that
bandwidth improvements beyond ~5 Mbps yield diminishing returns on page load time is:
Ilya Grigorik, *"Latency: The New Web Performance Bottleneck"*,
https://www.igvita.com/2012/07/19/latency-the-new-web-performance-bottleneck/, 2012.
This motivates testing all three protocols: the wall-clock difference between `pageload`,
`pageload2`, and `pageload3` reveals which bottleneck (connection setup vs multiplexing
vs QUIC handshake latency) dominates under real network conditions.

---

## [0.8.0] – 2026-02-28 — Page-load simulation, ALPN warning

### Added
- **`pageload` probe mode** — fetches `/page?assets=N&bytes=B` manifest from the endpoint
  then downloads all assets over up to 6 parallel HTTP/1.1 connections (browser-like).
  Measures wall-clock `total_ms`, `ttfb_ms`, `connections_opened`, per-asset timings,
  and total bytes. Configure with `--page-assets N` (default 20) and
  `--page-asset-size <size>` (default 10k, accepts k/m suffixes).
- **`pageload2` probe mode** — same N assets multiplexed over a single HTTP/2 TLS
  connection. Records `connections_opened = 1`. Requires an HTTPS target.
- **`/page` and `/asset` endpoints on `networker-endpoint`** — `GET /page?assets=N&bytes=B`
  returns a JSON manifest listing N asset URLs; `GET /asset?id=X&bytes=B` returns B
  zero bytes (cap 100 MiB).
- **ALPN warning** — startup warns with `[WARN]` when `http2`, `http3`, or `pageload2`
  mode is requested against a plain `http://` target (HTTP/2 requires TLS+ALPN; over
  plain HTTP every connection silently falls back to HTTP/1.1).
- **`PageLoadResult` struct** — `asset_count`, `assets_fetched`, `total_bytes`,
  `total_ms`, `ttfb_ms`, `connections_opened`, `asset_timings_ms`, `started_at`.
  Attached to `RequestAttempt.page_load` (serde-default, skip_serializing_if none).
- **Terminal comparison table** — when both `pageload` and `pageload2` are run in the
  same session, a `Protocol Comparison (Page Load)` table is printed showing N,
  assets, avg connections, p50/min/max total_ms per variant.
- **HTML Protocol Comparison card** — same data rendered as an HTML `<table>` after
  the Statistics Summary section whenever any `pageload`/`pageload2` attempts are present.
- `pageload` and `pageload2` appear in terminal averages + statistics tables, HTML
  Timing Breakdown, and HTML Statistics Summary.

### Changed
- CLI `--modes` help text extended to document `pageload` and `pageload2`.
- `runner/http.rs::build_tls_config` promoted to `pub(crate)` for reuse by `pageload.rs`.
- `cli::parse_size` promoted to `pub(crate)` for reuse in `resolve()`.
- Workspace version bumped to `0.8.0` (MINOR — new features).

---

## [0.7.0] – 2026-02-28 — native-TLS probe, curl probe, tls_backend field

### Added
- **`native` probe mode** — DNS + TCP + platform TLS + HTTP/1.1 using the OS TLS
  stack: SChannel (Windows), SecureTransport (macOS), OpenSSL (Linux). Requires
  recompiling with `--features native` (gates the `native-tls` / `tokio-native-tls`
  deps to avoid mandatory OpenSSL headers on Linux CI). Records leaf certificate
  info via `x509-parser`. TLS version and cipher suite are not exposed by
  `native-tls` and are reported as `"unknown"`.
- **`curl` probe mode** — spawns the system `curl` binary with `--write-out` timing
  fields and maps the output to the same `DnsResult` / `TcpResult` / `TlsResult` /
  `HttpResult` structs as an `http1` probe. Requires `curl` on `$PATH`; returns a
  graceful error at runtime if not found. Supports `--insecure`, `--proxy`,
  `--ca-bundle`, `--ipv4-only`, `--ipv6-only`, and `--timeout`.
- **`TlsResult.tls_backend: Option<String>`** — new serde-default field that records
  which TLS implementation performed the handshake: `"rustls"` for all existing
  rustls-based probes (`http1`, `http2`, `http3`, `tls`), `"native/schannel"` /
  `"native/secure-transport"` / `"native/openssl"` for the `native` probe, and
  `"curl"` for the `curl` probe.
- `native` and `curl` appear in the terminal summary tables, HTML Statistics
  Summary, and HTML Timing Breakdown.

### Changed
- CLI `--modes` help text extended to document `native` and `curl`.
- Workspace version bumped to `0.7.0` (MINOR — new features).

### Fixed
- `runner/tls.rs`: default port for non-HTTPS targets was incorrectly `443`; now `80`.

---

## [0.6.0] – 2026-02-28 — DNS probe, TLS probe, proxy support, CA bundle

### Added
- **`dns` probe mode** — standalone DNS resolution probe (`--modes dns`); records
  resolved IPs, query duration, and success state. No TCP or HTTP activity.
- **`tls` probe mode** — standalone TLS handshake probe (`--modes tls`); performs
  DNS + TCP connect + TLS handshake and records the full certificate chain (all
  certs with Subject, Issuer, SANs, and expiry), negotiated cipher suite, TLS
  version, and ALPN protocol. Advertises both `h2` and `http/1.1` in ALPN to
  discover server preference without sending an HTTP request.
- **`--proxy <url>`** — explicit HTTP proxy URL (e.g. `http://proxy.corp:3128`);
  overrides `HTTP_PROXY`/`HTTPS_PROXY` env vars. For HTTPS targets, a CONNECT
  tunnel is established through the proxy before TLS; for HTTP targets an
  absolute-form URI is used.
- **`--no-proxy`** — disable all proxy detection (both `--proxy` flag and
  `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` env vars). Respects `NO_PROXY` /
  `no_proxy` env var when reading proxy settings from the environment.
- **`--ca-bundle <path>`** — path to a PEM-format CA certificate bundle to add
  to the trust store; useful for corporate CAs not present in the OS store.
  Supported by both HTTP/HTTPS probes and the standalone TLS probe.
- **`CertEntry`** struct in `metrics.rs` — captures `subject`, `issuer`, `expiry`,
  and `sans` (Subject Alternative Names) for each certificate in the chain.
- **`cert_chain: Vec<CertEntry>`** field on `TlsResult` — populated by the
  standalone TLS probe.
- **`proxy` / `ca_bundle`** fields in `ConfigFile` / `ResolvedConfig` / `tester.example.json`.
- Terminal progress logging for `dns` and `tls` protocols.
- HTML and terminal summary tables now include `dns` and `tls` rows.

### Changed
- `RunConfig` gains `ca_bundle: Option<String>`, `proxy: Option<String>`, and
  `no_proxy: bool` fields (all defaulting to `None`/`false`).
- `build_tls_config()` in `runner/http.rs` now returns `anyhow::Result` and
  accepts an optional CA bundle path.
- Workspace version bumped to `0.6.0` (MINOR — new features).

---

## [0.5.0] – 2026-02-28 — Payload-grouped stats + collapsible HTML sections

### Added
- **Payload-grouped statistics** — the terminal Statistics Summary and Averages tables now group
  results by `(protocol, payload_size)` rather than by protocol alone. Running
  `--modes download,upload --payload-sizes 64k,1m,4m` produces separate rows for
  "download 64KiB", "download 1MiB", etc., each with their own N/Min/Mean/p50/p95/p99/Max/StdDev.
- **`attempt_payload_bytes()`** — new public helper in `metrics.rs` that returns the payload
  size for throughput attempts (`http.payload_bytes` or `udp_throughput.payload_bytes`),
  `None` for latency-only probes.
- **`fmt_bytes()` helper in `main.rs`** — formats byte counts as KiB/MiB/GiB for terminal output.
- **Collapsible `<details>` sections in HTML report** (no JS — pure HTML5):
  - **Throughput Results** — one `<details>` per `(proto, payload)` group; summary line shows
    `N runs · avg X MB/s · ±stddev · min Y · max Z`. Expanded by default only when there is
    exactly one group with ≤ 20 rows.
  - **UDP Throughput Results** — same treatment; summary line includes average loss %.
  - **All Attempts** — single collapsible block; summary shows succeeded/failed counts;
    open by default when total attempts ≤ 20.
  - **TCP Stats** — single collapsible block showing connection count; open by default when ≤ 20 rows.
- **Inline CSS** and **`assets/report.css`** updated with `<details>`/`<summary>` styles
  (`▶`/`▼` indicator, `.grp-lbl`, `.grp-meta` classes).

### Changed
- HTML Statistics Summary now emits one row per `(protocol, payload_size)` group, matching
  the terminal output. The "Protocol" column value becomes e.g. "download 64 KiB".
- Terminal averages table header widened from 9 → 16 chars to accommodate grouped labels.
- Workspace version bumped to `0.5.0` (MINOR — new feature).

---

## [0.4.0] – 2026-02-28 — JSON config file support

### Added
- **`--config` / `-c` flag (both binaries)** — accepts a path to a JSON config file. Any key
  from the file can be overridden by a CLI flag (priority: CLI arg > JSON key > built-in default).
- **`--log-level` flag (both binaries)** — set the `tracing` filter directly (e.g.
  `"debug"`, `"info,tower_http=debug"`). Overrides `--verbose` (tester only) and `RUST_LOG`.
- **`ConfigFile` / `ResolvedConfig` structs in `cli.rs`** — all previously hard-defaulted
  tester fields are now `Option<T>` in the raw `Cli` struct; `Cli::resolve(Option<ConfigFile>)`
  merges CLI + file + built-in defaults into a concrete `ResolvedConfig`.
- **`validate()`, `parsed_modes()`, `parsed_payload_sizes()`** moved to `ResolvedConfig`;
  `validate()` gains an explicit `ipv4_only && ipv6_only` conflict check (catches config-file
  sourced conflicts not covered by clap's `conflicts_with`).
- **`tester.example.json`** — repo-root example file showing every tester key with its default
  value.
- **`endpoint.example.json`** — repo-root example file showing every endpoint key with its
  default value.
- New unit tests: `resolved_defaults`, `config_file_overrides_defaults`,
  `cli_overrides_config_file`.

### Changed
- `Cli` struct field types changed from concrete types with `default_value` annotations to
  `Option<T>` (no observable behaviour change — defaults still apply via `resolve()`).
- Existing tests `defaults_parse`, `validate_save_to_sql_without_conn_string_fails`, and
  `payload_sizes_parsed_via_cli` updated to reflect the new raw/resolved split.
- Workspace version bumped to `0.4.0` (MINOR — new feature).

---

## [0.3.3] – 2026-02-28 — Fix RUST_LOG documentation

### Fixed
- **README `RUST_LOG` example** — `RUST_LOG=tower_http=debug` was documented as the way
  to get verbose HTTP logs, but a target-specific directive alone silently suppresses all
  other log targets (including the endpoint's own startup lines). Corrected to
  `RUST_LOG=info,tower_http=debug` with an explanatory note.

---

## [0.3.2] – 2026-02-28 — Endpoint version banner + request logging

### Added
- **Version banner at startup** — `networker-endpoint` now prints its version (e.g.
  `networker-endpoint v0.3.2`) as the first log line before the listening-address lines.
- **HTTP request/response logging** — `TraceLayer` (from `tower-http`) added to the axum
  router; every request is logged at `INFO` with method + URI, and every response with
  status code + latency. Verbosity is controlled by `RUST_LOG`
  (e.g. `RUST_LOG=info,tower_http=debug` for verbose HTTP spans).

---

## [0.3.1] – 2026-02-28 — webdownload/webupload path rewrite

### Fixed
- **`webdownload` and `webupload` path rewrite** — both probes previously left the URL path
  unchanged (e.g. `/health`), so `webdownload` returned whatever the target endpoint happened
  to respond with (e.g. 114 B health JSON) and `webupload` POSTed to a path that ignored the
  request body. Both probes now rewrite the URL path identically to their non-web counterparts:
  `webdownload` → `GET /download?bytes=N`, `webupload` → `POST /upload`. The `--target` flag
  may point at any path; the host and port are preserved and the path is replaced.
- **`--payload-sizes` now required for `webdownload`** — updated CLI help text to document that
  `webdownload` requires `--payload-sizes` (same as `download`), since it now issues a
  `?bytes=N` request and must have a size to request.

---

## [0.3.0] – 2026-02-28 — Web probes, UDP throughput, statistics

> Starting from this release every PR includes a version bump.
> Standard [Semantic Versioning](https://semver.org/) (`MAJOR.MINOR.PATCH`) is used:
> new features → MINOR bump, bug fixes → PATCH bump.

### Fixed
- **`webdownload` ignored `--payload-sizes`** — the mode previously ran once per cycle
  and GETed the target URL as-is, returning whatever the server happened to send (e.g. 114 B
  for a `/health` endpoint). `webdownload` now expands per payload size exactly like `download`,
  and appends `?bytes=N` to the target URL so that any server that supports the parameter (such
  as `networker-endpoint`'s `/download` route) will stream back the requested number of bytes.
  The actual body bytes received are always used for the throughput calculation.
  `--payload-sizes` is now required for `webdownload` (same as `download`).
- **`webupload` absurd throughput when server ignores the request body** — generic targets
  (e.g. a `/health` endpoint) may respond immediately without draining the POST body, making
  `ttfb_ms` near-zero and the computed throughput physically impossible (e.g. 1.3M MB/s).
  `webupload` now uses a dedicated `patch_webupload_throughput` helper that (a) falls back to
  `total_duration_ms` instead of `ttfb_ms` when no `Server-Timing: recv` header is present,
  and (b) caps results at 100,000 MB/s (≈ 800 Gbps — physically impossible on any real link);
  values above the cap are reported as `null`/`—` instead. Four new unit tests cover the
  server-recv, fallback, implausible, and plausible cases.
- **`webdownload`/`webupload` probes always failed** — `run_probe` in the HTTP runner only
  listed `Http1 | Http2 | Tcp | Download | Upload`; both web-probe variants fell through to the
  `other =>` error arm, returning "Protocol not handled by http runner" on every attempt.
  Added `WebDownload | WebUpload` to both match arms (`run_probe` entry point and the
  `send_http1` dispatch inside `run_http_or_tcp`).
- Clippy `redundant_closure` in `html.rs` (`.map(|b| format_bytes(b))`) and `main.rs`
  (`.filter_map(|a| primary_metric_value(a))`); both replaced with the bare function reference.
- Integration test `ServerConfig` initializer missing `udp_throughput_port` field (added in
  the `udpdownload`/`udpupload` PR but not reflected in the test harness).

### Added
- **`udpdownload` probe mode** — bulk UDP download from `networker-endpoint`'s UDP throughput
  server (default port 9998); measures datagrams sent/received, packet loss %, transfer window
  ms, and throughput MB/s. Requires `--payload-sizes`.
- **`udpupload` probe mode** — bulk UDP upload to `networker-endpoint`'s UDP throughput server;
  server reports bytes actually received (CMD_REPORT) so client-side and server-side counts are
  compared. Requires `--payload-sizes`.
- **UDP throughput protocol** — new custom datagram protocol (`b"NWKT"` magic) over a separate
  port. Control packets: CMD_DOWNLOAD, CMD_UPLOAD, CMD_DONE, CMD_ACK, CMD_REPORT. Data packets
  have 8-byte header (seq_num + total_seqs) + up to 1400-byte payload.
- **`UdpThroughputResult`** — new JSON field on `RequestAttempt`; stores remote_addr,
  payload_bytes, datagrams_sent, datagrams_received, bytes_acked, loss_percent, transfer_ms,
  throughput_mbps.
- **HTML UDP Throughput section** — new card in the report showing all UDP throughput attempts
  with loss %, throughput, and bytes-acked.
- **Excel UDP Throughput sheet** — new sheet in the `.xlsx` report.
- **`networker-endpoint --udp-throughput-port`** — new CLI flag (default 9998) for the bulk
  throughput listener.
- **`networker-tester --udp-throughput-port`** — new CLI flag (default 9998) matching the
  endpoint default.
- **`webdownload` probe mode** — GET the target URL as-is (no endpoint path rewriting),
  measures full HTTP phase timing (DNS, TCP, TLS, TTFB, Total) + response body throughput
  + TCP kernel stats. Works with any HTTP server, not just `networker-endpoint`.
- **`webupload` probe mode** — POST to the target URL with a payload body (requires
  `--payload-sizes`), measures full HTTP phase timing + upload throughput + TCP kernel
  stats. Works with any HTTP server.
- Both new modes appear in the HTML Throughput table, TCP Stats card, All Attempts table,
  and Excel Throughput sheet alongside the existing `download`/`upload` modes.
- **TCP Stats card in HTML report** — new section showing all per-connection kernel
  stats: local→remote addresses, MSS, RTT, RTT variance, min RTT, cwnd, ssthresh,
  retransmits, total retransmits, receive window, segments out/in, delivery rate (MB/s),
  and congestion algorithm.
- **Congestion algorithm** — `TCP_CONGESTION` getsockopt added to Linux and macOS;
  stored as `TcpResult.congestion_algorithm` (e.g. "cubic", "bbr").
- **Delivery rate** — `tcpi_delivery_rate` (Linux ≥ 4.9); bytes/sec stored as
  `TcpResult.delivery_rate_bps`; displayed as MB/s in HTML + Excel.
- **Minimum RTT** — `tcpi_min_rtt` (Linux ≥ 4.9); ms stored as `TcpResult.min_rtt_ms`.
- **segs_out / segs_in** — now populated on Linux ≥ 4.2 (were always `None` previously);
  switched from `libc::tcp_info` struct to raw byte-offset reads so all kernel-version-
  gated fields work without a matching libc struct definition.
- `sql/07_MoreTcpStats.sql` — idempotent `ALTER TABLE` adding `CongestionAlgorithm`,
  `DeliveryRateBps`, `MinRttMs` columns to `dbo.TcpResult`.
- Excel TCP Stats sheet gains **Min RTT ms**, **Delivery MB/s**, **Congestion** columns.
- **Statistics Summary** — per-protocol descriptive statistics (N, Min, Mean, p50, p95, p99,
  Max, StdDev) computed from each run's primary metric (total duration ms for HTTP/TCP, RTT avg
  ms for UDP echo, throughput MB/s for all bulk-transfer modes). Shown in three places: (#35)
  - Terminal: second table printed below the existing averages table.
  - HTML report: new "Statistics Summary" card (between Timing Breakdown and UDP Probe
    Statistics); success % column colour-coded green/amber/red.
  - Excel: new "Statistics" sheet (sheet 2, directly after Summary).
- `metrics.rs`: new public `Stats` struct, `compute_stats()`, `primary_metric_label()`, and
  `primary_metric_value()` functions; 3 new unit tests for the percentile calculations. (#35)

---

## [0.2.5] – 2026-02-28 — Install script fixes

### Fixed
- `install.sh`: revert curl URL from `raw.githubusercontent.com` back to public Gist —
  `raw.githubusercontent.com` returns 404 for private repos without authentication. (#29)

### Added
- `.github/workflows/sync-gist.yml`: auto-patches the Gist via GitHub API whenever
  `install.sh` changes on `main`. Requires `GIST_TOKEN` secret (PAT with `gist` scope). (#29)

---

## [0.2.4] – 2026-02-28 — Versioning display + install hardening

### Added
- `networker-endpoint` emits `X-Networker-Server-Version` on every response via the
  `add_server_timestamp` middleware. (#26)
- `ServerTimingResult` gains `server_version: Option<String>` — captured in JSON per attempt. (#26)
- Terminal summary prints both **Client version** and **Server version** rows. (#26)
- HTML report Run Summary card shows Client and Server version rows. (#26)
- Version logged at tester startup. (#26)
- `CHANGELOG.md` added following Keep a Changelog format. (#27)

### Changed
- Workspace version bumped to `0.2.4` in `Cargo.toml` — cascades to both binaries. (#26)

### Fixed
- Upload throughput showed absurdly high values (millions of MB/s) because `total_duration_ms`
  includes receiving the server's JSON response body — noise unrelated to the upload transfer.
  `ttfb_ms` is now the primary denominator: it starts just before `send_request()` and stops
  when the server sends response headers. Because `networker-endpoint` only responds after
  draining the full body, `ttfb_ms ≈ upload wire time`. Formula: `max(server_recv_ms, ttfb_ms)`. (#25)
- `install.sh`: added `--force` to `cargo install` so every run unconditionally rebuilds the
  binary, preventing a stale binary when cargo's git-SHA cache considers the installed rev
  current. (#28)
- `install.sh`: prints the installed version at the end (e.g. `networker-tester 0.2.4`)
  for immediate confirmation. (#28)

---

## [0.2.3] – 2026-02-27 — Upload throughput: max() guard

### Fixed
- Upload throughput: changed denominator from `server_recv_ms.unwrap_or(total_duration_ms)`
  to `max(server_recv_ms, total_duration_ms)` so the larger (correct) value is always used.
  Prevents near-zero `server_recv_ms` (kernel-buffer race on same-machine connections) from
  producing absurdly high throughput values. (#24)

---

## [0.2.2] – 2026-02-27 — Throughput unit tests

### Added
- Full unit test coverage for all throughput calculation paths in `runner/throughput.rs`. (#23)

---

## [0.2.1] – 2026-02-27 — Upload throughput: server-side timing

### Fixed
- Upload throughput: switched denominator to `server_recv_ms` from `Server-Timing: recv;dur=X`
  header — the time the server spent draining the request body, accurate regardless of network
  path. (#22)

---

## [0.2.0] – 2026-02-27 — Extended metrics: TCP kernel stats, retries, server timing, Excel

### Added
- **TCP kernel stats** — 8 new fields on `TcpResult`: `retransmits`, `total_retrans`, `snd_cwnd`,
  `snd_ssthresh`, `rtt_variance_ms`, `rcv_space`, `segs_out`, `segs_in`.
  - Linux: read via `TCP_INFO` socket option (no root required).
  - macOS: read via `TCP_CONNECTION_INFO` (`tcp_connection_info` struct at `IPPROTO_TCP` opt `0x24`).
- **Application-level retries** — `--retries N` CLI flag; failed probes are retried up to N times;
  `retry_count` field added to `RequestAttempt`.
- **Server timing** — `ServerTimingResult` struct captures `Server-Timing` header fields (`recv`,
  `proc`, `total`), `X-Networker-Server-Timestamp`, clock skew estimate, and echoed
  `X-Networker-Request-Id`.
- **Excel output** — `--excel` CLI flag; generates an `.xlsx` report alongside JSON + HTML using
  `rust_xlsxwriter`. 8 sheets: Summary, HTTP Timings, TCP Stats, TLS Details, UDP Stats,
  Throughput, Server Timing, Errors.
- **Privilege notice** — on Linux without root, a startup message explains which metrics are still
  captured vs. what would require elevated privileges.
- **`networker-endpoint` server timing** — `/download` returns `Server-Timing: proc;dur=X`;
  `/upload` returns `Server-Timing: recv;dur=X` and echoes `X-Networker-Request-Id`.
- **SQL migrations** — `sql/05_ExtendedTcpStats.sql` (8 new `TcpResult` columns + `RetryCount`
  on `RequestAttempt`) and `sql/06_ServerTiming.sql` (`ServerTimingResult` table). (#21)

---

## [0.1.0] – 2026-02-27 — Initial release

### Added
- **Workspace** — Cargo workspace with two crates: `networker-tester` (CLI) and
  `networker-endpoint` (server).
- **Probe modes** — `http1`, `http2`, `tcp`, `udp`, `download`, `upload`;
  HTTP/3 gated behind `--features http3`.
- **Per-phase timing** — DNS, TCP connect, TLS handshake, TTFB, total; measured using raw
  `hyper 1.x` connection APIs.
- **TLS** — `rustls 0.23` with `ring` provider; self-signed cert via `rcgen`; `--insecure` flag.
- **UDP echo** — configurable probe count, RTT percentiles, jitter, loss%.
- **Download/upload throughput probes** — `GET /download?bytes=N` and `POST /upload`.
- **Output formats** — JSON, HTML report (embedded CSS, protocol comparison tables),
  SQL Server via `tiberius`.
- **`networker-endpoint`** — axum-based server; routes: `/health`, `/echo`, `/download`,
  `/upload`, `/delay`, `/headers`, `/status/:code`, `/http-version`, `/info`;
  ALPN HTTP/1.1 + HTTP/2; `Server-Timing` headers; `X-Networker-Request-Id` echo.
- **SQL Server schema** — `dbo.TestRun`, `dbo.RequestAttempt`, `dbo.HttpResult`, `dbo.TcpResult`,
  `dbo.TlsResult`, `dbo.UdpResult`, `dbo.ThroughputResult`, `dbo.ServerTimingResult`;
  stored procedures; sample queries.
- **CI** — GitHub Actions on Ubuntu + Windows; `cargo test`, `cargo fmt --check`, `cargo clippy`.
- **Installation script** — public Gist serves `install.sh`; compiles from private repo via SSH.

---

[Unreleased]: https://github.com/irlm/networker-tester/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/irlm/networker-tester/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/irlm/networker-tester/compare/v0.3.3...v0.4.0
[0.3.3]: https://github.com/irlm/networker-tester/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/irlm/networker-tester/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/irlm/networker-tester/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/irlm/networker-tester/compare/v0.2.5...v0.3.0
[0.2.5]: https://github.com/irlm/networker-tester/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/irlm/networker-tester/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/irlm/networker-tester/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/irlm/networker-tester/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/irlm/networker-tester/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/irlm/networker-tester/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/irlm/networker-tester/releases/tag/v0.1.0
