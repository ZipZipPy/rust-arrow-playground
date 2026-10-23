//! HTTP server: axum router, shared state and graceful shutdown.

mod bench;
pub mod compression;
mod error;
mod handlers;
pub mod response;
mod static_files;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::Context;
use axum::Router;
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderValue, Method};
use axum::routing::get;
use axum::serve::ListenerExt;
use deadpool_postgres::Pool;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tower_http::LatencyUnit;
use tower_http::cors::{Any, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{Level, info, warn};

pub use self::error::ApiError;
use self::response::{CacheStatus, EXPOSED_HEADERS, TIMING_ALLOW_ORIGIN};
use crate::cache::RowCache;
use crate::config::{ServeConfig, ServerInfo};
use crate::db::{self, FetchedRows, GuardedClient};
use crate::query::RowsQuery;

/// Server-wide settings that are not part of the pool.
#[derive(Debug, Clone)]
pub struct Settings {
    pub max_rows: u64,
    pub cache_entries: usize,
    pub cache_max_rows: usize,
    /// How often cache hits re-read the seed generation (in the background).
    pub seed_check_interval: Duration,
    pub server_info: ServerInfo,
}

/// See [`Settings::seed_check_interval`]: a re-seed is noticed within about
/// this long, and back-to-back cache hits cost the database one tiny query
/// per interval instead of one each.
pub const SEED_CHECK_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    pool: Pool,
    cache: RowCache,
    seed_check: SeedCheck,
    /// One `/api/bench` at a time: each holds every fetched row plus its
    /// outputs, and runs the machine flat out.
    bench: Arc<Semaphore>,
    max_rows: u64,
    server_info: ServerInfo,
}

/// Rows for one request and where they came from.
pub struct LoadedRows {
    pub rows: Arc<FetchedRows>,
    /// Pool checkout plus query, or the cache lookup.
    pub db_time: Duration,
    pub cache: CacheStatus,
}

impl AppState {
    pub fn new(pool: Pool, settings: Settings) -> Self {
        Self(Arc::new(Inner {
            pool,
            cache: RowCache::new(settings.cache_entries, settings.cache_max_rows),
            seed_check: SeedCheck::new(settings.seed_check_interval),
            bench: Arc::new(Semaphore::new(1)),
            max_rows: settings.max_rows,
            server_info: settings.server_info,
        }))
    }

    pub fn pool(&self) -> &Pool {
        &self.0.pool
    }

    pub fn max_rows(&self) -> u64 {
        self.0.max_rows
    }

    pub fn server_info(&self) -> &ServerInfo {
        &self.0.server_info
    }

    /// Waits until no other benchmark runs. The permit should live as long
    /// as the benchmark's blocking work, not just its handler.
    pub async fn bench_permit(&self) -> OwnedSemaphorePermit {
        Arc::clone(&self.0.bench)
            .acquire_owned()
            .await
            .expect("the semaphore is never closed")
    }

    /// Reads the rows of `query`, from the cache when `use_cache` allows.
    ///
    /// A cache hit never waits for the database, so its DB time is only the
    /// lookup, even on a server far from the database. It is served from
    /// the generation seen last and, at most once per
    /// [`Settings::seed_check_interval`], starts a background re-read of the
    /// seed generation that drops the whole cache if the data was re-seeded.
    /// A miss reads the generation before its rows, as before.
    pub async fn load_rows(
        &self,
        query: RowsQuery,
        use_cache: bool,
    ) -> Result<LoadedRows, ApiError> {
        let started = Instant::now();
        if use_cache && let Some(rows) = self.0.cache.get(&query) {
            self.check_seed_in_background();
            return Ok(LoadedRows {
                rows,
                db_time: started.elapsed(),
                cache: CacheStatus::Hit,
            });
        }

        let client = GuardedClient::new(self.0.pool.get().await?);
        if !use_cache {
            let fetched = db::fetch_rows(&client, &query).await;
            // A query that failed has finished too: the connection is fine.
            client.release();
            let rows = Arc::new(fetched?);
            return Ok(LoadedRows {
                rows,
                db_time: started.elapsed(),
                cache: CacheStatus::Off,
            });
        }

        self.0.seed_check.started();
        let generation = match db::seed_status(&client).await {
            Ok(status) => status.map(|status| status.generation()),
            Err(err) => {
                client.release();
                return Err(err.into());
            }
        };
        drop_in_background(self.0.cache.sync_generation(generation));
        let fetched = db::fetch_rows(&client, &query).await;
        client.release();
        let rows = Arc::new(fetched?);
        let db_time = started.elapsed();
        drop_in_background(self.0.cache.insert(generation, query, Arc::clone(&rows)));
        Ok(LoadedRows {
            rows,
            db_time,
            cache: CacheStatus::Miss,
        })
    }

    /// Re-reads the seed generation off the request path, unless that was
    /// done within the interval or is still running.
    fn check_seed_in_background(&self) {
        if !self.0.seed_check.try_begin() {
            return;
        }
        let state = self.clone();
        tokio::spawn(async move {
            let result = async {
                let client = GuardedClient::new(state.0.pool.get().await?);
                let status = db::seed_status(&client).await;
                client.release();
                Ok::<_, ApiError>(status?.map(|status| status.generation()))
            }
            .await;
            // A failure is logged where the error is made; a hit after the
            // next interval tries again.
            if let Ok(generation) = result {
                drop_in_background(state.0.cache.sync_generation(generation));
            }
            state.0.seed_check.end();
        });
    }
}

/// When the seed generation was last read for the row cache, and whether a
/// background read is running.
struct SeedCheck {
    interval: Duration,
    last: Mutex<Option<Instant>>,
    running: AtomicBool,
}

impl SeedCheck {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: Mutex::new(None),
            running: AtomicBool::new(false),
        }
    }

    /// Notes a read that starts now (a miss reads the generation inline).
    fn started(&self) {
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
    }

    /// Claims the background read when the last one is older than the
    /// interval and none is running.
    fn try_begin(&self) -> bool {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if last.is_some_and(|at| at.elapsed() < self.interval)
            || self.running.swap(true, Ordering::AcqRel)
        {
            return false;
        }
        *last = Some(Instant::now());
        true
    }

    fn end(&self) {
        self.running.store(false, Ordering::Release);
    }
}

/// Frees `values` on a blocking thread: dropping a million fetched rows
/// takes tens of milliseconds, which should neither stall an async worker
/// nor delay a response.
pub(crate) fn drop_in_background<T: Send + 'static>(values: Vec<T>) {
    if !values.is_empty() {
        tokio::task::spawn_blocking(move || drop(values));
    }
}

/// The whole application: `/api` plus static files from `static_dir`.
pub fn router(state: AppState, static_dir: &Path) -> Router {
    let api = Router::new()
        .route("/health", get(handlers::health))
        .route("/ping", get(handlers::ping))
        .route("/info", get(handlers::info))
        .route("/data", get(handlers::data))
        .route("/bench", get(handlers::bench))
        .fallback(handlers::not_found)
        .method_not_allowed_fallback(handlers::method_not_allowed)
        .with_state(state)
        .layer(cors_layer())
        .layer(SetResponseHeaderLayer::overriding(
            TIMING_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ));

    // No compression layer: data responses are compressed by their handler
    // (see `compression`), static files by their own service.
    static_files::with_static_files(Router::new().nest("/api", api), static_dir).layer(
        TraceLayer::new_for_http()
            .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
            .on_response(
                DefaultOnResponse::new()
                    .level(Level::INFO)
                    .latency_unit(LatencyUnit::Micros),
            ),
    )
}

fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::HEAD, Method::OPTIONS])
        .allow_headers(Any)
        .expose_headers(EXPOSED_HEADERS)
        .max_age(Duration::from_secs(24 * 60 * 60))
}

/// `playground serve`
pub async fn run(config: ServeConfig) -> anyhow::Result<()> {
    info!(
        hosts = ?config.database.get_hosts(),
        dbname = ?config.database.get_dbname(),
        "connecting to PostgreSQL"
    );
    let pool = db::create_pool(config.database.clone(), config.pool_size.into())?;
    {
        let client = db::wait_until_ready(&pool)
            .await
            .context("connecting to PostgreSQL")?;
        let created = db::create_missing_relations(&client)
            .await
            .context("creating the dataset relations")?;
        if !created.is_empty() {
            info!(
                ?created,
                "created missing relations; run `playground seed` to fill them"
            );
        }
    }

    let state = AppState::new(
        pool,
        Settings {
            max_rows: config.max_rows,
            cache_entries: config.cache_entries,
            cache_max_rows: config.cache_max_rows,
            seed_check_interval: SEED_CHECK_INTERVAL,
            server_info: ServerInfo::from_env(),
        },
    );
    let app = router(state, &config.static_dir);

    let address = SocketAddr::from(([0, 0, 0, 0], config.port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("binding {address}"))?
        .tap_io(|tcp| {
            // Small responses such as /api/ping should not wait for Nagle.
            if let Err(err) = tcp.set_nodelay(true) {
                warn!(error = %err, "could not set TCP_NODELAY");
            }
        });
    info!(%address, static_dir = %config.static_dir.display(), "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    info!("server stopped");
    Ok(())
}

/// Resolves on SIGINT or SIGTERM (Cloud Run sends SIGTERM before stopping
/// an instance); in-flight requests are then allowed to finish.
pub async fn shutdown_signal() {
    let interrupt = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            warn!(error = %err, "cannot listen for SIGINT");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
            }
            Err(err) => {
                warn!(error = %err, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => info!("SIGINT received, shutting down"),
        () = terminate => info!("SIGTERM received, shutting down"),
    }
}
