#!/usr/bin/env bash
# One-time setup that lets the "Google Cloud" GitHub Actions workflow
# (.github/workflows/google-cloud.yml) deploy into a Google Cloud project
# without any stored key, through Workload Identity Federation. Run it once,
# for example in Cloud Shell:
#
#   curl -fsSL https://raw.githubusercontent.com/ZipZipPy/rust-arrow-playground/main/deploy/gcp/github-setup.sh | PROJECT_ID=my-project bash
#
# What it creates in PROJECT_ID:
#   - workload identity pool "github" with an OIDC provider "github" that
#     trusts GitHub's token issuer, but only for workflow runs of this
#     repository (matched by its numeric id, which a renamed or re-created
#     repository does not keep) on its main branch;
#   - service account github-deployer@PROJECT_ID.iam.gserviceaccount.com,
#     which those runs may impersonate, with the roles deploy.sh and
#     teardown.sh need (Editor, Project IAM Admin, Secret Manager Admin,
#     Service Account Admin). Use a project dedicated to the playground.
#
# Re-runnable: existing resources are kept, the provider's condition is reset
# to the one below, and role bindings are only added. Undo with
#   gcloud iam workload-identity-pools delete github --location=global
#   gcloud iam service-accounts delete github-deployer@PROJECT_ID.iam.gserviceaccount.com
set -euo pipefail

: "${PROJECT_ID:?Set PROJECT_ID to the Google Cloud project GitHub Actions may deploy into}"
REPO="${REPO:-ZipZipPy/rust-arrow-playground}"
REPO_ID="${REPO_ID:-1429926647}"
BRANCH="${BRANCH:-main}"
POOL="${POOL:-github}"
PROVIDER="${PROVIDER:-github}"
SA_NAME="${SA_NAME:-github-deployer}"
SA="${SA_NAME}@${PROJECT_ID}.iam.gserviceaccount.com"

export CLOUDSDK_CORE_PROJECT="$PROJECT_ID"
# Piped into bash, stdin is this script: a prompt would read the rest of it.
export CLOUDSDK_CORE_DISABLE_PROMPTS=1

log() { printf '\n==> %s\n' "$*" >&2; }
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

log "Enabling the APIs Workload Identity Federation and deploy.sh start from"
gcloud services enable \
  iam.googleapis.com \
  iamcredentials.googleapis.com \
  sts.googleapis.com \
  cloudresourcemanager.googleapis.com \
  serviceusage.googleapis.com

PROJECT_NUMBER="$(gcloud projects describe "$PROJECT_ID" --format='value(projectNumber)')"

if ! exists gcloud iam workload-identity-pools describe "$POOL" --location=global; then
  log "Creating workload identity pool $POOL"
  gcloud iam workload-identity-pools create "$POOL" \
    --location=global \
    --display-name="GitHub Actions"
fi

# GitHub's token carries the repository id and the ref of the run. Pinning
# both keeps forks, other repositories and pull-request branches out.
condition="assertion.repository_id == '${REPO_ID}' && assertion.ref == 'refs/heads/${BRANCH}'"
mapping="google.subject=assertion.sub,attribute.repository=assertion.repository,attribute.repository_id=assertion.repository_id,attribute.ref=assertion.ref"
if exists gcloud iam workload-identity-pools providers describe "$PROVIDER" \
  --location=global --workload-identity-pool="$POOL"; then
  log "Updating provider $PROVIDER"
  gcloud iam workload-identity-pools providers update-oidc "$PROVIDER" \
    --location=global \
    --workload-identity-pool="$POOL" \
    --issuer-uri="https://token.actions.githubusercontent.com" \
    --attribute-mapping="$mapping" \
    --attribute-condition="$condition" >/dev/null
else
  log "Creating provider $PROVIDER for $REPO ($BRANCH only)"
  gcloud iam workload-identity-pools providers create-oidc "$PROVIDER" \
    --location=global \
    --workload-identity-pool="$POOL" \
    --display-name="GitHub ${REPO#*/}" \
    --issuer-uri="https://token.actions.githubusercontent.com" \
    --attribute-mapping="$mapping" \
    --attribute-condition="$condition"
fi

if ! exists gcloud iam service-accounts describe "$SA"; then
  log "Creating service account $SA"
  gcloud iam service-accounts create "$SA_NAME" \
    --display-name="GitHub Actions deployer for $REPO"
fi

# Editor covers Cloud Run, Cloud SQL, Artifact Registry, Cloud Build and
# enabling APIs; deploy.sh also creates a service account and grants roles on
# the project and on a secret, which Editor alone may not do.
for role in \
  roles/editor \
  roles/resourcemanager.projectIamAdmin \
  roles/secretmanager.admin \
  roles/iam.serviceAccountAdmin; do
  log "Granting $role to $SA"
  retry gcloud projects add-iam-policy-binding "$PROJECT_ID" \
    --member="serviceAccount:$SA" \
    --role="$role" \
    --condition=None >/dev/null
done

log "Letting workflow runs from $REPO impersonate $SA"
retry gcloud iam service-accounts add-iam-policy-binding "$SA" \
  --role=roles/iam.workloadIdentityUser \
  --member="principalSet://iam.googleapis.com/projects/${PROJECT_NUMBER}/locations/global/workloadIdentityPools/${POOL}/attribute.repository_id/${REPO_ID}" \
  --condition=None >/dev/null

cat >&2 <<EOF

Done. Send this line to Claude (or put both values in deploy/gcp/github.env):

    GCP_PROJECT_ID=${PROJECT_ID} GCP_PROJECT_NUMBER=${PROJECT_NUMBER}

EOF
