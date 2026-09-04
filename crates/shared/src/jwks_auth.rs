//! Shared bearer-token (JWKS) authentication used by both the CLI and the HTTP
//! server.
//!
//! Responsibilities, made explicit here so they no longer live implicitly in
//! each binary:
//!
//! 1. **Fetch & cache JWKS** from the OIDC issuer endpoint
//!    ([`RemoteJwksAuth::initialize`] fetches once; the HTTP server additionally
//!    runs the `axum-jwt-auth` background refresh task).
//! 2. **Validate a bearer token** against those keys
//!    ([`RemoteJwksAuth::authenticate`]).
//! 3. **Extract the caller identity** (`sub`) and any expiry from the validated
//!    token ([`crate::jwt::JwtClaims`]).
//! 4. **Normalise user input**: both a bare JWT and a full
//!    `Bearer <token>` header value are accepted ([`strip_bearer_prefix`]).
//!
//! How each crate uses it:
//!
//! * **CLI** — builds a [`RemoteJwksAuth`], calls [`initialize`] + [`authenticate`]
//!   on a `--token`, and supplies the raw token to the HTTP server as
//!   `Authorization: Bearer <token>` via [`RemoteJwksAuth::bearer_auth_header`].
//! * **HTTP server** — builds the same [`RemoteJwksAuth`], passes the underlying
//!   decoder into `axum_jwt_auth::JwtDecoderState`, and keeps relying on the
//!   `Claims<JwtClaims>` extractor's injection into handlers. The validation /
//!   `sub` extraction logic is thereby shared while the server's claim-injection
//!   behaviour is unchanged.

use axum_jwt_auth::{JwtDecoder, RemoteJwksDecoder};
use jsonwebtoken_10::{Algorithm, Validation};
use std::fmt;
use tokio_util::sync::CancellationToken;

use crate::jwt::JwtClaims;

/// Claims carried by a token that has been validated against the JWKS.
///
/// This is simply a re-export of [`crate::jwt::JwtClaims`] under a more
/// descriptive name for the authentication context.
pub type AuthenticatedClaims = JwtClaims;

/// JWKS configuration used to validate incoming bearer tokens.
///
/// This mirrors the issuer whose tokens both the CLI and the HTTP server
/// accept; keeping it here means a single source of truth for the JWKS URL.
pub const JWKS_URL: &str = "https://ska-iam.stfc.ac.uk/jwk";

/// The token issuer enforced during validation. Kept alongside [`JWKS_URL`] so
/// that a token's signature *and* issuer are both checked against the same
/// trusted OIDC provider.
pub const ISSUER: &str = "https://ska-iam.stfc.ac.uk/";

/// Strips a leading `Bearer ` prefix (case-insensitive, ASCII) from an
/// authorization value, returning the raw JWT.
///
/// Accepting both forms lets callers pass either a raw token or a full HTTP
/// `Authorization` header value without special-casing.
pub fn strip_bearer_prefix(value: &str) -> &str {
    let trimmed = value.trim();
    trimmed
        .strip_prefix("Bearer ")
        .or_else(|| trimmed.strip_prefix("bearer "))
        .map(str::trim)
        .unwrap_or(trimmed)
}

/// Errors that can occur while validating a bearer token against the JWKS.
#[derive(Debug)]
pub enum AuthError {
    /// The supplied token failed signature verification or validation.
    InvalidToken(String),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthError::InvalidToken(msg) => write!(f, "invalid bearer token: {}", msg),
        }
    }
}

impl std::error::Error for AuthError {}

/// Shared remote-JWKS authenticator.
///
/// Wraps the `axum-jwt-auth` remote decoder so both the CLI and the HTTP
/// server validate tokens with identical configuration. The underlying
/// [`RemoteJwksDecoder`] is exposed via [`RemoteJwksAuth::decoder`] so the HTTP
/// server can plug it straight into `JwtDecoderState` for the `Claims`
/// extractor, keeping claim injection intact.
#[derive(Clone)]
pub struct RemoteJwksAuth {
    decoder: RemoteJwksDecoder,
}

impl RemoteJwksAuth {
    /// Builds an authenticator for the default [`JWKS_URL`], enforcing [`ISSUER`].
    pub fn new() -> Result<Self, axum_jwt_auth::Error> {
        Self::for_url(JWKS_URL, ISSUER)
    }

    /// Builds an authenticator for a specific JWKS endpoint and issuer (useful
    /// for tests).
    pub fn for_url(jwks_url: &str, issuer: &str) -> Result<Self, axum_jwt_auth::Error> {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[issuer]);
        // Accept tokens that either omit an `aud` claim or list the SKA authn
        // API as their intended audience.
        validation.set_audience(&["authn-api"]);
        let decoder = RemoteJwksDecoder::builder()
            .jwks_url(jwks_url.to_string())
            .validation(validation)
            .build()?;
        Ok(Self { decoder })
    }

    /// Fetches the JWKS once, populating the decoder's key cache.
    ///
    /// Use this from short-lived processes (the CLI) where no ongoing refresh
    /// is required.
    pub async fn initialize(&self) -> Result<(), axum_jwt_auth::Error> {
        self.decoder.refresh().await
    }

    /// Fetches the JWKS and starts the background refresh task.
    ///
    /// Use this from long-lived processes (the HTTP server). Returns the
    /// [`CancellationToken`] used to stop the refresh task on shutdown.
    pub async fn initialize_with_refresh(&self) -> Result<CancellationToken, axum_jwt_auth::Error> {
        self.decoder.initialize().await
    }

    /// Validates a bearer token (raw JWT or full `Bearer <token>` header)
    /// against the JWKS and returns the verified claims, including `sub`.
    ///
    /// The keys must already be cached — call [`initialize`] /
    /// [`initialize_with_refresh`] first.
    ///
    /// [`initialize`]: RemoteJwksAuth::initialize
    /// [`initialize_with_refresh`]: RemoteJwksAuth::initialize_with_refresh
    pub async fn authenticate(
        &self,
        authorization_value: &str,
    ) -> Result<AuthenticatedClaims, AuthError> {
        let token = strip_bearer_prefix(authorization_value);
        let token_data = self
            .decoder
            .decode(token)
            .await
            .map_err(|e| AuthError::InvalidToken(format!("{}", e)))?;
        Ok(token_data.claims)
    }

    /// Formats a token as an HTTP `Authorization` header value.
    ///
    /// The CLI uses this to forward the already-validated raw token to the HTTP
    /// server.
    pub fn bearer_auth_header(&self, token: &str) -> String {
        format!("Bearer {}", strip_bearer_prefix(token))
    }

    /// The underlying decoder, e.g. for plugging into the HTTP server's
    /// `JwtDecoderState` so the `Claims<JwtClaims>` extractor keeps working.
    pub fn decoder(&self) -> RemoteJwksDecoder {
        self.decoder.clone()
    }
}
