#!/bin/sh
# PreToolUse hook (matcher: Bash). Blocks `git commit` unless `just ci` and a
# gitleaks scan of the pending changes pass. Exit 2 blocks the tool call and
# shows stderr to Claude (ADR 0021).
set -u

input=$(cat)
if command -v jq >/dev/null 2>&1; then
    cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // empty')
    cwd=$(printf '%s' "$input" | jq -r '.cwd // empty')
else
    # Without jq, match against the raw JSON. This can only cause extra runs.
    cmd=$input
    cwd=$(printf '%s' "$input" |
        sed -n 's/.*"cwd"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
fi

# Act only on `git commit` (optionally `git -C <dir> commit`). grep exits 1
# on no match; any other failure blocks rather than skipping the checks.
printf '%s' "$cmd" |
    grep -Eq '(^|[^[:alnum:]_.-])git[[:space:]]+(-C[[:space:]]+[^[:space:]]+[[:space:]]+)?commit([^[:alnum:]_-]|$)'
case $? in
    0) ;;
    1) exit 0 ;;
    *) echo "pre-commit hook: grep failed; commit blocked." >&2; exit 2 ;;
esac

# Check the checkout Claude is working in. `cwd` follows Claude into a
# worktree, while $CLAUDE_PROJECT_DIR stays where the session started.
root=$(git -C "${cwd:-${CLAUDE_PROJECT_DIR:?}}" rev-parse --show-toplevel 2>/dev/null) ||
    root=${CLAUDE_PROJECT_DIR:?}
cd "$root" || exit 2
log=$(mktemp) || exit 2
trap 'rm -f "$log"' EXIT

fail() {
    {
        echo "pre-commit hook: $1 failed in $root; commit blocked."
        tail -n 30 "$log"
    } >&2
    exit 2
}

just ci >"$log" 2>&1 || fail "just ci"

# Scan staged and unstaged changes, and untracked files: the hook runs before
# the command, so `git add ... && git commit` has not staged anything yet.
gitleaks git --pre-commit --staged --redact --no-banner >"$log" 2>&1 ||
    fail "gitleaks (staged)"
gitleaks git --pre-commit --redact --no-banner >"$log" 2>&1 ||
    fail "gitleaks (unstaged)"
git ls-files -z --others --exclude-standard |
    xargs -r -0 -n 1 gitleaks dir --redact --no-banner >"$log" 2>&1 ||
    fail "gitleaks (untracked)"

exit 0
