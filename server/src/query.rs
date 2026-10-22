//! Validated `/api/data` and `/api/bench` parameters, and the SQL they
//! become.

use std::collections::HashMap;
use std::fmt::Display;
use std::ops::RangeInclusive;
use std::str::FromStr;

use crate::encode::{Encoding, Format, IpcCompression, ParquetCompression};
use crate::schema::{ColumnSelection, TABLE};

pub const DEFAULT_ROWS: u64 = 10_000;
pub const DEFAULT_BENCH_ROWS: u64 = 100_000;
/// `/api/bench` never encodes more rows than this, whatever `MAX_ROWS` is.
pub const BENCH_ROW_LIMIT: u64 = 1_000_000;
pub const DEFAULT_BATCH_SIZE: usize = 65_536;
pub const BATCH_SIZE_RANGE: RangeInclusive<usize> = 1024..=1_048_576;
const OFFSET_RANGE: RangeInclusive<u64> = 0..=i64::MAX as u64;

/// A rejected parameter; the message is returned to the client as is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ParamError(String);

/// Decoded query string. Unknown keys are kept but never read, a repeated key
/// keeps its last value, and an empty value counts as absent.
#[derive(Debug, Default)]
pub struct Params(HashMap<String, String>);

impl Params {
    pub fn parse(query: &str) -> Self {
        Self(
            form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect(),
        )
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    }

    fn bool(&self, key: &str, default: bool) -> Result<bool, ParamError> {
        self.get(key).map_or(Ok(default), |value| {
            parse_bool(value).ok_or_else(|| {
                ParamError(format!(
                    "{key} must be a boolean (true/false, 1/0, on/off, yes/no), got \"{value}\""
                ))
            })
        })
    }

    fn integer<T>(&self, key: &str, default: T, range: RangeInclusive<T>) -> Result<T, ParamError>
    where
        T: FromStr + PartialOrd + Display + Copy,
    {
        let Some(value) = self.get(key) else {
            return Ok(default);
        };
        value
            .parse::<T>()
            .ok()
            .filter(|parsed| range.contains(parsed))
            .ok_or_else(|| {
                ParamError(format!(
                    "{key} must be an integer from {} to {}, got \"{value}\"",
                    range.start(),
                    range.end()
                ))
            })
    }

    fn keyword<T>(&self, key: &str, default: T) -> Result<T, ParamError>
    where
        T: FromStr,
        T::Err: Display,
    {
        self.get(key).map_or(Ok(default), |value| {
            value
                .parse()
                .map_err(|err: T::Err| ParamError(err.to_string()))
        })
    }

    fn columns(&self) -> Result<ColumnSelection, ParamError> {
        ColumnSelection::parse(self.get("columns").unwrap_or_default())
            .map_err(|err| ParamError(err.to_string()))
    }

    fn rows_query(&self, default_rows: u64, max_rows: u64) -> Result<RowsQuery, ParamError> {
        let rows = self.integer("rows", default_rows.min(max_rows), 1..=max_rows)?;
        let offset = self.integer("offset", 0, OFFSET_RANGE)?;
        Ok(RowsQuery {
            after_id: offset as i64,
            limit: rows as i64,
            columns: self.columns()?,
        })
    }
}

/// Accepts `true/false`, `1/0`, `on/off` and `yes/no`, in any case.
pub fn parse_bool(value: &str) -> Option<bool> {
    const TRUE: [&str; 4] = ["true", "1", "on", "yes"];
    const FALSE: [&str; 4] = ["false", "0", "off", "no"];
    if TRUE.iter().any(|t| t.eq_ignore_ascii_case(value)) {
        Some(true)
    } else if FALSE.iter().any(|f| f.eq_ignore_ascii_case(value)) {
        Some(false)
    } else {
        None
    }
}

/// Which rows to read. Also the row cache key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RowsQuery {
    /// Keyset pagination: rows with `id > after_id`.
    pub after_id: i64,
    pub limit: i64,
    pub columns: ColumnSelection,
}

impl RowsQuery {
    /// `SELECT <cols> FROM orders WHERE id > $1 ORDER BY id LIMIT $2`, with
    /// column names taken from the catalog, never from the request.
    ///
    /// When `id` is not selected it is appended as an extra last column, so
    /// the id of the last row is always known; encoders only look at the
    /// selected columns.
    pub fn sql(&self) -> String {
        let mut names: Vec<String> = self.columns.columns().iter().map(|c| c.quoted()).collect();
        if self.columns.position("id").is_none() {
            names.push("\"id\"".to_owned());
        }
        format!(
            "SELECT {} FROM {TABLE} WHERE \"id\" > $1 ORDER BY \"id\" LIMIT $2",
            names.join(", ")
        )
    }

    /// Index of `id` in rows returned by [`Self::sql`].
    pub fn id_index(&self) -> usize {
        self.columns.position("id").unwrap_or(self.columns.len())
    }
}

/// A validated `/api/data` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataRequest {
    pub rows: RowsQuery,
    pub encoding: Encoding,
    pub http_compression: bool,
    pub cache: bool,
}

impl DataRequest {
    /// Validates every known parameter, including options that do not apply
    /// to the chosen format, so a typo never goes unnoticed.
    pub fn parse(params: &Params, max_rows: u64) -> Result<Self, ParamError> {
        let format = params.keyword("format", Format::Json)?;
        let rows = params.rows_query(DEFAULT_ROWS, max_rows)?;
        let http_compression = params.bool("http_compression", true)?;
        let dictionary = params.bool("dictionary", false)?;
        let ipc_compression = params.keyword("ipc_compression", IpcCompression::None)?;
        let parquet_compression =
            params.keyword("parquet_compression", ParquetCompression::Snappy)?;
        let batch_size = params.integer("batch_size", DEFAULT_BATCH_SIZE, BATCH_SIZE_RANGE)?;
        let cache = params.bool("cache", false)?;

        let encoding = match format {
            Format::Json => Encoding::Json,
            Format::Arrow => Encoding::Arrow {
                compression: ipc_compression,
                dictionary,
                batch_size,
            },
            Format::Parquet => Encoding::Parquet {
                compression: parquet_compression,
                batch_size,
            },
        };
        Ok(Self {
            rows,
            encoding,
            http_compression,
            cache,
        })
    }
}

/// A validated `/api/bench` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchRequest {
    pub rows: RowsQuery,
    pub cache: bool,
}

impl BenchRequest {
    pub fn parse(params: &Params, max_rows: u64) -> Result<Self, ParamError> {
        Ok(Self {
            rows: params.rows_query(DEFAULT_BENCH_ROWS, max_rows.min(BENCH_ROW_LIMIT))?,
            cache: params.bool("cache", false)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::COLUMNS;

    const MAX: u64 = 1_000_000;

    fn data(query: &str) -> Result<DataRequest, ParamError> {
        DataRequest::parse(&Params::parse(query), MAX)
    }

    fn error(query: &str) -> String {
        data(query).unwrap_err().to_string()
    }

    #[test]
    fn defaults_follow_the_design() {
        let request = data("").unwrap();
        assert_eq!(request.encoding, Encoding::Json);
        assert_eq!(request.rows.limit, 10_000);
        assert_eq!(request.rows.after_id, 0);
        assert_eq!(request.rows.columns.len(), COLUMNS.len());
        assert!(request.http_compression);
        assert!(!request.cache);

        let arrow = data("format=arrow").unwrap();
        assert_eq!(
            arrow.encoding,
            Encoding::Arrow {
                compression: IpcCompression::None,
                dictionary: false,
                batch_size: 65_536
            }
        );
        let parquet = data("format=parquet").unwrap();
        assert_eq!(
            parquet.encoding,
            Encoding::Parquet {
                compression: ParquetCompression::Snappy,
                batch_size: 65_536
            }
        );
    }

    #[test]
    fn parses_every_option() {
        let request = data(
            "format=arrow&rows=500&offset=1000&columns=total,id&http_compression=off\
             &dictionary=yes&ipc_compression=zstd&batch_size=2048&cache=1&_=123",
        )
        .unwrap();
        assert_eq!(request.rows.limit, 500);
        assert_eq!(request.rows.after_id, 1000);
        assert_eq!(
            request.rows.columns,
            ColumnSelection::parse("total,id").unwrap()
        );
        assert!(!request.http_compression);
        assert!(request.cache);
        assert_eq!(
            request.encoding,
            Encoding::Arrow {
                compression: IpcCompression::Zstd,
                dictionary: true,
                batch_size: 2048
            }
        );
    }

    #[test]
    fn booleans_accept_all_spellings() {
        for (value, expected) in [
            ("true", true),
            ("TRUE", true),
            ("1", true),
            ("on", true),
            ("yes", true),
            ("false", false),
            ("0", false),
            ("Off", false),
            ("no", false),
        ] {
            assert_eq!(parse_bool(value), Some(expected), "{value}");
        }
        assert_eq!(parse_bool("maybe"), None);
        assert!(error("cache=maybe").contains("cache must be a boolean"));
    }

    #[test]
    fn rows_must_be_within_one_and_max_rows() {
        assert!(error("rows=0").contains("rows must be an integer from 1 to 1000000"));
        assert!(error("rows=1000001").contains("rows"));
        assert!(error("rows=-5").contains("rows"));
        assert!(error("rows=ten").contains("rows"));
        assert_eq!(data("rows=1").unwrap().rows.limit, 1);
        assert_eq!(data("rows=1000000").unwrap().rows.limit, 1_000_000);
    }

    #[test]
    fn default_rows_never_exceed_max_rows() {
        let request = DataRequest::parse(&Params::parse(""), 500).unwrap();
        assert_eq!(request.rows.limit, 500);
    }

    #[test]
    fn batch_size_is_bounded() {
        assert!(
            error("batch_size=1023").contains("batch_size must be an integer from 1024 to 1048576")
        );
        assert!(error("batch_size=1048577").contains("batch_size"));
        assert!(data("batch_size=1024").is_ok());
        assert!(data("batch_size=1048576").is_ok());
    }

    #[test]
    fn offset_must_fit_a_bigint() {
        assert_eq!(
            data("offset=9223372036854775807").unwrap().rows.after_id,
            i64::MAX
        );
        assert!(error("offset=9223372036854775808").contains("offset"));
        assert!(error("offset=-1").contains("offset"));
    }

    #[test]
    fn keywords_are_validated_for_every_format() {
        assert!(error("format=csv").contains("unknown format \"csv\""));
        assert!(error("ipc_compression=snappy").contains("unknown ipc_compression"));
        assert!(
            error("format=json&parquet_compression=lzo").contains("unknown parquet_compression")
        );
    }

    #[test]
    fn columns_are_whitelisted() {
        assert!(error("columns=id,password").contains("unknown column \"password\""));
        assert!(error("columns=id;DROP TABLE orders").contains("unknown column"));
    }

    #[test]
    fn empty_values_and_unknown_params_are_ignored() {
        let request = data("rows=&format=&columns=&nonce=abc&cache=").unwrap();
        assert_eq!(request, data("").unwrap());
    }

    #[test]
    fn repeated_params_keep_the_last_value() {
        assert_eq!(data("rows=5&rows=7").unwrap().rows.limit, 7);
    }

    #[test]
    fn values_are_url_decoded() {
        let request = data("columns=total%2Cid&format=%20parquet%20").unwrap();
        assert_eq!(request.rows.columns.len(), 2);
        assert_eq!(request.encoding.format(), Format::Parquet);
    }

    #[test]
    fn sql_uses_quoted_catalog_names_and_bind_parameters() {
        let query = data("columns=total,country").unwrap().rows;
        assert_eq!(
            query.sql(),
            "SELECT \"total\", \"country\", \"id\" FROM orders WHERE \"id\" > $1 ORDER BY \"id\" LIMIT $2"
        );
        assert_eq!(query.id_index(), 2);

        let with_id = data("columns=country,id").unwrap().rows;
        assert_eq!(
            with_id.sql(),
            "SELECT \"country\", \"id\" FROM orders WHERE \"id\" > $1 ORDER BY \"id\" LIMIT $2"
        );
        assert_eq!(with_id.id_index(), 1);
    }

    #[test]
    fn bench_has_its_own_defaults_and_cap() {
        let bench = BenchRequest::parse(&Params::parse(""), MAX).unwrap();
        assert_eq!(bench.rows.limit, 100_000);
        let capped = BenchRequest::parse(&Params::parse("rows=2000000"), 5_000_000);
        assert!(
            capped
                .unwrap_err()
                .to_string()
                .contains("from 1 to 1000000")
        );
        let small = BenchRequest::parse(&Params::parse(""), 50_000).unwrap();
        assert_eq!(small.rows.limit, 50_000);
    }
}
