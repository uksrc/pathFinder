mod cli;

use anyhow::{Context, Result};
use clap::Parser;
use pathfinder_shared::{mount, oauth2::authenticate, path_finder::run};
use std::env;

use cli::{check_privileges, get_tokens_from_env, Args};

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

    // TODO: Add persistent store to the CLI
    run(&args.namespace, &args.file_name, &tokens)
}
