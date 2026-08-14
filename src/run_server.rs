mod api_client;
mod http_server;
mod models;
mod mount;
mod oauth2;
mod path_finder;

use tracing_subscriber::EnvFilter;

use http_server::run_server;

///
// Run the pathfinder tool in HTTP server mode
///
#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    configure_logging();

    let store = http_server::create_store();
    let addr = ([127, 0, 0, 1], 8765).into();

    run_server(addr, store).await
}

fn configure_logging() {
    let filters= ["hyper_util=info"];

    let env_filters = EnvFilter::from_default_env();
    let tracing_filters = filters.into_iter().fold(env_filters, |acc, filter| {
        acc.add_directive(filter.parse().unwrap())
    });

    tracing_subscriber::fmt()
        .with_env_filter(tracing_filters)
        .init();

}
