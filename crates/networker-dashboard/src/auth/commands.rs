//! Per-command JWT auth layer.
//!
//! Short-lived tokens authorize a single command (verb) on a specific agent
//! and (optionally) a specific benchmark-config or job. This sits on top of
//! the long-lived `agent.api_key` that authenticates the WebSocket session.
//!
//! Claims:
//! - `sub`: agent_id (UUID string)
//! - `aud`: config_id / job_id (UUID string; empty string for ad-hoc)
//! - `scope`: list of verbs this token authorizes
//! - `exp`: expiry (seconds since epoch)
//! - `iat`: issued-at (seconds since epoch)
//!
//! Consumers (dashboard REST handlers + agent command validator) are wired up
//! in subsequent tasks; silence dead-code warnings until then.
#![allow(dead_code)]

use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Minimum remaining lifetime (seconds) required at validation time.
/// A token that is about to expire can't reliably run a command.
const MIN_REMAINING_SECS: i64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandClaims {
    /// Agent UUID as string.
    pub sub: String,
    /// Config / job UUID as string. Empty string for ad-hoc commands.
    pub aud: String,
    /// Verbs this token authorizes (e.g. `["start_server", "health"]`).
    pub scope: Vec<String>,
    /// Expiry (seconds since unix epoch).
    pub exp: u64,
    /// Issued-at (seconds since unix epoch).
    pub iat: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CommandAuthError {
    #[error("invalid signature or malformed token")]
    Invalid,
    #[error("token expires in {secs}s (< 60s guard)")]
    TooCloseToExpiry { secs: i64 },
    #[error("token issued for agent {expected} but claims {actual}")]
    WrongAgent { expected: Uuid, actual: String },
    #[error("token issued for config {expected:?} but claims {actual:?}")]
    WrongConfig {
        expected: Option<Uuid>,
        actual: String,
    },
    #[error("verb {verb} not in token scope")]
    VerbNotInScope { verb: String },
}

fn now_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

/// Mint a short-lived command JWT.
///
/// `lifetime_secs` should be `max_duration + buffer` (spec suggests +300s).
/// When `config_id` is `None` the `aud` claim is stored as an empty string.
pub fn mint_command_token(
    secret: &str,
    agent_id: Uuid,
    config_id: Option<Uuid>,
    scope: &[String],
    lifetime_secs: u64,
) -> anyhow::Result<String> {
    let now = now_secs();
    let claims = CommandClaims {
        sub: agent_id.to_string(),
        aud: config_id.map(|u| u.to_string()).unwrap_or_default(),
        scope: scope.to_vec(),
        exp: now + lifetime_secs,
        iat: now,
    };
    // Disable default `aud` validation in jsonwebtoken (we validate aud ourselves).
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok(token)
}

/// Validate a JWT against expected agent, (optional) config, and required verb.
///
/// When `expected_config` is `None`, the `aud` claim is not checked.
/// When `expected_config` is `Some(id)`, the token's `aud` must match `id`.
pub fn validate_command_token(
    secret: &str,
    token: &str,
    expected_agent: Uuid,
    expected_config: Option<Uuid>,
    required_verb: &str,
) -> Result<CommandClaims, CommandAuthError> {
    // We set our own expiry rule; disable jsonwebtoken's default aud check
    // (our `aud` is not always a valid single-audience string).
    let mut validation = Validation::default();
    validation.validate_aud = false;
    // We'll apply our own stricter exp guard (MIN_REMAINING_SECS); keep the
    // library's default `validate_exp = true` so flat-out expired tokens are
    // rejected at the crypto layer as `Invalid`.

    let data = decode::<CommandClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|_| CommandAuthError::Invalid)?;
    let claims = data.claims;

    // 1. Agent must match.
    if claims.sub != expected_agent.to_string() {
        return Err(CommandAuthError::WrongAgent {
            expected: expected_agent,
            actual: claims.sub,
        });
    }

    // 2. Config must match when caller specifies one.
    if let Some(cfg) = expected_config {
        if claims.aud != cfg.to_string() {
            return Err(CommandAuthError::WrongConfig {
                expected: Some(cfg),
                actual: claims.aud,
            });
        }
    }

    // 3. Verb must be in scope.
    if !claims.scope.iter().any(|v| v == required_verb) {
        return Err(CommandAuthError::VerbNotInScope {
            verb: required_verb.to_string(),
        });
    }

    // 4. Expiry guard: must have > MIN_REMAINING_SECS left.
    let now = now_secs() as i64;
    let remaining = claims.exp as i64 - now;
    if remaining < MIN_REMAINING_SECS {
        return Err(CommandAuthError::TooCloseToExpiry { secs: remaining });
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_and_validate_round_trip() {
        let secret = "test-secret-not-used-in-prod";
        let agent = Uuid::new_v4();
        let cfg = Uuid::new_v4();
        let t = mint_command_token(secret, agent, Some(cfg), &["health".into()], 600).unwrap();
        let claims = validate_command_token(secret, &t, agent, Some(cfg), "health").unwrap();
        assert_eq!(claims.sub, agent.to_string());
        assert_eq!(claims.aud, cfg.to_string());
        assert!(claims.scope.iter().any(|s| s == "health"));
    }

    #[test]
    fn reject_wrong_signature() {
        let t =
            mint_command_token("secret-a", Uuid::new_v4(), None, &["health".into()], 600).unwrap();
        let err =
            validate_command_token("secret-b", &t, Uuid::new_v4(), None, "health").unwrap_err();
        assert!(matches!(err, CommandAuthError::Invalid));
    }

    #[test]
    fn reject_when_exp_too_close() {
        // lifetime 30s -> exp - now = 30 -> < 60s guard -> reject
        let agent = Uuid::new_v4();
        let t = mint_command_token("s", agent, None, &["health".into()], 30).unwrap();
        let err = validate_command_token("s", &t, agent, None, "health").unwrap_err();
        assert!(matches!(err, CommandAuthError::TooCloseToExpiry { .. }));
    }

    #[test]
    fn reject_verb_not_in_scope() {
        let agent = Uuid::new_v4();
        let t = mint_command_token("s", agent, None, &["health".into()], 600).unwrap();
        let err = validate_command_token("s", &t, agent, None, "run_probe").unwrap_err();
        assert!(matches!(err, CommandAuthError::VerbNotInScope { .. }));
    }

    #[test]
    fn reject_wrong_agent() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let t = mint_command_token("s", a, None, &["health".into()], 600).unwrap();
        let err = validate_command_token("s", &t, b, None, "health").unwrap_err();
        assert!(matches!(err, CommandAuthError::WrongAgent { .. }));
    }

    #[test]
    fn reject_wrong_config() {
        let agent = Uuid::new_v4();
        let cfg = Uuid::new_v4();
        let other = Uuid::new_v4();
        let t = mint_command_token("s", agent, Some(cfg), &["health".into()], 600).unwrap();
        let err = validate_command_token("s", &t, agent, Some(other), "health").unwrap_err();
        assert!(matches!(err, CommandAuthError::WrongConfig { .. }));
    }
}
