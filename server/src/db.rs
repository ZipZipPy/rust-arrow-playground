//! Connections, the seed lock and status, dataset statistics and row
//! fetching.

use std::time::Duration;

use deadpool_postgres::{Manager, ManagerConfig, Pool, PoolError, RecyclingMethod, Runtime};
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Config, NoTls, Row};
use tracing::{error, info, warn};

use crate::ddl;
use crate::pg_time::Timestamp;
use crate::query::RowsQuery;
use crate::schema::{ColumnSelection, Dataset, Stats};

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

/// Retries `attempt` while the database starts up, which matters on Cloud
/// Run (the Cloud SQL socket may appear shortly after the container starts)
/// and under docker compose (the first start of a volume runs an init phase).
async fn retry_startup<T, E: std::fmt::Display, F: Future<Output = Result<T, E>>>(
    mut attempt: impl FnMut() -> F,
) -> Result<T, E> {
    let mut tries = 1;
    loop {
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(err) if tries < STARTUP_ATTEMPTS => {
                warn!(attempt = tries, error = %err, "database not ready, retrying");
                tokio::time::sleep(STARTUP_RETRY_DELAY).await;
                tries += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Waits for the database to accept connections from the pool.
pub async fn wait_until_ready(pool: &Pool) -> Result<deadpool_postgres::Client, PoolError> {
    retry_startup(|| pool.get()).await
}

/// [`connect`] with the same startup retry as [`wait_until_ready`].
pub async fn connect_with_retry(config: &Config) -> Result<Client, tokio_postgres::Error> {
    retry_startup(|| connect(config)).await
}

/// A pooled connection used by one request. Dropped before
/// [`Self::release`] (the client went away while a query ran), it cancels
/// the running query and closes the connection instead of returning it to
/// the pool with an abandoned result still streaming in, which the next
/// request would otherwise wait for and report as its own DB time.
pub struct GuardedClient(Option<deadpool_postgres::Client>);

impl GuardedClient {
    pub fn new(client: deadpool_postgres::Client) -> Self {
        Self(Some(client))
    }

    /// Returns the connection to the pool: its queries have completed.
    pub fn release(mut self) {
        self.0.take();
    }
}

impl std::ops::Deref for GuardedClient {
    type Target = deadpool_postgres::Client;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("present until released or dropped")
    }
}

impl Drop for GuardedClient {
    fn drop(&mut self) {
        let Some(client) = self.0.take() else { return };
        let client = deadpool_postgres::Object::take(client);
        let token = client.cancel_token();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            if let Err(err) = token.cancel_query(NoTls).await {
                warn!(error = %err, "could not cancel an abandoned query");
            }
            // Closes the connection once the cancellation has been sent.
            drop(client);
        });
    }
}

/// Key of the session-level advisory lock a seeder holds for its whole run.
/// `serve` takes it briefly while it creates missing relations.
const SEED_LOCK_KEY: i64 = 0x7365_6564_706c_6179;

/// Takes the seed lock if nobody holds it.
pub async fn try_lock_seeding(client: &Client) -> Result<bool, tokio_postgres::Error> {
    client
        .query_one("SELECT pg_try_advisory_lock($1)", &[&SEED_LOCK_KEY])
        .await?
        .try_get(0)
}

/// Waits for the seed lock.
pub async fn lock_seeding(client: &Client) -> Result<(), tokio_postgres::Error> {
    client
        .execute("SELECT pg_advisory_lock($1)", &[&SEED_LOCK_KEY])
        .await
        .map(drop)
}

pub async fn unlock_seeding(client: &Client) -> Result<(), tokio_postgres::Error> {
    client
        .execute("SELECT pg_advisory_unlock($1)", &[&SEED_LOCK_KEY])
        .await
        .map(drop)
}

pub async fn relation_exists(client: &Client, name: &str) -> Result<bool, tokio_postgres::Error> {
    client
        .query_one("SELECT to_regclass($1) IS NOT NULL", &[&name])
        .await?
        .try_get(0)
}

/// Creates every missing relation of every dataset: empty tables with their
/// keys, the views, and the materialized view (cheap on empty tables). An
/// existing relation is left alone, even a table from an older schema, and
/// one that cannot be made (say, the tables of an interrupted seed still
/// lack their keys) is skipped with a warning: the server then serves what
/// exists until the next seed replaces everything. Skipped while a seeder
/// holds the seed lock, since it creates everything itself. Returns the
/// names of the relations it created.
pub async fn create_missing_relations(
    client: &Client,
) -> Result<Vec<&'static str>, tokio_postgres::Error> {
    if !try_lock_seeding(client).await? {
        info!("a seeder is running; leaving the relations to it");
        return Ok(Vec::new());
    }
    let created = create_missing_relations_locked(client).await;
    unlock_seeding(client).await?;
    created
}

async fn create_missing_relations_locked(
    client: &Client,
) -> Result<Vec<&'static str>, tokio_postgres::Error> {
    let tables = ddl::TABLES.iter().map(|table| {
        let mut statements = vec![table.create_sql()];
        statements.extend(table.constraints_sql());
        (table.name, statements)
    });
    // Plain views have no IF NOT EXISTS.
    let views = ddl::VIEWS.iter().map(|view| (view.name, view.create_sql()));
    let relations: Vec<(&'static str, Vec<String>)> = tables.chain(views).collect();
    let mut created = Vec::new();
    for (name, statements) in relations {
        if relation_exists(client, name).await? {
            continue;
        }
        // Several statements in one simple query run as one transaction, so
        // a failure leaves nothing half-made.
        match client.batch_execute(&statements.join(";\n")).await {
            Ok(()) => created.push(name),
            Err(err) => warn!(relation = name, error = %err, "could not create a missing relation"),
        }
    }
    Ok(created)
}

/// `state` of a seed that finished.
pub const SEED_COMPLETE: &str = "complete";
/// `state` while a seed runs, and after one that was interrupted.
pub const SEED_LOADING: &str = "loading";

/// The `seed_status` row: what the last seed did and whether it finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedStatus {
    pub schema_version: i32,
    pub state: String,
    pub orders: i64,
    pub order_items: i64,
    pub seed: i64,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

impl SeedStatus {
    /// A finished seed of the current schema, which `--if-empty` keeps.
    pub fn is_complete(&self) -> bool {
        self.state == SEED_COMPLETE && self.schema_version == ddl::SCHEMA_VERSION
    }

    /// Changes whenever a seed starts or finishes; the row cache drops its
    /// entries when it does.
    pub fn generation(&self) -> SeedGeneration {
        SeedGeneration {
            started_at: self.started_at,
            finished_at: self.finished_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedGeneration {
    started_at: Timestamp,
    finished_at: Option<Timestamp>,
}

/// The `seed_status` row, `None` before the first seed (no table or no row).
pub async fn seed_status(client: &Client) -> Result<Option<SeedStatus>, tokio_postgres::Error> {
    let row = match client
        .query_opt(
            "SELECT schema_version, state, orders, order_items, seed, started_at, finished_at \
             FROM seed_status",
            &[],
        )
        .await
    {
        Ok(row) => row,
        Err(err) if err.code() == Some(&SqlState::UNDEFINED_TABLE) => return Ok(None),
        Err(err) => return Err(err),
    };
    row.map(|row| {
        Ok(SeedStatus {
            schema_version: row.try_get(0)?,
            state: row.try_get(1)?,
            orders: row.try_get(2)?,
            order_items: row.try_get(3)?,
            seed: row.try_get(4)?,
            started_at: row.try_get(5)?,
            finished_at: row.try_get(6)?,
        })
    })
    .transpose()
}

/// What `/api/info` reports about a dataset; `None` where it has no cheap
/// answer, or the relation does not exist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DatasetStats {
    pub row_estimate: Option<i64>,
    pub max_key: Option<i64>,
    pub bytes: Option<i64>,
}

/// Row estimate, largest key and size of `dataset`, from catalog statistics
/// and index lookups only: never the dataset's own query.
pub async fn dataset_stats(
    client: &Client,
    dataset: &Dataset,
) -> Result<DatasetStats, tokio_postgres::Error> {
    match dataset.stats {
        Stats::Relation => relation_stats(client, dataset.relation, dataset.key).await,
        Stats::Join { rows, key, tables } => {
            let stats = relation_stats(client, rows, key).await?;
            if stats.row_estimate.is_none() {
                return Ok(stats);
            }
            let bytes = client
                .query_one(
                    "SELECT sum(pg_total_relation_size(to_regclass(t)))::bigint \
                     FROM unnest($1::text[]) AS t",
                    &[&tables],
                )
                .await?
                .try_get(0)?;
            Ok(DatasetStats { bytes, ..stats })
        }
        Stats::Mirror(twin) => {
            let stats = relation_stats(client, twin.relation, twin.key).await?;
            Ok(DatasetStats {
                bytes: None,
                ..stats
            })
        }
    }
}

/// Planner row estimate (an exact count if never analyzed), `max(key)` and
/// total size including indexes of one table or materialized view; nothing
/// if it does not exist.
async fn relation_stats(
    client: &Client,
    relation: &str,
    key: &str,
) -> Result<DatasetStats, tokio_postgres::Error> {
    let Some(row) = client
        .query_opt(
            "SELECT reltuples::float8, pg_total_relation_size(oid) \
             FROM pg_class WHERE oid = to_regclass($1)",
            &[&format!("\"{relation}\"")],
        )
        .await?
    else {
        return Ok(DatasetStats::default());
    };
    // An index lookup: the key is the first column of a unique index.
    let max_key = client
        .query_one(
            &format!("SELECT coalesce(max(\"{key}\"), 0) FROM \"{relation}\""),
            &[],
        )
        .await?
        .try_get(0)?;
    let reltuples: f64 = row.try_get(0)?;
    // Postgres 14+ reports -1 until the first VACUUM or ANALYZE.
    let row_estimate = if reltuples < 0.0 {
        let count = client
            .query_one(&format!("SELECT count(*) FROM \"{relation}\""), &[])
            .await?;
        count.try_get(0)?
    } else {
        reltuples.round() as i64
    };
    Ok(DatasetStats {
        row_estimate: Some(row_estimate),
        max_key: Some(max_key),
        bytes: Some(row.try_get(1)?),
    })
}

/// Rows read for one [`RowsQuery`]; the first `columns.len()` fields of
/// each row are the selected columns.
#[derive(Debug)]
pub struct FetchedRows {
    pub rows: Vec<Row>,
    pub columns: ColumnSelection,
    /// Key of the last row, `None` when no rows matched.
    pub last_key: Option<i64>,
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
        .query(&statement, &[&query.after_key, &query.limit])
        .await?;
    let last_key = rows
        .last()
        .map(|row| row.try_get::<_, i64>(query.key_index()))
        .transpose()?;
    Ok(FetchedRows {
        rows,
        columns: query.columns.clone(),
        last_key,
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
}
