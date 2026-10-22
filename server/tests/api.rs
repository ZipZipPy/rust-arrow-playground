//! End-to-end tests of the HTTP API against a seeded database, driving the
//! axum router in-process. Needs `TEST_DATABASE_URL`.

mod common;

use std::io::{Cursor, Read};
use std::path::PathBuf;

use arrow::datatypes::DataType;
use arrow::ipc::MessageHeader;
use arrow::ipc::reader::{FileReader, StreamReader};
use arrow::record_batch::RecordBatch;
use axum::Router;
use axum::body::{Body, HttpBody};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use bytes::Bytes;
use http_body_util::BodyExt;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use playground::config::{ExportConfig, ServerInfo};
use playground::encode::{self, ArrowContainer, Encoding, Format, IpcCompression};
use playground::query::RowsQuery;
use playground::schema::{COLUMNS, ColumnSelection};
use playground::server::{self, AppState, Settings};
use playground::{db, export, seed};
use serde_json::Value;
use tokio::sync::MutexGuard;
use tower::ServiceExt;

use common::{DB_LOCK, assert_batches_match_json, ipc_message_kinds};

const SCHEMA: &str = "playground_api_tests";
const ROWS: usize = 3000;
const MAX_ROWS: u64 = 5000;
/// Three record batches / row groups: 1024 + 1024 + 952 rows.
const BATCH: usize = 1024;
const DICT_COLUMNS: usize = 5;

struct TestApp {
    router: Router,
    database: tokio_postgres::Config,
    _lock: MutexGuard<'static, bool>,
}

struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    /// Body size known before reading, i.e. what becomes `Content-Length`.
    exact_size: Option<u64>,
    body: Bytes,
}

impl TestResponse {
    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .unwrap_or_else(|| panic!("missing header {name}"))
            .to_str()
            .unwrap()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("JSON body")
    }

    fn rows(&self) -> Vec<Value> {
        match self.json() {
            Value::Array(rows) => rows,
            other => panic!("expected a JSON array, got {other}"),
        }
    }
}

/// The router over a seeded `orders` table, holding the lock that
/// serializes this binary's tests. `None` without `TEST_DATABASE_URL`.
async fn app() -> Option<TestApp> {
    // Locked first: concurrent CREATE SCHEMA IF NOT EXISTS can still collide.
    let mut seeded = DB_LOCK.lock().await;
    let database = common::database(SCHEMA).await?;
    if !*seeded {
        seed::run(&common::seed_config(&database, ROWS as u64, 3))
            .await
            .expect("seed the test table");
        *seeded = true;
    }
    Some(TestApp {
        router: router_with_static_dir(&database, &static_dir()),
        database,
        _lock: seeded,
    })
}

fn router_with_static_dir(
    database: &tokio_postgres::Config,
    static_dir: &std::path::Path,
) -> Router {
    let pool = db::create_pool(database.clone(), 4).unwrap();
    let state = AppState::new(
        pool,
        Settings {
            max_rows: MAX_ROWS,
            cache_entries: 4,
            cache_max_rows: 10_000,
            server_info: ServerInfo::from_env(),
        },
    );
    server::router(state, static_dir)
}

fn static_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("playground-api-tests-static");
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html><title>spa</title>").unwrap();
    std::fs::write(dir.join("assets/app.js"), "console.log('app')").unwrap();
    dir
}

impl TestApp {
    async fn get(&self, uri: &str) -> TestResponse {
        self.request(Method::GET, uri, &[]).await
    }

    async fn request(&self, method: Method, uri: &str, headers: &[(&str, &str)]) -> TestResponse {
        send(&self.router, method, uri, headers).await
    }

    /// The JSON rows for `query` (no HTTP compression).
    async fn json_rows(&self, query: &str) -> Vec<Value> {
        let response = self
            .get(&format!(
                "/api/data?format=json&http_compression=false&{query}"
            ))
            .await;
        assert_eq!(response.status, StatusCode::OK);
        response.rows()
    }

    async fn arrow(&self, query: &str) -> (TestResponse, Vec<RecordBatch>) {
        let response = self
            .get(&format!(
                "/api/data?format=arrow&http_compression=false&{query}"
            ))
            .await;
        assert_eq!(
            response.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&response.body)
        );
        assert_eq!(
            response.header("content-type"),
            "application/vnd.apache.arrow.stream"
        );
        let reader = StreamReader::try_new(Cursor::new(response.body.clone()), None).unwrap();
        let batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
        (response, batches)
    }
}

async fn send(
    router: &Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
) -> TestResponse {
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    let exact_size = body.size_hint().exact();
    let body = body.collect().await.unwrap().to_bytes();
    TestResponse {
        status: parts.status,
        headers: parts.headers,
        exact_size,
        body,
    }
}

#[tokio::test]
async fn json_matches_the_database() {
    let Some(app) = app().await else { return };
    let rows = app.json_rows(&format!("rows={ROWS}")).await;
    assert_eq!(rows.len(), ROWS);
    for (row, id) in rows.iter().zip(1..) {
        assert_eq!(row["id"], id, "keyset order");
        assert_eq!(row.as_object().unwrap().len(), COLUMNS.len());
    }
    for nullable in ["ship_date", "rating", "notes"] {
        assert!(
            rows.iter().any(|row| row[nullable].is_null()),
            "{nullable} has nulls"
        );
        assert!(
            rows.iter().any(|row| !row[nullable].is_null()),
            "{nullable} has values"
        );
    }

    // Timestamps, dates and REALs decoded by the server's own wire readers
    // must agree with tokio-postgres' chrono conversions.
    let client = db::connect(&app.database).await.unwrap();
    let expected = client
        .query(
            "SELECT created_at, ship_date, discount FROM orders ORDER BY id",
            &[],
        )
        .await
        .unwrap();
    for (row, db_row) in rows.iter().zip(&expected) {
        let created_at: chrono::DateTime<chrono::Utc> = db_row.get(0);
        assert_eq!(
            row["created_at"],
            created_at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
        );
        let ship_date: Option<chrono::NaiveDate> = db_row.get(1);
        assert_eq!(
            row["ship_date"].as_str(),
            ship_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .as_deref()
        );
        let discount: f32 = db_row.get(2);
        assert_eq!(row["discount"].as_f64().unwrap() as f32, discount);
        assert!(row["created_at"].as_str().unwrap().ends_with('Z'));
    }
}

#[tokio::test]
async fn arrow_streams_match_json_for_every_option() {
    let Some(app) = app().await else { return };
    let json = app.json_rows(&format!("rows={ROWS}")).await;
    let mut sizes = Vec::new();
    for dictionary in [false, true] {
        for compression in ["none", "lz4", "zstd"] {
            let (response, batches) = app
                .arrow(&format!(
                    "rows={ROWS}&batch_size={BATCH}&dictionary={dictionary}&ipc_compression={compression}"
                ))
                .await;
            let label = format!("dictionary={dictionary} ipc_compression={compression}");
            assert_eq!(batches.len(), ROWS.div_ceil(BATCH), "{label}");
            assert_batches_match_json(&batches, &json);

            let schema = batches[0].schema();
            for column in &COLUMNS {
                let field = schema.field_with_name(column.name).unwrap();
                let expect_dictionary = dictionary && column.dictionary;
                assert_eq!(
                    matches!(field.data_type(), DataType::Dictionary(_, _)),
                    expect_dictionary,
                    "{label}: {}",
                    column.name
                );
            }

            // Each dictionary is sent once and shared by all batches.
            let kinds = ipc_message_kinds(&response.body);
            let count = |kind| kinds.iter().filter(|k| **k == kind).count();
            assert_eq!(kinds[0], MessageHeader::Schema);
            assert_eq!(
                count(MessageHeader::DictionaryBatch),
                if dictionary { DICT_COLUMNS } else { 0 }
            );
            assert_eq!(count(MessageHeader::RecordBatch), ROWS.div_ceil(BATCH));
            sizes.push((label, response.body.len()));
        }
    }
    let size = |label: &str| sizes.iter().find(|(l, _)| l == label).unwrap().1;
    assert!(
        size("dictionary=false ipc_compression=lz4")
            < size("dictionary=false ipc_compression=none")
    );
    assert!(
        size("dictionary=false ipc_compression=zstd")
            < size("dictionary=false ipc_compression=none")
    );
    assert!(
        size("dictionary=true ipc_compression=none")
            < size("dictionary=false ipc_compression=none")
    );
}

#[tokio::test]
async fn parquet_matches_json_for_every_codec() {
    let Some(app) = app().await else { return };
    let json = app.json_rows(&format!("rows={ROWS}")).await;
    for (codec, expected) in [
        ("none", "UNCOMPRESSED"),
        ("snappy", "SNAPPY"),
        ("gzip", "GZIP"),
        ("zstd", "ZSTD"),
        ("lz4", "LZ4_RAW"),
        ("brotli", "BROTLI"),
    ] {
        let response = app
            .get(&format!(
                "/api/data?format=parquet&http_compression=false&rows={ROWS}&batch_size={BATCH}&parquet_compression={codec}"
            ))
            .await;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(
            response.header("content-type"),
            "application/vnd.apache.parquet"
        );
        let builder = ParquetRecordBatchReaderBuilder::try_new(response.body.clone()).unwrap();
        let metadata = builder.metadata().clone();
        let row_groups: Vec<i64> = metadata
            .row_groups()
            .iter()
            .map(|rg| rg.num_rows())
            .collect();
        assert_eq!(
            row_groups,
            [1024, 1024, 952],
            "{codec}: row groups follow batch_size"
        );
        for row_group in metadata.row_groups() {
            for column in row_group.columns() {
                assert_eq!(codec_name(column.compression()), expected, "{codec}");
            }
        }
        // Strings stay plain Utf8 even though the dict columns exist.
        let batches = builder
            .build()
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            batches[0]
                .schema()
                .field_with_name("country")
                .unwrap()
                .data_type(),
            &DataType::Utf8
        );
        assert_batches_match_json(&batches, &json);
    }
}

fn codec_name(compression: Compression) -> &'static str {
    match compression {
        Compression::UNCOMPRESSED => "UNCOMPRESSED",
        Compression::SNAPPY => "SNAPPY",
        Compression::GZIP(_) => "GZIP",
        Compression::ZSTD(_) => "ZSTD",
        Compression::LZ4_RAW => "LZ4_RAW",
        Compression::BROTLI(_) => "BROTLI",
        other => panic!("unexpected codec {other:?}"),
    }
}

#[tokio::test]
async fn arrow_files_share_dictionaries_across_batches() {
    let Some(app) = app().await else { return };
    let json = app.json_rows(&format!("rows={ROWS}")).await;

    // The IPC file writer rejects dictionary replacement outright, so this
    // proves every batch carries the same dictionaries.
    let pool = db::create_pool(app.database.clone(), 1).unwrap();
    let client = pool.get().await.unwrap();
    let query = RowsQuery {
        after_id: 0,
        limit: ROWS as i64,
        columns: ColumnSelection::all(),
    };
    let fetched = db::fetch_rows(&client, &query).await.unwrap();
    for compression in IpcCompression::ALL {
        let encoding = Encoding::Arrow {
            compression: *compression,
            dictionary: true,
            batch_size: BATCH,
        };
        let file = encode::encode_to(
            Vec::new(),
            &fetched.rows,
            fetched.columns.columns(),
            &encoding,
            ArrowContainer::File,
        )
        .unwrap();
        let reader = FileReader::try_new(Cursor::new(file), None).unwrap();
        assert_eq!(reader.num_batches(), 3);
        let batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
        assert_batches_match_json(&batches, &json);
    }
}

#[tokio::test]
async fn export_writes_readable_files() {
    let Some(app) = app().await else { return };
    let json = app
        .json_rows("rows=2500&offset=100&columns=id,created_at,country,discount,notes")
        .await;
    let dir = std::env::temp_dir().join(format!("playground-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let config = |format: Format, file: &str, extra: &[&str]| {
        let mut config = ExportConfig {
            format,
            out: dir.join(file),
            rows: 2500,
            offset: 100,
            columns: Some("id,created_at,country,discount,notes".into()),
            compression: None,
            dictionary: false,
            batch_size: BATCH,
            database: app.database.clone(),
        };
        for flag in extra {
            match *flag {
                "dictionary" => config.dictionary = true,
                codec => config.compression = Some(codec.into()),
            }
        }
        config
    };

    let arrow = config(Format::Arrow, "orders.arrow", &["dictionary", "zstd"]);
    export::run(&arrow).await.unwrap();
    let reader = FileReader::try_new(std::fs::File::open(&arrow.out).unwrap(), None).unwrap();
    let batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
    assert_batches_match_json(&batches, &json);

    let parquet = config(Format::Parquet, "orders.parquet", &["brotli"]);
    export::run(&parquet).await.unwrap();
    let file = std::fs::File::open(&parquet.out).unwrap();
    let batches = ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_batches_match_json(&batches, &json);

    let json_export = config(Format::Json, "orders.json", &[]);
    export::run(&json_export).await.unwrap();
    let exported: Vec<Value> =
        serde_json::from_slice(&std::fs::read(&json_export.out).unwrap()).unwrap();
    assert_eq!(exported, json);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn http_compression_can_be_switched_off() {
    let Some(app) = app().await else { return };
    let uri = |on: bool| format!("/api/data?rows=2000&http_compression={on}");
    let all_encodings = [("accept-encoding", "gzip, br, zstd")];

    let plain = app.request(Method::GET, &uri(false), &all_encodings).await;
    assert!(plain.headers.get("content-encoding").is_none(), "opted out");
    let body_bytes: u64 = plain.header("x-body-bytes").parse().unwrap();
    assert_eq!(plain.body.len() as u64, body_bytes);
    assert_eq!(
        plain.exact_size,
        Some(body_bytes),
        "small bodies keep Content-Length"
    );

    let gzip = app
        .request(Method::GET, &uri(true), &[("accept-encoding", "gzip")])
        .await;
    assert_eq!(gzip.header("content-encoding"), "gzip");
    assert_eq!(gzip.header("x-body-bytes"), plain.header("x-body-bytes"));
    assert!(gzip.body.len() < plain.body.len() / 3);
    let mut decompressed = Vec::new();
    flate2::read::GzDecoder::new(&gzip.body[..])
        .read_to_end(&mut decompressed)
        .unwrap();
    assert_eq!(decompressed, plain.body);

    for format in ["arrow", "parquet"] {
        let off = app
            .request(
                Method::GET,
                &format!("/api/data?rows=2000&format={format}&http_compression=0"),
                &all_encodings,
            )
            .await;
        assert!(off.headers.get("content-encoding").is_none(), "{format}");
        let on = app
            .request(
                Method::GET,
                &format!("/api/data?rows=2000&format={format}"),
                &all_encodings,
            )
            .await;
        assert!(on.headers.get("content-encoding").is_some(), "{format}");
    }

    // Tiny bodies are never worth compressing.
    let tiny = app
        .request(Method::GET, "/api/data?rows=1&columns=id", &all_encodings)
        .await;
    assert_eq!(tiny.body, "[{\"id\":1}]");
    assert!(tiny.headers.get("content-encoding").is_none());
    let ping = app.request(Method::GET, "/api/ping", &all_encodings).await;
    assert!(ping.headers.get("content-encoding").is_none());
}

#[tokio::test]
async fn data_headers_and_keyset_pagination() {
    let Some(app) = app().await else { return };
    let response = app
        .get("/api/data?rows=1000&offset=500&columns=total,country,total&_=nonce")
        .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.header("content-type"), "application/json");
    assert_eq!(response.header("x-row-count"), "1000");
    assert_eq!(response.header("x-column-count"), "2");
    assert_eq!(
        response.header("x-last-id"),
        "1500",
        "known even though id is not selected"
    );
    assert_eq!(response.header("x-format"), "json");
    assert_eq!(response.header("x-cache"), "off");
    assert_eq!(response.header("cache-control"), "no-store");
    assert_eq!(response.header("timing-allow-origin"), "*");
    assert_eq!(
        response.header("x-body-bytes"),
        response.body.len().to_string()
    );

    let timing = response.header("server-timing");
    let parts: Vec<&str> = timing.split(", ").collect();
    assert_eq!(parts.len(), 3, "{timing}");
    assert!(parts[0].starts_with("db;dur=") && parts[0].ends_with(";desc=\"PostgreSQL\""));
    assert!(parts[1].starts_with("encode;dur=") && parts[1].ends_with(";desc=\"json encode\""));
    assert!(parts[2].starts_with("total;dur="));
    for (part, header) in parts.iter().zip(["x-db-ms", "x-encode-ms", "x-total-ms"]) {
        let millis = response.header(header);
        assert!(
            part.contains(&format!("dur={millis}")),
            "{part} vs {header}"
        );
        let decimals = millis.split('.').nth(1).unwrap();
        assert_eq!(decimals.len(), 3, "{header}: {millis}");
    }

    let rows = response.rows();
    assert_eq!(rows.len(), 1000);
    let keys: Vec<&String> = rows[0].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["total", "country"]);

    let next = app.get("/api/data?rows=10&offset=1500&columns=id").await;
    assert_eq!(next.rows()[0]["id"], 1501);
}

#[tokio::test]
async fn empty_results_are_valid_in_every_format() {
    let Some(app) = app().await else { return };
    let offset = ROWS + 10;
    let json = app.get(&format!("/api/data?offset={offset}")).await;
    assert_eq!(json.status, StatusCode::OK);
    assert_eq!(json.body, "[]");
    assert_eq!(json.header("x-row-count"), "0");
    assert_eq!(
        json.header("x-last-id"),
        offset.to_string(),
        "falls back to the offset"
    );

    let (arrow, batches) = app.arrow(&format!("offset={offset}&dictionary=true")).await;
    assert!(batches.is_empty());
    assert_eq!(ipc_message_kinds(&arrow.body), [MessageHeader::Schema]);

    let parquet = app
        .get(&format!("/api/data?format=parquet&offset={offset}"))
        .await;
    let reader = ParquetRecordBatchReaderBuilder::try_new(parquet.body).unwrap();
    assert_eq!(reader.metadata().file_metadata().num_rows(), 0);
    assert_eq!(reader.schema().fields().len(), COLUMNS.len());
}

#[tokio::test]
async fn bad_parameters_are_rejected_with_json_errors() {
    let Some(app) = app().await else { return };
    for (query, message) in [
        ("columns=id,secret", "unknown column \"secret\""),
        ("format=xml", "unknown format \"xml\""),
        ("rows=0", "rows must be an integer from 1 to 5000"),
        ("rows=5001", "rows must be an integer from 1 to 5000"),
        ("batch_size=10", "batch_size"),
        ("ipc_compression=brotli", "ipc_compression"),
        ("parquet_compression=lzo", "parquet_compression"),
        ("dictionary=perhaps", "dictionary must be a boolean"),
        ("offset=-3", "offset"),
    ] {
        let response = app.get(&format!("/api/data?{query}")).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(response.header("content-type"), "application/json");
        let error = response.json()["error"].as_str().unwrap().to_owned();
        assert!(error.contains(message), "{query}: {error}");
    }

    let bench = app.get("/api/bench?rows=5001").await;
    assert_eq!(bench.status, StatusCode::BAD_REQUEST);

    let unknown = app.get("/api/nope").await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.json()["error"], "not found");
    assert_eq!(unknown.header("timing-allow-origin"), "*");

    let post = app.request(Method::POST, "/api/data", &[]).await;
    assert_eq!(post.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(post.json()["error"], "method not allowed");
}

#[tokio::test]
async fn bench_reports_every_variant() {
    let Some(app) = app().await else { return };
    let response = app
        .get("/api/bench?rows=2000&columns=id,country,total")
        .await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(response.headers.contains_key("server-timing"));
    assert_eq!(response.header("x-cache"), "off");
    let report = response.json();
    assert_eq!(report["rows"], 2000);
    assert_eq!(report["columns"], 3);
    assert!(report["db_ms"].as_f64().unwrap() >= 0.0);

    let results = report["results"].as_array().unwrap();
    assert_eq!(results.len(), 13);
    for result in results {
        assert!(["json", "arrow", "parquet"].contains(&result["format"].as_str().unwrap()));
        assert!(result["variant"].is_string());
        assert!(result["options"].is_object());
        assert!(result["encode_ms"].as_f64().unwrap() >= 0.0);
        assert!(result["bytes"].as_u64().unwrap() > 0);
    }
    assert_eq!(results[0]["variant"], "json");
    assert_eq!(results[0]["options"], serde_json::json!({}));
    let zstd_dict = results
        .iter()
        .find(|r| r["variant"] == "arrow · zstd · dict")
        .unwrap();
    assert_eq!(
        zstd_dict["options"],
        serde_json::json!({"ipc_compression": "zstd", "dictionary": true})
    );

    // The JSON variant is exactly what /api/data would send.
    let data = app
        .get("/api/data?rows=2000&columns=id,country,total&http_compression=false")
        .await;
    assert_eq!(results[0]["bytes"], data.body.len());
}

#[tokio::test]
async fn info_describes_the_table() {
    let Some(app) = app().await else { return };
    let response = app.get("/api/info").await;
    assert_eq!(response.status, StatusCode::OK);
    let info = response.json();
    assert_eq!(info["table"], "orders");
    assert_eq!(info["row_estimate"], ROWS);
    assert_eq!(info["max_id"], ROWS);
    assert!(info["table_bytes"].as_i64().unwrap() > 0);
    assert_eq!(info["max_rows"], MAX_ROWS);
    assert_eq!(
        info["formats"],
        serde_json::json!(["json", "arrow", "parquet"])
    );
    assert_eq!(
        info["ipc_compression"],
        serde_json::json!(["none", "lz4", "zstd"])
    );
    assert_eq!(
        info["parquet_compression"],
        serde_json::json!(["none", "snappy", "gzip", "zstd", "lz4", "brotli"])
    );
    assert_eq!(info["server"]["version"], env!("CARGO_PKG_VERSION"));
    let columns = info["columns"].as_array().unwrap();
    assert_eq!(columns.len(), COLUMNS.len());
    assert_eq!(
        columns[0],
        serde_json::json!({"name": "id", "pg_type": "int8", "arrow_type": "Int64", "nullable": false, "dictionary": false, "group": "numeric"})
    );
    assert_eq!(columns[1]["arrow_type"], "Timestamp(Microsecond, \"UTC\")");
    let country = columns.iter().find(|c| c["name"] == "country").unwrap();
    assert_eq!(country["dictionary"], true);
    assert_eq!(country["group"], "text");
}

#[tokio::test]
async fn row_cache_hits_on_repeated_requests() {
    let Some(app) = app().await else { return };
    let uri = "/api/data?rows=700&offset=1234&columns=id,email&cache=true";
    let first = app.get(uri).await;
    assert_eq!(first.header("x-cache"), "miss");
    let second = app.get(&format!("{uri}&format=arrow")).await;
    assert_eq!(
        second.header("x-cache"),
        "hit",
        "the format is not part of the key"
    );
    let third = app.get(uri).await;
    assert_eq!(third.header("x-cache"), "hit");
    assert_eq!(third.body, first.body);
    assert_eq!(third.header("x-last-id"), "1934");

    let other_columns = app
        .get("/api/data?rows=700&offset=1234&columns=email,id&cache=true")
        .await;
    assert_eq!(other_columns.header("x-cache"), "miss");
    let uncached = app
        .get("/api/data?rows=700&offset=1234&columns=id,email")
        .await;
    assert_eq!(uncached.header("x-cache"), "off");
}

#[tokio::test]
async fn cors_exposes_the_measurement_headers() {
    let Some(app) = app().await else { return };
    let origin = [("origin", "https://ui.example")];
    let response = app.request(Method::GET, "/api/data?rows=5", &origin).await;
    assert_eq!(response.header("access-control-allow-origin"), "*");
    let exposed = response
        .header("access-control-expose-headers")
        .to_lowercase();
    for header in [
        "server-timing",
        "x-db-ms",
        "x-encode-ms",
        "x-total-ms",
        "x-row-count",
        "x-column-count",
        "x-last-id",
        "x-body-bytes",
        "x-format",
        "x-cache",
    ] {
        assert!(exposed.contains(header), "{header} in {exposed}");
    }

    let preflight = app
        .request(
            Method::OPTIONS,
            "/api/data",
            &[
                ("origin", "https://ui.example"),
                ("access-control-request-method", "GET"),
            ],
        )
        .await;
    assert!(preflight.status.is_success());
    assert!(
        preflight
            .header("access-control-allow-methods")
            .contains("GET")
    );
}

#[tokio::test]
async fn health_ping_and_static_files() {
    let Some(app) = app().await else { return };
    let health = app.get("/api/health").await;
    assert_eq!(health.json(), serde_json::json!({"status": "ok"}));

    let ping = app.get("/api/ping").await;
    assert_eq!(ping.status, StatusCode::NO_CONTENT);
    assert!(ping.body.is_empty());
    assert_eq!(ping.header("timing-allow-origin"), "*");

    let index = app.get("/").await;
    assert_eq!(index.status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&index.body).contains("<title>spa</title>"));
    let client_route = app.get("/runs/42").await;
    assert_eq!(client_route.status, StatusCode::OK);
    assert_eq!(client_route.body, index.body, "SPA fallback");
    let asset = app.get("/assets/app.js").await;
    assert_eq!(asset.body, "console.log('app')");
    let api_miss = app.get("/api/unknown/path").await;
    assert_eq!(api_miss.status, StatusCode::NOT_FOUND);
    assert_eq!(api_miss.header("content-type"), "application/json");

    let without_frontend =
        router_with_static_dir(&app.database, std::path::Path::new("/nonexistent/dist"));
    let note = send(&without_frontend, Method::GET, "/", &[]).await;
    assert_eq!(note.status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&note.body).contains("npm run build"));
}
