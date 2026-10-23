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

The playground serves several *datasets* (see "Datasets" below): the wide
`orders` table, a 7-table join view, and a daily aggregate as both a
materialized view and a live view. The catalog in `server/src/schema.rs` is the
single source of truth for each dataset's relation, key column, column names,
Postgres types, Arrow types, nullability and UI groups; the DDL is generated
from or kept in sync with it.

### `orders` (table)

| # | column | Postgres type | Arrow type | null | dict | group | data |
| - | --- | --- | --- | --- | --- | --- | --- |
| 1 | id | BIGINT (PK) | Int64 | no | | numeric | 1..N, sequential |
| 2 | created_at | TIMESTAMPTZ | Timestamp(Microsecond, "UTC") | no | | time | last ~2 years, microsecond precision |
| 3 | ship_date | DATE | Date32 | yes (~15%) | | time | created_at + 1..10 days, NULL = not shipped |
| 4 | customer_id | INTEGER | Int32 | no | | numeric | 1..max(N/10, 1000) |
| 5 | customer_name | TEXT | Utf8 | no | | text | "First M. Last" from large name pools; distinct names grow with the number of customers (high cardinality, like real data) |
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
| 20 | notes | TEXT | Utf8 | yes (~85%) | | text | short free-text sentence built from templates with variable parts (not a small fixed set) |

"dict" marks low-cardinality string columns that may be dictionary encoded in
Arrow when the `dictionary` option is on.

Indexes: primary key on `id`, plus an index on `created_at`. Data endpoints use
keyset pagination on `id`.

## Datasets

Requested by Ake on 2026-10-22: "complex queries like multiple joins and
database views". The playground now serves several *datasets*. Each one is
a relation (table, view or materialized view) with its own column catalog.
The Arrow-vs-JSON comparison works the same for all of them. What changes is
how much of the time the database takes and what shape the result has.

| dataset | kind | relation | key column | what the database does per request | typical rows (1M orders) |
| --- | --- | --- | --- | --- | --- |
| `orders` | table | `orders` | `id` | index range scan on one wide table | 1,000,000 |
| `order_lines` | view | `order_line_details` | `line_id` | 7-table join: order_items ⋈ orders ⋈ customers ⋈ cities ⋈ countries ⋈ products ⋈ categories | ~2,500,000 |
| `daily_sales` | materialized view | `daily_sales` | `id` | index range scan on a precomputed aggregate | ~200,000 |
| `daily_sales_live` | view | `daily_sales_live` | `id` | the same 7-table join plus GROUP BY over every line, on every request | ~200,000 |

`orders` keeps exactly its current 20 columns, and its behaviour does not change.

### Normalized tables (created and filled by the seeder)

All of these are deterministic for a given seed. They agree with the existing
flat `orders` columns: `orders.country`, `orders.city`, `orders.category`,
`orders.product`, `orders.customer_name` and `orders.email` are exactly the
names in these tables.

```
countries   (code TEXT PRIMARY KEY, name TEXT NOT NULL, region TEXT NOT NULL,
             currency TEXT NOT NULL, vat_rate REAL NOT NULL)                       -- ~25 rows, from the generator's country list
cities      (id INTEGER PRIMARY KEY, name TEXT NOT NULL,
             country_code TEXT NOT NULL REFERENCES countries,
             latitude DOUBLE PRECISION NOT NULL, longitude DOUBLE PRECISION NOT NULL,
             population INTEGER NOT NULL)                                          -- ids in generator list order
categories  (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, department TEXT NOT NULL)
products    (id INTEGER PRIMARY KEY, name TEXT NOT NULL,
             category_id INTEGER NOT NULL REFERENCES categories,
             brand TEXT NOT NULL, base_price DOUBLE PRECISION NOT NULL,
             weight_kg REAL NOT NULL, launched_on DATE NOT NULL)
customers   (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT NOT NULL,
             segment TEXT NOT NULL,             -- consumer ~80% / business ~15% / vip ~5%
             signup_date DATE NOT NULL,         -- before the customer's first possible order
             city_id INTEGER NOT NULL REFERENCES cities,
             birth_year SMALLINT,               -- ~20% NULL
             is_active BOOLEAN NOT NULL)        -- one row for every customer_id that orders can reference
order_items (id BIGINT PRIMARY KEY,             -- = order_id * 10 + line_no (deterministic, parallel-friendly, readable)
             order_id BIGINT NOT NULL REFERENCES orders,
             line_no SMALLINT NOT NULL,         -- 1..5, average about 2.5
             product_id INTEGER NOT NULL REFERENCES products,
             quantity SMALLINT NOT NULL, unit_price DOUBLE PRECISION NOT NULL,
             discount REAL NOT NULL)
             + index on order_items (order_id)
orders.customer_id REFERENCES customers                                            -- added with the other foreign keys
```

Line 1 of each order is the order's own main line: same product, quantity,
unit_price and discount as the flat `orders` row. Lines 2..k are extra
products. So `orders` reads as "order header + main line", and
`order_items` holds every line.

If customer generation already decides the order's city/country, then
`customers.city_id` must point at that same city. Otherwise it is the
customer's home city, chosen deterministically from `(seed, customer_id)`
with the same country weights.

### Views

`order_line_details` (VIEW) has 29 columns:

| column | type | dict | from |
| --- | --- | --- | --- |
| line_id | BIGINT | | oi.id |
| order_id | BIGINT | | oi.order_id |
| line_no | SMALLINT | | oi.line_no |
| created_at | TIMESTAMPTZ | | o.created_at |
| ship_date | DATE (null) | | o.ship_date |
| payment_method | TEXT | yes | o.payment_method |
| is_returned | BOOLEAN | | o.is_returned |
| customer_id | INTEGER | | c.id |
| customer_name | TEXT | | c.name |
| customer_segment | TEXT | yes | c.segment |
| customer_since | DATE | | c.signup_date |
| customer_city | TEXT | yes | ci.name |
| country_code | TEXT | yes | co.code |
| country_name | TEXT | yes | co.name |
| region | TEXT | yes | co.region |
| currency | TEXT | yes | co.currency |
| product_id | INTEGER | | p.id |
| product_name | TEXT | yes | p.name |
| brand | TEXT | yes | p.brand |
| category | TEXT | yes | ca.name |
| department | TEXT | yes | ca.department |
| quantity | SMALLINT | | oi.quantity |
| unit_price | DOUBLE PRECISION | | oi.unit_price |
| discount | REAL | | oi.discount |
| line_total | DOUBLE PRECISION | | round((quantity*unit_price*(1-discount))::numeric, 2)::float8 |
| vat_rate | REAL | | co.vat_rate |
| shipping_weight_kg | REAL | | (p.weight_kg * oi.quantity)::real |
| latitude | DOUBLE PRECISION | | ci.latitude |
| longitude | DOUBLE PRECISION | | ci.longitude |

```
FROM order_items oi
JOIN orders o      ON o.id  = oi.order_id
JOIN customers c   ON c.id  = o.customer_id
JOIN cities ci     ON ci.id = c.city_id
JOIN countries co  ON co.code = ci.country_code
JOIN products p    ON p.id  = oi.product_id
JOIN categories ca ON ca.id = p.category_id
```

`daily_sales` (MATERIALIZED VIEW, unique index on `id`) and `daily_sales_live`
(VIEW) are built from the *same* SQL string in Rust, so they cannot drift:

```
SELECT row_number() OVER (ORDER BY day, country_code, category)::bigint AS id, *
FROM (
  SELECT (o.created_at AT TIME ZONE 'UTC')::date AS day,
         co.code AS country_code, co.name AS country_name, co.region,
         ca.name AS category,
         count(*) FILTER (WHERE oi.line_no = 1)::bigint AS orders,   -- each order counted once, via its main line
         count(*)::bigint AS lines,
         sum(oi.quantity)::bigint AS units,
         round(sum(oi.quantity * oi.unit_price * (1 - oi.discount))::numeric, 2)::float8 AS revenue,
         avg(oi.discount)::real AS avg_discount,
         count(*) FILTER (WHERE o.is_returned)::bigint AS returned_lines
  FROM <the same 7-table join>
  GROUP BY 1, 2, 3, 4, 5
) s
```

Columns are `id` BIGINT, `day` DATE, `country_code`, `country_name`,
`region` and `category` TEXT (dict), `orders`, `lines`, `units` and
`returned_lines` BIGINT, `revenue` DOUBLE PRECISION, `avg_discount` REAL.
Countries are grouped by the customer's country.

Every view column has an explicit type, and the catalog types must match what
Postgres reports. Add a test that prepares `SELECT * FROM <relation>` for each
dataset and compares the column names and types with the catalog.

### SQL per request

Always `SELECT <cols> FROM <relation> WHERE <key> > $1 ORDER BY <key> LIMIT $2`.
The relation and key come from the dataset catalog, never from user text.
For `order_lines`, the planner must turn this into an index range scan on
`order_items_pkey` plus primary-key nested loops. Check it with
`EXPLAIN ANALYZE`, and report the DB time for 100k rows next to `orders`.
`daily_sales_live` recomputes the whole aggregate on every request, by
design.

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
roughly 0.8 KB); a result larger than `CACHE_MAX_ROWS` is not cached. Each
entry records the seed generation (`seed_status.started_at`); a hit first
re-reads it with one tiny query (counted as DB time) and drops the whole cache
if the data was re-seeded since, so a re-seed while the app runs never serves
rows from the old tables.

Logging via `RUST_LOG` (default `info`). On startup the server waits for the
database, creates every missing relation of every dataset (empty tables, the
views, and the materialized view `WITH DATA`, which is cheap on empty tables;
plain views have no `IF NOT EXISTS`, so it checks `to_regclass` first), so
`/api/info` and every dataset answer before the first seed, then listens on
`0.0.0.0:$PORT`. A relation that cannot be created (an interrupted seed can
leave tables without their keys) is skipped with a warning instead of stopping
the server; `/api/info` then reports `null` stats for datasets whose relation
is missing, and the next seed replaces everything. Graceful shutdown on SIGTERM/SIGINT (Cloud
Run sends SIGTERM).

`DATABASE_URL` accepts both URL form (`postgres://user:pass@host:5432/db`) and
key/value form (`host=/cloudsql/PROJECT:REGION:INSTANCE user=... dbname=...`),
which is how Cloud Run connects to Cloud SQL over a unix socket. Connections
are made without TLS (local, docker network or the Cloud SQL socket).

### `playground seed`

| flag | env | default | meaning |
| --- | --- | --- | --- |
| `--rows` | `SEED_ROWS` | 1000000 | orders to generate (order lines are about 2.5× this) |
| `--workers` | `SEED_WORKERS` | 4 | parallel COPY connections |
| `--batch-size` | `SEED_BATCH_SIZE` | 50000 | orders per COPY chunk |
| `--mode` | `SEED_MODE` | `replace` | `replace` drops and recreates every relation; `append` adds orders (and their lines) after `max(id)` |
| `--if-empty` | `SEED_IF_EMPTY` | false | do nothing if a complete seed of the current schema version exists (used by docker compose and the Cloud Run job) |
| `--seed` | `SEED_RNG` | 42 | RNG seed |
| `--database-url` | `DATABASE_URL` | required | |

The seeder connects with the same startup retry as `serve` (the database may
still be starting), then takes a session-level advisory lock
(`pg_advisory_lock`) for the whole run, so two seeders never interleave; a
second seeder waits and then re-checks.

Completion is recorded in a one-row table:

```
seed_status (id BOOLEAN PRIMARY KEY DEFAULT true CHECK (id),
             schema_version INTEGER NOT NULL,   -- bumped whenever the seeded schema changes
             state TEXT NOT NULL,               -- 'loading' | 'complete'
             orders BIGINT NOT NULL, order_items BIGINT NOT NULL, seed BIGINT NOT NULL,
             started_at TIMESTAMPTZ NOT NULL, finished_at TIMESTAMPTZ)
```

`replace` writes `state = 'loading'` first and `'complete'` only as the very
last step, so an interrupted load (Ctrl-C, OOM, a job timeout) is never
mistaken for a finished one. `--if-empty` skips only when `seed_status` says
`complete` with the current `schema_version`; anything else (no row, `loading`,
an older schema such as a v1 database with `orders` only) gets a full
`replace`.

`replace`, step by step (each step's time is logged):

1. drop every relation of every dataset (CASCADE) and write `seed_status` = loading;
2. create and fill the small dimension tables (`countries`, `cities`, `categories`, `products`);
3. generate `customers` for every customer id that orders can reference, in parallel chunks with binary COPY;
4. create `orders` and `order_items` without indexes;
5. run the parallel chunk workers; each chunk COPYs its orders, then its order lines (`COPY ... FROM STDIN (FORMAT binary)`);
6. add the primary keys, the `created_at` index, the `order_items(order_id)` index, then the foreign keys;
7. create the views, then the materialized view `WITH DATA` and its unique index on `id`;
8. `VACUUM (ANALYZE)` every table (sets hint bits and the visibility map now, so neither first reads nor a background autovacuum land in the measurements);
9. write `seed_status` = complete.

`append` adds orders and their lines after `max(id)`, inserts any newly
referenced customers, refreshes the materialized view, runs
`VACUUM (ANALYZE)` and updates `seed_status`.

Data is deterministic: each chunk seeds its own RNG from `(seed, first id of the
chunk)`, the lines of an order depend only on `(seed, order_id)`, and customer
attributes come from `(seed, customer_id)`. So the same seed, rows and batch
size give the same tables whatever the worker count, and `append` never
repeats earlier rows. `created_at` spans 2024-01-01 to 2025-12-31 UTC (fixed,
so data does not depend on the day it is seeded). Progress and rows/s (orders
and lines) are logged.

### `playground export`

`playground export --format parquet|arrow|json --out FILE [--dataset NAME] [--rows N] [--offset ID] [--columns a,b,c] [--compression CODEC] [--dictionary] [--batch-size N]`

Writes a file from the database using the same encoders as the API. `arrow`
writes the Arrow IPC *file* format (`.arrow`, readable by DuckDB, pyarrow,
polars); `parquet` writes a Parquet file; `json` writes the JSON array.
Defaults: `--dataset orders`, `--rows 10000`, `--offset 0` (on the dataset's key column), all columns, `--batch-size 65536`
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
  "default_dataset": "orders",
  "datasets": [
    {
      "name": "order_lines",
      "label": "Order lines (7-table join view)",
      "kind": "view",
      "relation": "order_line_details",
      "key_column": "line_id",
      "description": "One sentence for the UI.",
      "sql": "The defining SELECT, pretty-printed, for the UI's Show SQL (for the orders table: its CREATE TABLE and CREATE INDEX statements).",
      "row_estimate": 2500000,
      "max_key": 10000005,
      "bytes": 512000000,
      "columns": [
        {"name": "line_id", "pg_type": "int8", "arrow_type": "Int64", "nullable": false, "dictionary": false, "group": "numeric"}
      ]
    }
  ],
  "seed": {"state": "complete", "schema_version": 2, "orders": 1000000, "order_items": 2500000, "finished_at": "2026-10-22T20:00:00Z"},
  "max_rows": 1000000,
  "formats": ["json", "arrow", "parquet"],
  "ipc_compression": ["none", "lz4", "zstd"],
  "parquet_compression": ["none", "snappy", "gzip", "zstd", "lz4", "brotli"],
  "http_compression": ["zstd", "br", "gzip"],
  "server": {"version": "0.2.0", "region": null, "service": null, "revision": null}
}
```

Datasets are listed in the order of the table in "Datasets". `kind` is
`table`, `view` or `materialized_view`. Per dataset:

- `row_estimate`: `pg_class.reltuples` for tables and the materialized view
  (falls back to `count(*)` when never analyzed); `order_lines` uses
  `order_items`' estimate; `daily_sales_live` uses `daily_sales`' estimate or
  `null`.
- `max_key`: a cheap `max(key)` (`max(order_items.id)` for `order_lines`, the
  materialized view's `max(id)` or `null` for `daily_sales_live`). `/api/info`
  never runs the live aggregate.
- `bytes`: `pg_total_relation_size` for tables and the materialized view, the
  sum over the joined tables for `order_lines`, `null` for `daily_sales_live`.

`seed` is the `seed_status` row (`null` before the first seed), so the UI can
warn while a seed is loading or incomplete. `server.region` is `$REGION`,
`server.service` is `$K_SERVICE`, `server.revision` is `$K_REVISION` (Cloud
Run sets the latter two).

### `GET /api/data`

| param | values | default | applies to |
| --- | --- | --- | --- |
| `dataset` | `orders`, `order_lines`, `daily_sales`, `daily_sales_live` | `orders` | all |
| `format` | `json`, `arrow`, `parquet` | `json` | all |
| `rows` | 1..=MAX_ROWS | 10000 | all |
| `offset` | u64: return rows with `<key column> > offset` | 0 | all |
| `columns` | comma-separated column names of the chosen dataset (unknown → 400; duplicates removed; order kept) | all | all |
| `http_compression` | bool | true | all: when false the response is never `Content-Encoding`-compressed |
| `dictionary` | bool | false | arrow: dictionary-encode the "dict" columns as `Dictionary(Int32, Utf8)` |
| `ipc_compression` | `none`, `lz4`, `zstd` | `none` | arrow: IPC body compression (LZ4_FRAME / ZSTD) |
| `parquet_compression` | `none`, `snappy`, `gzip`, `zstd`, `lz4`, `brotli` | `snappy` | parquet (`lz4` means LZ4_RAW) |
| `batch_size` | 1024..=1048576 | 65536 | arrow: rows per record batch; parquet: max row-group size |
| `cache` | bool | false | reuse rows already fetched for the same (dataset, rows, offset, columns) |

Bools accept `true/false/1/0/on/off/yes/no`; bools and keyword values are
case-insensitive. Unknown parameters are ignored (the UI adds a `_`
cache-busting nonce), an empty value counts as absent, and every known
parameter is validated even when it does not apply to the chosen format.

SQL is always `SELECT <cols> FROM <relation> WHERE <key> > $1 ORDER BY <key> LIMIT $2`,
where the relation, key and `<cols>` are quoted identifiers taken from the
dataset catalog, never from user text.

Response:

- `200` with `Content-Type`:
  - json → `application/json`
  - arrow → `application/vnd.apache.arrow.stream`
  - parquet → `application/vnd.apache.parquet`
- Headers on every successful data response:
  - `Server-Timing: db;dur=<ms>;desc="PostgreSQL", encode;dur=<ms>;desc="<format> encode", compress;dur=<ms>;desc="HTTP <encoding>", total;dur=<ms>` (milliseconds, 3 decimals; `compress` is 0 with `desc="HTTP identity"` when no HTTP compression was applied)
  - `X-Db-Ms`, `X-Encode-Ms`, `X-Compress-Ms`, `X-Total-Ms` (same numbers; total = db + encode + compress + small overhead)
  - `X-Row-Count`, `X-Column-Count`, `X-Last-Id` (key column value of the last row, or the offset if no rows)
  - `X-Body-Bytes`: payload size before any HTTP compression
  - `X-Encoded-Bytes`: bytes actually sent (after HTTP compression; equals `X-Body-Bytes` when none was applied), so the UI knows the wire size even without Resource Timing
  - `X-Format`: `json` / `arrow` / `parquet`
  - `X-Dataset`: the dataset name
  - `Vary: Accept-Encoding`
  - `X-Cache`: `hit`, `miss` or `off`
  - `Cache-Control: no-store`
- HTTP compression is done by the handler, not by a streaming middleware, so
  its CPU time is measured (`compress`) instead of hiding inside the client's
  "download" time. The encoding is chosen from `Accept-Encoding` (q-values
  respected; on ties the server prefers zstd, then br, then gzip; levels: zstd
  3, brotli 4, gzip 6), and the compressed body is built in the same
  `spawn_blocking` task as the encoder. Compression is applied only when
  `http_compression` is true, the body is larger than 1 KiB, and the payload
  is not already strongly compressed (it is skipped for Arrow IPC with
  `ipc_compression=zstd` and Parquet with `zstd`, `gzip` or `brotli`;
  re-compressing those costs CPU for 1-2% fewer bytes).
- Bodies larger than 16 MiB are sent with chunked transfer encoding (1 MiB
  chunks, no `Content-Length`), so uncompressed responses stay under Cloud
  Run's 32 MiB limit for non-streamed HTTP/1 responses.
- Errors: `400 {"error": "..."}` for bad parameters, `500 {"error": "..."}`
  for database or encoding failures (a request whose client went away ends
  as `499 {"error": "cancelled"}` in the logs; no client sees it).

Encoding runs in `tokio::task::spawn_blocking` so large payloads do not stall
the async runtime. The encode timer stops inside that task, before the
fetched rows are freed, so freeing memory is not counted as encoding.

Cancellation: when the client goes away (the UI's Cancel button, navigation),
the handler's drop guard cancels the running Postgres query
(`cancel_query`) and discards that pooled connection instead of returning it
with an abandoned result still streaming in, so the next request's DB time is
not inflated. Encoding and `/api/bench` check a cancellation flag between
record batches and between variants and stop early.

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

Server-side encoding benchmark, no network involved. Params: `dataset`, `rows` (default
100000, max `min(MAX_ROWS, 1000000)`), `offset`, `columns`, `cache`. Only one
benchmark runs at a time per server (a second one waits for the first); a
cancelled benchmark stops at the next variant. Fetches
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
`index.html`, except that a missing file under `/assets/` is a plain 404 (a
stale hashed chunk must not come back as HTML). `index.html` (and the
fallback) is sent with `Cache-Control: no-cache`; `/assets/*` (content-hashed)
with `Cache-Control: public, max-age=31536000, immutable`. If `STATIC_DIR`
does not exist, `/` returns a short plain-text note explaining how to build
the frontend.

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

- **Dataset** picker at the top: label, kind badge (table / view /
  materialized view), row estimate, the one-line description, a short note on
  what to expect, and a "Show SQL" disclosure with the dataset's `sql`. The
  column list, column presets (groups) and offset range follow the selected
  dataset. The choice is persisted; a stored dataset the server no longer has
  falls back to `default_dataset`. While `/api/info` reports a seed that is
  `loading` or missing, the UI says so.
- A prominent **Use Arrow** switch. Off → JSON. On → choose Arrow IPC or Parquet.
- Rows (presets 1k / 10k / 100k / 250k / 500k / 1M, capped by `max_rows`, plus a number input).
- Columns: presets All / Numeric / Text / Time, plus per-column checkboxes.
- Start id (offset), with an option to randomize the offset each run.
- HTTP compression on/off.
- Arrow: dictionary encoding, IPC compression, batch size.
- Parquet: compression codec, row-group size.
- Server row cache on/off.
- **Touch data** (what the "use" step does after decoding): `Every value`
  (default: reads every value of every selected column in every format;
  strings are decoded, Int64 and timestamps converted, so JSON and Arrow do
  the same work) or `One column` (sums one numeric column: the columnar fast
  path that Arrow enables, where JS objects are never built).
- Iterations (1–20) and an optional warm-up run.

### Actions

- **Run**: the current settings, N iterations.
- **Compare**: JSON vs Arrow IPC vs Parquet with the same dataset, rows and
  columns and the current per-format options, N iterations each, in
  interleaved order. With the server row cache on, each round's offset is
  first primed by one unrecorded request, so every measured request is a
  cache hit and no format pays the DB time alone (a round whose offset was
  already fetched in the same Compare, such as a fixed start id, is not primed
  again). With a random start id the warm-up draws its own offset, so round 0
  is not the only warm one.
- Results, the comparison header, history (a Dataset column), CSV/Markdown
  export and the data preview all name the dataset. The "Arrow vs JSON"
  callout on the last run only pairs runs with the same dataset, rows,
  columns, HTTP compression, cache, offset mode and Touch data, and otherwise
  lists what differs.
- **Server benchmark**: calls `/api/bench` and shows encode time and size per variant.

### Measurements per request

`fetch(url, {cache: "no-store"})` with a unique `_` nonce, then:

| metric | source |
| --- | --- |
| server DB, encode, compress, total | `X-Db-Ms` / `X-Encode-Ms` / `X-Compress-Ms` / `X-Total-Ms` (or `serverTiming`) |
| time to first byte | Resource Timing `responseStart − requestStart` (fallback: fetch resolved) |
| network wait | TTFB − server total |
| download | `responseEnd − responseStart` (fallback: `arrayBuffer()` time) |
| wire bytes | `encodedBodySize` (fallback: `X-Encoded-Bytes`) |
| payload bytes | `X-Body-Bytes` / `decodedBodySize` |
| client decode | JSON: `TextDecoder` + `JSON.parse`; Arrow: `tableFromIPC`; Parquet: `readParquet` + `tableFromIPC` |
| client use | per the Touch data setting: every value of every selected column (default), or the sum of one numeric column; the same work in every format |
| end to end | start of fetch → end of "use" |

### Results views

- Last run: stacked time bar (DB, encode, compress, network wait, download,
  decode, use, other) and sizes (wire vs payload), rows/s and MB/s.
- Comparison table: median / min / max end-to-end, median wire bytes, speed-up
  and size ratio relative to JSON, decode and use, with grouped bars. The
  headline compares browser work (decode + use), not decode alone, because
  Arrow builds its values in "use".
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
