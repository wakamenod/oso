#!/bin/sh
# Downloads the newest performance result named $1 from the runs of perf.yml
# on main into directory $2 (ADR 0046). Only pushes to main in this repository
# count, so no pull request can plant a baseline: a fork can have a branch
# called main too, but its runs are not push events here. Finding no result
# is not an error, and the comparison then passes with a note. A failing API
# call or download is an error, so an outage does not switch the check off.
set -eu
export NAME="$1"
dest=$2
runs=$(gh api "repos/$GITHUB_REPOSITORY/actions/workflows/perf.yml/runs?branch=main&event=push&per_page=20" \
    --jq '.workflow_runs[] | select(.event == "push" and .head_branch == "main" and .head_repository.full_name == env.GITHUB_REPOSITORY) | .id')
for run in $runs; do
    # A run that is still going, or that failed early, has no such artifact.
    found=$(gh api "repos/$GITHUB_REPOSITORY/actions/runs/$run/artifacts?per_page=100" \
        --jq '[.artifacts[] | select(.name == env.NAME and (.expired | not))] | length')
    if [ "$found" -gt 0 ]; then
        gh run download "$run" --repo "$GITHUB_REPOSITORY" --name "$NAME" --dir "$dest"
        echo "baseline: run $run"
        exit 0
    fi
done
echo "no baseline named $NAME"
