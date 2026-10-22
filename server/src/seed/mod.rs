//! `playground seed`: deterministic mock data for every dataset, loaded
//! with parallel binary `COPY`.
//!
//! A seeder holds a session-level advisory lock for its whole run, so two
//! seeders never interleave, and records its progress in `seed_status`:
//! `loading` first, `complete` only as the very last step. `--if-empty`
//! keeps nothing but a complete seed of the current schema version.

mod chunks;
mod copy;
mod data;
mod generator;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use tokio::task::JoinSet;
use tokio_postgres::Client;
use tracing::{info, warn};

pub use self::chunks::{Chunk, ChunkPlan, ChunkQueue};
use self::copy::copy_in;
pub use self::generator::{Customer, Generator, Order, OrderLine};
use crate::config::{SeedConfig, SeedMode};
use crate::db::{self, SEED_COMPLETE, SEED_LOADING, SeedStatus};
use crate::ddl::{self, CREATE_SEED_STATUS_SQL, DROP_ALL_SQL, Index, SCHEMA_VERSION, TABLES};

const PROGRESS_INTERVAL: Duration = Duration::from_secs(2);
/// Dimension tables, created complete (with keys) and filled in step 2.
const DIMENSIONS: [&str; 4] = ["countries", "cities", "categories", "products"];
/// Tables that get their keys, indexes and foreign keys after loading.
const LOADED: [&str; 3] = ["customers", "orders", "order_items"];
/// Everything `VACUUM (ANALYZE)` runs on after a replace.
const VACUUMED: [&str; 8] = [
    "countries",
    "cities",
    "categories",
    "products",
    "customers",
    "orders",
    "order_items",
    "daily_sales",
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SeedOutcome {
    /// `--if-empty` and a complete seed of the current schema existed.
    Skipped,
    Seeded {
        /// Orders written by this run.
        rows: u64,
        /// Order lines written by this run.
        lines: u64,
        /// Time spent loading orders and lines only.
        load: Duration,
        /// Everything, including DDL, indexes, views and `VACUUM`.
        total: Duration,
    },
}

pub async fn run(config: &SeedConfig) -> anyhow::Result<SeedOutcome> {
    let started = Instant::now();
    let client = db::connect_with_retry(&config.database)
        .await
        .context("connecting to PostgreSQL")?;
    if !db::try_lock_seeding(&client).await? {
        info!("another seeder is running; waiting for it to finish");
        db::lock_seeding(&client).await?;
    }
    let outcome = run_locked(&client, config, started).await;
    // Closing the connection would release the lock as well.
    if let Err(err) = db::unlock_seeding(&client).await {
        warn!(error = %err, "could not release the seed lock");
    }
    outcome
}

/// Decides what to do from `seed_status`, read only once the lock is held,
/// so a seeder that waited sees what the other one left.
async fn run_locked(
    client: &Client,
    config: &SeedConfig,
    started: Instant,
) -> anyhow::Result<SeedOutcome> {
    let status = db::seed_status(client).await?;
    let complete = status.as_ref().filter(|status| status.is_complete());
    if config.if_empty {
        if let Some(status) = complete {
            info!(
                orders = status.orders,
                order_items = status.order_items,
                "a complete seed of schema version {SCHEMA_VERSION} exists; nothing to do (--if-empty)"
            );
            return Ok(SeedOutcome::Skipped);
        }
        info!(
            found = %describe(status.as_ref()),
            "no complete seed of schema version {SCHEMA_VERSION}; replacing everything (--if-empty)"
        );
        return replace(client, config, started).await;
    }
    match (config.mode, complete) {
        (SeedMode::Replace, _) => replace(client, config, started).await,
        (SeedMode::Append, Some(status)) => append(client, config, status, started).await,
        (SeedMode::Append, None) => bail!(
            "--mode append needs a complete seed of schema version {SCHEMA_VERSION}, found {}; \
             run a replace seed first",
            describe(status.as_ref())
        ),
    }
}

fn describe(status: Option<&SeedStatus>) -> String {
    status.map_or_else(
        || "no seed_status".to_owned(),
        |status| {
            format!(
                "state '{}' of schema version {}",
                status.state, status.schema_version
            )
        },
    )
}

/// Logs each step's duration as it finishes.
struct Steps(Instant);

impl Steps {
    fn start() -> Self {
        Self(Instant::now())
    }

    fn done(&mut self, step: &str) -> Duration {
        let elapsed = self.0.elapsed();
        info!(seconds = elapsed.as_secs_f64(), "{step}");
        self.0 = Instant::now();
        elapsed
    }
}

/// Drops and recreates every relation; see "playground seed" in
/// docs/DESIGN.md for the steps.
async fn replace(
    client: &Client,
    config: &SeedConfig,
    started: Instant,
) -> anyhow::Result<SeedOutcome> {
    let generator = Arc::new(Generator::new(config.rng_seed, config.rows));
    info!(
        orders = config.rows,
        customers = generator.customer_count(),
        workers = config.workers,
        batch_size = config.batch_size,
        seed = config.rng_seed,
        "replacing every dataset"
    );
    let mut steps = Steps::start();

    // 1. The marker and the drop commit together: from here until step 9 the
    // database never claims a complete seed.
    client
        .batch_execute(&format!(
            "BEGIN;
SET LOCAL client_min_messages = warning;
{CREATE_SEED_STATUS_SQL};
INSERT INTO seed_status (schema_version, state, orders, order_items, seed, started_at, finished_at)
VALUES ({SCHEMA_VERSION}, '{SEED_LOADING}', {orders}, 0, {seed}, now(), NULL)
ON CONFLICT (id) DO UPDATE SET schema_version = excluded.schema_version, state = excluded.state,
  orders = excluded.orders, order_items = excluded.order_items, seed = excluded.seed,
  started_at = excluded.started_at, finished_at = NULL;
{DROP_ALL_SQL};
COMMIT",
            orders = config.rows,
            // BIGINT column: seeds above i64::MAX are stored as their bit pattern.
            seed = config.rng_seed as i64,
        ))
        .await
        .context("dropping the old relations")?;
    steps.done("1/9 drop every relation, seed_status = loading");

    // 2. Small dimension tables, created with their keys.
    let dimensions: Vec<String> = DIMENSIONS
        .iter()
        .map(|&name| ddl::table(name).create_sql())
        .collect();
    client.batch_execute(&dimensions.join(";\n")).await?;
    copy_in(client, generator.countries()).await?;
    copy_in(client, generator.cities()).await?;
    copy_in(client, generator.categories()).await?;
    copy_in(client, generator.products()).await?;
    steps.done("2/9 countries, cities, categories, products");

    // 3. Every customer an order can reference.
    client
        .batch_execute(&ddl::table("customers").create_sql())
        .await?;
    let customers = ChunkPlan::new(1, generator.customer_count(), config.batch_size);
    load(config, customers, Arc::clone(&generator), Job::Customers).await?;
    steps.done("3/9 customers");

    // 4. Loading into tables without indexes and building them afterwards is
    // much faster than maintaining them row by row.
    client
        .batch_execute(&format!(
            "{};\n{}",
            ddl::table("orders").create_sql(),
            ddl::table("order_items").create_sql()
        ))
        .await?;
    steps.done("4/9 create orders and order_items");

    // 5. Orders and their lines.
    let plan = ChunkPlan::new(1, config.rows, config.batch_size);
    let lines = load(config, plan, generator, Job::Orders).await?;
    let load_time = steps.done("5/9 load orders and order lines");
    info!(
        "loaded {} orders ({:.0}/s) and {lines} lines ({:.0}/s)",
        config.rows,
        per_second(config.rows, load_time),
        per_second(lines, load_time)
    );

    // 6.
    build_constraints(client, &config.database).await?;
    steps.done("6/9 primary keys, indexes, foreign keys");

    // 7. The views read the tables at query time; the materialized view is
    // computed now and indexed for keyset reads.
    for view in &ddl::VIEWS {
        client.batch_execute(&view.create_sql().join(";\n")).await?;
    }
    steps.done("7/9 views and materialized view");

    // 8.
    vacuum_analyze(&config.database, &VACUUMED).await?;
    steps.done("8/9 VACUUM (ANALYZE)");

    // 9.
    mark_complete(client, config.rows, lines).await?;
    steps.done("9/9 seed_status = complete");

    let total = started.elapsed();
    info!(
        "seeded {} orders and {lines} lines in {:.2}s: load {:.2}s, total {:.0} orders/s",
        config.rows,
        total.as_secs_f64(),
        load_time.as_secs_f64(),
        per_second(config.rows, total),
    );
    Ok(SeedOutcome::Seeded {
        rows: config.rows,
        lines,
        load: load_time,
        total,
    })
}

/// Adds orders and their lines after `max(id)`, the customers they newly
/// reference, and refreshes the materialized view.
async fn append(
    client: &Client,
    config: &SeedConfig,
    status: &SeedStatus,
    started: Instant,
) -> anyhow::Result<SeedOutcome> {
    let mut steps = Steps::start();
    // A new `started_at` is a new generation: running servers drop their
    // row caches.
    client
        .execute(
            "UPDATE seed_status SET state = $1, started_at = now(), finished_at = NULL",
            &[&SEED_LOADING],
        )
        .await?;
    let last_id: i64 = client
        .query_one("SELECT coalesce(max(id), 0) FROM orders", &[])
        .await?
        .try_get(0)?;
    let last_customer: i32 = client
        .query_one("SELECT coalesce(max(id), 0) FROM customers", &[])
        .await?
        .try_get(0)?;
    let generator = Arc::new(Generator::new(
        config.rng_seed,
        last_id as u64 + config.rows,
    ));
    info!(
        orders = config.rows,
        first_id = last_id + 1,
        workers = config.workers,
        seed = config.rng_seed,
        "appending orders"
    );

    let new_customers = generator
        .customer_count()
        .saturating_sub(last_customer as u64);
    if new_customers > 0 {
        let plan = ChunkPlan::new(
            i64::from(last_customer) + 1,
            new_customers,
            config.batch_size,
        );
        load(config, plan, Arc::clone(&generator), Job::Customers).await?;
    }
    steps.done("new customers");

    let plan = ChunkPlan::new(last_id + 1, config.rows, config.batch_size);
    let lines = load(config, plan, generator, Job::Orders).await?;
    let load_time = steps.done("load orders and order lines");

    client
        .batch_execute("REFRESH MATERIALIZED VIEW daily_sales")
        .await?;
    steps.done("refresh daily_sales");
    vacuum_analyze(
        &config.database,
        &["customers", "orders", "order_items", "daily_sales"],
    )
    .await?;
    steps.done("VACUUM (ANALYZE)");
    mark_complete(
        client,
        status.orders as u64 + config.rows,
        status.order_items as u64 + lines,
    )
    .await?;

    let total = started.elapsed();
    info!(
        "appended {} orders and {lines} lines in {:.2}s",
        config.rows,
        total.as_secs_f64()
    );
    Ok(SeedOutcome::Seeded {
        rows: config.rows,
        lines,
        load: load_time,
        total,
    })
}

async fn mark_complete(client: &Client, orders: u64, order_items: u64) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE seed_status SET state = $1, orders = $2, order_items = $3, \
             finished_at = clock_timestamp()",
            &[&SEED_COMPLETE, &(orders as i64), &(order_items as i64)],
        )
        .await?;
    Ok(())
}

/// Builds every index of the loaded tables at the same time, one connection
/// each, then turns the unique ones into primary keys (a catalog-only
/// change; `ADD PRIMARY KEY` directly would lock the table against the
/// other builds), then adds the foreign keys, which check existing rows.
async fn build_constraints(
    client: &Client,
    database: &tokio_postgres::Config,
) -> anyhow::Result<()> {
    let tables: Vec<&'static ddl::Table> = LOADED.iter().map(|&name| ddl::table(name)).collect();
    let mut builds = JoinSet::new();
    for index in tables.iter().flat_map(|table| table.indexes) {
        let database = database.clone();
        builds.spawn(async move {
            let client = db::connect(&database).await?;
            client
                .batch_execute(&index.create_sql())
                .await
                .with_context(|| format!("building {}", index.name))
        });
    }
    while let Some(result) = builds.join_next().await {
        result.context("index build panicked")??;
    }
    // Collected first: an iterator of closures held across `.await` would
    // make this future (and `seed::run`) not `Send`.
    let primary_keys = tables
        .iter()
        .flat_map(|table| table.indexes.iter().filter_map(Index::primary_key_sql));
    let foreign_keys = TABLES
        .iter()
        .filter(|table| LOADED.contains(&table.name))
        .flat_map(|table| table.foreign_keys.iter().map(|&fk| fk.to_owned()));
    let statements: Vec<String> = primary_keys.chain(foreign_keys).collect();
    for statement in statements {
        client
            .batch_execute(&statement)
            .await
            .with_context(|| statement.clone())?;
    }
    Ok(())
}

/// `VACUUM (ANALYZE)` of each relation on its own connection, at the same
/// time: sets hint bits and the visibility map now, so neither first reads
/// nor a background autovacuum land in the measurements.
async fn vacuum_analyze(
    database: &tokio_postgres::Config,
    relations: &[&'static str],
) -> anyhow::Result<()> {
    let mut tasks = JoinSet::new();
    for &relation in relations {
        let database = database.clone();
        tasks.spawn(async move {
            let client = db::connect(&database).await?;
            client
                .batch_execute(&format!("VACUUM (ANALYZE) {relation}"))
                .await
                .with_context(|| format!("VACUUM (ANALYZE) {relation}"))
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.context("VACUUM panicked")??;
    }
    Ok(())
}

/// What the COPY workers load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Job {
    /// Chunks are customer ids.
    Customers,
    /// Chunks are order ids; each chunk's lines follow its orders.
    Orders,
}

impl Job {
    const fn noun(self) -> &'static str {
        match self {
            Self::Customers => "customers",
            Self::Orders => "orders",
        }
    }
}

#[derive(Debug, Default)]
struct Progress {
    rows: AtomicU64,
    lines: AtomicU64,
}

/// Runs `config.workers` COPY connections that pull chunks of `plan` until
/// it is exhausted, logging progress meanwhile. Returns the number of order
/// lines written.
async fn load(
    config: &SeedConfig,
    plan: ChunkPlan,
    generator: Arc<Generator>,
    job: Job,
) -> anyhow::Result<u64> {
    let queue = Arc::new(plan.queue());
    let progress = Arc::new(Progress::default());
    let workers = u64::from(config.workers).min(plan.chunk_count());

    let mut tasks = JoinSet::new();
    for _ in 0..workers {
        tasks.spawn(copy_worker(
            config.database.clone(),
            Arc::clone(&queue),
            Arc::clone(&generator),
            Arc::clone(&progress),
            job,
        ));
    }
    let reporter = tokio::spawn(report_progress(Arc::clone(&progress), plan.rows(), job));
    // Dropping the set on the first error aborts the remaining workers.
    let result = async {
        while let Some(result) = tasks.join_next().await {
            result.context("COPY worker panicked")??;
        }
        anyhow::Ok(())
    }
    .await;
    reporter.abort();
    result?;
    Ok(progress.lines.load(Ordering::Relaxed))
}

async fn copy_worker(
    database: tokio_postgres::Config,
    queue: Arc<ChunkQueue>,
    generator: Arc<Generator>,
    progress: Arc<Progress>,
    job: Job,
) -> anyhow::Result<()> {
    let client = db::connect(&database).await?;
    // Seeding can be repeated, so trading durability for speed is fine.
    client.batch_execute("SET synchronous_commit = off").await?;

    let mut lines = Vec::new();
    while let Some(chunk) = queue.next() {
        let context = || format!("COPY of {} from id {}", job.noun(), chunk.first_id);
        match job {
            Job::Customers => {
                copy_in(&client, generator.customers(chunk))
                    .await
                    .with_context(context)?;
            }
            Job::Orders => {
                // The chunk's lines are committed after its orders, so their
                // foreign keys (checked in append mode) always find them.
                lines.clear();
                let orders = generator
                    .orders(chunk)
                    .inspect(|order| generator.push_lines(order, &mut lines));
                copy_in(&client, orders).await.with_context(context)?;
                let line_count = copy_in(&client, lines.drain(..))
                    .await
                    .with_context(context)?;
                progress.lines.fetch_add(line_count, Ordering::Relaxed);
            }
        }
        progress.rows.fetch_add(chunk.len, Ordering::Relaxed);
    }
    Ok(())
}

async fn report_progress(progress: Arc<Progress>, total: u64, job: Job) {
    let started = Instant::now();
    let mut ticker = tokio::time::interval(PROGRESS_INTERVAL);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let done = progress.rows.load(Ordering::Relaxed);
        let lines = progress.lines.load(Ordering::Relaxed);
        let elapsed = started.elapsed();
        let percent = done as f64 * 100.0 / total.max(1) as f64;
        let rate = per_second(done, elapsed);
        match job {
            Job::Customers => info!("copied {done}/{total} customers ({percent:.0}%), {rate:.0}/s"),
            Job::Orders => info!(
                "copied {done}/{total} orders ({percent:.0}%), {rate:.0}/s, and {lines} lines, {:.0}/s",
                per_second(lines, elapsed)
            ),
        }
    }
}

fn per_second(rows: u64, elapsed: Duration) -> f64 {
    rows as f64 / elapsed.as_secs_f64().max(f64::EPSILON)
}
