# Path Finder

A Rust implementation of the SKA path finder tool for authentication, locating & mounting data from the SKA storage system within a Slurm login host.

## Overview

This project replaces the Python/Bash-based path finder (see git history) with a portable Rust implementation. It provides a single binary and an RPM installer.

## Features

- OAuth2 device code flow authentication
- Data location lookup via Data Management API
- Site capabilities verification via Site Capabilities API
- Secure data mounting with proper permissions

## Building

The binary and RPM are built and published on a GitHub release.

## Installation

1. Find the latest release in GitHub, and copy the URL of the published RPM.

2. On the Slurm login node:

    sudo dnf install [URL_TO_RELEASE_ARTEFACT]

## Usage

With OAuth2 authentication (recommended):

```bash
sudo pathFinder \
    --namespace daac \
    --file_name pi24_test_run_1_cleaned.fits
```

With environment variables (for automation):

```bash
export DATA_MANAGEMENT_ACCESS_TOKEN="your_token_here"
export SITE_CAPABILITIES_ACCESS_TOKEN="your_token_here"

sudo pathFinder \
    --namespace daac \
    --file_name pi24_test_run_1_cleaned.fits \
    --no-login
```

**Note**: The tool will automatically check if the file exists locally at `/skadata`. If the file is not found locally, it will display the sites where the file is available and prompt you to ensure the data has been staged to your local site before mounting.

## Architecture

### Modules

- **main.rs** - Main path finder CLI logic
- **api_client.rs** - HTTP client for Data Management and Site Capabilities APIs
- **oauth2_auth.rs** - OAuth2 device code flow implementation with token caching
- **models.rs** - Data structures for API responses (sites, nodes, storage areas, data locations)
- **mount.rs** - Mount/unmount utility for data access

## System Requirements

- **bindfs** - FUSE filesystem for permission remapping
- **sudo** - Required for mount operations
- **mountpoint** - Used to verify mount status

The system needs to have the local RSE mounted at `/skadata` as a 700 mount owned by root:root.  TODO: Ensure the program checks this and reports correctly if the share is not present.

A sudoers file needs to be added to allow members for the pathfinders group sudo privileges to the executable - TODO: Add this to the RPM.

