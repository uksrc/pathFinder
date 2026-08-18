use axum::{
    extract::Path,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Extension, Router,
};
use futures::future::join_all;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use pathfinder_shared::oauth2::{async_obtain_api_tokens, Tokens};
use pathfinder_shared::path_finder::run_spawn;

// ---------------------------------------------------------------------------
// Request / response models (mirroring the Python FastAPI app)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StageInRequest {
    token: String,
    dids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct StageInResponse {
    request_id: String,
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    work_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl From<StageInRecord> for StageInResponse {
    fn from(record: StageInRecord) -> Self {
        StageInResponse {
            request_id: record.request_id,
            state: format!("{:?}", record.state),
            input_path: record.input_path,
            output_path: record.output_path,
            work_path: record.work_path,
            message: record.message,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct StageOutRequest {
    token: String,
    data: String,
    scratch_space: String,
    request_id: String,
}

#[derive(Debug, Serialize)]
pub struct StageOutResponse {
    acknowledged: bool,
    audit_ref: String,
}

// ---------------------------------------------------------------------------
// Stub: shared in-memory record store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
enum RecordState {
    Started,
    Finished,
    Failed,
}

#[derive(Debug, Clone)]
pub struct StageInRecord {
    request_id: String,
    state: RecordState,
    input_path: Option<String>,
    output_path: Option<String>,
    work_path: Option<String>,
    message: Option<String>,
}

pub type SharedStore = Arc<Mutex<std::collections::HashMap<String, StageInRecord>>>;

pub fn create_store() -> SharedStore {
    Arc::new(Mutex::new(std::collections::HashMap::new()))
}

// ---------------------------------------------------------------------------
// DID parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct DidParse {
    pub namespace: String,
    pub filename: String,
}

/// Parse a DID string into its component namespace & filename parts
///
/// On error return the unparsed DID
fn parse_did(unparsed_did: &str) -> Result<DidParse, String> {
    let parts: Vec<&str> = unparsed_did.split(':').collect();
    match parts.len() {
        2 => Ok(DidParse {
            namespace: parts[0].trim().to_string(),
            filename: parts[1].trim().to_string(),
        }),
        _ => Err(unparsed_did.to_string()),
    }
}

/// Parse the DID or return a failed stage-in record
async fn validate_dids(
    request: &StageInRequest,
    request_id: &str,
    store: &SharedStore,
) -> Result<Vec<DidParse>, StageInRecord> {
    // Try to parse all DIDs
    let results = join_all(request.dids.iter().map(|did| async {
        match parse_did(did) {
            Ok(result) => {
                tracing::debug!("DID parsed: {{'namespace':'{}','filename':'{}'}}", result.namespace, result.filename);
                Ok(result)
            }
            Err(unparsed_did) => {
              tracing::debug!("DID not parsed: '{}'", unparsed_did);
              Err(unparsed_did)
            },
        }
    }))
    .await;

    // Handle any errors
    let (parsed_dids, parsing_errors): (Vec<DidParse>, Vec<String>) = results.into_iter().partition_result();
    match parsing_errors.is_empty() {
        true => Ok(parsed_dids),
        false => {
            let error_message = format!("Unparsable DIDs in request: [{}]", parsing_errors.join(", "));
            Err(record_error(request_id.to_string(), error_message, store).await)
        }
    }
}

/// Parse the auth token or return a failed stage-in record
async fn validate_auth_token(
    auth_token: &str,
    request_id: &str,
    store: &SharedStore,
) -> Result<Tokens, StageInRecord> {
    match async_obtain_api_tokens(&auth_token.to_string()).await {
        Ok(tokens) => Ok(tokens),
        Err(error) => Err(record_error(request_id.to_string(), error.to_string(), store).await),
    }
}

/// Record an error record
async fn record_error(request_id: String, error_msg: String, store: &SharedStore) -> StageInRecord {
    let err_record = StageInRecord {
        request_id: request_id.clone(),
        state: RecordState::Failed,
        input_path: None,
        output_path: None,
        work_path: None,
        message: Some(error_msg),
    };
    store.lock().await.insert(request_id, err_record.clone());
    err_record
}

// ---------------------------------------------------------------------------`
// Stub handler: actual stage-in logic (called by POST /stage-in)
// ---------------------------------------------------------------------------

async fn process_stage_in(
    store: &SharedStore,
    request: &StageInRequest,
) -> (StatusCode, StageInResponse) {
    tracing::debug!("process_stage_in called");
    let request_id = Uuid::new_v4().to_string();

    let dids: Vec<DidParse> = match validate_dids(request, &request_id, store).await {
        Err(error_record) => {
            return (StatusCode::BAD_REQUEST, error_record.into());
        }
        Ok(result) => result,
    };

    let api_tokens = match validate_auth_token(&request.token, &request_id, store).await {
        Err(error_record) => {
            return (StatusCode::UNAUTHORIZED, error_record.into());
        }
        Ok(result) => result,
    };

    let parent_path = "/home/ska_service_user"; // TODO: Read from config file

    let record = StageInRecord {
        request_id: request_id.clone(),
        state: RecordState::Started,
        input_path: Some(format!("{}/{}/data", parent_path, request_id)),
        output_path: Some(format!("{}/{}/project", parent_path, request_id)),
        work_path: Some(format!("{}/{}/scratch", parent_path, request_id)),
        message: None,
    };

    store
        .lock()
        .await
        .insert(request_id.clone(), record.clone());

    // Spawn the mount operation on a blocking thread pool (ApiClient uses reqwest::blocking::Client).
    tokio::task::spawn_blocking(move || {
        for did in dids {
        if let Err(error) = run_spawn(did.namespace.as_str(), did.filename.as_str(), &api_tokens) {
            tracing::error!("run_spawn failed: {}", error);
        }
      }
    });

    // Return immediately with 202 Accepted (async operation started)
    (StatusCode::ACCEPTED, record.into())
}

// ---------------------------------------------------------------------------
// Stub handler: actual stage-out logic (called by POST /stage-out)
// ---------------------------------------------------------------------------

async fn process_stage_out(_request: &StageOutRequest) -> StageOutResponse {
    let audit_ref = format!("audit-{}", Uuid::new_v4().simple());

    StageOutResponse {
        acknowledged: true,
        audit_ref,
    }
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

/// POST /stage-in — initiate async data staging into scratch.
async fn stage_in(
    Extension(store): Extension<SharedStore>,
    Json(body): Json<StageInRequest>,
) -> (StatusCode, Json<StageInResponse>) {
    tracing::info!("stage-in | dids=[{}]", body.dids.join(","));

    let (status, response) = process_stage_in(&store, &body).await;
    (status, Json(response))
}

/// GET /stage-in/{request_id} — poll the status of an async stage-in request.
async fn get_stage_in_status(
    Extension(store): Extension<SharedStore>,
    Path(request_id): Path<String>,
) -> (StatusCode, Json<StageInResponse>) {
    let store = store.lock().await;

    match store.get(&request_id) {
        Some(record) => {
            let response = record.clone().into();
            (StatusCode::OK, Json(response))
        }
        None => {
            let response = StageInResponse {
                request_id: request_id.clone(),
                state: "NOT_FOUND".into(),
                input_path: None,
                output_path: None,
                work_path: None,
                message: Some(format!("No stage-in record found for {}", request_id)),
            };
            (StatusCode::NOT_FOUND, Json(response))
        }
    }
}

/// POST /stage-out — signal payload completion and trigger stage-out.
async fn stage_out(Json(body): Json<StageOutRequest>) -> Json<StageOutResponse> {
    tracing::info!(
        "stage-out | data={} scratch={} request={}",
        body.data,
        body.scratch_space,
        body.request_id
    );

    let response = process_stage_out(&body).await;
    Json(response)
}

// ---------------------------------------------------------------------------
// Server setup
// ---------------------------------------------------------------------------

/// Build the router with all battle-API endpoints.
pub fn build_router(store: SharedStore) -> Router {
    Router::new()
        .route("/stage-in", post(stage_in))
        .route("/stage-in/{request_id}", get(get_stage_in_status))
        .route("/stage-out", post(stage_out))
        .layer(Extension(store)) // in-memory stateful store
        .layer(TraceLayer::new_for_http())
}

/// Start the HTTP server and block.
pub async fn run_server(addr: SocketAddr, store: SharedStore) -> Result<(), anyhow::Error> {
    let app = build_router(store);

    tracing::info!("HTTP server listening on {}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;

    Ok(())
}
