use anyhow::Context;
use chrono::{DateTime, Utc};
use rustls::pki_types::{CertificateDer, ServerName};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use x509_parser::extensions::{GeneralName, ParsedExtension};
use x509_parser::prelude::*;

use crate::runner::tls::load_ca_bundle;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TlsProfileTargetKind {
    ManagedEndpoint,
    ExternalUrl,
    ExternalHost,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TlsProfileCoverageLevel {
    FullControl,
    ClientObserved,
    BestEffort,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TlsPathClassification {
    Direct,
    IndirectExpected,
    IndirectSuspicious,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsEndpointProfile {
    pub target_kind: TlsProfileTargetKind,
    pub coverage_level: TlsProfileCoverageLevel,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported_checks: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
    pub target: TlsProfileTarget,
    pub path_characteristics: TlsPathCharacteristics,
    pub connectivity: TlsProfileConnectivity,
    pub certificate: TlsCertificateSection,
    pub trust: TlsTrustSection,
    pub capabilities: Option<serde_json::Value>,
    pub resumption: TlsResumptionSection,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<TlsFinding>,
    pub summary: TlsProfileSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsProfileTarget {
    pub host: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_ips: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsPathCharacteristics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connected_ip: Option<String>,
    pub direct_ip_match: bool,
    pub proxy_detected: bool,
    pub classification: TlsPathClassification,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsProfileConnectivity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_connect_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_handshake_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negotiated_tls_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negotiated_cipher_suite: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negotiated_key_exchange_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpn: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsCertificateSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf: Option<TlsCertificateInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chain: Vec<TlsCertificateInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsCertificateInfo {
    pub subject: String,
    pub issuer: String,
    pub serial_number: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub san_dns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub san_ip: Vec<String>,
    pub key_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_bits: Option<u32>,
    pub signature_algorithm: String,
    pub is_ca: bool,
    pub sha256_fingerprint: String,
    pub spki_sha256: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ocsp_urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crl_urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aia_issuers: Vec<String>,
    pub must_staple: bool,
    pub scts_present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsTrustSection {
    pub hostname_matches: bool,
    pub chain_valid: bool,
    pub trusted_by_system_store: bool,
    pub verification_performed: bool,
    pub chain_presented: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_chain_depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<String>,
    pub chain_diagnostics: TlsChainDiagnostics,
    pub revocation: TlsRevocationInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caa: Option<TlsCaaInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsRevocationInfo {
    pub ocsp_stapled: bool,
    pub method: String,
    pub status: String,
    pub ocsp_urls: Vec<String>,
    pub crl_urls: Vec<String>,
    pub online_check_attempted: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsChainDiagnostics {
    pub presented_chain_length: u32,
    pub leaf_self_signed: bool,
    pub has_intermediate: bool,
    pub ordered_subject_issuer_links: bool,
    pub root_included: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsCaaInfo {
    pub present: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsResumptionSection {
    pub supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_handshake_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_handshake_ms: Option<f64>,
    pub early_data_offered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub early_data_accepted: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsFinding {
    pub severity: TlsFindingSeverity,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TlsFindingSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsProfileSummary {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct TlsProfileRequest {
    pub target_kind: TlsProfileTargetKind,
    pub source_url: Option<String>,
    pub host: String,
    pub port: u16,
    pub ip_override: Option<IpAddr>,
    pub sni_override: Option<String>,
    pub dns_enabled: bool,
    pub ipv4_only: bool,
    pub ipv6_only: bool,
    pub insecure: bool,
    pub ca_bundle: Option<String>,
    pub timeout_ms: u64,
}

pub async fn run_tls_endpoint_profile(
    req: TlsProfileRequest,
) -> anyhow::Result<TlsEndpointProfile> {
    let mut limitations = vec![
        "Result based on client-visible handshake and DNS behavior".to_string(),
        "Capabilities matrix is not implemented in Phase 1".to_string(),
    ];
    let unsupported_checks = vec![
        "protocol_matrix".to_string(),
        "cipher_matrix".to_string(),
        "deep_mtls_behavior".to_string(),
        "server_side_policy_validation".to_string(),
    ];
    let coverage_level = match req.target_kind {
        TlsProfileTargetKind::ManagedEndpoint => TlsProfileCoverageLevel::BestEffort,
        TlsProfileTargetKind::ExternalUrl | TlsProfileTargetKind::ExternalHost => {
            TlsProfileCoverageLevel::ClientObserved
        }
    };

    let resolved_ips = if req.dns_enabled {
        let addrs = tokio::net::lookup_host((req.host.as_str(), req.port))
            .await
            .with_context(|| format!("resolve host '{}'", req.host))?;
        let mut uniq = Vec::<IpAddr>::new();
        for addr in addrs {
            let ip = addr.ip();
            if req.ipv4_only && !ip.is_ipv4() {
                continue;
            }
            if req.ipv6_only && !ip.is_ipv6() {
                continue;
            }
            if !uniq.contains(&ip) {
                uniq.push(ip);
            }
        }
        uniq
    } else {
        vec![]
    };

    let connect_ip = req
        .ip_override
        .or_else(|| resolved_ips.first().copied())
        .context("no target IP available for TLS profile")?;
    let resolved_ip_strings = resolved_ips
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let direct_ip_match = req
        .ip_override
        .map(|ip| resolved_ips.is_empty() || resolved_ips.contains(&ip))
        .unwrap_or(true);

    let sni = req.sni_override.clone().unwrap_or_else(|| req.host.clone());
    let proxy_detected = std::env::var_os("HTTPS_PROXY").is_some()
        || std::env::var_os("https_proxy").is_some()
        || std::env::var_os("HTTP_PROXY").is_some()
        || std::env::var_os("http_proxy").is_some();
    if proxy_detected {
        limitations.push("Proxy environment variables may influence path characteristics".into());
    }
    if req.insecure {
        limitations.push("Certificate verification was disabled with --insecure; trust results are observational only".into());
    }

    let addr = SocketAddr::new(connect_ip, req.port);
    let first = run_single_handshake(&req, addr, &sni).await?;
    let hostname_matches = first
        .leaf
        .as_ref()
        .map(|leaf| cert_matches_hostname(leaf, &req.host))
        .unwrap_or(false);
    let caa = lookup_caa(&req.host).await;
    let resumption = run_resumption_check(&req, addr, &sni)
        .await
        .unwrap_or(TlsResumptionSection {
            supported: false,
            method: None,
            initial_handshake_ms: None,
            resumed_handshake_ms: None,
            early_data_offered: false,
            early_data_accepted: None,
        });

    let mut trust_issues = first.chain_diagnostics.notes.clone();
    if !hostname_matches {
        trust_issues.push("hostname mismatch".into());
    }
    if first.verification_performed && !first.trusted_by_system_store {
        trust_issues.push("not trusted by system store".into());
    }
    if !first.verification_performed {
        trust_issues.push("system trust verification was not performed".into());
    }
    if first.chain.is_empty() {
        trust_issues.push("server did not present a certificate chain".into());
    }

    let mut evidence = Vec::new();
    if req.ip_override.is_some() && !direct_ip_match {
        evidence.push("requested IP override does not match resolved host IP set".into());
    }
    if proxy_detected {
        evidence.push("proxy environment variables detected".into());
    }
    let classification = if proxy_detected {
        TlsPathClassification::IndirectExpected
    } else if req.ip_override.is_some() && !direct_ip_match {
        TlsPathClassification::IndirectSuspicious
    } else {
        TlsPathClassification::Direct
    };

    let chain_presented = !first.chain.is_empty();
    let mut findings = build_findings(
        &first,
        hostname_matches,
        &resumption,
        &classification,
        req.insecure,
    );
    if first.revocation.ocsp_stapled {
        findings.push(TlsFinding {
            severity: TlsFindingSeverity::Info,
            code: "OCSP_STAPLED".into(),
            message: "OCSP stapling observed".into(),
        });
    }
    let summary = summarize_findings(&findings);

    Ok(TlsEndpointProfile {
        target_kind: req.target_kind,
        coverage_level,
        unsupported_checks,
        limitations,
        target: TlsProfileTarget {
            host: req.host,
            port: req.port,
            requested_ip: req.ip_override.map(|ip| ip.to_string()),
            sni: Some(sni),
            resolved_ips: resolved_ip_strings,
            source_url: req.source_url,
        },
        path_characteristics: TlsPathCharacteristics {
            connected_ip: Some(connect_ip.to_string()),
            direct_ip_match,
            proxy_detected,
            classification,
            evidence,
        },
        connectivity: TlsProfileConnectivity {
            tcp_connect_ms: Some(first.tcp_connect_ms),
            tls_handshake_ms: Some(first.tls_handshake_ms),
            negotiated_tls_version: first.negotiated_tls_version,
            negotiated_cipher_suite: first.negotiated_cipher_suite,
            negotiated_key_exchange_group: None,
            alpn: first.alpn,
        },
        certificate: TlsCertificateSection {
            leaf: first.leaf,
            chain: first.chain,
        },
        trust: TlsTrustSection {
            hostname_matches,
            chain_valid: first.chain_valid,
            trusted_by_system_store: first.trusted_by_system_store,
            verification_performed: first.verification_performed,
            chain_presented,
            verified_chain_depth: first.verified_chain_depth,
            issues: trust_issues,
            chain_diagnostics: first.chain_diagnostics,
            revocation: first.revocation,
            caa,
        },
        capabilities: None,
        resumption,
        findings,
        summary,
    })
}

struct HandshakeSnapshot {
    tcp_connect_ms: f64,
    tls_handshake_ms: f64,
    negotiated_tls_version: Option<String>,
    negotiated_cipher_suite: Option<String>,
    alpn: Option<String>,
    leaf: Option<TlsCertificateInfo>,
    chain: Vec<TlsCertificateInfo>,
    chain_valid: bool,
    trusted_by_system_store: bool,
    verification_performed: bool,
    verified_chain_depth: Option<u32>,
    chain_diagnostics: TlsChainDiagnostics,
    revocation: TlsRevocationInfo,
}

async fn run_single_handshake(
    req: &TlsProfileRequest,
    addr: SocketAddr,
    sni: &str,
) -> anyhow::Result<HandshakeSnapshot> {
    let server_name = ServerName::try_from(sni.to_string()).map_err(|e| anyhow::anyhow!(e))?;
    let tls_config = build_tls_config(req.insecure, req.ca_bundle.as_deref())?;
    let connector = TlsConnector::from(Arc::new(tls_config));

    let t_tcp = Instant::now();
    let tcp_stream = tokio::time::timeout(
        std::time::Duration::from_millis(req.timeout_ms),
        TcpStream::connect(addr),
    )
    .await
    .context("timed out connecting TCP socket for TLS profile")??;
    let tcp_connect_ms = t_tcp.elapsed().as_secs_f64() * 1000.0;

    let t_tls = Instant::now();
    let tls_stream = tokio::time::timeout(
        std::time::Duration::from_millis(req.timeout_ms),
        connector.connect(server_name, tcp_stream),
    )
    .await
    .context("timed out during TLS handshake for TLS profile")??;
    let tls_handshake_ms = t_tls.elapsed().as_secs_f64() * 1000.0;

    let (_, conn) = tls_stream.get_ref();
    let negotiated_tls_version = conn.protocol_version().map(|v| format!("{v:?}"));
    let negotiated_cipher_suite = conn
        .negotiated_cipher_suite()
        .map(|c| format!("{:?}", c.suite()));
    let alpn = conn
        .alpn_protocol()
        .and_then(|b| std::str::from_utf8(b).ok())
        .map(str::to_string);

    let certs = conn
        .peer_certificates()
        .map(|c| c.to_vec())
        .unwrap_or_default();
    let chain = certs
        .iter()
        .filter_map(|c| parse_certificate_info(c))
        .collect::<Vec<_>>();
    let leaf = chain.first().cloned();
    let chain_diagnostics = build_chain_diagnostics(&chain);
    let verification_performed = !req.insecure;
    let trusted_by_system_store = verification_performed && !certs.is_empty();
    let chain_valid =
        verification_performed && !chain_diagnostics.leaf_self_signed && !chain.is_empty();
    let verified_chain_depth = if trusted_by_system_store {
        Some(chain.len() as u32)
    } else {
        None
    };
    let revocation = leaf
        .as_ref()
        .map(|leaf| TlsRevocationInfo {
            ocsp_stapled: false,
            method: "best_effort".into(),
            status: if leaf.ocsp_urls.is_empty() && leaf.crl_urls.is_empty() {
                "no_metadata".into()
            } else {
                "unknown".into()
            },
            ocsp_urls: leaf.ocsp_urls.clone(),
            crl_urls: leaf.crl_urls.clone(),
            online_check_attempted: false,
            notes: if leaf.ocsp_urls.is_empty() && leaf.crl_urls.is_empty() {
                vec!["No OCSP/CRL endpoints advertised by leaf certificate".into()]
            } else {
                vec!["Revocation metadata collected; online status not actively validated in Phase 2".into()]
            },
        })
        .unwrap_or(TlsRevocationInfo {
            ocsp_stapled: false,
            method: "best_effort".into(),
            status: "unknown".into(),
            ocsp_urls: vec![],
            crl_urls: vec![],
            online_check_attempted: false,
            notes: vec!["No peer certificate available".into()],
        });

    Ok(HandshakeSnapshot {
        tcp_connect_ms,
        tls_handshake_ms,
        negotiated_tls_version,
        negotiated_cipher_suite,
        alpn,
        leaf,
        chain_valid,
        trusted_by_system_store,
        verification_performed,
        verified_chain_depth,
        chain_diagnostics,
        chain,
        revocation,
    })
}

async fn run_resumption_check(
    req: &TlsProfileRequest,
    addr: SocketAddr,
    sni: &str,
) -> anyhow::Result<TlsResumptionSection> {
    let server_name = ServerName::try_from(sni.to_string()).map_err(|e| anyhow::anyhow!(e))?;
    let tls_config = Arc::new(build_tls_config(req.insecure, req.ca_bundle.as_deref())?);
    let connector = TlsConnector::from(tls_config.clone());

    let one = async {
        let tcp = tokio::time::timeout(
            std::time::Duration::from_millis(req.timeout_ms),
            TcpStream::connect(addr),
        )
        .await
        .context("timed out connecting TCP socket during TLS resumption check")??;
        let t = Instant::now();
        let stream = tokio::time::timeout(
            std::time::Duration::from_millis(req.timeout_ms),
            connector.connect(server_name.clone(), tcp),
        )
        .await
        .context("timed out during TLS handshake in resumption check")??;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let (_, conn) = stream.get_ref();
        Ok::<(f64, bool, Option<String>), anyhow::Error>((
            ms,
            matches!(conn.handshake_kind(), Some(rustls::HandshakeKind::Resumed)),
            conn.handshake_kind()
                .map(|k| format!("{k:?}").to_lowercase()),
        ))
    };

    let (initial_ms, _, _) = one.await?;
    let tcp = tokio::time::timeout(
        std::time::Duration::from_millis(req.timeout_ms),
        TcpStream::connect(addr),
    )
    .await
    .context("timed out connecting TCP socket during second TLS resumption probe")??;
    let t = Instant::now();
    let stream = tokio::time::timeout(
        std::time::Duration::from_millis(req.timeout_ms),
        connector.connect(server_name, tcp),
    )
    .await
    .context("timed out during second TLS handshake in resumption check")??;
    let resumed_ms = t.elapsed().as_secs_f64() * 1000.0;
    let (_, conn) = stream.get_ref();
    let resumed = matches!(conn.handshake_kind(), Some(rustls::HandshakeKind::Resumed));
    let method = match conn.handshake_kind() {
        Some(rustls::HandshakeKind::Resumed) => Some("ticket".into()),
        _ => None,
    };
    Ok(TlsResumptionSection {
        supported: resumed,
        method,
        initial_handshake_ms: Some(initial_ms),
        resumed_handshake_ms: Some(resumed_ms),
        early_data_offered: false,
        early_data_accepted: None,
    })
}

fn build_tls_config(
    insecure: bool,
    ca_bundle: Option<&str>,
) -> anyhow::Result<rustls::ClientConfig> {
    let mut config = if insecure {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth()
    } else {
        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let native = rustls_native_certs::load_native_certs();
        for cert in native.certs {
            let _ = root_store.add(cert);
        }
        if let Some(bundle_path) = ca_bundle {
            load_ca_bundle(&mut root_store, bundle_path)?;
        }
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth()
    };
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

fn parse_certificate_info(cert: &CertificateDer<'_>) -> Option<TlsCertificateInfo> {
    let (_, cert) = X509Certificate::from_der(cert.as_ref()).ok()?;
    let san = cert.subject_alternative_name().ok().flatten();
    let san_dns = san
        .as_ref()
        .map(|ext| {
            ext.value
                .general_names
                .iter()
                .filter_map(|g| match g {
                    GeneralName::DNSName(name) => Some(name.to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let san_ip = san
        .as_ref()
        .map(|ext| {
            ext.value
                .general_names
                .iter()
                .filter_map(|g| match g {
                    GeneralName::IPAddress(bytes) if bytes.len() == 4 => {
                        Some(IpAddr::from(<[u8; 4]>::try_from(*bytes).ok()?).to_string())
                    }
                    GeneralName::IPAddress(bytes) if bytes.len() == 16 => {
                        Some(IpAddr::from(<[u8; 16]>::try_from(*bytes).ok()?).to_string())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut ocsp_urls = Vec::new();
    let mut crl_urls = Vec::new();
    let mut aia_issuers = Vec::new();
    let must_staple = false;
    let mut scts_present = false;
    for ext in cert.extensions() {
        match ext.parsed_extension() {
            ParsedExtension::AuthorityInfoAccess(aia) => {
                for ad in &aia.accessdescs {
                    let access_method = ad.access_method.to_id_string();
                    if let GeneralName::URI(uri) = &ad.access_location {
                        if access_method == "1.3.6.1.5.5.7.48.1" {
                            ocsp_urls.push(uri.to_string());
                        } else if access_method == "1.3.6.1.5.5.7.48.2" {
                            aia_issuers.push(uri.to_string());
                        }
                    }
                }
            }
            ParsedExtension::CRLDistributionPoints(points) => {
                for point in &points.points {
                    if let Some(x509_parser::extensions::DistributionPointName::FullName(names)) =
                        &point.distribution_point
                    {
                        for gn in names {
                            if let GeneralName::URI(uri) = gn {
                                crl_urls.push(uri.to_string());
                            }
                        }
                    }
                }
            }
            ParsedExtension::SCT(scts) => {
                scts_present = !scts.is_empty();
            }
            _ => {}
        }
    }

    let public_key = cert.public_key().raw.to_vec();
    let spki_sha256 = hex_lower(Sha256::digest(public_key));
    let fingerprint = hex_lower(Sha256::digest(cert.as_ref()));
    let serial_number = cert.raw_serial_as_string();
    let key_bits = cert.public_key().parsed().ok().and_then(|pk| match pk {
        x509_parser::public_key::PublicKey::RSA(key) => Some((key.modulus.len() * 8) as u32),
        x509_parser::public_key::PublicKey::EC(_) => None,
        _ => None,
    });
    let key_type = match cert.public_key().parsed().ok()? {
        x509_parser::public_key::PublicKey::RSA(_) => "RSA".to_string(),
        x509_parser::public_key::PublicKey::EC(_) => "EC".to_string(),
        x509_parser::public_key::PublicKey::DSA(_) => "DSA".to_string(),
        _ => "Unknown".to_string(),
    };

    Some(TlsCertificateInfo {
        subject: cert.subject().to_string(),
        issuer: cert.issuer().to_string(),
        serial_number,
        not_before: DateTime::from_timestamp(cert.validity().not_before.timestamp(), 0),
        not_after: DateTime::from_timestamp(cert.validity().not_after.timestamp(), 0),
        san_dns,
        san_ip,
        key_type,
        key_bits,
        signature_algorithm: cert.signature_algorithm.algorithm.to_id_string(),
        is_ca: cert
            .basic_constraints()
            .ok()
            .flatten()
            .map(|bc| bc.value.ca)
            .unwrap_or(false),
        sha256_fingerprint: fingerprint,
        spki_sha256,
        ocsp_urls,
        crl_urls,
        aia_issuers,
        must_staple,
        scts_present,
    })
}

fn cert_matches_hostname(cert: &TlsCertificateInfo, host: &str) -> bool {
    if cert.san_dns.is_empty() && cert.san_ip.is_empty() {
        return extract_subject_cn(&cert.subject)
            .map(|cn| dns_name_matches(cn, host))
            .unwrap_or(false);
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return cert
            .san_ip
            .iter()
            .any(|candidate| candidate == &ip.to_string());
    }
    cert.san_dns
        .iter()
        .any(|candidate| dns_name_matches(candidate, host))
}

fn dns_name_matches(pattern: &str, host: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    if pattern == host {
        return true;
    }
    if let Some(rest) = pattern.strip_prefix("*.") {
        if let Some(prefix) = host.strip_suffix(rest) {
            return prefix.ends_with('.') && !prefix[..prefix.len() - 1].contains('.');
        }
    }
    false
}

fn extract_subject_cn(subject: &str) -> Option<&str> {
    let cn_start = subject.find("CN=")? + 3;
    Some(subject[cn_start..].split(',').next()?.trim())
}

fn build_chain_diagnostics(chain: &[TlsCertificateInfo]) -> TlsChainDiagnostics {
    let leaf_self_signed = chain
        .first()
        .map(|leaf| leaf.subject == leaf.issuer)
        .unwrap_or(false);
    let has_intermediate = chain.len() > 1;
    let ordered_subject_issuer_links = chain
        .windows(2)
        .all(|pair| pair[0].issuer == pair[1].subject);
    let root_included = chain
        .last()
        .map(|cert| cert.subject == cert.issuer)
        .unwrap_or(false);

    let mut notes = Vec::new();
    if chain.is_empty() {
        notes.push("no certificates were presented".into());
    }
    if leaf_self_signed {
        notes.push("leaf certificate appears self-signed".into());
    }
    if !ordered_subject_issuer_links && chain.len() > 1 {
        notes.push("presented chain ordering does not form a clean issuer/subject path".into());
    }
    if !has_intermediate && chain.len() == 1 {
        notes.push("no intermediate certificates were presented".into());
    }
    if root_included {
        notes.push("presented chain includes a self-signed root".into());
    }

    TlsChainDiagnostics {
        presented_chain_length: chain.len() as u32,
        leaf_self_signed,
        has_intermediate,
        ordered_subject_issuer_links,
        root_included,
        notes,
    }
}

async fn lookup_caa(host: &str) -> Option<TlsCaaInfo> {
    if host.starts_with('-') || host.len() > 253 {
        return None;
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new("dig")
            .args(["+short", "+time=3", "+tries=1", "CAA", "--", host])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let records = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    Some(TlsCaaInfo {
        present: !records.is_empty(),
        records,
    })
}

fn build_findings(
    snapshot: &HandshakeSnapshot,
    hostname_matches: bool,
    resumption: &TlsResumptionSection,
    classification: &TlsPathClassification,
    insecure: bool,
) -> Vec<TlsFinding> {
    let mut findings = Vec::new();
    if !hostname_matches {
        findings.push(TlsFinding {
            severity: TlsFindingSeverity::Error,
            code: "HOSTNAME_MISMATCH".into(),
            message: "Certificate does not match the requested host".into(),
        });
    }
    if insecure {
        findings.push(TlsFinding {
            severity: TlsFindingSeverity::Warning,
            code: "INSECURE_MODE_ACTIVE".into(),
            message: "Certificate verification was disabled with --insecure; trust results are observational only".into(),
        });
    } else if !snapshot.trusted_by_system_store {
        findings.push(TlsFinding {
            severity: TlsFindingSeverity::Warning,
            code: "UNTRUSTED_CHAIN".into(),
            message: "Certificate chain was not validated against the system trust store".into(),
        });
    }
    if snapshot.chain_diagnostics.leaf_self_signed {
        findings.push(TlsFinding {
            severity: TlsFindingSeverity::Warning,
            code: "SELF_SIGNED_LEAF".into(),
            message: "Leaf certificate appears self-signed".into(),
        });
    }
    if !snapshot.chain_diagnostics.ordered_subject_issuer_links
        && snapshot.chain_diagnostics.presented_chain_length > 1
    {
        findings.push(TlsFinding {
            severity: TlsFindingSeverity::Warning,
            code: "CHAIN_ORDERING_ODD".into(),
            message: "Presented certificate chain ordering does not follow issuer-to-subject links cleanly".into(),
        });
    }
    if !snapshot.chain_diagnostics.has_intermediate
        && snapshot.chain_diagnostics.presented_chain_length == 1
    {
        findings.push(TlsFinding {
            severity: TlsFindingSeverity::Info,
            code: "NO_INTERMEDIATE_PRESENTED".into(),
            message: "Server presented only a leaf certificate and no intermediate certificates"
                .into(),
        });
    }
    if snapshot.chain_diagnostics.root_included {
        findings.push(TlsFinding {
            severity: TlsFindingSeverity::Info,
            code: "ROOT_INCLUDED_IN_CHAIN".into(),
            message: "Presented chain includes a self-signed root certificate".into(),
        });
    }
    if !snapshot.revocation.ocsp_urls.is_empty() || !snapshot.revocation.crl_urls.is_empty() {
        findings.push(TlsFinding {
            severity: TlsFindingSeverity::Info,
            code: "REVOCATION_METADATA_PRESENT".into(),
            message: "Certificate advertises revocation metadata (OCSP and/or CRL endpoints)"
                .into(),
        });
    }
    if snapshot
        .leaf
        .as_ref()
        .map(|leaf| leaf.scts_present)
        .unwrap_or(false)
    {
        findings.push(TlsFinding {
            severity: TlsFindingSeverity::Info,
            code: "CT_PRESENT".into(),
            message: "Certificate transparency evidence present in certificate extensions".into(),
        });
    }
    if snapshot
        .leaf
        .as_ref()
        .map(|leaf| leaf.must_staple)
        .unwrap_or(false)
    {
        findings.push(TlsFinding {
            severity: TlsFindingSeverity::Info,
            code: "MUST_STAPLE".into(),
            message: "Leaf certificate indicates OCSP Must-Staple".into(),
        });
    }
    if let Some(leaf) = &snapshot.leaf {
        if let Some(not_after) = leaf.not_after {
            if not_after < Utc::now() {
                findings.push(TlsFinding {
                    severity: TlsFindingSeverity::Error,
                    code: "CERT_EXPIRED".into(),
                    message: "Leaf certificate is expired".into(),
                });
            }
        }
        if leaf.ocsp_urls.is_empty() && leaf.crl_urls.is_empty() {
            findings.push(TlsFinding {
                severity: TlsFindingSeverity::Info,
                code: "NO_REVOCATION_ENDPOINTS".into(),
                message: "Leaf certificate does not advertise OCSP or CRL endpoints".into(),
            });
        }
    }
    if !resumption.supported {
        findings.push(TlsFinding {
            severity: TlsFindingSeverity::Info,
            code: "RESUMPTION_NOT_OBSERVED".into(),
            message: "TLS session resumption was not observed on the second handshake".into(),
        });
    }
    if matches!(classification, TlsPathClassification::IndirectSuspicious) {
        findings.push(TlsFinding {
            severity: TlsFindingSeverity::Warning,
            code: "PATH_SUSPICIOUS".into(),
            message: "Connection path appears indirect or inconsistent with DNS resolution".into(),
        });
    }
    findings
}

fn summarize_findings(findings: &[TlsFinding]) -> TlsProfileSummary {
    let status = if findings
        .iter()
        .any(|f| f.severity == TlsFindingSeverity::Error)
    {
        "error"
    } else if findings
        .iter()
        .any(|f| f.severity == TlsFindingSeverity::Warning)
    {
        "warn"
    } else {
        "ok"
    };
    TlsProfileSummary {
        status: status.into(),
        score: None,
    }
}

fn hex_lower<T: AsRef<[u8]>>(bytes: T) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer,
        _intermediates: &[rustls::pki_types::CertificateDer],
        _server_name: &rustls::pki_types::ServerName,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_hostname_matches_single_level_subdomain() {
        assert!(dns_name_matches("*.example.com", "www.example.com"));
        assert!(!dns_name_matches("*.example.com", "example.com"));
        assert!(!dns_name_matches("*.example.com", "a.b.example.com"));
    }

    #[test]
    fn summary_uses_null_score() {
        let summary = summarize_findings(&[]);
        assert_eq!(summary.status, "ok");
        assert!(summary.score.is_none());
    }

    #[test]
    fn chain_diagnostics_detect_self_signed_leaf_and_root() {
        let chain = vec![
            TlsCertificateInfo {
                subject: "CN=leaf".into(),
                issuer: "CN=leaf".into(),
                serial_number: "01".into(),
                not_before: None,
                not_after: None,
                san_dns: vec![],
                san_ip: vec![],
                key_type: "RSA".into(),
                key_bits: Some(2048),
                signature_algorithm: "1.2.3".into(),
                is_ca: false,
                sha256_fingerprint: "aa".into(),
                spki_sha256: "bb".into(),
                ocsp_urls: vec![],
                crl_urls: vec![],
                aia_issuers: vec![],
                must_staple: false,
                scts_present: false,
            },
            TlsCertificateInfo {
                subject: "CN=root".into(),
                issuer: "CN=root".into(),
                serial_number: "02".into(),
                not_before: None,
                not_after: None,
                san_dns: vec![],
                san_ip: vec![],
                key_type: "RSA".into(),
                key_bits: Some(4096),
                signature_algorithm: "1.2.3".into(),
                is_ca: true,
                sha256_fingerprint: "cc".into(),
                spki_sha256: "dd".into(),
                ocsp_urls: vec![],
                crl_urls: vec![],
                aia_issuers: vec![],
                must_staple: false,
                scts_present: false,
            },
        ];
        let diag = build_chain_diagnostics(&chain);
        assert!(diag.leaf_self_signed);
        assert!(diag.root_included);
        assert!(diag.has_intermediate);
        assert!(!diag.notes.is_empty());
    }

    #[test]
    fn hostname_match_uses_san_dns() {
        let cert = TlsCertificateInfo {
            subject: "CN=example.com".into(),
            issuer: "CN=CA".into(),
            serial_number: "01".into(),
            not_before: None,
            not_after: None,
            san_dns: vec!["example.com".into(), "*.example.org".into()],
            san_ip: vec![],
            key_type: "RSA".into(),
            key_bits: Some(2048),
            signature_algorithm: "1.2.3".into(),
            is_ca: false,
            sha256_fingerprint: "aa".into(),
            spki_sha256: "bb".into(),
            ocsp_urls: vec![],
            crl_urls: vec![],
            aia_issuers: vec![],
            must_staple: false,
            scts_present: false,
        };
        assert!(cert_matches_hostname(&cert, "example.com"));
        assert!(cert_matches_hostname(&cert, "foo.example.org"));
        assert!(!cert_matches_hostname(&cert, "bad.test"));
    }

    #[test]
    fn hostname_match_cn_fallback_is_exact_not_substring() {
        let cert = TlsCertificateInfo {
            subject: "CN=example.com, O=Org".into(),
            issuer: "CN=CA".into(),
            serial_number: "01".into(),
            not_before: None,
            not_after: None,
            san_dns: vec![],
            san_ip: vec![],
            key_type: "RSA".into(),
            key_bits: Some(2048),
            signature_algorithm: "1.2.3".into(),
            is_ca: false,
            sha256_fingerprint: "aa".into(),
            spki_sha256: "bb".into(),
            ocsp_urls: vec![],
            crl_urls: vec![],
            aia_issuers: vec![],
            must_staple: false,
            scts_present: false,
        };
        assert!(cert_matches_hostname(&cert, "example.com"));
        assert!(!cert_matches_hostname(&cert, "ple.com"));
        assert!(!cert_matches_hostname(&cert, "com"));
    }
}
