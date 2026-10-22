//! `/api` handlers.

use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::header::ACCEPT_ENCODING;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::future::try_join_all;
use serde::Serialize;
use tracing::debug;

use super::bench::{self, BenchReport};
use super::compression::{HttpEncoding, MIN_COMPRESSED_BYTES};
use super::error::ApiError;
use super::response::{DataResponse, Timings, X_CACHE, X_DATASET};
use super::{AppState, LoadedRows, drop_in_background};
use crate::config::ServerInfo;
use crate::db::{self, FetchedRows, SeedStatus};
use crate::ddl;
use crate::encode::{
    self, CancelFlag, EncodeError, Encoding, Format, IpcCompression, ParquetCompression,
};
use crate::pg_time::{RFC3339_LEN, Timestamp};
use crate::query::{BenchRequest, DataRequest, Params};
use crate::schema::{Column, DATASETS, DEFAULT_DATASET, Dataset, Group, RelationKind};

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
    default_dataset: &'static str,
    datasets: Vec<DatasetInfo>,
    seed: Option<SeedInfo>,
    max_rows: u64,
    formats: &'static [Format],
    ipc_compression: &'static [IpcCompression],
    parquet_compression: &'static [ParquetCompression],
    http_compression: &'static [HttpEncoding],
    server: ServerInfo,
}

#[derive(Serialize)]
struct DatasetInfo {
    name: &'static str,
    label: &'static str,
    kind: RelationKind,
    relation: &'static str,
    key_column: &'static str,
    description: &'static str,
    sql: String,
    row_estimate: Option<i64>,
    max_key: Option<i64>,
    bytes: Option<i64>,
    columns: Vec<ColumnInfo>,
}

impl DatasetInfo {
    fn new(dataset: &'static Dataset, stats: db::DatasetStats) -> Self {
        Self {
            name: dataset.name,
            label: dataset.label,
            kind: dataset.kind,
            relation: dataset.relation,
            key_column: dataset.key,
            description: dataset.description,
            sql: ddl::defining_sql(dataset),
            row_estimate: stats.row_estimate,
            max_key: stats.max_key,
            bytes: stats.bytes,
            columns: dataset.columns.iter().map(ColumnInfo::from).collect(),
        }
    }
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

/// The `seed_status` row, so the UI can warn while a seed is incomplete.
#[derive(Serialize)]
struct SeedInfo {
    state: String,
    schema_version: i32,
    orders: i64,
    order_items: i64,
    finished_at: Option<String>,
}

impl From<SeedStatus> for SeedInfo {
    fn from(status: SeedStatus) -> Self {
        Self {
            state: status.state,
            schema_version: status.schema_version,
            orders: status.orders,
            order_items: status.order_items,
            finished_at: status.finished_at.and_then(rfc3339),
        }
    }
}

fn rfc3339(timestamp: Timestamp) -> Option<String> {
    let mut buf = [0; RFC3339_LEN];
    timestamp.write_rfc3339(&mut buf).ok().map(str::to_owned)
}

/// Catalog statistics and index lookups only (pipelined on one connection);
/// never a dataset's own query, so it stays cheap even for the live view.
pub async fn info(State(state): State<AppState>) -> Result<Json<Info>, ApiError> {
    let client = state.pool().get().await?;
    let stats = DATASETS
        .iter()
        .map(|dataset| db::dataset_stats(&client, dataset));
    let (stats, seed) = tokio::try_join!(try_join_all(stats), db::seed_status(&client))?;
    Ok(Json(Info {
        default_dataset: DEFAULT_DATASET.name,
        datasets: DATASETS
            .iter()
            .zip(stats)
            .map(|(dataset, stats)| DatasetInfo::new(dataset, stats))
            .collect(),
        seed: seed.map(SeedInfo::from),
        max_rows: state.max_rows(),
        formats: Format::ALL,
        ipc_compression: IpcCompression::ALL,
        parquet_compression: ParquetCompression::ALL,
        http_compression: HttpEncoding::ALL,
        server: state.server_info().clone(),
    }))
}

/// A data response body with the time it took to make.
struct EncodedBody {
    body: Vec<u8>,
    payload_bytes: usize,
    content_encoding: Option<HttpEncoding>,
    encode: Duration,
    compress: Duration,
}

/// Encodes `fetched`, then compresses the payload with `http_encoding` if it
/// is worth it. Blocking: call it off the runtime. Each timer stops before
/// anything is freed.
fn encode_body(
    fetched: &FetchedRows,
    encoding: &Encoding,
    http_encoding: Option<HttpEncoding>,
    cancel: &CancelFlag,
) -> Result<EncodedBody, EncodeError> {
    let started = Instant::now();
    let payload = encode::encode(&fetched.rows, fetched.columns.columns(), encoding, cancel)?;
    let encode_time = started.elapsed();
    let payload_bytes = payload.len();
    let Some(http_encoding) = http_encoding.filter(|_| payload_bytes >= MIN_COMPRESSED_BYTES)
    else {
        return Ok(EncodedBody {
            body: payload,
            payload_bytes,
            content_encoding: None,
            encode: encode_time,
            compress: Duration::ZERO,
        });
    };
    let started = Instant::now();
    let body = http_encoding.compress(&payload, cancel)?;
    let compress = started.elapsed();
    drop_in_background(vec![payload]);
    Ok(EncodedBody {
        body,
        payload_bytes,
        content_encoding: Some(http_encoding),
        encode: encode_time,
        compress,
    })
}

pub async fn data(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Result<DataResponse, ApiError> {
    let started = Instant::now();
    let request = DataRequest::parse(
        &Params::parse(query.as_deref().unwrap_or_default()),
        state.max_rows(),
    )?;
    let encoding = request.encoding;
    // Bodies already compressed with a strong codec go out as they are.
    let http_encoding = (request.http_compression && !encoding.is_strongly_compressed())
        .then(|| {
            HttpEncoding::negotiate(
                headers
                    .get_all(ACCEPT_ENCODING)
                    .iter()
                    .filter_map(|value| value.to_str().ok()),
            )
        })
        .flatten();
    let dataset = request.rows.dataset.name;
    let after_key = request.rows.after_key;
    // hyper drops this future when the client goes away; the guard then
    // stops the encoder at its next check.
    let cancel = CancelFlag::default();
    let _cancel_on_drop = cancel.cancel_on_drop();

    let LoadedRows {
        rows: fetched,
        db_time,
        cache,
    } = state.load_rows(request.rows, request.cache).await?;
    let (row_count, column_count) = (fetched.rows.len(), fetched.columns.len());
    let last_key = fetched.last_key.unwrap_or(after_key);

    let encoded = tokio::task::spawn_blocking(move || {
        let encoded = encode_body(&fetched, &encoding, http_encoding, &cancel);
        drop_in_background(vec![fetched]);
        encoded
    })
    .await??;
    let timings = Timings {
        db: db_time,
        encode: encoded.encode,
        compress: encoded.compress,
        total: started.elapsed(),
    };

    debug!(
        dataset,
        format = %encoding.format(),
        rows = row_count,
        bytes = encoded.payload_bytes,
        wire_bytes = encoded.body.len(),
        db_ms = timings.db.as_secs_f64() * 1e3,
        encode_ms = timings.encode.as_secs_f64() * 1e3,
        compress_ms = timings.compress.as_secs_f64() * 1e3,
        "encoded data response"
    );
    Ok(DataResponse {
        body: encoded.body,
        payload_bytes: encoded.payload_bytes,
        content_encoding: encoded.content_encoding,
        format: encoding.format(),
        dataset,
        rows: row_count,
        columns: column_count,
        last_key,
        cache,
        timings,
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
    let dataset = request.rows.dataset.name;
    let cancel = CancelFlag::default();
    let _cancel_on_drop = cancel.cancel_on_drop();
    // Taken before the rows are fetched, so abandoned benchmarks cannot pile
    // up their rows either; released when the blocking work ends.
    let permit = state.bench_permit().await;

    let LoadedRows {
        rows: fetched,
        db_time,
        cache,
    } = state.load_rows(request.rows, request.cache).await?;
    let (row_count, column_count) = (fetched.rows.len(), fetched.columns.len());

    let encode_started = Instant::now();
    let results = tokio::task::spawn_blocking(move || {
        let results = bench::run(&fetched, &cancel);
        drop_in_background(vec![fetched]);
        drop(permit);
        results
    })
    .await??;
    let timings = Timings {
        db: db_time,
        encode: encode_started.elapsed(),
        compress: Duration::ZERO,
        total: started.elapsed(),
    };

    let mut headers = HeaderMap::new();
    timings.write_headers(&mut headers, "bench encode", None);
    headers.insert(X_CACHE, HeaderValue::from_static(cache.as_str()));
    headers.insert(X_DATASET, HeaderValue::from_static(dataset));
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
