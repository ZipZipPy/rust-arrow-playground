#!/usr/bin/env bash
# Deletes everything deploy.sh created, so nothing keeps billing.
#
#   PROJECT_ID=my-project REGION=asia-southeast1 deploy/gcp/teardown.sh
#
# Uses the same variables and defaults as deploy.sh. Asks for confirmation
# unless YES=1. Resources that do not exist are skipped. Enabled APIs are
# left on (they cost nothing).
set -euo pipefail

: "${PROJECT_ID:?Set PROJECT_ID to the Google Cloud project to clean up}"
REGION="${REGION:-asia-southeast1}"
SERVICE="${SERVICE:-arrow-playground}"
SEED_JOB="${SEED_JOB:-${SERVICE}-seed}"
AR_REPO="${AR_REPO:-arrow-playground}"
RUNTIME_SA_NAME="${RUNTIME_SA_NAME:-${SERVICE}}"
SQL_INSTANCE="${SQL_INSTANCE:-arrow-playground-pg}"
DB_SECRET="${DB_SECRET:-${SERVICE}-database-url}"
RUNTIME_SA="${RUNTIME_SA_NAME}@${PROJECT_ID}.iam.gserviceaccount.com"

export CLOUDSDK_CORE_PROJECT="$PROJECT_ID"
# Deletions below run without per-command prompts; the one prompt is here.
export CLOUDSDK_CORE_DISABLE_PROMPTS=1

cat >&2 <<EOF
This permanently deletes, in project $PROJECT_ID:
  Cloud Run service   $SERVICE ($REGION)
  Cloud Run job       $SEED_JOB ($REGION)
  Cloud SQL instance  $SQL_INSTANCE, with all its data
  Secret              $DB_SECRET
  Artifact Registry   $AR_REPO ($REGION), with all images
  Service account     $RUNTIME_SA
EOF
if [[ "${YES:-}" != 1 ]]; then
  read -r -p "Type the project id to continue: " answer
  [[ "$answer" == "$PROJECT_ID" ]] || { echo "Aborted." >&2; exit 1; }
fi

log() { printf '\n==> %s\n' "$*" >&2; }
exists() { "$@" >/dev/null 2>&1; }

if exists gcloud run services describe "$SERVICE" --region="$REGION"; then
  log "Deleting Cloud Run service $SERVICE"
  gcloud run services delete "$SERVICE" --region="$REGION"
fi

if exists gcloud run jobs describe "$SEED_JOB" --region="$REGION"; then
  log "Deleting Cloud Run job $SEED_JOB"
  gcloud run jobs delete "$SEED_JOB" --region="$REGION"
fi

if exists gcloud sql instances describe "$SQL_INSTANCE"; then
  log "Deleting Cloud SQL instance $SQL_INSTANCE"
  gcloud sql instances delete "$SQL_INSTANCE"
fi

if exists gcloud secrets describe "$DB_SECRET"; then
  log "Deleting secret $DB_SECRET"
  gcloud secrets delete "$DB_SECRET"
fi

if exists gcloud artifacts repositories describe "$AR_REPO" --location="$REGION"; then
  log "Deleting Artifact Registry repository $AR_REPO"
  gcloud artifacts repositories delete "$AR_REPO" --location="$REGION"
fi

if exists gcloud iam service-accounts describe "$RUNTIME_SA"; then
  log "Removing project role and deleting service account $RUNTIME_SA"
  gcloud projects remove-iam-policy-binding "$PROJECT_ID" \
    --member="serviceAccount:$RUNTIME_SA" \
    --role=roles/cloudsql.client \
    --condition=None >/dev/null || true
  gcloud iam service-accounts delete "$RUNTIME_SA"
fi

log "Done. Cloud Build source archives remain in gs://${PROJECT_ID}_cloudbuild (delete with: gcloud storage rm -r gs://${PROJECT_ID}_cloudbuild)"
