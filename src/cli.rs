//! CLI argument parsing and environment bootstrapping.

use anyhow::{Context, Result};
use clap::Parser;
use std::env;

use crate::oauth2::Tokens;

/// Command-line arguments for pathFinder.
#[derive(Parser, Debug)]
#[command(name = "path-finder")]
#[command(about = "A tool for finding SKA data paths for mounting purposes")]
pub struct Args {
    /// Namespace of the data
    #[arg(long)]
    pub namespace: String,

    /// Name of the data file
    #[arg(long)]
    pub file_name: String,

    /// Do not use OAuth2 for authentication - use environment variables instead
    #[arg(long)]
    pub no_login: bool,

    /// Unmount previously mounted data instead of mounting
    #[arg(long)]
    pub unmount: bool,
}

/// Checks that the process is running as root via `sudo` and that `SUDO_USER` is set.
///
/// Exits early with a helpful message if not running with sufficient privileges.
pub fn check_privileges(args: &Args) -> Result<()> {
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

/// Reads API access tokens from the `DATA_MANAGEMENT_ACCESS_TOKEN` and
/// `SITE_CAPABILITIES_ACCESS_TOKEN` environment variables.
pub fn get_tokens_from_env() -> Result<Tokens> {
    let dm_token = env::var("DATA_MANAGEMENT_ACCESS_TOKEN")
        .context("Please set DATA_MANAGEMENT_ACCESS_TOKEN environment variable or omit --no-login to use OAuth2")?;

    let sc_token = env::var("SITE_CAPABILITIES_ACCESS_TOKEN")
        .context("Please set SITE_CAPABILITIES_ACCESS_TOKEN environment variable or omit --no-login to use OAuth2")?;

    Ok(Tokens {
        data_management_token: dm_token,
        site_capabilities_token: sc_token,
    })
}
