#!/usr/bin/env bash
# Deploys Rust Arrow Playground to Google Cloud:
#   Artifact Registry (image) -> Cloud SQL for PostgreSQL 18 (data)
#   -> Cloud Run job (seed) -> Cloud Run service (API + UI).
#
# Usage, from any directory:
#   PROJECT_ID=my-project REGION=asia-southeast1 deploy/gcp/deploy.sh
#
# Re-runnable: resources that already exist are kept as they are (the Cloud
# SQL instance is never resized by this script), the image is rebuilt, and
# the job and the service are redeployed with it. The seed job runs with
# --if-empty, so it only loads data into an empty table. See README.md in
# this directory for the reasoning behind every setting and for teardown.
#
# Requires: gcloud (authenticated: `gcloud auth login`), openssl, and Docker
# only when BUILDER=docker.
set -euo pipefail

# --- Configuration (override any of these through the environment) ---------

: "${PROJECT_ID:?Set PROJECT_ID to the Google Cloud project to deploy into}"
REGION="${REGION:-asia-southeast1}"

SERVICE="${SERVICE:-arrow-playground}"
SEED_JOB="${SEED_JOB:-${SERVICE}-seed}"
AR_REPO="${AR_REPO:-arrow-playground}"
RUNTIME_SA_NAME="${RUNTIME_SA_NAME:-${SERVICE}}"

SQL_INSTANCE="${SQL_INSTANCE:-arrow-playground-pg}"
SQL_CPU="${SQL_CPU:-4}"
SQL_MEMORY="${SQL_MEMORY:-16GiB}"
SQL_STORAGE_GB="${SQL_STORAGE_GB:-100}"
DB_NAME="${DB_NAME:-playground}"
DB_USER="${DB_USER:-playground}"
DB_SECRET="${DB_SECRET:-${SERVICE}-database-url}"

SEED_ROWS="${SEED_ROWS:-20000000}"
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
BUILDER="${BUILDER:-cloud-build}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE_TAG="${IMAGE_TAG:-$(git -C "$REPO_ROOT" describe --always --dirty 2>/dev/null || date -u +%Y%m%d-%H%M%S)}"
IMAGE="${IMAGE:-${REGION}-docker.pkg.dev/${PROJECT_ID}/${AR_REPO}/arrow-playground:${IMAGE_TAG}}"
RUNTIME_SA="${RUNTIME_SA_NAME}@${PROJECT_ID}.iam.gserviceaccount.com"

# Every gcloud call below targets this project without touching the user's
# active gcloud configuration.
export CLOUDSDK_CORE_PROJECT="$PROJECT_ID"

# --- Helpers ----------------------------------------------------------------

log() { printf '\n==> %s\n' "$*" >&2; }

# True when a `describe` command finds the resource.
exists() { "$@" >/dev/null 2>&1; }

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

# --- 2. Artifact Registry ---------------------------------------------------

if ! exists gcloud artifacts repositories describe "$AR_REPO" --location="$REGION"; then
  log "Creating Artifact Registry repository $AR_REPO in $REGION"
  gcloud artifacts repositories create "$AR_REPO" \
    --repository-format=docker \
    --location="$REGION" \
    --description="Rust Arrow Playground images"
fi

# --- 3. Cloud SQL for PostgreSQL 18 -----------------------------------------

if ! exists gcloud sql instances describe "$SQL_INSTANCE"; then
  log "Creating Cloud SQL instance $SQL_INSTANCE (takes 5-15 minutes)"
  # Enterprise edition with a custom machine: 16 GiB keeps a 20M-row table
  # (about 5 GB) in memory. Zonal, no backups: the data can be regenerated.
  # Public IP without authorized networks only accepts connections through
  # the Cloud SQL connector, which is what Cloud Run uses.
  gcloud sql instances create "$SQL_INSTANCE" \
    --database-version=POSTGRES_18 \
    --edition=enterprise \
    --cpu="$SQL_CPU" \
    --memory="$SQL_MEMORY" \
    --region="$REGION" \
    --availability-type=zonal \
    --storage-type=SSD \
    --storage-size="$SQL_STORAGE_GB" \
    --no-backup
fi
CONNECTION_NAME="$(gcloud sql instances describe "$SQL_INSTANCE" --format='value(connectionName)')"

if ! exists gcloud sql databases describe "$DB_NAME" --instance="$SQL_INSTANCE"; then
  log "Creating database $DB_NAME"
  gcloud sql databases create "$DB_NAME" --instance="$SQL_INSTANCE"
fi

# --- 4. Database user and DATABASE_URL secret -------------------------------

# tokio-postgres does not read PGPASSWORD, so the password lives inside the
# connection string, and the whole string is the secret. Created once; delete
# the secret to rotate the password on the next run.
if ! exists gcloud secrets describe "$DB_SECRET"; then
  log "Creating database user $DB_USER and secret $DB_SECRET"
  password="$(openssl rand -hex 24)"
  if [[ -n "$(gcloud sql users list --instance="$SQL_INSTANCE" --filter="name=$DB_USER" --format='value(name)')" ]]; then
    gcloud sql users set-password "$DB_USER" --instance="$SQL_INSTANCE" --password="$password"
  else
    gcloud sql users create "$DB_USER" --instance="$SQL_INSTANCE" --password="$password"
  fi
  printf 'host=/cloudsql/%s user=%s password=%s dbname=%s' \
    "$CONNECTION_NAME" "$DB_USER" "$password" "$DB_NAME" |
    gcloud secrets create "$DB_SECRET" --replication-policy=automatic --data-file=-
  unset password
fi

# --- 5. Runtime service account ---------------------------------------------

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

# --- 6. Image ---------------------------------------------------------------

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
    gcloud auth configure-docker "${REGION}-docker.pkg.dev" --quiet
    docker build --platform=linux/amd64 --tag="$IMAGE" "$REPO_ROOT"
    docker push "$IMAGE"
    ;;
  *)
    echo "BUILDER must be cloud-build or docker, not '$BUILDER'" >&2
    exit 1
    ;;
esac

# --- 7. Seed job ------------------------------------------------------------

log "Deploying and running seed job $SEED_JOB ($SEED_ROWS rows, skipped if the table has data)"
# The seeder is CPU-bound on data generation and on the database side of
# COPY; more workers than Cloud SQL has vCPUs does not help.
gcloud run jobs deploy "$SEED_JOB" \
  --image="$IMAGE" \
  --region="$REGION" \
  --service-account="$RUNTIME_SA" \
  --set-cloudsql-instances="$CONNECTION_NAME" \
  --set-secrets="DATABASE_URL=${DB_SECRET}:latest" \
  --args="seed,--if-empty,--rows,${SEED_ROWS},--workers,${SEED_WORKERS}" \
  --cpu=4 \
  --memory=2Gi \
  --task-timeout=2h \
  --max-retries=0 \
  --wait

# --- 8. Service -------------------------------------------------------------

if [[ "$USE_HTTP2" == true ]]; then http2_flag=--use-http2; else http2_flag=--no-use-http2; fi

log "Deploying service $SERVICE"
# Sizing for 1M-row responses (see README.md): one such request peaks at
# about 1.5 GB of memory, so 4 GiB holds two at a time; a third goes to a
# second instance. One instance stays warm so cold starts do not land in the
# measurements, and slow links get 15 minutes to download.
gcloud run deploy "$SERVICE" \
  --image="$IMAGE" \
  --region="$REGION" \
  --service-account="$RUNTIME_SA" \
  --add-cloudsql-instances="$CONNECTION_NAME" \
  --set-secrets="DATABASE_URL=${DB_SECRET}:latest" \
  --set-env-vars="REGION=${REGION},MAX_ROWS=${MAX_ROWS},DB_POOL_SIZE=${DB_POOL_SIZE}" \
  --port=8080 \
  --cpu="$RUN_CPU" \
  --memory="$RUN_MEMORY" \
  --concurrency="$RUN_CONCURRENCY" \
  --min-instances="$RUN_MIN_INSTANCES" \
  --max-instances="$RUN_MAX_INSTANCES" \
  --timeout=900 \
  --execution-environment=gen2 \
  --cpu-boost \
  "$http2_flag" \
  --allow-unauthenticated

URL="$(gcloud run services describe "$SERVICE" --region="$REGION" --format='value(status.url)')"
log "Done: $URL"
cat >&2 <<EOF
    Open $URL in a browser, or point a local UI at it (API base URL setting).
    Quick check:  curl -sS $URL/api/info
    Pause costs:  gcloud run services update $SERVICE --region=$REGION --min-instances=0
                  gcloud sql instances patch $SQL_INSTANCE --activation-policy=NEVER
    Tear down:    PROJECT_ID=$PROJECT_ID REGION=$REGION $(dirname "${BASH_SOURCE[0]}")/teardown.sh
EOF
