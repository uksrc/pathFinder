//! Core path-finding logic: locating replica paths, checking local availability, and mounting.

use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashSet;
use std::env;

use crate::models::{DataLocation, StorageAreaIDToNodeAndSite};

/// Prints each data location enriched with its node, site, and storage area name.
///
/// Falls back to printing the raw storage area ID if the area is not found in `site_stores`.
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
    use std::path::Path;
    let local_path = format!("/skadata{}", rse_path);
    Path::new(&local_path).exists()
}

/// Extracts the canonical RSE path from the replica URIs in `data_locations`.
///
/// Searches each replica URI for a `/<namespace>/...` suffix using a regex. Logs a
/// warning if some URIs do not match. Returns an error if no paths are found or if
/// multiple distinct paths are found (cross-site disambiguation is not yet implemented).
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

/// Mounts the data file at `rse_path` into the `SUDO_USER`'s home directory via bindfs.
///
/// Reads `SUDO_USER` from the environment (guaranteed to be set by [`crate::cli::check_privileges`]).
pub fn mount_data(rse_path: &str, namespace: &str) -> Result<()> {
    println!(
        "Mounting data from RSE path: {} in namespace: {}",
        rse_path, namespace
    );

    // Get the original user (already verified in check_privileges())
    let sudo_user = env::var("SUDO_USER").context("SUDO_USER not set")?;

    crate::mount::mount_operation(rse_path, namespace, &sudo_user)?;
    println!(
        "Successfully mounted {} in namespace {}",
        rse_path, namespace
    );

    Ok(())
}
