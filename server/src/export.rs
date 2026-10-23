//! `playground export`: write JSON, an Arrow IPC file or a Parquet file from
//! the database with the same encoders as the API.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Instant;

use anyhow::{Context, bail};
use tracing::info;

use crate::config::ExportConfig;
use crate::db;
use crate::encode::{
    self, ArrowContainer, CancelFlag, Encoding, Format, IpcCompression, ParquetCompression,
};
use crate::query::RowsQuery;
use crate::schema::ColumnSelection;

/// The encoding `config` asks for, with `--compression` checked against the
/// codecs of the chosen format.
pub fn encoding(config: &ExportConfig) -> anyhow::Result<Encoding> {
    let compression = config.compression.as_deref();
    Ok(match config.format {
        Format::Json => {
            if let Some(codec) = compression.filter(|codec| !codec.eq_ignore_ascii_case("none")) {
                bail!("--compression {codec} does not apply to json");
            }
            Encoding::Json
        }
        Format::Arrow => Encoding::Arrow {
            compression: compression.map_or(Ok(IpcCompression::None), str::parse)?,
            dictionary: config.dictionary,
            batch_size: config.batch_size,
        },
        Format::Parquet => Encoding::Parquet {
            compression: compression.map_or(Ok(ParquetCompression::Snappy), str::parse)?,
            batch_size: config.batch_size,
        },
    })
}

pub async fn run(config: &ExportConfig) -> anyhow::Result<()> {
    let encoding = encoding(config)?;
    let query = RowsQuery {
        dataset: config.dataset,
        after_key: i64::try_from(config.offset).context("--offset is too large")?,
        limit: i64::try_from(config.rows).context("--rows is too large")?,
        columns: ColumnSelection::parse(
            config.dataset,
            config.columns.as_deref().unwrap_or_default(),
        )?,
    };

    let pool = db::create_pool(config.database.clone(), 1)?;
    let client = pool.get().await.context("connecting to PostgreSQL")?;
    let db_started = Instant::now();
    let fetched = db::fetch_rows(&client, &query).await?;
    let db_time = db_started.elapsed();
    let row_count = fetched.rows.len();

    let path = config.out.clone();
    let encode_started = Instant::now();
    let bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<u64> {
        let file = File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        let writer = encode::encode_to(
            BufWriter::new(file),
            &fetched.rows,
            fetched.columns.columns(),
            &encoding,
            ArrowContainer::File,
            &CancelFlag::default(),
        )?;
        let mut file = writer.into_inner().map_err(|err| err.into_error())?;
        file.flush()?;
        Ok(file.metadata()?.len())
    })
    .await??;

    info!(
        dataset = config.dataset.name,
        rows = row_count,
        format = %encoding.format(),
        variant = %encoding.label(),
        bytes,
        db_ms = db_time.as_secs_f64() * 1e3,
        encode_ms = encode_started.elapsed().as_secs_f64() * 1e3,
        "wrote {}",
        config.out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        export: ExportConfig,
    }

    fn config(args: &[&str]) -> ExportConfig {
        let mut argv = vec![
            "export",
            "--database-url",
            "postgres://localhost/db",
            "--out",
            "x",
        ];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).unwrap().export
    }

    #[test]
    fn compression_defaults_per_format() {
        assert_eq!(
            encoding(&config(&["--format", "json"])).unwrap(),
            Encoding::Json
        );
        assert_eq!(
            encoding(&config(&["--format", "arrow", "--dictionary"])).unwrap(),
            Encoding::Arrow {
                compression: IpcCompression::None,
                dictionary: true,
                batch_size: 65_536
            }
        );
        assert_eq!(
            encoding(&config(&["--format", "parquet", "--batch-size", "2048"])).unwrap(),
            Encoding::Parquet {
                compression: ParquetCompression::Snappy,
                batch_size: 2048
            }
        );
    }

    #[test]
    fn compression_must_suit_the_format() {
        let zstd = config(&["--format", "arrow", "--compression", "zstd"]);
        assert!(matches!(
            encoding(&zstd).unwrap(),
            Encoding::Arrow {
                compression: IpcCompression::Zstd,
                ..
            }
        ));
        assert!(encoding(&config(&["--format", "arrow", "--compression", "snappy"])).is_err());
        assert!(encoding(&config(&["--format", "json", "--compression", "gzip"])).is_err());
        assert!(encoding(&config(&["--format", "json", "--compression", "none"])).is_ok());
    }

    #[test]
    fn dataset_defaults_to_orders_and_is_validated() {
        assert_eq!(config(&["--format", "json"]).dataset.name, "orders");
        let lines = config(&["--format", "json", "--dataset", "order_lines"]);
        assert_eq!(lines.dataset.name, "order_lines");
        let argv = [
            "export",
            "--database-url",
            "postgres://localhost/db",
            "--out",
            "x",
            "--format",
            "json",
            "--dataset",
            "users",
        ];
        let err = Cli::try_parse_from(argv).err().unwrap().to_string();
        assert!(err.contains("unknown dataset \"users\""), "{err}");
    }

    #[test]
    fn batch_size_is_validated_by_the_parser() {
        let mut argv = vec![
            "export",
            "--database-url",
            "postgres://localhost/db",
            "--out",
            "x",
        ];
        argv.extend_from_slice(&["--format", "arrow", "--batch-size", "10"]);
        assert!(Cli::try_parse_from(argv).is_err());
    }
}
