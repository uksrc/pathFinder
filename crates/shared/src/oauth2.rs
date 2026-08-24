//! OAuth2 device code flow authentication for the SRCNet APIs.
//!
//! Implements the device authorisation grant. The user is directed to a
//! browser URL to authenticate; once approved the resulting OIDC token is exchanged
//! for API-specific access tokens for the Data Management and Site Capabilities APIs.
//! Tokens are cached on disk (mode `0600`) and reused until they expire.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

pub const AUTHN_BASE_URL: &str = "https://authn.srcnet.skao.int/api/v1";
pub const DATA_MANAGEMENT: &str = "data-management-api";
pub const SITE_CAPABILITIES: &str = "site-capabilities-api";

/// API access tokens for the Data Management and Site Capabilities APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub data_management_token: String,
    pub site_capabilities_token: String,
}

/// Response from the device code initiation endpoint (`GET /login/device`).
#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    /// Opaque code used to poll for the access token.
    device_code: String,
    /// Short human-readable code shown to the user.
    user_code: String,
    /// URL the user must visit to complete authentication.
    verification_uri: String,
    /// Recommended polling interval in seconds (default: 5).
    #[serde(default = "default_interval")]
    interval: u64,
    /// Token lifetime in seconds — present in the response but not used by this client.
    #[serde(default)]
    _expires_in: Option<u64>,
}

fn default_interval() -> u64 {
    5
}

/// Response from the token polling endpoint (`GET /token`) and the token exchange endpoint.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    /// Nested token object returned by some API versions.
    token: Option<TokenData>,
    /// Flat access token returned by other API versions.
    access_token: Option<String>,
    /// OAuth2 error code (e.g. `"authorization_pending"`, `"slow_down"`).
    error: Option<String>,
    /// Human-readable elaboration of `error`.
    error_description: Option<String>,
    /// Detail string that may embed a JSON error payload.
    detail: Option<String>,
}

/// Nested token object within a [`TokenResponse`].
#[derive(Debug, Deserialize)]
struct TokenData {
    access_token: String,
}

/// Tokens serialised to the on-disk cache file, together with their expiry timestamp.
#[derive(Debug, Serialize, Deserialize)]
struct CachedTokens {
    tokens: Tokens,
    /// Unix timestamp (seconds since the epoch) after which the tokens are considered expired.
    expires_at: u64,
}

/// Authenticates the user via the OAuth2 device code flow and returns API access tokens.
///
/// If `use_cache` is `true` and a valid (non-expired) token set exists on disk, those tokens
/// are returned immediately without prompting the user. Otherwise the full device-code flow is
/// performed: the user is directed to a browser URL, and once authenticated the resulting tokens
/// are cached for subsequent calls.
///
/// This is a synchronous function, but calls the underlying async function with a blocking command
pub fn authenticate(use_cache: bool) -> Result<Tokens> {
    tokio::runtime::Runtime::new()
        .context("Failed to create runtime")?
        .block_on(authenticate_impl(use_cache, AUTHN_BASE_URL, None))
}

/// Inner implementation of [`authenticate`] with injectable base URL and cache path for testing.
async fn authenticate_impl(
    use_cache: bool,
    base_url: &str,
    cache_path: Option<&Path>,
) -> Result<Tokens> {
    // If caching is enabled, check for valid cached tokens — if found, re-save (refresh expiry) and return
    if use_cache {
        if let Some(cached) = load_tokens(cache_path)? {
            let default_path = get_token_cache_path()?;
            let path = cache_path.unwrap_or(&default_path);
            save_tokens_to_path(&cached, 3600, path)?;
            return Ok(cached);
        }
    }

    let client = Client::new();

    let device_info = initiate_device_code_flow(&client, base_url).await?;
    display_user_instructions(&device_info);

    let auth_token = poll_for_authentication(
        &client,
        base_url,
        &device_info.device_code,
        device_info.interval,
    )
    .await?;

    let tokens = obtain_api_tokens(&client, base_url, &auth_token).await?;

    save_tokens(cache_path, &tokens, 3600)?;

    Ok(tokens)
}

pub async fn async_obtain_api_tokens(auth_token: &String) -> Result<Tokens> {
    tracing::debug!(
        "obtain_api_tokens_external called with {}",
        shorten_token(auth_token)
    );
    let client = Client::new();
    obtain_api_tokens(&client, AUTHN_BASE_URL, auth_token).await
}

fn shorten_token(auth_token: &String) -> String {
    let s = auth_token.as_str();
    if s.chars().count() <= 10 {
        return auth_token.clone();
    }

    let start = s.chars().take(5).collect::<String>();
    let end = s.chars().rev().take(5).collect::<String>();
    format!("{}...{}", start, end)
}

async fn obtain_api_tokens(client: &Client, base_url: &str, auth_token: &String) -> Result<Tokens> {
    let dm_token =
        exchange_token_for_api_token(&client, base_url, &auth_token, DATA_MANAGEMENT).await?;
    let sc_token =
        exchange_token_for_api_token(&client, base_url, &auth_token, SITE_CAPABILITIES).await?;

    let tokens = Tokens {
        data_management_token: dm_token,
        site_capabilities_token: sc_token,
    };

    Ok(tokens)
}

/// Initiates the device code flow by calling `GET /login/device`.
///
/// Returns the server's [`DeviceCodeResponse`] containing the `device_code` to poll with
/// and the `verification_uri` + `user_code` to display to the user.
async fn initiate_device_code_flow(client: &Client, base_url: &str) -> Result<DeviceCodeResponse> {
    let url = format!("{}/login/device", base_url);
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .context("Failed to initiate device code flow")?;

    response
        .error_for_status()
        .context("Device code flow request failed")?
        .json()
        .await
        .context("Failed to parse device code response")
}

/// Prints the authentication URL and user code to stdout so the user knows where to go.
fn display_user_instructions(device_info: &DeviceCodeResponse) {
    println!("\nACTION REQUIRED:");
    println!(
        "    Open this URL in a browser and authenticate: {}?user_code={}",
        device_info.verification_uri, device_info.user_code
    );
    println!("\nWaiting for authentication (timeout: 5 minutes)...");
}

/// Polls `GET /token` until the user authorises the device or the 5-minute timeout is reached.
///
/// Handles the following RFC 8628 polling error codes:
/// - `authorization_pending` — keeps polling at the current interval
/// - `slow_down` — backs off by 5 seconds and keeps polling
/// - `expired_token` / `access_denied` — bails immediately with a descriptive error
async fn poll_for_authentication(
    client: &Client,
    base_url: &str,
    device_code: &str,
    mut interval: u64,
) -> Result<String> {
    let timeout = Duration::from_secs(300);
    let start = SystemTime::now();

    loop {
        if start.elapsed()? > timeout {
            anyhow::bail!("Authorization timeout. Please try again.");
        }

        let url = format!("{}/token", base_url);
        let response = client
            .get(&url)
            .query(&[("device_code", device_code)])
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .context("Failed to poll for authentication")?;

        if response.status().is_success() {
            let token_data: TokenResponse = response
                .json()
                .await
                .context("Unable to parse JSON from token exchange response.")?;

            if let Some(token) = token_data.token {
                return Ok(token.access_token);
            } else if let Some(access_token) = token_data.access_token {
                return Ok(access_token);
            } else {
                anyhow::bail!("No access token in response");
            }
        }

        let error_data: TokenResponse = response
            .json()
            .await
            .context("Unable to parse JSON from token exchange error response.")?;
        let error = parse_error_response(&error_data);

        match error.as_deref() {
            Some("authorization_pending") => {
                thread::sleep(Duration::from_secs(interval));
                continue;
            }
            Some("slow_down") => {
                interval += 5;
                thread::sleep(Duration::from_secs(interval));
                continue;
            }
            Some("expired_token") => {
                anyhow::bail!("Device code expired. Please try again.");
            }
            Some("access_denied") => {
                anyhow::bail!("User denied authorization.");
            }
            Some(err) => {
                let msg = error_data
                    .error_description
                    .map(|d| format!("{}: {}", err, d))
                    .unwrap_or_else(|| err.to_string());
                anyhow::bail!("Authorization error: {}", msg);
            }
            None => {
                anyhow::bail!("Unknown authorization error");
            }
        }
    }
}

/// Extracts the OAuth2 error code from a [`TokenResponse`].
///
/// Some API responses embed a JSON error payload inside a `detail` string of the form
/// `"response: {...}"`. This function attempts to parse that first before falling back
/// to the top-level `error` field.
fn parse_error_response(error_data: &TokenResponse) -> Option<String> {
    if let Some(detail) = &error_data.detail {
        // Try to extract JSON from "response: {...}" pattern
        if let Some(start) = detail.find("response:") {
            let json_part = detail[start + 9..].trim();
            if let Ok(embedded) = serde_json::from_str::<TokenResponse>(json_part) {
                if embedded.error.is_some() {
                    return embedded.error;
                }
            }
        }
    }
    error_data.error.clone()
}

/// Exchanges the OIDC auth token for an API-specific access token via
/// `GET /token/exchange/<api_name>`.
pub async fn exchange_token_for_api_token(
    client: &Client,
    base_url: &str,
    auth_token: &str,
    api_name: &str,
) -> Result<String> {
    let url = format!("{}/token/exchange/{}", base_url, api_name);
    let response = client
        .get(&url)
        .header("Content-Type", "application/json")
        .query(&[
            ("version", "latest"),
            ("try_use_cache", "false"),
            ("access_token", auth_token),
        ])
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .with_context(|| format!("Failed to exchange token for {} API", api_name))?;

    let token_data: TokenResponse = response
        .error_for_status()?
        .json()
        .await
        .with_context(|| format!("Token exchange failed for {}", api_name))?;

    if let Some(token) = token_data.token {
        Ok(token.access_token)
    } else if let Some(access_token) = token_data.access_token {
        Ok(access_token)
    } else {
        anyhow::bail!("No access token in response for {}", api_name)
    }
}

/// Returns the path to the on-disk token cache file, creating intermediate directories if needed.
fn get_token_cache_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .context("Failed to find config directory")?
        .join("path-finder");

    fs::create_dir_all(&config_dir)?;
    Ok(config_dir.join("tokens.json"))
}

/// Saves tokens to the cache, using `cache_path` if provided or the default path otherwise.
fn save_tokens(cache_path: Option<&Path>, tokens: &Tokens, expires_in: u64) -> Result<()> {
    let default_path;
    let path = match cache_path {
        Some(p) => p,
        None => {
            default_path = get_token_cache_path()?;
            &default_path
        }
    };
    save_tokens_to_path(tokens, expires_in, path)
}

/// Serialises `tokens` to `path` with a Unix timestamp expiry, setting file permissions to `0600`.
fn save_tokens_to_path(tokens: &Tokens, expires_in: u64, path: &Path) -> Result<()> {
    let expires_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs()
        + expires_in;

    let cached = CachedTokens {
        tokens: tokens.clone(),
        expires_at,
    };

    let json = serde_json::to_string_pretty(&cached)?;
    fs::write(path, json)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
    }

    println!("Tokens cached for {} seconds", expires_in);
    Ok(())
}

/// Loads tokens from the cache at `cache_path`, or the default path if `None`.
///
/// Returns `None` if the cache file does not exist or the tokens have expired.
fn load_tokens(cache_path: Option<&Path>) -> Result<Option<Tokens>> {
    let default_path;
    let path = match cache_path {
        Some(p) => p,
        None => {
            default_path = get_token_cache_path()?;
            &default_path
        }
    };
    load_tokens_from_path(path)
}

/// Reads and deserialises tokens from `path`, returning `None` if absent or expired.
fn load_tokens_from_path(path: &Path) -> Result<Option<Tokens>> {
    if !path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(path)?;
    let cached: CachedTokens = serde_json::from_str(&contents).context("Invalid cache file")?;

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs();

    if now >= cached.expires_at {
        println!("Cached tokens expired");
        return Ok(None);
    }

    println!("Using cached tokens");
    Ok(Some(cached.tokens))
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use tempfile::TempDir;

    fn make_tokens() -> Tokens {
        Tokens {
            data_management_token: "dm-token-abc".to_string(),
            site_capabilities_token: "sc-token-xyz".to_string(),
        }
    }

    // --- parse_error_response ---

    #[test]
    fn parse_error_returns_none_when_no_error_fields() {
        let resp = TokenResponse {
            token: None,
            access_token: None,
            error: None,
            error_description: None,
            detail: None,
        };
        assert!(parse_error_response(&resp).is_none());
    }

    #[test]
    fn parse_error_returns_top_level_error_field() {
        let resp = TokenResponse {
            token: None,
            access_token: None,
            error: Some("access_denied".to_string()),
            error_description: None,
            detail: None,
        };
        assert_eq!(
            parse_error_response(&resp).as_deref(),
            Some("access_denied")
        );
    }

    #[test]
    fn parse_error_extracts_error_from_embedded_detail_json() {
        let resp = TokenResponse {
            token: None,
            access_token: None,
            error: None,
            error_description: None,
            detail: Some(
                r#"response: {"error": "authorization_pending", "error_description": null}"#
                    .to_string(),
            ),
        };
        assert_eq!(
            parse_error_response(&resp).as_deref(),
            Some("authorization_pending")
        );
    }

    #[test]
    fn parse_error_falls_back_to_top_level_when_detail_json_is_malformed() {
        let resp = TokenResponse {
            token: None,
            access_token: None,
            error: Some("slow_down".to_string()),
            error_description: None,
            detail: Some("response: not valid json at all".to_string()),
        };
        assert_eq!(parse_error_response(&resp).as_deref(), Some("slow_down"));
    }

    #[test]
    fn parse_error_prefers_embedded_detail_over_top_level_error() {
        let resp = TokenResponse {
            token: None,
            access_token: None,
            error: Some("top_level_error".to_string()),
            error_description: None,
            detail: Some(r#"response: {"error": "embedded_error"}"#.to_string()),
        };
        assert_eq!(
            parse_error_response(&resp).as_deref(),
            Some("embedded_error")
        );
    }

    // --- token cache round-trip ---

    #[test]
    fn cache_round_trip_returns_same_tokens() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json");
        let tokens = make_tokens();

        save_tokens_to_path(&tokens, 3600, &path).unwrap();
        let loaded = load_tokens_from_path(&path).unwrap().unwrap();

        assert_eq!(loaded.data_management_token, tokens.data_management_token);
        assert_eq!(
            loaded.site_capabilities_token,
            tokens.site_capabilities_token
        );
    }

    #[test]
    fn load_tokens_returns_none_when_file_absent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json");

        let result = load_tokens_from_path(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_tokens_returns_none_when_expired() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json");

        // Write a cache file whose expires_at is already in the past.
        let expired = CachedTokens {
            tokens: make_tokens(),
            expires_at: 1, // 1970 — definitely expired
        };
        fs::write(&path, serde_json::to_string(&expired).unwrap()).unwrap();

        let result = load_tokens_from_path(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_tokens_errors_on_malformed_json() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json");
        fs::write(&path, "not json").unwrap();

        let err = load_tokens_from_path(&path).unwrap_err();
        assert!(err.to_string().contains("Invalid cache file"), "{err}");
    }

    // --- initiate_device_code_flow ---

    #[test]
    fn initiate_device_code_flow_parses_success_response() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/login/device");
            then.status(200).body(
                r#"{
                    "device_code": "dev-code-abc",
                    "user_code": "ABCD-1234",
                    "verification_uri": "https://authn.srcnet.skao.int/device",
                    "interval": 5
                }"#,
            );
        });

        let client = Client::new();
        let resp = initiate_device_code_flow(&client, &server.base_url()).unwrap();
        assert_eq!(resp.device_code, "dev-code-abc");
        assert_eq!(resp.user_code, "ABCD-1234");
        assert_eq!(resp.interval, 5);
    }

    #[test]
    fn initiate_device_code_flow_uses_default_interval_when_absent() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/login/device");
            then.status(200).body(
                r#"{
                    "device_code": "dev-code-abc",
                    "user_code": "ABCD-1234",
                    "verification_uri": "https://authn.srcnet.skao.int/device"
                }"#,
            );
        });

        let client = Client::new();
        let resp = initiate_device_code_flow(&client, &server.base_url()).unwrap();
        assert_eq!(resp.interval, 5);
    }

    #[test]
    fn initiate_device_code_flow_propagates_http_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/login/device");
            then.status(500);
        });

        let client = Client::new();
        let err = initiate_device_code_flow(&client, &server.base_url()).unwrap_err();
        assert!(
            err.to_string().contains("Device code flow request failed"),
            "{err}"
        );
    }

    // --- poll_for_authentication ---

    #[test]
    fn poll_returns_nested_token_on_success() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/token");
            then.status(200)
                .body(r#"{"token": {"access_token": "oidc-token-abc"}}"#);
        });

        let client = Client::new();
        let token = poll_for_authentication(&client, &server.base_url(), "dev-code", 0).unwrap();
        assert_eq!(token, "oidc-token-abc");
    }

    #[test]
    fn poll_returns_flat_access_token_on_success() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/token");
            then.status(200)
                .body(r#"{"access_token": "oidc-token-flat"}"#);
        });

        let client = Client::new();
        let token = poll_for_authentication(&client, &server.base_url(), "dev-code", 0).unwrap();
        assert_eq!(token, "oidc-token-flat");
    }

    #[test]
    fn poll_errors_on_expired_token() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/token");
            then.status(400).body(r#"{"error": "expired_token"}"#);
        });

        let client = Client::new();
        let err = poll_for_authentication(&client, &server.base_url(), "dev-code", 0).unwrap_err();
        assert!(err.to_string().contains("expired"), "{err}");
    }

    #[test]
    fn poll_errors_on_access_denied() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/token");
            then.status(400).body(r#"{"error": "access_denied"}"#);
        });

        let client = Client::new();
        let err = poll_for_authentication(&client, &server.base_url(), "dev-code", 0).unwrap_err();
        assert!(err.to_string().contains("denied"), "{err}");
    }

    // --- exchange_token_for_api_token ---

    #[test]
    fn exchange_token_returns_nested_token() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/token/exchange/data-management-api");
            then.status(200)
                .body(r#"{"token": {"access_token": "dm-token-abc"}}"#);
        });

        let client = Client::new();
        let token = exchange_token_for_api_token(
            &client,
            &server.base_url(),
            "oidc-token",
            "data-management-api",
        )
        .unwrap();
        assert_eq!(token, "dm-token-abc");
    }

    #[test]
    fn exchange_token_returns_flat_access_token() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/token/exchange/site-capabilities-api");
            then.status(200)
                .body(r#"{"access_token": "sc-token-flat"}"#);
        });

        let client = Client::new();
        let token = exchange_token_for_api_token(
            &client,
            &server.base_url(),
            "oidc-token",
            "site-capabilities-api",
        )
        .unwrap();
        assert_eq!(token, "sc-token-flat");
    }

    #[test]
    fn exchange_token_propagates_http_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/token/exchange/data-management-api");
            then.status(401);
        });

        let client = Client::new();
        let err = exchange_token_for_api_token(
            &client,
            &server.base_url(),
            "oidc-token",
            "data-management-api",
        )
        .unwrap_err();
        assert!(err.to_string().contains("Token exchange failed"), "{err}");
    }

    #[test]
    fn exchange_token_errors_when_no_token_in_response() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/token/exchange/data-management-api");
            then.status(200).body(r#"{"detail": "some_other_field"}"#);
        });

        let client = Client::new();
        let err = exchange_token_for_api_token(
            &client,
            &server.base_url(),
            "oidc-token",
            "data-management-api",
        )
        .unwrap_err();
        assert!(err.to_string().contains("No access token"), "{err}");
    }
}
