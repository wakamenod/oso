#!/bin/sh
# Structural changes need an ADR (ADR 0038, ADR 0039). Lists the structural
# changes from BASE_SHA to HEAD_SHA, and fails if there are any and PR_BODY
# cites no ADR as `ADR NNNN`. Run locally with, for example:
#
#   BASE_SHA=origin/main HEAD_SHA=HEAD PR_BODY='...' sh .github/scripts/structural-change.sh
#
# PR_BODY is untrusted text: it is only ever passed to grep as data.
set -eu
: "${BASE_SHA:?}" "${HEAD_SHA:?}"
range="$BASE_SHA...$HEAD_SHA"
found=$(mktemp)
trap 'rm -f "$found"' EXIT
note() { echo "- $1" >>"$found"; }

git diff --quiet "$range" -- guardrails/layers.txt ||
    note "the layer rules changed (guardrails/layers.txt)"
# A new public module or trait (ADR 0038). Other API changes show up in
# the snapshot diff for review but need no ADR.
public=$(git diff -U0 "$range" -- guardrails/public-api |
    grep -oE '^\+pub (unsafe )?(mod|trait) [^ ]+' | sed 's/^+//' || true)
[ -z "$public" ] ||
    note "a public module or trait was added: $(echo "$public" | tr '\n' ' ')"
# The checks themselves, and the size thresholds in clippy.toml.
git diff --quiet "$range" -- crates/oso/tests/architecture crates/oso/tests/workspace_lints.rs ||
    note "the architecture checks changed (crates/oso/tests)"
if git diff -U0 "$range" -- clippy.toml | grep -Eq '^[-+][[:space:]]*(too-many-lines|cognitive-complexity)-threshold'; then
    note "a size threshold in clippy.toml changed"
fi

# Removing an entry or lowering its maximum is free; adding or raising is not.
grown=$(git diff -U0 "$range" -- guardrails/size-allowlist.txt | awk '
    /^(---|\+\+\+)/ { next }
    /^[-+][^#[:space:]]/ {
        split(substr($0, 2), field, " ")
        if (substr($0, 1, 1) == "-") old[field[1]] = field[2]; else new[field[1]] = field[2]
    }
    END { for (path in new) if (!(path in old) || new[path] + 0 > old[path] + 0) print path }')
[ -z "$grown" ] ||
    note "the size allowlist grew: $(echo "$grown" | tr '\n' ' ')"

added=$(git diff --name-only --no-renames --diff-filter=AD "$range" -- 'crates/*/Cargo.toml')
[ -z "$added" ] ||
    note "a crate was added or removed: $(echo "$added" | tr '\n' ' ')"
# A line naming a workspace crate as a dependency: `oso-x = ...`,
# `oso-x.workspace = true`, a `[dependencies.oso-x]` table, a rename
# (`package = "oso-x"`), or the workspace members.
edge='^[-+]([[:space:]]*"?oso(-[a-z0-9]+)?"?[[:space:]]*[=.]|\[.*dependencies\."?oso|.*package[[:space:]]*=[[:space:]]*["'"'"']oso|[[:space:]]*members[[:space:]]*=)'
if git diff -U0 "$range" -- Cargo.toml 'crates/*/Cargo.toml' | grep -Eq "$edge"; then
    note "a dependency between workspace crates changed"
fi

if [ ! -s "$found" ]; then
    echo "No structural changes."
    exit 0
fi
echo "Structural changes (ADR 0038):"
cat "$found"
if printf '%s\n' "${PR_BODY:-}" | grep -Eq '(^|[^[:alnum:]])ADR[[:space:]]+[0-9]{4}([^0-9]|$)'; then
    echo "The pull request description cites an ADR."
    exit 0
fi
echo "::error::Structural changes need an ADR. Cite it in the pull request description as 'ADR NNNN'."
exit 1
