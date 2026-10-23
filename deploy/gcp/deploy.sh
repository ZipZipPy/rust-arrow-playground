#!/usr/bin/env bash
# Deploys Rust Arrow Playground to Google Cloud, in one or more regions:
#   Artifact Registry (image) and Cloud SQL for PostgreSQL 18 (data) in
#   SQL_REGION -> Cloud Run job (seed) in SQL_REGION -> one Cloud Run
#   service (API + UI) in every region of REGIONS, all reading that database.
#
# Usage, from any directory:
#   PROJECT_ID=my-project REGION=asia-southeast1 deploy/gcp/deploy.sh
#   PROJECT_ID=my-project REGIONS="asia-southeast3 asia-southeast1 us-central1" deploy/gcp/deploy.sh
#
# Re-runnable: resources that already exist are kept as they are (the Cloud
# SQL instance is never resized or moved by this script, only started again
# if pause.sh stopped it), the image is rebuilt unless BUILDER=none, and the
# job and the services are redeployed with it. The seed job runs with
# --if-empty: it keeps a complete seed of the current schema version, and
# otherwise drops and reloads every table (minutes, while the services have
# no data). See README.md in this directory for the reasoning behind every
# setting, for pause.sh and for teardown.sh.
#
# Requires: gcloud (authenticated: `gcloud auth login`), openssl, and Docker
# only when BUILDER=docker.
set -euo pipefail

# --- Configuration (override any of these through the environment) ---------

# PROJECT_ID, REGIONS, SQL_REGION, resource names and the helpers.
# shellcheck source=SCRIPTDIR/config.sh
source "$(dirname "${BASH_SOURCE[0]}")/config.sh"

SQL_CPU="${SQL_CPU:-4}"
SQL_MEMORY="${SQL_MEMORY:-16GiB}"
SQL_STORAGE_GB="${SQL_STORAGE_GB:-50}"
DB_NAME="${DB_NAME:-playground}"
DB_USER="${DB_USER:-playground}"

SEED_ROWS="${SEED_ROWS:-5000000}"
SEED_WORKERS="${SEED_WORKERS:-4}"

RUN_CPU="${RUN_CPU:-2}"
RUN_MEMORY="${RUN_MEMORY:-4Gi}"
RUN_CONCURRENCY="${RUN_CONCURRENCY:-2}"
RUN_MIN_INSTANCES="${RUN_MIN_INSTANCES:-1}"
RUN_MAX_INSTANCES="${RUN_MAX_INSTANCES:-2}"
MAX_ROWS="${MAX_ROWS:-1000000}"
DB_POOL_SIZE="${DB_POOL_SIZE:-8}"
USE_HTTP2="${USE_HTTP2:-false}"

# cloud-build: build remotely with Cloud Build (no local Docker needed).
# docker:      build locally for linux/amd64 and push.
# none:        build nothing and deploy IMAGE as it is.
BUILDER="${BUILDER:-cloud-build}"
case "$BUILDER" in
  cloud-build | docker | none) ;;
  *)
    echo "BUILDER must be cloud-build, docker or none, not '$BUILDER'" >&2
    exit 1
    ;;
esac

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE_TAG="${IMAGE_TAG:-$(git -C "$REPO_ROOT" describe --always --dirty 2>/dev/null || date -u +%Y%m%d-%H%M%S)}"
IMAGE="${IMAGE:-${SQL_REGION}-docker.pkg.dev/${PROJECT_ID}/${AR_REPO}/arrow-playground:${IMAGE_TAG}}"

log "Deploying $SERVICE to ${REGION_LIST[*]}; Cloud SQL, image and seed job in $SQL_REGION"
if [[ -n "$SQL_REGION_NOTE" ]]; then echo "    $SQL_REGION_NOTE" >&2; fi

# New service accounts take a few seconds to become visible to IAM.
retry() {
  local attempt
  for attempt in 1 2 3 4 5; do
    "$@" && return 0
    log "Retrying in $((attempt * 5))s: $*"
    sleep $((attempt * 5))
  done
  "$@"
}

# --- 1. APIs ----------------------------------------------------------------

log "Enabling APIs (no-op when already enabled)"
gcloud services enable \
  run.googleapis.com \
  sqladmin.googleapis.com \
  artifactregistry.googleapis.com \
  cloudbuild.googleapis.com \
  secretmanager.googleapis.com \
  iam.googleapis.com

# Projects created since 2024 run Cloud Build as the Compute Engine default
# service account, which only has what it needs to read the uploaded source,
# write logs and push the image when the project still grants it Editor
# automatically. Grant the Cloud Build role outright, early, so the grant has
# spread by the time the build starts. Older projects build as the Cloud
# Build service account, which has the role already.
if [[ "$BUILDER" == cloud-build ]]; then
  project_number="$(gcloud projects describe "$PROJECT_ID" --format='value(projectNumber)')"
  build_sa="${project_number}-compute@developer.gserviceaccount.com"
  if exists gcloud iam service-accounts describe "$build_sa"; then
    log "Granting $build_sa the Cloud Build role"
    retry gcloud projects add-iam-policy-binding "$PROJECT_ID" \
      --member="serviceAccount:$build_sa" \
      --role=roles/cloudbuild.builds.builder \
      --condition=None >/dev/null
  fi
  unset project_number build_sa
fi

# Stop here, before creating anything, if the existing database is in
# another region, the image to deploy does not exist, or the DATABASE_URL
# secret belongs to another instance (the secret, the job and the services
# are named after SERVICE, not SQL_INSTANCE, so a new SQL_INSTANCE alone
# would get a secret it cannot connect with).
check_sql_instance
if [[ "$BUILDER" == none && "$IMAGE" == *-docker.pkg.dev/* ]] &&
  ! gcloud artifacts docker images describe "$IMAGE" >/dev/null; then
  echo "Build the image once without BUILDER=none, or set IMAGE to an existing image." >&2
  exit 1
fi
if exists gcloud secrets describe "$DB_SECRET"; then
  database_url="$(CLOUDSDK_CORE_DISABLE_FILE_LOGGING=1 gcloud secrets versions access latest --secret="$DB_SECRET")"
  if [[ "$database_url" != "host=/cloudsql/$PROJECT_ID:$SQL_REGION:$SQL_INSTANCE "* ]]; then
    echo "Secret $DB_SECRET points at ${database_url%% *}, not at $SQL_INSTANCE in $SQL_REGION." >&2
    echo "Run with the SQL_INSTANCE and SQL_REGION it was created for, or delete the secret if that instance is gone." >&2
    exit 1
  fi
  unset database_url
fi

# --- 2. Artifact Registry ---------------------------------------------------

# One repository next to the database. Cloud Run copies the image into each
# region when it deploys a revision, so requests never read from here.
if ! exists gcloud artifacts repositories describe "$AR_REPO" --location="$SQL_REGION"; then
  log "Creating Artifact Registry repository $AR_REPO in $SQL_REGION"
  gcloud artifacts repositories create "$AR_REPO" \
    --repository-format=docker \
    --location="$SQL_REGION" \
    --description="Rust Arrow Playground images"
fi

# --- 3. Cloud SQL for PostgreSQL 18: start creating it ----------------------

# Creation runs in the background (--async) while the image builds; step 5
# waits for it. A re-run after a dropped session waits for the same
# operation instead of starting another.
if [[ -z "$SQL_EXISTS" ]]; then
  log "Creating Cloud SQL instance $SQL_INSTANCE in $SQL_REGION (5-15 minutes, while the image builds)"
  # Enterprise edition with a custom machine: 16 GiB keeps every dataset of
  # the default 5M-order seed (about 4.3 GB) in memory. 50 GB of SSD is
  # about ten times that, for the seed's index builds and WAL; automatic
  # storage increase (on by default) grows it for larger seeds. Zonal, no
  # backups: the data can be regenerated. Public IP without authorized
  # networks only accepts connections through the Cloud SQL connector,
  # which is what Cloud Run uses, from any region.
  gcloud sql instances create "$SQL_INSTANCE" \
    --database-version=POSTGRES_18 \
    --edition=enterprise \
    --cpu="$SQL_CPU" \
    --memory="$SQL_MEMORY" \
    --region="$SQL_REGION" \
    --availability-type=zonal \
    --storage-type=SSD \
    --storage-size="$SQL_STORAGE_GB" \
    --no-backup \
    --async >/dev/null
elif [[ "$(gcloud sql instances describe "$SQL_INSTANCE" --format='value(settings.activationPolicy)')" == NEVER ]]; then
  log "Starting Cloud SQL instance $SQL_INSTANCE (pause.sh stopped it)"
  gcloud sql instances patch "$SQL_INSTANCE" --activation-policy=ALWAYS --async >/dev/null
fi

# --- 4. Image ---------------------------------------------------------------

case "$BUILDER" in
  cloud-build)
    log "Building $IMAGE with Cloud Build"
    # 8 vCPUs: the release build (Arrow, Parquet, thin LTO) takes roughly
    # 10 minutes there, against 30+ on the default machine.
    gcloud builds submit "$REPO_ROOT" \
      --tag="$IMAGE" \
      --machine-type=e2-highcpu-8 \
      --timeout=3600s
    ;;
  docker)
    log "Building $IMAGE locally and pushing it"
    gcloud auth configure-docker "${IMAGE%%/*}" --quiet
    docker build --platform=linux/amd64 --tag="$IMAGE" "$REPO_ROOT"
    docker push "$IMAGE"
    ;;
  none)
    log "Deploying the existing image $IMAGE (BUILDER=none)"
    ;;
esac

# --- 5. Cloud SQL: wait until ready, then the database ----------------------

wait_for_sql
# A creation that failed while the image built (a zone out of capacity, a
# machine the region does not offer) is already done and leaves the
# instance in state FAILED; say so here, with the error, rather than fail
# on the database below.
state="$(gcloud sql instances describe "$SQL_INSTANCE" --format='value(state)')"
if [[ "$state" != RUNNABLE ]]; then
  echo "Cloud SQL instance $SQL_INSTANCE is $state, not RUNNABLE. Its latest operation:" >&2
  gcloud sql operations list --instance="$SQL_INSTANCE" --limit=1 \
    --format='value(operationType,status,error.errors[0].code,error.errors[0].message)' >&2
  if [[ "$state" == FAILED ]]; then
    echo "Delete it (gcloud sql instances delete $SQL_INSTANCE) and run again, with SQL_REGION set to a nearby region if the error is about this one." >&2
  fi
  exit 1
fi
CONNECTION_NAME="$(gcloud sql instances describe "$SQL_INSTANCE" --format='value(connectionName)')"

if ! exists gcloud sql databases describe "$DB_NAME" --instance="$SQL_INSTANCE"; then
  log "Creating database $DB_NAME"
  gcloud sql databases create "$DB_NAME" --instance="$SQL_INSTANCE"
fi

# --- 6. Database user and DATABASE_URL secret -------------------------------

# tokio-postgres does not read PGPASSWORD, so the password lives inside the
# connection string, and the whole string is the secret. Created once; delete
# the secret to rotate the password on the next run.
if ! exists gcloud secrets describe "$DB_SECRET"; then
  log "Creating database user $DB_USER and secret $DB_SECRET"
  password="$(openssl rand -hex 24)"
  # gcloud logs every command line, flag values included, under
  # ~/.config/gcloud/logs; keep the password out of those files.
  if [[ -n "$(gcloud sql users list --instance="$SQL_INSTANCE" --filter="name=$DB_USER" --format='value(name)')" ]]; then
    CLOUDSDK_CORE_DISABLE_FILE_LOGGING=1 \
      gcloud sql users set-password "$DB_USER" --instance="$SQL_INSTANCE" --password="$password"
  else
    CLOUDSDK_CORE_DISABLE_FILE_LOGGING=1 \
      gcloud sql users create "$DB_USER" --instance="$SQL_INSTANCE" --password="$password"
  fi
  printf 'host=/cloudsql/%s user=%s password=%s dbname=%s' \
    "$CONNECTION_NAME" "$DB_USER" "$password" "$DB_NAME" |
    gcloud secrets create "$DB_SECRET" --replication-policy=automatic --data-file=-
  unset password
fi

# --- 7. Runtime service account ---------------------------------------------

if ! exists gcloud iam service-accounts describe "$RUNTIME_SA"; then
  log "Creating service account $RUNTIME_SA"
  gcloud iam service-accounts create "$RUNTIME_SA_NAME" \
    --display-name="Rust Arrow Playground runtime"
fi

log "Granting $RUNTIME_SA Cloud SQL client and access to $DB_SECRET"
retry gcloud projects add-iam-policy-binding "$PROJECT_ID" \
  --member="serviceAccount:$RUNTIME_SA" \
  --role=roles/cloudsql.client \
  --condition=None >/dev/null
retry gcloud secrets add-iam-policy-binding "$DB_SECRET" \
  --member="serviceAccount:$RUNTIME_SA" \
  --role=roles/secretmanager.secretAccessor \
  --condition=None >/dev/null

# --- 8. Seed job ------------------------------------------------------------

log "Deploying and running seed job $SEED_JOB in $SQL_REGION ($SEED_ROWS orders, skipped if a complete seed exists)"
# The seeder is CPU-bound on data generation and on the database side of
# COPY; more workers than Cloud SQL has vCPUs does not help.
gcloud run jobs deploy "$SEED_JOB" \
  --image="$IMAGE" \
  --region="$SQL_REGION" \
  --service-account="$RUNTIME_SA" \
  --set-cloudsql-instances="$CONNECTION_NAME" \
  --set-secrets="DATABASE_URL=${DB_SECRET}:latest" \
  --args="seed,--if-empty,--rows,${SEED_ROWS},--workers,${SEED_WORKERS}" \
  --cpu=4 \
  --memory=2Gi \
  --task-timeout=2h \
  --max-retries=0 \
  --wait

# --- 9. Services, one per region --------------------------------------------

if [[ "$USE_HTTP2" == true ]]; then http2_flag=--use-http2; else http2_flag=--no-use-http2; fi

# Sizing for 1M-row responses (see README.md): one such request peaks at
# about 1.5 GB of memory, so 4 GiB holds two at a time; a third goes to a
# second instance. One instance stays warm (--min, the service-level minimum,
# which pause.sh changes without a new revision) so cold starts do not land
# in the measurements; --min-instances=default removes the per-revision
# minimum that earlier versions of this script set, which pause.sh cannot
# lower. Slow links get 15 minutes to download. Every region connects to
# the one database, with at most DB_POOL_SIZE x RUN_MAX_INSTANCES x regions
# connections in all (48 for three regions), far below its max_connections
# (500 with 16 GiB; see README.md).
urls=()
for region in "${REGION_LIST[@]}"; do
  log "Deploying service $SERVICE in $region"
  gcloud run deploy "$SERVICE" \
    --image="$IMAGE" \
    --region="$region" \
    --service-account="$RUNTIME_SA" \
    --add-cloudsql-instances="$CONNECTION_NAME" \
    --set-secrets="DATABASE_URL=${DB_SECRET}:latest" \
    --set-env-vars="REGION=${region},MAX_ROWS=${MAX_ROWS},DB_POOL_SIZE=${DB_POOL_SIZE}" \
    --port=8080 \
    --cpu="$RUN_CPU" \
    --memory="$RUN_MEMORY" \
    --concurrency="$RUN_CONCURRENCY" \
    --min="$RUN_MIN_INSTANCES" \
    --min-instances=default \
    --max-instances="$RUN_MAX_INSTANCES" \
    --timeout=900 \
    --execution-environment=gen2 \
    --cpu-boost \
    "$http2_flag" \
    --allow-unauthenticated
  # --allow-unauthenticated only warns when it cannot set the policy (the
  # deploying account lacks run.services.setIamPolicy), which would leave a
  # service that answers 403 to every browser. This fails instead.
  retry gcloud run services add-iam-policy-binding "$SERVICE" \
    --region="$region" \
    --member=allUsers \
    --role=roles/run.invoker \
    --condition=None >/dev/null
  urls+=("$(gcloud run services describe "$SERVICE" --region="$region" --format='value(status.url)')")
done

# --- 10. Summary ------------------------------------------------------------

scripts="$(dirname "${BASH_SOURCE[0]}")"
settings="PROJECT_ID=$PROJECT_ID REGIONS=\"${REGION_LIST[*]}\""
log "Done"
{
  for i in "${!REGION_LIST[@]}"; do
    printf '    %-24s %s\n' "${REGION_LIST[i]}" "${urls[i]}"
  done
  echo
  if ((${#urls[@]} > 1)); then
    # The UI adds every URL in ?servers= to its server list (docs/DESIGN.md).
    echo "    Compare the servers from one page:"
    echo "      ${urls[0]}/?servers=$(IFS=,; echo "${urls[*]}")"
  else
    echo "    Open ${urls[0]} in a browser, or point a local UI at it."
  fi
  # Each server reports its own REGION in /api/info.
  # shellcheck disable=SC2016 # $u is for the user's shell
  printf '    Quick check:  for u in %s; do curl -sS "$u/api/info" | grep -o %s; done\n' \
    "${urls[*]}" "'\"server\":{[^}]*}'"
  echo "    Pause costs:  $settings $scripts/pause.sh off    (on to resume)"
  echo "    Tear down:    $settings SQL_REGION=$SQL_REGION $scripts/teardown.sh"
} >&2

# Run from GitHub Actions (.github/workflows/google-cloud.yml), the links
# also go on the run's summary page.
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  {
    echo "| Region | Service |"
    echo "| --- | --- |"
    for i in "${!REGION_LIST[@]}"; do
      echo "| ${REGION_LIST[i]} | ${urls[i]} |"
    done
    if ((${#urls[@]} > 1)); then
      echo
      echo "Compare the servers from one page: ${urls[0]}/?servers=$(IFS=,; echo "${urls[*]}")"
    fi
  } >>"$GITHUB_STEP_SUMMARY"
fi
