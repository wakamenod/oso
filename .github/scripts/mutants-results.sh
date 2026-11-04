#!/bin/sh
# Downloads the lists of the newest successful run of mutants.yml on main into
# directory $1, for the design health report (ADR 0050), and describes the run
# in $1/run.md. Only scheduled and dispatched runs on main in this repository
# count, so no pull request or fork can plant a result. Finding none is not an
# error, and the report then says so. A failing API call or download is an
# error, so an outage does not look like an empty list.
set -eu
dest=$1
runs=$(gh api "repos/$GITHUB_REPOSITORY/actions/workflows/mutants.yml/runs?branch=main&status=success&per_page=20" \
    --jq '.workflow_runs[] | select((.event == "schedule" or .event == "workflow_dispatch") and .head_branch == "main" and .head_repository.full_name == env.GITHUB_REPOSITORY) | .id')
for run in $runs; do
    found=$(gh api "repos/$GITHUB_REPOSITORY/actions/runs/$run/artifacts?per_page=100" \
        --jq '[.artifacts[] | select(.name == "mutants" and (.expired | not))] | length')
    if [ "$found" -gt 0 ]; then
        gh run download "$run" --repo "$GITHUB_REPOSITORY" --name mutants --dir "$dest"
        gh api "repos/$GITHUB_REPOSITORY/actions/runs/$run" \
            --jq '"From the [run](\(.html_url)) of \(.created_at) on commit \(.head_sha[0:7])."' >"$dest/run.md"
        echo "mutation results: run $run"
        exit 0
    fi
done
echo "no mutation results"
