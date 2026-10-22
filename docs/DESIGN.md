# Rust Arrow Playground: design and contracts

This document is the shared contract between the Rust server, the seeder, the
frontend and the deployment files. If you change an API parameter, a header,
a column or a CLI flag, update this file in the same change.

## Goal

Compare retrieving tabular data from PostgreSQL over HTTP in three ways:

| Mode | What the server sends | What the browser does |
| --- | --- | --- |
| JSON (no Arrow) | Array of row objects, built with serde | `JSON.parse` |
| Arrow IPC | Arrow IPC *stream* (record batches) | `apache-arrow` `tableFromIPC` |
| Parquet | One Parquet file | `parquet-wasm` `readParquet` → Arrow table |

For every request we measure server DB time, server encode time, bytes on the
wire, bytes of the payload, time to first byte, download time and client decode
time. The app runs locally with `docker compose`, and is designed to be
deployed later to GCP (Cloud Run + Cloud SQL for PostgreSQL) so the same
measurements can be taken over real internet links.

## Toolchain versions (pinned)

The project owner pinned these versions on 2026-10-22. They supersede any
other version mentioned anywhere else (including older task notes that say
Rust 1.97, Node 22, `rust:1-bookworm`, `node:22-bookworm-slim` or
`postgres:16-alpine`).

| Tool | Version | Where it is pinned |
| --- | --- | --- |
| Rust | 1.99.1 | root `rust-toolchain.toml` (`channel = "1.99.1"`, components rustfmt + clippy); `rust-version = "1.99.1"` in `server/Cargo.toml`; Docker builder `rust:1.99.1-trixie`; CI uses the toolchain file |
| Node.js | 26.12.1 | root `.nvmrc` and `.node-version` (`26.12.1`); `"engines": {"node": ">=26.12.1"}` in `web/package.json`; Docker web stage `node:26.12.1-trixie-slim`; CI `actions/setup-node` with `node-version-file: .nvmrc` |
| Docker Engine | 29.9.2 | README prerequisites (Docker Engine 29.9.2 with the Compose plugin); CI docker job installs Engine v29.9.2 with `docker/setup-docker-action` |
| PostgreSQL | 18.6 | docker compose and CI service image `postgres:18.6-alpine`; Cloud SQL `POSTGRES_18` |

Runtime image is `debian:trixie-slim`, matching the glibc of the
`rust:1.99.1-trixie` builder.

PostgreSQL 18 images changed their data layout: `PGDATA` is
`/var/lib/postgresql/18/docker`, and the named volume must be mounted at
`/var/lib/postgresql` (not `/var/lib/postgresql/data`, which the 18 image
rejects).

## Repository layout

```
.
├── server/                    Rust package `playground` (lib + bin `playground`)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs            clap CLI: serve | seed | export
│   │   ├── lib.rs
│   │   ├── config.rs          env / flag configuration
│   │   ├── db.rs              connection pool, DDL, table stats
│   │   ├── schema.rs          column catalog of table `orders`
│   │   ├── query.rs           validated request params → SQL
│   │   ├── pg_time.rs         TIMESTAMPTZ / DATE wire values, RFC 3339 text
│   │   ├── convert.rs         tokio-postgres rows → Arrow RecordBatch
│   │   ├── encode/            json.rs, ipc.rs, parquet.rs (+ mod.rs)
│   │   ├── cache.rs           small in-memory row cache
│   │   ├── server/            axum router, handlers, response headers, bench
│   │   ├── seed/              mock-data generator + parallel binary COPY
│   │   └── export.rs          write JSON / Arrow / Parquet files to disk
│   └── tests/                 integration tests (need TEST_DATABASE_URL)
├── web/                       Vite + React + TypeScript frontend
├── Dockerfile                 one image: server binary + built frontend
├── docker-compose.yml         db + seed + app
├── deploy/gcp/                GCP deployment notes and scripts (later step)
├── .github/workflows/ci.yml
├── docs/DESIGN.md             this file
└── README.md
```

## Database

One table, `orders`. The catalog in `server/src/schema.rs` is the single source
of truth for column names, Postgres types, Arrow types, nullability and UI
groups; the DDL is generated from or kept in sync with it.

| # | column | Postgres type | Arrow type | null | dict | group | data |
| - | --- | --- | --- | --- | --- | --- | --- |
| 1 | id | BIGINT (PK) | Int64 | no | | numeric | 1..N, sequential |
| 2 | created_at | TIMESTAMPTZ | Timestamp(Microsecond, "UTC") | no | | time | last ~2 years, microsecond precision |
| 3 | ship_date | DATE | Date32 | yes (~15%) | | time | created_at + 1..10 days, NULL = not shipped |
| 4 | customer_id | INTEGER | Int32 | no | | numeric | 1..max(N/10, 1000) |
| 5 | customer_name | TEXT | Utf8 | no | | text | "First Last", high cardinality |
| 6 | email | TEXT | Utf8 | no | | text | derived from name + customer_id |
| 7 | country | TEXT | Utf8 | no | yes | text | ~25 ISO-3166 alpha-2 codes, weighted |
| 8 | city | TEXT | Utf8 | no | yes | text | 4..10 cities per country |
| 9 | category | TEXT | Utf8 | no | yes | text | ~12 categories |
| 10 | product | TEXT | Utf8 | no | yes | text | 8..20 products per category |
| 11 | quantity | SMALLINT | Int16 | no | | numeric | 1..20, skewed to small |
| 12 | unit_price | DOUBLE PRECISION | Float64 | no | | numeric | per-product base price ± noise, 2 decimals |
| 13 | discount | REAL | Float32 | no | | numeric | one of 0, 0.05, 0.10, 0.15, 0.20, 0.25 |
| 14 | total | DOUBLE PRECISION | Float64 | no | | numeric | quantity × unit_price × (1 − discount), 2 decimals |
| 15 | payment_method | TEXT | Utf8 | no | yes | text | ~6 values |
| 16 | is_returned | BOOLEAN | Boolean | no | | numeric | ~5% true |
| 17 | rating | SMALLINT | Int16 | yes (~40%) | | numeric | 1..5 |
| 18 | latitude | DOUBLE PRECISION | Float64 | no | | numeric | city coordinate + small jitter |
| 19 | longitude | DOUBLE PRECISION | Float64 | no | | numeric | city coordinate + small jitter |
| 20 | notes | TEXT | Utf8 | yes (~85%) | | text | short free-text sentence |

"dict" marks low-cardinality string columns that may be dictionary encoded in
Arrow when the `dictionary` option is on.

Indexes: primary key on `id`, plus an index on `created_at`. Data endpoints use
keyset pagination on `id`.

## CLI: `playground`

The single binary has three subcommands. Every flag can also be set by the
environment variable shown.

### `playground serve`

| flag | env | default |
| --- | --- | --- |
| `--port` | `PORT` | 8080 |
| `--database-url` | `DATABASE_URL` | required |
| `--static-dir` | `STATIC_DIR` | `./web/dist` (Docker image: `/app/static`) |
| `--max-rows` | `MAX_ROWS` | 1000000 |
| `--pool-size` | `DB_POOL_SIZE` | 16 |
| `--cache-entries` | `CACHE_ENTRIES` | 8 |
| `--cache-max-rows` | `CACHE_MAX_ROWS` | 1000000 |

The row cache (`cache=true`) is an LRU bounded by both `CACHE_ENTRIES` results
and `CACHE_MAX_ROWS` rows in total (a cached row with all 20 columns takes
roughly 0.8 KB); a result larger than `CACHE_MAX_ROWS` is not cached.

Logging via `RUST_LOG` (default `info`). On startup the server creates the
`orders` table if it does not exist (so `/api/info` works before seeding),
then listens on `0.0.0.0:$PORT`. Graceful shutdown on SIGTERM/SIGINT (Cloud
Run sends SIGTERM).

`DATABASE_URL` accepts both URL form (`postgres://user:pass@host:5432/db`) and
key/value form (`host=/cloudsql/PROJECT:REGION:INSTANCE user=... dbname=...`),
which is how Cloud Run connects to Cloud SQL over a unix socket. Connections
are made without TLS (local, docker network or the Cloud SQL socket).

### `playground seed`

| flag | env | default | meaning |
| --- | --- | --- | --- |
| `--rows` | `SEED_ROWS` | 1000000 | rows to generate |
| `--workers` | `SEED_WORKERS` | 4 | parallel COPY connections |
| `--batch-size` | `SEED_BATCH_SIZE` | 50000 | rows per COPY chunk |
| `--mode` | `SEED_MODE` | `replace` | `replace` drops and recreates the table; `append` adds rows after `max(id)` |
| `--if-empty` | `SEED_IF_EMPTY` | false | do nothing if the table already has rows (used by docker compose) |
| `--seed` | `SEED_RNG` | 42 | RNG seed |
| `--database-url` | `DATABASE_URL` | required | |

`replace` creates the table without indexes, loads with parallel binary
`COPY ... FROM STDIN (FORMAT binary)`, then adds the primary key, the
`created_at` index (both built at once), and runs `ANALYZE`. Data is
deterministic: each chunk seeds its own RNG from `(seed, first id of the
chunk)` and customer attributes come from `(seed, customer_id)`, so the same
seed, rows and batch size give the same table whatever the worker count, and
`append` never repeats earlier rows. `created_at` spans 2024-01-01 to
2025-12-31 UTC (fixed, so data does not depend on the day it is seeded).
Progress and final rows/s are logged.

### `playground export`

`playground export --format parquet|arrow|json --out FILE [--rows N] [--offset ID] [--columns a,b,c] [--compression CODEC] [--dictionary] [--batch-size N]`

Writes a file from the database using the same encoders as the API. `arrow`
writes the Arrow IPC *file* format (`.arrow`, readable by DuckDB, pyarrow,
polars); `parquet` writes a Parquet file; `json` writes the JSON array.
Defaults: `--rows 10000`, `--offset 0`, all columns, `--batch-size 65536`
(1024..=1048576), compression `none` for arrow (`none|lz4|zstd`) and `snappy`
for parquet (`none|snappy|gzip|zstd|lz4|brotli`); json takes no compression.
`--database-url` / `DATABASE_URL` as for `serve`.

## HTTP API

Everything lives under `/api`. CORS allows any origin for `GET`/`OPTIONS`, and
exposes all custom headers listed below plus `Content-Encoding`. Every `/api` response carries
`Timing-Allow-Origin: *` so the browser Resource Timing API reports sizes and
timings even when the UI runs on another origin.

### `GET /api/health`

`200 {"status":"ok"}`. Does not touch the database (used by Cloud Run probes).

### `GET /api/ping`

`204` with an empty body. Used by the UI to measure round-trip time.

### `GET /api/info`

```json
{
  "table": "orders",
  "row_estimate": 1000000,
  "max_id": 1000000,
  "table_bytes": 260000000,
  "max_rows": 1000000,
  "columns": [
    {"name": "id", "pg_type": "int8", "arrow_type": "Int64", "nullable": false, "dictionary": false, "group": "numeric"}
  ],
  "formats": ["json", "arrow", "parquet"],
  "ipc_compression": ["none", "lz4", "zstd"],
  "parquet_compression": ["none", "snappy", "gzip", "zstd", "lz4", "brotli"],
  "server": {"version": "0.1.0", "region": null, "service": null, "revision": null}
}
```

`row_estimate` comes from `pg_class.reltuples` (falls back to `count(*)` when the
estimate is unknown, e.g. never analyzed), `max_id` from `max(id)`,
`table_bytes` from `pg_total_relation_size`. `server.region` is `$REGION`,
`server.service` is `$K_SERVICE`, `server.revision` is `$K_REVISION` (Cloud
Run sets the latter two).

### `GET /api/data`

| param | values | default | applies to |
| --- | --- | --- | --- |
| `format` | `json`, `arrow`, `parquet` | `json` | all |
| `rows` | 1..=MAX_ROWS | 10000 | all |
| `offset` | u64: return rows with `id > offset` | 0 | all |
| `columns` | comma-separated column names (unknown → 400; duplicates removed; order kept) | all | all |
| `http_compression` | bool | true | all: when false the response is never `Content-Encoding`-compressed |
| `dictionary` | bool | false | arrow: dictionary-encode the "dict" columns as `Dictionary(Int32, Utf8)` |
| `ipc_compression` | `none`, `lz4`, `zstd` | `none` | arrow: IPC body compression (LZ4_FRAME / ZSTD) |
| `parquet_compression` | `none`, `snappy`, `gzip`, `zstd`, `lz4`, `brotli` | `snappy` | parquet (`lz4` means LZ4_RAW) |
| `batch_size` | 1024..=1048576 | 65536 | arrow: rows per record batch; parquet: max row-group size |
| `cache` | bool | false | reuse rows already fetched for the same (rows, offset, columns) |

Bools accept `true/false/1/0/on/off/yes/no`; bools and keyword values are
case-insensitive. Unknown parameters are ignored (the UI adds a `_`
cache-busting nonce), an empty value counts as absent, and every known
parameter is validated even when it does not apply to the chosen format.

SQL is always `SELECT <cols> FROM orders WHERE id > $1 ORDER BY id LIMIT $2`,
where `<cols>` are quoted identifiers taken from the catalog, never from user
text.

Response:

- `200` with `Content-Type`:
  - json → `application/json`
  - arrow → `application/vnd.apache.arrow.stream`
  - parquet → `application/vnd.apache.parquet`
- Headers on every successful data response:
  - `Server-Timing: db;dur=<ms>;desc="PostgreSQL", encode;dur=<ms>;desc="<format> encode", total;dur=<ms>` (milliseconds, 3 decimals)
  - `X-Db-Ms`, `X-Encode-Ms`, `X-Total-Ms` (same numbers)
  - `X-Row-Count`, `X-Column-Count`, `X-Last-Id` (id of the last row, or the offset if no rows)
  - `X-Body-Bytes`: payload size before any HTTP compression
  - `X-Format`: `json` / `arrow` / `parquet`
  - `X-Cache`: `hit`, `miss` or `off`
  - `Cache-Control: no-store`
- HTTP compression (gzip, br, zstd or deflate, chosen from `Accept-Encoding`) is
  applied only when `http_compression` is true and the body is larger than
  1 KiB.
- Bodies larger than 16 MiB are sent with chunked transfer encoding (1 MiB
  chunks, no `Content-Length`), so uncompressed responses stay under Cloud
  Run's 32 MiB limit for non-streamed HTTP/1 responses.
- Errors: `400 {"error": "..."}` for bad parameters, `500 {"error": "..."}`
  for database or encoding failures.

Encoding runs in `tokio::task::spawn_blocking` so large payloads do not stall
the async runtime.

#### JSON shape

An array of row objects in column order:

```json
[{"id":1,"created_at":"2025-01-01T12:00:00.123456Z","ship_date":"2025-01-03","customer_id":17,"customer_name":"Ada Lovelace","email":"ada.lovelace17@example.com","country":"GB","city":"London","category":"Books","product":"...","quantity":2,"unit_price":19.99,"discount":0.1,"total":35.98,"payment_method":"card","is_returned":false,"rating":null,"latitude":51.5,"longitude":-0.12,"notes":null}]
```

Timestamps are RFC 3339 UTC with microseconds and `Z`; dates are
`YYYY-MM-DD`; NULL is `null`; numbers are JSON numbers. The JSON is written
straight from the Postgres rows with serde (no intermediate `serde_json::Value`),
which is how a typical well-written REST API would do it.

#### Arrow shape

Arrow IPC streaming format: one schema message, then `ceil(rows / batch_size)`
record batches, then end-of-stream. Zero rows → schema + end-of-stream. Types
as in the table above; with `dictionary=true` the dict columns are
`Dictionary(Int32, Utf8)`.

#### Parquet shape

One Parquet file written with `parquet::arrow::ArrowWriter` from the same
record batches (plain `Utf8` for strings; Parquet does its own dictionary
encoding), with the chosen compression and `max_row_group_size = batch_size`.

### `GET /api/bench`

Server-side encoding benchmark, no network involved. Params: `rows` (default
100000, max `min(MAX_ROWS, 1000000)`), `offset`, `columns`, `cache`. Fetches
the rows once, then encodes every variant and reports time and size:

```json
{
  "rows": 100000,
  "columns": 20,
  "db_ms": 120.5,
  "results": [
    {"format": "json", "variant": "json", "options": {}, "encode_ms": 45.6, "bytes": 31000000},
    {"format": "arrow", "variant": "arrow · zstd · dict", "options": {"ipc_compression": "zstd", "dictionary": true}, "encode_ms": 12.3, "bytes": 6000000},
    {"format": "parquet", "variant": "parquet · snappy", "options": {"parquet_compression": "snappy"}, "encode_ms": 20.1, "bytes": 5000000}
  ]
}
```

Variants: json; arrow × ipc_compression {none, lz4, zstd} × dictionary {false,
true}; parquet × {none, snappy, gzip, zstd, lz4, brotli}; batch size 65536.
Variants run one after another; each `encode_ms` includes the rows → Arrow
conversion. The response carries `Server-Timing`, `X-Db-Ms`, `X-Encode-Ms`
(all variants), `X-Total-Ms` and `X-Cache` like `/api/data`; errors as for
`/api/data`.

### Static files

Every path outside `/api` is served from `STATIC_DIR` with SPA fallback to
`index.html`. If `STATIC_DIR` does not exist, `/` returns a short plain-text
note explaining how to build the frontend.

## Frontend (`web/`)

Vite + React + TypeScript, no UI kit; hand-written CSS with light and dark
themes. Libraries: `apache-arrow` (decode IPC), `parquet-wasm` (decode Parquet
to Arrow), and small decompressors registered in Arrow's
`compressionRegistry` so LZ4/ZSTD IPC bodies decode in the browser.

- `npm run dev`: Vite on :5173, proxying `/api` to `http://localhost:8080`.
- `npm run build`: output in `web/dist`, served by the Rust server.
- API base URL is a UI setting (default: same origin, saved in
  `localStorage`) so a local UI can target a remote Cloud Run server.

### Settings

- A prominent **Use Arrow** switch. Off → JSON. On → choose Arrow IPC or Parquet.
- Rows (presets 1k / 10k / 100k / 250k / 500k / 1M, capped by `max_rows`, plus a number input).
- Columns: presets All / Numeric / Text / Time, plus per-column checkboxes.
- Start id (offset), with an option to randomize the offset each run.
- HTTP compression on/off.
- Arrow: dictionary encoding, IPC compression, batch size.
- Parquet: compression codec, row-group size.
- Server row cache on/off.
- Iterations (1–20) and an optional warm-up run.

### Actions

- **Run**: the current settings, N iterations.
- **Compare**: JSON vs Arrow IPC vs Parquet with the same rows/columns and the
  current per-format options, N iterations each, in interleaved order.
- **Server benchmark**: calls `/api/bench` and shows encode time and size per variant.

### Measurements per request

`fetch(url, {cache: "no-store"})` with a unique `_` nonce, then:

| metric | source |
| --- | --- |
| server DB, encode, total | `X-Db-Ms` / `X-Encode-Ms` / `X-Total-Ms` (or `serverTiming`) |
| time to first byte | Resource Timing `responseStart − requestStart` (fallback: fetch resolved) |
| network wait | TTFB − server total |
| download | `responseEnd − responseStart` (fallback: `arrayBuffer()` time) |
| wire bytes | `transferSize` / `encodedBodySize` |
| payload bytes | `X-Body-Bytes` / `decodedBodySize` |
| client decode | JSON: `TextDecoder` + `JSON.parse`; Arrow: `tableFromIPC`; Parquet: `readParquet` + `tableFromIPC` |
| client use | sum of `total` (or the first numeric column) over all rows, to touch the data |
| end to end | start of fetch → end of "use" |

### Results views

- Last run: stacked time bar (DB, encode, network wait, download, decode, use)
  and sizes (wire vs payload), rows/s and MB/s.
- Comparison table: median / min / max end-to-end, median wire bytes, speed-up
  and size ratio relative to JSON, with grouped bars.
- History of all runs with Clear and Copy-as-CSV/Markdown.
- Data preview of the first 20 rows of the last result.
- Server benchmark table with bars.

## Docker and compose

One multi-stage `Dockerfile` at the repo root: Node stage builds `web/dist`,
Rust stage builds `playground` in release mode (dependency layer cached),
runtime stage is a slim image with `/app/playground` and `/app/static`,
`ENTRYPOINT ["/app/playground"]`, `CMD ["serve"]`, runs as non-root,
`EXPOSE 8080`.

`docker-compose.yml`:

- `db`: `postgres:18.6-alpine`, healthcheck, named volume mounted at `/var/lib/postgresql`.
- `seed`: same image, `seed --if-empty --rows ${SEED_ROWS:-1000000}`, waits for `db` healthy.
- `app`: same image, `serve`, waits for `db` healthy and `seed` completed, port 8080.

## CI

GitHub Actions: Rust job (fmt, clippy `-D warnings`, tests with a Postgres
service), web job (`npm ci`, typecheck, build), Docker build job (no push).

## GCP (later step)

`deploy/gcp/` documents Artifact Registry, Cloud SQL for PostgreSQL, a Cloud
Run service (`DATABASE_URL=host=/cloudsql/...`), a Cloud Run Job for seeding,
and recommended sizing (memory for 1M-row JSON responses).

Measured locally: a 1,000,000-row, 20-column request peaks at about 1.4 GiB
RSS (≈0.8 GB of fetched rows plus the 435 MB JSON body); the binary uses
jemalloc with background purging, so RSS falls back to tens of MB about a
second after the response. Budget at least 2 GiB per instance for 1M-row
requests (more with the row cache or concurrent large requests).
