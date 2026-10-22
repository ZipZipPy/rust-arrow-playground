# Rust Arrow Playground

A small Rust server that reads rows from PostgreSQL and returns them as
**JSON**, **Arrow IPC** or **Parquet**, and a browser UI that downloads,
decodes and uses those rows while timing every step. It answers one
question with numbers instead of opinions: *is sending Apache Arrow to the
browser worth it, compared to a plain JSON API?*

Run it on your laptop with one command, then deploy the same image to Google
Cloud (Cloud Run + Cloud SQL) and repeat the measurements over real internet
links.

## Why compare at all

A typical REST API sends rows as JSON: every row repeats every column name,
numbers and timestamps travel as text, and the browser has to parse every
character and build one JavaScript object per row. Arrow sends the same rows
column by column, in the binary layout they have in memory; the browser
wraps the received bytes as typed arrays with almost no work. Parquet is
Arrow's storage cousin: encoded and compressed per column, usually the
smallest on the wire, but it has to be decoded (here with WebAssembly) before
use.

Which one wins depends on where the time goes. On `localhost` the network is
nearly free and the cost is in encoding and parsing. Over the internet,
bytes and round trips dominate, and HTTP compression shrinks JSON a lot. The
playground measures all of it: database time, server encode time, HTTP
compression time, time to first byte, download time, bytes on the wire,
browser decode time and the time to actually touch the data.

The data comes in four **datasets**, so you can also see how the shape of the
query changes the picture: one wide table, a 7-table join view, a
materialized aggregate and the same aggregate computed live. A heavier query
adds the same database time to every format, so it never widens the Arrow vs
JSON gap by itself, but it shows how much of a real request the format can
actually win back.

## The UI

- **Header**: which server you are measuring (same origin by default, or any
  *API base URL*, for example a Cloud Run service, saved in the browser),
  its region and row count, a live round-trip time, and a light/dark theme
  switch.
- **Use Arrow** switch, the prominent control: off sends JSON; on sends Arrow
  IPC or Parquet. Next to it:
  - **Run** fetches with the current settings, *N* iterations.
  - **Compare JSON · Arrow · Parquet** runs all three formats with the same
    rows and columns, interleaved, *N* times each.
  - **Server benchmark** asks the server to encode the same rows in every
    variant (JSON; Arrow with each IPC compression, with and without
    dictionaries; Parquet with each codec) and reports encode time and size,
    with no network or browser involved.
- **Settings** sidebar:
  - **Dataset**: which relation to read, with its kind (table, view,
    materialized view), row count, what to expect, and a **Show SQL**
    disclosure with the view definition and the query each request runs:

    | Dataset | Kind | What PostgreSQL does per request |
    | --- | --- | --- |
    | Orders | table (20 columns) | index range scan on one wide table |
    | Order lines | view (29 columns) | 7-table join: order_items, orders, customers, cities, countries, products, categories |
    | Daily sales | materialized view (12 columns) | index range scan on a precomputed aggregate |
    | Daily sales, live | view (12 columns) | the same join plus GROUP BY over every order line, on every request |
  - **Rows**: presets 1k to 1M (capped by the server's `MAX_ROWS`) or any
    number.
  - **Columns**: All / Numeric / Text / Time presets and per-column
    checkboxes.
  - **Start id**: the offset, optionally randomized per run so repeated runs
    do not read the same rows.
  - **Transport**: HTTP compression on/off, server row cache on/off.
  - **Arrow IPC**: dictionary encoding, IPC body compression (none, LZ4,
    Zstandard), record batch size.
  - **Parquet**: compression codec, row-group size.
  - **Touch data**: what the browser does after decoding. *Every value*
    (default) reads every value of every selected column in every format,
    so JSON and Arrow do the same work; *One column* sums one numeric column,
    the columnar shortcut Arrow allows.
  - **Repeat**: iterations (1-20) and an optional warm-up request.
- **Results**:
  - **Last run**: a stacked time bar (DB, encode, compress, network wait,
    download, decode, use) and wire vs payload size, with rows/s and MB/s.
  - **Comparison**: median, min and max end-to-end time, median wire bytes,
    speed-up and size ratio relative to JSON, with grouped bars.
  - **Server benchmark** table with bars, a **data preview** of the first
    20 rows, a **history** of every request (copy as CSV or Markdown), and
    **How to read this**, which explains each number in plain words.

## Quick start with Docker

Prerequisites: Docker Engine 29.9.2 with the Compose plugin, about 6 GB of
disk (mostly the Rust build image and build cache), and 4 GB of memory for
1M-row requests.

```sh
docker compose up --build
```

Then open <http://localhost:8080>. The first start builds the image (a
release build of the server plus the frontend, several minutes the first
time), starts PostgreSQL 18.6, generates 1,000,000 orders with about
2.5 million order lines plus customers, products and the other dimension
tables (about 20 seconds, most of it building keys and the materialized
view), and starts the app. Later starts take seconds and keep the data.

| Want to | Do |
| --- | --- |
| Use another port | `APP_PORT=9000 docker compose up` |
| Seed a different row count on first start | `SEED_ROWS=10000000 docker compose up --build` |
| Re-seed an existing database | `docker compose run --rm seed seed --rows 5000000` |
| Start over with an empty database | `docker compose down --volumes`, then `up` again |
| Stop, keeping the data | `docker compose down` |
| Reach PostgreSQL from the host | uncomment the `ports` lines of `db` in `docker-compose.yml` (it is not published by default, since 5432 is often taken) |

Settings can also go in a `.env` file next to `docker-compose.yml`; see
[`.env.example`](.env.example). `SEED_ROWS` (the number of orders) only
applies to a database without a complete seed: the `seed` service runs
`seed --if-empty` on every `up`, which does nothing once a seed of the
current schema has finished. An interrupted seed, or a database from an older
version of the schema, is seeded again from scratch. Running `seed` without
`--if-empty`, as in the re-seed command, drops and recreates everything while
the app keeps running; the app notices and drops its row cache. Each million
orders takes about 850 MB in PostgreSQL (orders, order lines, customers and
the materialized view).

## Local development without Docker

Prerequisites: Rust 1.99.1 (rustup installs it from `rust-toolchain.toml`
automatically), Node.js 26.12.1 (`.nvmrc` / `.node-version`), and a
PostgreSQL server (18.6 is what the project pins and tests).

```sh
# 1. A database (or use `make dev-db`, which runs PostgreSQL in a container on port 5433)
createdb playground
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/playground

# 2. Generate rows and start the API on http://localhost:8080
cd server
cargo run --release -- seed --rows 1000000
cargo run --release -- serve --static-dir ../web/dist

# 3. In another terminal: the UI with hot reload on http://localhost:5173
cd web
npm ci
npm run dev
```

Use `--release` for anything you measure: a debug build encodes many times
slower and makes every format look worse than it is. `npm run dev` proxies
`/api` to `http://localhost:8080`; set `API_PROXY=https://...` to point it at
another server. `npm run build` writes `web/dist`, which `serve` hosts from
`--static-dir` (the default `./web/dist` is relative to the working
directory).

`DATABASE_URL` accepts the URL form above and the key/value form
(`host=/var/run/postgresql user=postgres dbname=playground`). The seeder and
server flags are listed in [docs/DESIGN.md](docs/DESIGN.md#cli-playground)
and in `cargo run -- serve --help`.

### Makefile shortcuts

`make` (or `make help`) lists them. The most useful:

| Target | Does |
| --- | --- |
| `make up` / `make down` / `make reset` | `docker compose` up (build, wait until healthy) / down / down with volumes |
| `make reseed SEED_ROWS=5000000` | re-seed the compose database |
| `make dev-db` | PostgreSQL 18.6 for local development on `localhost:5433`, with `playground` and `playground_test` databases |
| `make seed`, `make dev-server`, `make dev-web` | the three local development steps above, against `DATABASE_URL` (default: the `dev-db` database) |
| `make test`, `make lint`, `make fmt` | server and web tests; rustfmt, clippy and TypeScript checks; Rust formatting |

## Architecture

```mermaid
flowchart LR
  subgraph Browser
    UI[React UI] -->|"fetch /api/data?format=..."| F[fetch + Resource Timing]
    F --> D{decode}
    D -->|JSON| JP[TextDecoder + JSON.parse]
    D -->|Arrow IPC| AT[apache-arrow tableFromIPC]
    D -->|Parquet| PQ[parquet-wasm readParquet]
  end
  subgraph Server["playground serve (Rust, axum)"]
    Q[validate params] --> SQL["SELECT cols FROM dataset<br/>WHERE key > offset ORDER BY key LIMIT rows"]
    SQL --> R[rows]
    R -->|json| SJ[serde, straight from rows]
    R --> RB[Arrow RecordBatches]
    RB -->|arrow| IPC[IPC stream writer]
    RB -->|parquet| PW[ArrowWriter]
    SJ & IPC & PW --> H["HTTP compression, timed (optional)<br/>chunked when > 16 MiB<br/>Server-Timing + X-* headers"]
  end
  F -->|HTTP GET| Q
  H -->|"body + timing headers"| F
  SQL <--> PG[("PostgreSQL<br/>orders, order_items, customers, ...<br/>views + materialized view")]
  Seed["playground seed<br/>parallel binary COPY"] --> PG
```

- One binary, `playground`, with three subcommands: `serve` (API + static
  UI), `seed` (deterministic mock data, loaded with parallel binary `COPY`,
  indexes built afterwards) and `export` (write files with the same
  encoders).
- Four datasets over one normalized schema (`countries`, `cities`,
  `categories`, `products`, `customers`, `orders`, `order_items`): the wide
  `orders` table, the `order_line_details` join view, the `daily_sales`
  materialized view and the `daily_sales_live` view (same SQL as the
  materialized one). Their columns cover the types that behave differently in
  each format: integers of three widths, floats, a boolean, a timestamp with
  time zone, a date, high-cardinality text, low-cardinality text (dictionary
  candidates) and nullable columns. See [docs/DESIGN.md](docs/DESIGN.md#datasets).
- Requests use keyset pagination on each dataset's key, so any offset is as
  fast as the first page (except the live aggregate, which recomputes
  everything on purpose). Encoding and HTTP compression run on a blocking
  thread so large responses do not stall other requests, and stop early when
  the client goes away.
- Every data response reports its own timings (`Server-Timing`, `X-Db-Ms`,
  `X-Encode-Ms`, `X-Compress-Ms`, `X-Total-Ms`) and sizes (`X-Body-Bytes`,
  `X-Encoded-Bytes`, `X-Row-Count`).
  The browser adds what only it can see through the Resource Timing API.
- In Docker, one image serves everything; in Docker Compose a `db`, a
  one-shot `seed` and the `app` service share it.

## API at a glance

The full contract (every parameter, header, the column catalog and the JSON
shape) is in [docs/DESIGN.md](docs/DESIGN.md#http-api).

| Endpoint | Returns |
| --- | --- |
| `GET /api/health` | `{"status":"ok"}`, without touching the database |
| `GET /api/ping` | `204`, for round-trip time |
| `GET /api/info` | the datasets (kind, SQL, row estimate, size, column catalog), seed status, supported formats and codecs, server region |
| `GET /api/data` | the rows, as `json`, `arrow` (IPC stream) or `parquet` |
| `GET /api/bench` | encode time and size of every format variant for the same rows, measured on the server |

Main `/api/data` parameters: `dataset` (`orders`, `order_lines`,
`daily_sales`, `daily_sales_live`), `format`, `rows` (default 10,000, at most
`MAX_ROWS`, which defaults to 1,000,000), `offset` (rows whose key is
greater), `columns`, `http_compression`, `dictionary`,
`ipc_compression`, `parquet_compression`, `batch_size`, `cache`.

```sh
curl -s localhost:8080/api/info | jq '.datasets[] | {name, kind, row_estimate, bytes}'
curl -s -o lines.arrow 'localhost:8080/api/data?dataset=order_lines&format=arrow&rows=100000&dictionary=true'
curl -s -o orders.arrow 'localhost:8080/api/data?format=arrow&rows=100000&dictionary=true&ipc_compression=zstd'
curl -s -D - -o /dev/null 'localhost:8080/api/data?format=parquet&rows=100000&columns=id,created_at,total' | grep -i -E 'server-timing|x-'
```

## Reading the results

| Metric | Meaning |
| --- | --- |
| DB | time PostgreSQL took to return the rows (server-measured) |
| Encode | time to turn the rows into the response body (server-measured) |
| Compress | time the server spent on HTTP compression (server-measured; 0 when off or skipped) |
| Network wait | time to first byte minus server total: latency plus queuing |
| Download | first byte to last byte: the transfer, plus the browser decompressing as it goes |
| Decode | browser time to turn the body into JSON objects or an Arrow table |
| Use | reading every value of every column (default), or one numeric column, see *Touch data* |
| End to end | from starting the request to finishing "use" |
| Wire / payload bytes | bytes received (after HTTP compression) / bytes of the body itself |

What to expect, from a local run of the `orders` dataset with all 20 columns
and 1,000,000 rows (measured with curl, so no browser; numbers vary by about
±20% between runs on a shared machine):

| Format | Payload | With HTTP zstd | Server encode | Server zstd | DB |
| --- | ---: | ---: | ---: | ---: | ---: |
| JSON | 439 MB | 81 MB | 1.6 s | 2.6 s | 2.1-2.4 s |
| Arrow IPC | 188 MB | 75 MB | 1.2 s | 1.0 s | 2.1 s |
| Parquet (snappy) | 79 MB | 62 MB | 1.7 s | 0.4 s | 2.1 s |

And in the browser (headless Chromium on the same machine, `localhost`,
100,000 rows, Compare with 3 interleaved rounds, medians, *Touch data: every
value*):

| Dataset | Format | HTTP compression | End to end | On the wire | Payload | Decode + use |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| Orders (20 columns) | JSON | zstd | 1.05 s | 8.1 MB | 43.8 MB | 333 ms |
| | Arrow IPC | zstd | 563 ms | 7.6 MB | 18.8 MB | 57 ms |
| | Parquet (snappy) | zstd | 627 ms | 6.4 MB | 7.9 MB | 116 ms |
| | JSON | off | 774 ms | 43.8 MB | 43.8 MB | 339 ms |
| | Arrow IPC | off | 399 ms | 18.8 MB | 18.8 MB | 47 ms |
| | Parquet (snappy) | off | 531 ms | 7.9 MB | 7.9 MB | 111 ms |
| Order lines (7-table join, 29 columns) | JSON | zstd | 1.87 s | 4.7 MB | 64.7 MB | 622 ms |
| | Arrow IPC | zstd | 1.03 s | 6.9 MB | 23.7 MB | 55 ms |
| | Parquet (snappy) | zstd | 1.03 s | 3.3 MB | 4.6 MB | 106 ms |
| | JSON | off | 1.88 s | 64.7 MB | 64.7 MB | 931 ms |
| | Arrow IPC | off | 895 ms | 23.7 MB | 23.7 MB | 58 ms |
| | Parquet (snappy) | off | 958 ms | 4.6 MB | 4.6 MB | 106 ms |

Server DB time for 100,000 rows, same machine: `orders` about 0.2-0.3 s,
`order_lines` about 0.7-0.8 s (the join), `daily_sales` about 0.15 s, and
`daily_sales_live` about 5 s, because it aggregates all 2.5 million lines on
every request.

How to interpret your own numbers:

- **The database time is the same for every format.** A heavier query (the
  join view, the live aggregate) makes the database a bigger share of the
  total, so the *percentage* gained by Arrow shrinks, while the milliseconds
  saved in encode, transfer and decode stay. With the live aggregate the
  formats look almost the same, because almost all the time is the query.
  Compare it with the materialized view to see what precomputing buys. Turn
  on the server row cache to take the database out of repeated runs.
- **HTTP compression narrows the size gap, and can reverse it.** Browsers
  negotiate zstd, Brotli or gzip, and JSON compresses 5-14×. The order lines
  repeat each order's fields on every line, so JSON + zstd is *smaller* on
  the wire than plain Arrow IPC there (turn on Arrow's dictionary encoding to
  close the gap). Compression is not free: the server's compression time is
  shown as **Compress** (JSON takes the longest because it is the biggest
  body), and the browser decompresses during **Download**. Bodies that are
  already compressed (Arrow IPC with zstd, Parquet with zstd, gzip or brotli)
  are not compressed again.
- **Arrow's biggest win is in the browser.** `JSON.parse` of tens of megabytes
  takes hundreds of milliseconds and builds one object per row;
  `tableFromIPC` only wraps buffers. With *Touch data: every value* the
  Arrow path still reads every value afterwards, so the comparison is like for
  like, and Arrow is still about 6-16× faster in the browser. A 1M-row JSON
  body is also close to V8's maximum string length (about 536 million
  characters): a real limit of the JSON approach for wide tables.
- **Parquet trades CPU for bytes.** It is the smallest, but the browser has
  to download the parquet-wasm module (about 5 MB, once, then cached) and
  decode in WebAssembly before converting to Arrow. It shines on slow
  links and loses on fast ones.
- **Arrow IPC compression vs HTTP compression.** IPC LZ4/zstd compresses each
  buffer and is decoded in JavaScript, which is slower than the browser's
  native HTTP decompression.
- **Dictionary encoding** shrinks the low-cardinality text columns (country,
  city, category, product, payment method, and most text columns of the
  order lines and daily sales).
- **Caching never hides the network.** The UI fetches with
  `cache: "no-store"` and a unique nonce, and the server sends
  `Cache-Control: no-store`. Only the explicit server row cache reuses
  anything, and Compare loads each round's rows before measuring so no
  format pays the query alone.
- **`localhost` is the best case for JSON.** With no real network, the
  download is mostly the browser decompressing, and encode and decode
  dominate. Over the internet, bytes and latency matter: deploy to Cloud Run
  (below), or use the browser's network throttling for a rough
  approximation.
- Use a warm-up run and several iterations, and read medians. Compare
  interleaves the formats so a background hiccup does not hit one format
  only.

## Exporting files

`playground export` writes the same data to disk with the same encoders, for
use in other tools. `arrow` writes the Arrow IPC **file** format (also known
as Feather v2), while the API streams the IPC **stream** format.

```sh
mkdir -p exports
# Locally (DATABASE_URL set):
cargo run --release --manifest-path server/Cargo.toml -- \
  export --format parquet --out exports/orders.parquet --rows 1000000
cargo run --release --manifest-path server/Cargo.toml -- \
  export --format arrow --out exports/orders.arrow --rows 1000000 --compression zstd --dictionary

# Against the Docker Compose database (the container writes as your user):
docker compose run --rm --no-deps --user "$(id -u):$(id -g)" -v "$PWD/exports:/exports" \
  app export --format parquet --out /exports/orders.parquet --rows 1000000
```

Other flags: `--dataset` (`orders`, `order_lines`, `daily_sales`,
`daily_sales_live`), `--offset`, `--columns a,b,c`, `--batch-size`, and
`--format json`. Then:

```sh
duckdb -c "SELECT country, count(*), round(sum(total), 2) FROM 'exports/orders.parquet' GROUP BY ALL ORDER BY 3 DESC LIMIT 5"
```

```python
import pandas as pd, polars as pl, pyarrow.feather as feather, duckdb

df = pd.read_parquet("exports/orders.parquet")          # pandas
lf = pl.read_ipc("exports/orders.arrow")                 # polars (also pl.read_parquet)
orders = feather.read_table("exports/orders.arrow")      # pyarrow Table
duckdb.sql("SELECT category, avg(total) FROM orders GROUP BY ALL").show()  # DuckDB over the Arrow table
```

An Arrow body saved from the API (`curl -o orders.arrows '...format=arrow'`)
is a stream: read it with `pyarrow.ipc.open_stream`. The DuckDB CLI reads
Arrow IPC files through its `nanoarrow` community extension.

## Testing

```sh
# Server: unit tests, plus integration tests against a real database.
# Each test binary works in its own schema inside TEST_DATABASE_URL; without
# the variable the database tests are skipped.
cd server
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/playground_test cargo test
cargo fmt --check && cargo clippy --all-targets -- -D warnings

# Web: unit tests, type check, production build.
cd web
npm test
npm run typecheck
npm run build
```

CI (`.github/workflows/ci.yml`) runs the same on every push and pull
request: the Rust job against a PostgreSQL 18.6 service, the web job on
Node.js from `.nvmrc`, and a Docker job that builds the image with the
pinned Docker Engine, starts the Compose stack with 20,000 rows and checks
every format end to end.

## Deploying to Google Cloud

[`deploy/gcp/README.md`](deploy/gcp/README.md) is the step-by-step plan:
Artifact Registry, Cloud SQL for PostgreSQL 18 sized for tens of millions of
rows, the database URL in Secret Manager, a Cloud Run job that seeds 20M
rows, and a Cloud Run service sized for 1M-row responses, with notes on
regions, measuring from different places, costs and teardown.
`deploy/gcp/deploy.sh` runs it end to end.

## Project layout

```
.
├── server/                  Rust package `playground` (lib + binary)
│   ├── src/
│   │   ├── main.rs          CLI: serve | seed | export
│   │   ├── config.rs        flags and environment variables
│   │   ├── db.rs            connection pool, DDL, table stats
│   │   ├── schema.rs        column catalog of `orders` (single source of truth)
│   │   ├── query.rs         request parameters -> SQL
│   │   ├── convert.rs       PostgreSQL rows -> Arrow RecordBatch
│   │   ├── encode/          JSON, Arrow IPC and Parquet encoders
│   │   ├── cache.rs         small in-memory row cache
│   │   ├── server/          axum router, handlers, response streaming, static files
│   │   ├── seed/            mock-data generator and parallel COPY
│   │   └── export.rs        files on disk
│   └── tests/               integration tests (TEST_DATABASE_URL)
├── web/                     Vite + React + TypeScript UI
├── deploy/gcp/              Cloud Run + Cloud SQL plan and scripts
├── docs/DESIGN.md           contracts: API, headers, columns, CLI, deployment
├── Dockerfile               one image: server binary + built UI
├── docker-compose.yml       db + seed + app
├── Makefile                 shortcuts for the commands above
├── .github/workflows/ci.yml
├── rust-toolchain.toml      Rust 1.99.1
└── .nvmrc, .node-version    Node.js 26.12.1
```

Pinned versions: Rust 1.99.1, Node.js 26.12.1, Docker Engine 29.9.2,
PostgreSQL 18.6 (see [docs/DESIGN.md](docs/DESIGN.md#toolchain-versions-pinned)).
