/// Module containing the functions (etc.) required to use an on-disk SQLite store
use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::{migrate, types::Json, FromRow};
use std::{str::FromStr, time::Duration};
use uuid::Uuid;

async fn create_pool(db_path: &str) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(10))
        .pragma("foreign_keys", "ON")
        .optimize_on_close(true, Some(1000));

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    Ok(pool)
}

#[derive(Debug, Clone, Serialize, sqlx::Type, Deserialize, PartialEq, Eq)]
#[sqlx(type_name = "TEXT")]
pub enum RecordState {
    StagingIn,
    StagingOut,
    StagedIn,
    StagedOut,
    Failed,
    Unknown,
}

impl std::fmt::Display for RecordState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageInRecord {
    pub request_id: Uuid,
    pub state: RecordState,
    pub input_path: Option<String>,
    pub output_path: Option<String>, // TODO: Consider project path?  It could have input data after all!
    pub work_path: Option<String>,
    pub dids: Json<Vec<String>>,
    pub message: Option<String>,
}

#[derive(FromRow, Debug)]
pub struct RequestStoreRow {
    #[sqlx(try_from = "String")]
    pub request_id: Uuid,
    pub user_sub: String,
    pub input_path: Option<String>,
    pub output_path: Option<String>,
    pub work_path: Option<String>,
    pub dids_mounted: Json<Vec<String>>,
    pub dids_requested: Json<Vec<String>>,
    pub status: RecordState,
    pub message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl RequestStoreRow {
    pub fn to_stage_in_record(&self) -> StageInRecord {
        StageInRecord {
            request_id: self.request_id,
            state: self.status.clone(),
            input_path: self.input_path.clone(),
            output_path: self.output_path.clone(),
            work_path: self.work_path.clone(),
            dids: self.dids_requested.clone(),
            message: self.message.clone(),
        }
    }
}

impl From<&StageInRecord> for RequestStoreRow {
    fn from(record: &StageInRecord) -> Self {
        let now_utc = chrono::Utc::now();
        RequestStoreRow {
            request_id: record.request_id,
            user_sub: String::new(),
            input_path: record.input_path.clone(),
            output_path: record.output_path.clone(),
            work_path: record.work_path.clone(),
            dids_mounted: Json(Vec::<String>::default()),
            dids_requested: Json(record.dids.to_vec()),
            status: record.state.clone(),
            message: record.message.clone(),
            created_at: now_utc,
            updated_at: now_utc,
        }
    }
}

#[derive(Clone)]
pub struct SharedStore {
    pool: SqlitePool,
}

impl SharedStore {
    pub async fn new(db_path: &str) -> anyhow::Result<Self> {
        tracing::info!("Using SQLite DB: {}", db_path);
        let pool = create_pool(db_path).await?;
        migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn get(&self, request_id: &Uuid) -> anyhow::Result<Option<RequestStoreRow>> {
        let entry = sqlx::query_as::<_, RequestStoreRow>(
            r#"SELECT *
               FROM request_store
               WHERE request_id = ?"#,
        )
        .bind(request_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        Ok(entry)
    }

    pub async fn get_for_user(
        &self,
        request_id: &Uuid,
        user_sub: &String,
    ) -> anyhow::Result<Option<RequestStoreRow>> {
        let entry = sqlx::query_as::<_, RequestStoreRow>(
            r#"SELECT *
               FROM request_store
               WHERE request_id = ?
               AND user_sub = ?"#,
        )
        .bind(request_id.to_string())
        .bind(user_sub)
        .fetch_optional(&self.pool)
        .await?;
        Ok(entry)
    }

    pub async fn exists(&self, request_id: &Uuid) -> anyhow::Result<bool> {
        let row: (bool,) =
            sqlx::query_as("SELECT EXISTS(SELECT 1 FROM request_store WHERE request_id = ?)")
                .bind(request_id.to_string())
                .fetch_one(&self.pool)
                .await?;
        Ok(row.0)
    }

    pub async fn initialise_request_record(
        &self,
        request_id: &Uuid,
        user_sub: &String,
        record: Option<StageInRecord>,
        dids: Option<Vec<String>>,
        status: &RecordState,
    ) -> anyhow::Result<()> {
        let dids = dids.unwrap_or_default();
        let record = record.unwrap_or(StageInRecord {
            request_id: *request_id,
            state: status.clone(),
            input_path: None,
            output_path: None,
            work_path: None,
            dids: Json(dids.clone()),
            message: None,
        });

        let row = RequestStoreRow::from(&record);
        sqlx::query(
            r#"INSERT INTO request_store
                (request_id, user_sub, input_path, output_path, work_path, dids_mounted, dids_requested, status, message, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(request_id) DO UPDATE SET
                user_sub = excluded.user_sub,
                input_path = excluded.input_path,
                output_path = excluded.output_path,
                work_path = excluded.work_path,
                dids_mounted = excluded.dids_mounted,
                dids_requested = excluded.dids_requested,
                status = excluded.status,
                message = excluded.message,
                updated_at = excluded.updated_at"#,
        )
        .bind(row.request_id.to_string())
        .bind(user_sub)
        .bind(row.input_path)
        .bind(row.output_path)
        .bind(row.work_path)
        .bind(Json::<Vec<String>>(vec![]))
        .bind(Json(dids))
        .bind(row.status.to_string())
        .bind(row.message)
        .bind(row.created_at)
        .bind(row.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn touch_updated_at(&self, request_id: &Uuid) -> anyhow::Result<()> {
        let now = chrono::Utc::now();
        sqlx::query("UPDATE request_store SET updated_at = ? WHERE request_id = ?")
            .bind(now)
            .bind(request_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_dids_mounted(
        &self,
        request_id: &Uuid,
        dids: Vec<String>,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE request_store SET dids_mounted = ? WHERE request_id = ?")
            .bind(Json(dids))
            .bind(request_id.to_string())
            .execute(&self.pool)
            .await?;
        self.touch_updated_at(request_id).await
    }

    /// Append a single DID to the mounted list for a given request.
    pub async fn add_did_mounted(&self, request_id: &Uuid, did: &str) -> anyhow::Result<()> {
        // Read current list, append, write back
        if let Some(row) = self.get(request_id).await? {
            let mut mounted: Vec<String> = row.dids_mounted.to_vec();
            mounted.push(did.to_string());
            self.update_dids_mounted(request_id, mounted).await?;
        }
        Ok(())
    }

    /// Remove a single DID from the mounted list for a given request.
    pub async fn remove_did_mounted(&self, request_id: &Uuid, did: &str) -> anyhow::Result<()> {
        if let Some(row) = self.get(request_id).await? {
            let mut mounted: Vec<String> = row.dids_mounted.to_vec();
            mounted.retain(|m| m != did);
            self.update_dids_mounted(request_id, mounted).await?;
        }
        Ok(())
    }

    pub async fn update_dids_requested(
        &self,
        request_id: &Uuid,
        dids: Vec<String>,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE request_store SET dids_requested = ? WHERE request_id = ?")
            .bind(Json(dids))
            .bind(request_id.to_string())
            .execute(&self.pool)
            .await?;
        self.touch_updated_at(request_id).await
    }

    pub async fn update_status(
        &self,
        request_id: &Uuid,
        status: &RecordState,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE request_store SET status = ? WHERE request_id = ?")
            .bind(status.to_string())
            .bind(request_id.to_string())
            .execute(&self.pool)
            .await?;
        self.touch_updated_at(request_id).await
    }

    pub async fn update_message(&self, request_id: &Uuid, message: String) -> anyhow::Result<()> {
        sqlx::query("UPDATE request_store SET message = ? WHERE request_id = ?")
            .bind(message)
            .bind(request_id.to_string())
            .execute(&self.pool)
            .await?;
        self.touch_updated_at(request_id).await
    }

    pub async fn add_to_message(
        &self,
        request_id: &Uuid,
        new_message: String,
    ) -> anyhow::Result<()> {
        let record = self
            .get(request_id)
            .await
            .context("cannot find record to add message")?
            .ok_or_else(|| anyhow!("error querying store for record"))?;

        let new_message = format!("{}/{}", record.message.unwrap_or_default(), new_message);
        self.update_message(request_id, new_message).await
    }

    pub async fn fail_stale_requests(&self) -> anyhow::Result<u64> {
        let now = chrono::Utc::now();
        let result = sqlx::query(
            "UPDATE request_store SET status = ?, message = ?, updated_at = ? WHERE status = ? OR status = ?",
        )
        .bind(RecordState::Failed.to_string())
        .bind("pathfinder service restart invalidated request")
        .bind(now)
        .bind(RecordState::StagingIn.to_string())
        .bind(RecordState::StagingOut.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn delete(&self, request_id: &Uuid) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM request_store WHERE request_id = ?")
            .bind(request_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list(&self, limit: i64, offset: i64) -> anyhow::Result<Vec<RequestStoreRow>> {
        let entries = sqlx::query_as::<_, RequestStoreRow>(
            "SELECT request_id, user_sub, input_path, output_path, work_path, dids_mounted, dids_requested, status, message, created_at, updated_at FROM request_store ORDER BY created_at DESC LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(entries)
    }
}
