# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Shared JWKS authentication module (`pathfinder-shared::jwks_auth`) used by both the CLI and the HTTP server.
- CLI `--token <TOKEN>` option for supplying a bearer token directly.
- CLI support for the `PATHFINDER_SKA_AUTH_TOKEN` environment variable as a token source.
- CLI validates the supplied bearer token against the SKA-IAM JWKS and extracts the caller `sub` before exchanging it for Data Management and Site Capabilities API tokens.
- HTTP server now reuses the shared JWKS authenticator while preserving the `Claims<JwtClaims>` extractor injection.

### Changed

- CLI authentication now requires a bearer token (`--token` or `PATHFINDER_SKA_AUTH_TOKEN`); the OAuth2 device-code flow and cached-token path have been removed.

### Removed

- `--no-login` CLI option.
- `DATA_MANAGEMENT_ACCESS_TOKEN` and `SITE_CAPABILITIES_ACCESS_TOKEN` environment variables as direct CLI token sources.

## v1.0.2 (2026-04-13)

### Fixed

- Mounting two or more files no longer causes error when settting permissions.
- Fixed documentation when the `--help` flag is set.

## v1.0.1 (2026-04-01)

### Added

- Test coverage and GitHub action to run them
- GitHub action to create and publish RPM on release

### Changed

- Removed unwanted logging of RSE paths
- Don't make site capabilities API call unless file isn't found in local RSE mount

## v1.0.0

### Added

- Initial Rust implementation
