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
playground measures all of it: database time, server encode time, time to
first byte, download time, bytes on the wire, browser decode time and the
time to actually touch the data.

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
  - **Repeat**: iterations (1-20) and an optional warm-up request.
- **Results**:
  - **Last run**: a stacked time bar (DB, encode, network wait, download,
    decode, use) and wire vs payload size, with rows/s and MB/s.
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
time), starts PostgreSQL 18.6, generates 1,000,000 rows (a few seconds), and
starts the app. Later starts take seconds and keep the data.

| Want to | Do |
| --- | --- |
| Use another port | `APP_PORT=9000 docker compose up` |
| Seed a different row count on first start | `SEED_ROWS=10000000 docker compose up --build` |
| Re-seed an existing database | `docker compose run --rm seed seed --rows 5000000` |
| Start over with an empty database | `docker compose down --volumes`, then `up` again |
| Stop, keeping the data | `docker compose down` |
| Reach PostgreSQL from the host | uncomment the `ports` lines of `db` in `docker-compose.yml` (it is not published by default, since 5432 is often taken) |

Settings can also go in a `.env` file next to `docker-compose.yml`; see
[`.env.example`](.env.example). `SEED_ROWS` only applies to an empty
database: the `seed` service runs `seed --if-empty` on every `up` and does
nothing once the table has rows. Running `seed` without `--if-empty`, as in
the re-seed command, drops and recreates the table while the app keeps
running. Each million rows takes about 250 MB in PostgreSQL.

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
    Q[validate params] --> SQL["SELECT cols FROM orders<br/>WHERE id > offset ORDER BY id LIMIT rows"]
    SQL --> R[rows]
    R -->|json| SJ[serde, straight from rows]
    R --> RB[Arrow RecordBatches]
    RB -->|arrow| IPC[IPC stream writer]
    RB -->|parquet| PW[ArrowWriter]
    SJ & IPC & PW --> H["HTTP compression (optional)<br/>chunked when > 16 MiB<br/>Server-Timing + X-* headers"]
  end
  F -->|HTTP GET| Q
  H -->|"body + timing headers"| F
  SQL <--> PG[("PostgreSQL<br/>table orders")]
  Seed["playground seed<br/>parallel binary COPY"] --> PG
```

- One binary, `playground`, with three subcommands: `serve` (API + static
  UI), `seed` (deterministic mock data, loaded with parallel binary `COPY`,
  indexes built afterwards) and `export` (write files with the same
  encoders).
- One table, `orders`, with 20 columns chosen to cover the types that behave
  differently in each format: integers of three widths, floats, a boolean,
  a timestamp with time zone, a date, high-cardinality text, low-cardinality
  text (dictionary candidates) and nullable columns.
- Requests use keyset pagination on `id`, so any offset is as fast as the
  first page. Encoding runs on a blocking thread so large responses do not
  stall other requests.
- Every data response reports its own timings (`Server-Timing`, `X-Db-Ms`,
  `X-Encode-Ms`, `X-Total-Ms`) and sizes (`X-Body-Bytes`, `X-Row-Count`).
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
| `GET /api/info` | table size and row estimate, column catalog, supported formats and codecs, server region |
| `GET /api/data` | the rows, as `json`, `arrow` (IPC stream) or `parquet` |
| `GET /api/bench` | encode time and size of every format variant for the same rows, measured on the server |

Main `/api/data` parameters: `format`, `rows` (default 10,000, at most
`MAX_ROWS`, which defaults to 1,000,000), `offset` (rows with
`id > offset`), `columns`, `http_compression`, `dictionary`,
`ipc_compression`, `parquet_compression`, `batch_size`, `cache`.

```sh
curl -s localhost:8080/api/info | jq '{row_estimate, table_bytes}'
curl -s -o orders.arrow 'localhost:8080/api/data?format=arrow&rows=100000&dictionary=true&ipc_compression=zstd'
curl -s -D - -o /dev/null 'localhost:8080/api/data?format=parquet&rows=100000&columns=id,created_at,total' | grep -i -E 'server-timing|x-'
```

## Reading the results

| Metric | Meaning |
| --- | --- |
| DB | time PostgreSQL took to return the rows (server-measured) |
| Encode | time to turn the rows into the response body (server-measured) |
| Network wait | time to first byte minus server total: latency plus queuing |
| Download | first byte to last byte |
| Decode | browser time to turn the body into usable data |
| Use | summing a numeric column over all rows, to prove the data is usable |
| End to end | from starting the request to finishing "use" |
| Wire / payload bytes | bytes received (after HTTP compression) / bytes of the body itself |

What to expect, from a local run with all 20 columns and 1,000,000 rows
(Docker on a Linux workstation, measured with curl, so no browser decode):

| Format | Payload | With HTTP zstd | Server encode | DB |
| --- | ---: | ---: | ---: | ---: |
| JSON | 435 MB | 77 MB | 1.4 s | 2.6 s |
| Arrow IPC | 183 MB | 73 MB | 0.9 s | 2.6 s |
| Parquet (snappy) | 70 MB | 54 MB | 1.9 s | 2.6 s |

And in the browser (headless Chromium on the same machine, `localhost`,
100,000 rows × 20 columns, Compare with 3 interleaved rounds, medians):

| Format | HTTP compression | End to end | On the wire | Payload | Browser decode |
| --- | --- | ---: | ---: | ---: | ---: |
| JSON | zstd | 1.00 s | 7.7 MB | 43.4 MB | 330 ms |
| Arrow IPC | zstd | 539 ms | 7.3 MB | 18.3 MB | 1.2 ms |
| Parquet (snappy) | zstd | 509 ms | 5.5 MB | 7.0 MB | 46 ms |
| JSON | off | 821 ms | 43.4 MB | 43.4 MB | 349 ms |
| Arrow IPC | off | 456 ms | 18.3 MB | 18.3 MB | 1.4 ms |
| Parquet (snappy) | off | 516 ms | 7.0 MB | 7.0 MB | 55 ms |

Arrow IPC with its own body compression trades decode time for bytes: with
dictionary encoding, LZ4 decodes in about 50 ms and zstd in about 150 ms
(both in JavaScript) for the same 100,000 rows.

How to interpret your own numbers:

- **The database time is the same for every format** and is often the
  largest part on the server. The format only changes encode, transfer and
  decode. Turn on the server row cache to take the database out of repeated
  runs.
- **HTTP compression narrows the size gap.** Browsers negotiate zstd, Brotli
  or gzip, and JSON compresses about 5-6×, so JSON with compression is close
  to Arrow on the wire. It is not free: compressing costs server CPU (gzip is
  much slower than zstd) and the browser decompresses while downloading.
  Turn compression off to see the raw sizes.
- **Arrow's biggest win is often in the browser.** `JSON.parse` of hundreds
  of megabytes takes seconds and builds a million objects; `tableFromIPC`
  mostly wraps buffers. A 1M-row JSON body is also close to V8's maximum
  string length (about 536 million characters): a real limit of the JSON
  approach for wide tables.
- **Parquet trades CPU for bytes.** It is the smallest, but the browser has
  to download the parquet-wasm module (about 5 MB, once, then cached) and
  decode in WebAssembly before converting to Arrow. It shines on slow
  links and loses on fast ones.
- **Arrow IPC compression vs HTTP compression.** IPC LZ4/zstd compresses each
  buffer and is decoded in JavaScript, which is slower than the browser's
  native HTTP decompression. Combining both buys little.
- **Dictionary encoding** shrinks the low-cardinality text columns
  (`country`, `city`, `category`, `product`, `payment_method`).
- **Caching never hides the network.** The UI fetches with
  `cache: "no-store"` and a unique nonce, and the server sends
  `Cache-Control: no-store`. Only the explicit server row cache reuses
  anything.
- **`localhost` is the best case for JSON.** With no real network, download
  time is almost zero and only encode and decode differ. Over the internet,
  bytes and latency matter: deploy to Cloud Run (below), or use the
  browser's network throttling for a rough approximation.
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

Other flags: `--offset`, `--columns a,b,c`, `--batch-size`, and `--format
json`. Then:

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
