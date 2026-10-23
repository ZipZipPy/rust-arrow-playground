# Settings and helpers shared by deploy.sh, teardown.sh and pause.sh, which
# source this file. Every setting can be overridden through the environment;
# README.md in this directory explains each one.
# shellcheck shell=bash
# shellcheck disable=SC2034 # the scripts that source this file use the rest

: "${PROJECT_ID:?Set PROJECT_ID to the Google Cloud project}"

# Cloud Run regions, separated by spaces or commas. Every region gets its own
# service with the same name (services are regional, so the names do not
# clash). REGION is the single-region form and the default.
REGION="${REGION:-asia-southeast1}"
REGIONS="${REGIONS:-$REGION}"
REGION_LIST=()
read -r -a regions_given <<<"${REGIONS//,/ }"
for region in "${regions_given[@]}"; do
  if [[ ! "$region" =~ ^[a-z]+-[a-z]+[0-9]+$ ]]; then
    echo "REGIONS: '$region' is not a region name such as asia-southeast1" >&2
    exit 1
  fi
  [[ " ${REGION_LIST[*]} " == *" $region "* ]] || REGION_LIST+=("$region")
done
unset regions_given region
if ((${#REGION_LIST[@]} == 0)); then
  echo "REGIONS is empty; name at least one region such as asia-southeast1" >&2
  exit 1
fi

# Where the one Cloud SQL instance, the image repository and the seed job
# live: the first of REGIONS, unless Cloud SQL there cannot create the
# default machine (a 4 vCPU / 16 GiB custom machine), in which case the
# nearest region that can. Region availability of machine types:
# https://cloud.google.com/sql/docs/postgres/region-availability-overview
sql_region_for() {
  case "$1" in
    # Bangkok: Enterprise edition only offers shared-core, 1-vCPU
    # dedicated-core and N4 machines.
    asia-southeast3) echo asia-southeast1 ;;
    *) echo "$1" ;;
  esac
}
SQL_REGION_NOTE=""
if [[ -z "${SQL_REGION:-}" ]]; then
  SQL_REGION="$(sql_region_for "${REGION_LIST[0]}")"
  if [[ "$SQL_REGION" != "${REGION_LIST[0]}" ]]; then
    SQL_REGION_NOTE="Cloud SQL in ${REGION_LIST[0]} cannot create the default custom machine, so the database, the image and the seed job go to $SQL_REGION (set SQL_REGION to choose another region)."
  fi
fi

SERVICE="${SERVICE:-arrow-playground}"
SEED_JOB="${SEED_JOB:-${SERVICE}-seed}"
AR_REPO="${AR_REPO:-arrow-playground}"
RUNTIME_SA_NAME="${RUNTIME_SA_NAME:-${SERVICE}}"
RUNTIME_SA="${RUNTIME_SA_NAME}@${PROJECT_ID}.iam.gserviceaccount.com"
SQL_INSTANCE="${SQL_INSTANCE:-arrow-playground-pg}"
DB_SECRET="${DB_SECRET:-${SERVICE}-database-url}"

# Every gcloud call targets this project without touching the user's active
# gcloud configuration.
export CLOUDSDK_CORE_PROJECT="$PROJECT_ID"

log() { printf '\n==> %s\n' "$*" >&2; }

# True when a `describe` command finds the resource.
exists() { "$@" >/dev/null 2>&1; }

# Sets SQL_EXISTS to 1 when the Cloud SQL instance exists, else to "". An
# existing instance never moves, so it has to be in SQL_REGION; otherwise
# the repository and the seed job would be looked for in the wrong region.
check_sql_instance() {
  local region
  SQL_EXISTS=""
  region="$(gcloud sql instances describe "$SQL_INSTANCE" --format='value(region)' 2>/dev/null)" || return 0
  SQL_EXISTS=1
  if [[ "$region" != "$SQL_REGION" ]]; then
    echo "Cloud SQL instance $SQL_INSTANCE is in $region, not in SQL_REGION=$SQL_REGION." >&2
    echo "Keep the earlier first region at the head of REGIONS, or set SQL_REGION=$region." >&2
    exit 1
  fi
}

# Waits for every unfinished operation on the Cloud SQL instance (a creation
# started with --async, a start or a stop): Cloud SQL rejects other changes
# to the instance until they are done.
wait_for_sql() {
  local pending
  pending="$(gcloud sql operations list --instance="$SQL_INSTANCE" --filter='status!=DONE' --format='value(name)')"
  if [[ -n "$pending" ]]; then
    log "Waiting for Cloud SQL instance $SQL_INSTANCE to finish its running operation"
    # shellcheck disable=SC2086 # one operation name per word
    gcloud sql operations wait $pending --timeout=unlimited >/dev/null
  fi
}

# Prints, one per line, every region of REGIONS and any other region where
# this project has a Cloud Run service named $SERVICE, so that pause.sh and
# teardown.sh also reach a region that was left out of REGIONS.
service_regions() {
  {
    printf '%s\n' "${REGION_LIST[@]}"
    gcloud run services list --format='value(metadata.name,region)' 2>/dev/null |
      awk -v service="$SERVICE" '$1 == service { print $2 }' || true
  } | awk '!seen[$0]++'
}
