use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime};

const AUTHN_BASE_URL: &str = "https://authn.srcnet.skao.int/api/v1";
const DATA_MANAGEMENT: &str = "data-management-api";
const SITE_CAPABILITIES: &str = "site-capabilities-api";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub data_management_token: String,
    pub site_capabilities_token: String,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default = "default_interval")]
    interval: u64,
    #[serde(default)]
    _expires_in: Option<u64>,
}

fn default_interval() -> u64 {
    5
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    token: Option<TokenData>,
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
    detail: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenData {
    access_token: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedTokens {
    tokens: Tokens,
    expires_at: u64,
}

pub fn authenticate(use_cache: bool) -> Result<Tokens> {
    if use_cache {
        if let Some(cached) = load_tokens_from_cache()? {
            return Ok(cached);
        }
    }

    let client = Client::new();

    let device_info = initiate_device_code_flow(&client)?;
    display_user_instructions(&device_info);

    let auth_token = poll_for_authentication(&client, &device_info.device_code, device_info.interval)?;

    let dm_token = exchange_token_for_api_token(&client, &auth_token, DATA_MANAGEMENT)?;
    let sc_token = exchange_token_for_api_token(&client, &auth_token, SITE_CAPABILITIES)?;

    let tokens = Tokens {
        data_management_token: dm_token,
        site_capabilities_token: sc_token,
    };

    save_tokens_to_cache(&tokens, 3600)?;

    Ok(tokens)
}

fn initiate_device_code_flow(client: &Client) -> Result<DeviceCodeResponse> {
    let url = format!("{}/login/device", AUTHN_BASE_URL);
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .context("Failed to initiate device code flow")?;

    response
        .error_for_status()
        .context("Device code flow request failed")?
        .json()
        .context("Failed to parse device code response")
}

fn display_user_instructions(device_info: &DeviceCodeResponse) {
    println!("\nACTION REQUIRED:");
    println!("    Open this URL in a browser and authenticate: {}?user_code={}",
        device_info.verification_uri, device_info.user_code);
    println!("\nWaiting for authentication (timeout: 5 minutes)...");
}

fn poll_for_authentication(client: &Client, device_code: &str, mut interval: u64) -> Result<String> {
    let timeout = Duration::from_secs(300);
    let start = SystemTime::now();

    loop {
        if start.elapsed()? > timeout {
            anyhow::bail!("Authorization timeout. Please try again.");
        }

        let url = format!("{}/token", AUTHN_BASE_URL);
        let response = client
            .get(&url)
            .query(&[("device_code", device_code)])
            .timeout(Duration::from_secs(10))
            .send()
            .context("Failed to poll for authentication")?;

        if response.status().is_success() {
            let token_data: TokenResponse = response.json()?;

            if let Some(token) = token_data.token {
                return Ok(token.access_token);
            } else if let Some(access_token) = token_data.access_token {
                return Ok(access_token);
            } else {
                anyhow::bail!("No access token in response");
            }
        }

        let error_data: TokenResponse = response.json()?;
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
                let msg = error_data.error_description
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

fn parse_error_response(error_data: &TokenResponse) -> Option<String> {
    if let Some(detail) = &error_data.detail {
        // Try to extract JSON from "response: {...}" pattern
        if let Some(start) = detail.find("response:") {
            let json_part = &detail[start + 9..].trim();
            if let Ok(embedded) = serde_json::from_str::<TokenResponse>(json_part) {
                if embedded.error.is_some() {
                    return embedded.error;
                }
            }
        }
    }
    error_data.error.clone()
}

fn exchange_token_for_api_token(client: &Client, auth_token: &str, api_name: &str) -> Result<String> {
    let url = format!("{}/token/exchange/{}", AUTHN_BASE_URL, api_name);
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
        .with_context(|| format!("Failed to exchange token for {} API", api_name))?;

    let token_data: TokenResponse = response
        .error_for_status()
        .with_context(|| format!("Token exchange failed for {}", api_name))?
        .json()?;

    if let Some(token) = token_data.token {
        Ok(token.access_token)
    } else if let Some(access_token) = token_data.access_token {
        Ok(access_token)
    } else {
        anyhow::bail!("No access token in response for {}", api_name)
    }
}

fn get_token_cache_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .context("Failed to find config directory")?
        .join("path-finder");

    fs::create_dir_all(&config_dir)?;
    Ok(config_dir.join("tokens.json"))
}

fn save_tokens_to_cache(tokens: &Tokens, expires_in: u64) -> Result<()> {
    let cache_path = get_token_cache_path()?;
    let expires_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs() + expires_in;

    let cached = CachedTokens {
        tokens: tokens.clone(),
        expires_at,
    };

    let json = serde_json::to_string_pretty(&cached)?;
    fs::write(&cache_path, json)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&cache_path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&cache_path, perms)?;
    }

    println!("Tokens cached for {} seconds", expires_in);
    Ok(())
}

fn load_tokens_from_cache() -> Result<Option<Tokens>> {
    let cache_path = get_token_cache_path()?;

    if !cache_path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&cache_path)?;
    let cached: CachedTokens = serde_json::from_str(&contents)
        .context("Invalid cache file")?;

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
