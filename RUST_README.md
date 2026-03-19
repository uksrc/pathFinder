# Path Finder - Rust Implementation

A Rust implementation of the SKA path finder tool for locating and mounting data from SKA storage systems.

## Overview

This project replaces the Python-based path finder with a high-performance Rust implementation. It provides two binaries:

1. **path-finder** - Main CLI tool for authenticating and locating SKA data
2. **pathfinder-mount** - Utility for mounting/unmounting data using bindfs

## Features

- OAuth2 device code flow authentication
- Token caching for improved performance
- Data location lookup via Data Management API
- Site capabilities verification via Site Capabilities API
- Automated data mounting with proper permissions
- Error handling and validation

## Building

```bash
cargo build --release
```

The binaries will be available in `target/release/`:

- `target/release/path-finder`
- `target/release/pathfinder-mount`

## Installation

### Option 1: Install from source

```bash
cargo install --path .
```

### Option 2: Manual installation

```bash
sudo cp target/release/path-finder /usr/local/bin/
sudo cp target/release/pathfinder-mount /usr/local/bin/
sudo chmod +x /usr/local/bin/path-finder
sudo chmod +x /usr/local/bin/pathfinder-mount
```

## Usage

### Main Path Finder

With OAuth2 authentication (recommended):

```bash
path-finder \
    --namespace daac \
    --file_name pi24_test_run_1_cleaned.fits
```

With environment variables (for automation):

```bash
export DATA_MANAGEMENT_ACCESS_TOKEN="your_token_here"
export SITE_CAPABILITIES_ACCESS_TOKEN="your_token_here"

path-finder \
    --namespace daac \
    --file_name pi24_test_run_1_cleaned.fits \
    --no-login
```

**Note**: The tool will automatically check if the file exists locally at `/skadata`. If the file is not found locally, it will display the sites where the file is available and prompt you to ensure the data has been staged to your local site before mounting.

### Mount Utility

The mount utility is designed to be called with sudo privileges. It handles:

- Creating bind mounts from `/skadata` to user home directories
- Setting appropriate permissions
- Managing mount points to avoid cyclic mounts

Mount data:

```bash
sudo pathfinder-mount --mount /daac/pi24_test_run_1_cleaned.fits daac
```

Unmount data:

```bash
sudo pathfinder-mount --unmount /daac/pi24_test_run_1_cleaned.fits daac
```

## Architecture

### Modules

- **oauth2_auth.rs** - OAuth2 device code flow implementation with token caching
- **models.rs** - Data structures for API responses (sites, nodes, storage areas, data locations)
- **api_client.rs** - HTTP client for Data Management and Site Capabilities APIs
- **main.rs** - Main path finder CLI logic
- **mount.rs** - Mount/unmount utility for data access

### Authentication Flow

1. Initiate device code flow with authn service
2. Display user code and verification URL
3. Poll for authentication completion
4. Exchange device token for API-specific tokens
5. Cache tokens for future use (default: 1 hour)

### Data Location Flow

1. Verify namespace exists in Data Management API
2. Verify site name exists in Site Capabilities API
3. Fetch site storage area mappings
4. Locate data file in namespace
5. Verify data is available at requested site
6. Extract RSE path from replica URIs
7. Call mount utility to make data accessible

## Dependencies

- **clap** - Command-line argument parsing
- **reqwest** - HTTP client
- **serde** - Serialization/deserialization
- **anyhow** - Error handling
- **regex** - Pattern matching for RSE paths
- **dirs** - Cross-platform config directory location

## System Requirements

- **bindfs** - FUSE filesystem for permission remapping
- **sudo** - Required for mount operations
- **mountpoint** - Used to verify mount status

## Token Caching

Tokens are cached in `~/.config/path-finder/tokens.json` with secure permissions (0600).
Cache expires after 1 hour (configurable in code).

## Error Handling

The tool provides detailed error messages for:

- Network failures
- Authentication failures
- Missing data or sites
- Permission issues
- Mount failures

## Comparison with Python Implementation

| Feature        | Python                    | Rust              |
| -------------- | ------------------------- | ----------------- |
| Performance    | Slower                    | Faster            |
| Memory Usage   | Higher                    | Lower             |
| Binary Size    | N/A (interpreted)         | ~6MB (release)    |
| Dependencies   | Runtime Python + packages | Statically linked |
| Error Messages | Good                      | Excellent         |
| Type Safety    | Runtime (Pydantic)        | Compile-time      |

## Development

Run tests:

```bash
cargo test
```

Format code:

```bash
cargo fmt
```

Lint code:

```bash
cargo clippy
```

## License

Same as the original Python implementation.
