mod api_client;
mod models;
mod mount;
mod oauth2;

use anyhow::{Context, Result};
use clap::Parser;
use regex::Regex;
use std::collections::HashSet;
use std::env;
use std::process::exit;

use api_client::{ApiClient, PathFinderApiClient};
use models::{DataLocation, StorageAreaIDToNodeAndSite};
use oauth2::{authenticate, Tokens};

#[derive(Parser, Debug)]
#[command(name = "path-finder")]
#[command(about = "A tool for finding SKA data paths for mounting purposes")]
struct Args {
    /// Namespace of the data
    #[arg(long)]
    namespace: String,

    /// Name of the data file
    #[arg(long)]
    file_name: String,

    /// Do not use OAuth2 for authentication - use environment variables instead
    #[arg(long)]
    no_login: bool,

    /// Unmount previously mounted data instead of mounting
    #[arg(long)]
    unmount: bool,
}

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

fn check_privileges(args: &Args) -> Result<()> {
    // Check for root privileges early to avoid wasting time on API calls
    #[cfg(unix)]
    {
        let euid = unsafe { libc::geteuid() };
        if euid != 0 {
            eprintln!("\nError: This tool requires root privileges for mount/unmount operations.");
            eprintln!("Please re-run with sudo:");
            if args.unmount {
                eprintln!(
                    "  sudo -E path-finder --namespace {} --file_name {} --unmount",
                    args.namespace, args.file_name
                );
            } else {
                eprintln!(
                    "  sudo -E path-finder --namespace {} --file_name {}",
                    args.namespace, args.file_name
                );
            }
            anyhow::bail!("Insufficient privileges - sudo required");
        }

        // Verify SUDO_USER is set
        if env::var("SUDO_USER").is_err() {
            eprintln!("\nWarning: SUDO_USER not set. Are you running as root directly?");
            eprintln!("Please use 'sudo' rather than running as root user.");
            anyhow::bail!("SUDO_USER environment variable not set");
        }
    }

    #[cfg(not(unix))]
    {
        anyhow::bail!("This tool is only supported on Unix systems");
    }

    Ok(())
}

fn get_tokens_from_env() -> Result<Tokens> {
    let dm_token = env::var("DATA_MANAGEMENT_ACCESS_TOKEN")
        .context("Please set DATA_MANAGEMENT_ACCESS_TOKEN environment variable or omit --no-login to use OAuth2")?;

    let sc_token = env::var("SITE_CAPABILITIES_ACCESS_TOKEN")
        .context("Please set SITE_CAPABILITIES_ACCESS_TOKEN environment variable or omit --no-login to use OAuth2")?;

    Ok(Tokens {
        data_management_token: dm_token,
        site_capabilities_token: sc_token,
    })
}

fn run(namespace: &str, file_name: &str, tokens: &Tokens) -> Result<()> {
    let client = ApiClient::new(
        tokens.data_management_token.clone(),
        tokens.site_capabilities_token.clone(),
    );

    client.check_namespace_available(namespace)?;

    let site_storages = client.site_storage_areas()?;
    let data_locations = client.locate_data(namespace, file_name)?;

    print_data_locations_with_sites(&site_storages, &data_locations);

    let rse_path = extract_rse_path(&data_locations, namespace, file_name)?;
    println!(
        "RSE Path for file '{}' in namespace '{}': {}",
        file_name, namespace, rse_path
    );

    // Check if the file exists locally
    if !check_local_file_exists(&rse_path) {
        println!("\n⚠️  File not found locally! ⚠️");
        println!("\nThe file is available at the following locations:");
        print_data_locations_with_sites(&site_storages, &data_locations);
        println!("\nPlease ensure the data has been staged to this local site before mounting.");
        exit(1);
    }

    mount_data(&rse_path, namespace)?;

    Ok(())
}

fn print_data_locations_with_sites(
    site_stores: &StorageAreaIDToNodeAndSite,
    data_locations: &[DataLocation],
) {
    for location in data_locations {
        if let Some((node_name, site_name, area_name)) =
            site_stores.get(&location.associated_storage_area_id)
        {
            println!(
                "Data location ID: {}, Storage Area: {} ({}), Node: {}, Site: {}",
                location.identifier,
                area_name,
                location.associated_storage_area_id,
                node_name,
                site_name
            );
        } else {
            println!(
                "Data location ID: {}, Storage Area ID: {}, Node/Site: Not found",
                location.identifier, location.associated_storage_area_id
            );
        }
    }
}

fn check_local_file_exists(rse_path: &str) -> bool {
    use std::path::Path;
    let local_path = format!("/skadata{}", rse_path);
    Path::new(&local_path).exists()
}

fn extract_rse_path(
    data_locations: &[DataLocation],
    namespace: &str,
    file_name: &str,
) -> Result<String> {
    let pattern = format!(r"/{}/.*$", regex::escape(namespace));
    let rse_path_regex = Regex::new(&pattern)?;

    let mut matched_paths = HashSet::new();
    let mut unmatched_paths = Vec::new();

    for location in data_locations {
        for uri in &location.replicas {
            if let Some(captures) = rse_path_regex.find(uri) {
                matched_paths.insert(captures.as_str().to_string());
            } else {
                unmatched_paths.push(uri.clone());
            }
        }
    }

    if !unmatched_paths.is_empty() {
        println!(
            "Warning: {} URIs did not match the expected pattern.",
            unmatched_paths.len()
        );
        println!("Unmatched URIs: {:?}", unmatched_paths);
    }

    if matched_paths.is_empty() {
        anyhow::bail!(
            "No valid paths found for file '{}' in namespace '{}'.",
            file_name,
            namespace
        );
    }

    if matched_paths.len() > 1 {
        println!("Warning: Multiple unique paths found: {:?}", matched_paths);
        println!("We should check the path for the local RSE - by cross-referencing with site capabilities.");
        anyhow::bail!("Handling multiple matched paths is not implemented.");
    }

    Ok(matched_paths.into_iter().next().unwrap())
}

fn mount_data(rse_path: &str, namespace: &str) -> Result<()> {
    println!(
        "Mounting data from RSE path: {} in namespace: {}",
        rse_path, namespace
    );

    // Get the original user (already verified in check_privileges())
    let sudo_user = env::var("SUDO_USER").context("SUDO_USER not set")?;

    mount::mount_operation(rse_path, namespace, &sudo_user)?;
    println!(
        "Successfully mounted {} in namespace {}",
        rse_path, namespace
    );

    Ok(())
}
