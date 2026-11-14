//! What the agents' session tools must never reach (ADR 0121, step 6): no tool types into a
//! terminal, starts a process itself, or reads the messaging tokens Claude Code keeps beside its
//! registry. A tool that starts something hands it to the start thread or the worktree jobs,
//! after the person's `y`; this check keeps the tools' own files from doing it directly.
//!
//! Must not depend on: the file system or the other checks; it sees only paths, tokens and
//! the raw text.

use crate::lexer::{Tok, tokenize};

/// The files of the session tools: their rules, the app's side, and the channel's methods and
/// `oso mcp`'s descriptions of them.
pub const TOOL_FILES: [&str; 4] = [
    "crates/oso/src/sessions/tools",
    "crates/oso/src/app/sessions/tools",
    "crates/oso/src/channel/protocol/sessions",
    "crates/oso/src/channel/mcp/session_tools",
];

/// Names that write to a terminal, or reach processes, which no tool file may use.
const BANNED: [&str; 9] = [
    "oso_term",
    "Terminal",
    "input",
    "paste",
    "send_text",
    "hang_up",
    "process",
    "spawn",
    "Spawn",
];

/// Whether `at`, a path relative to the workspace, is a session tool's file.
fn is_tool_file(at: &str) -> bool {
    TOOL_FILES
        .iter()
        .any(|file| at == format!("{file}.rs") || at.starts_with(&format!("{file}/")))
}

/// What is wrong with `files`, given as paths and sources. Every tool file must be there.
pub fn problems(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for file in TOOL_FILES {
        let main = format!("{file}.rs");
        if !files.iter().any(|(at, _)| *at == main) {
            problems.push(format!("the walk did not see {main}"));
        }
    }
    for (at, src) in files.iter().filter(|(at, _)| is_tool_file(at)) {
        for token in tokenize(src) {
            if let Tok::Ident(name) = &token.tok
                && BANNED.contains(&name.as_str())
            {
                problems.push(format!(
                    "{at}:{}: a session tool must not use {name}: no tool types into a terminal or starts a process (ADR 0121)",
                    token.line
                ));
            }
        }
        if src.contains(".key\"") {
            problems.push(format!(
                "{at}: a session tool never names Claude Code's .key files (ADR 0121)"
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::{TOOL_FILES, problems};

    fn tool_files(extra: &str) -> Vec<(String, String)> {
        let mut files: Vec<(String, String)> = TOOL_FILES
            .iter()
            .map(|file| (format!("{file}.rs"), "fn a() {}\n".to_owned()))
            .collect();
        files.push((
            "crates/oso/src/app/sessions/tools/asks.rs".to_owned(),
            extra.to_owned(),
        ));
        files
    }

    #[test]
    fn clean_tool_files_pass() {
        assert!(problems(&tool_files("fn b() { begin_session(); }\n")).is_empty());
    }

    #[test]
    fn typing_spawning_or_naming_a_token_file_is_caught() {
        for bad in [
            "fn b(t: &Terminal) { t.paste(x); }\n",
            "use oso_platform::process::spawn;\n",
            "fn b() { let _ = \"1.abc.key\"; }\n",
        ] {
            assert!(!problems(&tool_files(bad)).is_empty(), "{bad}");
        }
        // Another file may.
        let mut files = tool_files("");
        files.push((
            "crates/oso/src/app/terminal.rs".to_owned(),
            "fn b() { t.paste(); }\n".to_owned(),
        ));
        assert!(problems(&files).is_empty());
    }

    #[test]
    fn a_missing_tool_file_is_caught() {
        let files = vec![("crates/oso/src/sessions/tools.rs".to_owned(), String::new())];
        assert_eq!(problems(&files).len(), TOOL_FILES.len() - 1);
    }
}
