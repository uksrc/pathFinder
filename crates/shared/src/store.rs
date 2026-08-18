/// Module containing the functions (etc.) required to use an on-disk SQLite store
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

#[derive(Debug, Clone, Serialize, sqlx::Type, Deserialize)]
#[sqlx(type_name = "TEXT")]
pub enum RecordState {
    Started,
    Finished,
    Failed,
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
    pub output_path: Option<String>,
    pub work_path: Option<String>,
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
    pub dids_mounted: Json<Option<Vec<String>>>,
    pub dids_unmounted: Json<Option<Vec<String>>>,
    pub status: RecordState,
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
            message: None,
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
            dids_mounted: Json(None),
            dids_unmounted: Json(None),
            status: record.state.clone(),
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
            message: None,
        });

        let row = RequestStoreRow::from(&record);
        sqlx::query(
            r#"INSERT INTO request_store
                (request_id, user_sub, input_path, output_path, work_path, dids_mounted, dids_unmounted, status, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(request_id) DO UPDATE SET
                user_sub = excluded.user_sub,
                input_path = excluded.input_path,
                output_path = excluded.output_path,
                work_path = excluded.work_path,
                dids_mounted = excluded.dids_mounted,
                dids_unmounted = excluded.dids_unmounted,
                status = excluded.status,
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
        .bind(row.created_at)
        .bind(row.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
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
            "SELECT request_id, input_path, output_path, work_path, dids_mounted, dids_unmounted, status, created_at, updated_at FROM request_store ORDER BY created_at DESC LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(entries)
    }
}
