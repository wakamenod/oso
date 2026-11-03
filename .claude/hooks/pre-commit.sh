#!/bin/sh
# PreToolUse hook (matcher: Bash), ADR 0021. Before `git commit`:
# - block (exit 2) unless `just ci` and gitleaks pass;
# - ask the human if the commit touches files that need approval.
# Any unexpected failure also exits 2, so the hook fails closed.
set -u

log=
list=
# shellcheck disable=SC2329 # invoked by the EXIT trap
on_exit() {
    rc=$?
    rm -f ${log:+"$log"} ${list:+"$list"}
    [ "$rc" -eq 0 ] || exit 2
}
trap on_exit EXIT

block() {
    echo "pre-commit hook: $1; commit blocked." >&2
    exit 2
}

input=$(cat) || block "cannot read hook input"
if command -v jq >/dev/null 2>&1; then
    cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // empty') ||
        block "cannot parse hook input"
    cwd=$(printf '%s' "$input" | jq -r '.cwd // empty') ||
        block "cannot parse hook input"
else
    # Without jq, match against the raw JSON. This errs toward extra runs.
    cmd=$input
    cwd=$(printf '%s' "$input" |
        sed -n 's/.*"cwd"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
fi

# Match loosely, so global options (`git -c k=v commit`), quoting and line
# continuations are still caught. grep exits 1 on no match.
printf '%s' "$cmd" | grep -Eq 'git.*commit'
case $? in
    0) ;;
    1) exit 0 ;;
    *) block "grep failed" ;;
esac

# The checks below run in Claude's working directory, which `git -C` would
# bypass.
printf '%s' "$cmd" | grep -Eq 'git[[:space:]]+-C[^;&|]*commit' &&
    block "run git commit from the checkout itself, not with git -C"

# `cwd` follows Claude into a worktree, while $CLAUDE_PROJECT_DIR stays where
# the session started.
if [ -n "$cwd" ]; then
    root=$(git -C "$cwd" rev-parse --show-toplevel 2>/dev/null) ||
        block "$cwd is not in a git checkout"
else
    root=${CLAUDE_PROJECT_DIR:?}
fi
cd "$root" || block "cannot cd to $root"
log=$(mktemp) || block "mktemp failed"
list=$(mktemp) || block "mktemp failed"

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
# gitleaks:allow comments are ignored.
leaks() { gitleaks "$@" --redact --no-banner --ignore-gitleaks-allow; }
leaks git --pre-commit --staged >"$log" 2>&1 || fail "gitleaks (staged)"
leaks git --pre-commit >"$log" 2>&1 || fail "gitleaks (unstaged)"
git ls-files -z --others --exclude-standard >"$list" || fail "git ls-files"
xargs -r -0 -I{} gitleaks dir --redact --no-banner --ignore-gitleaks-allow \
    ./{} <"$list" >"$log" 2>&1 || fail "gitleaks (untracked)"

# Files that need human approval (AGENTS.md). Edit rules in settings.json do
# not see edits made through Bash, so ask again here.
protected='(^|/)(Cargo\.(toml|lock)|deny\.toml|\.?clippy\.toml|rust-toolchain(\.toml)?|[Jj]ustfile|\.gitleaks[^/]*)$|(^|/)\.cargo/|^\.github/|^\.claude/(settings[^/]*\.json|hooks/|skills/|agents/|commands/)'
{
    git diff --name-only HEAD 2>/dev/null || git diff --name-only --cached
    git ls-files --others --exclude-standard
} >"$log" || fail "git diff"
touched=$(grep -E "$protected" "$log")
case $? in
    0) ;;
    1) exit 0 ;;
    *) fail "grep" ;;
esac
reason="Commit touches files that need approval: $(printf '%s' "$touched" | tr '\n' ' ')"
if command -v jq >/dev/null 2>&1; then
    jq -n --arg r "$reason" '{hookSpecificOutput: {hookEventName: "PreToolUse",
        permissionDecision: "ask", permissionDecisionReason: $r}}'
else
    reason=$(printf '%s' "$reason" | sed 's/[\\"]/\\&/g')
    printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"%s"}}\n' "$reason"
fi
exit 0
