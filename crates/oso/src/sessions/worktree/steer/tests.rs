//! Tests of `steer`: which tool calls the hook denies, its answer and its exit status.
//!
//! Must not depend on: Claude Code or a spawn.

use serde_json::{Value, json};

use super::{HOOK, MAX_INPUT, REASON, decide, pattern, reply, run};

fn bash(command: &str) -> Vec<u8> {
    json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": command}})
        .to_string()
        .into_bytes()
}

#[test]
fn the_pattern_compiles() {
    assert!(pattern().is_some());
}

#[test]
fn enter_worktree_is_denied() {
    let input = json!({"tool_name": "EnterWorktree", "tool_input": {}}).to_string();
    assert_eq!(decide(input.as_bytes()), Some(REASON));
}

#[test]
fn git_worktree_add_is_denied() {
    for command in [
        "git worktree add ../x",
        "cd a && git -C b worktree  add x",
        "git --no-pager worktree add -b y p",
        "git -c core.hooksPath=/dev/null worktree add p",
        "git -C \"dir with space\" --git-dir=.git worktree add p",
        "git --work-tree /w worktree add p",
        "/usr/bin/git worktree add p",
        "echo hi; git worktree add p",
        "ls\ngit worktree add p",
        "x=$(git worktree add p)",
        "(git worktree add p)",
        "git worktree \\\nadd p",
        "git worktree add",
        "bash -c 'git worktree add p'",
    ] {
        assert_eq!(decide(&bash(command)), Some(REASON), "{command:?}");
    }
    // The hook steers and is no boundary: text that only names the command is denied too.
    assert_eq!(decide(&bash("echo \"git worktree add\"")), Some(REASON));
}

#[test]
fn other_calls_are_allowed() {
    for command in [
        "git worktree list",
        "git worktrees add x",
        "git worktree addx",
        "git add worktree",
        "echo hi",
        "mygit worktree add p",
        "git worktree\nadd p",
        "git status; worktree add",
        "git log --grep worktree",
    ] {
        assert_eq!(decide(&bash(command)), None, "{command:?}");
    }
    let other = json!({"tool_name": "Read", "tool_input": {"command": "git worktree add p"}});
    assert_eq!(decide(other.to_string().as_bytes()), None);
    assert_eq!(decide(b"{not json"), None);
    assert_eq!(decide(b""), None);
    let number = json!({"tool_name": "Bash", "tool_input": {"command": 7}});
    assert_eq!(decide(number.to_string().as_bytes()), None);
    assert_eq!(decide(br#"{"tool_name": 3}"#), None);
    // Over the limit: allowed unread.
    let long = format!("git worktree add p #{}", "x".repeat(70 * 1024));
    assert!(bash(&long).len() > MAX_INPUT);
    assert_eq!(decide(&bash(&long)), None);
}

#[test]
fn the_reply_denies_with_the_reason() {
    let value: Value = serde_json::from_str(&reply(REASON)).unwrap();
    assert_eq!(
        value,
        json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": REASON,
        }})
    );
}

#[test]
fn the_hook_runs_over_its_input() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let input = bash("git worktree add p");
    assert_eq!(run(Some(HOOK[1]), input.as_slice(), &mut out, &mut err), 0);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        format!("{}\n", reply(REASON))
    );
    assert!(err.is_empty());

    let mut out = Vec::new();
    let input = bash("git status");
    assert_eq!(run(Some(HOOK[1]), input.as_slice(), &mut out, &mut err), 0);
    assert!(out.is_empty());

    // Input over the limit is allowed, however it ends.
    let mut out = Vec::new();
    let long = bash(&format!("git worktree add p #{}", "x".repeat(MAX_INPUT)));
    assert_eq!(run(Some(HOOK[1]), long.as_slice(), &mut out, &mut err), 0);
    assert!(out.is_empty());
}

#[test]
fn a_read_error_allows() {
    struct Broken;
    impl std::io::Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("broken"))
        }
    }
    let mut out = Vec::new();
    assert_eq!(run(Some(HOOK[1]), Broken, &mut out, Vec::new()), 0);
    assert!(out.is_empty());
}

#[test]
fn another_hook_is_a_usage_error() {
    for name in [None, Some("other"), Some("")] {
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(run(name, &b"{}"[..], &mut out, &mut err), 2);
        assert!(out.is_empty());
        let err = String::from_utf8(err).unwrap();
        assert_eq!(err, "usage: oso hook deny-worktree\n");
    }
}
