//! Shared, immutable-after-init endpoint state built once from a [`Config`].

use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use subtle::ConstantTimeEq;

use crate::body::FillBuffer;
use crate::config::{
    Config, RouteToggles, ABSOLUTE_MAX_BYTES, SDK_LANG, SDK_VERSION, TOKEN_MIN_BYTES,
};
use crate::limits::{ByteBudgetTracker, Concurrency, RateLimiter};

/// Runtime state shared across all handlers. Cheap to clone (`Arc`).
#[derive(Clone)]
pub struct State(pub(crate) Arc<Inner>);

pub struct Inner {
    pub prefix: String,
    pub tokens: Vec<Vec<u8>>,
    pub download_cap_bytes: u64,
    pub upload_cap_bytes: u64,
    pub routes: RouteToggles,
    pub started: Instant,
    pub fill: FillBuffer,
    pub rate: RateLimiter,
    pub concurrency: Concurrency,
    pub budget: Option<ByteBudgetTracker>,
    /// Precomputed `/health` JSON minus `uptime_s` (contract §3.1: O(1)).
    pub health_template: Value,
    /// Precomputed `/info` JSON minus `uptime_s`.
    pub info_template: Value,
}

impl State {
    pub fn from_config(cfg: Config) -> Self {
        let tokens: Vec<Vec<u8>> = cfg.tokens.iter().map(|t| t.as_bytes().to_vec()).collect();
        let routes = cfg.routes;
        let sdk = json!({ "lang": SDK_LANG, "version": SDK_VERSION });
        let routes_json = json!({
            "health": true,
            "echo": routes.echo,
            "download": routes.download,
            "upload": routes.upload,
            "info": routes.info,
        });

        let mut health = json!({
            "contract": "v1",
            "status": "ok",
            "sdk": sdk.clone(),
            "routes": routes_json.clone(),
        });
        if let Some(app) = &cfg.app_name {
            health["app"] = json!(app);
        }

        let byte_budget = cfg
            .byte_budget
            .map(|b| json!({ "bytes": b.bytes, "window_s": b.window_s }))
            .unwrap_or(Value::Null);
        let mut info = json!({
            "contract": "v1",
            "sdk": sdk,
            "prefix": cfg.prefix,
            "token_set": !tokens.is_empty(),
            "caps": {
                "download_bytes": cfg.download_cap_bytes,
                "upload_bytes": cfg.upload_cap_bytes,
                "absolute_max_bytes": ABSOLUTE_MAX_BYTES,
            },
            "limits": {
                "rate_per_ip": { "rps": cfg.rate_per_ip.rps, "burst": cfg.rate_per_ip.burst },
                "rate_global": { "rps": cfg.rate_global.rps, "burst": cfg.rate_global.burst },
                "max_concurrent": cfg.max_concurrent,
                "max_concurrent_transfers": cfg.max_concurrent_transfers,
                "byte_budget": byte_budget,
            },
            "routes": routes_json,
        });
        if let Some(app) = &cfg.app_name {
            info["app"] = json!(app);
        }

        let inner = Inner {
            prefix: cfg.prefix.clone(),
            tokens,
            download_cap_bytes: cfg.download_cap_bytes,
            upload_cap_bytes: cfg.upload_cap_bytes,
            routes,
            started: Instant::now(),
            fill: FillBuffer::new(),
            rate: RateLimiter::new(
                cfg.rate_per_ip,
                cfg.rate_global,
                cfg.per_ip_table_max_entries,
            ),
            concurrency: Concurrency::new(cfg.max_concurrent, cfg.max_concurrent_transfers),
            budget: cfg.byte_budget.map(ByteBudgetTracker::new),
            health_template: health,
            info_template: info,
        };
        State(Arc::new(inner))
    }

    pub fn uptime_s(&self) -> u64 {
        self.0.started.elapsed().as_secs()
    }

    /// Effective download cap (contract §3.3).
    pub fn effective_download_cap(&self) -> u64 {
        self.0.download_cap_bytes.min(ABSOLUTE_MAX_BYTES)
    }

    /// Effective upload cap (contract §3.4).
    pub fn effective_upload_cap(&self) -> u64 {
        self.0.upload_cap_bytes.min(ABSOLUTE_MAX_BYTES)
    }

    /// Constant-time token check over the full candidate length (contract §5).
    /// Length mismatch does not short-circuit observably: every configured
    /// token is compared, and the candidate is compared byte-for-byte via
    /// `subtle::ConstantTimeEq` (which itself is length-guarded but does not
    /// leak *which* token matched or how far the compare got).
    pub fn token_matches(&self, candidate: &[u8]) -> bool {
        // Reject implausibly short candidates the same way a real token would
        // fail — still constant-time against each configured token.
        let mut matched = 0u8;
        for tok in &self.0.tokens {
            // ConstantTimeEq requires equal lengths; when lengths differ we
            // still run a fixed-cost compare against a length-padded view so
            // timing does not reveal the token length.
            let eq = if tok.len() == candidate.len() {
                tok.ct_eq(candidate).unwrap_u8()
            } else {
                // Compare candidate against itself to keep the work constant,
                // then force a non-match.
                let _ = candidate.ct_eq(candidate);
                0u8
            };
            matched |= eq;
        }
        // Guard against a pathological empty-token config (build() forbids
        // tokens < 16 bytes, so this is defense in depth).
        matched == 1 && candidate.len() >= TOKEN_MIN_BYTES
    }
}
