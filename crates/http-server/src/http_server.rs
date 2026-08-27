use axum::{
    extract::{FromRef, Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::Json,
    routing::{get, post},
    Router,
};
use axum_jwt_auth::{Claims, Decoder};
use futures::future::join_all;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use sqlx::types::Json as SqlxJson;
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use pathfinder_shared::{
    jwt::JwtClaims,
    oauth2::{async_obtain_api_tokens, Tokens},
    path_finder::run_spawn,
    store::{RecordState, SharedStore, StageInRecord},
};

// ---------------------------------------------------------------------------
// Request / response models (mirroring the Python FastAPI app)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StageInRequest {
    dids: Vec<String>,
    project_name: Path,
}

#[derive(Debug, Serialize)]
pub struct StageInResponse {
    request_id: Uuid,
    state: RecordState,
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
            state: record.state,
            input_path: record.input_path,
            output_path: record.output_path,
            work_path: record.work_path,
            message: record.message,
        }
    }
}

impl From<pathfinder_shared::store::RequestStoreRow> for StageInResponse {
    fn from(row: pathfinder_shared::store::RequestStoreRow) -> Self {
        let record = row.to_stage_in_record();
        StageInResponse {
            request_id: record.request_id,
            state: record.state,
            input_path: record.input_path,
            output_path: record.output_path,
            work_path: record.work_path,
            message: record.message,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct StageOutRequest {
    request_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct StageOutResponse {
    request_id: Uuid,
    state: RecordState,
    message: Option<String>,
}

// ---------------------------------------------------------------------------
// Store manipulation functions
// ---------------------------------------------------------------------------

/// Record an error record
async fn record_error(
    request_id: &Uuid,
    user_sub: &str,
    error_msg: String,
    store: &SharedStore,
) -> StageInRecord {
    let err_record = StageInRecord {
        request_id: request_id.clone(),
        state: RecordState::Failed,
        input_path: None,
        output_path: None,
        work_path: None,
        dids: SqlxJson(Vec::<String>::default()),
        message: Some(error_msg),
    };
    if let Err(err) = store
        .initialise_request_record(
            request_id,
            &user_sub.to_string(),
            Some(err_record.clone()),
            None,
            &RecordState::Failed,
        )
        .await
    {
        tracing::error!("failed to record error: {}", err);
    }
    err_record
}

// ---------------------------------------------------------------------------
// DID processing
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
    request_id: &Uuid,
    user_sub: &str,
    store: &SharedStore,
) -> Result<Vec<DidParse>, StageInRecord> {
    // Try to parse all DIDs
    let results = join_all(request.dids.iter().map(|did| async {
        match parse_did(did) {
            Ok(result) => {
                tracing::debug!(
                    "DID parsed: {{'namespace':'{}','filename':'{}'}}",
                    result.namespace,
                    result.filename
                );
                Ok(result)
            }
            Err(unparsed_did) => {
                tracing::debug!("DID not parsed: '{}'", unparsed_did);
                Err(unparsed_did)
            }
        }
    }))
    .await;

    // Handle any errors
    let (parsed_dids, parsing_errors): (Vec<DidParse>, Vec<String>) =
        results.into_iter().partition_result();
    match parsing_errors.is_empty() {
        true => Ok(parsed_dids),
        false => {
            let error_message = format!(
                "Unparsable DIDs in request: [{}]",
                parsing_errors.join(", ")
            );
            Err(record_error(request_id, user_sub, error_message, store).await)
        }
    }
}

/// Mount a single DID on a blocking thread and update the store on completion.
async fn mount_did(store: SharedStore, request_id: Uuid, did: DidParse, api_tokens: Tokens) {
    let did_str = format!("{}:{}", did.namespace, did.filename);
    let result =
        tokio::task::spawn_blocking(move || run_spawn(&did.namespace, &did.filename, &api_tokens))
            .await;

    match result {
        Err(err) => {
            tracing::error!("mount task panicked: {}", err);
            let _ = store.update_status(&request_id, &RecordState::Failed).await;
            let _ = store.add_to_message(&request_id, err.to_string()).await;
        }
        Ok(Err(err)) => {
            tracing::error!("run_spawn failed: {}", err);
            let _ = store.update_status(&request_id, &RecordState::Failed).await;
            let _ = store.add_to_message(&request_id, err.to_string()).await;
        }
        Ok(Ok(())) => {
            if let Err(err) = store.add_did_mounted(&request_id, &did_str).await {
                let message = format!("failed to record mounted DID: {}", err);
                tracing::error!(message);
                let _ = store.update_status(&request_id, &RecordState::Failed).await;
                let _ = store.add_to_message(&request_id, message).await;
            } else {
                // Check if all dids_requested are now in dids_mounted - if so, set status to StagedIn
                let store_result = match store.get(&request_id).await {
                    Ok(result) => result,
                    Err(err) => {
                        tracing::error!("failed to retrieve current record - {}", err);
                        return;
                    }
                };
                match store_result {
                    Some(mut record) => {
                        if record.dids_mounted.sort() == record.dids_requested.sort() {
                            // Mark the stage-in as completed!
                            let _ = store
                                .update_status(&request_id, &RecordState::StagedIn)
                                .await;
                        };
                    }
                    None => {
                        tracing::error!("no record matching request ID {}", request_id);
                        return;
                    }
                };
            }
        }
    }
}

async fn unmount_did(
    store: SharedStore,
    request_id: Uuid,
    unparsed_did: String,
    base_path: String,
) {
    let did = match parse_did(&unparsed_did) {
        Ok(result) => result,
        Err(err) => {
            // set record to failed and add a message
            return;
        }
    };
    // call the pathfinder function to unmount the file
    let result = tokio::task::spawn_blocking(move || {
        spawned_unmount_data(&base_path, &did.namespace, &did.filename)
    })
    .await;
}

// ---------------------------------------------------------------------------
// Auth token processing
// ---------------------------------------------------------------------------

/// Parse the auth token or return a failed stage-in record
async fn validate_auth_token(
    auth_token: &str,
    request_id: &Uuid,
    user_sub: &str,
    store: &SharedStore,
) -> Result<Tokens, StageInRecord> {
    match async_obtain_api_tokens(&auth_token.to_string()).await {
        Ok(tokens) => Ok(tokens),
        Err(error) => Err(record_error(request_id, user_sub, error.to_string(), store).await),
    }
}

/// Extract the raw bearer token from the `Authorization: Bearer <token>` header.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
/// Stage in and stage out processing functions
// ---------------------------------------------------------------------------

async fn process_stage_in(
    store: &SharedStore,
    claims: &JwtClaims,
    raw_token: &str,
    request: &StageInRequest,
) -> (StatusCode, StageInResponse) {
    tracing::debug!("process_stage_in called");
    let request_id = Uuid::new_v4();

    // The JWT has already been validated by the `Claims` extractor; the `sub`
    // claim identifies the authenticated user.
    let user_sub = claims.sub.clone();

    let dids: Vec<DidParse> = match validate_dids(request, &request_id, &user_sub, store).await {
        Err(error_record) => {
            return (StatusCode::BAD_REQUEST, error_record.into());
        }
        Ok(result) => result,
    };

    // Exchange the validated bearer token for SRCNet API access tokens.
    let api_tokens = match validate_auth_token(raw_token, &request_id, &user_sub, store).await {
        Err(error_record) => {
            return (StatusCode::UNAUTHORIZED, error_record.into());
        }
        Ok(result) => result,
    };

    let parent_path = "/home/ska_service_user"; // TODO: Read from config file
    let record = StageInRecord {
        request_id: request_id.clone(),
        state: RecordState::StagingIn,
        input_path: Some(format!("{}/{}/data", parent_path, request_id)),
        output_path: Some(format!("{}/{}/project", parent_path, request_id)),
        work_path: Some(format!("{}/{}/scratch", parent_path, request_id)),
        dids: SqlxJson(request.dids.clone()),
        message: None,
    };

    if let Err(err) = store
        .initialise_request_record(
            &request_id,
            &user_sub,
            Some(record.clone()),
            Some(request.dids.clone()),
            &RecordState::StagedIn,
        )
        .await
    {
        tracing::error!("failed to initialise request record: {}", err);
    }

    // Spawn one async task per DID — each runs its mount on a blocking thread
    // and updates the store on failure. The handler returns 202 immediately.
    for did in dids {
        let store = store.clone();
        let api_tokens = api_tokens.clone();
        tokio::spawn(mount_did(store, request_id, did, api_tokens));
    }

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
    Claims { claims, .. }: Claims<JwtClaims>,
    State(store): State<SharedStore>,
    headers: HeaderMap,
    Json(body): Json<StageInRequest>,
) -> (StatusCode, Json<StageInResponse>) {
    tracing::info!(user = %claims.sub, "stage-in | dids=[{}]", body.dids.join(","));

    // The `Claims` extractor has already validated the token; we only need its
    // raw string form to exchange for SRCNet API access tokens.
    let raw_token = match bearer_token(&headers) {
        Some(token) => token,
        None => {
            let request_id = Uuid::new_v4();
            let record = record_error(
                &request_id,
                &claims.sub,
                "missing or malformed Authorization bearer token".into(),
                &store,
            )
            .await;
            return (StatusCode::UNAUTHORIZED, Json(record.into()));
        }
    };

    let (status, response) = process_stage_in(&store, &claims, &raw_token, &body).await;
    (status, Json(response))
}

/// GET /stage-in/{request_id} — poll the status of an async stage-in request.
async fn get_stage_in_status(
    Claims { claims, .. }: Claims<JwtClaims>,
    State(store): State<SharedStore>,
    Path(request_id): Path<Uuid>,
) -> (StatusCode, Json<StageInResponse>) {
    tracing::info!(user = %claims.sub, request_id = %request_id, "get stage-in status");
    match store.get(&request_id).await {
        Ok(Some(record)) => {
            let response = record.into();
            (StatusCode::OK, Json(response))
        }
        Ok(None) => {
            let response = StageInResponse {
                request_id: request_id.clone(),
                state: RecordState::Unknown,
                input_path: None,
                output_path: None,
                work_path: None,
                message: Some(format!("No request found with ID: {}", request_id)),
            };
            (StatusCode::NOT_FOUND, Json(response))
        }
        Err(err) => {
            let response = StageInResponse {
                request_id: request_id.clone(),
                state: RecordState::Unknown,
                input_path: None,
                output_path: None,
                work_path: None,
                message: Some(format!("Error retrieving request record. {}", err)),
            };
            (StatusCode::INTERNAL_SERVER_ERROR, Json(response))
        }
    }
}

/// POST /stage-out — signal payload completion and trigger stage-out.
async fn stage_out(
    Claims { claims, .. }: Claims<JwtClaims>,
    Json(body): Json<StageOutRequest>,
) -> Json<StageOutResponse> {
    tracing::info!(
        user = %claims.sub,
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

/// Application state shared across all request handlers.
///
/// Carries both the shared store and the JWT decoder that backs the
/// `Claims<JwtClaims>` extractor (via `FromRef`).
#[derive(Clone, FromRef)]
pub struct AppState {
    pub store: SharedStore,
    pub decoder: Decoder<JwtClaims>,
}

/// Build the router with all battle-API endpoints.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/stage-in", post(stage_in))
        .route("/stage-in/{request_id}", get(get_stage_in_status))
        .route("/stage-out", post(stage_out))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

/// Start the HTTP server and block.
pub async fn run_server(addr: SocketAddr, state: AppState) -> Result<(), anyhow::Error> {
    let app = build_router(state);

    tracing::info!("HTTP server listening on {}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;

    Ok(())
}
