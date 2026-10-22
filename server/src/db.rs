//! Connection pool, DDL, table statistics and row fetching.

use std::time::Duration;

use deadpool_postgres::{Manager, ManagerConfig, Pool, PoolError, RecyclingMethod, Runtime};
use tokio_postgres::{Client, Config, NoTls, Row};
use tracing::{error, warn};

use crate::query::RowsQuery;
use crate::schema::{COLUMNS, ColumnSelection, TABLE};

const APPLICATION_NAME: &str = "rust-arrow-playground";
/// Prepared statements kept per pooled connection before the cache is reset.
/// Every distinct column selection is its own statement.
const MAX_CACHED_STATEMENTS: usize = 64;
const STARTUP_ATTEMPTS: u32 = 15;
const STARTUP_RETRY_DELAY: Duration = Duration::from_secs(2);

/// Parses `DATABASE_URL` in URL form (`postgres://user:pass@host/db`) or
/// key/value form (`host=/cloudsql/PROJECT:REGION:INSTANCE user=... dbname=...`).
pub fn parse_config(database_url: &str) -> Result<Config, tokio_postgres::Error> {
    let mut config: Config = database_url.parse()?;
    if config.get_application_name().is_none() {
        config.application_name(APPLICATION_NAME);
    }
    Ok(config)
}

/// Builds a pool from an already parsed config, so both `DATABASE_URL`
/// forms reach deadpool unchanged.
pub fn create_pool(config: Config, max_size: usize) -> anyhow::Result<Pool> {
    let manager = Manager::from_config(
        config,
        NoTls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    Ok(Pool::builder(manager)
        .max_size(max_size)
        .runtime(Runtime::Tokio1)
        .create_timeout(Some(Duration::from_secs(10)))
        .wait_timeout(Some(Duration::from_secs(60)))
        .build()?)
}

/// Opens one dedicated connection and drives it on a background task.
pub async fn connect(config: &Config) -> Result<Client, tokio_postgres::Error> {
    let (client, connection) = config.connect(NoTls).await?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            error!(error = %err, "database connection failed");
        }
    });
    Ok(client)
}

/// Waits for the database to accept connections, which matters on Cloud Run
/// where the Cloud SQL socket may appear shortly after the container starts.
pub async fn wait_until_ready(pool: &Pool) -> Result<deadpool_postgres::Client, PoolError> {
    let mut attempt = 1;
    loop {
        match pool.get().await {
            Ok(client) => return Ok(client),
            Err(err) if attempt < STARTUP_ATTEMPTS => {
                warn!(attempt, error = %err, "database not ready, retrying");
                tokio::time::sleep(STARTUP_RETRY_DELAY).await;
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// `CREATE TABLE IF NOT EXISTS orders (...)` from the catalog, optionally
/// with the primary key (the seeder adds it after loading).
pub fn create_table_sql(primary_key: bool) -> String {
    let mut definitions: Vec<String> = COLUMNS
        .iter()
        .map(|column| {
            let not_null = if column.nullable { "" } else { " NOT NULL" };
            format!("{} {}{not_null}", column.quoted(), column.ty.sql())
        })
        .collect();
    if primary_key {
        definitions.push("PRIMARY KEY (\"id\")".to_owned());
    }
    format!(
        "CREATE TABLE IF NOT EXISTS {TABLE} (\n  {}\n)",
        definitions.join(",\n  ")
    )
}

pub const CREATE_INDEXES_SQL: &str =
    "CREATE INDEX IF NOT EXISTS orders_created_at_idx ON orders (\"created_at\")";

/// Creates `orders` with its indexes unless it already exists. An existing
/// table is left alone: even `CREATE INDEX IF NOT EXISTS` would queue behind
/// a running autovacuum.
pub async fn ensure_table(client: &Client) -> Result<(), tokio_postgres::Error> {
    if table_exists(client).await? {
        return Ok(());
    }
    client
        .batch_execute(&format!(
            "{};\n{CREATE_INDEXES_SQL}",
            create_table_sql(true)
        ))
        .await
}

pub async fn table_exists(client: &Client) -> Result<bool, tokio_postgres::Error> {
    let row = client
        .query_one(&format!("SELECT to_regclass('{TABLE}') IS NOT NULL"), &[])
        .await?;
    row.try_get(0)
}

pub async fn table_has_rows(client: &Client) -> Result<bool, tokio_postgres::Error> {
    if !table_exists(client).await? {
        return Ok(false);
    }
    let row = client
        .query_one(&format!("SELECT EXISTS (SELECT 1 FROM {TABLE})"), &[])
        .await?;
    row.try_get(0)
}

/// Largest id in the table, 0 when empty.
pub async fn max_id(client: &Client) -> Result<i64, tokio_postgres::Error> {
    let row = client
        .query_one(
            &format!("SELECT coalesce(max(\"id\"), 0) FROM {TABLE}"),
            &[],
        )
        .await?;
    row.try_get(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableStats {
    pub row_estimate: i64,
    pub max_id: i64,
    pub table_bytes: i64,
}

/// Planner row estimate (exact count if the table was never analyzed),
/// `max(id)` and total size including indexes.
pub async fn table_stats(client: &Client) -> Result<TableStats, tokio_postgres::Error> {
    let row = client
        .query_one(
            &format!(
                "SELECT c.reltuples::float8, pg_total_relation_size(c.oid), \
                        (SELECT coalesce(max(\"id\"), 0) FROM {TABLE}) \
                 FROM pg_class c WHERE c.oid = '{TABLE}'::regclass"
            ),
            &[],
        )
        .await?;
    let reltuples: f64 = row.try_get(0)?;
    // Postgres 14+ reports -1 until the first VACUUM or ANALYZE.
    let row_estimate = if reltuples < 0.0 {
        let count = client
            .query_one(&format!("SELECT count(*) FROM {TABLE}"), &[])
            .await?;
        count.try_get(0)?
    } else {
        reltuples.round() as i64
    };
    Ok(TableStats {
        row_estimate,
        table_bytes: row.try_get(1)?,
        max_id: row.try_get(2)?,
    })
}

/// Rows read for one [`RowsQuery`]; the first `columns.len()` fields of
/// each row are the selected columns.
#[derive(Debug)]
pub struct FetchedRows {
    pub rows: Vec<Row>,
    pub columns: ColumnSelection,
    /// Id of the last row, `None` when no rows matched.
    pub last_id: Option<i64>,
}

pub async fn fetch_rows(
    client: &deadpool_postgres::Client,
    query: &RowsQuery,
) -> Result<FetchedRows, tokio_postgres::Error> {
    if client.statement_cache.size() >= MAX_CACHED_STATEMENTS {
        client.statement_cache.clear();
    }
    let statement = client.prepare_cached(&query.sql()).await?;
    let rows = client
        .query(&statement, &[&query.after_id, &query.limit])
        .await?;
    let last_id = rows
        .last()
        .map(|row| row.try_get::<_, i64>(query.id_index()))
        .transpose()?;
    Ok(FetchedRows {
        rows,
        columns: query.columns.clone(),
        last_id,
    })
}

#[cfg(test)]
mod tests {
    use tokio_postgres::config::Host;

    use super::*;

    #[test]
    fn parses_url_form() {
        let config = parse_config("postgres://app:secret@db.example:6543/shop").unwrap();
        assert_eq!(config.get_user(), Some("app"));
        assert_eq!(config.get_dbname(), Some("shop"));
        assert_eq!(config.get_ports(), [6543]);
        assert_eq!(config.get_application_name(), Some(APPLICATION_NAME));
    }

    #[test]
    fn parses_cloud_sql_unix_socket_form() {
        let config =
            parse_config("host=/cloudsql/proj:europe-west1:pg user=app dbname=shop password=x")
                .unwrap();
        assert_eq!(config.get_user(), Some("app"));
        assert_eq!(config.get_dbname(), Some("shop"));
        assert!(matches!(
            config.get_hosts(),
            [Host::Unix(path)] if path.to_str() == Some("/cloudsql/proj:europe-west1:pg")
        ));
    }

    #[test]
    fn keeps_an_explicit_application_name() {
        let config = parse_config("postgres://localhost/db?application_name=probe").unwrap();
        assert_eq!(config.get_application_name(), Some("probe"));
    }

    #[test]
    fn ddl_follows_the_catalog() {
        let ddl = create_table_sql(true);
        assert!(ddl.starts_with("CREATE TABLE IF NOT EXISTS orders ("));
        assert!(ddl.contains("\"id\" BIGINT NOT NULL"));
        assert!(ddl.contains("\"ship_date\" DATE,"));
        assert!(ddl.contains("\"discount\" REAL NOT NULL"));
        assert!(ddl.contains("PRIMARY KEY (\"id\")"));
        assert!(!create_table_sql(false).contains("PRIMARY KEY"));
    }
}
