//! CLI argument parsing and environment bootstrapping.
//!
//! This module owns everything that touches the command line and the process
//! environment before any network calls are made:
//!
//! * [`Args`] — the `clap`-derived struct that models the accepted flags.
//! * [`check_privileges`] — verifies the process is running as root via `sudo`
//!   and that `SUDO_USER` is set, bailing out with a user-friendly re-invocation
//!   hint otherwise.
//! * [`get_tokens_from_env`] — reads pre-issued API tokens from environment
//!   variables, used when the caller wants to skip the OAuth2 device-code flow
//!   (`--no-login`).

use anyhow::{Context, Result};
use clap::Parser;
use std::env;

use crate::oauth2::Tokens;

/// Command-line arguments for pathFinder.
///
/// Parse these with [`clap::Parser::parse`]; the resulting struct is then
/// passed to [`check_privileges`] before any API work begins.
#[derive(Parser, Debug)]
#[command(name = "path-finder")]
#[command(about = "A tool for finding SKA data paths for mounting purposes")]
pub struct Args {
    /// Namespace of the data (e.g. `"ska:ska-sdp/eb-m001-20240101-00000"`).
    #[arg(long)]
    pub namespace: String,

    /// Name of the data file within the namespace.
    #[arg(long)]
    pub file_name: String,

    /// Skip the OAuth2 device-code flow and read tokens from
    /// `DATA_MANAGEMENT_ACCESS_TOKEN` and `SITE_CAPABILITIES_ACCESS_TOKEN`
    /// instead.
    #[arg(long)]
    pub no_login: bool,

    /// Unmount a previously mounted file instead of mounting it.
    #[arg(long)]
    pub unmount: bool,
}

/// Checks that the process is running as root via `sudo` and that `SUDO_USER`
/// is set.
///
/// Both conditions are necessary: the mount/unmount OS calls require root, and
/// `SUDO_USER` is used to build the bind-mount target path inside the invoking
/// user's home directory.  Running as the root user directly (without `sudo`)
/// is rejected so that the home-directory expansion is always safe.
///
/// Prints an actionable re-invocation hint to stderr before bailing.
pub fn check_privileges(args: &Args) -> Result<()> {
    #[cfg(unix)]
    {
        let euid = unsafe { libc::geteuid() };
        let sudo_user = env::var("SUDO_USER").ok();
        check_privileges_impl(euid, sudo_user.as_deref(), args)?;
    }

    #[cfg(not(unix))]
    {
        anyhow::bail!("This tool is only supported on Unix systems");
    }

    Ok(())
}

/// Inner implementation of [`check_privileges`] with injectable `euid` and
/// `sudo_user` values so the privilege logic can be unit-tested without
/// running the test suite as root.
///
/// * `euid` — effective user-ID of the current process (`0` = root).
/// * `sudo_user` — value of the `SUDO_USER` environment variable, if set.
/// * `args` — parsed CLI flags, used to tailor the re-invocation hint.
#[cfg(unix)]
fn check_privileges_impl(euid: u32, sudo_user: Option<&str>, args: &Args) -> Result<()> {
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

    if sudo_user.is_none() {
        eprintln!("\nWarning: SUDO_USER not set. Are you running as root directly?");
        eprintln!("Please use 'sudo' rather than running as root user.");
        anyhow::bail!("SUDO_USER environment variable not set");
    }

    Ok(())
}

/// Reads API access tokens from the `DATA_MANAGEMENT_ACCESS_TOKEN` and
/// `SITE_CAPABILITIES_ACCESS_TOKEN` environment variables.
///
/// This is the token source used with `--no-login`.  Both variables must be
/// present; a descriptive error is returned if either is absent so the user
/// knows exactly which one to export.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialise tests that mutate the process environment to avoid races when
    /// the test binary runs suites in parallel.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn mount_args() -> Args {
        Args {
            namespace: "ska:ska-sdp/eb-m001-20240101-00000".into(),
            file_name: "data.fits".into(),
            no_login: false,
            unmount: false,
        }
    }

    fn unmount_args() -> Args {
        Args {
            namespace: "ska:ska-sdp/eb-m001-20240101-00000".into(),
            file_name: "data.fits".into(),
            no_login: false,
            unmount: true,
        }
    }

    // ── check_privileges_impl ────────────────────────────────────────────────

    #[test]
    #[cfg(unix)]
    fn check_privileges_fails_when_not_root() {
        let err = check_privileges_impl(1000, Some("alice"), &mount_args()).unwrap_err();
        assert!(
            err.to_string().contains("sudo required"),
            "unexpected error: {err}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn check_privileges_fails_when_not_root_and_unmounting() {
        // The bail message is identical; this path exercises the branch that
        // includes `--unmount` in the eprintln! hint.
        let err = check_privileges_impl(1000, Some("alice"), &unmount_args()).unwrap_err();
        assert!(
            err.to_string().contains("sudo required"),
            "unexpected error: {err}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn check_privileges_fails_when_sudo_user_absent() {
        let err = check_privileges_impl(0, None, &mount_args()).unwrap_err();
        assert!(
            err.to_string().contains("SUDO_USER"),
            "unexpected error: {err}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn check_privileges_succeeds_when_root_with_sudo_user() {
        check_privileges_impl(0, Some("alice"), &mount_args())
            .expect("should succeed when euid == 0 and SUDO_USER is set");
    }

    // ── get_tokens_from_env ──────────────────────────────────────────────────

    #[test]
    fn get_tokens_from_env_returns_tokens_when_both_set() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::set_var("DATA_MANAGEMENT_ACCESS_TOKEN", "dm-test-token");
        env::set_var("SITE_CAPABILITIES_ACCESS_TOKEN", "sc-test-token");

        let result = get_tokens_from_env();

        env::remove_var("DATA_MANAGEMENT_ACCESS_TOKEN");
        env::remove_var("SITE_CAPABILITIES_ACCESS_TOKEN");

        let tokens = result.expect("should succeed when both vars are set");
        assert_eq!(tokens.data_management_token, "dm-test-token");
        assert_eq!(tokens.site_capabilities_token, "sc-test-token");
    }

    #[test]
    fn get_tokens_from_env_errors_when_dm_token_absent() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::remove_var("DATA_MANAGEMENT_ACCESS_TOKEN");
        env::set_var("SITE_CAPABILITIES_ACCESS_TOKEN", "sc-test-token");

        let result = get_tokens_from_env();

        env::remove_var("SITE_CAPABILITIES_ACCESS_TOKEN");

        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("DATA_MANAGEMENT_ACCESS_TOKEN"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn get_tokens_from_env_errors_when_sc_token_absent() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::set_var("DATA_MANAGEMENT_ACCESS_TOKEN", "dm-test-token");
        env::remove_var("SITE_CAPABILITIES_ACCESS_TOKEN");

        let result = get_tokens_from_env();

        env::remove_var("DATA_MANAGEMENT_ACCESS_TOKEN");

        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("SITE_CAPABILITIES_ACCESS_TOKEN"),
            "unexpected error: {err}"
        );
    }
}
