#!/usr/bin/env bash
# Deletes everything deploy.sh created, so nothing keeps billing.
#
#   PROJECT_ID=my-project REGIONS="asia-southeast3 asia-southeast1 us-central1" deploy/gcp/teardown.sh
#
# Uses the same variables and defaults as deploy.sh (config.sh). Deletes the
# service in every region of REGIONS and in any other region where the
# project has a service with the same name. Asks for confirmation unless
# YES=1. Resources that do not exist are skipped. Enabled APIs are left on
# (they cost nothing).
set -euo pipefail

# shellcheck source=SCRIPTDIR/config.sh
source "$(dirname "${BASH_SOURCE[0]}")/config.sh"

# Deletions below run without per-command prompts; the one prompt is here.
export CLOUDSDK_CORE_DISABLE_PROMPTS=1

check_sql_instance
mapfile -t regions < <(service_regions)

cat >&2 <<EOF
This permanently deletes, in project $PROJECT_ID:
  Cloud Run service   $SERVICE (${regions[*]})
  Cloud Run job       $SEED_JOB ($SQL_REGION)
  Cloud SQL instance  $SQL_INSTANCE, with all its data
  Secret              $DB_SECRET
  Artifact Registry   $AR_REPO ($SQL_REGION), with all images
  Service account     $RUNTIME_SA
EOF
if [[ "${YES:-}" != 1 ]]; then
  read -r -p "Type the project id to continue: " answer
  [[ "$answer" == "$PROJECT_ID" ]] || { echo "Aborted." >&2; exit 1; }
fi

for region in "${regions[@]}"; do
  if exists gcloud run services describe "$SERVICE" --region="$region"; then
    log "Deleting Cloud Run service $SERVICE in $region"
    gcloud run services delete "$SERVICE" --region="$region"
  fi
done

if exists gcloud run jobs describe "$SEED_JOB" --region="$SQL_REGION"; then
  log "Deleting Cloud Run job $SEED_JOB"
  gcloud run jobs delete "$SEED_JOB" --region="$SQL_REGION"
fi

if [[ -n "$SQL_EXISTS" ]]; then
  # A deploy.sh that stopped early can leave the creation still running.
  wait_for_sql
  log "Deleting Cloud SQL instance $SQL_INSTANCE"
  gcloud sql instances delete "$SQL_INSTANCE"
fi

if exists gcloud secrets describe "$DB_SECRET"; then
  log "Deleting secret $DB_SECRET"
  gcloud secrets delete "$DB_SECRET"
fi

if exists gcloud artifacts repositories describe "$AR_REPO" --location="$SQL_REGION"; then
  log "Deleting Artifact Registry repository $AR_REPO"
  gcloud artifacts repositories delete "$AR_REPO" --location="$SQL_REGION"
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
