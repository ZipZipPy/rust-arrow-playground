# Deploying to Google Cloud

The local stack measures JSON, Arrow IPC and Parquet over `localhost`, where
the network is effectively free. This directory takes the same image to
Cloud Run in one or more regions, with the data in one Cloud SQL for
PostgreSQL instance, so the same measurements run over real internet links:
a server in your own country next to servers abroad, compared side by side
from one page, from your home connection, a phone on 4G/5G or an office
across the world.

Nothing here has been run against a real project yet; the scripts were only
dry-run against a stub `gcloud`. Read them before running them.

[![Open in Cloud Shell](https://gstatic.com/cloudssh/images/open-btn.svg)](https://shell.cloud.google.com/cloudshell/editor?cloudshell_git_repo=https://github.com/ZipZipPy/rust-arrow-playground.git&cloudshell_git_branch=main&cloudshell_workspace=.&cloudshell_tutorial=deploy/gcp/TUTORIAL.md&show=ide%2Cterminal)

The button clones this repository into Cloud Shell and opens
[`TUTORIAL.md`](TUTORIAL.md), which walks through choosing a project and
regions, running the deployment, opening the comparison, pausing and
deleting everything.

### Or let GitHub Actions run it

The **Google Cloud** workflow (`.github/workflows/google-cloud.yml`) runs
the same scripts from GitHub, so nothing needs to be installed or cloned.
It signs in through Workload Identity Federation, without a stored key.
Set it up once per project, in Cloud Shell:

```sh
curl -fsSL https://raw.githubusercontent.com/ZipZipPy/rust-arrow-playground/main/deploy/gcp/github-setup.sh | PROJECT_ID=my-project bash
```

[`github-setup.sh`](github-setup.sh) creates a workload identity pool
`github` whose provider only accepts workflow runs of this repository
(matched by its numeric id) on `main`, and a service account
`github-deployer` those runs may impersonate, with Editor, Project IAM
Admin, Secret Manager Admin and Service Account Admin on the project. Use a
project dedicated to the playground. The script ends with a line such as
`GCP_PROJECT_ID=my-project GCP_PROJECT_NUMBER=123456789012`: put both
values in [`github.env`](github.env) on `main`, or set them as repository
variables of the same names.

Then, under **Actions → Google Cloud → Run workflow**, pick `deploy`,
`pause`, `resume` or `teardown` and give the regions, nearest first
(`asia-southeast3 asia-southeast1 us-central1`; keep the same first region
for every action). A deploy lists each region's URL and the compare link on
the run's summary page. `teardown` does not ask again: choosing it in the
form is the confirmation.

```
                  nearest edge    Google's backbone
 browser ─HTTPS─▶ Google Front End ─┬─▶ Cloud Run  REGION=asia-southeast3 ─┐
 (anywhere)       (TLS, HTTP/2,     ├─▶ Cloud Run  REGION=asia-southeast1 ─┤ Cloud SQL connector,
                   HTTP/3)          └─▶ Cloud Run  REGION=us-central1 ─────┤ unix socket
                                        "playground serve", one per region │ /cloudsql/PROJECT:
                                                                           ▼ SQL_REGION:INSTANCE
 Cloud Run job ── "playground seed", once, in SQL_REGION ─────▶ Cloud SQL for PostgreSQL 18
 (one-off)                                                      in SQL_REGION: 5M orders,
                                                                ~12.5M order lines
```

| Resource | Default name | Where | Purpose |
| --- | --- | --- | --- |
| Artifact Registry repository | `arrow-playground` | `SQL_REGION` | holds the image built from the root `Dockerfile` |
| Cloud SQL instance (PostgreSQL 18) | `arrow-playground-pg` | `SQL_REGION` | the data of every dataset |
| Secret Manager secret | `arrow-playground-database-url` | global | the full `DATABASE_URL`, password included |
| Service account | `arrow-playground@PROJECT.iam.gserviceaccount.com` | global | identity of the job and the services |
| Cloud Run job | `arrow-playground-seed` | `SQL_REGION` | `playground seed` |
| Cloud Run service | `arrow-playground` | every region of `REGIONS` | `playground serve`: the API and the UI |

Cloud Run services are regional, so every region's service has the same
name, and each reports its own region in `/api/info` (`REGION` is set per
service).

## Quick path

Prerequisites: a project with billing enabled, an account that can create
the resources above (project Owner is simplest), and the `gcloud` CLI,
which Cloud Shell has ready (see
[Running it from Cloud Shell](#running-it-from-cloud-shell)).

```sh
gcloud auth login      # in Cloud Shell only if `gcloud auth list` shows no account
PROJECT_ID=my-project REGIONS="asia-southeast3 asia-southeast1 us-central1" deploy/gcp/deploy.sh
```

The first run takes roughly 30-45 minutes: the Cloud SQL instance (about 10
minutes) is created while the image builds (about 10), then the seed job
loads 5 million orders (an estimate: several minutes up to a quarter of an
hour), then each region's service deploys in a minute or two. Later runs
rebuild the image and redeploy in about 15 minutes, or a few minutes with
`BUILDER=none`. The script ends with one line per region and a link that
compares all of them:

```
==> Done
    asia-southeast3          https://arrow-playground-….run.app
    asia-southeast1          https://arrow-playground-….run.app
    us-central1              https://arrow-playground-….run.app

    Compare the servers from one page:
      https://<first>/?servers=https://<first>,https://<second>,https://<third>
```

Every setting is an environment variable with a default. `deploy.sh`,
`pause.sh` and `teardown.sh` read the shared ones from `config.sh`, so give
all three the same `PROJECT_ID` and `REGIONS`.

| Variable | Default | Meaning |
| --- | --- | --- |
| `PROJECT_ID` | required | Google Cloud project |
| `REGIONS` | `$REGION` | Cloud Run regions, separated by spaces or commas; one service in each (see [Choosing regions](#choosing-regions)) |
| `REGION` | `asia-southeast1` | the single-region form, used when `REGIONS` is not set |
| `SQL_REGION` | the first of `REGIONS` | region of the Cloud SQL instance, the image repository and the seed job; for a first region whose Cloud SQL cannot create the default machine, the nearest one that can (`asia-southeast3` Bangkok → `asia-southeast1` Singapore) |
| `SERVICE` | `arrow-playground` | Cloud Run service name in every region; also the base of the job, secret and service-account names |
| `SQL_INSTANCE` | `arrow-playground-pg` | Cloud SQL instance name |
| `SQL_CPU` / `SQL_MEMORY` / `SQL_STORAGE_GB` | `4` / `16GiB` / `50` | Cloud SQL machine and SSD size (only used when creating it) |
| `DB_NAME` / `DB_USER` | `playground` / `playground` | database and user |
| `SEED_ROWS` / `SEED_WORKERS` | `5000000` / `4` | orders to generate (order lines are about 2.5 times this), parallel `COPY` connections |
| `RUN_CPU` / `RUN_MEMORY` | `2` / `4Gi` | Cloud Run instance size |
| `RUN_CONCURRENCY` | `2` | concurrent requests per instance |
| `RUN_MIN_INSTANCES` / `RUN_MAX_INSTANCES` | `1` / `2` | warm instances per region (service-level minimum, which `pause.sh` sets to 0) / cost cap per region |
| `MAX_ROWS` | `1000000` | largest `rows` a request may ask for |
| `DB_POOL_SIZE` | `8` | database connections per instance (see [Database connections](#database-connections)) |
| `USE_HTTP2` | `false` | `true` deploys with `--use-http2` (h2c to the container) |
| `BUILDER` | `cloud-build` | `docker` builds locally and pushes; `none` builds nothing and deploys `IMAGE` |
| `IMAGE_TAG` / `IMAGE` | `git describe` / `SQL_REGION-docker.pkg.dev/PROJECT/arrow-playground/arrow-playground:TAG` | the image to build, or with `BUILDER=none` the existing image to deploy |

`deploy/gcp/pause.sh off` stops what bills by the hour between sessions and
`pause.sh on` starts it again (see [Pausing](#pausing));
`deploy/gcp/teardown.sh` deletes everything (see [Teardown](#teardown)).

## Running it from Cloud Shell

Cloud Shell is a terminal in the browser with `gcloud` signed in, so nothing
needs to be installed and the image builds in Cloud Build. The *Open in
Cloud Shell* button at the top clones this repository and starts the
tutorial; or open Cloud Shell from the console and
`git clone https://github.com/ZipZipPy/rust-arrow-playground.git`.

- **Credentials.** Cloud Shell opens repositories that are not Google's in
  a temporary environment without your credentials. If `gcloud auth list`
  shows no active account, run `gcloud auth login`. A temporary
  environment's files are deleted when the session ends; the deployment
  itself lives in your project, and the button clones the repository again.
- **Keep the tab open.** Cloud Shell ends a session after about 40 minutes
  without interaction and caps sessions at 12 hours. The first deployment
  takes about that long, so click into the terminal now and then while it
  runs.
- **If the session drops, run the same command again.** The slow parts keep
  running in Google Cloud without the session: the Cloud SQL instance
  creation, the Cloud Build build and the seed job's execution. A re-run
  skips what exists, waits for an instance that is still being created, and
  starts a second seed that waits for the first (the seeder holds an
  advisory lock) and then finds the data complete (`--if-empty`). Add
  `BUILDER=none` if the image build had already finished, to skip
  rebuilding it.
- **Set the variables again in a new session.** `export PROJECT_ID=...
  REGIONS="..."` once per session, so `deploy.sh`, `pause.sh` and
  `teardown.sh` all see the same values.

## Step by step

These are the commands the script runs, in order, with the reasoning. The
snippets assume:

```sh
export PROJECT_ID=my-project
export REGIONS="asia-southeast3 asia-southeast1 us-central1" SQL_REGION=asia-southeast1
export CLOUDSDK_CORE_PROJECT=$PROJECT_ID     # project for every gcloud call in this shell
IMAGE=$SQL_REGION-docker.pkg.dev/$PROJECT_ID/arrow-playground/arrow-playground:$(git describe --always --dirty)
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
  --repository-format=docker --location=$SQL_REGION
```

One repository, next to the database. Cloud Run copies the image into each
region when it deploys a revision, so requests never read from it, and a
service in another region pulls it only at deploy time.

### 3. Start creating the Cloud SQL instance

```sh
gcloud sql instances create arrow-playground-pg \
  --database-version=POSTGRES_18 --edition=enterprise \
  --cpu=4 --memory=16GiB --region=$SQL_REGION \
  --availability-type=zonal --storage-type=SSD --storage-size=50 --no-backup \
  --async
```

`--async` returns at once; the instance takes 5-15 minutes, which overlaps
with the image build in the next step. Step 5 waits for it.

Sizing for millions of rows:

- **Data size.** One order takes about 850 bytes on disk across all
  datasets: about 250 bytes in `orders`, about 275 bytes for its ~2.5 order
  lines in `order_items`, plus customers, indexes, foreign keys and the
  `daily_sales` materialized view (`/api/info` reports `bytes` per dataset:
  253 MB for `orders` and 554 MB for everything behind `order_lines` at 1M
  orders). The default 5M orders are about 4.3 GB (about 12.5M order lines,
  so 1M-row requests fit in every dataset); 20M orders about 17 GB; 50M
  orders about 42 GB.
- **Memory.** The benchmark should measure PostgreSQL and the network, not
  disk reads. With 16 GiB, every dataset of the default 5M orders stays in
  shared buffers and the OS page cache after the first read. At 20M orders
  the `orders` table (about 5 GB) still does, but reading the join view over
  all of its ~11 GB of joined tables needs more: give the instance 26-32 GiB
  if you benchmark `order_lines` at that size. For 50M+ rows use 8 vCPUs
  with 32 GiB (`SQL_CPU=8 SQL_MEMORY=32GiB`; Enterprise custom machines
  allow at most 6.5 GB per vCPU, so 4 vCPUs top out at 26 GiB), or
  Enterprise Plus with its data cache (`--edition=enterprise-plus
  --tier=db-perf-optimized-N-4 --enable-data-cache`), which costs more.
- **vCPUs.** One request is one query on one backend, so a single benchmark
  stream uses about one core. Four leave room for the parallel seed and
  for a few viewers at once.
- **Storage.** 50 GB is about ten times the default seed: room for the
  seed's index builds and WAL. SSD throughput grows with the disk size, so
  for 20M orders or more use `SQL_STORAGE_GB=100` or larger, which also
  loads faster. Automatic storage increase is on by default (gcloud's
  `--storage-auto-increase`), so a larger seed grows the disk rather than
  failing; storage can grow but never shrink, and it is billed while the
  instance is stopped.
- **Availability and backups.** Zonal and no backups: the data is generated
  and can be regenerated in minutes.
- **Network.** The instance gets a public IP with no authorized networks, so
  it only accepts connections through the Cloud SQL connector, which is what
  Cloud Run's `--add-cloudsql-instances` uses, from any region of the
  project.
- **Region.** Not every region offers every machine. In `asia-southeast3`
  (Bangkok), Cloud SQL Enterprise only offers shared-core, 1-vCPU
  dedicated-core and N4 machines
  ([region availability](https://cloud.google.com/sql/docs/postgres/region-availability-overview)),
  so when Bangkok comes first in `REGIONS`, `SQL_REGION` defaults to
  `asia-southeast1` (Singapore) and the script says so. The table is the
  `sql_region_for` function in `config.sh`. An existing instance is never
  moved: the scripts stop if it is in another region than `SQL_REGION`.

### 4. Build and push the image

With Cloud Build, from the repository root (no local Docker needed; the
upload skips everything in `.gitignore`):

```sh
gcloud builds submit . --tag=$IMAGE --machine-type=e2-highcpu-8 --timeout=3600s
```

The release build compiles Arrow and Parquet with thin LTO. On
`e2-highcpu-8` it takes roughly 10 minutes; the default machine is several
times slower. The build runs in Google Cloud and finishes even if the shell
that started it goes away.

Or build locally and push (`BUILDER=docker`). Cloud Run runs `linux/amd64`,
so on an Apple Silicon Mac the build runs under emulation and is slow:

```sh
gcloud auth configure-docker $SQL_REGION-docker.pkg.dev
docker build --platform=linux/amd64 -t $IMAGE .
docker push $IMAGE
```

`BUILDER=none` skips this step and deploys `IMAGE` as it is: by default the
image of the current commit, which an earlier run built. The script checks
that an Artifact Registry image exists before it creates anything.

### 5. Wait for Cloud SQL, then create the database

```sh
gcloud sql operations list --instance=arrow-playground-pg --filter='status!=DONE' --format='value(name)'
gcloud sql operations wait OPERATION --timeout=unlimited    # each name listed above
gcloud sql instances describe arrow-playground-pg --format='value(state)'    # RUNNABLE
gcloud sql databases create playground --instance=arrow-playground-pg
```

The script waits for whatever operation is still running on the instance,
so a re-run after a dropped session waits for the creation that the first
run started, and a re-run after `pause.sh off` starts the instance and
waits for that. A creation that failed while the image built leaves the
instance in state `FAILED`; the script then prints the error of its latest
operation and stops (see [Troubleshooting](#troubleshooting)).

### 6. Database user and the `DATABASE_URL` secret

The server accepts a key/value connection string. A `host` that starts with
`/` is a unix-socket directory, and Cloud Run mounts the instance's socket
under `/cloudsql/PROJECT:REGION:INSTANCE`, in every region. tokio-postgres
does not read `PGPASSWORD`, so the password has to be inside the string, and
the whole string goes into Secret Manager:

```sh
PASSWORD=$(openssl rand -hex 24)
CONNECTION=$(gcloud sql instances describe arrow-playground-pg --format='value(connectionName)')
# gcloud logs command lines (flag values included) under ~/.config/gcloud/logs,
# so turn file logging off for the command that carries the password.
CLOUDSDK_CORE_DISABLE_FILE_LOGGING=1 \
  gcloud sql users create playground --instance=arrow-playground-pg --password="$PASSWORD"
printf 'host=/cloudsql/%s user=playground password=%s dbname=playground' "$CONNECTION" "$PASSWORD" |
  gcloud secrets create arrow-playground-database-url --replication-policy=automatic --data-file=-
```

The secret is named after `SERVICE`, not after `SQL_INSTANCE`. Before it
creates anything, the script checks that an existing secret points at
`SQL_INSTANCE` in `SQL_REGION`, and stops otherwise.

### 7. Service account

```sh
gcloud iam service-accounts create arrow-playground --display-name="Rust Arrow Playground runtime"
gcloud projects add-iam-policy-binding $PROJECT_ID \
  --member=serviceAccount:$SA --role=roles/cloudsql.client --condition=None
gcloud secrets add-iam-policy-binding arrow-playground-database-url \
  --member=serviceAccount:$SA --role=roles/secretmanager.secretAccessor --condition=None
```

### 8. Seed with a Cloud Run job

```sh
gcloud run jobs deploy arrow-playground-seed --image=$IMAGE --region=$SQL_REGION \
  --service-account=$SA --set-cloudsql-instances=$CONNECTION \
  --set-secrets=DATABASE_URL=arrow-playground-database-url:latest \
  --args=seed,--if-empty,--rows,5000000,--workers,4 \
  --cpu=4 --memory=2Gi --task-timeout=2h --max-retries=0 --wait
```

- **Where.** Once, in `SQL_REGION`, next to the database: every region's
  service reads the same data, and the bulk `COPY` stays inside one region.
- **What it does.** `seed` creates the table without indexes, loads it with
  parallel binary `COPY`, then builds the primary key and the `created_at`
  index and runs `ANALYZE`. With `--if-empty` it does nothing when the
  database holds a complete seed of the current schema version, so the
  script can run the job on every deploy. Otherwise (a seed that did not
  finish, or data from an older schema version after you pulled a newer
  playground) it drops and reloads every table, which takes minutes while
  the services have no data.
- **CPU and workers.** Each worker generates rows on one core and feeds one
  `COPY` connection, which keeps one database core busy. Four workers match
  a 4-vCPU instance; more workers than database vCPUs only adds contention.
- **Memory.** Workers stream chunks of `SEED_BATCH_SIZE` (50,000) rows; a
  local 1M-row seed peaked at about 12 MB. 2 GiB is simply the minimum Cloud
  Run accepts with 4 vCPUs.
- **Timeout.** Locally 1M orders take about 20 s; on Cloud SQL disk
  throughput, the key and foreign-key builds and the materialized view
  dominate, and 5M orders should load in a quarter of an hour at most (20M
  in well under an hour). The 2-hour task timeout leaves room for larger
  loads, and `--max-retries=0` avoids repeating a failed load.

To replace the data with a different size, run the job once with other
arguments (without `--if-empty`, `seed` drops and recreates every table and
view; the running services notice the new seed and drop their row caches):

```sh
gcloud run jobs execute arrow-playground-seed --region=$SQL_REGION \
  --args=seed,--rows,20000000,--workers,4 --wait
```

### 9. Deploy the service in every region

```sh
for region in $REGIONS; do
  gcloud run deploy arrow-playground --image=$IMAGE --region=$region \
    --service-account=$SA --add-cloudsql-instances=$CONNECTION \
    --set-secrets=DATABASE_URL=arrow-playground-database-url:latest \
    --set-env-vars=REGION=$region,MAX_ROWS=1000000,DB_POOL_SIZE=8 \
    --port=8080 --cpu=2 --memory=4Gi --concurrency=2 \
    --min=1 --min-instances=default --max-instances=2 --timeout=900 \
    --execution-environment=gen2 --cpu-boost --no-use-http2 \
    --allow-unauthenticated
  gcloud run services describe arrow-playground --region=$region --format='value(status.url)'
done
```

Every service gets the full connection name of the one instance, so its
connector reaches the database across regions. `REGION` is only reported by
`/api/info` (and shown in the UI) so results say where the server ran;
Cloud Run sets `K_SERVICE` and `K_REVISION` itself. `PORT` is reserved and
set by Cloud Run from `--port`.

`--min=1` is the service-level minimum, which Google recommends over the
per-revision `--min-instances`: `pause.sh` changes it without deploying a
new revision. `--min-instances=default` removes a per-revision minimum
that an earlier version of this script set; a redeploy would otherwise
carry it into every new revision, and it would keep an instance warm after
`pause.sh off`.

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
- **The live aggregate.** `daily_sales_live` aggregates every order line on
  every request: about 5 s at 1M orders locally, so roughly half a minute
  at 5M orders, longer with Cloud SQL's default `work_mem` (sorts spill to
  disk) and from a region far from the database. That is the point of the
  dataset, but raise `work_mem` with `--database-flags=work_mem=65536`
  (64 MB) when creating the instance if you want it to stay well inside the
  request timeout, and compare it with the `daily_sales` materialized view.
- **Cold starts.** `--min=1` keeps one instance (and its database
  connection pool) warm in every region, so the first run is not a cold
  start that skews the results. `--cpu-boost` shortens the cold starts that
  still happen when a second instance starts. Use the UI's warm-up option
  anyway.
- **`--max-instances`.** Caps cost if a URL leaks. Two per region is enough
  for benchmarking.

### Database connections

Each instance opens at most `DB_POOL_SIZE` connections, each region runs at
most `RUN_MAX_INSTANCES` instances, and every region connects to the same
database: 8 × 2 × 3 = 48 connections for three regions, 96 for six. The seed
job adds about a dozen while it runs (one per `COPY` worker, then one per
index build), and during a deploy an old and a new revision can briefly
both hold connections. Cloud SQL sets `max_connections` from the memory of
the instance: 500 for 15 GB to under 30 GB, so the default 16 GiB instance
stays far below it. Cloud Run's own limit, 100 connections per instance to
one Cloud SQL database, is far above 8. If you raise `DB_POOL_SIZE`,
`RUN_MAX_INSTANCES` or the number of regions, or give the instance less
memory, keep the product under `max_connections` (`SHOW max_connections;`
in Cloud SQL Studio).

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

## Choosing regions

The point of the cloud deployment is the distance between the browser and
the server. Pick one region in your own country and one or two abroad, with
the nearest first (its service also serves the comparison page):

| Example | Region | Distance from Thailand |
| --- | --- | --- |
| same country | `asia-southeast3` (Bangkok) | a few milliseconds of round trip |
| neighbouring country | `asia-southeast1` (Singapore) | tens of milliseconds |
| another continent | `us-central1` (Iowa) or `europe-west1` (Belgium) | around 200 milliseconds |

```sh
REGIONS="asia-southeast3 asia-southeast1 us-central1"
REGIONS="asia-southeast3 asia-southeast1 europe-west1 us-central1"
```

From elsewhere, use the same pattern: your nearest region, a neighbour, and
one on another continent ([Cloud Run locations](https://cloud.google.com/run/docs/locations)).

How the traffic flows, and why that is realistic:

- **The browser always connects nearby.** A `run.app` URL resolves to
  Google's edge: the Google Front End closest to the browser terminates
  TLS and speaks HTTP/2 or HTTP/3 with it, and Google's private backbone
  carries the request to the service's region and the response back. Every
  Cloud Run app reaches its users this way, so the measurement shows what a
  real app in that region would see: your last mile to the edge is the same
  for every server, and the distance changes the backbone part.
- **Distance hurts large responses.** Each round trip costs more the
  farther the region, and a download needs many round trips while the
  congestion window grows. Smaller bodies (Arrow, Parquet, HTTP
  compression) and fewer round trips should matter more against a far
  server than against a near one; that is what the cross-country
  comparison shows.
- **The database stays in one region.** A far service's database queries
  cross regions too. Compare servers in the UI therefore always uses the
  server row cache and primes each round with one unrecorded request, so
  the measured requests are cache hits, which never wait for the database
  (see `docs/DESIGN.md`). Their *DB* part is only the cache lookup on every
  server, and the times show the distance between the browser and each
  server: compare the RTT column and the *Network wait* and *Download*
  parts. Without the cache (Run and Compare with the cache off), a far
  service's *DB* time includes its round trips to `SQL_REGION`.

### Adding or removing a region

Run `deploy.sh` again with the whole new list and the same first region
(the first region decides `SQL_REGION`, and the instance never moves). The
services that exist are redeployed with the same image, the new region
gets its own service, and the seed job finds the complete seed and does
nothing. `BUILDER=none` reuses the image of the current commit; if there
is none (the repository has newer commits), the script stops before
creating anything:

```sh
PROJECT_ID=my-project REGIONS="asia-southeast3 asia-southeast1 us-central1 europe-west1" BUILDER=none deploy/gcp/deploy.sh
```

A region left out of the list keeps its service, connected to the database
and billed for its warm instance. Delete it with
`gcloud run services delete arrow-playground --region=REGION`; `pause.sh`
and `teardown.sh` reach it either way.

## Measuring from different places

- **Compare servers from one page.** Open the link the script prints:
  `https://<first>/?servers=https://<first>,https://<second>,...`. The UI
  adds every URL in `servers` to its saved server list (the contract is in
  `docs/DESIGN.md`) and compares JSON, Arrow IPC and Parquet against each
  server side by side; `/api/info` names each server's region. The link
  works from any UI, for example a local one:
  `http://localhost:5173/?servers=https://<first>,https://<second>`.
- **The deployed UI.** Open any service URL. The UI is served by that
  instance, so by default it measures that server.
- **A local UI against the remote servers.** Run `npm run dev` in `web/` and
  add the Cloud Run URLs as servers. The server allows any origin and sends
  `Timing-Allow-Origin: *`, so the browser still reports exact sizes and
  timings.
- **Different networks.** Home broadband, a phone hotspot, office Wi-Fi, a
  friend abroad. The history table records the target and can be copied as
  CSV or Markdown, so results from several places can be collected.
- **Without a browser.** curl shows the wire side alone (no decode). With
  `cache=true` only the first request to each server reads the database:

  ```sh
  for url in https://arrow-playground-xxxx.a.run.app https://arrow-playground-yyyy.a.run.app; do
    for f in json arrow parquet; do
      curl -sS -o /dev/null -H 'Accept-Encoding: gzip, br, zstd' \
        -w "$url $f: %{size_download} bytes, ttfb %{time_starttransfer}s, total %{time_total}s\n" \
        "$url/api/data?format=$f&rows=100000&cache=true"
    done
  done
  ```

  Run it from a Compute Engine VM in another region to measure the network
  between two Google regions without your own last mile.

## Cost notes

Rough figures from the
[Cloud SQL](https://cloud.google.com/sql/pricing) and
[Cloud Run](https://cloud.google.com/run/pricing) price lists for Iowa
(`us-central1`); other regions cost somewhat more, and prices change, so
check the [pricing calculator](https://cloud.google.com/products/calculator)
before leaving anything running. With N regions there is one Cloud SQL
instance and N Cloud Run services:

| While running | Per hour | Per month |
| --- | --- | --- |
| Cloud SQL, 4 vCPU / 16 GiB Enterprise (one instance) | about USD 0.28 | about USD 200 |
| Cloud SQL, 50 GB SSD (also while stopped) | about USD 0.01 | about USD 9 |
| Cloud Run, one warm 2 vCPU / 4 GiB instance per region, idle rate | about USD 0.05 × N | about USD 40 × N |
| Three regions in total | about USD 0.45 | about USD 330 |
| Paused: SSD and the stopped instance's IPv4 address | about USD 0.02 | about USD 16 |

- **Cloud SQL is the main cost**, and it does not grow with the number of
  regions. `pause.sh off` stops it (see [Pausing](#pausing)); storage is
  still billed, and the price list also bills the IPv4 address of a
  stopped instance at USD 0.01 per hour.
- **Cloud Run.** A warm minimum instance is billed at the reduced idle rate
  (USD 0.0000025 per vCPU-second and per GiB-second); requests are billed
  at the active rate while they run. `pause.sh off` sets the minimum to 0
  in every region, after which an idle service costs nothing.
- **Internet egress** is charged per GB downloaded (roughly USD 0.10-0.20
  per GB depending on the destination), and Compare servers downloads the
  same data once per server. One uncompressed 1M-row JSON response is about
  0.44 GB, so a comparison of 5 iterations of 1M rows against three servers
  moves several GB. Arrow, Parquet and HTTP compression cut that a lot,
  which is part of what you are measuring.
- **Between regions.** Rows that a far service reads from the database
  (cache misses and the priming requests) cross regions, which Google
  bills as inter-region transfer (free within a continent for Google
  products, about USD 0.12 per GB between continents). The cache keeps this
  small next to the egress to the browser.
- **Cloud Build** has a free daily allowance on the default machine; the
  `e2-highcpu-8` machine is billed per build minute (cents per build).
- **Artifact Registry** storage for a few images of about 50 MB each is
  negligible, and one repository serves every region.

## Pausing

Between sessions, stop what bills by the hour and keep the data:

```sh
PROJECT_ID=my-project REGIONS="asia-southeast3 asia-southeast1 us-central1" deploy/gcp/pause.sh off
PROJECT_ID=my-project REGIONS="asia-southeast3 asia-southeast1 us-central1" deploy/gcp/pause.sh on
```

`off` sets every regional service's minimum instances to 0 and stops the
Cloud SQL instance (activation policy `NEVER`); `on` starts the instance
(a minute or two) and sets the minimum back to `RUN_MIN_INSTANCES`. Neither
deploys a new revision, and both first wait for an operation still running
on the instance. Both also reach a service of the same name in a
region missing from `REGIONS`, so a region that was dropped from the list
does not keep billing. While paused, a request still starts an instance,
which cannot reach the database. Running `deploy.sh` again also starts a
stopped instance. The same by hand:

```sh
for region in $REGIONS; do
  gcloud run services update arrow-playground --region=$region --min=0     # --min=1 to resume
done
gcloud sql instances patch arrow-playground-pg --activation-policy=NEVER    # ALWAYS to resume
```

## Teardown

```sh
PROJECT_ID=my-project REGIONS="asia-southeast3 asia-southeast1 us-central1" deploy/gcp/teardown.sh
```

It lists what it will delete, asks you to type the project id (or set
`YES=1`), and then deletes the service in every region (those in `REGIONS`
and any other region with a service of the same name), the job, the Cloud
SQL instance with its data, the secret, the Artifact Registry repository
with its images, and the service account. Give it the same `REGIONS` (or
`SQL_REGION`) as the deployment, so that it looks for the job and the
repository in the right region; it stops if the instance is elsewhere.
Before deleting the instance it waits for an operation still running on
it, such as a creation that a stopped `deploy.sh` left behind. The same by
hand:

```sh
for region in $REGIONS; do gcloud run services delete arrow-playground --region=$region; done
gcloud run jobs delete arrow-playground-seed --region=$SQL_REGION
gcloud sql instances delete arrow-playground-pg
gcloud secrets delete arrow-playground-database-url
gcloud artifacts repositories delete arrow-playground --location=$SQL_REGION
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
- **`Cloud SQL instance ... is in A, not in SQL_REGION=B`.** The instance
  exists from an earlier deployment with a different first region, and it
  never moves. Put that first region first in `REGIONS` again (see
  [Adding or removing a region](#adding-or-removing-a-region)), or run with
  `SQL_REGION=A`.
- **`Secret ... points at host=/cloudsql/..., not at ...`.** The secret,
  the service account, the job and the services are named after `SERVICE`,
  so a new `SQL_INSTANCE` alone does not make a second deployment: it
  would share them with the first. Use the `SQL_INSTANCE` and `SQL_REGION`
  of the existing deployment. If the instance the secret names no longer
  exists, delete the secret
  (`gcloud secrets delete arrow-playground-database-url`) and run again.
- **`Cloud SQL instance ... is FAILED, not RUNNABLE`.** Creating the
  instance failed while the image built, for example because the zone had
  no capacity or the region does not offer the custom machine (see the
  *Region* note in step 3); the line after it shows the error. Delete the
  failed instance (`gcloud sql instances delete arrow-playground-pg`), then
  run again, with `SQL_REGION` set to a nearby region if the error is
  about this one.
- **The job or a service cannot connect to the database.** Check that the
  `host=/cloudsql/...` part of the secret equals the instance's
  `connectionName`, that the job or service lists the instance
  (`--set-cloudsql-instances` / `--add-cloudsql-instances`), that the
  service account has `roles/cloudsql.client`, and that the instance is not
  stopped (`pause.sh on`).
- **`permission denied for schema public` during seeding.** On PostgreSQL 15+
  only the database owner may create tables in `public` by default. Connect
  as `postgres` in Cloud SQL Studio and run
  `GRANT CREATE ON SCHEMA public TO playground;`.
- **A large request fails or the instance restarts.** Check the service logs
  for out-of-memory terminations and raise `--memory` or lower
  `--concurrency` / `MAX_ROWS`.
- **Cloud Shell: `gcloud` has no active account, or asks to authorize.**
  Run `gcloud auth login`, or click *Authorize* in the prompt.
