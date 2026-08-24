mod cli;

use anyhow::{Context, Result};
use clap::Parser;
use pathfinder_shared::{
    jwks_auth::RemoteJwksAuth,
    mount,
    oauth2::{
        authenticate, exchange_token_for_api_token, Tokens, AUTHN_BASE_URL, DATA_MANAGEMENT,
        SITE_CAPABILITIES,
    },
    path_finder::run,
};
use reqwest::Client;
use std::env;

use cli::{check_privileges, resolve_auth_token, Args};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    check_privileges(&args)?;

    // Handle unmount operation (no API calls needed)
    if args.unmount {
        let sudo_user = env::var("SUDO_USER").context("SUDO_USER not set")?;

        let fits_path = format!("/{}/{}", args.namespace, args.file_name);
        mount::unmount_operation(&fits_path, &args.namespace, &sudo_user)?;
        return Ok(());
    }

    // Mount operation requires authentication and API calls.
    //
    // Three modes:
    //   * no --token            → interactive OAuth2 device-code flow
    //   * --token <TOKEN>       → validate TOKEN against JWKS and exchange it
    //   * --token               → use PATHFINDER_SKA_AUTH_TOKEN env var
    let tokens = match resolve_auth_token(&args)? {
        Some(token) => {
            // Validate the token against the OIDC JWKS and extract the caller identity.
            // The CLI expects a raw JWT without a "Bearer " prefix.
            let raw_token = token.trim();
            println!("Validating bearer token against the JWKS...");
            let auth = RemoteJwksAuth::new().context("failed to build JWKS authenticator")?;
            auth.initialize()
                .await
                .context("failed to fetch JWKS for token validation")?;
            let claims = auth
                .authenticate(raw_token)
                .await
                .context("provided token failed JWKS validation")?;
            println!("Authenticated as subject '{}'", claims.sub);

            // Exchange the validated bearer token for the SRCNet API tokens.
            exchange_token_for_api_tokens(raw_token).await?
        }
        None => {
            println!("Authenticating with OAuth2...");
            let tokens = authenticate(true)?;
            println!("Authentication successful!");
            tokens
        }
    };

    // TODO: Add persistent store to the CLI
    run(&args.namespace, &args.file_name, &tokens)
}

/// Exchanges the validated OIDC bearer token for Data Management and Site
/// Capabilities API tokens via [`exchange_token_for_api_token`].
async fn exchange_token_for_api_tokens(auth_token: &str) -> Result<Tokens> {
    let client = Client::new();
    let dm_token =
        exchange_token_for_api_token(&client, AUTHN_BASE_URL, auth_token, DATA_MANAGEMENT)
            .await
            .context("failed to exchange token for Data Management API")?;
    let sc_token =
        exchange_token_for_api_token(&client, AUTHN_BASE_URL, auth_token, SITE_CAPABILITIES)
            .await
            .context("failed to exchange token for Site Capabilities API")?;

    Ok(Tokens {
        data_management_token: dm_token,
        site_capabilities_token: sc_token,
    })
}
