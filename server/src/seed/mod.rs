//! `playground seed`: deterministic mock orders loaded with parallel binary
//! `COPY`.

mod chunks;
mod data;
mod generator;

use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use tokio::task::JoinSet;
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::Type;
use tracing::info;

pub use self::chunks::{Chunk, ChunkPlan, ChunkQueue};
pub use self::generator::{Generator, Order};
use crate::config::{SeedConfig, SeedMode};
use crate::db;
use crate::schema::{COLUMNS, TABLE};

const PROGRESS_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SeedOutcome {
    /// `--if-empty` and the table already had rows.
    Skipped,
    Seeded {
        rows: u64,
        /// Time spent in `COPY` only.
        load: Duration,
        /// Everything, including DDL, indexes and `ANALYZE`.
        total: Duration,
    },
}

pub async fn run(config: &SeedConfig) -> anyhow::Result<SeedOutcome> {
    let started = Instant::now();
    let client = db::connect(&config.database)
        .await
        .context("connecting to PostgreSQL")?;

    if config.if_empty && db::table_has_rows(&client).await? {
        info!("{TABLE} already has rows; nothing to do (--if-empty)");
        return Ok(SeedOutcome::Skipped);
    }

    let last_existing_id = match config.mode {
        SeedMode::Replace => {
            // Loading into a table without indexes and building them
            // afterwards is much faster than maintaining them row by row.
            client
                .batch_execute(&format!("DROP TABLE IF EXISTS {TABLE}"))
                .await?;
            client.batch_execute(&db::create_table_sql(false)).await?;
            0
        }
        SeedMode::Append => {
            db::ensure_table(&client).await?;
            db::max_id(&client).await?
        }
    };

    let plan = ChunkPlan::new(last_existing_id + 1, config.rows, config.batch_size);
    let generator = Arc::new(Generator::new(
        config.rng_seed,
        last_existing_id as u64 + config.rows,
    ));
    info!(
        rows = config.rows,
        first_id = last_existing_id + 1,
        workers = config.workers,
        chunks = plan.chunk_count(),
        mode = ?config.mode,
        seed = config.rng_seed,
        "seeding {TABLE}"
    );

    let load_started = Instant::now();
    load(config, plan, generator).await?;
    let load = load_started.elapsed();

    if config.mode == SeedMode::Replace {
        let index_started = Instant::now();
        build_indexes(&client, &config.database).await?;
        info!(
            seconds = index_started.elapsed().as_secs_f64(),
            "indexes built"
        );
    }
    client.batch_execute(&format!("ANALYZE {TABLE}")).await?;

    let total = started.elapsed();
    info!(
        "seeded {} rows in {:.2}s: COPY {:.2}s ({:.0} rows/s), total {:.0} rows/s",
        config.rows,
        total.as_secs_f64(),
        load.as_secs_f64(),
        per_second(config.rows, load),
        per_second(config.rows, total),
    );
    Ok(SeedOutcome::Seeded {
        rows: config.rows,
        load,
        total,
    })
}

/// Builds the primary key index and the `created_at` index at the same time
/// on two connections, then turns the unique index into the primary key
/// (a catalog-only change). `ADD PRIMARY KEY` directly would lock the table
/// against the second index build.
async fn build_indexes(
    client: &tokio_postgres::Client,
    database: &tokio_postgres::Config,
) -> anyhow::Result<()> {
    let unique_id = async {
        client
            .batch_execute(&format!(
                "CREATE UNIQUE INDEX orders_pkey ON {TABLE} (\"id\")"
            ))
            .await
    };
    let created_at = async {
        let other = db::connect(database).await?;
        other.batch_execute(db::CREATE_INDEXES_SQL).await
    };
    tokio::try_join!(unique_id, created_at)?;
    client
        .batch_execute(&format!(
            "ALTER TABLE {TABLE} ADD CONSTRAINT orders_pkey PRIMARY KEY USING INDEX orders_pkey"
        ))
        .await?;
    Ok(())
}

/// Runs `config.workers` COPY connections that pull chunks until the plan is
/// exhausted, logging progress meanwhile.
async fn load(
    config: &SeedConfig,
    plan: ChunkPlan,
    generator: Arc<Generator>,
) -> anyhow::Result<()> {
    let queue = Arc::new(plan.queue());
    let copied = Arc::new(AtomicU64::new(0));
    let workers = u64::from(config.workers).min(plan.chunk_count());

    let mut tasks = JoinSet::new();
    for _ in 0..workers {
        tasks.spawn(copy_worker(
            config.database.clone(),
            Arc::clone(&queue),
            Arc::clone(&generator),
            Arc::clone(&copied),
        ));
    }
    let progress = tokio::spawn(report_progress(Arc::clone(&copied), config.rows));
    // Dropping the set on the first error aborts the remaining workers.
    while let Some(result) = tasks.join_next().await {
        result.context("COPY worker panicked")??;
    }
    progress.abort();
    Ok(())
}

async fn copy_worker(
    database: tokio_postgres::Config,
    queue: Arc<ChunkQueue>,
    generator: Arc<Generator>,
    copied: Arc<AtomicU64>,
) -> anyhow::Result<()> {
    let client = db::connect(&database).await?;
    // Seeding can be repeated, so trading durability for speed is fine.
    client.batch_execute("SET synchronous_commit = off").await?;

    let column_names: Vec<String> = COLUMNS.iter().map(|c| c.quoted()).collect();
    let copy_sql = format!(
        "COPY {TABLE} ({}) FROM STDIN (FORMAT binary)",
        column_names.join(", ")
    );
    let types: Vec<Type> = COLUMNS.iter().map(|c| c.ty.pg_type()).collect();

    while let Some(chunk) = queue.next() {
        let sink = client.copy_in(&copy_sql).await?;
        let mut writer = pin!(BinaryCopyInWriter::new(sink, &types));
        // Rows are generated while earlier ones are already on the wire.
        for order in generator.orders(chunk) {
            writer.as_mut().write(&order.values()).await?;
        }
        writer
            .finish()
            .await
            .with_context(|| format!("COPY of ids from {}", chunk.first_id))?;
        copied.fetch_add(chunk.len, Ordering::Relaxed);
    }
    Ok(())
}

async fn report_progress(copied: Arc<AtomicU64>, total: u64) {
    let started = Instant::now();
    let mut ticker = tokio::time::interval(PROGRESS_INTERVAL);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let done = copied.load(Ordering::Relaxed);
        info!(
            "copied {done}/{total} rows ({:.0}%), {:.0} rows/s",
            done as f64 * 100.0 / total.max(1) as f64,
            per_second(done, started.elapsed())
        );
    }
}

fn per_second(rows: u64, elapsed: Duration) -> f64 {
    rows as f64 / elapsed.as_secs_f64().max(f64::EPSILON)
}
