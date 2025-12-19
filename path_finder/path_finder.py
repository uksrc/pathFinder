#!/usr/bin/env python3
#
#
# path-finder: A tool for finding SKA data paths for mounting purposes.
#

import itertools
import os
import re

import requests


# Inputs - these can be inputs
DATA_NAMESPACE="daac"
DATA_FILE="pi24_test_run_1_cleaned.fits"

# Upstream services
DM_API_BASEURL="https://data-management.srcnet.skao.int/api/v1"


def locate_data(namespace: str, file_name: str) -> str:
  """Locate a data file within a specified namespace.

  Args:
    namespace (str): the file namespace - e.g. 'testing', 'daac', 'teal', 'neon'
    file_name (str): the path of the file within the namespace - e.g. 'pi24_test_run_1_cleaned.fits', 'pi25_daac_tests'

  Returns:
    str: Description
  """
  if namespace not in ALL_NAMESPACES:
    raise ValueError(f"Namespace '{namespace}' not found. Available namespaces: {ALL_NAMESPACES}")

  # Query the Data Management API to locate the file
  try:
      response = requests.get(
          f"{DM_API_BASEURL}/data/locate/{namespace}/{file_name}",
          headers=HEADERS,
      )
      response.raise_for_status()
      data_locations = response.json()
  except requests.exceptions.RequestException as e:
      raise RuntimeError(f"Error locating file '{file_name}' in namespace '{namespace}': {e}")

  if not data_locations:
      raise FileNotFoundError(f"File '{file_name}' not found in namespace '{namespace}'.")

  uris = itertools.chain.from_iterable([location["replicas"] for location in data_locations])

  # Extract the last part of the URIs, from the namespace path segment onwards
  # Use a regex
  rse_path_match = re.compile(rf"/{namespace}/.*$")
  matched_paths: set[str] = set()
  unmatched_paths: list[str] = []
  for uri in uris:
      match = rse_path_match.search(uri)
      if match:
          matched_paths.add(match.group(0))
      else:
          print(f"Warning: No match found in URI '{uri}' for namespace '{namespace}'")
          unmatched_paths.append(uri)

  if len(unmatched_paths) > 0:
      print(f"Warning: {len(unmatched_paths)} URIs did not match the expected pattern.")
      print(f"Unmatched URIs: {unmatched_paths}")

  if len(matched_paths) > 1:
      print(f"Warning: Multiple unique matched paths found for file '{file_name}' in namespace '{namespace}': {matched_paths}")
      print(f"Matched paths: {matched_paths}")
      print("We should check the path for the local RSE - by cross-referencing with site capabilities.")
      raise NotImplementedError("Handling multiple matched paths is not implemented.")

  if len(matched_paths) == 0:
      raise RuntimeError(f"No valid paths found for file '{file_name}' in namespace '{namespace}'.")

  return matched_paths.pop()


def main():
  file_path = locate_data(DATA_NAMESPACE, DATA_FILE)
  print(f"Located file path: {file_path}")


if __name__ == "__main__":

  # Set up the Data Management API access
  try:
      DATA_MANAGEMENT_ACCESS_TOKEN=os.environ["DATA_MANAGEMENT_ACCESS_TOKEN"]
  except KeyError:
      print("Error: Please set DATA_MANAGEMENT_ACCESS_TOKEN environment variable.")
      exit(1)

  HEADERS = {
    "Authorization": f"Bearer {DATA_MANAGEMENT_ACCESS_TOKEN}"
  }

  # Fetch all available namespaces from the Data Management API
  try:
      response = requests.get(f"{DM_API_BASEURL}/data/list", headers=HEADERS)
      response.raise_for_status()
      ALL_NAMESPACES = response.json()
  except requests.exceptions.RequestException as e:
      print("Error: Unable to fetch data namespaces from Data Management API.")
      print(f"Details: {e}")
      exit(1)

  main()
