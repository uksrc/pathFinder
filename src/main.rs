mod api_client;
mod cli;
mod models;
mod mount;
mod oauth2;
mod path_finder;

use anyhow::{Context, Result};
use clap::Parser;
use std::env;
use std::process::exit;

use api_client::{ApiClient, PathFinderApiClient};
use cli::{Args, check_privileges, get_tokens_from_env};
use oauth2::{authenticate, Tokens};

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

fn run(namespace: &str, file_name: &str, tokens: &Tokens) -> Result<()> {
    let client = ApiClient::new(
        tokens.data_management_token.clone(),
        tokens.site_capabilities_token.clone(),
    );

    client.check_namespace_available(namespace)?;

    let site_storages = client.site_storage_areas()?;
    let data_locations = client.locate_data(namespace, file_name)?;

    path_finder::print_data_locations_with_sites(&site_storages, &data_locations);

    let rse_path = path_finder::extract_rse_path(&data_locations, namespace, file_name)?;
    println!(
        "RSE Path for file '{}' in namespace '{}': {}",
        file_name, namespace, rse_path
    );

    // Check if the file exists locally
    if !path_finder::check_local_file_exists(&rse_path) {
        println!("\n⚠️  File not found locally! ⚠️");
        println!("\nThe file is available at the following locations:");
        path_finder::print_data_locations_with_sites(&site_storages, &data_locations);
        println!("\nPlease ensure the data has been staged to this local site before mounting.");
        exit(1);
    }

    path_finder::mount_data(&rse_path, namespace)?;

    Ok(())
}
