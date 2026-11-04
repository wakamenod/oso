#!/bin/sh
# PreToolUse hook (matcher: Edit|Write|MultiEdit), ADR 0048. Asks the human
# before an edit to a file that needs approval, except in auto mode, where
# edits need no approval (ADR 0043). Settings ask rules cannot depend on the
# mode, and they prompt in auto mode too. If the input cannot be read, it asks.
set -u

ask() {
    printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"%s"}}\n' "$1"
    exit 0
}

command -v jq >/dev/null 2>&1 || ask "jq is missing, so this edit cannot be checked"
input=$(cat) || ask "cannot read hook input"
mode=$(printf '%s' "$input" | jq -r '.permission_mode // empty') ||
    ask "cannot parse hook input"
[ "$mode" = auto ] && exit 0

path=$(printf '%s' "$input" | jq -r '.tool_input.file_path // empty') ||
    ask "cannot parse hook input"
[ -n "$path" ] || ask "cannot find the file to edit"

# `cwd` follows Claude into a worktree, while $CLAUDE_PROJECT_DIR stays where
# the session started.
cwd=$(printf '%s' "$input" | jq -r '.cwd // empty')
root=$(git -C "${cwd:-.}" rev-parse --show-toplevel 2>/dev/null) || root=
relative=${path#"$root"/}

. "$(dirname "$0")/gated-paths.sh" || ask "cannot read the list of gated files"
printf '%s\n' "$relative" | grep -Eq "$gated_paths"
case $? in
    0) ask "This file needs approval (AGENTS.md)" ;;
    1) exit 0 ;;
    *) ask "cannot check the list of gated files" ;;
esac
