//! CLI argument parsing and environment bootstrapping.
//!
//! This module owns everything that touches the command line and the process
//! environment before any network calls are made:
//!
//! * [`Args`] — the `clap`-derived struct that models the accepted flags.
//! * [`check_privileges`] — verifies the process is running as root via `sudo`
//!   and that `SUDO_USER` is set, bailing out with a user-friendly re-invocation
//!   hint otherwise.
//! * [`resolve_auth_token`] — determines the bearer token to use, either from
//!   `--token`, the `PATHFINDER_SKA_AUTH_TOKEN` environment variable, or an
//!   error if neither is set.

use anyhow::Result;
use clap::Parser;
use std::env;

/// ** pathFinder **
///
/// A CLI tool for mounting SKA data.
#[derive(Parser, Debug)]
#[command(name = "pathFinder")]
#[command(about = "A CLI tool for mounting SKA data.")]
pub struct Args {
    /// Namespace of the data (e.g. "teal").
    #[arg(long)]
    pub namespace: String,

    /// Name of the data file within the namespace.
    #[arg(long)]
    pub file_name: String,

    /// Raw JWT token used to authenticate the caller. The token is validated
    /// against the OIDC JWKS and then exchanged for the Data Management and
    /// Site Capabilities API tokens.
    ///
    /// If omitted, the interactive OAuth2 device-code flow is used. If
    /// provided without a value, the `PATHFINDER_SKA_AUTH_TOKEN` environment
    /// variable is used.
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    pub token: Option<String>,

    /// Unmount a file instead of mounting it.
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

/// Environment variable used as the fallback bearer token source when
/// `--token` is not supplied with a value.
pub const AUTH_TOKEN_ENV_VAR: &str = "PATHFINDER_SKA_AUTH_TOKEN";

/// Resolves the bearer token to use for authentication.
///
/// * `--token <TOKEN>` → returns `Ok(Some(TOKEN))`.
/// * `--token` with no value → looks at the `PATHFINDER_SKA_AUTH_TOKEN`
///   environment variable. If it is set and non-empty, returns
///   `Ok(Some(token))`; otherwise returns an error explaining how to supply
///   the token.
/// * No `--token` flag at all → returns `Ok(None)`, signalling the caller to
///   use the interactive OAuth2 device-code flow.
pub fn resolve_auth_token(args: &Args) -> Result<Option<String>> {
    match args.token.as_deref() {
        Some(token) if !token.is_empty() => Ok(Some(token.to_string())),
        Some(_) => match env::var(AUTH_TOKEN_ENV_VAR) {
            Ok(token) if !token.is_empty() => Ok(Some(token)),
            _ => {
                eprintln!("\nError: --token was provided without a value, but PATHFINDER_SKA_AUTH_TOKEN is not set.");
                eprintln!("Provide a token on the command line with:");
                eprintln!("  --token <TOKEN>");
                eprintln!("Or set the PATHFINDER_SKA_AUTH_TOKEN environment variable.");
                anyhow::bail!("missing authentication token")
            }
        },
        None => Ok(None),
    }
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
            token: None,
            unmount: false,
        }
    }

    fn unmount_args() -> Args {
        Args {
            namespace: "ska:ska-sdp/eb-m001-20240101-00000".into(),
            file_name: "data.fits".into(),
            token: None,
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

    // ── resolve_auth_token ───────────────────────────────────────────────────

    #[test]
    fn resolve_auth_token_prefers_command_line_value() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::set_var(AUTH_TOKEN_ENV_VAR, "env-token");

        let mut args = mount_args();
        args.token = Some("cli-token".to_string());

        let token = resolve_auth_token(&args).unwrap();
        assert_eq!(token, Some("cli-token".to_string()));

        env::remove_var(AUTH_TOKEN_ENV_VAR);
    }

    #[test]
    fn resolve_auth_token_falls_back_to_environment_variable() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::set_var(AUTH_TOKEN_ENV_VAR, "env-token");

        let token = resolve_auth_token(&mount_args()).unwrap();
        assert_eq!(token, Some("env-token".to_string()));

        env::remove_var(AUTH_TOKEN_ENV_VAR);
    }

    #[test]
    fn resolve_auth_token_returns_none_when_missing() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::remove_var(AUTH_TOKEN_ENV_VAR);

        let token = resolve_auth_token(&mount_args()).unwrap();
        assert_eq!(token, None);
    }

    #[test]
    fn resolve_auth_token_ignores_empty_command_line_token() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::set_var(AUTH_TOKEN_ENV_VAR, "env-token");

        let mut args = mount_args();
        args.token = Some("".to_string());

        let token = resolve_auth_token(&args).unwrap();
        assert_eq!(token, Some("env-token".to_string()));

        env::remove_var(AUTH_TOKEN_ENV_VAR);
    }
}
