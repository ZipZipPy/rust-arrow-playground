//! `/api/bench`: encode the same rows in every variant and report time and
//! size, with no network involved.

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::db::FetchedRows;
use crate::encode::{self, EncodeError, Encoding, Format, IpcCompression, ParquetCompression};
use crate::query::DEFAULT_BATCH_SIZE;

#[derive(Debug, Serialize)]
pub struct BenchReport {
    pub rows: usize,
    pub columns: usize,
    pub db_ms: f64,
    pub results: Vec<BenchResult>,
}

#[derive(Debug, Serialize)]
pub struct BenchResult {
    pub format: Format,
    pub variant: String,
    pub options: VariantOptions,
    pub encode_ms: f64,
    pub bytes: usize,
}

/// The options that distinguish a variant; empty for JSON.
#[derive(Debug, Default, Serialize)]
pub struct VariantOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipc_compression: Option<IpcCompression>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dictionary: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parquet_compression: Option<ParquetCompression>,
}

impl From<Encoding> for VariantOptions {
    fn from(encoding: Encoding) -> Self {
        match encoding {
            Encoding::Json => Self::default(),
            Encoding::Arrow {
                compression,
                dictionary,
                ..
            } => Self {
                ipc_compression: Some(compression),
                dictionary: Some(dictionary),
                ..Self::default()
            },
            Encoding::Parquet { compression, .. } => Self {
                parquet_compression: Some(compression),
                ..Self::default()
            },
        }
    }
}

/// Encodes `fetched` once per variant, one after another, so each
/// measurement has the machine to itself. Blocking: call it off the runtime.
pub fn run(fetched: &FetchedRows) -> Result<Vec<BenchResult>, EncodeError> {
    let columns = fetched.columns.columns();
    Encoding::bench_variants(DEFAULT_BATCH_SIZE)
        .into_iter()
        .map(|encoding| {
            let started = Instant::now();
            let bytes = encode::encode(&fetched.rows, columns, &encoding)?.len();
            Ok(BenchResult {
                format: encoding.format(),
                variant: encoding.label(),
                options: encoding.into(),
                encode_ms: millis_f64(started.elapsed()),
                bytes,
            })
        })
        .collect()
}

/// Milliseconds rounded to three decimals.
pub fn millis_f64(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1_000_000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_name_only_what_distinguishes_a_variant() {
        let json = serde_json::to_string(&VariantOptions::from(Encoding::Json)).unwrap();
        assert_eq!(json, "{}");

        let arrow = VariantOptions::from(Encoding::Arrow {
            compression: IpcCompression::Zstd,
            dictionary: true,
            batch_size: 1024,
        });
        assert_eq!(
            serde_json::to_string(&arrow).unwrap(),
            r#"{"ipc_compression":"zstd","dictionary":true}"#
        );

        let parquet = VariantOptions::from(Encoding::Parquet {
            compression: ParquetCompression::Snappy,
            batch_size: 1024,
        });
        assert_eq!(
            serde_json::to_string(&parquet).unwrap(),
            r#"{"parquet_compression":"snappy"}"#
        );
    }

    #[test]
    fn rounds_to_microseconds() {
        assert_eq!(millis_f64(Duration::from_nanos(45_612_345)), 45.612);
    }
}
