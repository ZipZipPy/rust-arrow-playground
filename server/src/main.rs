use clap::{Parser, Subcommand};
use playground::config::{ExportConfig, SeedConfig, ServeConfig};
use playground::{export, seed, server};
use tracing_subscriber::EnvFilter;

/// A million-row response allocates over a gigabyte. glibc malloc keeps
/// freed memory in per-thread arenas, so the process would stay at its peak
/// size; jemalloc with a background purge thread hands it back to the OS
/// about a second after each request.
#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// jemalloc reads its options from this `const char *` symbol at startup.
#[cfg(not(target_env = "msvc"))]
#[unsafe(export_name = "_rjem_malloc_conf")]
static MALLOC_CONF: MallocConf =
    MallocConf(c"background_thread:true,dirty_decay_ms:1000,muzzy_decay_ms:0".as_ptr());

#[repr(transparent)]
struct MallocConf(*const std::ffi::c_char);

// SAFETY: it points at an immutable, 'static C string.
unsafe impl Sync for MallocConf {}

/// Compare JSON, Arrow IPC and Parquet for serving PostgreSQL rows over HTTP.
#[derive(Parser)]
#[command(name = "playground", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the HTTP API and the built frontend.
    Serve(ServeConfig),
    /// Fill every dataset with deterministic mock data.
    Seed(SeedConfig),
    /// Write rows to a JSON, Arrow IPC or Parquet file.
    Export(ExportConfig),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match Cli::parse().command {
        Command::Serve(config) => server::run(config).await,
        // As PID 1 in a container the seeder gets no default signal handling,
        // so stop on SIGTERM/SIGINT here; `seed_status` stays 'loading'.
        Command::Seed(config) => tokio::select! {
            outcome = seed::run(&config) => outcome.map(drop),
            () = server::shutdown_signal() => {
                anyhow::bail!("seed interrupted; the next run (even with --if-empty) seeds again")
            }
        },
        Command::Export(config) => export::run(&config).await,
    }
}
