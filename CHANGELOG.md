# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
