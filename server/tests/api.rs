//! End-to-end tests of the HTTP API against a seeded database, driving the
//! axum router in-process. Needs `TEST_DATABASE_URL`.

mod common;

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
use playground::encode::{self, ArrowContainer, CancelFlag, Encoding, Format, IpcCompression};
use playground::query::RowsQuery;
use playground::schema::{DATASETS, Dataset, ORDER_LINES, ORDERS, ORDERS_COLUMNS};
use playground::server::{self, AppState, SEED_CHECK_INTERVAL, SEED_CHECK_MAX_AGE, Settings};
use playground::{db, export, seed};
use serde_json::{Value, json};
use tokio::sync::MutexGuard;
use tower::ServiceExt;

use common::{DB_LOCK, assert_batches_match_json, count, ipc_message_kinds};

const SCHEMA: &str = "playground_api_tests";
const ROWS: usize = 3000;
const MAX_ROWS: u64 = 5000;
/// Three record batches / row groups: 1024 + 1024 + 952 rows.
const BATCH: usize = 1024;
/// Far beyond every dataset's largest key.
const PAST_THE_END: u64 = 1_000_000_000_000;

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

    fn millis(&self, name: &str) -> f64 {
        self.header(name).parse().unwrap()
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

/// The router over every dataset, seeded once, holding the lock that
/// serializes this binary's tests. `None` without `TEST_DATABASE_URL`.
async fn app() -> Option<TestApp> {
    // Locked first: concurrent CREATE SCHEMA IF NOT EXISTS can still collide.
    let mut seeded = DB_LOCK.lock().await;
    let database = common::database(SCHEMA).await?;
    if !*seeded {
        seed::run(&common::seed_config(&database, ROWS as u64, 3))
            .await
            .expect("seed the test tables");
        *seeded = true;
    }
    Some(TestApp {
        router: router_with(&database, &static_dir(), 4),
        database,
        _lock: seeded,
    })
}

fn router_with(database: &tokio_postgres::Config, static_dir: &Path, pool_size: usize) -> Router {
    router_from(database, static_dir, pool_size, settings())
}

fn router_from(
    database: &tokio_postgres::Config,
    static_dir: &Path,
    pool_size: usize,
    settings: Settings,
) -> Router {
    let pool = db::create_pool(database.clone(), pool_size).unwrap();
    server::router(AppState::new(pool, settings), static_dir)
}

fn settings() -> Settings {
    Settings {
        max_rows: MAX_ROWS,
        cache_entries: 4,
        cache_max_rows: 10_000,
        seed_check_interval: SEED_CHECK_INTERVAL,
        seed_check_max_age: SEED_CHECK_MAX_AGE,
        server_info: ServerInfo::from_env(),
    }
}

fn static_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("playground-api-tests-static");
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html><title>spa</title>").unwrap();
    std::fs::write(dir.join("assets/app-3f2a.js"), "console.log('app')").unwrap();
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
        assert_eq!(
            response.status,
            StatusCode::OK,
            "{query}: {}",
            String::from_utf8_lossy(&response.body)
        );
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

    async fn parquet(&self, query: &str) -> (TestResponse, Vec<RecordBatch>) {
        let response = self
            .get(&format!(
                "/api/data?format=parquet&http_compression=false&{query}"
            ))
            .await;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(
            response.header("content-type"),
            "application/vnd.apache.parquet"
        );
        let batches = ParquetRecordBatchReaderBuilder::try_new(response.body.clone())
            .unwrap()
            .build()
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        (response, batches)
    }

    async fn client(&self) -> tokio_postgres::Client {
        db::connect(&self.database).await.unwrap()
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

fn decompress(encoding: &str, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    match encoding {
        "zstd" => out = zstd::decode_all(body).unwrap(),
        "br" => {
            brotli::Decompressor::new(body, 4096)
                .read_to_end(&mut out)
                .unwrap();
        }
        "gzip" => {
            flate2::read::GzDecoder::new(body)
                .read_to_end(&mut out)
                .unwrap();
        }
        other => panic!("unexpected Content-Encoding {other}"),
    }
    out
}

/// Every column of `dataset`, read with tokio-postgres in key order.
async fn postgres_rows(
    client: &tokio_postgres::Client,
    dataset: &Dataset,
    limit: usize,
) -> Vec<tokio_postgres::Row> {
    let names: Vec<String> = dataset.columns.iter().map(|c| c.quoted()).collect();
    let sql = format!(
        "SELECT {} FROM \"{}\" ORDER BY \"{}\" LIMIT {limit}",
        names.join(", "),
        dataset.relation,
        dataset.key
    );
    client.query(&sql, &[]).await.unwrap()
}

#[tokio::test]
async fn every_dataset_agrees_across_formats_and_with_postgres() {
    let Some(app) = app().await else { return };
    let client = app.client().await;
    for dataset in DATASETS {
        let name = dataset.name;
        let query = format!("dataset={name}&rows={ROWS}");
        let json = app.json_rows(&query).await;
        let total = count(
            &client,
            &format!("SELECT count(*) FROM {}", dataset.relation),
        )
        .await;
        assert_eq!(json.len(), ROWS.min(total as usize), "{name}");
        assert!(json.len() > 1000, "{name}: enough rows for several batches");
        let names: Vec<&str> = json[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let catalog: Vec<&str> = dataset.columns.iter().map(|c| c.name).collect();
        assert_eq!(names, catalog, "{name}: every column, in catalog order");
        let keys: Vec<i64> = json
            .iter()
            .map(|r| r[dataset.key].as_i64().unwrap())
            .collect();
        assert!(keys.is_sorted_by(|a, b| a < b), "{name}: keyset order");

        let expected = postgres_rows(&client, dataset, ROWS).await;
        common::assert_json_matches_postgres(&json, &expected, dataset.columns);

        for dictionary in [false, true] {
            let (response, batches) = app
                .arrow(&format!(
                    "{query}&batch_size={BATCH}&dictionary={dictionary}&ipc_compression=lz4"
                ))
                .await;
            let label = format!("{name} dictionary={dictionary}");
            assert_eq!(response.header("x-dataset"), name);
            assert_eq!(batches.len(), json.len().div_ceil(BATCH), "{label}");
            assert_batches_match_json(&batches, &json);
            let schema = batches[0].schema();
            for column in dataset.columns {
                let field = schema.field_with_name(column.name).unwrap();
                assert_eq!(
                    matches!(field.data_type(), DataType::Dictionary(_, _)),
                    dictionary && column.dictionary,
                    "{label}: {}",
                    column.name
                );
                assert_eq!(field.is_nullable(), column.nullable, "{label}");
            }
            let kinds = ipc_message_kinds(&response.body);
            let dictionaries = kinds
                .iter()
                .filter(|kind| **kind == MessageHeader::DictionaryBatch)
                .count();
            assert_eq!(
                dictionaries,
                if dictionary {
                    dataset.dictionary_columns()
                } else {
                    0
                },
                "{label}: each dictionary is sent once"
            );
        }

        let (response, batches) = app
            .parquet(&format!(
                "{query}&batch_size={BATCH}&parquet_compression=zstd"
            ))
            .await;
        assert_eq!(response.header("x-dataset"), name);
        assert_batches_match_json(&batches, &json);
    }
}

#[tokio::test]
async fn the_catalog_matches_postgres() {
    let Some(app) = app().await else { return };
    let client = app.client().await;
    for dataset in DATASETS {
        let statement = client
            .prepare(&format!("SELECT * FROM \"{}\"", dataset.relation))
            .await
            .unwrap();
        let actual: Vec<(&str, String)> = statement
            .columns()
            .iter()
            .map(|c| (c.name(), c.type_().name().to_owned()))
            .collect();
        let expected: Vec<(&str, String)> = dataset
            .columns
            .iter()
            .map(|c| (c.name, c.ty.pg_type().name().to_owned()))
            .collect();
        assert_eq!(actual, expected, "{}", dataset.name);
        for column in dataset.columns {
            assert_eq!(column.ty.pg_type().name(), column.ty.pg_name());
        }

        // A view does not record NOT NULL, so check the data instead.
        for column in dataset.columns.iter().filter(|c| !c.nullable) {
            let nulls = count(
                &client,
                &format!(
                    "SELECT count(*) FROM \"{}\" WHERE {} IS NULL",
                    dataset.relation,
                    column.quoted()
                ),
            )
            .await;
            assert_eq!(nulls, 0, "{}.{}", dataset.name, column.name);
        }
    }

    let not_null: Vec<(String, bool)> = client
        .query(
            "SELECT attname::text, attnotnull FROM pg_attribute \
             WHERE attrelid = 'orders'::regclass AND attnum > 0 AND NOT attisdropped \
             ORDER BY attnum",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    let expected: Vec<(String, bool)> = ORDERS_COLUMNS
        .iter()
        .map(|c| (c.name.to_owned(), !c.nullable))
        .collect();
    assert_eq!(not_null, expected);
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

            // Each dictionary is sent once and shared by all batches.
            let kinds = ipc_message_kinds(&response.body);
            let count = |kind| kinds.iter().filter(|k| **k == kind).count();
            assert_eq!(kinds[0], MessageHeader::Schema);
            assert_eq!(
                count(MessageHeader::DictionaryBatch),
                if dictionary {
                    ORDERS.dictionary_columns()
                } else {
                    0
                }
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
    let pool = db::create_pool(app.database.clone(), 1).unwrap();
    let client = pool.get().await.unwrap();
    for dataset in [&ORDERS, &ORDER_LINES] {
        let json = app
            .json_rows(&format!("dataset={}&rows={ROWS}", dataset.name))
            .await;
        // The IPC file writer rejects dictionary replacement outright, so
        // this proves every batch carries the same dictionaries.
        let query = RowsQuery::all_columns(dataset, 0, ROWS as i64);
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
                &CancelFlag::default(),
            )
            .unwrap();
            let reader = FileReader::try_new(Cursor::new(file), None).unwrap();
            assert_eq!(reader.num_batches(), 3);
            let batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
            assert_batches_match_json(&batches, &json);
        }
    }
}

#[tokio::test]
async fn export_writes_readable_files() {
    let Some(app) = app().await else { return };
    let orders_columns = "id,created_at,country,discount,notes";
    let lines_columns = "line_id,created_at,country_code,product_name,vat_rate,customer_since";
    let dir = std::env::temp_dir().join(format!("playground-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let columns_of = |dataset: &Dataset| {
        if *dataset == ORDERS {
            orders_columns
        } else {
            lines_columns
        }
    };
    let config = |dataset: &'static Dataset, format: Format, file: &str, extra: &[&str]| {
        let mut config = ExportConfig {
            format,
            out: dir.join(file),
            dataset,
            rows: 2500,
            offset: 100,
            columns: Some(columns_of(dataset).into()),
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

    for dataset in [&ORDERS, &ORDER_LINES] {
        let json = app
            .json_rows(&format!(
                "dataset={}&rows=2500&offset=100&columns={}",
                dataset.name,
                columns_of(dataset)
            ))
            .await;
        assert_eq!(json.len(), 2500);

        let arrow = config(
            dataset,
            Format::Arrow,
            "rows.arrow",
            &["dictionary", "zstd"],
        );
        export::run(&arrow).await.unwrap();
        let reader = FileReader::try_new(std::fs::File::open(&arrow.out).unwrap(), None).unwrap();
        let batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
        assert_batches_match_json(&batches, &json);

        let parquet = config(dataset, Format::Parquet, "rows.parquet", &["brotli"]);
        export::run(&parquet).await.unwrap();
        let file = std::fs::File::open(&parquet.out).unwrap();
        let batches = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_batches_match_json(&batches, &json);

        let json_export = config(dataset, Format::Json, "rows.json", &[]);
        export::run(&json_export).await.unwrap();
        let exported: Vec<Value> =
            serde_json::from_slice(&std::fs::read(&json_export.out).unwrap()).unwrap();
        assert_eq!(exported, json, "{}", dataset.name);
    }

    let wrong_columns = config(&ORDER_LINES, Format::Json, "x.json", &[]);
    let wrong_columns = ExportConfig {
        columns: Some("id".into()),
        ..wrong_columns
    };
    let err = export::run(&wrong_columns).await.unwrap_err().to_string();
    assert!(err.contains("unknown column \"id\""), "{err}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn http_compression_follows_accept_encoding() {
    let Some(app) = app().await else { return };
    let uri = "/api/data?rows=2000";
    let every_encoding = [("accept-encoding", "gzip, deflate, br, zstd")];
    let plain = app
        .request(
            Method::GET,
            &format!("{uri}&http_compression=false"),
            &every_encoding,
        )
        .await;
    assert!(plain.headers.get("content-encoding").is_none(), "opted out");
    assert_eq!(plain.header("vary"), "Accept-Encoding");
    assert_eq!(plain.header("x-compress-ms"), "0.000");
    assert!(
        plain
            .header("server-timing")
            .contains("compress;dur=0.000;desc=\"HTTP identity\"")
    );
    let body_bytes: u64 = plain.header("x-body-bytes").parse().unwrap();
    assert_eq!(plain.body.len() as u64, body_bytes);
    assert_eq!(
        plain.header("x-encoded-bytes"),
        plain.header("x-body-bytes")
    );
    assert_eq!(
        plain.exact_size,
        Some(body_bytes),
        "small bodies keep Content-Length"
    );

    for (accept, expected) in [
        ("gzip", "gzip"),
        ("br", "br"),
        ("zstd", "zstd"),
        ("gzip, deflate, br, zstd", "zstd"),
        ("gzip, deflate, br", "br"),
        ("zstd;q=0.5, gzip", "gzip"),
        ("identity, *;q=0.1", "zstd"),
    ] {
        let response = app
            .request(Method::GET, uri, &[("accept-encoding", accept)])
            .await;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.header("content-encoding"), expected, "{accept}");
        assert_eq!(response.header("vary"), "Accept-Encoding");
        assert!(response.millis("x-compress-ms") > 0.0, "{accept}");
        assert!(
            response
                .header("server-timing")
                .contains(&format!("desc=\"HTTP {expected}\"")),
            "{accept}"
        );
        assert_eq!(
            response.header("x-encoded-bytes"),
            response.body.len().to_string(),
            "{accept}: the bytes actually sent"
        );
        assert_eq!(response.exact_size, Some(response.body.len() as u64));
        assert_eq!(
            response.header("x-body-bytes"),
            plain.header("x-body-bytes")
        );
        assert!(response.body.len() < plain.body.len() / 3, "{accept}");
        assert_eq!(decompress(expected, &response.body), plain.body, "{accept}");
        let total = response.millis("x-total-ms");
        let parts = response.millis("x-db-ms")
            + response.millis("x-encode-ms")
            + response.millis("x-compress-ms");
        assert!(total >= parts - 0.003, "{accept}: {total} < {parts}");
    }

    for headers in [
        &[][..],
        &[("accept-encoding", "identity")],
        &[("accept-encoding", "deflate")],
        &[("accept-encoding", "gzip;q=0, *;q=0")],
    ] {
        let response = app.request(Method::GET, uri, headers).await;
        assert!(
            response.headers.get("content-encoding").is_none(),
            "{headers:?}"
        );
        assert_eq!(response.header("x-compress-ms"), "0.000");
        assert_eq!(response.body, plain.body);
    }

    // Payloads already compressed with a strong codec go out as they are;
    // the others are compressed like JSON.
    for (format, options, compressed) in [
        ("arrow", "ipc_compression=none", true),
        ("arrow", "ipc_compression=lz4&dictionary=true", true),
        ("arrow", "ipc_compression=zstd", false),
        ("parquet", "parquet_compression=none", true),
        ("parquet", "parquet_compression=snappy", true),
        ("parquet", "parquet_compression=lz4", true),
        ("parquet", "parquet_compression=zstd", false),
        ("parquet", "parquet_compression=gzip", false),
        ("parquet", "parquet_compression=brotli", false),
    ] {
        let label = format!("{format} {options}");
        let response = app
            .request(
                Method::GET,
                &format!("{uri}&format={format}&{options}"),
                &every_encoding,
            )
            .await;
        assert_eq!(response.status, StatusCode::OK, "{label}");
        assert_eq!(
            response.headers.contains_key("content-encoding"),
            compressed,
            "{label}"
        );
        assert_eq!(
            response.header("x-encoded-bytes"),
            response.body.len().to_string(),
            "{label}"
        );
        if compressed {
            assert!(response.millis("x-compress-ms") > 0.0, "{label}");
            let payload = decompress("zstd", &response.body);
            assert_eq!(
                payload.len().to_string(),
                response.header("x-body-bytes"),
                "{label}"
            );
        } else {
            assert_eq!(response.header("x-compress-ms"), "0.000", "{label}");
            assert_eq!(
                response.header("x-encoded-bytes"),
                response.header("x-body-bytes")
            );
        }
        let off = app
            .request(
                Method::GET,
                &format!("{uri}&format={format}&{options}&http_compression=0"),
                &every_encoding,
            )
            .await;
        assert!(off.headers.get("content-encoding").is_none(), "{label}");
        assert_eq!(off.header("x-compress-ms"), "0.000", "{label}");
    }

    // Tiny bodies are never worth compressing.
    let tiny = app
        .request(Method::GET, "/api/data?rows=1&columns=id", &every_encoding)
        .await;
    assert_eq!(tiny.body, "[{\"id\":1}]");
    assert!(tiny.headers.get("content-encoding").is_none());
    assert_eq!(tiny.header("x-compress-ms"), "0.000");
    let ping = app.request(Method::GET, "/api/ping", &every_encoding).await;
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
    assert_eq!(response.header("x-dataset"), "orders", "the default");
    assert_eq!(response.header("x-cache"), "off");
    assert_eq!(response.header("cache-control"), "no-store");
    assert_eq!(response.header("timing-allow-origin"), "*");
    assert_eq!(response.header("vary"), "Accept-Encoding");
    assert_eq!(
        response.header("x-body-bytes"),
        response.body.len().to_string()
    );
    assert_eq!(
        response.header("x-encoded-bytes"),
        response.body.len().to_string()
    );

    let timing = response.header("server-timing");
    let parts: Vec<&str> = timing.split(", ").collect();
    assert_eq!(parts.len(), 4, "{timing}");
    assert!(parts[0].starts_with("db;dur=") && parts[0].ends_with(";desc=\"PostgreSQL\""));
    assert!(parts[1].starts_with("encode;dur=") && parts[1].ends_with(";desc=\"json encode\""));
    assert!(parts[2].starts_with("compress;dur=") && parts[2].ends_with(";desc=\"HTTP identity\""));
    assert!(parts[3].starts_with("total;dur="));
    for (part, header) in
        parts
            .iter()
            .zip(["x-db-ms", "x-encode-ms", "x-compress-ms", "x-total-ms"])
    {
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
async fn keyset_pagination_follows_each_datasets_key() {
    let Some(app) = app().await else { return };
    for dataset in DATASETS {
        let (name, key) = (dataset.name, dataset.key);
        let other = dataset.columns[1].name;
        let whole = app
            .json_rows(&format!("dataset={name}&rows=900&columns={key},{other}"))
            .await;
        let keys: Vec<i64> = whole.iter().map(|r| r[key].as_i64().unwrap()).collect();
        assert!(keys.is_sorted_by(|a, b| a < b), "{name}");

        // Three pages without the key column: X-Last-Id still carries it.
        let mut offset = 0;
        for page in 0..3 {
            let response = app
                .get(&format!(
                    "/api/data?dataset={name}&rows=300&offset={offset}&columns={other}"
                ))
                .await;
            assert_eq!(response.header("x-dataset"), name);
            let rows = response.rows();
            assert_eq!(rows.len(), 300, "{name}");
            for (i, row) in rows.iter().enumerate() {
                assert_eq!(row.as_object().unwrap().len(), 1);
                assert_eq!(row[other], whole[page * 300 + i][other], "{name}");
            }
            let last: i64 = response.header("x-last-id").parse().unwrap();
            assert_eq!(last, keys[page * 300 + 299], "{name} page {page}");
            offset = last;
        }
    }

    // Order line ids are order_id * 10 + line_no, so an offset between
    // orders starts at the next order's first line.
    let lines = app
        .json_rows("dataset=order_lines&rows=3&offset=19&columns=line_id,order_id,line_no")
        .await;
    assert_eq!(
        lines[0],
        json!({"line_id": 21, "order_id": 2, "line_no": 1})
    );
    for line in &lines {
        assert_eq!(
            line["line_id"].as_i64().unwrap(),
            line["order_id"].as_i64().unwrap() * 10 + line["line_no"].as_i64().unwrap()
        );
    }
}

#[tokio::test]
async fn empty_results_are_valid_in_every_format() {
    let Some(app) = app().await else { return };
    for dataset in DATASETS {
        let query = format!("dataset={}&offset={PAST_THE_END}", dataset.name);
        let json = app.get(&format!("/api/data?{query}")).await;
        assert_eq!(json.status, StatusCode::OK);
        assert_eq!(json.body, "[]");
        assert_eq!(json.header("x-row-count"), "0");
        assert_eq!(
            json.header("x-last-id"),
            PAST_THE_END.to_string(),
            "falls back to the offset"
        );

        let (arrow, batches) = app.arrow(&format!("{query}&dictionary=true")).await;
        assert!(batches.is_empty());
        assert_eq!(ipc_message_kinds(&arrow.body), [MessageHeader::Schema]);
        let schema = StreamReader::try_new(Cursor::new(arrow.body), None)
            .unwrap()
            .schema();
        assert_eq!(schema.fields().len(), dataset.columns.len());

        let parquet = app.get(&format!("/api/data?format=parquet&{query}")).await;
        let reader = ParquetRecordBatchReaderBuilder::try_new(parquet.body).unwrap();
        assert_eq!(reader.metadata().file_metadata().num_rows(), 0);
        assert_eq!(reader.schema().fields().len(), dataset.columns.len());
    }
}

#[tokio::test]
async fn datasets_choose_the_relation_and_the_columns() {
    let Some(app) = app().await else { return };
    let response = app
        .get("/api/data?dataset=Order_Lines&rows=5&columns=line_total,country_code,line_total")
        .await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "names are case-insensitive"
    );
    assert_eq!(response.header("x-dataset"), "order_lines");
    assert_eq!(response.header("x-column-count"), "2");
    let keys: Vec<String> = response.rows()[0]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, ["line_total", "country_code"]);

    let default = app.get("/api/data?dataset=&rows=1&columns=id").await;
    assert_eq!(default.header("x-dataset"), "orders", "empty means absent");

    for dataset in DATASETS {
        let all = app
            .get(&format!("/api/data?dataset={}&rows=1", dataset.name))
            .await;
        assert_eq!(
            all.header("x-column-count"),
            dataset.columns.len().to_string()
        );
    }

    let live = app
        .json_rows("dataset=daily_sales_live&rows=500&offset=250")
        .await;
    let stored = app
        .json_rows("dataset=daily_sales&rows=500&offset=250")
        .await;
    assert_eq!(
        live, stored,
        "the live view and the materialized view agree"
    );
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
        (
            "dataset=lines",
            "unknown dataset \"lines\" (expected one of: orders, order_lines, daily_sales, daily_sales_live)",
        ),
        ("dataset=order_lines&columns=id", "unknown column \"id\""),
        (
            "dataset=daily_sales&columns=total",
            "unknown column \"total\"",
        ),
        (
            "dataset=orders&columns=line_id",
            "unknown column \"line_id\"",
        ),
    ] {
        let response = app.get(&format!("/api/data?{query}")).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(response.header("content-type"), "application/json");
        let error = response.json()["error"].as_str().unwrap().to_owned();
        assert!(error.contains(message), "{query}: {error}");
    }

    let bench = app.get("/api/bench?rows=5001").await;
    assert_eq!(bench.status, StatusCode::BAD_REQUEST);
    let bench = app.get("/api/bench?dataset=nope").await;
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
    assert_eq!(response.header("x-dataset"), "orders");
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
    assert_eq!(results[0]["options"], json!({}));
    let zstd_dict = results
        .iter()
        .find(|r| r["variant"] == "arrow · zstd · dict")
        .unwrap();
    assert_eq!(
        zstd_dict["options"],
        json!({"ipc_compression": "zstd", "dictionary": true})
    );

    // The JSON variant is exactly what /api/data would send.
    let data = app
        .get("/api/data?rows=2000&columns=id,country,total&http_compression=false")
        .await;
    assert_eq!(results[0]["bytes"], data.body.len());

    let lines = app
        .get("/api/bench?dataset=order_lines&rows=1500&offset=40")
        .await;
    assert_eq!(lines.status, StatusCode::OK);
    assert_eq!(lines.header("x-dataset"), "order_lines");
    let report = lines.json();
    assert_eq!(report["rows"], 1500);
    assert_eq!(report["columns"], ORDER_LINES.columns.len());
    let data = app
        .get("/api/data?dataset=order_lines&rows=1500&offset=40&http_compression=false")
        .await;
    assert_eq!(report["results"][0]["bytes"], data.body.len());
}

#[tokio::test]
async fn info_describes_every_dataset() {
    let Some(app) = app().await else { return };
    let client = app.client().await;
    let response = app.get("/api/info").await;
    assert_eq!(response.status, StatusCode::OK);
    let info = response.json();
    let keys: Vec<&str> = info
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "default_dataset",
            "datasets",
            "seed",
            "max_rows",
            "formats",
            "ipc_compression",
            "parquet_compression",
            "http_compression",
            "server"
        ]
    );
    assert_eq!(info["default_dataset"], "orders");
    assert_eq!(info["max_rows"], MAX_ROWS);
    assert_eq!(info["formats"], json!(["json", "arrow", "parquet"]));
    assert_eq!(info["ipc_compression"], json!(["none", "lz4", "zstd"]));
    assert_eq!(
        info["parquet_compression"],
        json!(["none", "snappy", "gzip", "zstd", "lz4", "brotli"])
    );
    assert_eq!(info["http_compression"], json!(["zstd", "br", "gzip"]));
    assert_eq!(info["server"]["version"], "0.2.0");
    assert_eq!(info["server"]["version"], env!("CARGO_PKG_VERSION"));

    let lines = count(&client, "SELECT count(*) FROM order_items").await;
    let max_line = count(&client, "SELECT max(id) FROM order_items").await;
    let days = count(&client, "SELECT count(*) FROM daily_sales").await;
    let seed = &info["seed"];
    assert_eq!(seed["state"], "complete");
    assert_eq!(seed["schema_version"], 2);
    assert_eq!(seed["orders"], ROWS);
    assert_eq!(seed["order_items"], lines);
    let finished_at = seed["finished_at"].as_str().unwrap();
    assert!(chrono::DateTime::parse_from_rfc3339(finished_at).is_ok());
    assert!(finished_at.ends_with('Z'), "{finished_at}");

    let datasets = info["datasets"].as_array().unwrap();
    let summary: Vec<Value> = datasets
        .iter()
        .map(|d| {
            json!([
                d["name"],
                d["kind"],
                d["relation"],
                d["key_column"],
                d["row_estimate"],
                d["max_key"]
            ])
        })
        .collect();
    assert_eq!(
        summary,
        [
            json!(["orders", "table", "orders", "id", ROWS, ROWS]),
            json!([
                "order_lines",
                "view",
                "order_line_details",
                "line_id",
                lines,
                max_line
            ]),
            json!([
                "daily_sales",
                "materialized_view",
                "daily_sales",
                "id",
                days,
                days
            ]),
            json!([
                "daily_sales_live",
                "view",
                "daily_sales_live",
                "id",
                days,
                days
            ]),
        ]
    );
    let bytes: Vec<Option<i64>> = datasets.iter().map(|d| d["bytes"].as_i64()).collect();
    assert!(bytes[0].unwrap() > 0);
    assert!(
        bytes[1].unwrap() > bytes[0].unwrap(),
        "the join reads every table"
    );
    assert!(bytes[2].unwrap() > 0);
    assert_eq!(datasets[3]["bytes"], Value::Null, "no storage of its own");

    for (dataset, info) in DATASETS.iter().zip(datasets) {
        assert_eq!(info["label"], dataset.label);
        assert!(info["description"].as_str().unwrap().len() > 20);
        let sql = info["sql"].as_str().unwrap();
        if dataset.name == "orders" {
            assert!(sql.starts_with("CREATE TABLE orders ("), "{sql}");
            assert!(sql.contains("CREATE INDEX orders_created_at_idx"), "{sql}");
        } else {
            assert!(sql.starts_with("SELECT"), "{sql}");
            assert!(sql.contains("JOIN categories ca ON ca.id = p.category_id"));
        }
        let columns = info["columns"].as_array().unwrap();
        assert_eq!(columns.len(), dataset.columns.len());
        for (json, column) in columns.iter().zip(dataset.columns) {
            assert_eq!(json["name"], column.name);
            assert_eq!(json["pg_type"], column.ty.pg_name());
            assert_eq!(json["nullable"], column.nullable);
            assert_eq!(json["dictionary"], column.dictionary);
        }
    }
    assert_eq!(
        datasets[0]["columns"][0],
        json!({"name": "id", "pg_type": "int8", "arrow_type": "Int64", "nullable": false, "dictionary": false, "group": "numeric"})
    );
    assert_eq!(
        datasets[0]["columns"][1]["arrow_type"],
        "Timestamp(Microsecond, \"UTC\")"
    );
    let country = &datasets[1]["columns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "country_code")
        .unwrap();
    assert_eq!(country["dictionary"], true);
    assert_eq!(country["group"], "text");
}

#[tokio::test]
async fn row_cache_hits_on_repeated_requests() {
    let Some(app) = app().await else { return };
    let uri = "/api/data?rows=700&offset=1234&columns=id&cache=true";
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

    let other_dataset = app.get(&format!("{uri}&dataset=daily_sales")).await;
    assert_eq!(
        other_dataset.header("x-cache"),
        "miss",
        "same rows, offset and columns of another dataset"
    );
    assert_eq!(other_dataset.header("x-dataset"), "daily_sales");
    let other_columns = app
        .get("/api/data?rows=700&offset=1234&columns=id,email&cache=true")
        .await;
    assert_eq!(other_columns.header("x-cache"), "miss");
    let uncached = app.get("/api/data?rows=700&offset=1234&columns=id").await;
    assert_eq!(uncached.header("x-cache"), "off");

    // A seed that finished since the rows were cached drops the whole cache,
    // once a hit after the check interval has re-read the generation in the
    // background. Hits are served from the old generation until then.
    let client = app.client().await;
    client
        .execute(
            "UPDATE seed_status SET finished_at = clock_timestamp()",
            &[],
        )
        .await
        .unwrap();
    tokio::time::sleep(SEED_CHECK_INTERVAL).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    let after_reseed = loop {
        let response = app.get(uri).await;
        if response.header("x-cache") == "miss" || Instant::now() > deadline {
            break response;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(after_reseed.header("x-cache"), "miss");
    assert_eq!(after_reseed.body, first.body);
    assert_eq!(app.get(uri).await.header("x-cache"), "hit");
    let bench = app
        .get("/api/bench?rows=700&offset=1234&columns=id&cache=true")
        .await;
    assert_eq!(
        bench.header("x-cache"),
        "hit",
        "/api/bench shares the cache"
    );
}

/// A cache hit needs no database connection, so it neither queues for a
/// busy pool nor pays the round trip to a database in another region.
#[tokio::test]
async fn row_cache_hits_do_not_wait_for_the_database() {
    let Some(app) = app().await else { return };
    let router = router_with(&app.database, &static_dir(), 1);
    let cached = "/api/data?rows=300&offset=4321&columns=id,email&cache=true";
    let first = send(&router, Method::GET, cached, &[]).await;
    assert_eq!(first.header("x-cache"), "miss");

    // The pool's only connection waits for the lock in an uncached request.
    let locker = app.client().await;
    locker
        .batch_execute("BEGIN; LOCK TABLE orders IN ACCESS EXCLUSIVE MODE")
        .await
        .unwrap();
    let blocked = tokio::spawn({
        let router = router.clone();
        async move { send(&router, Method::GET, "/api/data?rows=10", &[]).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!blocked.is_finished(), "blocked by the lock");

    let hit = tokio::time::timeout(
        Duration::from_secs(5),
        send(&router, Method::GET, cached, &[]),
    )
    .await
    .expect("a cache hit does not wait for a database connection");
    assert_eq!(hit.header("x-cache"), "hit");
    assert_eq!(hit.body, first.body);
    assert!(hit.millis("x-db-ms") < 50.0, "{}", hit.header("x-db-ms"));

    locker.batch_execute("ROLLBACK").await.unwrap();
    let unblocked = blocked.await.unwrap();
    assert_eq!(unblocked.status, StatusCode::OK);
}

/// After a quiet spell longer than the max age, the first hit reads the
/// seed generation again before it is served, so it cannot serve rows of a
/// seed that finished meanwhile; with the rows unchanged it is still a hit.
#[tokio::test]
async fn row_cache_rereads_the_seed_after_a_quiet_spell() {
    let Some(app) = app().await else { return };
    let max_age = Duration::from_millis(500);
    let router = router_from(
        &app.database,
        &static_dir(),
        2,
        Settings {
            // No background re-read: only the max age can notice a seed.
            seed_check_interval: Duration::from_secs(3600),
            seed_check_max_age: max_age,
            ..settings()
        },
    );
    let uri = "/api/data?rows=200&offset=2345&columns=id&cache=true";
    let first = send(&router, Method::GET, uri, &[]).await;
    assert_eq!(first.header("x-cache"), "miss");

    tokio::time::sleep(max_age).await;
    let unchanged = send(&router, Method::GET, uri, &[]).await;
    assert_eq!(unchanged.header("x-cache"), "hit");
    assert_eq!(unchanged.body, first.body);

    app.client()
        .await
        .execute(
            "UPDATE seed_status SET finished_at = clock_timestamp()",
            &[],
        )
        .await
        .unwrap();
    tokio::time::sleep(max_age).await;
    let reseeded = send(&router, Method::GET, uri, &[]).await;
    assert_eq!(reseeded.header("x-cache"), "miss");
    assert_eq!(reseeded.body, first.body);
    assert_eq!(
        send(&router, Method::GET, uri, &[]).await.header("x-cache"),
        "hit"
    );
}

/// A request abandoned while its query waits must not leave that query
/// running on a pooled connection: with a pool of one, the next request
/// would queue behind it and report its wait as DB time.
#[tokio::test]
async fn an_abandoned_request_cancels_its_query() {
    let Some(app) = app().await else { return };
    let router = router_with(&app.database, &static_dir(), 1);
    let locker = app.client().await;
    locker
        .batch_execute("BEGIN; LOCK TABLE orders IN ACCESS EXCLUSIVE MODE")
        .await
        .unwrap();
    let monitor = app.client().await;
    // Running queries of the data endpoint on `orders` (not this one).
    let running = async || {
        count(
            &monitor,
            "SELECT count(*) FROM pg_stat_activity WHERE pid <> pg_backend_pid() \
             AND datname = current_database() AND state = 'active' \
             AND query LIKE 'SELECT %FROM \"orders\" WHERE \"id\" > $1%'",
        )
        .await
    };

    let request = tokio::spawn({
        let router = router.clone();
        async move { send(&router, Method::GET, "/api/data?rows=10", &[]).await }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while running().await == 0 {
        assert!(
            Instant::now() < deadline,
            "the request never reached Postgres"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!request.is_finished(), "blocked by the lock");
    // What hyper does when the client disconnects.
    request.abort();
    let Err(aborted) = request.await else {
        panic!("the request finished despite the lock");
    };
    assert!(aborted.is_cancelled());

    let deadline = Instant::now() + Duration::from_secs(10);
    while running().await > 0 {
        assert!(
            Instant::now() < deadline,
            "the abandoned query is still running"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // The lock is still held, but the next request does not wait for it.
    let next = tokio::time::timeout(
        Duration::from_secs(10),
        send(
            &router,
            Method::GET,
            "/api/data?dataset=daily_sales&rows=10",
            &[],
        ),
    )
    .await
    .expect("the next request is not stuck behind the abandoned query");
    assert_eq!(next.status, StatusCode::OK);
    assert!(
        next.millis("x-db-ms") < 2000.0,
        "{}",
        next.header("x-db-ms")
    );
    locker.batch_execute("ROLLBACK").await.unwrap();

    let after = send(&router, Method::GET, "/api/data?rows=10", &[]).await;
    assert_eq!(after.status, StatusCode::OK);
    assert_eq!(after.header("x-row-count"), "10");
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
        "x-compress-ms",
        "x-total-ms",
        "x-row-count",
        "x-column-count",
        "x-last-id",
        "x-body-bytes",
        "x-encoded-bytes",
        "x-format",
        "x-dataset",
        "x-cache",
        "content-encoding",
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
    assert_eq!(health.json(), json!({"status": "ok"}));

    let ping = app.get("/api/ping").await;
    assert_eq!(ping.status, StatusCode::NO_CONTENT);
    assert!(ping.body.is_empty());
    assert_eq!(ping.header("timing-allow-origin"), "*");

    let index = app.get("/").await;
    assert_eq!(index.status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&index.body).contains("<title>spa</title>"));
    assert_eq!(index.header("cache-control"), "no-cache");
    let client_route = app.get("/runs/42").await;
    assert_eq!(client_route.status, StatusCode::OK);
    assert_eq!(client_route.body, index.body, "SPA fallback");
    assert_eq!(client_route.header("cache-control"), "no-cache");

    let asset = app.get("/assets/app-3f2a.js").await;
    assert_eq!(asset.status, StatusCode::OK);
    assert_eq!(asset.body, "console.log('app')");
    assert_eq!(
        asset.header("cache-control"),
        "public, max-age=31536000, immutable"
    );
    let stale_chunk = app.get("/assets/app-0000.js").await;
    assert_eq!(
        stale_chunk.status,
        StatusCode::NOT_FOUND,
        "never the HTML page"
    );
    assert!(!String::from_utf8_lossy(&stale_chunk.body).contains("<title>"));
    assert!(
        stale_chunk
            .headers
            .get("cache-control")
            .is_none_or(|value| !value.to_str().unwrap().contains("immutable")),
        "a 404 is not cached for a year"
    );
    let compressed = app
        .request(Method::GET, "/", &[("accept-encoding", "gzip")])
        .await;
    assert_eq!(compressed.status, StatusCode::OK);

    let api_miss = app.get("/api/unknown/path").await;
    assert_eq!(api_miss.status, StatusCode::NOT_FOUND);
    assert_eq!(api_miss.header("content-type"), "application/json");

    let without_frontend = router_with(&app.database, Path::new("/nonexistent/dist"), 1);
    let note = send(&without_frontend, Method::GET, "/", &[]).await;
    assert_eq!(note.status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&note.body).contains("npm run build"));
}
