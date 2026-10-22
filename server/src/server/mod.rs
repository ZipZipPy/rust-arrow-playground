//! HTTP server: axum router, shared state and graceful shutdown.

mod bench;
mod error;
mod handlers;
pub mod response;
mod static_files;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use axum::Router;
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderValue, Method};
use axum::routing::get;
use axum::serve::ListenerExt;
use deadpool_postgres::Pool;
use tower_http::LatencyUnit;
use tower_http::cors::{Any, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{Level, info, warn};

pub use self::error::ApiError;
use self::response::{CacheStatus, EXPOSED_HEADERS, TIMING_ALLOW_ORIGIN};
use crate::cache::RowCache;
use crate::config::{ServeConfig, ServerInfo};
use crate::db::{self, FetchedRows};
use crate::query::RowsQuery;

/// Server-wide settings that are not part of the pool.
#[derive(Debug, Clone)]
pub struct Settings {
    pub max_rows: u64,
    pub cache_entries: usize,
    pub cache_max_rows: usize,
    pub server_info: ServerInfo,
}

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    pool: Pool,
    cache: RowCache,
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

    pub async fn load_rows(
        &self,
        query: RowsQuery,
        use_cache: bool,
    ) -> Result<LoadedRows, ApiError> {
        let started = Instant::now();
        if !use_cache {
            let rows = Arc::new(self.fetch(&query).await?);
            return Ok(LoadedRows {
                rows,
                db_time: started.elapsed(),
                cache: CacheStatus::Off,
            });
        }
        if let Some(rows) = self.0.cache.get(&query) {
            return Ok(LoadedRows {
                rows,
                db_time: started.elapsed(),
                cache: CacheStatus::Hit,
            });
        }
        let rows = Arc::new(self.fetch(&query).await?);
        let db_time = started.elapsed();
        self.0
            .cache
            .insert(query, Arc::clone(&rows), rows.rows.len());
        Ok(LoadedRows {
            rows,
            db_time,
            cache: CacheStatus::Miss,
        })
    }

    async fn fetch(&self, query: &RowsQuery) -> Result<FetchedRows, ApiError> {
        let client = self.0.pool.get().await?;
        Ok(db::fetch_rows(&client, query).await?)
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

    static_files::with_static_files(Router::new().nest("/api", api), static_dir)
        .layer(response::compression_layer())
        .layer(
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
        db::ensure_table(&client)
            .await
            .context("creating table orders")?;
    }

    let state = AppState::new(
        pool,
        Settings {
            max_rows: config.max_rows,
            cache_entries: config.cache_entries,
            cache_max_rows: config.cache_max_rows,
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
async fn shutdown_signal() {
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
