//! Steering Claude away from its own worktree tools (ADR 0121, Q15): `oso hook deny-worktree`,
//! a `PreToolUse` hook that denies `EnterWorktree` and a `Bash` call that runs
//! `git worktree add`. `sessions::settings` installs it for a session oso starts when
//! `[sessions] steer_worktrees = true`.
//!
//! Must not depend on: `app`, `dispatch`, the file system or sockets; the hook
//! reads its standard input, writes its answer and starts nothing.
//!
//! The hook steers; it is no boundary. A command can make a worktree in ways no pattern sees,
//! and a pattern also denies text that only names the command, as in `echo "git worktree add"`.

use std::io::{self, Read, Write};

use regex::{Regex, RegexBuilder};
use serde_json::{Value, json};

/// The arguments after oso's own path that run the hook.
pub const HOOK: [&str; 2] = ["hook", "deny-worktree"];

/// The tools the hook is asked about, as a hook's matcher has them: those [`decide`] reads.
pub const MATCHER: &str = "EnterWorktree|Bash";

/// The most bytes of hook input looked at; more is allowed unread.
pub const MAX_INPUT: usize = 64 * 1024;

/// Why the hook denies: it names the session tool an agent makes worktrees with (ADR 0121,
/// step 6).
pub const REASON: &str = "oso makes worktrees for its sessions. Use the oso tool start_worktree_session: it makes the worktree, a space and a session with your brief, once the person allows it. Do not make a worktree another way.";

/// The most memory the command pattern compiles to.
const PATTERN_LIMIT: usize = 1 << 20;

/// `git`, with its global options, running `worktree add`, in one simple command. It starts
/// where a command can: at the start, after a separator, a substitution's opening, a quote or
/// whitespace. Words are split by spaces, tabs and escaped line breaks only, so the parts cannot
/// come from two commands on two lines. Its classes are ASCII where a class may be (`(?-u)`);
/// a negated one stays Unicode, as the pattern matches text.
const PATTERN: &str = concat!(
    r#"(?:^|(?-u:[\s;&|(`"']))"#,
    r"(?:(?-u:[\w./~-])*/)?git",
    r"(?:(?:[ \t]|\\\n)+",
    r"(?:-[Cc]|--(?:git-dir|work-tree|namespace|config-env|super-prefix|exec-path|attr-source))",
    r#"(?:[ \t]|\\\n)+(?:"[^"\n]*"|'[^'\n]*'|[^\t\n\x0B\x0C\r ;&|]+)"#,
    r#"|(?:[ \t]|\\\n)+-(?-u:[\w-])+(?:=(?:"[^"\n]*"|'[^'\n]*'|[^\t\n\x0B\x0C\r ;&|]+))?"#,
    r")*",
    r"(?:[ \t]|\\\n)+worktree(?:[ \t]|\\\n)+add",
    r#"(?:$|(?-u:[\s;&|)`"']))"#,
);

/// Whether a `PreToolUse` input, JSON, is a call to deny: `tool_name` `EnterWorktree`, or `Bash`
/// with a `tool_input.command` string that runs `git … worktree add` in one simple command.
/// Anything else, JSON that does not parse, and input over [`MAX_INPUT`] are allowed (`None`).
pub fn decide(input: &[u8]) -> Option<&'static str> {
    if input.len() > MAX_INPUT {
        return None;
    }
    let value: Value = serde_json::from_slice(input).ok()?;
    match value.get("tool_name")?.as_str()? {
        "EnterWorktree" => Some(REASON),
        "Bash" => {
            let command = value.pointer("/tool_input/command")?.as_str()?;
            // Most commands name no worktree; they are allowed before the pattern compiles.
            if !command.contains("worktree") {
                return None;
            }
            pattern()?.is_match(command).then_some(REASON)
        }
        _ => None,
    }
}

/// [`PATTERN`], compiled. The hook decides once per process, so it is compiled where it is
/// used; `None` if it does not compile, which a test rules out.
fn pattern() -> Option<Regex> {
    RegexBuilder::new(PATTERN)
        .size_limit(PATTERN_LIMIT)
        .build()
        .ok()
}

/// The hook's answer that denies with `reason`.
pub fn reply(reason: &str) -> String {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

/// Runs `oso hook …` if oso's first argument is `hook`, and returns its exit status. `hook
/// deny-worktree` reads at most [`MAX_INPUT`] + 1 bytes of standard input, prints
/// [`reply`]`(`[`REASON`]`)` and a line break when [`decide`] denies, and exits 0, also when the
/// input cannot be read. Any other hook gets a usage line on standard error and exits 2. It
/// opens no file or socket and starts nothing.
pub fn main_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_none_or(|first| first != HOOK[0]) {
        return None;
    }
    let rest: Vec<_> = args.collect();
    let name = match rest.as_slice() {
        [name] => name.to_str(),
        _ => None,
    };
    Some(run(
        name,
        io::stdin().lock(),
        io::stdout().lock(),
        io::stderr().lock(),
    ))
}

/// The hook `name` over `input`, answering on `output` and saying usage on `errors`. `name` is
/// `None` when the arguments after `hook` are not one name in UTF-8.
fn run(
    name: Option<&str>,
    input: impl Read,
    mut output: impl Write,
    mut errors: impl Write,
) -> i32 {
    if name != Some(HOOK[1]) {
        let _ = writeln!(errors, "usage: oso hook {}", HOOK[1]);
        return 2;
    }
    let mut bytes = Vec::new();
    // Over the limit is allowed, so one byte more tells.
    if input
        .take(MAX_INPUT as u64 + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return 0;
    }
    if let Some(reason) = decide(&bytes) {
        let _ = writeln!(output, "{}", reply(reason));
    }
    0
}

#[cfg(test)]
mod tests;
