//! Rust Arrow Playground: serve PostgreSQL rows as JSON, Arrow IPC or Parquet
//! and measure the difference.

pub mod cache;
pub mod config;
pub mod convert;
pub mod db;
pub mod encode;
pub mod export;
pub mod pg_time;
pub mod query;
pub mod schema;
pub mod seed;
pub mod server;
