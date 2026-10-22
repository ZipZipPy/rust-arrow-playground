//! Response encoders: JSON straight from Postgres rows, and Arrow IPC or
//! Parquet from Arrow record batches.

pub mod ipc;
pub mod json;
pub mod parquet;

use std::fmt;
use std::io::Write;
use std::str::FromStr;

use ::parquet::errors::ParquetError;
use arrow::error::ArrowError;
use serde::{Serialize, Serializer};
use tokio_postgres::Row;

use crate::convert::{self, ConvertError};
use crate::schema::Column;

/// Rejection of a keyword parameter, listing what would have been accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {what} \"{value}\" (expected one of: {expected})")]
pub struct UnknownValue {
    what: &'static str,
    value: String,
    expected: String,
}

/// Declares a fieldless enum whose variants have fixed lowercase names, used
/// in query strings, CLI flags and JSON alike.
macro_rules! keyword_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident ($what:literal) {
            $($variant:ident = $text:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl FromStr for $name {
            type Err = UnknownValue;

            fn from_str(value: &str) -> Result<Self, UnknownValue> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|variant| variant.as_str().eq_ignore_ascii_case(value))
                    .ok_or_else(|| UnknownValue {
                        what: $what,
                        value: value.to_owned(),
                        expected: Self::ALL.iter().map(|v| v.as_str()).collect::<Vec<_>>().join(", "),
                    })
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl clap::ValueEnum for $name {
            fn value_variants<'a>() -> &'a [Self] {
                Self::ALL
            }

            fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
                Some(clap::builder::PossibleValue::new(self.as_str()))
            }
        }
    };
}

keyword_enum! {
    /// Response body format.
    pub enum Format ("format") {
        Json = "json",
        Arrow = "arrow",
        Parquet = "parquet",
    }
}

keyword_enum! {
    /// Arrow IPC body compression.
    pub enum IpcCompression ("ipc_compression") {
        None = "none",
        Lz4 = "lz4",
        Zstd = "zstd",
    }
}

keyword_enum! {
    /// Parquet column chunk compression; `lz4` is LZ4_RAW.
    pub enum ParquetCompression ("parquet_compression") {
        None = "none",
        Snappy = "snappy",
        Gzip = "gzip",
        Zstd = "zstd",
        Lz4 = "lz4",
        Brotli = "brotli",
    }
}

impl Format {
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Json => "application/json",
            Self::Arrow => "application/vnd.apache.arrow.stream",
            Self::Parquet => "application/vnd.apache.parquet",
        }
    }
}

/// A format together with its options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Json,
    Arrow {
        compression: IpcCompression,
        dictionary: bool,
        batch_size: usize,
    },
    Parquet {
        compression: ParquetCompression,
        /// Maximum rows per row group.
        batch_size: usize,
    },
}

/// Arrow IPC framing: the API serves streams, `export` writes files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowContainer {
    Stream,
    File,
}

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error(transparent)]
    Convert(#[from] ConvertError),
    #[error("JSON encoding failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Arrow IPC encoding failed: {0}")]
    Ipc(#[from] ArrowError),
    #[error("Parquet encoding failed: {0}")]
    Parquet(#[from] ParquetError),
}

impl Encoding {
    pub const fn format(&self) -> Format {
        match self {
            Self::Json => Format::Json,
            Self::Arrow { .. } => Format::Arrow,
            Self::Parquet { .. } => Format::Parquet,
        }
    }

    /// Short human-readable name, e.g. `arrow · zstd · dict`.
    pub fn label(&self) -> String {
        match self {
            Self::Json => "json".to_owned(),
            Self::Arrow {
                compression,
                dictionary,
                ..
            } => {
                let dict = if *dictionary { " · dict" } else { "" };
                format!("arrow · {compression}{dict}")
            }
            Self::Parquet { compression, .. } => format!("parquet · {compression}"),
        }
    }

    /// Every variant `/api/bench` measures: JSON, Arrow for each IPC
    /// compression with and without dictionaries, Parquet for each codec.
    pub fn bench_variants(batch_size: usize) -> Vec<Encoding> {
        let arrow = IpcCompression::ALL.iter().flat_map(|&compression| {
            [false, true].map(|dictionary| Self::Arrow {
                compression,
                dictionary,
                batch_size,
            })
        });
        let parquet = ParquetCompression::ALL
            .iter()
            .map(|&compression| Self::Parquet {
                compression,
                batch_size,
            });
        std::iter::once(Self::Json)
            .chain(arrow)
            .chain(parquet)
            .collect()
    }
}

/// Encodes `rows` (whose first `columns.len()` fields are `columns`) into an
/// in-memory response body.
pub fn encode(
    rows: &[Row],
    columns: &[&'static Column],
    encoding: &Encoding,
) -> Result<Vec<u8>, EncodeError> {
    let capacity = match encoding {
        // JSON is large and growing a huge Vec copies it repeatedly.
        Encoding::Json => rows.len() * json::row_size_hint(columns),
        _ => 0,
    };
    encode_to(
        Vec::with_capacity(capacity),
        rows,
        columns,
        encoding,
        ArrowContainer::Stream,
    )
}

/// Encodes `rows` into `out` and returns it.
pub fn encode_to<W: Write + Send>(
    out: W,
    rows: &[Row],
    columns: &[&'static Column],
    encoding: &Encoding,
    container: ArrowContainer,
) -> Result<W, EncodeError> {
    match *encoding {
        Encoding::Json => Ok(json::write(out, rows, columns)?),
        Encoding::Arrow {
            compression,
            dictionary,
            batch_size,
        } => {
            let (schema, batches) = convert::record_batches(rows, columns, batch_size, dictionary)?;
            let out = match container {
                ArrowContainer::Stream => ipc::write_stream(out, &schema, &batches, compression)?,
                ArrowContainer::File => ipc::write_file(out, &schema, &batches, compression)?,
            };
            Ok(out)
        }
        Encoding::Parquet {
            compression,
            batch_size,
        } => {
            // Parquet does its own dictionary encoding, so strings stay Utf8.
            let (schema, batches) = convert::record_batches(rows, columns, batch_size, false)?;
            Ok(parquet::write(
                out,
                schema,
                &batches,
                compression,
                batch_size,
            )?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_parse_case_insensitively() {
        assert_eq!("ARROW".parse::<Format>().unwrap(), Format::Arrow);
        assert_eq!(
            "lz4".parse::<IpcCompression>().unwrap(),
            IpcCompression::Lz4
        );
        assert_eq!(
            "Brotli".parse::<ParquetCompression>().unwrap(),
            ParquetCompression::Brotli
        );
    }

    #[test]
    fn unknown_keywords_list_the_alternatives() {
        let err = "xml".parse::<Format>().unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown format \"xml\" (expected one of: json, arrow, parquet)"
        );
    }

    #[test]
    fn bench_covers_every_variant_once() {
        let variants = Encoding::bench_variants(65_536);
        assert_eq!(variants.len(), 1 + 3 * 2 + 6);
        let labels: Vec<String> = variants.iter().map(Encoding::label).collect();
        assert_eq!(labels[0], "json");
        assert!(labels.contains(&"arrow · zstd · dict".to_owned()));
        assert!(labels.contains(&"arrow · none".to_owned()));
        assert!(labels.contains(&"parquet · snappy".to_owned()));
        let mut unique = labels.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), labels.len());
    }
}
