#!/usr/bin/env python3
#
# path-finder: A tool for finding SKA data paths for mounting purposes.
#

import itertools
import os
import re

import requests

from models.data_management import DataLocationAPIResponse, DataLocation
from models.site_capabilities import (
    StorageAreaIDToNodeAndSite,
    NodesAPIResponse,
    get_all_node_storage_areas,
)


# Inputs - these can be inputs
DATA_NAMESPACE = "daac"
DATA_FILE = "pi24_test_run_1_cleaned.fits"

# Upstream services
DM_API_BASEURL = "https://data-management.srcnet.skao.int/api/v1"
SC_API_BASEURL = "https://site-capabilities.srcnet.skao.int/api/v1"


def main(namespace: str = DATA_NAMESPACE, file_name: str = DATA_FILE) -> None:
    """Main function to locate data and print out storage area information."""

    check_namespace_available(namespace)

    site_storages = site_storage_areas()
    data_locations = locate_data(namespace, file_name)

    print_data_locations_with_sites(site_storages, data_locations)


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
      list[str]: A list of available namespace strings.
    """
    headers = {"Authorization": f"Bearer {DATA_MANAGEMENT_ACCESS_TOKEN}"}
    try:
        response = requests.get(f"{DM_API_BASEURL}/data/list", headers=headers)
        response.raise_for_status()
    except requests.exceptions.RequestException as e:
        raise RuntimeError(f"Error requesting namespaces from DM API:\n{e}")
    namespaces = response.json()
    return namespaces


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
      list[DataLocation]: A list of DataLocation objects representing the locations of the data file.
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


def extract_rse_path(
    data_locations: list[DataLocation], namespace: str, file_name: str
) -> str:
    replica_uris = itertools.chain.from_iterable(
        [location.replicas for location in data_locations]
    )

    # Extract the last part of the URIs, from the namespace path segment onwards
    # Use a regex
    rse_path_match = re.compile(rf"/{namespace}/.*$")
    matched_paths: set[str] = set()
    unmatched_paths: list[str] = []
    for uri in replica_uris:
        match = rse_path_match.search(uri)
        if match:
            matched_paths.add(match.group(0))
        else:
            print(f"Warning: No match found in URI '{uri}' for namespace '{namespace}'")
            unmatched_paths.append(uri)

    if len(unmatched_paths) > 0:
        print(
            f"Warning: {len(unmatched_paths)} URIs did not match the expected pattern."
        )
        print(f"Unmatched URIs: {unmatched_paths}")

    if len(matched_paths) > 1:
        print(
            f"Warning: Multiple unique matched paths found for file '{file_name}' in namespace '{namespace}': {matched_paths}"
        )
        print(f"Matched paths: {matched_paths}")
        print(
            "We should check the path for the local RSE - by cross-referencing with site capabilities."
        )
        raise NotImplementedError("Handling multiple matched paths is not implemented.")

    if len(matched_paths) == 0:
        raise RuntimeError(
            f"No valid paths found for file '{file_name}' in namespace '{namespace}'."
        )

    return matched_paths.pop()


if __name__ == "__main__":
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

    main()
