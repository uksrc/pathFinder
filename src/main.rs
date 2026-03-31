mod api_client;
mod cli;
mod models;
mod mount;
mod oauth2;
mod path_finder;

use anyhow::{Context, Result};
use clap::Parser;
use std::env;

use api_client::{ApiClient, PathFinderApiClient};
use cli::{check_privileges, get_tokens_from_env, Args};
use models::{DataLocation, StorageAreaIDToNodeAndSite};
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

/// Production wrapper: constructs an [`ApiClient`] from the supplied tokens and
/// delegates to [`run_impl`] with the real path-finder helpers and [`do_exit`].
fn run(namespace: &str, file_name: &str, tokens: &Tokens) -> Result<()> {
    let client = ApiClient::new(
        tokens.data_management_token.clone(),
        tokens.site_capabilities_token.clone(),
    );
    run_impl(
        namespace,
        file_name,
        &client,
        path_finder::print_data_locations_with_sites,
        path_finder::extract_rse_path,
        path_finder::check_local_file_exists,
        path_finder::mount_data,
        do_exit,
    )
}

/// Wraps [`std::process::exit`] so that [`run_impl`] can accept an injectable
/// `Fn(i32)` rather than calling `process::exit` directly, keeping the
/// orchestration logic unit-testable without spawning a subprocess.
fn do_exit(code: i32) {
    std::process::exit(code);
}

/// Core code for the mount workflow.
///
/// All external dependencies are injected so the function can be exercised in
/// unit tests without live API endpoints, a real `/skadata` tree, or root
/// privileges.
///
/// # Parameters
/// * `namespace`       — data namespace passed on the command line.
/// * `file_name`       — file name passed on the command line.
/// * `client`          — SRCNet API client; see [`PathFinderApiClient`].
/// * `print_locations` — displays the replica list enriched with site names.
///                       Called once on the happy path and a second time when
///                       the file has not yet been staged locally.
/// * `extract_path`    — extracts the `/<namespace>/…` RSE path from replica URIs.
/// * `file_exists`     — returns `true` when the file is present under `/skadata`.
/// * `mount`           — performs the OS-level bind mount.
/// * `exit_fn`         — called with `1` when the file is not locally staged.
///                       In production this is [`do_exit`], which does not return.
fn run_impl(
    namespace: &str,
    file_name: &str,
    client: &dyn PathFinderApiClient,
    print_locations: impl Fn(&StorageAreaIDToNodeAndSite, &[DataLocation]),
    extract_path: impl Fn(&[DataLocation], &str, &str) -> Result<String>,
    file_exists: impl Fn(&str) -> bool,
    mount: impl Fn(&str, &str) -> Result<()>,
    exit_fn: impl Fn(i32),
) -> Result<()> {
    client.check_namespace_available(namespace)?;

    let data_locations = client.locate_data(namespace, file_name)?;
    let rse_path = extract_path(&data_locations, namespace, file_name)?;

    println!(
        "RSE Path for file '{}' in namespace '{}': {}",
        file_name, namespace, rse_path
    );

    if !file_exists(&rse_path) {
        println!("\n⚠️  File not found locally! ⚠️");
        println!("Checking available storage areas at this site...");
        let site_storages = client.site_storage_areas()?;
        println!("\nThe file is available at the following locations:");
        print_locations(&site_storages, &data_locations);
        println!("\nPlease ensure the data has been staged to this local site before mounting.");
        exit_fn(1);
        return Ok(()); // unreachable in production (used for testing when exist_fn is mocked)
    }

    mount(&rse_path, namespace)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use models::DataLocationAPIResponse;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    // ── constants ────────────────────────────────────────────────────────────

    const NS: &str = "ska:ska-sdp/eb-m001-20240101-00000";
    const FILE: &str = "data.fits";
    const RSE_PATH: &str = "/ska:ska-sdp/eb-m001-20240101-00000/data.fits";
    const OLYMPUSMONS_AREA_ID: &str = "2a73d212-8793-4011-a687-cad99841c269";

    // ── helpers ──────────────────────────────────────────────────────────────

    fn make_location() -> DataLocation {
        DataLocation {
            identifier: "MARSSRC-OLYMPUSMONS-T0".into(),
            associated_storage_area_id: OLYMPUSMONS_AREA_ID.into(),
            replicas: vec![format!(
                "davs://xrootd01.olympusmons.marssrc.org:1094/skadata{RSE_PATH}"
            )],
            is_dataset: false,
        }
    }

    fn make_site_storages() -> StorageAreaIDToNodeAndSite {
        let mut m = HashMap::new();
        m.insert(
            OLYMPUSMONS_AREA_ID.to_string(),
            (
                "MARSSRC".to_string(),
                "MARSSRC-OLYMPUSMONS".to_string(),
                "MARSSRC_OLYMPUSMONS_XRD".to_string(),
            ),
        );
        m
    }

    // ── MockApiClient ────────────────────────────────────────────────────────

    struct MockApiClient {
        namespace_ok: bool,
        site_storages_ok: bool,
        locate_data_ok: bool,
        // call recording
        check_namespace_called_with: RefCell<Option<String>>,
        site_storages_called: Cell<bool>,
        locate_data_called_with: RefCell<Option<(String, String)>>,
    }

    impl MockApiClient {
        fn new_golden() -> Self {
            Self {
                namespace_ok: true,
                site_storages_ok: true,
                locate_data_ok: true,
                check_namespace_called_with: RefCell::new(None),
                site_storages_called: Cell::new(false),
                locate_data_called_with: RefCell::new(None),
            }
        }
    }

    impl PathFinderApiClient for MockApiClient {
        fn check_namespace_available(&self, namespace: &str) -> Result<()> {
            *self.check_namespace_called_with.borrow_mut() = Some(namespace.to_string());
            if self.namespace_ok {
                Ok(())
            } else {
                anyhow::bail!("namespace '{}' not available", namespace)
            }
        }

        fn get_all_namespaces(&self) -> Result<Vec<String>> {
            Ok(vec![NS.to_string()])
        }

        fn site_storage_areas(&self) -> Result<StorageAreaIDToNodeAndSite> {
            self.site_storages_called.set(true);
            if self.site_storages_ok {
                Ok(make_site_storages())
            } else {
                anyhow::bail!("site_storage_areas failed")
            }
        }

        fn locate_data(&self, namespace: &str, file_name: &str) -> Result<DataLocationAPIResponse> {
            *self.locate_data_called_with.borrow_mut() =
                Some((namespace.to_string(), file_name.to_string()));
            if self.locate_data_ok {
                Ok(vec![make_location()])
            } else {
                anyhow::bail!("locate_data API error")
            }
        }
    }

    // ── run_impl tests ───────────────────────────────────────────────────────

    #[test]
    fn run_impl_golden_path_calls_all_with_correct_args() {
        let client = MockApiClient::new_golden();
        let print_count = Cell::new(0u32);
        let extract_called_with: RefCell<Option<(String, String)>> = RefCell::new(None);
        let file_exists_called_with: RefCell<Option<String>> = RefCell::new(None);
        let mount_called_with: RefCell<Option<(String, String)>> = RefCell::new(None);
        let exit_called = Cell::new(false);

        run_impl(
            NS,
            FILE,
            &client,
            |_, _| {
                print_count.set(print_count.get() + 1);
            },
            |_locs, ns, file| {
                *extract_called_with.borrow_mut() = Some((ns.to_string(), file.to_string()));
                Ok(RSE_PATH.to_string())
            },
            |rse| {
                *file_exists_called_with.borrow_mut() = Some(rse.to_string());
                true
            },
            |rse, ns| {
                *mount_called_with.borrow_mut() = Some((rse.to_string(), ns.to_string()));
                Ok(())
            },
            |_| exit_called.set(true),
        )
        .unwrap();

        // API client called with the right args
        assert_eq!(
            client.check_namespace_called_with.borrow().as_deref(),
            Some(NS),
            "check_namespace_available should be called with the provided namespace"
        );
        assert!(
            !client.site_storages_called.get(),
            "site_storage_areas function should not be called in golden path"
        );
        assert_eq!(
            *client.locate_data_called_with.borrow(),
            Some((NS.to_string(), FILE.to_string())),
            "locate_data should be called with the provided namespace and file"
        );

        // path-finder helpers called with the right args
        assert_eq!(print_count.get(), 0, "print_locations not called");
        assert_eq!(
            *extract_called_with.borrow(),
            Some((NS.to_string(), FILE.to_string())),
            "extract_path should be called with the provided namespace and file"
        );
        assert_eq!(
            file_exists_called_with.borrow().as_deref(),
            Some(RSE_PATH),
            "file_exists should be called with the extracted RSE path"
        );
        assert_eq!(
            *mount_called_with.borrow(),
            Some((RSE_PATH.to_string(), NS.to_string())),
            "mount should be called with the extracted RSE path and namespace"
        );
        assert!(
            !exit_called.get(),
            "exit_fn must not be called on golden path"
        );
    }

    #[test]
    fn run_impl_calls_exit_and_skips_mount_when_file_not_staged() {
        let client = MockApiClient::new_golden();
        let exit_called = Cell::new(false);
        let mount_called = Cell::new(false);

        run_impl(
            NS,
            FILE,
            &client,
            |_, _| {},
            |_, _, _| Ok(RSE_PATH.to_string()),
            |_| false, // file not present locally
            |_, _| {
                mount_called.set(true);
                Ok(())
            },
            |_| exit_called.set(true),
        )
        .unwrap();

        assert!(exit_called.get(), "exit_fn should be called");
        assert!(
            !mount_called.get(),
            "mount must not be called when file not staged"
        );
    }

    #[test]
    fn run_impl_prints_locations_when_file_not_staged() {
        let client = MockApiClient::new_golden();
        let print_count = Cell::new(0u32);

        run_impl(
            NS,
            FILE,
            &client,
            |_, _| print_count.set(print_count.get() + 1),
            |_, _, _| Ok(RSE_PATH.to_string()),
            |_| false,
            |_, _| Ok(()),
            |_| {},
        )
        .unwrap();

        assert_eq!(
            print_count.get(),
            1,
            "print_locations should be called when file not staged"
        );
    }

    #[test]
    fn run_impl_propagates_namespace_not_available_error() {
        let client = MockApiClient {
            namespace_ok: false,
            ..MockApiClient::new_golden()
        };

        let err = run_impl(
            NS,
            FILE,
            &client,
            |_, _| {},
            |_, _, _| unreachable!("extract_path must not be called"),
            |_| unreachable!("file_exists must not be called"),
            |_, _| unreachable!("mount must not be called"),
            |_| unreachable!("exit_fn must not be called"),
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("not available"),
            "unexpected error: {err}"
        );
        assert_eq!(
            client.check_namespace_called_with.borrow().as_deref(),
            Some(NS)
        );
    }

    #[test]
    fn run_impl_propagates_locate_data_error() {
        let client = MockApiClient {
            locate_data_ok: false,
            ..MockApiClient::new_golden()
        };

        let err = run_impl(
            NS,
            FILE,
            &client,
            |_, _| {},
            |_, _, _| unreachable!("extract_path must not be called"),
            |_| unreachable!("file_exists must not be called"),
            |_, _| unreachable!("mount must not be called"),
            |_| unreachable!("exit_fn must not be called"),
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("locate_data API error"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn run_impl_propagates_extract_path_error() {
        let client = MockApiClient::new_golden();

        let err = run_impl(
            NS,
            FILE,
            &client,
            |_, _| {},
            |_, _, _| anyhow::bail!("no matching replica paths"),
            |_| unreachable!("file_exists must not be called"),
            |_, _| unreachable!("mount must not be called"),
            |_| unreachable!("exit_fn must not be called"),
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("no matching replica paths"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn run_impl_propagates_mount_error() {
        let client = MockApiClient::new_golden();

        let err = run_impl(
            NS,
            FILE,
            &client,
            |_, _| {},
            |_, _, _| Ok(RSE_PATH.to_string()),
            |_| true, // file exists
            |_, _| anyhow::bail!("bindfs: permission denied"),
            |_| unreachable!("exit_fn must not be called"),
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("bindfs: permission denied"),
            "unexpected error: {err}"
        );
    }
}
