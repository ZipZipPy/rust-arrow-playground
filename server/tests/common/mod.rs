//! Shared helpers for the database-backed integration tests.
//!
//! The tests need `TEST_DATABASE_URL`; without it they print a note and pass.
//! Each test binary works in its own Postgres schemas (via `search_path`), so
//! binaries never touch each other's tables, and a binary's tests take
//! [`DB_LOCK`] because they share them (and the seed lock is per database).

#![allow(dead_code)]

use arrow::array::{Array, AsArray};
use arrow::datatypes::{
    DataType, Date32Type, Float32Type, Float64Type, Int16Type, Int32Type, Int64Type, TimeUnit,
    TimestampMicrosecondType,
};
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use playground::config::{SeedConfig, SeedMode};
use playground::db;
use playground::schema::{Column, ColumnType};
use serde_json::Value;

pub static DB_LOCK: tokio::sync::Mutex<bool> = tokio::sync::Mutex::const_new(false);

/// Connection settings for `schema` inside `TEST_DATABASE_URL`, creating
/// the schema if needed. `None` (after a note on stderr) when the variable
/// is not set.
pub async fn database(schema: &str) -> Option<tokio_postgres::Config> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("skipping: set TEST_DATABASE_URL to run the database tests");
        return None;
    };
    let mut config =
        db::parse_config(&url).expect("TEST_DATABASE_URL is a valid connection string");
    let client = db::connect(&config)
        .await
        .expect("connect to TEST_DATABASE_URL");
    client
        .batch_execute(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"))
        .await
        .expect("create test schema");
    config.options(format!("-c search_path={schema}"));
    Some(config)
}

/// Like [`database`], but `schema` is dropped and created empty first.
pub async fn fresh_database(schema: &str) -> Option<tokio_postgres::Config> {
    let config = database(schema).await?;
    let client = db::connect(&config)
        .await
        .expect("connect to the test schema");
    client
        .batch_execute(&format!(
            "SET client_min_messages = warning; DROP SCHEMA {schema} CASCADE; CREATE SCHEMA {schema}"
        ))
        .await
        .expect("recreate test schema");
    Some(config)
}

/// The single `bigint` that `sql` returns.
pub async fn count(client: &tokio_postgres::Client, sql: &str) -> i64 {
    client
        .query_one(sql, &[])
        .await
        .unwrap_or_else(|err| panic!("{sql}: {err}"))
        .get(0)
}

pub fn seed_config(database: &tokio_postgres::Config, rows: u64, workers: u16) -> SeedConfig {
    SeedConfig {
        rows,
        workers,
        batch_size: 700,
        mode: SeedMode::Replace,
        if_empty: false,
        rng_seed: 11,
        database: database.clone(),
    }
}

/// Asserts that `batches` hold exactly the rows of the JSON array `json`:
/// same columns in the same order, same values, same nulls.
pub fn assert_batches_match_json(batches: &[RecordBatch], json: &[Value]) {
    let total: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(total, json.len(), "row count");
    let mut row = 0;
    for batch in batches {
        let schema = batch.schema();
        for i in 0..batch.num_rows() {
            let object = json[row].as_object().expect("rows are objects");
            let keys: Vec<&String> = object.keys().collect();
            let names: Vec<&String> = schema.fields().iter().map(|f| f.name()).collect();
            assert_eq!(keys, names, "column order of row {row}");
            for (field, array) in schema.fields().iter().zip(batch.columns()) {
                assert_cell(field.name(), array.as_ref(), i, &object[field.name()], row);
            }
            row += 1;
        }
    }
}

fn assert_cell(name: &str, array: &dyn Array, i: usize, expected: &Value, row: usize) {
    let context = format!("column {name}, row {row}");
    if array.is_null(i) {
        assert!(expected.is_null(), "{context}: Arrow null, JSON {expected}");
        return;
    }
    assert!(
        !expected.is_null(),
        "{context}: JSON null, Arrow has a value"
    );
    match array.data_type() {
        DataType::Int64 => assert_eq!(
            Some(array.as_primitive::<Int64Type>().value(i)),
            expected.as_i64(),
            "{context}"
        ),
        DataType::Int32 => assert_eq!(
            Some(i64::from(array.as_primitive::<Int32Type>().value(i))),
            expected.as_i64(),
            "{context}"
        ),
        DataType::Int16 => assert_eq!(
            Some(i64::from(array.as_primitive::<Int16Type>().value(i))),
            expected.as_i64(),
            "{context}"
        ),
        DataType::Float64 => assert_eq!(
            Some(array.as_primitive::<Float64Type>().value(i)),
            expected.as_f64(),
            "{context}"
        ),
        // JSON prints the shortest text that round-trips the f32.
        DataType::Float32 => assert_eq!(
            Some(array.as_primitive::<Float32Type>().value(i)),
            expected.as_f64().map(|v| v as f32),
            "{context}"
        ),
        DataType::Boolean => assert_eq!(
            Some(array.as_boolean().value(i)),
            expected.as_bool(),
            "{context}"
        ),
        DataType::Utf8 => assert_eq!(
            Some(array.as_string::<i32>().value(i)),
            expected.as_str(),
            "{context}"
        ),
        DataType::Dictionary(key, value)
            if **key == DataType::Int32 && **value == DataType::Utf8 =>
        {
            let dictionary = array.as_dictionary::<Int32Type>();
            let key = dictionary.keys().value(i) as usize;
            let text = dictionary.values().as_string::<i32>().value(key);
            assert_eq!(Some(text), expected.as_str(), "{context}");
        }
        DataType::Timestamp(TimeUnit::Microsecond, Some(tz)) if tz.as_ref() == "UTC" => {
            let micros = array.as_primitive::<TimestampMicrosecondType>().value(i);
            let text = DateTime::from_timestamp_micros(micros)
                .expect("timestamp in range")
                .to_rfc3339_opts(SecondsFormat::Micros, true);
            assert_eq!(Some(text.as_str()), expected.as_str(), "{context}");
        }
        DataType::Date32 => {
            let days = array.as_primitive::<Date32Type>().value(i);
            let date = NaiveDate::from_num_days_from_ce_opt(days + 719_163).expect("date in range");
            assert_eq!(
                Some(date.format("%Y-%m-%d").to_string().as_str()),
                expected.as_str(),
                "{context}"
            );
        }
        other => panic!("{context}: unexpected Arrow type {other}"),
    }
}

/// Header type of every message in an Arrow IPC stream, in order.
pub fn ipc_message_kinds(stream: &[u8]) -> Vec<arrow::ipc::MessageHeader> {
    const CONTINUATION: [u8; 4] = [0xff; 4];
    let mut kinds = Vec::new();
    let mut position = 0;
    loop {
        if stream[position..position + 4] == CONTINUATION {
            position += 4;
        }
        let length =
            i32::from_le_bytes(stream[position..position + 4].try_into().unwrap()) as usize;
        position += 4;
        if length == 0 {
            assert_eq!(position, stream.len(), "nothing follows end-of-stream");
            return kinds;
        }
        let message = arrow::ipc::root_as_message(&stream[position..position + length])
            .expect("valid IPC message");
        kinds.push(message.header_type());
        position += length + message.bodyLength() as usize;
    }
}

/// Asserts that the JSON rows `json` hold what tokio-postgres reads for
/// `columns` from `rows`: an independent check of the server's own wire
/// decoders for timestamps, dates and REALs.
pub fn assert_json_matches_postgres(
    json: &[Value],
    rows: &[tokio_postgres::Row],
    columns: &[Column],
) {
    assert_eq!(json.len(), rows.len(), "row count");
    for (row, (object, db_row)) in json.iter().zip(rows).enumerate() {
        for (i, column) in columns.iter().enumerate() {
            let context = format!("column {}, row {row}", column.name);
            let expected = postgres_value(db_row, i, column.ty);
            let actual = &object[column.name];
            match (column.ty, actual.as_f64(), expected.as_f64()) {
                // JSON prints the shortest text that round-trips the f32.
                (ColumnType::Float32, Some(actual), Some(expected)) => {
                    assert_eq!(actual as f32, expected as f32, "{context}");
                }
                // `2.0` may be printed as `2`, which parses as an integer.
                (ColumnType::Float64, Some(actual), Some(expected)) => {
                    assert_eq!(actual, expected, "{context}");
                }
                _ => assert_eq!(actual, &expected, "{context}"),
            }
        }
    }
}

fn postgres_value(row: &tokio_postgres::Row, i: usize, ty: ColumnType) -> Value {
    match ty {
        ColumnType::Int64 => row.get::<_, Option<i64>>(i).into(),
        ColumnType::Int32 => row.get::<_, Option<i32>>(i).into(),
        ColumnType::Int16 => row.get::<_, Option<i16>>(i).into(),
        ColumnType::Float64 => row.get::<_, Option<f64>>(i).into(),
        ColumnType::Float32 => row.get::<_, Option<f32>>(i).map(f64::from).into(),
        ColumnType::Boolean => row.get::<_, Option<bool>>(i).into(),
        ColumnType::Text => row.get::<_, Option<String>>(i).into(),
        ColumnType::Timestamp => row
            .get::<_, Option<DateTime<Utc>>>(i)
            .map(|t| t.to_rfc3339_opts(SecondsFormat::Micros, true))
            .into(),
        ColumnType::Date => row
            .get::<_, Option<NaiveDate>>(i)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .into(),
    }
}
