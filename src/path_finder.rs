//! Core path-finding logic: locating replica paths, checking local availability, and mounting.
//!
//! This module bridges the API layer ([`crate::api_client`]) and the OS layer
//! ([`crate::mount`]).  The four public functions implement the logical
//! steps in the mount workflow:
//!

use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashSet;
use std::env;
use std::path::Path;

use crate::api_client::{ApiClient, PathFinderApiClient};
use crate::models::{DataLocation, StorageAreaIDToNodeAndSite};
use crate::oauth2::Tokens;

/// Production wrapper: constructs an [`ApiClient`] from the supplied tokens and
/// delegates to [`run_impl`] with the real path-finder helpers and [`do_exit`].
pub fn run(namespace: &str, file_name: &str, tokens: &Tokens) -> Result<()> {
    let client = ApiClient::new(
        tokens.data_management_token.clone(),
        tokens.site_capabilities_token.clone(),
    );
    run_impl(
        namespace,
        file_name,
        &client,
        print_data_locations_with_sites,
        extract_rse_path,
        check_local_file_exists,
        mount_data,
        do_exit,
    )
}

/// Wraps [`std::process::exit`] so that [`run_impl`] can accept an injectable
/// `Fn(i32)` rather than calling `process::exit` directly, keeping the
/// orchestration logic unit-testable without spawning a subprocess.
fn do_exit(code: i32) {
    std::process::exit(code);
}

/// Production wrapper: constructs an [`ApiClient`] from the supplied tokens and
/// delegates to [`run_impl`] with the real path-finder helpers and [`do_exit`].
pub fn run_spawn(namespace: &str, file_name: &str, tokens: Tokens) -> Result<()> {
    let client = ApiClient::new(
        tokens.data_management_token.clone(),
        tokens.site_capabilities_token.clone(),
    );
    run_impl(
        namespace,
        file_name,
        &client,
        print_data_locations_with_sites,
        extract_rse_path,
        check_local_file_exists,
        mount_data,
        |code| {
            eprintln!("run exited with code {}", code);
        },
    )
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

/// Prints each data location enriched with its node, site, and storage area name.
///
/// `site_stores` is the [`StorageAreaIDToNodeAndSite`] map produced by
/// [`crate::api_client::PathFinderApiClient::site_storage_areas`]; it maps
/// storage-area UUIDs to `(node_name, site_name, area_name)` tuples.
///
/// When a location's `associated_storage_area_id` is not found in the map the
/// function falls back to printing the raw UUID so the caller still has
/// something actionable.
pub fn print_data_locations_with_sites(
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

/// Returns `true` if `rse_path` exists under the local `/skadata` mount point.
pub fn check_local_file_exists(rse_path: &str) -> bool {
    check_local_file_exists_impl(rse_path, "/skadata")
}

/// Inner implementation of [`check_local_file_exists`] with an injectable
/// `base` directory, allowing unit tests to probe a test directory instead
/// of `/skadata`.
fn check_local_file_exists_impl(rse_path: &str, base: &str) -> bool {
    let local_path = format!("{}{}", base, rse_path);
    Path::new(&local_path).exists()
}

/// Extracts the canonical RSE path from the replica URIs in `data_locations`.
///
/// Each replica URI (e.g.
/// `"davs://xrootd01.olympusmons.marssrc.org:1094/skadata/ska:ska-sdp/eb-m001/data.fits"`)
/// is searched for a `/<namespace>/…` suffix.  The suffix becomes the RSE
/// path that is later passed to [`mount_data`].
///
/// **Error conditions:**
/// - No URIs match the pattern
/// - Two or more *distinct* paths are found across all replicas
///
/// Duplicate URIs pointing to the same path (i.e. the same file staged to
/// multiple replicas of the same RSE) are de-duplicated silently; only
/// *distinct* paths trigger the multiple-paths error.
///
/// A warning is printed for each URI that does not match, but this is not
/// treated as fatal
pub fn extract_rse_path(
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
        println!(
            "We should check the path for the local RSE - by cross-referencing with site capabilities."
        );
        anyhow::bail!("Handling multiple matched paths is not implemented.");
    }

    Ok(matched_paths.into_iter().next().unwrap())
}

/// Mounts the data file at `rse_path` into the invoking user's home directory.
///
/// Reads `SUDO_USER` from the environment (set by `sudo`; guaranteed to be
/// present after [`crate::cli::check_privileges`] succeeds) and delegates to
/// [`crate::mount::mount_operation`].
///
/// Prints progress messages to stdout before and after the mount syscall.
pub fn mount_data(rse_path: &str, namespace: &str) -> Result<()> {
    let sudo_user = env::var("SUDO_USER").context("SUDO_USER not set")?;
    mount_data_impl(
        rse_path,
        namespace,
        &sudo_user,
        crate::mount::mount_operation,
    )
}

/// Inner implementation of [`mount_data`] with an injectable `mount_fn` and
/// `sudo_user`, so the code can be tested without performing a
/// real OS mount.
///
/// * `rse_path`  — the `/<namespace>/…` path on the RSE.
/// * `namespace` — the data namespace (used for bind-mount target naming).
/// * `sudo_user` — the original (non-root) user on whose behalf to mount.
/// * `mount_fn`  — called as `mount_fn(rse_path, namespace, sudo_user)`; in
///   production this is [`crate::mount::mount_operation`].
fn mount_data_impl(
    rse_path: &str,
    namespace: &str,
    sudo_user: &str,
    mount_fn: impl Fn(&str, &str, &str) -> Result<()>,
) -> Result<()> {
    mount_fn(rse_path, namespace, sudo_user)
}
#[cfg(test)]
mod tests {

    use super::*;
    use crate::models::DataLocationAPIResponse;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // ── constants ────────────────────────────────────────────────────────────

    const NS: &str = "ska:ska-sdp/eb-m001-20240101-00000";
    const FILE: &str = "data.fits";
    const RSE_PATH: &str = "/ska:ska-sdp/eb-m001-20240101-00000/data.fits";
    const OLYMPUSMONS_AREA_ID: &str = "12345678-90ab-cdef-1234-567890abcdef";
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // ── helpers ──────────────────────────────────────────────────────────────

    fn make_default_location() -> DataLocation {
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

    fn make_location(id: &str, area_id: &str, replicas: &[&str]) -> DataLocation {
        DataLocation {
            identifier: id.to_string(),
            associated_storage_area_id: area_id.to_string(),
            replicas: replicas.iter().map(|s| s.to_string()).collect(),
            is_dataset: false,
        }
    }

    fn make_site_stores() -> StorageAreaIDToNodeAndSite {
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
                Ok(vec![make_location(
                    "MARSSRC-OLYMPUSMONS-T0",
                    OLYMPUSMONS_AREA_ID,
                    &["davs://xrootd01.olympusmons.marssrc.org:1094/skadata/ska:ska-sdp/eb-m001-20240101-00000/data.fits"],
                )])
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
    // ── print_data_locations_with_sites ──────────────────────────────────────

    #[test]
    fn print_locations_does_not_panic() {
        let stores = make_site_stores();
        let locations = vec![make_location(
            "MARSSRC-OLYMPUSMONS-T0",
            OLYMPUSMONS_AREA_ID,
            &["davs://xrootd01.olympusmons.marssrc.org:1094/skadata/ska:ns/data.fits"],
        )];
        // We exercise the enriched branch; the test passes if no panic occurs.
        print_data_locations_with_sites(&stores, &locations);
    }

    #[test]
    fn print_locations_does_not_panic_with_empty_slice() {
        let stores = make_site_stores();
        print_data_locations_with_sites(&stores, &[]);
    }

    // ── check_local_file_exists_impl ─────────────────────────────────────────

    #[test]
    fn check_local_file_exists_returns_true_when_file_present() {
        let test_dir = TempDir::new().unwrap();
        let base_path = test_dir.path().to_str().unwrap();
        // Create  <tmp>/skadata/ska:ns/data.fits by writing a file at the path.
        let rse_path = "/ska:ns/data.fits";
        let full = test_dir.path().join("ska:ns").join("data.fits");
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, b"").unwrap();

        assert!(check_local_file_exists_impl(rse_path, base_path));
    }

    #[test]
    fn check_local_file_exists_returns_false_when_file_absent() {
        let test_dir = TempDir::new().unwrap();
        let base_path = test_dir.path().to_str().unwrap();
        assert!(!check_local_file_exists_impl(
            "/ska:ns/missing.fits",
            base_path
        ));
    }

    // ── extract_rse_path ─────────────────────────────────────────────────────

    #[test]
    fn extract_rse_path_returns_path_for_single_match() {
        let ns = "ska:ska-sdp/eb-m001-20240101-00000";
        let locations = vec![make_location(
            "MARSSRC-OLYMPUSMONS-T0",
            OLYMPUSMONS_AREA_ID,
            &[&format!(
                "davs://xrootd01.olympusmons.marssrc.org:1094/skadata/{ns}/data.fits"
            )],
        )];

        let result = extract_rse_path(&locations, ns, "data.fits").unwrap();
        assert_eq!(result, format!("/{ns}/data.fits"));
    }

    #[test]
    fn extract_rse_path_deduplicates_identical_paths_across_replicas() {
        let ns = "ska:ska-sdp/eb-m001-20240101-00000";
        let uri = format!("davs://xrootd01.olympusmons.marssrc.org:1094/skadata/{ns}/data.fits");
        // Same logical path served from two replica URIs → should succeed.
        let locations = vec![
            make_location(
                "MARSSRC-OLYMPUSMONS-T0",
                OLYMPUSMONS_AREA_ID,
                &[uri.as_str()],
            ),
            make_location(
                "MARSSRC-OLYMPUSMONS-T1",
                OLYMPUSMONS_AREA_ID,
                &[uri.as_str()],
            ),
        ];

        let result = extract_rse_path(&locations, ns, "data.fits").unwrap();
        assert_eq!(result, format!("/{ns}/data.fits"));
    }

    #[test]
    fn extract_rse_path_errors_when_no_locations() {
        let err = extract_rse_path(&[], "ska:ns", "data.fits").unwrap_err();
        assert!(
            err.to_string().contains("No valid paths found"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extract_rse_path_errors_when_no_replicas_match() {
        let locations = vec![make_location(
            "MARSSRC-OLYMPUSMONS-T0",
            OLYMPUSMONS_AREA_ID,
            &["davs://xrootd01.olympusmons.marssrc.org:1094/unrelated/path/data.fits"],
        )];
        let err = extract_rse_path(&locations, "ska:ns", "data.fits").unwrap_err();
        assert!(
            err.to_string().contains("No valid paths found"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extract_rse_path_errors_when_multiple_distinct_paths() {
        let ns = "ska:ns";
        let locations = vec![
            make_location(
                "MARSSRC-OLYMPUSMONS-T0",
                OLYMPUSMONS_AREA_ID,
                &[&format!(
                    "davs://xrootd01.olympusmons.marssrc.org:1094/skadata/{ns}/v1/data.fits"
                )],
            ),
            make_location(
                "MARSSRC-OLYMPUSMONS-T1",
                OLYMPUSMONS_AREA_ID,
                &[&format!(
                    "davs://xrootd01.olympusmons.marssrc.org:1094/skadata/{ns}/v2/data.fits"
                )],
            ),
        ];

        let err = extract_rse_path(&locations, ns, "data.fits").unwrap_err();
        assert!(
            err.to_string().contains("not implemented"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extract_rse_path_escapes_special_chars_in_namespace() {
        // Namespaces contain ':', '/', and '-' which carry meaning in regex
        // without escaping.  Verify the regex still matches correctly.
        let ns = "ska:ska-sdp/eb-m001";
        let locations = vec![make_location(
            "MARSSRC-OLYMPUSMONS-T0",
            OLYMPUSMONS_AREA_ID,
            &[&format!(
                "davs://xrootd01.olympusmons.marssrc.org:1094/skadata/{ns}/data.fits"
            )],
        )];

        let result = extract_rse_path(&locations, ns, "data.fits").unwrap();
        assert_eq!(result, format!("/{ns}/data.fits"));
    }

    // ── mount_data_impl ──────────────────────────────────────────────────────

    #[test]
    fn mount_data_impl_calls_mount_fn_with_correct_args() {
        use std::cell::Cell;

        let called = Cell::new(false);
        let mock_mount = |rse: &str, ns: &str, user: &str| -> Result<()> {
            assert_eq!(rse, "/ska:ns/data.fits");
            assert_eq!(ns, "ska:ns");
            assert_eq!(user, "alice");
            called.set(true);
            Ok(())
        };

        mount_data_impl("/ska:ns/data.fits", "ska:ns", "alice", mock_mount).unwrap();
        assert!(called.get(), "mount_fn was never called");
    }

    #[test]
    fn mount_data_impl_propagates_mount_fn_error() {
        let failing_mount = |_: &str, _: &str, _: &str| -> Result<()> {
            anyhow::bail!("bindfs failed");
        };

        let err =
            mount_data_impl("/ska:ns/data.fits", "ska:ns", "alice", failing_mount).unwrap_err();
        assert!(
            err.to_string().contains("bindfs failed"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mount_data_errors_when_sudo_user_not_set() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::remove_var("SUDO_USER");

        let err = mount_data("/ska:ns/data.fits", "ska:ns").unwrap_err();
        assert!(
            err.to_string().contains("SUDO_USER"),
            "unexpected error: {err}"
        );
    }
}
