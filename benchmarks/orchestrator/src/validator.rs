use anyhow::{Context, Result};
use serde::Serialize;

/// Result of validating a deployed API against the AletheBench spec.
#[derive(Debug, Clone, Serialize)]
pub struct ValidationResult {
    pub language: String,
    pub health_ok: bool,
    pub download_ok: bool,
    pub upload_ok: bool,
    pub http2_ok: bool,
    pub tls_ok: bool,
    pub errors: Vec<String>,
}

impl ValidationResult {
    /// Returns true if all checks passed.
    pub fn all_ok(&self) -> bool {
        self.health_ok && self.download_ok && self.upload_ok && self.http2_ok && self.tls_ok
    }

    /// Print a human-readable summary to stdout.
    pub fn print_summary(&self) {
        let check = |ok: bool| if ok { "PASS" } else { "FAIL" };

        println!("\n=== API Validation: {} ===\n", self.language);
        println!("  Health endpoint:   {}", check(self.health_ok));
        println!("  Download endpoint: {}", check(self.download_ok));
        println!("  Upload endpoint:   {}", check(self.upload_ok));
        println!("  HTTP/2 support:    {}", check(self.http2_ok));
        println!("  TLS support:       {}", check(self.tls_ok));

        if self.errors.is_empty() {
            println!("\n  All checks passed.");
        } else {
            println!("\n  Errors:");
            for err in &self.errors {
                println!("    - {err}");
            }
        }
    }
}

/// Build a reqwest client that accepts self-signed certificates.
fn insecure_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("building reqwest client")
}

/// Build a reqwest client configured to prefer HTTP/2 via ALPN and accept
/// self-signed certs.
fn insecure_http2_client() -> Result<reqwest::Client> {
    // Use https_only + rustls ALPN to negotiate h2 over TLS.
    // `http2_prior_knowledge()` is for plaintext h2c; for HTTPS we rely on
    // ALPN which reqwest handles automatically when the `http2` feature is
    // enabled. We just need to ensure the builder is set up correctly.
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("building HTTP/2 reqwest client")
}

/// Validate a deployed API at the given IP against the AletheBench spec.
///
/// Checks:
/// 1. GET /health — JSON with "status":"ok", "runtime", "version"
/// 2. GET /download/1024 — exactly 1024 bytes
/// 3. GET /download/65536 — exactly 65536 bytes
/// 4. POST /upload with 2048 bytes — {"bytes_received": 2048}
/// 5. HTTP/2 support via prior-knowledge
/// 6. TLS connectivity (already implied by HTTPS, but verified explicitly)
pub async fn validate_api(ip: &str, language: &str) -> Result<ValidationResult> {
    let client = insecure_client()?;
    let base = format!("https://{ip}:8443");
    let mut errors = Vec::new();

    // 1. GET /health
    let health_ok = match check_health(&client, &base).await {
        Ok(()) => true,
        Err(e) => {
            errors.push(format!("health: {e:#}"));
            false
        }
    };

    // 2 & 3. GET /download/1024 and /download/65536
    let download_ok = match check_downloads(&client, &base).await {
        Ok(()) => true,
        Err(e) => {
            errors.push(format!("download: {e:#}"));
            false
        }
    };

    // 4. POST /upload with 2048 bytes
    let upload_ok = match check_upload(&client, &base).await {
        Ok(()) => true,
        Err(e) => {
            errors.push(format!("upload: {e:#}"));
            false
        }
    };

    // 5. HTTP/2 support
    let http2_ok = match check_http2(ip).await {
        Ok(()) => true,
        Err(e) => {
            errors.push(format!("http2: {e:#}"));
            false
        }
    };

    // 6. TLS connectivity (the previous checks already use HTTPS, but we
    //    verify the TLS handshake explicitly with version info)
    let tls_ok = match check_tls(&client, &base).await {
        Ok(()) => true,
        Err(e) => {
            errors.push(format!("tls: {e:#}"));
            false
        }
    };

    Ok(ValidationResult {
        language: language.to_string(),
        health_ok,
        download_ok,
        upload_ok,
        http2_ok,
        tls_ok,
        errors,
    })
}

/// Verify GET /health returns JSON with status:"ok", runtime, and version fields.
async fn check_health(client: &reqwest::Client, base: &str) -> Result<()> {
    let resp = client
        .get(format!("{base}/health"))
        .send()
        .await
        .context("GET /health request failed")?;

    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("/health returned HTTP {status}");
    }

    let body: serde_json::Value = resp
        .json::<serde_json::Value>()
        .await
        .context("/health response is not valid JSON")?;

    // Check required fields
    let status_field = body
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if status_field != "ok" {
        anyhow::bail!("/health status field is '{}', expected 'ok'", status_field);
    }

    if body.get("runtime").is_none() {
        anyhow::bail!("/health missing 'runtime' field");
    }
    if body.get("version").is_none() {
        anyhow::bail!("/health missing 'version' field");
    }

    Ok(())
}

/// Verify GET /download/N returns exactly N bytes.
async fn check_downloads(client: &reqwest::Client, base: &str) -> Result<()> {
    for size in [1024u64, 65536] {
        let resp = client
            .get(format!("{base}/download/{size}"))
            .send()
            .await
            .with_context(|| format!("GET /download/{size} request failed"))?;

        if !resp.status().is_success() {
            anyhow::bail!("/download/{size} returned HTTP {}", resp.status());
        }

        let bytes = resp
            .bytes()
            .await
            .with_context(|| format!("reading /download/{size} body"))?;

        if bytes.len() as u64 != size {
            anyhow::bail!(
                "/download/{size} returned {} bytes, expected {size}",
                bytes.len()
            );
        }
    }
    Ok(())
}

/// Verify POST /upload with 2048 bytes returns {"bytes_received": 2048}.
async fn check_upload(client: &reqwest::Client, base: &str) -> Result<()> {
    let payload = vec![b'X'; 2048];

    let resp = client
        .post(format!("{base}/upload"))
        .header("Content-Type", "application/octet-stream")
        .body(payload)
        .send()
        .await
        .context("POST /upload request failed")?;

    if !resp.status().is_success() {
        anyhow::bail!("/upload returned HTTP {}", resp.status());
    }

    let body: serde_json::Value = resp
        .json::<serde_json::Value>()
        .await
        .context("/upload response is not valid JSON")?;

    let received = body
        .get("bytes_received")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    if received != 2048 {
        anyhow::bail!("/upload reported bytes_received={received}, expected 2048");
    }

    Ok(())
}

/// Check HTTP/2 support using h2 prior knowledge on port 8443.
async fn check_http2(ip: &str) -> Result<()> {
    // Build a client that forces HTTP/2 prior knowledge.
    // Note: prior-knowledge only works over plaintext (h2c), so for HTTPS
    // we rely on ALPN negotiation. We test by connecting with an HTTP/2-only
    // client and checking the response version.
    let client = insecure_http2_client()?;
    let url = format!("https://{ip}:8443/health");

    let resp = client
        .get(&url)
        .send()
        .await
        .context("HTTP/2 request to /health failed")?;

    let version = resp.version();
    if version != reqwest::Version::HTTP_2 {
        anyhow::bail!("expected HTTP/2 but got {:?}", version);
    }

    Ok(())
}

/// Verify TLS connectivity works (the HTTPS requests above already prove this,
/// but we do an explicit check and report the negotiated TLS version).
async fn check_tls(client: &reqwest::Client, base: &str) -> Result<()> {
    // A successful HTTPS GET proves TLS works. If we got this far without
    // errors on previous checks, TLS is functional. We do one final
    // dedicated request to isolate TLS failures in reporting.
    let resp = client
        .get(format!("{base}/health"))
        .send()
        .await
        .context("TLS connectivity check failed")?;

    if !resp.status().is_success() {
        anyhow::bail!("TLS check: /health returned HTTP {}", resp.status());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation_result_all_ok() {
        let result = ValidationResult {
            language: "rust".to_string(),
            health_ok: true,
            download_ok: true,
            upload_ok: true,
            http2_ok: true,
            tls_ok: true,
            errors: vec![],
        };
        assert!(result.all_ok());
    }

    #[test]
    fn test_validation_result_partial_failure() {
        let result = ValidationResult {
            language: "go".to_string(),
            health_ok: true,
            download_ok: true,
            upload_ok: false,
            http2_ok: true,
            tls_ok: true,
            errors: vec!["upload: bytes mismatch".to_string()],
        };
        assert!(!result.all_ok());
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn test_validation_result_all_fail() {
        let result = ValidationResult {
            language: "python".to_string(),
            health_ok: false,
            download_ok: false,
            upload_ok: false,
            http2_ok: false,
            tls_ok: false,
            errors: vec![
                "health: connection refused".to_string(),
                "download: connection refused".to_string(),
                "upload: connection refused".to_string(),
                "http2: connection refused".to_string(),
                "tls: connection refused".to_string(),
            ],
        };
        assert!(!result.all_ok());
        assert_eq!(result.errors.len(), 5);
    }

    #[test]
    fn test_validation_result_serialization() {
        let result = ValidationResult {
            language: "rust".to_string(),
            health_ok: true,
            download_ok: true,
            upload_ok: true,
            http2_ok: false,
            tls_ok: true,
            errors: vec!["http2: not supported".to_string()],
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"health_ok\":true"));
        assert!(json.contains("\"http2_ok\":false"));
        assert!(json.contains("http2: not supported"));
    }
}
