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

use crate::models::{DataLocation, StorageAreaIDToNodeAndSite};

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
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Serialise tests that mutate the process environment.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // ── helpers ──────────────────────────────────────────────────────────────

    fn make_location(id: &str, area_id: &str, replicas: &[&str]) -> DataLocation {
        DataLocation {
            identifier: id.to_string(),
            associated_storage_area_id: area_id.to_string(),
            replicas: replicas.iter().map(|s| s.to_string()).collect(),
            is_dataset: false,
        }
    }

    const OLYMPUSMONS_AREA_ID: &str = "12345678-90ab-cdef-1234-567890abcdef";

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
