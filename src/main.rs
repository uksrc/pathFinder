mod path_finder;
mod api_client;
mod cli;
mod models;
mod mount;
mod oauth2;

use anyhow::{Context, Result};
use clap::Parser;
use std::env;

use cli::{check_privileges, get_tokens_from_env, Args};
use oauth2::authenticate;
use path_finder::run;

fn main() -> Result<()> {
    let args = Args::parse();

    check_privileges(&args)?;

    // Handle unmount operation (no API calls needed)
    if args.unmount {
        let sudo_user = env::var("SUDO_USER").context("SUDO_USER not set")?;

        let fits_path = format!("/{}/{}", args.namespace, args.file_name);
        mount::unmount_operation(&fits_path, &args.namespace, &sudo_user)?;
        return Ok(());
    }

    // Mount operation requires authentication and API calls
    let tokens = if args.no_login {
        get_tokens_from_env()?
    } else {
        println!("Authenticating with OAuth2...");
        let tokens = authenticate(true)?;
        println!("Authentication successful!");
        tokens
    };

    run(&args.namespace, &args.file_name, &tokens)
}
