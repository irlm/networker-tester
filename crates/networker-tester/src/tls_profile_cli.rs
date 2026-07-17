//! `--tls-profile-url` CLI mode: one-shot TLS endpoint profile run.

use super::*;

pub(crate) async fn run_tls_profile_cli(cfg: &ResolvedConfig) -> anyhow::Result<()> {
    let url = url::Url::parse(
        cfg.tls_profile_url
            .as_deref()
            .context("--tls-profile-url is required")?,
    )
    .context("parsing --tls-profile-url")?;
    let host = url
        .host_str()
        .context("--tls-profile-url must include a host")?;
    let port = url.port_or_known_default().unwrap_or(443);
    let target_kind = match cfg
        .tls_profile_target_kind
        .as_deref()
        .unwrap_or("external-url")
        .replace('_', "-")
        .as_str()
    {
        "managed-endpoint" => TlsProfileTargetKind::ManagedEndpoint,
        "external-host" => TlsProfileTargetKind::ExternalHost,
        _ => TlsProfileTargetKind::ExternalUrl,
    };
    let req = TlsProfileRequest {
        target_kind,
        source_url: Some(url.to_string()),
        host: host.to_string(),
        port,
        ip_override: cfg
            .tls_profile_ip
            .as_deref()
            .map(str::parse)
            .transpose()
            .context("invalid --tls-profile-ip")?,
        sni_override: cfg.tls_profile_sni.clone(),
        dns_enabled: cfg.dns_enabled,
        ipv4_only: cfg.ipv4_only,
        ipv6_only: cfg.ipv6_only,
        insecure: cfg.insecure,
        ca_bundle: cfg.ca_bundle.clone(),
        timeout_ms: cfg.timeout.saturating_mul(1000).max(1000),
    };

    let profile = run_tls_endpoint_profile(req).await?;
    let tls_profile_project_id = cfg.tls_profile_project_id.clone();
    let out_dir = PathBuf::from(&cfg.output_dir);
    std::fs::create_dir_all(&out_dir).context("Cannot create output directory")?;
    let ts = Utc::now().format("%Y%m%d-%H%M%S");
    let json_path = out_dir.join(format!("tls-profile-{ts}.json"));
    json::save_tls_profile(&profile, &json_path)?;

    if cfg.save_to_db || cfg.save_to_sql {
        let db_url = cfg
            .db_url
            .as_deref()
            .or(cfg.connection_string.as_deref())
            .context(
            "--save-to-db requires --db-url (or legacy --connection-string) for TLS profile runs",
        )?;
        let backend = db::connect(db_url).await?;
        if cfg.db_migrate {
            backend.migrate().await?;
        }
        backend
            .save_tls_profile(&profile, tls_profile_project_id.as_deref())
            .await?;
    }

    if cfg.tls_profile_json {
        println!("{}", json::to_string_tls_profile(&profile)?);
    } else {
        println!("TLS Endpoint Profile");
        println!("--------------------");
        println!("Host: {}:{}", profile.target.host, profile.target.port);
        println!("Status: {}", profile.summary.status);
        println!("JSON: {}", json_path.display());
    }
    Ok(())
}
