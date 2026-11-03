#!/bin/sh
# PostToolUse hook (matcher: Edit|Write|MultiEdit). Runs `cargo fmt` after a
# Rust file in the current checkout is edited. Never fails the tool call
# (ADR 0021).
set -u

input=$(cat)
if command -v jq >/dev/null 2>&1; then
    file=$(printf '%s' "$input" | jq -r '.tool_input.file_path // empty')
    cwd=$(printf '%s' "$input" | jq -r '.cwd // empty')
else
    file=$(printf '%s' "$input" |
        sed -n 's/.*"file_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
    cwd=$(printf '%s' "$input" |
        sed -n 's/.*"cwd"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
fi

case $file in
    *.rs) ;;
    *) exit 0 ;;
esac

# Format only inside the checkout Claude is working in (see pre-commit.sh).
root=$(git -C "${cwd:-${CLAUDE_PROJECT_DIR:?}}" rev-parse --show-toplevel 2>/dev/null) ||
    root=${CLAUDE_PROJECT_DIR:?}
case $file in
    "$root"/*) ;;
    *) exit 0 ;;
esac

cd "$root" && cargo fmt --all >/dev/null 2>&1
exit 0
