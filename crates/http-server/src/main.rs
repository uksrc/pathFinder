mod http_server;

use std::sync::Arc;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

use http_server::{run_server, AppState};

use pathfinder_shared::jwks_auth::RemoteJwksAuth;
use pathfinder_shared::store::SharedStore;

///
// Run the pathfinder tool in HTTP server mode
///
#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    configure_logging();

    let db_path = "/Users/roger.duthie/.sqlite/pathfinder.db"; // TODO: Read from a config file
    let store = SharedStore::new(db_path).await?;
    store.fail_stale_requests().await?; // Any existing "Started" requests are set to "Failed" on restart

    // Build the shared JWKS authenticator: fetch the signing keys now and start
    // the background refresh task. The shutdown token must live for the life of
    // the server. The same validation / `sub`-extraction logic is available to
    // the CLI via `RemoteJwksAuth`; here we additionally expose the decoder to
    // the `Claims<JwtClaims>` extractor, preserving its injection into handlers.
    let auth = RemoteJwksAuth::new().context("failed to build remote JWKS authenticator")?;
    let _decoder_shutdown = auth
        .initialize_with_refresh()
        .await
        .context("failed to initialise remote JWKS authenticator")?;

    let state = AppState {
        store,
        decoder: Arc::new(auth.decoder()),
        obtain_tokens_fn: http_server::default_obtain_tokens_fn(),
        mount_fn: http_server::default_mount_fn(),
        unmount_fn: http_server::default_unmount_fn(),
    };

    let addr = ([127, 0, 0, 1], 8765).into();

    run_server(addr, state).await
}

fn configure_logging() {
    let filters = ["hyper_util=info"];

    let env_filters = EnvFilter::from_default_env();
    let tracing_filters = filters.into_iter().fold(env_filters, |acc, filter| {
        acc.add_directive(filter.parse().unwrap())
    });

    tracing_subscriber::fmt()
        .with_env_filter(tracing_filters)
        .init();
}
