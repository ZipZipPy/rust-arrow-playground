#!/usr/bin/env bash
# Stops what bills by the hour between sessions, and starts it again. The
# data, the image and the services stay.
#
#   PROJECT_ID=my-project REGIONS="asia-southeast3 asia-southeast1 us-central1" deploy/gcp/pause.sh off
#   PROJECT_ID=my-project REGIONS="asia-southeast3 asia-southeast1 us-central1" deploy/gcp/pause.sh on
#
# off: every regional service keeps no warm instance (service-level minimum
#      0), then the Cloud SQL instance stops (activation policy NEVER). Its
#      storage is still billed. A request that arrives meanwhile starts an
#      instance that cannot reach the database.
# on:  the Cloud SQL instance starts (a minute or two), then every service
#      keeps RUN_MIN_INSTANCES warm again.
#
# Both act on every region of REGIONS and on any other region where the
# project has a service with the same name. Neither deploys a new revision.
# Uses the same variables and defaults as deploy.sh (config.sh).
set -euo pipefail

# shellcheck source=SCRIPTDIR/config.sh
source "$(dirname "${BASH_SOURCE[0]}")/config.sh"

RUN_MIN_INSTANCES="${RUN_MIN_INSTANCES:-1}"

# Sets the service-level minimum instances of $SERVICE in every region.
set_min_instances() {
  local region
  for region in $(service_regions); do
    if exists gcloud run services describe "$SERVICE" --region="$region"; then
      log "Setting minimum instances of $SERVICE in $region to $1"
      gcloud run services update "$SERVICE" --region="$region" --min="$1"
    fi
  done
}

case "${1:-}" in
  off)
    set_min_instances 0
    wait_for_sql
    log "Stopping Cloud SQL instance $SQL_INSTANCE (storage is still billed)"
    gcloud sql instances patch "$SQL_INSTANCE" --activation-policy=NEVER
    ;;
  on)
    wait_for_sql
    log "Starting Cloud SQL instance $SQL_INSTANCE"
    gcloud sql instances patch "$SQL_INSTANCE" --activation-policy=ALWAYS
    set_min_instances "$RUN_MIN_INSTANCES"
    ;;
  *)
    echo "Usage: $0 off|on   (off stops the costs, on starts again)" >&2
    exit 2
    ;;
esac
