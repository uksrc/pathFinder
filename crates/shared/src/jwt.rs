//! JWT claims shared between the HTTP server and its authentication middleware.
//!
//! Signature verification, expiry checking and issuer validation are handled
//! by the `axum-jwt-auth` extractor pipeline (a remote-JWKS decoder) at the
//! HTTP layer; this module only defines the strongly-typed claims payload that
//! the validated tokens decode into.

use serde::{Deserialize, Serialize};

/// Claims expected on an incoming bearer token.
///
/// `sub` identifies the authenticated user; `exp` is validated by the
/// `axum-jwt-auth` decoder when present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: String,
    #[serde(default)]
    pub exp: Option<i64>,
}
