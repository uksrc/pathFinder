# Development Guide

There are CI pipelines in GitHub for testing and building the executable, plus publishing an RPM on a release.

For manual testing there here are some hints:

## Prerequisites

- [Rust toolchain](https://rustup.rs/) (stable)
- Docker (for running integration tests locally)

## Building

      cargo build

For a release build:

      cargo build --release

## Running the binary locally

      cargo run -- --namespace daac --file-name pi24_test_run_1_cleaned.fits

## Testing

### Unit tests

Run the fast unit test suite (no root required, all I/O mocked):

      cargo test

### Integration tests

The integration tests exercise real `chown` behaviour — they create a temporary system user (`pf_testuser`), call the actual `chown` binary, and verify ownership changes on disk. They require root on Linux.

#### Locally via Docker (recommended before pushing)

      docker build -f Dockerfile.test -t pf-test .
      docker run --rm pf-test

This builds a `rust:latest` container (runs as root) and executes `cargo test -- --include-ignored`.

#### Directly (if already root on a Linux machine)

      cargo test -- --include-ignored

Integration tests are skipped automatically when not running as root, so it is safe to run this command in any environment.

#### In CI

The `integration-test` job in `.github/workflows/ci.yml` runs `cargo test -- --include-ignored` inside a `rust:latest` container, which provides root automatically. It runs in parallel with the regular unit test job on every push and pull request.

### Cleaning up after an interrupted integration test

If a test run is interrupted before the `pf_testuser` system user is removed, subsequent runs will panic with a clear message. Remove the leftover user with:

      sudo userdel pf_testuser
