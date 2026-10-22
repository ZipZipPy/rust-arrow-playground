//! Command-line flags and their environment variables.

use std::path::PathBuf;

use clap::builder::BoolishValueParser;
use clap::{ArgAction, Args, ValueEnum};
use serde::Serialize;

use crate::db;
use crate::encode::Format;
use crate::query::BATCH_SIZE_RANGE;

fn database_config(value: &str) -> Result<tokio_postgres::Config, String> {
    db::parse_config(value).map_err(|err| format!("invalid database URL: {err}"))
}

fn batch_size(value: &str) -> Result<usize, String> {
    value
        .parse()
        .ok()
        .filter(|size| BATCH_SIZE_RANGE.contains(size))
        .ok_or_else(|| {
            format!(
                "must be an integer from {} to {}",
                BATCH_SIZE_RANGE.start(),
                BATCH_SIZE_RANGE.end()
            )
        })
}

/// `playground serve`
#[derive(Debug, Clone, Args)]
pub struct ServeConfig {
    #[arg(long, env = "PORT", default_value_t = 8080)]
    pub port: u16,

    /// PostgreSQL connection string, URL or key/value form.
    #[arg(long = "database-url", env = "DATABASE_URL", hide_env_values = true, value_parser = database_config)]
    pub database: tokio_postgres::Config,

    /// Built frontend served for every path outside /api.
    #[arg(long, env = "STATIC_DIR", default_value = "./web/dist")]
    pub static_dir: PathBuf,

    /// Upper bound of the `rows` parameter.
    #[arg(long, env = "MAX_ROWS", default_value_t = 1_000_000, value_parser = clap::value_parser!(u64).range(1..))]
    pub max_rows: u64,

    #[arg(long, env = "DB_POOL_SIZE", default_value_t = 16, value_parser = clap::value_parser!(u16).range(1..))]
    pub pool_size: u16,

    /// Row cache: maximum number of cached results.
    #[arg(long, env = "CACHE_ENTRIES", default_value_t = 8)]
    pub cache_entries: usize,

    /// Row cache: maximum number of rows across all cached results.
    #[arg(long, env = "CACHE_MAX_ROWS", default_value_t = 1_000_000)]
    pub cache_max_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SeedMode {
    /// Drop and recreate the table.
    Replace,
    /// Add rows after max(id).
    Append,
}

/// `playground seed`
#[derive(Debug, Clone, Args)]
pub struct SeedConfig {
    /// Rows to generate.
    #[arg(long, env = "SEED_ROWS", default_value_t = 1_000_000)]
    pub rows: u64,

    /// Parallel COPY connections.
    #[arg(long, env = "SEED_WORKERS", default_value_t = 4, value_parser = clap::value_parser!(u16).range(1..=256))]
    pub workers: u16,

    /// Rows per COPY chunk.
    #[arg(long, env = "SEED_BATCH_SIZE", default_value_t = 50_000, value_parser = clap::value_parser!(u64).range(1..))]
    pub batch_size: u64,

    #[arg(long, env = "SEED_MODE", value_enum, default_value_t = SeedMode::Replace)]
    pub mode: SeedMode,

    /// Do nothing if the table already has rows.
    #[arg(long, env = "SEED_IF_EMPTY", action = ArgAction::SetTrue, value_parser = BoolishValueParser::new())]
    pub if_empty: bool,

    /// RNG seed; the same seed, rows and batch size give the same data.
    #[arg(long = "seed", env = "SEED_RNG", default_value_t = 42)]
    pub rng_seed: u64,

    /// PostgreSQL connection string, URL or key/value form.
    #[arg(long = "database-url", env = "DATABASE_URL", hide_env_values = true, value_parser = database_config)]
    pub database: tokio_postgres::Config,
}

/// `playground export`
#[derive(Debug, Clone, Args)]
pub struct ExportConfig {
    #[arg(long, value_enum)]
    pub format: Format,

    /// Output file.
    #[arg(long)]
    pub out: PathBuf,

    #[arg(long, default_value_t = crate::query::DEFAULT_ROWS, value_parser = clap::value_parser!(u64).range(1..))]
    pub rows: u64,

    /// Export rows with id > OFFSET.
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u64).range(0..=i64::MAX as u64))]
    pub offset: u64,

    /// Comma-separated column names (default: all).
    #[arg(long)]
    pub columns: Option<String>,

    /// arrow: none|lz4|zstd (default none); parquet: none|snappy|gzip|zstd|lz4|brotli (default snappy).
    #[arg(long)]
    pub compression: Option<String>,

    /// arrow: dictionary-encode the low-cardinality text columns.
    #[arg(long)]
    pub dictionary: bool,

    /// arrow: rows per record batch; parquet: maximum row-group size.
    #[arg(long, default_value_t = crate::query::DEFAULT_BATCH_SIZE, value_parser = batch_size)]
    pub batch_size: usize,

    /// PostgreSQL connection string, URL or key/value form.
    #[arg(long = "database-url", env = "DATABASE_URL", hide_env_values = true, value_parser = database_config)]
    pub database: tokio_postgres::Config,
}

/// Where the server runs, reported by `/api/info`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ServerInfo {
    pub version: &'static str,
    /// `$REGION`
    pub region: Option<String>,
    /// `$K_SERVICE`, set by Cloud Run.
    pub service: Option<String>,
    /// `$K_REVISION`, set by Cloud Run.
    pub revision: Option<String>,
}

impl ServerInfo {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        Self {
            version: env!("CARGO_PKG_VERSION"),
            region: var("REGION"),
            service: var("K_SERVICE"),
            revision: var("K_REVISION"),
        }
    }
}
