# Deploying to Google Cloud

The local stack measures JSON, Arrow IPC and Parquet over `localhost`, where
the network is effectively free. This directory takes the same image to
Cloud Run, with the data in Cloud SQL for PostgreSQL, so the same
measurements run over real internet links: your home connection, a phone on
4G/5G, an office across the world.

This is a plan for a later step. Nothing here has been run against a real
project yet; read the script before running it.

```
 browser / curl ──HTTPS──▶ Google Front End ──HTTP/1.1 or h2c──▶ Cloud Run service
 (anywhere)                (TLS, HTTP/2, HTTP/3)                 "playground serve"
                                                                       │ unix socket
                                                                       ▼ /cloudsql/PROJECT:REGION:INSTANCE
 Cloud Run job ───────────── "playground seed" ──────────────▶ Cloud SQL for PostgreSQL 18
 (one-off)                                                      table orders, ~20M rows
```

| Resource | Default name | Purpose |
| --- | --- | --- |
| Artifact Registry repository | `arrow-playground` | holds the image built from the root `Dockerfile` |
| Cloud SQL instance (PostgreSQL 18) | `arrow-playground-pg` | the `orders` table |
| Secret Manager secret | `arrow-playground-database-url` | the full `DATABASE_URL`, password included |
| Service account | `arrow-playground@PROJECT.iam.gserviceaccount.com` | identity of the job and the service |
| Cloud Run job | `arrow-playground-seed` | `playground seed` |
| Cloud Run service | `arrow-playground` | `playground serve`: the API and the UI |

## Quick path

Prerequisites: the `gcloud` CLI, a project with billing enabled, and an
account that can create the resources above (project Owner is simplest).

```sh
gcloud auth login
PROJECT_ID=my-project REGION=asia-southeast1 deploy/gcp/deploy.sh
```

The first run takes 20-40 minutes: about 10 for the Cloud SQL instance, about
10 for the image build, and the rest for seeding 20 million rows. Later runs
rebuild the image and redeploy in about 15 minutes. The script prints the
service URL at the end.

Every setting is an environment variable with a default:

| Variable | Default | Meaning |
| --- | --- | --- |
| `PROJECT_ID` | required | Google Cloud project |
| `REGION` | `asia-southeast1` | region of everything (see [Choosing a region](#choosing-a-region)) |
| `SERVICE` | `arrow-playground` | Cloud Run service name; also the base of the job, secret and service-account names |
| `SQL_INSTANCE` | `arrow-playground-pg` | Cloud SQL instance name |
| `SQL_CPU` / `SQL_MEMORY` / `SQL_STORAGE_GB` | `4` / `16GiB` / `100` | Cloud SQL machine and SSD size (only used when creating it) |
| `DB_NAME` / `DB_USER` | `playground` / `playground` | database and user |
| `SEED_ROWS` / `SEED_WORKERS` | `20000000` / `4` | rows to generate, parallel `COPY` connections |
| `RUN_CPU` / `RUN_MEMORY` | `2` / `4Gi` | Cloud Run instance size |
| `RUN_CONCURRENCY` | `2` | concurrent requests per instance |
| `RUN_MIN_INSTANCES` / `RUN_MAX_INSTANCES` | `1` / `2` | warm instances / cost cap |
| `MAX_ROWS` | `1000000` | largest `rows` a request may ask for |
| `DB_POOL_SIZE` | `8` | connections per instance |
| `USE_HTTP2` | `false` | `true` deploys with `--use-http2` (h2c to the container) |
| `BUILDER` | `cloud-build` | `docker` builds locally and pushes instead |
| `IMAGE_TAG` / `IMAGE` | `git describe` / Artifact Registry path | override to deploy an existing image |

`deploy/gcp/teardown.sh` deletes everything again (see [Teardown](#teardown)).

## Step by step

These are the commands the script runs, in order, with the reasoning. The
snippets assume:

```sh
export PROJECT_ID=my-project REGION=asia-southeast1
export CLOUDSDK_CORE_PROJECT=$PROJECT_ID     # project for every gcloud call in this shell
IMAGE=$REGION-docker.pkg.dev/$PROJECT_ID/arrow-playground/arrow-playground:$(git describe --always --dirty)
SA=arrow-playground@$PROJECT_ID.iam.gserviceaccount.com
```

### 1. Enable the APIs

```sh
gcloud services enable run.googleapis.com sqladmin.googleapis.com \
  artifactregistry.googleapis.com cloudbuild.googleapis.com \
  secretmanager.googleapis.com iam.googleapis.com
```

### 2. Create the Artifact Registry repository

```sh
gcloud artifacts repositories create arrow-playground \
  --repository-format=docker --location=$REGION
```

### 3. Build and push the image

With Cloud Build, from the repository root (no local Docker needed; the
upload skips everything in `.gitignore`):

```sh
gcloud builds submit . --tag=$IMAGE --machine-type=e2-highcpu-8 --timeout=3600s
```

The release build compiles Arrow and Parquet with thin LTO. On
`e2-highcpu-8` it takes roughly 10 minutes; the default machine is several
times slower.

Or build locally and push (`BUILDER=docker`). Cloud Run runs `linux/amd64`,
so on an Apple Silicon Mac the build runs under emulation and is slow:

```sh
gcloud auth configure-docker $REGION-docker.pkg.dev
docker build --platform=linux/amd64 -t $IMAGE .
docker push $IMAGE
```

### 4. Create the Cloud SQL instance

```sh
gcloud sql instances create arrow-playground-pg \
  --database-version=POSTGRES_18 --edition=enterprise \
  --cpu=4 --memory=16GiB --region=$REGION \
  --availability-type=zonal --storage-type=SSD --storage-size=100 --no-backup
gcloud sql databases create playground --instance=arrow-playground-pg
```

Sizing for tens of millions of rows:

- **Data size.** A row of `orders` takes about 250 bytes on disk including
  its two indexes (`/api/info` reports `table_bytes`: 249 MB for 1M rows).
  20M rows are about 5 GB; 50M rows about 12.5 GB.
- **Memory.** The benchmark should measure PostgreSQL and the network, not
  disk reads. With 16 GiB, a 20M-row table stays in shared buffers and the OS
  page cache after the first read. For 50M+ rows use `--memory=32GiB`, or
  Enterprise Plus with its data cache (`--edition=enterprise-plus
  --tier=db-perf-optimized-N-4 --enable-data-cache`), which costs more.
- **vCPUs.** One request is one query on one backend, so a single benchmark
  stream uses about one core. Four leave room for the parallel seed and
  for a few viewers at once.
- **Storage.** SSD throughput grows with the disk size; 100 GB loads 20M rows
  noticeably faster than the 10 GB default. Automatic storage increase is on
  by default.
- **Availability and backups.** Zonal and no backups: the data is generated
  and can be regenerated in minutes.
- **Network.** The instance gets a public IP with no authorized networks, so
  it only accepts connections through the Cloud SQL connector, which is what
  Cloud Run's `--add-cloudsql-instances` uses.

### 5. Database user and the `DATABASE_URL` secret

The server accepts a key/value connection string. A `host` that starts with
`/` is a unix-socket directory, and Cloud Run mounts the instance's socket
under `/cloudsql/PROJECT:REGION:INSTANCE`. tokio-postgres does not read
`PGPASSWORD`, so the password has to be inside the string, and the whole
string goes into Secret Manager:

```sh
PASSWORD=$(openssl rand -hex 24)
CONNECTION=$(gcloud sql instances describe arrow-playground-pg --format='value(connectionName)')
gcloud sql users create playground --instance=arrow-playground-pg --password=$PASSWORD
printf 'host=/cloudsql/%s user=playground password=%s dbname=playground' "$CONNECTION" "$PASSWORD" |
  gcloud secrets create arrow-playground-database-url --replication-policy=automatic --data-file=-
```

### 6. Service account

```sh
gcloud iam service-accounts create arrow-playground --display-name="Rust Arrow Playground runtime"
gcloud projects add-iam-policy-binding $PROJECT_ID \
  --member=serviceAccount:$SA --role=roles/cloudsql.client --condition=None
gcloud secrets add-iam-policy-binding arrow-playground-database-url \
  --member=serviceAccount:$SA --role=roles/secretmanager.secretAccessor --condition=None
```

### 7. Seed with a Cloud Run job

```sh
gcloud run jobs deploy arrow-playground-seed --image=$IMAGE --region=$REGION \
  --service-account=$SA --set-cloudsql-instances=$CONNECTION \
  --set-secrets=DATABASE_URL=arrow-playground-database-url:latest \
  --args=seed,--if-empty,--rows,20000000,--workers,4 \
  --cpu=4 --memory=2Gi --task-timeout=2h --max-retries=0 --wait
```

- **What it does.** `seed` creates the table without indexes, loads it with
  parallel binary `COPY`, then builds the primary key and the `created_at`
  index and runs `ANALYZE`. `--if-empty` makes it a no-op once data exists,
  so the script can run the job on every deploy.
- **CPU and workers.** Each worker generates rows on one core and feeds one
  `COPY` connection, which keeps one database core busy. Four workers match
  a 4-vCPU instance; more workers than database vCPUs only adds contention.
- **Memory.** Workers stream chunks of `SEED_BATCH_SIZE` (50,000) rows; a
  local 1M-row seed peaked at about 12 MB. 2 GiB is simply the minimum Cloud
  Run accepts with 4 vCPUs.
- **Timeout.** 20M rows usually load in well under an hour (disk throughput
  and the index build dominate). The 2-hour task timeout leaves room for
  larger loads, and `--max-retries=0` avoids repeating a failed load.

To replace the data with a different size, run the job once with other
arguments (without `--if-empty`, `seed` drops and recreates the table):

```sh
gcloud run jobs execute arrow-playground-seed --region=$REGION \
  --args=seed,--rows,50000000,--workers,4 --wait
```

### 8. Deploy the service

```sh
gcloud run deploy arrow-playground --image=$IMAGE --region=$REGION \
  --service-account=$SA --add-cloudsql-instances=$CONNECTION \
  --set-secrets=DATABASE_URL=arrow-playground-database-url:latest \
  --set-env-vars=REGION=$REGION,MAX_ROWS=1000000,DB_POOL_SIZE=8 \
  --port=8080 --cpu=2 --memory=4Gi --concurrency=2 \
  --min-instances=1 --max-instances=2 --timeout=900 \
  --execution-environment=gen2 --cpu-boost --no-use-http2 \
  --allow-unauthenticated
gcloud run services describe arrow-playground --region=$REGION --format='value(status.url)'
```

`REGION` is only reported by `/api/info` (and shown in the UI) so results
say where the server ran; Cloud Run sets `K_SERVICE` and `K_REVISION`
itself. `PORT` is reserved and set by Cloud Run from `--port`.

## Sizing the service for 1M-row responses

- **Memory.** Measured locally with all 20 columns, one 1M-row request
  peaks at about 1.3 GB of resident memory for JSON (a 435 MB body) and
  1.5 GB for Arrow IPC (183 MB body; Parquet with snappy is 70 MB), because
  the fetched rows, the Arrow batches and the encoded body coexist for a
  moment. Three concurrent 1M-row JSON requests (Brotli) peaked at 3.7 GB.
  Hence `--memory=4Gi` with `--concurrency=2`: a third concurrent request
  starts a second instance instead of risking an out-of-memory restart in
  the middle of a measurement. For several people benchmarking at once use
  `--memory=8Gi --concurrency=4`, or lower `MAX_ROWS`. The server uses
  jemalloc and returns freed memory to the OS within seconds (resident
  memory drops back to about 80 MB when idle).
- **CPU.** Encoding runs on a blocking thread per request and HTTP
  compression on the async runtime, so 2 vCPUs serve one large request
  without the two competing. Use `--cpu=4` when several people run Compare
  at once. HTTP compression costs server CPU: locally, a 1M-row JSON
  response took 4.3 s uncompressed (435 MB), 6.4 s with zstd (77 MB, what
  current browsers negotiate) and 11.5 s with gzip (84 MB).
- **Request timeout.** A 435 MB uncompressed JSON body over a 50 Mbit/s link
  takes over a minute; over a weak mobile link much longer. `--timeout=900`
  (15 minutes) leaves room.
- **Cold starts.** `--min-instances=1` keeps one instance (and its database
  connection pool) warm, so the first run is not a cold start that skews the
  results. `--cpu-boost` shortens the cold starts that still happen when a
  second instance starts. Use the UI's warm-up option anyway.
- **`--max-instances`.** Caps cost if the URL leaks. Two is enough for
  benchmarking.

### The 32 MiB response limit and HTTP/2

Cloud Run rejects HTTP/1 responses larger than 32 MiB that are sent with a
`Content-Length`. A 1M-row JSON body is more than ten times that. The server
avoids the limit without any configuration: bodies over 16 MiB are streamed
with chunked transfer encoding in 1 MiB chunks (no `Content-Length`), and
HTTP-compressed responses are always streamed. Streamed responses have no
size limit.

`--use-http2` (`USE_HTTP2=true`) makes Cloud Run speak HTTP/2 cleartext (h2c)
to the container, which also lifts the limit. The server accepts h2c and
HTTP/1.1 on the same port, so both settings work. The browser's connection
to Google's front end uses HTTP/2 or HTTP/3 either way; the flag only changes
the hop inside Google's network. It is worth deploying both ways once and
comparing download times for large bodies, but there is no need to.

## Choosing a region

The point of the cloud deployment is the distance between the browser and
the server, so:

- Deploy Cloud Run and Cloud SQL in the **same** region. The query runs
  inside that region and the `db` time stays comparable to local runs.
- Start with the region closest to you (for Thailand or Singapore:
  `asia-southeast1`). That is the best case your users would see.
- Then measure from somewhere far away, or deploy a second copy in a distant
  region (for example `us-central1` or `europe-west1`) with a different
  `SERVICE` and `SQL_INSTANCE`, and compare. Long round trips punish
  large, slow-starting downloads more than small ones, which is where Arrow's
  smaller bodies should show.

## Measuring from different places

- **The deployed UI.** Open the service URL. The UI is served by the same
  instance, so it measures that server.
- **A local UI against the remote server.** Run `npm run dev` in `web/` and
  set *API base URL* in the UI to the Cloud Run URL. The server allows any
  origin and sends `Timing-Allow-Origin: *`, so the browser still reports
  exact sizes and timings. Handy for comparing several deployments from one
  page.
- **Different networks.** Home broadband, a phone hotspot, office Wi-Fi, a
  friend abroad. The history table records the target and can be copied as
  CSV or Markdown, so results from several places can be collected.
- **Without a browser.** curl shows the wire side alone (no decode):

  ```sh
  URL=https://arrow-playground-xxxx.a.run.app
  for f in json arrow parquet; do
    curl -sS -o /dev/null -H 'Accept-Encoding: gzip, br, zstd' \
      -w "$f: %{size_download} bytes, ttfb %{time_starttransfer}s, total %{time_total}s\n" \
      "$URL/api/data?format=$f&rows=100000"
  done
  ```

  Run it from a Compute Engine VM in another region to measure the network
  between two Google regions without your own last mile.

## Cost notes

Rough figures; prices vary by region and change, so check the
[pricing calculator](https://cloud.google.com/products/calculator) before
leaving anything running.

- **Cloud SQL is the main cost.** 4 vCPU / 16 GiB Enterprise is on the order
  of USD 0.30 per hour, about USD 200 per month if left running, plus about
  USD 0.17 per GB-month of SSD. Stop it between sessions (storage is still
  billed while stopped):

  ```sh
  gcloud sql instances patch arrow-playground-pg --activation-policy=NEVER    # stop
  gcloud sql instances patch arrow-playground-pg --activation-policy=ALWAYS   # start
  ```

- **Cloud Run.** Billed per request time; a warm minimum instance is billed
  at a reduced idle rate, on the order of tens of USD per month for 2 vCPU /
  4 GiB. Drop it when you are not measuring:

  ```sh
  gcloud run services update arrow-playground --region=$REGION --min-instances=0
  ```

- **Internet egress** is charged per GB downloaded (roughly USD 0.10-0.20
  per GB depending on the destination). One uncompressed 1M-row JSON
  response is about 0.44 GB, so a Compare with 5 iterations of 1M rows moves
  a few GB. Arrow, Parquet and HTTP compression cut that a lot, which is part
  of what you are measuring.
- **Cloud Build** has a free daily allowance on the default machine; the
  `e2-highcpu-8` machine is billed per build minute (cents per build).
- **Artifact Registry** storage for a few images of about 50 MB each is
  negligible.

## Teardown

```sh
PROJECT_ID=my-project REGION=asia-southeast1 deploy/gcp/teardown.sh
```

It asks you to type the project id (or set `YES=1`) and then deletes the
service, the job, the Cloud SQL instance with its data, the secret, the
Artifact Registry repository with its images, and the service account. The
same by hand:

```sh
gcloud run services delete arrow-playground --region=$REGION
gcloud run jobs delete arrow-playground-seed --region=$REGION
gcloud sql instances delete arrow-playground-pg
gcloud secrets delete arrow-playground-database-url
gcloud artifacts repositories delete arrow-playground --location=$REGION
gcloud projects remove-iam-policy-binding $PROJECT_ID \
  --member=serviceAccount:$SA --role=roles/cloudsql.client --condition=None
gcloud iam service-accounts delete $SA
```

Deleting the whole project (`gcloud projects delete $PROJECT_ID`) is the
surest way if it was created only for this.

## Troubleshooting

- **`gcloud builds submit` fails with a permission error on push or logs.**
  New projects run builds as the Compute Engine default service account,
  which may lack roles there. Find it with
  `gcloud builds get-default-service-account` and grant it
  `roles/artifactregistry.writer` and `roles/logging.logWriter`, or build
  locally with `BUILDER=docker`.
- **`--allow-unauthenticated` fails with an organization policy error.**
  Your organization blocks public services (domain-restricted sharing). Ask
  for an exception for this project, or deploy into a project outside the
  organization.
- **The job or service cannot connect to the database.** Check that the
  `host=/cloudsql/...` part of the secret equals the instance's
  `connectionName`, that the job or service lists the instance
  (`--set-cloudsql-instances` / `--add-cloudsql-instances`), and that the
  service account has `roles/cloudsql.client`.
- **`permission denied for schema public` during seeding.** On PostgreSQL 15+
  only the database owner may create tables in `public` by default. Connect
  as `postgres` in Cloud SQL Studio and run
  `GRANT CREATE ON SCHEMA public TO playground;`.
- **A large request fails or the instance restarts.** Check the service logs
  for out-of-memory terminations and raise `--memory` or lower
  `--concurrency` / `MAX_ROWS`.
