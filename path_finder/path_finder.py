#!/usr/bin/env python3
#
# path-finder: A tool for finding SKA data paths for mounting purposes.
#

import argparse
import grp
import itertools
import os
import re
import subprocess
from venv import logger

import requests

from models.data_management import DataLocationAPIResponse, DataLocation
from models.site_capabilities import (
    Site,
    SitesAPIResponse,
    StorageAreaIDToNodeAndSite,
    NodesAPIResponse,
    get_all_node_storage_areas,
)


# Inputs - these can be inputs
DATA_NAMESPACE = "daac"
DATA_FILE = "pi24_test_run_1_cleaned.fits"
SLURM_SITE_NAME = "UKSRC-CAM-PREPROD"

# Upstream services
DM_API_BASEURL = "https://data-management.srcnet.skao.int/api/v1"
SC_API_BASEURL = "https://site-capabilities.srcnet.skao.int/api/v1"


def main(
    namespace: str = DATA_NAMESPACE,
    file_name: str = DATA_FILE,
    site_name: str = SLURM_SITE_NAME,
) -> None:
    """Main function to locate data and print out storage area information."""

    check_namespace_available(namespace)
    check_site_name_exists(site_name)

    site_storages = site_storage_areas()
    data_locations = locate_data(namespace, file_name)

    # FOR DEBUGGING PURPOSES ONLY - PRINT DATA LOCATIONS WITH SITES
    print_data_locations_with_sites(site_storages, data_locations)

    if not is_data_located_at_site(site_name, data_locations, site_storages):
        print(
            f"Data file '{file_name}' in namespace '{namespace}' is not located at site '{site_name}'."
        )
        print("Ensure that the data is staged to the site before proceeding.")
        # TODO: If the data isn't available at the SLURM_SITE_NAME, perhaps we could stage it
        exit(1)

    rse_path = extract_rse_path(data_locations, namespace, file_name)
    print(f"RSE Path for file '{file_name}' in namespace '{namespace}': {rse_path}")

    mount_data(rse_path, namespace)


def check_namespace_available(namespace: str) -> None:
    """Check if the specified namespace is available.

    Args:
      namespace (str): The namespace to check.

    Raises:
      RuntimeError: If the namespace is not available.
    """
    all_namespaces = get_all_namespaces()
    if namespace not in all_namespaces:
        raise RuntimeError(
            f"Namespace '{namespace}' not found in available namespaces: {all_namespaces}"
        )


def get_all_namespaces() -> list[str]:
    """Fetch all available namespaces from the Data Management API.

    Returns:
      A list of available namespace strings.
    """
    headers = {"Authorization": f"Bearer {DATA_MANAGEMENT_ACCESS_TOKEN}"}
    try:
        response = requests.get(f"{DM_API_BASEURL}/data/list", headers=headers)
        response.raise_for_status()
    except requests.exceptions.RequestException as e:
        raise RuntimeError(f"Error requesting namespaces from DM API:\n{e}")
    namespaces = response.json()
    return namespaces


def check_site_name_exists(site_name: str) -> None:
    """Check if the specified site name exists.

    Args:
      site_name (str): The site name to check.

    Raises:
      RuntimeError: If the site name does not exist.
    """
    all_sites = all_site_names()
    if site_name not in all_sites:
        logger.error(
            f"Error: Site name '{site_name}' not found in available sites:\n\n{', '.join(all_sites)}"
        )
        exit(1)


def all_site_names() -> list[str]:
    """Fetch the complete site capabilities and return all site name strings.

    Returns:
      A list of all available site name strings.
    """
    headers = {"Authorization": f"Bearer {SITE_CAPABILITIES_ACCESS_TOKEN}"}
    try:
        response = requests.get(f"{SC_API_BASEURL}/sites", headers=headers)
        response.raise_for_status()
    except requests.exceptions.RequestException as e:
        raise RuntimeError(f"Error requesting node information from SC API:\n{e}")

    nodes_response = SitesAPIResponse.validate_python(response.json())

    return [site.name for site in nodes_response]


def site_storage_areas() -> StorageAreaIDToNodeAndSite:
    """Fetch the site capabilities and obtain a storage area mapping of storage area IDs.

    Returns:
      StorageAreaIDToNodeAndSite: A mapping of storage area IDs to their corresponding node
        and site names.
    """
    headers = {"Authorization": f"Bearer {SITE_CAPABILITIES_ACCESS_TOKEN}"}
    try:
        response = requests.get(f"{SC_API_BASEURL}/nodes", headers=headers)
        response.raise_for_status()
    except requests.exceptions.RequestException as e:
        raise RuntimeError(f"Error requesting node information from SC API:\n{e}")

    nodes_response = NodesAPIResponse.validate_python(response.json())

    return get_all_node_storage_areas(nodes_response)


def locate_data(
    namespace: str,
    file_name: str,
) -> list[DataLocation]:
    """Locate a data file within a specified namespace.

    Args:
      namespace (str): the file namespace - e.g. 'testing', 'daac', 'teal', 'neon'
      file_name (str): the path of the file within the namespace - e.g. 'pi24_test_run_1_cleaned.fits', 'pi25_daac_tests'

    Returns:
      A list of DataLocation objects representing the locations of the data file.
    """

    headers = {"Authorization": f"Bearer {DATA_MANAGEMENT_ACCESS_TOKEN}"}

    # Query the Data Management API to locate the file
    try:
        response = requests.get(
            f"{DM_API_BASEURL}/data/locate/{namespace}/{file_name}",
            headers=headers,
        )
        response.raise_for_status()
    except requests.exceptions.RequestException as e:
        raise RuntimeError(
            f"Error requesting location of file '{file_name}' in namespace '{namespace}' from DM API:\n{e}"
        )

    data_locations_response = DataLocationAPIResponse.validate_python(response.json())
    return data_locations_response


def print_data_locations_with_sites(
    site_stores: StorageAreaIDToNodeAndSite, data_locations: list[DataLocation]
) -> None:
    """Print data locations with their associated site information.

    Args:
      site_storages: Mapping of storage area IDs to node and site names.
      data_locations: List of data location objects to print.
    """
    for location in data_locations:
        node_site = site_stores.get(location.associated_storage_area_id)
        if node_site:
            node_name, site_name = node_site
            print(
                f"Data location ID: {location.identifier}, Storage Area ID: {location.associated_storage_area_id}, Node: {node_name}, Site: {site_name}"
            )
        else:
            print(
                f"Data location ID: {location.identifier}, Storage Area ID: {location.associated_storage_area_id}, Node/Site: Not found"
            )


def is_data_located_at_site(
    site_name: str, data_locations: list[DataLocation], site_stores: StorageAreaIDToNodeAndSite
) -> bool:
    """Check if any data locations are associated with the specified site name.

    Args:
      site_name (str): The site name to check.
      data_locations (list[DataLocation]): The list of data locations to search.

    Returns:
      True if any data location is associated with the specified site name, False otherwise.
    """
    sites_with_data = [site_stores.get(location.associated_storage_area_id, (None, None))[1] for location in data_locations]

    print(f"Sites with data: {sites_with_data}")
    if site_name in sites_with_data:
        return True
    return False


def extract_rse_path(
    data_locations: list[DataLocation], namespace: str, file_name: str
) -> str:
    """Extract the RSE path from data locations for a given namespace and file name.

    Do checks:
      - at least one path is found
      - consistency across paths from different replicas

    Args:
      data_locations (list[DataLocation]): The list of data locations to search.
      namespace (str): The namespace of the data.
      file_name (str): The name of the data file.

    Returns:
      The extracted RSE path.
    """

    rse_path_match = re.compile(rf"/{namespace}/.*$")
    matched_paths: set[str] = set()
    unmatched_paths: list[str] = []

    replica_uris = itertools.chain.from_iterable(
        [location.replicas for location in data_locations]
    )
    for uri in replica_uris:
        match = rse_path_match.search(uri)
        if match:
            matched_paths.add(match.group(0))
        else:
            unmatched_paths.append(uri)

    # Report any unmatched URIs
    if unmatched_paths:
        print(
            f"Warning: {len(unmatched_paths)} URIs did not match the expected pattern."
        )
        print(f"Unmatched URIs: {unmatched_paths}")

    # Validate we have exactly one unique path
    if not matched_paths:
        raise RuntimeError(
            f"No valid paths found for file '{file_name}' in namespace '{namespace}'."
        )

    if len(matched_paths) > 1:
        print(f"Warning: Multiple unique paths found: {matched_paths}")
        print(
            "We should check the path for the local RSE - by cross-referencing with site capabilities."
        )
        raise NotImplementedError("Handling multiple matched paths is not implemented.")

    return matched_paths.pop()


def mount_data(rse_path: str, namespace: str) -> None:
    """Mount the data at the specified RSE path using sudo pathfinder.

    Args:
      rse_path (str): The RSE path to mount.
      namespace (str): The namespace of the data.

    Raises:
      RuntimeError: If the mount command fails.
    """
    print(f"Mounting data from RSE path: {rse_path} in namespace: {namespace}")

    # Construct the sudo command
    cmd = ["sudo", "pathfinder", "--mount", rse_path, namespace]

    try:
        # Execute the command
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            check=False,  # Don't raise exception, handle manually
            timeout=30  # 30 second timeout
        )

        # Print stdout if available
        if result.stdout:
            print(f"Mount output: {result.stdout.strip()}")

        # Check return code
        if result.returncode != 0:
            error_msg = f"Mount command failed with exit code {result.returncode}"
            if result.stderr:
                error_msg += f": {result.stderr.strip()}"
            raise RuntimeError(error_msg)

        print(f"Successfully mounted {rse_path} in namespace {namespace}")

    except subprocess.TimeoutExpired:
        raise RuntimeError("Mount command timed out after 30 seconds")
    except FileNotFoundError:
        raise RuntimeError("pathfinder command not found. Ensure it's installed and in PATH.")
    except PermissionError:
        raise RuntimeError("Permission denied. Ensure sudo is configured correctly.")
    except Exception as e:
        raise RuntimeError(f"Unexpected error during mount: {str(e)}")



if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Path Finder")
    parser.add_argument("--namespace", required=True, help="Namespace of the data")
    parser.add_argument("--file_name", required=True, help="Name of the data file")
    parser.add_argument(
        "--site_name", required=True, help="Site name where data is needed"
    )
    args = parser.parse_args()

    user = os.getlogin()
    groups = os.getgroups()
    sudo_user = os.environ.get("SUDO_USER")
    if sudo_user:
        print(f"Running path-finder as sudo user: {sudo_user}")
    else:
        print("Not running Python as sudo.")
    print(f"Running path-finder as local user: {user}")
    group_names = [grp.getgrgid(gid).gr_name for gid in groups]
    print(f"User '{user}' belongs to groups: {group_names}")

    # Ensure availability of API access tokens
    try:
        DATA_MANAGEMENT_ACCESS_TOKEN = os.environ["DATA_MANAGEMENT_ACCESS_TOKEN"]
    except KeyError:
        print("Error: Please set DATA_MANAGEMENT_ACCESS_TOKEN environment variable.")
        exit(1)

    try:
        SITE_CAPABILITIES_ACCESS_TOKEN = os.environ["SITE_CAPABILITIES_ACCESS_TOKEN"]
    except KeyError:
        print("Error: Please set SITE_CAPABILITIES_ACCESS_TOKEN environment variable.")
        exit(1)

    main(**vars(args))
