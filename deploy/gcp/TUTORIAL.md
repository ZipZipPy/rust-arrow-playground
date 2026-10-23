# Compare JSON, Arrow and Parquet across regions

<walkthrough-tutorial-duration duration="60"></walkthrough-tutorial-duration>

## Before you start

This tutorial deploys Rust Arrow Playground to Cloud Run in two to four
regions, one in your country and the others abroad, all reading one Cloud
SQL for PostgreSQL database. At the end you open one page that times JSON,
Arrow IPC and Parquet against every server side by side, from your own
browser and network.

It creates resources that bill by the hour: with three regions, about
USD 0.45 per hour while they run (one Cloud SQL instance and one warm Cloud
Run instance per region) at Iowa list prices; with the database in
Singapore it costs somewhat more (see *Cost notes* in
`deploy/gcp/README.md`). The last steps pause them between sessions or
delete them.

`deploy/gcp/README.md` explains every setting the script uses.

Click **Start** to begin.

## Pick a project with billing

Choose or create a project for this. A project of its own is easiest to
clean up afterwards. It needs billing enabled:

<walkthrough-project-setup billing="true"></walkthrough-project-setup>

Point the terminal at it:

```sh
export PROJECT_ID="<walkthrough-project-id/>"
gcloud config set project "$PROJECT_ID"
```

Check that `gcloud` is signed in (if Cloud Shell asks to authorize
`gcloud`, click **Authorize**):

```sh
gcloud auth list
```

If no account is marked active, Cloud Shell opened this repository in a
temporary environment without your credentials. Sign in, and follow the
link it prints:

```sh
gcloud auth login
```

## Choose the regions

Pick one region in your country and one or two abroad, nearest first. The
first region's service also serves the comparison page.

For Thailand, for example:

- `asia-southeast3`: Bangkok, same country
- `asia-southeast1`: Singapore, a neighbouring country
- `us-central1`: Iowa, or `europe-west1`: Belgium, another continent

```sh
export REGIONS="asia-southeast3 asia-southeast1 us-central1"
```

Elsewhere, use the same pattern with the regions from
[Cloud Run locations](https://cloud.google.com/run/docs/locations).

Cloud SQL in Bangkok cannot create the database's default machine, so with
Bangkok first the script puts the database in Singapore and says so. The
comparison reads rows from each server's cache, so where the database sits
does not show in its times.

## Deploy

Run the script from the repository root:

```sh
deploy/gcp/deploy.sh
```

It enables the APIs, starts creating the Cloud SQL instance, builds the
image with Cloud Build meanwhile, loads 5 million orders into the
database, and deploys one Cloud Run service per region. The first run
takes roughly 30-45 minutes.

Keep this tab open, and click into the terminal now and then: Cloud Shell
ends a session after about 40 minutes without interaction.

If the session ends anyway, the slow parts keep running in Google Cloud.
Open Cloud Shell again with the same *Open in Cloud Shell* button, set
`PROJECT_ID` and `REGIONS` as before, and run the script again. It skips what already
exists and waits for what is still running. If the image build had
finished, add `BUILDER=none` to skip building it again:

```sh
BUILDER=none deploy/gcp/deploy.sh
```

<walkthrough-footnote>The script is safe to run again: it keeps what exists, and the seed job keeps a complete seed of the current schema version. Only when it finds none (a seed that did not finish, or data from an older version of the playground) does it drop and reload the data, which takes minutes while the servers have no data.</walkthrough-footnote>

## Open the comparison

The script ends with one URL per region and a compare link:

```terminal
    Compare the servers from one page:
      https://<first>/?servers=https://<first>,https://<second>,https://<third>
```

Open the compare link in your browser. The page adds every server to its
server list; run **Compare servers** there. Each server reports its region.
To see what the distance between your browser and each server adds, look
at the RTT column and at the *Network wait* and *Download* parts of each
bar, and at how much the smaller Arrow and Parquet bodies help. Compare
servers reads every row from each server's row cache, so the distance
between a server and the database does not show in these times.

Try it from other networks too: a phone hotspot, office Wi-Fi, a friend
abroad. The history table can be copied as CSV or Markdown.

To check every server from the terminal:

```sh
for region in $REGIONS; do
  url=$(gcloud run services describe arrow-playground --region="$region" --format='value(status.url)')
  curl -sS "$url/api/info" | grep -o '"server":{[^}]*}'
done
```

To add a region later, run the script again with the whole list, keeping
the same region first (it decides where the database is). The services
that exist are redeployed, and the new region gets its own. `BUILDER=none`
reuses the image already built; if the repository has newer commits, the
script stops before creating anything, and you run it again without it:

```sh
export REGIONS="asia-southeast3 asia-southeast1 us-central1 europe-west1"
BUILDER=none deploy/gcp/deploy.sh
```

## Pause between sessions

Stop what bills by the hour and keep the data. This sets every service's
warm instances to zero and stops the Cloud SQL instance. Its storage and
its idle IP address are still billed, about USD 16 per month:

```sh
deploy/gcp/pause.sh off
```

Before the next session, start everything again (a minute or two):

```sh
deploy/gcp/pause.sh on
```

In a new Cloud Shell session, set `PROJECT_ID` and `REGIONS` again first.

## Delete everything

When you are done, delete every service, the database with its data, the
image repository, the secret and the service account. The script lists
what it will delete and asks you to type the project id:

```sh
deploy/gcp/teardown.sh
```

If you created the project only for this, deleting the project removes
everything at once:

```sh
gcloud projects delete "$PROJECT_ID"
```

## Done

<walkthrough-conclusion-trophy></walkthrough-conclusion-trophy>

You compared JSON, Arrow IPC and Parquet from your browser against servers
in your own country and abroad. `deploy/gcp/README.md` covers sizing, costs,
other regions and measuring without a browser.
