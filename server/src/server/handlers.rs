//! `/api` handlers.

use std::time::Instant;

use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tracing::debug;

use super::bench::{self, BenchReport};
use super::error::ApiError;
use super::response::{DataResponse, Timings, X_CACHE};
use super::{AppState, LoadedRows};
use crate::config::ServerInfo;
use crate::db::{self, TableStats};
use crate::encode::{self, Format, IpcCompression, ParquetCompression};
use crate::query::{BenchRequest, DataRequest, Params};
use crate::schema::{COLUMNS, Column, Group, TABLE};

#[derive(Serialize)]
pub struct Health {
    status: &'static str,
}

/// Liveness only: never touches the database.
pub async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

/// Empty response for round-trip time measurements.
pub async fn ping() -> StatusCode {
    StatusCode::NO_CONTENT
}

#[derive(Serialize)]
pub struct Info {
    table: &'static str,
    row_estimate: i64,
    max_id: i64,
    table_bytes: i64,
    max_rows: u64,
    columns: Vec<ColumnInfo>,
    formats: &'static [Format],
    ipc_compression: &'static [IpcCompression],
    parquet_compression: &'static [ParquetCompression],
    server: ServerInfo,
}

#[derive(Serialize)]
struct ColumnInfo {
    name: &'static str,
    pg_type: &'static str,
    arrow_type: &'static str,
    nullable: bool,
    dictionary: bool,
    group: Group,
}

impl From<&Column> for ColumnInfo {
    fn from(column: &Column) -> Self {
        Self {
            name: column.name,
            pg_type: column.ty.pg_name(),
            arrow_type: column.ty.arrow_name(),
            nullable: column.nullable,
            dictionary: column.dictionary,
            group: column.group,
        }
    }
}

pub async fn info(State(state): State<AppState>) -> Result<Json<Info>, ApiError> {
    let client = state.pool().get().await?;
    let TableStats {
        row_estimate,
        max_id,
        table_bytes,
    } = db::table_stats(&client).await?;
    Ok(Json(Info {
        table: TABLE,
        row_estimate,
        max_id,
        table_bytes,
        max_rows: state.max_rows(),
        columns: COLUMNS.iter().map(ColumnInfo::from).collect(),
        formats: Format::ALL,
        ipc_compression: IpcCompression::ALL,
        parquet_compression: ParquetCompression::ALL,
        server: state.server_info().clone(),
    }))
}

pub async fn data(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
) -> Result<DataResponse, ApiError> {
    let started = Instant::now();
    let request = DataRequest::parse(
        &Params::parse(query.as_deref().unwrap_or_default()),
        state.max_rows(),
    )?;
    let after_id = request.rows.after_id;
    let LoadedRows {
        rows: fetched,
        db_time,
        cache,
    } = state.load_rows(request.rows, request.cache).await?;
    let (row_count, column_count) = (fetched.rows.len(), fetched.columns.len());
    let last_id = fetched.last_id.unwrap_or(after_id);

    let encode_started = Instant::now();
    let encoding = request.encoding;
    // Moving the rows in also frees them (up to a million allocations) off
    // the async workers.
    let payload = tokio::task::spawn_blocking(move || {
        encode::encode(&fetched.rows, fetched.columns.columns(), &encoding)
    })
    .await??;
    let timings = Timings {
        db: db_time,
        encode: encode_started.elapsed(),
        total: started.elapsed(),
    };

    debug!(
        format = %encoding.format(),
        rows = row_count,
        bytes = payload.len(),
        db_ms = timings.db.as_secs_f64() * 1e3,
        encode_ms = timings.encode.as_secs_f64() * 1e3,
        "encoded data response"
    );
    Ok(DataResponse {
        payload,
        format: encoding.format(),
        rows: row_count,
        columns: column_count,
        last_id,
        cache,
        timings,
        http_compression: request.http_compression,
    })
}

pub async fn bench(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let started = Instant::now();
    let request = BenchRequest::parse(
        &Params::parse(query.as_deref().unwrap_or_default()),
        state.max_rows(),
    )?;
    let LoadedRows {
        rows: fetched,
        db_time,
        cache,
    } = state.load_rows(request.rows, request.cache).await?;
    let (row_count, column_count) = (fetched.rows.len(), fetched.columns.len());

    let encode_started = Instant::now();
    let results = tokio::task::spawn_blocking(move || bench::run(&fetched)).await??;
    let timings = Timings {
        db: db_time,
        encode: encode_started.elapsed(),
        total: started.elapsed(),
    };

    let mut headers = HeaderMap::new();
    timings.write_headers(&mut headers, "bench encode");
    headers.insert(X_CACHE, HeaderValue::from_static(cache.as_str()));
    let report = BenchReport {
        rows: row_count,
        columns: column_count,
        db_ms: bench::millis_f64(db_time),
        results,
    };
    Ok((headers, Json(report)).into_response())
}

pub async fn not_found() -> ApiError {
    ApiError::NotFound
}

pub async fn method_not_allowed() -> ApiError {
    ApiError::MethodNotAllowed
}
