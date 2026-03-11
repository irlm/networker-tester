mod http3_server;
mod routes;
mod udp_echo;
mod udp_throughput;

use anyhow::Context;
use axum_server::accept::NoDelayAcceptor;
use axum_server::tls_rustls::{RustlsAcceptor, RustlsConfig};
pub use routes::{build_router, AppState, SystemMeta};
use std::net::SocketAddr;
use tokio::sync::oneshot;
use tracing::info;

// ─────────────────────────────────────────────────────────────────────────────
// Public configuration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub http_port: u16,
    pub https_port: u16,
    pub udp_port: u16,
    /// UDP bulk throughput server port (udpdownload / udpupload probes).
    pub udp_throughput_port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            http_port: 8080,
            https_port: 8443,
            udp_port: 9999,
            udp_throughput_port: 9998,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Server startup
// ─────────────────────────────────────────────────────────────────────────────

/// Run both HTTP and HTTPS servers plus the UDP echo listener until the
/// process exits or `shutdown_rx` fires.
pub async fn run_with_shutdown(
    cfg: ServerConfig,
    shutdown_rx: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    // rustls 0.23 requires a global CryptoProvider; install ring if none set.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let (cert_pem, key_pem) = generate_self_signed_cert().context("generate self-signed cert")?;

    let tls_config = RustlsConfig::from_pem(cert_pem.clone(), key_pem.clone())
        .await
        .context("Build RustlsConfig")?;

    let http_addr = SocketAddr::from(([0, 0, 0, 0], cfg.http_port));
    let https_addr = SocketAddr::from(([0, 0, 0, 0], cfg.https_port));

    info!("networker-endpoint v{}", env!("CARGO_PKG_VERSION"));
    info!("HTTP  → http://0.0.0.0:{}", cfg.http_port);
    info!(
        "HTTPS → https://0.0.0.0:{}  (self-signed, use --insecure)",
        cfg.https_port
    );
    info!("UDP echo       → 0.0.0.0:{}", cfg.udp_port);
    info!("UDP throughput → 0.0.0.0:{}", cfg.udp_throughput_port);

    // Pass the QUIC port so the router can advertise H3 via Alt-Svc headers.
    // Chrome only acts on Alt-Svc from HTTPS origins, so HTTP clients ignore it.
    #[cfg(feature = "http3")]
    let h3_port = Some(cfg.https_port);
    #[cfg(not(feature = "http3"))]
    let h3_port: Option<u16> = None;

    let system_meta = SystemMeta::collect();
    info!(
        "System: {} {} | {} cores | {} MB RAM | {}",
        system_meta.os,
        system_meta.arch,
        system_meta.cpu_cores,
        system_meta.total_memory_mb.unwrap_or(0),
        system_meta.os_version.as_deref().unwrap_or("unknown"),
    );

    let state = AppState {
        h3_port,
        http_port: cfg.http_port,
        https_port: cfg.https_port,
        udp_port: cfg.udp_port,
        udp_throughput_port: cfg.udp_throughput_port,
        started_at: std::time::Instant::now(),
        system_meta,
    };

    let router = build_router(state);

    // Spawn UDP echo
    let udp_handle = tokio::spawn(udp_echo::run_udp_echo(cfg.udp_port));
    // Spawn UDP throughput server
    let udp_tp_handle = tokio::spawn(udp_throughput::run_udp_throughput(cfg.udp_throughput_port));

    // HTTP server — NoDelayAcceptor sets TCP_NODELAY on every accepted socket,
    // preventing 40 ms Nagle + delayed-ACK stalls during the HTTP/2 handshake.
    let http_router = router.clone();
    let http_handle = tokio::spawn(async move {
        axum_server::bind(http_addr)
            .acceptor(NoDelayAcceptor)
            .serve(http_router.into_make_service())
            .await
            .expect("HTTP server error");
    });

    // HTTPS server – axum-server handles HTTP/1.1 + HTTP/2 ALPN automatically.
    // Chain NoDelayAcceptor inside RustlsAcceptor so TCP_NODELAY is set before
    // the TLS handshake begins.
    let https_handle = tokio::spawn(async move {
        let acceptor = RustlsAcceptor::new(tls_config).acceptor(NoDelayAcceptor);
        axum_server::Server::bind(https_addr)
            .acceptor(acceptor)
            .serve(router.into_make_service())
            .await
            .expect("HTTPS server error");
    });

    // HTTP/3 QUIC server – same UDP port as HTTPS, sharing the self-signed cert
    #[cfg(feature = "http3")]
    let h3_handle = tokio::spawn(async move {
        if let Err(e) = http3_server::server::run_h3_server(cert_pem, key_pem, https_addr).await {
            tracing::error!("HTTP/3 server error: {e:#}");
        }
    });

    // Wait for shutdown signal
    let _ = shutdown_rx.await;
    http_handle.abort();
    https_handle.abort();
    udp_handle.abort();
    udp_tp_handle.abort();
    #[cfg(feature = "http3")]
    h3_handle.abort();

    Ok(())
}

/// Run forever (used by the binary).
pub async fn run(cfg: ServerConfig) -> anyhow::Result<()> {
    let (_tx, rx) = oneshot::channel();
    // This awaits the (never-resolving) shutdown channel, keeping the server alive.
    tokio::select! {
        res = run_with_shutdown(cfg, rx) => res,
        _ = tokio::signal::ctrl_c() => {
            info!("Ctrl-C received, shutting down");
            Ok(())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Self-signed TLS certificate (rcgen 0.13)
// ─────────────────────────────────────────────────────────────────────────────

/// Detect the primary non-loopback LAN IP by routing to an external address.
/// Uses the UDP socket trick (no packets are sent — just reads the routing table).
fn get_primary_local_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    let ip = addr.ip();
    if ip.is_loopback() {
        None
    } else {
        Some(ip.to_string())
    }
}

fn generate_self_signed_cert() -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};

    // SANs: localhost (DNS), 127.0.0.1, ::1, and the primary LAN IP (if any).
    // rcgen 0.13 auto-detects IP strings and creates IP SANs; DNS strings get DNS SANs.
    // Including the LAN IP means the cert is valid for remote clients (e.g. 172.16.x.x)
    // without needing SPKI-pin to bypass a hostname-mismatch error.
    let mut subject_alt_names = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    if let Some(lan_ip) = get_primary_local_ip() {
        if !subject_alt_names.contains(&lan_ip) {
            subject_alt_names.push(lan_ip);
        }
    }

    // Use CertificateParams directly so we can set `is_ca = CA`.
    // A cert with basicConstraints: CA:TRUE can be used as a trust anchor by
    // Chrome's cert verifier (both the macOS Security Framework path and
    // Chrome's built-in BoringSSL QUIC path).  A plain leaf cert (no CA flag)
    // is silently rejected as a trust root even when added to the macOS Keychain
    // or Linux NSS db, causing QUIC TLS to fail with CERTIFICATE_VERIFY_FAILED.
    let mut params =
        CertificateParams::new(subject_alt_names).context("rcgen CertificateParams::new")?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);

    let key_pair = KeyPair::generate().context("rcgen KeyPair::generate")?;
    let cert = params
        .self_signed(&key_pair)
        .context("rcgen CertificateParams::self_signed")?;

    let cert_pem = cert.pem().into_bytes();
    let key_pem = key_pair.serialize_pem().into_bytes();

    Ok((cert_pem, key_pem))
}

// ─────────────────────────────────────────────────────────────────────────────
// Static site generation (for nginx / IIS comparison)
// ─────────────────────────────────────────────────────────────────────────────

/// Preset asset sizes matching the tester's page-load presets.
pub fn resolve_preset(name: &str) -> anyhow::Result<Vec<usize>> {
    match name {
        "mixed" => {
            let mut v = vec![512; 5];
            v.extend(vec![2_048; 8]);
            v.extend(vec![8_192; 7]);
            v.extend(vec![25_600; 7]);
            v.extend(vec![51_200; 6]);
            v.extend(vec![102_400; 5]);
            v.extend(vec![204_800; 4]);
            v.extend(vec![409_600; 4]);
            v.extend(vec![614_400; 2]);
            v.extend(vec![1_048_576; 2]);
            Ok(v)
        }
        "small" => Ok(vec![1_024; 5]),
        "default" => {
            let mut v = vec![1_024; 10];
            v.extend(vec![5_120; 8]);
            v.extend(vec![10_240; 5]);
            v.extend(vec![51_200; 3]);
            v.extend(vec![102_400; 2]);
            v.extend(vec![204_800; 2]);
            Ok(v)
        }
        other => Err(anyhow::anyhow!(
            "Unknown preset '{other}'. Valid: small, default, mixed"
        )),
    }
}

/// Generate a static website in `dir` that nginx/IIS can serve.
///
/// Creates:
/// - `index.html` with `<img>` tags referencing `asset-{i}.bin` files
/// - `style.css` with minimal styling
/// - `asset-{i}.bin` files at the sizes defined by the preset
/// - `health` file for stack detection
pub fn generate_static_site(
    dir: &std::path::Path,
    preset: &str,
    stack_name: &str,
) -> anyhow::Result<()> {
    use std::fmt::Write;
    use std::fs;

    let sizes = resolve_preset(preset)?;
    fs::create_dir_all(dir)?;

    // Generate asset files (zero-filled)
    for (i, &size) in sizes.iter().enumerate() {
        fs::write(dir.join(format!("asset-{i}.bin")), vec![0u8; size])?;
    }

    // Generate style.css
    fs::write(
        dir.join("style.css"),
        "body{margin:0;font-family:system-ui,sans-serif;background:#f5f5f5}\n\
         img{display:block;width:1px;height:1px;position:absolute}\n",
    )?;

    // Generate index.html
    let mut html = String::with_capacity(4096);
    html.push_str(
        "<!DOCTYPE html>\n\
         <html><head><title>Networker Page Load Test</title>\n\
         <link rel=\"stylesheet\" href=\"style.css\">\n\
         <link rel=\"icon\" href=\"data:,\">\n\
         </head><body>\n",
    );
    for i in 0..sizes.len() {
        let _ = writeln!(
            html,
            "<img src=\"asset-{i}.bin\" width=\"1\" height=\"1\" alt=\"\">"
        );
    }
    html.push_str("</body></html>\n");
    fs::write(dir.join("index.html"), &html)?;

    // Generate health endpoint (JSON file)
    let health = format!(
        "{{\"status\":\"ok\",\"service\":\"networker-static\",\"stack\":\"{stack_name}\"}}\n"
    );
    fs::write(dir.join("health"), &health)?;

    let total_bytes: usize = sizes.iter().sum();
    info!(
        "Generated static site: {} assets, {:.1} MB total, preset={preset}, stack={stack_name}",
        sizes.len(),
        total_bytes as f64 / 1_048_576.0
    );
    Ok(())
}
