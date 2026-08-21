mod http_server;

use tracing_subscriber::EnvFilter;

use http_server::run_server;

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

    let addr = ([127, 0, 0, 1], 8765).into();

    run_server(addr, store).await
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
