mod routes;
mod udp_echo;
mod udp_throughput;

use anyhow::Context;
use axum_server::tls_rustls::RustlsConfig;
pub use routes::build_router;
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

    let tls_config = RustlsConfig::from_pem(cert_pem, key_pem)
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

    let router = build_router();

    // Spawn UDP echo
    let udp_handle = tokio::spawn(udp_echo::run_udp_echo(cfg.udp_port));
    // Spawn UDP throughput server
    let udp_tp_handle = tokio::spawn(udp_throughput::run_udp_throughput(cfg.udp_throughput_port));

    // HTTP server
    let http_router = router.clone();
    let http_handle = tokio::spawn(async move {
        axum_server::bind(http_addr)
            .serve(http_router.into_make_service())
            .await
            .expect("HTTP server error");
    });

    // HTTPS server – axum-server handles HTTP/1.1 + HTTP/2 ALPN automatically
    let https_handle = tokio::spawn(async move {
        axum_server::bind_rustls(https_addr, tls_config)
            .serve(router.into_make_service())
            .await
            .expect("HTTPS server error");
    });

    // Wait for shutdown signal
    let _ = shutdown_rx.await;
    http_handle.abort();
    https_handle.abort();
    udp_handle.abort();
    udp_tp_handle.abort();

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

fn generate_self_signed_cert() -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    use rcgen::generate_simple_self_signed;

    // SANs: localhost (DNS), 127.0.0.1, ::1
    let subject_alt_names = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];

    let certified_key = generate_simple_self_signed(subject_alt_names)
        .context("rcgen generate_simple_self_signed")?;

    let cert_pem = certified_key.cert.pem().into_bytes();
    let key_pem = certified_key.key_pair.serialize_pem().into_bytes();

    Ok((cert_pem, key_pem))
}
