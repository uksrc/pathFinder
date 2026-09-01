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
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use pathfinder_shared::{
    jwt::JwtClaims,
    oauth2::{async_obtain_api_tokens, Tokens},
    path_finder::{run, spawned_unmount_data},
    store::{RecordState, SharedStore, StageInRecord},
};

// ---------------------------------------------------------------------------
// Request / response models (mirroring the Python FastAPI app)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StageInRequest {
    dids: Vec<String>,
    project_name: String,
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

type MountFn = Arc<dyn Fn(&str, &str, &str, &Tokens, fn(i32)) -> anyhow::Result<()> + Send + Sync>;

pub fn default_mount_fn() -> MountFn {
    Arc::new(
        |namespace: &str, filename: &str, base_path: &str, tokens: &Tokens, exit_fn: fn(i32)| {
            run(namespace, filename, base_path, tokens, exit_fn)
        },
    )
}

/// Mount a single DID on a blocking thread and update the store on completion.
async fn mount_did(
    store: SharedStore,
    request_id: Uuid,
    did: DidParse,
    api_tokens: Tokens,
    base_path: String,
    mount_fn: MountFn,
) {
    let did_str = format!("{}:{}", did.namespace, did.filename);
    let result = tokio::task::spawn_blocking(move || {
        mount_fn(
            &did.namespace,
            &did.filename,
            &base_path,
            &api_tokens,
            |code| {
                eprintln!("mount process exited with code {}", code);
            },
        )
    })
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
                    Some(record) => {
                        let mut mounted = record.dids_mounted.to_vec();
                        let mut requested = record.dids_requested.to_vec();
                        mounted.sort();
                        requested.sort();
                        if mounted == requested {
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

type UnmountFn = Arc<dyn Fn(&str, &str, &str) -> anyhow::Result<()> + Send + Sync>;

pub fn default_unmount_fn() -> UnmountFn {
    Arc::new(|base_path: &str, namespace: &str, filename: &str| {
        spawned_unmount_data(base_path, namespace, filename)
    })
}

async fn unmount_did(
    store: SharedStore,
    request_id: Uuid,
    did: DidParse,
    base_path: String,
    unmount_fn: UnmountFn,
) {
    let did_str = format!("{}:{}", did.namespace, did.filename);
    let result =
        tokio::task::spawn_blocking(move || unmount_fn(&base_path, &did.namespace, &did.filename))
            .await;

    match result {
        Err(err) => {
            tracing::error!("unmount task panicked: {}", err);
            let _ = store.update_status(&request_id, &RecordState::Failed).await;
            let _ = store.add_to_message(&request_id, err.to_string()).await;
        }
        Ok(Err(err)) => {
            tracing::error!("spawned_unmount_data failed: {}", err);
            let _ = store.update_status(&request_id, &RecordState::Failed).await;
            let _ = store.add_to_message(&request_id, err.to_string()).await;
        }
        Ok(Ok(())) => {
            if let Err(err) = store.remove_did_mounted(&request_id, &did_str).await {
                let message = format!("failed to record unmounted DID: {}", err);
                tracing::error!(message);
                let _ = store.update_status(&request_id, &RecordState::Failed).await;
                let _ = store.add_to_message(&request_id, message).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Auth token processing
// ---------------------------------------------------------------------------

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

type ObtainTokensFn =
    Arc<dyn Fn(&str) -> Pin<Box<dyn Future<Output = anyhow::Result<Tokens>> + Send>> + Send + Sync>;

pub fn default_obtain_tokens_fn() -> ObtainTokensFn {
    Arc::new(|token: &str| {
        let token = token.to_string();
        Box::pin(async move { async_obtain_api_tokens(&token).await })
    })
}

async fn process_stage_in(
    store: &SharedStore,
    claims: &JwtClaims,
    raw_token: &str,
    request: &StageInRequest,
    mount_fn: MountFn,
    obtain_tokens_fn: ObtainTokensFn,
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
    let api_tokens = match obtain_tokens_fn(raw_token).await {
        Err(error) => {
            return (
                StatusCode::UNAUTHORIZED,
                record_error(&request_id, &user_sub, error.to_string(), store)
                    .await
                    .into(),
            );
        }
        Ok(result) => result,
    };

    process_stage_in_inner(
        store, claims, request, request_id, dids, api_tokens, mount_fn,
    )
    .await
}

async fn process_stage_in_inner(
    store: &SharedStore,
    claims: &JwtClaims,
    request: &StageInRequest,
    request_id: Uuid,
    dids: Vec<DidParse>,
    api_tokens: Tokens,
    mount_fn: MountFn,
) -> (StatusCode, StageInResponse) {
    let user_sub = claims.sub.clone();
    let parent_path = "/home/ska_service_user"; // TODO: Read from config file
    let record = StageInRecord {
        request_id: request_id.clone(),
        state: RecordState::StagingIn,
        input_path: Some(format!("{}/{}/data", parent_path, request_id)),
        output_path: Some(format!(
            "{}/{}/project/{}",
            parent_path, request_id, request.project_name
        )),
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

    // Spawn a single async task that iterates over DIDs and launches a
    // per-DID mount task for each. The handler can then return a 202 immediately.
    let store = store.clone();
    tokio::spawn(async move {
        let input_path = format!("{}/{}/data", parent_path, request_id);
        for did in dids {
            let store = store.clone();
            let api_tokens = api_tokens.clone();
            tokio::spawn(mount_did(
                store,
                request_id,
                did,
                api_tokens,
                input_path.clone(),
                mount_fn.clone(),
            ));
        }
    });

    // Return immediately with 202 Accepted (async operation started)
    (StatusCode::ACCEPTED, record.into())
}

async fn process_stage_out(
    store: &SharedStore,
    claim: JwtClaims,
    request: &StageOutRequest,
    unmount_fn: UnmountFn,
) -> StageOutResponse {
    // Get record from store
    match store.get_for_user(&request.request_id, &claim.sub).await {
        Err(err) => StageOutResponse {
            request_id: request.request_id.clone(),
            state: RecordState::Unknown,
            message: Some(format!(
                "Error retrieving request from pathfinder store: {}",
                err
            )),
        },
        Ok(None) => StageOutResponse {
            request_id: request.request_id.clone(),
            state: RecordState::Unknown,
            message: Some(
                "Request not found in pathfinder store on this node for this user".to_string(),
            ),
        },
        Ok(Some(record)) => {
            let store = store.clone();
            let request_id = request.request_id.clone();
            match record.input_path {
                None => StageOutResponse {
                    request_id: request_id,
                    state: RecordState::Failed,
                    message: Some(format!(
                        "Record doesn't have an input path set: id={}",
                        request_id
                    )),
                },
                Some(input_path) => {
                    // For each did in the record, unmount
                    tokio::spawn(async move {
                        for did_str in record.dids_mounted.into_inner() {
                            match parse_did(&did_str) {
                                Ok(did) => {
                                    tokio::spawn(unmount_did(
                                        store.clone(),
                                        request_id.clone(),
                                        did,
                                        input_path.clone(),
                                        unmount_fn.clone(),
                                    ));
                                }
                                Err(unparsed) => {
                                    tracing::error!(
                                        "cannot parse mounted DID for unmount: {}",
                                        unparsed
                                    );
                                }
                            }
                        }
                    });
                    StageOutResponse {
                        request_id: request_id,
                        state: RecordState::StagingOut,
                        message: None,
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

/// POST /stage-in — initiate async data staging into scratch.
async fn stage_in(
    Claims { claims, .. }: Claims<JwtClaims>,
    State(state): State<AppState>,
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
                &state.store,
            )
            .await;
            return (StatusCode::UNAUTHORIZED, Json(record.into()));
        }
    };

    let (status, response) = process_stage_in(
        &state.store,
        &claims,
        &raw_token,
        &body,
        state.mount_fn.clone(),
        state.obtain_tokens_fn.clone(),
    )
    .await;
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
    State(state): State<AppState>,
    Json(body): Json<StageOutRequest>,
) -> Json<StageOutResponse> {
    tracing::info!(
        user = %claims.sub,
        "stage-out | request={}",
        body.request_id
    );

    let response = process_stage_out(&state.store, claims, &body, state.unmount_fn.clone()).await;
    Json(response)
}

// ---------------------------------------------------------------------------
// Server setup
// ---------------------------------------------------------------------------

/// Application state shared across all request handlers.
///
/// Carries both the shared store and the JWT decoder that backs the
/// `Claims<JwtClaims>` extractor (via `FromRef`). It also holds the
/// external-facing operations so tests can inject mocks.
#[derive(Clone, FromRef)]
pub struct AppState {
    pub store: SharedStore,
    pub decoder: Decoder<JwtClaims>,
    pub obtain_tokens_fn: ObtainTokensFn,
    pub mount_fn: MountFn,
    pub unmount_fn: UnmountFn,
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::Request;
    use httpmock::prelude::*;
    use pathfinder_shared::jwks_auth::RemoteJwksAuth;
    use pathfinder_shared::store::SharedStore;
    use serde_json::json;
    use std::time::Duration;
    use tempfile::TempDir;
    use tower::ServiceExt;

    // Test RSA key pair used to sign JWTs for handler tests.
    const TEST_PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
        MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC3nGpsk4cVdZeM\n\
        4ov1KYEPDlIQ4CV1Hw2ay+2fpvXGtAxtC/HDLPhl4LIHqoJMFYJylmvN4bf3+nby\n\
        mLzHvQxAjrrcf1LUpj5ILOgDfgEd8H3eyvpiYa6Ht7TiF/ZyqvRvKHgizigWorkq\n\
        U3fMkcWblBIHkwRBuW8hzTMN0yZLV62TlEH3Ol44+2VL45Af5y3kd0e3nIA9BuBI\n\
        P2J6ks+A6MLK82jgTM0hHoJakHGdQjmAd5kLVd/rzFXZ9O3V7aao0e91sLGfIwnl\n\
        JCOMQmf/0UikYn9DnpxLRkUzcaxOkLjATOogUB9KFT0T4ytsIlv40nxEqy2kp+cL\n\
        x62e/EgDAgMBAAECggEAFoj0iOHsbOZTVN/DNLJE3D+yM88G2eKXTV3lCri3po0X\n\
        j1StdfpxfDOBNi6nskXbjkvG7GxdI2rSqYC0fsFFnTDHX2OjG2VR9JLKYQ9YfL+0\n\
        +yCnbWa2wIJ8CVnOjhFMUc5CPGdYBTswhbDb3bgwbCFWuyZAmf5z1M62CubU5t8l\n\
        FCJ5Vta4+w8/G6ccEHOHwiTzDpjarvZxEEY2XanEOno70miHsy0BJLmwL/N+kI/G\n\
        avSfjC65i72vMvINDZfaPmCaFlIz1CnwxBKSjgJ1b8hUhntmcNcgPAdlyxwim72e\n\
        PzQlchzfGJ1o/0sIApO1AuWUqz5hATlyKvyiJCJVpQKBgQDquam/DYolNpYMEDCm\n\
        ooCBHURpHTyCga/QeIC96OEwidoRgHWMcvycOzCjPvEcg2Lkf0pB8qdDep6FEj73\n\
        Ag13aHf6dEowUQNzOlSYZk+uWBcWKvB8UUUIxuS7GYwe1YqxLp0/qTFhjz36UZqL\n\
        DAI1jDTvBZI56jZX/e80VEV+lwKBgQDIQL9KjaSUbFtBDEUSBP8qwm8YepUqP2LW\n\
        PuAm87+Yr/keyXxgrWPp3niboV6MVb50mfYGX6GuncsCvolj/U0uXmq3Dokk2idC\n\
        1au788XFeKGBbVZrhbjxezHaOamJNj5Q30ifFDUwOkqpZPNF2zGGWO1zgQqXF7rc\n\
        E9HJO4ybdQKBgFJLL7E1DQcJAUhPcM8rUAR0f2SfBHT5BOwBI5nxiOocmqDiOdQ5\n\
        CEm6Es5ZJe2KPuS/oAhJC82DswoSoJK3XINN1CqyFMSl0qDWhYw86pjEd6uk+FWN\n\
        pLd0DANw7Ihu88Y1ApqsNgzvTJpze8xeNHQTqQdYG7FEZTMqa3AcT5UXAoGARyoL\n\
        UPlJNZ3USCeOHDs+WvnB9VcKz3q7KxwpGG6i9iYDSBeeZdT4ntH61oPgT8rg5hsY\n\
        vWca1C0rSgxgUvJfjUzsa6V0w23raer5HtAgxm56Jr6uaYOaF+cJ7l1zjFmEh8Tx\n\
        z+akiEEO62f+tCKTVQUhTVzcYJmERFWexf6tl0kCgYEA1uf1IxRNWJfPxLrAyZvt\n\
        T1RpnjCCkVS5WkhIoEvsHuwEkZZgMYKCwpjknYICu4Sk5zW1RushvIuZEedQdfIr\n\
        PCbP/6XVBYU2Iwknj75pUInVwh4QsGvOpVCJ5abxcI0nlIQKWK2VevZQdYPl1Obd\n\
        13EoU1Z9w2DQVvsLD95GYyQ=\n\
        -----END PRIVATE KEY-----";

    const TEST_MODULUS_B64: &str = "t5xqbJOHFXWXjOKL9SmBDw5SEOAldR8Nmsvtn6b1xrQMbQvxwyz4ZeCyB6qCTBWCcpZrzeG39_p28pi8x70MQI663H9S1KY-SCzoA34BHfB93sr6YmGuh7e04hf2cqr0byh4Is4oFqK5KlN3zJHFm5QSB5MEQblvIc0zDdMmS1etk5RB9zpeOPtlS-OQH-ct5HdHt5yAPQbgSD9iepLPgOjCyvNo4EzNIR6CWpBxnUI5gHeZC1Xf68xV2fTt1e2mqNHvdbCxnyMJ5SQjjEJn_9FIpGJ_Q56cS0ZFM3GsTpC4wEzqIFAfShU9E-MrbCJb-NJ8RKstpKfnC8etnvxIAw";
    const TEST_EXPONENT_B64: &str = "AQAB";

    #[derive(Serialize)]
    struct TestClaims {
        iss: String,
        sub: String,
        exp: i64,
    }

    async fn test_store() -> (SharedStore, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let store = SharedStore::new(db_path.to_str().unwrap()).await.unwrap();
        (store, tmp)
    }

    async fn wait_for<F, Fut>(mut condition: F) -> anyhow::Result<()>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !condition().await {
            if tokio::time::Instant::now() > deadline {
                anyhow::bail!("timed out waiting for condition");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }

    fn sign_token(sub: &str, issuer: &str) -> String {
        let exp = std::time::SystemTime::now()
            .checked_add(Duration::from_secs(3600))
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let claims = TestClaims {
            iss: issuer.to_string(),
            sub: sub.to_string(),
            exp,
        };
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some("test-key".to_string());
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY_PEM.as_bytes()).unwrap();
        jsonwebtoken::encode(&header, &claims, &key).unwrap()
    }

    async fn test_auth() -> (RemoteJwksAuth, String) {
        let server = MockServer::start();
        let jwks = format!(
            "{{\"keys\":[{{\"kty\":\"RSA\",\"use\":\"sig\",\"kid\":\"test-key\",\"alg\":\"RS256\",\"n\":\"{}\",\"e\":\"{}\"}}]}}",
            TEST_MODULUS_B64, TEST_EXPONENT_B64
        );
        server.mock(|when, then| {
            when.method(GET).path("/jwks");
            then.status(200)
                .header("content-type", "application/json")
                .body(jwks);
        });
        let issuer = "https://test-issuer.example.com/";
        let auth = RemoteJwksAuth::for_url(&format!("{}/jwks", server.base_url()), issuer).unwrap();
        auth.initialize().await.unwrap();
        (auth, issuer.to_string())
    }

    async fn app_with_store(store: SharedStore) -> Router {
        let (auth, _) = test_auth().await;
        let state = AppState {
            store,
            decoder: Arc::new(auth.decoder()),
            obtain_tokens_fn: ok_obtain_fn(),
            mount_fn: ok_mount_fn(),
            unmount_fn: ok_unmount_fn(),
        };
        build_router(state)
    }

    async fn test_app() -> (Router, TempDir) {
        let (store, tmp) = test_store().await;
        (app_with_store(store).await, tmp)
    }

    // --- parse_did ---

    #[test]
    fn parse_did_splits_valid_did() {
        let did = parse_did("ska:ns/file.fits").unwrap();
        assert_eq!(did.namespace, "ska");
        assert_eq!(did.filename, "ns/file.fits");
    }

    #[test]
    fn parse_did_trims_whitespace() {
        let did = parse_did("  ska:ns/file.fits  ").unwrap();
        assert_eq!(did.namespace, "ska");
        assert_eq!(did.filename, "ns/file.fits");
    }

    #[test]
    fn parse_did_rejects_missing_colon() {
        assert!(parse_did("skansfile.fits").is_err());
    }

    #[test]
    fn parse_did_rejects_multiple_colons() {
        assert!(parse_did("ska:ns:file.fits").is_err());
    }

    // --- bearer_token ---

    #[test]
    fn bearer_token_extracts_token() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer test-token-123".parse().unwrap());
        assert_eq!(bearer_token(&headers), Some("test-token-123".to_string()));
    }

    #[test]
    fn bearer_token_requires_bearer_prefix() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Basic dXNlcjpwYXNz".parse().unwrap());
        assert_eq!(bearer_token(&headers), None);
    }

    #[test]
    fn bearer_token_missing_header() {
        let headers = HeaderMap::new();
        assert_eq!(bearer_token(&headers), None);
    }

    // --- record_error ---

    #[tokio::test]
    async fn record_error_inserts_failed_record() {
        let (store, _tmp) = test_store().await;
        let request_id = Uuid::new_v4();
        let record =
            record_error(&request_id, "user-1", "something went wrong".into(), &store).await;
        assert_eq!(record.state, RecordState::Failed);
        assert!(record
            .message
            .as_ref()
            .unwrap()
            .contains("something went wrong"));

        let stored = store.get(&request_id).await.unwrap().unwrap();
        assert_eq!(stored.status, RecordState::Failed);
        assert_eq!(stored.user_sub, "user-1");
    }

    // --- validate_dids ---

    #[tokio::test]
    async fn validate_dids_returns_parsed_dids() {
        let (store, _tmp) = test_store().await;
        let request = StageInRequest {
            dids: vec!["ns1:file1.fits".into(), "ns2:file2.fits".into()],
            project_name: "proj".into(),
        };
        let request_id = Uuid::new_v4();
        let dids = validate_dids(&request, &request_id, "user", &store)
            .await
            .unwrap();
        assert_eq!(dids.len(), 2);
        assert_eq!(dids[0].namespace, "ns1");
        assert_eq!(dids[1].filename, "file2.fits");
    }

    #[tokio::test]
    async fn validate_dids_returns_error_record_for_invalid_dids() {
        let (store, _tmp) = test_store().await;
        let request = StageInRequest {
            dids: vec!["invalid-did".into()],
            project_name: "proj".into(),
        };
        let request_id = Uuid::new_v4();
        let err = validate_dids(&request, &request_id, "user", &store)
            .await
            .unwrap_err();
        assert_eq!(err.state, RecordState::Failed);
        assert!(err.message.unwrap().contains("invalid-did"));
    }

    // --- mount_did ---

    fn ok_mount_fn() -> MountFn {
        Arc::new(|_, _, _, _, _| Ok(()))
    }

    fn fail_mount_fn() -> MountFn {
        Arc::new(|_, _, _, _, _| Err(anyhow::anyhow!("mount failed")))
    }

    #[tokio::test]
    async fn mount_did_records_mounted_did_on_success() {
        let (store, _tmp) = test_store().await;
        let request_id = Uuid::new_v4();
        store
            .initialise_request_record(
                &request_id,
                &"user".to_string(),
                None,
                Some(vec!["ns:file.fits".into()]),
                &RecordState::StagedIn,
            )
            .await
            .unwrap();

        let did = DidParse {
            namespace: "ns".into(),
            filename: "file.fits".into(),
        };
        mount_did(
            store.clone(),
            request_id,
            did,
            test_tokens(),
            "/base".into(),
            ok_mount_fn(),
        )
        .await;

        wait_for(|| async {
            let record = store.get(&request_id).await.unwrap().unwrap();
            record
                .dids_mounted
                .to_vec()
                .contains(&"ns:file.fits".to_string())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn mount_did_updates_status_to_failed_on_error() {
        let (store, _tmp) = test_store().await;
        let request_id = Uuid::new_v4();
        store
            .initialise_request_record(
                &request_id,
                &"user".to_string(),
                None,
                Some(vec!["ns:file.fits".into()]),
                &RecordState::StagedIn,
            )
            .await
            .unwrap();

        let did = DidParse {
            namespace: "ns".into(),
            filename: "file.fits".into(),
        };
        mount_did(
            store.clone(),
            request_id,
            did,
            test_tokens(),
            "/base".into(),
            fail_mount_fn(),
        )
        .await;

        wait_for(|| async {
            let record = store.get(&request_id).await.unwrap().unwrap();
            matches!(record.status, RecordState::Failed)
        })
        .await
        .unwrap();
    }

    // --- unmount_did ---

    fn ok_unmount_fn() -> UnmountFn {
        Arc::new(|_, _, _| Ok(()))
    }

    fn fail_unmount_fn() -> UnmountFn {
        Arc::new(|_, _, _| Err(anyhow::anyhow!("unmount failed")))
    }

    #[tokio::test]
    async fn unmount_did_removes_mounted_did_on_success() {
        let (store, _tmp) = test_store().await;
        let request_id = Uuid::new_v4();
        store
            .initialise_request_record(
                &request_id,
                &"user".to_string(),
                None,
                Some(vec!["ns:file.fits".into()]),
                &RecordState::StagedIn,
            )
            .await
            .unwrap();
        store
            .update_dids_mounted(&request_id, vec!["ns:file.fits".into()])
            .await
            .unwrap();

        let did = DidParse {
            namespace: "ns".into(),
            filename: "file.fits".into(),
        };
        unmount_did(
            store.clone(),
            request_id,
            did,
            "/base".into(),
            ok_unmount_fn(),
        )
        .await;

        wait_for(|| async {
            let record = store.get(&request_id).await.unwrap().unwrap();
            record.dids_mounted.to_vec().is_empty()
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn unmount_did_updates_status_to_failed_on_error() {
        let (store, _tmp) = test_store().await;
        let request_id = Uuid::new_v4();
        store
            .initialise_request_record(
                &request_id,
                &"user".to_string(),
                None,
                Some(vec!["ns:file.fits".into()]),
                &RecordState::StagedIn,
            )
            .await
            .unwrap();
        store
            .update_dids_mounted(&request_id, vec!["ns:file.fits".into()])
            .await
            .unwrap();

        let did = DidParse {
            namespace: "ns".into(),
            filename: "file.fits".into(),
        };
        unmount_did(
            store.clone(),
            request_id,
            did,
            "/base".into(),
            fail_unmount_fn(),
        )
        .await;

        wait_for(|| async {
            let record = store.get(&request_id).await.unwrap().unwrap();
            matches!(record.status, RecordState::Failed)
        })
        .await
        .unwrap();
    }

    // --- process_stage_in_inner ---

    fn test_tokens() -> Tokens {
        Tokens {
            data_management_token: "dm".into(),
            site_capabilities_token: "sc".into(),
        }
    }

    fn ok_obtain_fn() -> ObtainTokensFn {
        let tokens = test_tokens();
        Arc::new(move |_raw_token| Box::pin(std::future::ready(Ok(tokens.clone()))))
    }

    fn fail_obtain_fn() -> ObtainTokensFn {
        Arc::new(|_raw_token| {
            Box::pin(std::future::ready(Err(anyhow::anyhow!(
                "token exchange failed"
            ))))
        })
    }

    #[tokio::test]
    async fn process_stage_in_inner_returns_accepted_and_records_request() {
        let (store, _tmp) = test_store().await;
        let claims = JwtClaims {
            sub: "user".into(),
            exp: None,
        };
        let request = StageInRequest {
            dids: vec!["ns:file.fits".into()],
            project_name: "proj".into(),
        };
        let request_id = Uuid::new_v4();
        let dids = vec![DidParse {
            namespace: "ns".into(),
            filename: "file.fits".into(),
        }];

        let (status, response) = process_stage_in_inner(
            &store,
            &claims,
            &request,
            request_id,
            dids,
            test_tokens(),
            ok_mount_fn(),
        )
        .await;

        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(response.request_id, request_id);
        assert_eq!(response.state, RecordState::StagingIn);

        let stored = store.get(&request_id).await.unwrap().unwrap();
        assert_eq!(stored.status, RecordState::StagingIn);
        assert_eq!(stored.dids_requested.to_vec(), vec!["ns:file.fits"]);
    }

    #[tokio::test]
    async fn process_stage_in_returns_bad_request_for_invalid_did() {
        let (store, _tmp) = test_store().await;
        let claims = JwtClaims {
            sub: "user".into(),
            exp: None,
        };
        let request = StageInRequest {
            dids: vec!["invalid".into()],
            project_name: "proj".into(),
        };
        let (status, response) = process_stage_in(
            &store,
            &claims,
            "token",
            &request,
            ok_mount_fn(),
            ok_obtain_fn(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response.state, RecordState::Failed);
    }

    #[tokio::test]
    async fn process_stage_in_returns_unauthorized_when_token_exchange_fails() {
        let (store, _tmp) = test_store().await;
        let claims = JwtClaims {
            sub: "user".into(),
            exp: None,
        };
        let request = StageInRequest {
            dids: vec!["ns:file.fits".into()],
            project_name: "proj".into(),
        };
        let (status, response) = process_stage_in(
            &store,
            &claims,
            "token",
            &request,
            ok_mount_fn(),
            fail_obtain_fn(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(response.state, RecordState::Failed);
    }

    // --- process_stage_out ---

    #[tokio::test]
    async fn process_stage_out_spawns_unmount_and_returns_staging_out() {
        let (store, _tmp) = test_store().await;
        let request_id = Uuid::new_v4();
        let user_sub = "user".to_string();
        let record = StageInRecord {
            request_id,
            state: RecordState::StagedIn,
            input_path: Some("/home/user/data".into()),
            output_path: None,
            work_path: None,
            dids: SqlxJson(vec!["ns:file.fits".into()]),
            message: None,
        };
        store
            .initialise_request_record(
                &request_id,
                &user_sub,
                Some(record),
                None,
                &RecordState::StagedIn,
            )
            .await
            .unwrap();
        store
            .update_dids_mounted(&request_id, vec!["ns:file.fits".into()])
            .await
            .unwrap();

        let claim = JwtClaims {
            sub: user_sub,
            exp: None,
        };
        let request = StageOutRequest { request_id };
        let response = process_stage_out(&store, claim, &request, ok_unmount_fn()).await;

        assert_eq!(response.request_id, request_id);
        assert_eq!(response.state, RecordState::StagingOut);

        wait_for(|| async {
            let record = store.get(&request_id).await.unwrap().unwrap();
            record.dids_mounted.to_vec().is_empty()
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn process_stage_out_returns_unknown_for_missing_request() {
        let (store, _tmp) = test_store().await;
        let request_id = Uuid::new_v4();
        let claim = JwtClaims {
            sub: "user".into(),
            exp: None,
        };
        let request = StageOutRequest { request_id };
        let response = process_stage_out(&store, claim, &request, ok_unmount_fn()).await;
        assert_eq!(response.state, RecordState::Unknown);
    }

    // --- route handlers ---

    #[tokio::test]
    async fn stage_in_returns_unauthorized_without_auth_header() {
        let (app, _tmp) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stage-in")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        json!({"dids": ["ns:file.fits"], "project_name": "proj"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn stage_in_accepts_valid_token_and_returns_accepted() {
        let (app, _tmp) = test_app().await;
        let (auth, issuer) = test_auth().await;
        let token = sign_token("user", &issuer);

        // Verify the token validates against the same decoder used by the handler.
        let auth_result = auth.authenticate(&format!("Bearer {}", token)).await;
        assert!(
            auth_result.is_ok(),
            "token failed authentication: {:?}",
            auth_result
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stage-in")
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        json!({"dids": ["ns:file.fits"], "project_name": "proj"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn stage_in_returns_bad_request_for_invalid_did() {
        let (app, _tmp) = test_app().await;
        let (_auth, issuer) = test_auth().await;
        let token = sign_token("user", &issuer);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stage-in")
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        json!({"dids": ["not-a-did"], "project_name": "proj"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_stage_in_status_returns_record() {
        let (store, _tmp) = test_store().await;
        let request_id = Uuid::new_v4();
        let record = StageInRecord {
            request_id,
            state: RecordState::StagedIn,
            input_path: None,
            output_path: None,
            work_path: None,
            dids: SqlxJson(vec![]),
            message: None,
        };
        store
            .initialise_request_record(
                &request_id,
                &"user".to_string(),
                Some(record),
                None,
                &RecordState::StagedIn,
            )
            .await
            .unwrap();

        let app = app_with_store(store).await;
        let (_auth, issuer) = test_auth().await;
        let token = sign_token("user", &issuer);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/stage-in/{}", request_id))
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn get_stage_in_status_returns_not_found_for_missing_record() {
        let (app, _tmp) = test_app().await;
        let (_auth, issuer) = test_auth().await;
        let token = sign_token("user", &issuer);
        let request_id = Uuid::new_v4();

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/stage-in/{}", request_id))
                    .header("Authorization", format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn stage_out_accepts_valid_token_and_returns_ok() {
        let (store, _tmp) = test_store().await;
        let request_id = Uuid::new_v4();
        store
            .initialise_request_record(
                &request_id,
                &"user".to_string(),
                None,
                Some(vec!["ns:file.fits".into()]),
                &RecordState::StagedIn,
            )
            .await
            .unwrap();
        store
            .update_dids_mounted(&request_id, vec!["ns:file.fits".into()])
            .await
            .unwrap();

        let app = app_with_store(store).await;
        let (_auth, issuer) = test_auth().await;
        let token = sign_token("user", &issuer);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stage-out")
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(json!({"request_id": request_id}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
