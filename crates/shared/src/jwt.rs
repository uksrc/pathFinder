//! Minimal JWT validation for incoming bearer tokens.
//!
//! The service cannot verify token signatures locally (it has no issuer
//! keys), so validation here is limited to checking that the token is a
//! well-formed JWT, that it carries a non-empty `sub` claim, and that it
//! has not expired (when an `exp` claim is present).

// TODO: Use proper JWT-based auth for the HTTP Service - e.g. axum-jwt-auth

use anyhow::{anyhow, bail};
use chrono::Utc;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct JwtClaims {
    pub sub: String,
    #[serde(default)]
    pub exp: Option<i64>,
}

/// Decode `token` as a JWT and return its claims.
///
/// Fails if the token is not a well-formed JWT, if the `sub` claim is
/// missing or empty, or if the token has expired.
pub fn validate_jwt(token: &str) -> anyhow::Result<JwtClaims> {
    let token_data = jsonwebtoken::dangerous::insecure_decode::<JwtClaims>(token)
        .map_err(|e| anyhow!("token is not a valid JWT: {e}"))?;

    let claims = token_data.claims;
    if claims.sub.trim().is_empty() {
        bail!("JWT is missing a 'sub' claim");
    }
    if let Some(exp) = claims.exp {
        if exp <= Utc::now().timestamp() {
            bail!("JWT has expired");
        }
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde::Serialize;

    fn make_token(claims: &impl Serialize) -> String {
        encode(
            &Header::default(),
            claims,
            &EncodingKey::from_secret(b"test-secret"),
        )
        .unwrap()
    }

    #[derive(Serialize)]
    struct TestClaims {
        sub: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        exp: Option<i64>,
    }

    #[test]
    fn extracts_sub_from_valid_token() {
        let token = make_token(&TestClaims {
            sub: "alice".to_string(),
            exp: Some(Utc::now().timestamp() + 3600),
        });
        let claims = validate_jwt(&token).unwrap();
        assert_eq!(claims.sub, "alice");
    }

    #[test]
    fn accepts_token_without_exp() {
        let token = make_token(&TestClaims {
            sub: "bob".to_string(),
            exp: None,
        });
        assert_eq!(validate_jwt(&token).unwrap().sub, "bob");
    }

    #[test]
    fn rejects_expired_token() {
        let token = make_token(&TestClaims {
            sub: "alice".to_string(),
            exp: Some(Utc::now().timestamp() - 10),
        });
        let err = validate_jwt(&token).unwrap_err();
        assert!(err.to_string().contains("expired"), "got: {err}");
    }

    #[test]
    fn rejects_empty_sub() {
        let token = make_token(&TestClaims {
            sub: "  ".to_string(),
            exp: None,
        });
        let err = validate_jwt(&token).unwrap_err();
        assert!(err.to_string().contains("sub"), "got: {err}");
    }

    #[derive(Serialize)]
    struct NoSubClaims {
        scope: String,
    }

    #[test]
    fn rejects_missing_sub() {
        let token = make_token(&NoSubClaims {
            scope: "read".to_string(),
        });
        assert!(validate_jwt(&token).is_err());
    }

    #[test]
    fn rejects_malformed_tokens() {
        assert!(validate_jwt("not-a-jwt").is_err());
        assert!(validate_jwt("a.b").is_err());
        assert!(validate_jwt("a.b.c.d").is_err());
        assert!(validate_jwt("").is_err());
    }
}
