/// HTTP/3 over QUIC server for the diagnostics endpoint.
///
/// Binds a Quinn QUIC endpoint on the HTTPS port (UDP) and serves the same
/// core routes as the axum HTTP/1.1+HTTP/2 server:
///
///   GET  /health            – liveness probe
///   GET  /download?bytes=N  – N-byte response + Server-Timing
///   POST /upload            – drain body + Server-Timing
///   GET  /page?assets=N&bytes=B         – page-load JSON manifest (for synthetic probes)
///   GET  /browser-page?assets=N&bytes=B – HTML page with img tags (for real browser probe)
///   GET  /asset?bytes=B     – B-byte asset body
///
/// All responses include the standard `X-Networker-Server-Timestamp` and
/// `X-Networker-Server-Version` headers.
#[cfg(feature = "http3")]
pub mod server {
    use anyhow::Context;
    use bytes::{Buf, Bytes};
    use chrono::Utc;
    use h3_quinn::Connection as H3QuinnConn;
    use http::{Request, Response};
    use quinn::Endpoint;
    use std::io::Cursor;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Instant;
    use tracing::{debug, info, warn};

    // ─────────────────────────────────────────────────────────────────────────
    // Entry point
    // ─────────────────────────────────────────────────────────────────────────

    pub async fn run_h3_server(
        cert_pem: Vec<u8>,
        key_pem: Vec<u8>,
        addr: SocketAddr,
    ) -> anyhow::Result<()> {
        let quinn_cfg =
            build_quinn_server_config(&cert_pem, &key_pem).context("build QUIC server config")?;
        let endpoint = Endpoint::server(quinn_cfg, addr).context("bind QUIC endpoint")?;

        info!(
            "HTTP/3 QUIC  → udp://0.0.0.0:{}  (self-signed, use --insecure)",
            addr.port()
        );

        while let Some(incoming) = endpoint.accept().await {
            tokio::spawn(async move {
                let conn = match incoming.await {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("QUIC accept error: {e}");
                        return;
                    }
                };
                let h3_conn = match h3::server::Connection::new(H3QuinnConn::new(conn)).await {
                    Ok(c) => c,
                    Err(e) => {
                        debug!("H3 connection setup error: {e}");
                        return;
                    }
                };
                handle_connection(h3_conn).await;
            });
        }
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Quinn server config (TLS from PEM)
    // ─────────────────────────────────────────────────────────────────────────

    fn build_quinn_server_config(
        cert_pem: &[u8],
        key_pem: &[u8],
    ) -> anyhow::Result<quinn::ServerConfig> {
        let certs: Vec<_> = rustls_pemfile::certs(&mut Cursor::new(cert_pem))
            .collect::<Result<Vec<_>, _>>()
            .context("parse cert PEM")?;
        let key = rustls_pemfile::private_key(&mut Cursor::new(key_pem))
            .context("parse key PEM")?
            .context("no private key found")?;

        let mut tls_cfg = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .context("build rustls ServerConfig")?;

        tls_cfg.max_early_data_size = u32::MAX;
        tls_cfg.alpn_protocols = vec![b"h3".to_vec()];

        let quic_cfg = quinn::crypto::rustls::QuicServerConfig::try_from(tls_cfg)
            .context("build QuicServerConfig")?;

        Ok(quinn::ServerConfig::with_crypto(Arc::new(quic_cfg)))
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Connection handler
    // ─────────────────────────────────────────────────────────────────────────

    async fn handle_connection(mut conn: h3::server::Connection<H3QuinnConn, Bytes>) {
        loop {
            match conn.accept().await {
                Ok(Some(resolver)) => match resolver.resolve_request().await {
                    Ok((req, stream)) => {
                        tokio::spawn(handle_request(req, stream));
                    }
                    Err(e) => debug!("H3 resolve_request error: {e}"),
                },
                Ok(None) => break,
                Err(e) => {
                    debug!("H3 accept error: {e}");
                    break;
                }
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Request routing
    // ─────────────────────────────────────────────────────────────────────────

    async fn handle_request<T>(req: Request<()>, mut stream: h3::server::RequestStream<T, Bytes>)
    where
        T: h3::quic::BidiStream<Bytes> + Send + 'static,
        T::SendStream: Send,
        T::RecvStream: Send,
    {
        let method = req.method().as_str().to_string();
        let path = req.uri().path().to_string();
        let query = req.uri().query().unwrap_or("").to_string();

        debug!("{method} {path}?{query} (HTTP/3)");

        match (method.as_str(), path.as_str()) {
            ("GET", "/health") => handle_health(&mut stream).await,
            ("GET", "/download") => handle_download(&query, &mut stream).await,
            ("POST", "/upload") => {
                let req_id: Option<String> = req
                    .headers()
                    .get("x-networker-request-id")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_owned());
                handle_upload(req_id, &mut stream).await;
            }
            ("GET", "/page") => handle_page(&query, &mut stream).await,
            ("GET", "/browser-page") => handle_browser_page(&query, &mut stream).await,
            ("GET", "/asset") => handle_asset(&query, &mut stream).await,
            _ => handle_not_found(&mut stream).await,
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Route handlers
    // ─────────────────────────────────────────────────────────────────────────

    async fn handle_health<T: h3::quic::BidiStream<Bytes>>(
        stream: &mut h3::server::RequestStream<T, Bytes>,
    ) {
        let body = serde_json::json!({
            "status": "ok",
            "timestamp": Utc::now().to_rfc3339(),
            "service": "networker-endpoint",
            "version": env!("CARGO_PKG_VERSION"),
        })
        .to_string();
        send_json(stream, 200, body, &[]).await;
    }

    async fn handle_download<T: h3::quic::BidiStream<Bytes>>(
        query: &str,
        stream: &mut h3::server::RequestStream<T, Bytes>,
    ) {
        let n = parse_query_usize(query, "bytes")
            .unwrap_or(1024)
            .min(2 * 1024 * 1024 * 1024);

        let t0 = Instant::now();
        #[cfg(unix)]
        let (csw_v0, csw_i0) = csw_snapshot();

        let body = vec![0u8; n];
        let proc_ms = t0.elapsed().as_secs_f64() * 1000.0;

        #[cfg(unix)]
        let csw_part = {
            let (csw_v1, csw_i1) = csw_snapshot();
            format!(
                ", csw-v;dur={}, csw-i;dur={}",
                csw_v1 - csw_v0,
                csw_i1 - csw_i0
            )
        };
        #[cfg(not(unix))]
        let csw_part = String::new();

        let timing = format!("proc;dur={proc_ms:.3}{csw_part}");
        let ts = Utc::now().to_rfc3339();

        let resp = Response::builder()
            .status(200)
            .header("content-type", "application/octet-stream")
            .header("content-length", n.to_string())
            .header("x-download-bytes", n.to_string())
            .header("server-timing", timing.as_str())
            .header("x-networker-server-timestamp", ts.as_str())
            .header("x-networker-server-version", env!("CARGO_PKG_VERSION"))
            .body(())
            .unwrap();

        if let Err(e) = stream.send_response(resp).await {
            debug!("download send_response: {e}");
            return;
        }
        if let Err(e) = stream.send_data(Bytes::from(body)).await {
            debug!("download send_data: {e}");
            return;
        }
        let _ = stream.finish().await;
    }

    async fn handle_upload<T: h3::quic::BidiStream<Bytes>>(
        request_id: Option<String>,
        stream: &mut h3::server::RequestStream<T, Bytes>,
    ) {
        let t0 = Instant::now();
        #[cfg(unix)]
        let (csw_v0, csw_i0) = csw_snapshot();

        let mut received_bytes: usize = 0;
        while let Ok(Some(data)) = stream.recv_data().await {
            received_bytes += data.remaining();
        }

        let recv_ms = t0.elapsed().as_secs_f64() * 1000.0;

        #[cfg(unix)]
        let csw_part = {
            let (csw_v1, csw_i1) = csw_snapshot();
            format!(
                ", csw-v;dur={}, csw-i;dur={}",
                csw_v1 - csw_v0,
                csw_i1 - csw_i0
            )
        };
        #[cfg(not(unix))]
        let csw_part = String::new();

        let timing = format!("recv;dur={recv_ms:.3}{csw_part}");
        let ts = Utc::now().to_rfc3339();
        let body = serde_json::json!({
            "received_bytes": received_bytes,
            "timestamp": ts,
        })
        .to_string();

        let mut resp_builder = Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .header("server-timing", timing.as_str())
            .header("x-networker-server-timestamp", ts.as_str())
            .header("x-networker-server-version", env!("CARGO_PKG_VERSION"));

        if let Some(ref rid) = request_id {
            resp_builder = resp_builder.header("x-networker-request-id", rid.as_str());
        }

        let resp = resp_builder.body(()).unwrap();

        if let Err(e) = stream.send_response(resp).await {
            debug!("upload send_response: {e}");
            return;
        }
        if let Err(e) = stream.send_data(Bytes::from(body)).await {
            debug!("upload send_data: {e}");
            return;
        }
        let _ = stream.finish().await;
    }

    async fn handle_browser_page<T: h3::quic::BidiStream<Bytes>>(
        query: &str,
        stream: &mut h3::server::RequestStream<T, Bytes>,
    ) {
        let n = parse_query_usize(query, "assets").unwrap_or(20).min(500);
        let b = parse_query_usize(query, "bytes").unwrap_or(10_240);

        let mut html = String::from(
            "<!DOCTYPE html>\n\
             <html><head><title>Networker Page Load Test</title><link rel=\"icon\" href=\"data:,\"></head>\n\
             <body>\n",
        );
        for i in 0..n {
            html.push_str(&format!(
                "<img src=\"/asset?id={i}&bytes={b}\" width=\"1\" height=\"1\" alt=\"\">\n"
            ));
        }
        html.push_str("</body></html>\n");

        let resp = Response::builder()
            .status(200)
            .header("content-type", "text/html; charset=utf-8")
            .header("content-length", html.len().to_string())
            .body(())
            .unwrap();
        if let Err(e) = stream.send_response(resp).await {
            debug!("browser-page send_response: {e}");
            return;
        }
        if let Err(e) = stream.send_data(Bytes::from(html)).await {
            debug!("browser-page send_data: {e}");
            return;
        }
        let _ = stream.finish().await;
    }

    async fn handle_page<T: h3::quic::BidiStream<Bytes>>(
        query: &str,
        stream: &mut h3::server::RequestStream<T, Bytes>,
    ) {
        let n = parse_query_usize(query, "assets").unwrap_or(20).min(500);
        let b = parse_query_usize(query, "bytes").unwrap_or(10_240);
        let assets: Vec<String> = (0..n).map(|i| format!("/asset?id={i}&bytes={b}")).collect();
        let body = serde_json::json!({
            "asset_count": n,
            "asset_bytes": b,
            "assets": assets,
        })
        .to_string();
        send_json(stream, 200, body, &[]).await;
    }

    async fn handle_asset<T: h3::quic::BidiStream<Bytes>>(
        query: &str,
        stream: &mut h3::server::RequestStream<T, Bytes>,
    ) {
        let n = parse_query_usize(query, "bytes")
            .unwrap_or(10_240)
            .min(100 * 1024 * 1024);
        let body = vec![0u8; n];
        let ts = Utc::now().to_rfc3339();

        let resp = Response::builder()
            .status(200)
            .header("content-type", "application/octet-stream")
            .header("content-length", n.to_string())
            .header("x-networker-server-timestamp", ts.as_str())
            .header("x-networker-server-version", env!("CARGO_PKG_VERSION"))
            .body(())
            .unwrap();

        if let Err(e) = stream.send_response(resp).await {
            debug!("asset send_response: {e}");
            return;
        }
        if let Err(e) = stream.send_data(Bytes::from(body)).await {
            debug!("asset send_data: {e}");
            return;
        }
        let _ = stream.finish().await;
    }

    async fn handle_not_found<T: h3::quic::BidiStream<Bytes>>(
        stream: &mut h3::server::RequestStream<T, Bytes>,
    ) {
        send_json(stream, 404, r#"{"error":"not found"}"#.to_string(), &[]).await;
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────────────────────────────────

    async fn send_json<T: h3::quic::BidiStream<Bytes>>(
        stream: &mut h3::server::RequestStream<T, Bytes>,
        status: u16,
        body: String,
        extra_headers: &[(&str, &str)],
    ) {
        let ts = Utc::now().to_rfc3339();
        let mut resp_builder = Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .header("x-networker-server-timestamp", ts.as_str())
            .header("x-networker-server-version", env!("CARGO_PKG_VERSION"));

        for (k, v) in extra_headers {
            resp_builder = resp_builder.header(*k, *v);
        }

        let resp = resp_builder.body(()).unwrap();

        if let Err(e) = stream.send_response(resp).await {
            debug!("send_json send_response: {e}");
            return;
        }
        if let Err(e) = stream.send_data(Bytes::from(body)).await {
            debug!("send_json send_data: {e}");
            return;
        }
        let _ = stream.finish().await;
    }

    /// Parse a single named parameter from a `key=value&...` query string.
    fn parse_query_usize(query: &str, key: &str) -> Option<usize> {
        for part in query.split('&') {
            let mut kv = part.splitn(2, '=');
            if kv.next() == Some(key) {
                return kv.next().and_then(|v| v.parse().ok());
            }
        }
        None
    }

    /// Snapshot voluntary/involuntary context switches (Unix only).
    #[cfg(unix)]
    fn csw_snapshot() -> (i64, i64) {
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
        (u.ru_nvcsw, u.ru_nivcsw)
    }
}
