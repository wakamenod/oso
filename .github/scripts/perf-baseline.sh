#!/bin/sh
# Downloads the newest performance result named $1 from the runs of perf.yml
# on main into directory $2 (ADR 0044). Only pushes to main in this repository
# count, so no pull request can plant a baseline: a fork can have a branch
# called main too, but its runs are not push events here. Finding no result
# is not an error: the comparison then passes with a note.
set -eu
name=$1
dest=$2
runs=$(gh api "repos/$GITHUB_REPOSITORY/actions/workflows/perf.yml/runs?branch=main&event=push&per_page=20" \
    --jq '.workflow_runs[] | select(.head_repository.full_name == env.GITHUB_REPOSITORY) | .id')
for run in $runs; do
    # A run that is still going, or that failed early, has no such artifact.
    if gh run download "$run" --repo "$GITHUB_REPOSITORY" --name "$name" --dir "$dest" 2>/dev/null; then
        echo "baseline: run $run"
        exit 0
    fi
done
echo "no baseline named $name"
