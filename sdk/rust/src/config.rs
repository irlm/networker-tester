//! Configuration for a LagHound endpoint (contract v1 §2).

/// SDK language tag reported on `/health` and `/info` (`sdk.lang`).
pub const SDK_LANG: &str = "rust";

/// SDK package version reported on `/health` and `/info` (`sdk.version`).
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Absolute maximum download/upload payload, not configurable (contract §2).
pub const ABSOLUTE_MAX_BYTES: u64 = 33_554_432; // 32 MiB

/// Default download/upload cap (contract §2).
pub const DEFAULT_CAP_BYTES: u64 = 4_194_304; // 4 MiB

/// Maximum request body accepted on `/echo` before `413` (contract §3.2, §6.1).
pub const ECHO_REQUEST_BODY_MAX_BYTES: u64 = 65_536; // 64 KiB

/// Streaming chunk size for `/download` (contract §3.3: chunks <= 64 KiB).
pub const DOWNLOAD_CHUNK_BYTES: usize = 65_536;

/// Fill byte for `/download` bodies — `0x42` (`'B'`), matching
/// `networker-endpoint`'s `DOWNLOAD_FILL` (contract §3.3).
pub const DOWNLOAD_FILL: u8 = 0x42;

/// Minimum token length in bytes (contract §2, `token_min_bytes`).
pub const TOKEN_MIN_BYTES: usize = 16;

/// Kill-switch environment variable (contract §6.5).
pub const ENV_DISABLED: &str = "LAGHOUND_DISABLED";

/// Token-source environment variable (contract §2).
pub const ENV_TOKEN: &str = "LAGHOUND_TOKEN";

/// A token-bucket rate limit: sustained `rps` with a `burst` ceiling.
#[derive(Clone, Copy, Debug)]
pub struct RateLimit {
    pub rps: u32,
    pub burst: u32,
}

/// Optional sliding-window byte budget for transfer routes (contract §6.4).
#[derive(Clone, Copy, Debug)]
pub struct ByteBudget {
    pub bytes: u64,
    pub window_s: u64,
}

/// Per-route enable/disable map (contract §3.1 `routes`). `health` is always
/// enabled while the SDK is mounted.
#[derive(Clone, Copy, Debug)]
pub struct RouteToggles {
    pub echo: bool,
    pub download: bool,
    pub upload: bool,
    pub info: bool,
}

impl Default for RouteToggles {
    fn default() -> Self {
        Self {
            echo: true,
            download: true,
            upload: true,
            info: true,
        }
    }
}

/// LagHound endpoint configuration.
///
/// Build with [`Config::new`] then chain the builder methods:
///
/// ```
/// let cfg = laghound::Config::new("a-sufficiently-long-token")
///     .prefix("/laghound")
///     .download_cap(4 * 1024 * 1024);
/// ```
#[derive(Clone, Debug)]
pub struct Config {
    pub(crate) prefix: String,
    pub(crate) tokens: Vec<String>,
    pub(crate) download_cap_bytes: u64,
    pub(crate) upload_cap_bytes: u64,
    pub(crate) rate_per_ip: RateLimit,
    pub(crate) rate_global: RateLimit,
    pub(crate) max_concurrent: u32,
    pub(crate) max_concurrent_transfers: u32,
    pub(crate) per_ip_table_max_entries: usize,
    pub(crate) byte_budget: Option<ByteBudget>,
    pub(crate) app_name: Option<String>,
    pub(crate) routes: RouteToggles,
}

/// Reasons [`Config::build`] / mounting can fail-closed (contract §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// No token was supplied programmatically or via `LAGHOUND_TOKEN`.
    MissingToken,
    /// A supplied token is shorter than [`TOKEN_MIN_BYTES`].
    TokenTooShort,
    /// More than two tokens supplied (contract §5: current + previous only).
    TooManyTokens,
    /// `prefix` must start with `/` and not end with `/`.
    InvalidPrefix,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ConfigError::MissingToken => {
                "laghound: no token supplied (refusing to mount open routes)"
            }
            ConfigError::TokenTooShort => "laghound: token shorter than 16 bytes",
            ConfigError::TooManyTokens => "laghound: at most 2 tokens (current + previous) allowed",
            ConfigError::InvalidPrefix => {
                "laghound: prefix must start with '/' and have no trailing slash"
            }
        };
        f.write_str(s)
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Start a config with a single shared token. Additional builder calls
    /// refine it; call [`Config::build`] (or hand it to [`crate::service`] /
    /// [`crate::router`], which build internally) to validate.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            prefix: "/laghound".to_string(),
            tokens: vec![token.into()],
            download_cap_bytes: DEFAULT_CAP_BYTES,
            upload_cap_bytes: DEFAULT_CAP_BYTES,
            rate_per_ip: RateLimit { rps: 10, burst: 20 },
            rate_global: RateLimit {
                rps: 50,
                burst: 100,
            },
            max_concurrent: 8,
            max_concurrent_transfers: 2,
            per_ip_table_max_entries: 10_000,
            byte_budget: None,
            app_name: None,
            routes: RouteToggles::default(),
        }
    }

    /// Build a config sourcing the token from `LAGHOUND_TOKEN` (contract §2).
    /// Returns [`ConfigError::MissingToken`] if the variable is unset/empty.
    pub fn from_env() -> Result<Self, ConfigError> {
        match std::env::var(ENV_TOKEN) {
            Ok(t) if !t.is_empty() => Ok(Self::new(t)),
            _ => Err(ConfigError::MissingToken),
        }
    }

    /// Set the mount prefix (default `/laghound`).
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Add a second token (current + previous) for zero-downtime rotation
    /// (contract §5). At most two tokens total.
    pub fn add_token(mut self, token: impl Into<String>) -> Self {
        self.tokens.push(token.into());
        self
    }

    /// Set the effective `/download` cap (clamped to the absolute max).
    pub fn download_cap(mut self, bytes: u64) -> Self {
        self.download_cap_bytes = bytes;
        self
    }

    /// Set the effective `/upload` cap (clamped to the absolute max).
    pub fn upload_cap(mut self, bytes: u64) -> Self {
        self.upload_cap_bytes = bytes;
        self
    }

    /// Set the per-IP rate limit (default 10 rps, burst 20).
    pub fn rate_per_ip(mut self, rps: u32, burst: u32) -> Self {
        self.rate_per_ip = RateLimit { rps, burst };
        self
    }

    /// Set the global rate limit (default 50 rps, burst 100).
    pub fn rate_global(mut self, rps: u32, burst: u32) -> Self {
        self.rate_global = RateLimit { rps, burst };
        self
    }

    /// Set the overall concurrency cap (default 8).
    pub fn max_concurrent(mut self, n: u32) -> Self {
        self.max_concurrent = n;
        self
    }

    /// Set the transfer-route concurrency cap (default 2).
    pub fn max_concurrent_transfers(mut self, n: u32) -> Self {
        self.max_concurrent_transfers = n;
        self
    }

    /// Enable the optional sliding-window byte budget (contract §6.4).
    pub fn byte_budget(mut self, bytes: u64, window_s: u64) -> Self {
        self.byte_budget = Some(ByteBudget { bytes, window_s });
        self
    }

    /// Set the optional `app_name` echoed on `/health` and `/info`.
    pub fn app_name(mut self, name: impl Into<String>) -> Self {
        self.app_name = Some(name.into());
        self
    }

    /// Enable/disable individual routes (`health` is always on).
    pub fn routes(mut self, toggles: RouteToggles) -> Self {
        self.routes = toggles;
        self
    }

    /// Validate the config (contract §2). Applies cap clamping and normalizes.
    pub fn build(mut self) -> Result<Config, ConfigError> {
        if self.tokens.is_empty() {
            return Err(ConfigError::MissingToken);
        }
        if self.tokens.len() > 2 {
            return Err(ConfigError::TooManyTokens);
        }
        if self.tokens.iter().any(|t| t.len() < TOKEN_MIN_BYTES) {
            return Err(ConfigError::TokenTooShort);
        }
        if !self.prefix.starts_with('/') || (self.prefix.len() > 1 && self.prefix.ends_with('/')) {
            return Err(ConfigError::InvalidPrefix);
        }
        // Enforce the absolute max even if config asks for more (contract §2).
        self.download_cap_bytes = self.download_cap_bytes.min(ABSOLUTE_MAX_BYTES);
        self.upload_cap_bytes = self.upload_cap_bytes.min(ABSOLUTE_MAX_BYTES);
        Ok(self)
    }
}
