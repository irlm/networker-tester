use anyhow::Result;

/// Send an email. Uses Azure Communication Services if configured,
/// otherwise logs the email content to stderr (development fallback).
pub async fn send_email(to: &str, subject: &str, body: &str) -> Result<()> {
    let conn_str = std::env::var("DASHBOARD_ACS_CONNECTION_STRING");
    let sender = std::env::var("DASHBOARD_ACS_SENDER");

    match (conn_str, sender) {
        (Ok(conn), Ok(from)) => send_via_acs(&conn, &from, to, subject, body).await,
        _ => {
            // Fallback: log the email for development
            tracing::info!(
                to = %to,
                subject = %subject,
                "EMAIL (ACS not configured — logging instead):\n{body}"
            );
            Ok(())
        }
    }
}

/// Send via Azure Communication Services REST API.
/// POST https://{endpoint}/emails:send?api-version=2023-03-31
async fn send_via_acs(
    conn_str: &str,
    from: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<()> {
    use base64::engine::{general_purpose::STANDARD, Engine};
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};

    type HmacSha256 = Hmac<Sha256>;

    // Parse connection string: "endpoint=https://xxx.communication.azure.com/;accesskey=base64key"
    let (endpoint, access_key) = parse_acs_connection_string(conn_str)?;

    // Build request body
    let request_body = serde_json::json!({
        "senderAddress": from,
        "recipients": {
            "to": [{ "address": to }]
        },
        "content": {
            "subject": subject,
            "plainText": body
        }
    });

    let url = format!("{endpoint}/emails:send?api-version=2023-03-31");
    let body_bytes = serde_json::to_vec(&request_body)?;

    // Compute content hash (SHA-256 of body, base64 encoded)
    let content_hash = STANDARD.encode(Sha256::digest(&body_bytes));

    // Build string to sign per ACS HMAC-SHA256 auth spec
    let date = chrono::Utc::now()
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();
    let url_parsed: url::Url = url.parse()?;
    let path_and_query = format!(
        "{}?{}",
        url_parsed.path(),
        url_parsed.query().unwrap_or("")
    );
    let host = url_parsed.host_str().unwrap_or("");

    let string_to_sign = format!(
        "POST\n{path_and_query}\n{date};{host};{content_hash}"
    );

    // HMAC-SHA256 sign
    let key_bytes = STANDARD.decode(&access_key)?;
    let mut mac = HmacSha256::new_from_slice(&key_bytes)?;
    mac.update(string_to_sign.as_bytes());
    let signature = STANDARD.encode(mac.finalize().into_bytes());

    let auth_header = format!(
        "HMAC-SHA256 SignedHeaders=x-ms-date;host;x-ms-content-sha256&Signature={signature}"
    );

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("x-ms-date", &date)
        .header("x-ms-content-sha256", &content_hash)
        .header("Authorization", &auth_header)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await?;

    if resp.status().is_success() || resp.status().as_u16() == 202 {
        tracing::info!(to = %to, subject = %subject, "Email sent via ACS");
        Ok(())
    } else {
        let status = resp.status();
        let err_body = resp.text().await.unwrap_or_default();
        anyhow::bail!("ACS email failed: HTTP {status} — {err_body}")
    }
}

fn parse_acs_connection_string(conn_str: &str) -> Result<(String, String)> {
    let mut endpoint = String::new();
    let mut access_key = String::new();
    for part in conn_str.split(';') {
        if let Some(val) = part.strip_prefix("endpoint=") {
            endpoint = val.trim_end_matches('/').to_string();
        } else if let Some(val) = part.strip_prefix("accesskey=") {
            access_key = val.to_string();
        }
    }
    if endpoint.is_empty() || access_key.is_empty() {
        anyhow::bail!("Invalid ACS connection string — expected endpoint=...;accesskey=...");
    }
    Ok((endpoint, access_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_acs_connection_string() {
        let (ep, key) = parse_acs_connection_string(
            "endpoint=https://my-acs.communication.azure.com/;accesskey=dGVzdGtleQ==",
        )
        .unwrap();
        assert_eq!(ep, "https://my-acs.communication.azure.com");
        assert_eq!(key, "dGVzdGtleQ==");
    }

    #[test]
    fn test_parse_acs_connection_string_no_trailing_slash() {
        let (ep, key) = parse_acs_connection_string(
            "endpoint=https://my-acs.communication.azure.com;accesskey=abc123==",
        )
        .unwrap();
        assert_eq!(ep, "https://my-acs.communication.azure.com");
        assert_eq!(key, "abc123==");
    }

    #[test]
    fn test_parse_acs_connection_string_invalid() {
        assert!(parse_acs_connection_string("invalid").is_err());
        assert!(parse_acs_connection_string("endpoint=https://foo").is_err());
        assert!(parse_acs_connection_string("accesskey=bar").is_err());
    }
}
