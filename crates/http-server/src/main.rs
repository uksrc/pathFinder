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

    let db_path = "/Users/roger.duthie/.sqlite/pathfinder.db";
    let store = SharedStore::new(db_path).await?;

    // TODO: Check if option to mark `Started` requests as `Failed` in the database is set
    // TODO: Think of a good verb to describe what this process is (`cleanse`?)

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
