//! The rules of who answers a native worker's request: each reason it stays the person's, and
//! the tricks of paths, names and quoting each must catch (ADR 0186).
//!
//! Must not depend on: the app or the file system; these run on values alone.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{MAX_LISTED, PersonOnly, Rules, eligible, listed};
use crate::channel::protocol::permission::Call;
use crate::sessions::native::asks::RequestId;

/// The worker's directory: a worktree under its repository's `.claude`, as oso makes them.
pub(super) fn directory() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\r\.claude\worktrees\w")
    } else {
        PathBuf::from("/r/.claude/worktrees/w")
    }
}

/// `rest` under the worker's directory, as an absolute path.
pub(super) fn under(rest: &str) -> String {
    directory().join(rest).display().to_string()
}

pub(super) fn rules(tools: &[&str]) -> Rules {
    let tools: Vec<String> = tools.iter().map(|&tool| tool.to_owned()).collect();
    Rules::new(&tools, &[]).unwrap()
}

pub(super) fn all() -> Rules {
    rules(&[
        "Read",
        "Edit",
        "Write",
        "Grep",
        "Glob",
        "NotebookEdit",
        "Bash",
    ])
}

pub(super) fn judge(tool: &str, input: &Value) -> Result<(), PersonOnly> {
    eligible(tool, Some(input), false, &directory(), &all())
}

pub(super) fn bash(command: &str) -> Result<(), PersonOnly> {
    judge("Bash", &json!({ "command": command }))
}

#[test]
fn a_listed_tool_on_a_file_of_the_workers_own_is_answerable() {
    let edit = json!({ "file_path": under("src/main.rs"), "old_string": "a", "new_string": "b" });
    assert_eq!(judge("Edit", &edit), Ok(()));
    assert_eq!(
        judge("Read", &json!({ "file_path": under("src/lib.rs") })),
        Ok(())
    );
    let grep = json!({ "pattern": "fn main", "path": under("") });
    assert_eq!(judge("Grep", &grep), Ok(()));
    let glob = json!({ "pattern": "src/**/*.rs", "path": under("") });
    assert_eq!(judge("Glob", &glob), Ok(()));
    assert_eq!(bash("cargo test -p oso-core"), Ok(()));
    assert_eq!(bash("git status && git diff"), Ok(()));
    assert_eq!(
        bash(&format!("cd {} && cargo build", directory().display())),
        Ok(()),
        "the worker's own directory, under .claude, is not denied"
    );
}

#[test]
fn every_reason_stays_the_persons() {
    let read = json!({ "file_path": under("a.rs") });
    let dir = directory();
    // Off: no tool listed, whatever else holds.
    assert_eq!(
        eligible("Read", Some(&read), false, &dir, &rules(&[])),
        Err(PersonOnly::Off)
    );
    // Tool: not listed.
    assert_eq!(
        eligible("Write", Some(&read), false, &dir, &rules(&["Read"])),
        Err(PersonOnly::Tool)
    );
    // Card: a question or a plan, even of a listed tool.
    let plan = rules(&["ExitPlanMode", "AskUserQuestion"]);
    assert_eq!(
        eligible("ExitPlanMode", Some(&json!({})), true, &dir, &plan),
        Err(PersonOnly::Card)
    );
    // NotShown: no input, or one over MAX_LISTED.
    assert_eq!(
        eligible("Read", None, false, &dir, &all()),
        Err(PersonOnly::NotShown)
    );
    let big = json!({ "file_path": under("a.rs"), "content": "x".repeat(MAX_LISTED) });
    assert_eq!(judge("Write", &big), Err(PersonOnly::NotShown));
    // Secret, Outside and Denied.
    assert_eq!(
        judge("Read", &json!({ "file_path": under(".env") })),
        Err(PersonOnly::Secret)
    );
    assert_eq!(
        judge("Read", &json!({ "file_path": "/etc/hosts" })),
        Err(PersonOnly::Outside)
    );
    assert_eq!(bash("git push origin main"), Err(PersonOnly::Denied));
    // Each says why, in words of its own.
    let says: Vec<&str> = [
        PersonOnly::Off,
        PersonOnly::Tool,
        PersonOnly::Card,
        PersonOnly::NotShown,
        PersonOnly::Secret,
        PersonOnly::Outside,
        PersonOnly::Denied,
    ]
    .into_iter()
    .map(PersonOnly::says)
    .collect();
    for (at, said) in says.iter().enumerate() {
        assert!(!said.is_empty());
        assert!(!says[..at].contains(said), "{said}");
    }
}

#[test]
fn a_secret_is_judged_before_the_rules_so_its_listing_never_shows_it() {
    let secret = json!({ "file_path": under(".env") });
    for rules in [rules(&[]), rules(&["Edit"])] {
        assert_eq!(
            eligible("Read", Some(&secret), false, &directory(), &rules),
            Err(PersonOnly::Secret)
        );
    }
}

#[test]
fn mcp_tools_are_never_answerable_even_listed() {
    let rules = rules(&["mcp__oso__open", "mcp__github__merge"]);
    for tool in ["mcp__oso__open", "mcp__github__merge"] {
        assert_eq!(
            eligible(tool, Some(&json!({})), false, &directory(), &rules),
            Err(PersonOnly::Tool),
            "{tool}"
        );
    }
}

#[test]
fn a_path_outside_the_workers_directory_is_the_persons() {
    let outside = |path: &str| judge("Edit", &json!({ "file_path": path }));
    for path in [
        "../other/a.rs".to_owned(),
        under("../a.rs"),
        under("src/../../a.rs"),
        under("src/../a.rs"),
        "/etc/passwd".to_owned(),
        "~/a.rs".to_owned(),
        String::new(),
        // A sibling whose name starts as the directory's does.
        format!("{}2/a.rs", directory().display()),
        // Case: lexically another directory, even where the disk folds case.
        under("a.rs").replace("worktrees", "Worktrees"),
    ] {
        assert_eq!(outside(&path), Err(PersonOnly::Outside), "{path:?}");
    }
    assert_eq!(
        judge("Edit", &json!({ "file_path": 3 })),
        Err(PersonOnly::Outside),
        "a path that is no string"
    );
    for (tool, field) in [("Grep", "path"), ("NotebookEdit", "notebook_path")] {
        assert_eq!(
            judge(tool, &json!({ field: "/tmp/x" })),
            Err(PersonOnly::Outside),
            "{tool}"
        );
    }
    // A search's own pattern names where it looks, too.
    for input in [
        json!({ "pattern": "/etc/**" }),
        json!({ "pattern": "../**/*.rs" }),
        json!({ "pattern": "src/../../**" }),
    ] {
        assert_eq!(judge("Glob", &input), Err(PersonOnly::Outside), "{input}");
    }
    let grep = json!({ "pattern": "x", "glob": "../**" });
    assert_eq!(judge("Grep", &grep), Err(PersonOnly::Outside));
}

#[test]
fn git_claude_and_mcp_json_under_the_workers_directory_are_the_persons() {
    for rest in [
        ".git/config",
        ".git/hooks/pre-commit",
        ".git",
        "sub/.git/config",
        ".claude/settings.json",
        ".claude/settings.local.json",
        ".claude",
        ".mcp.json",
        "sub/.mcp.json",
        // Case, as macOS and Windows fold it.
        ".GIT/config",
        ".Claude/settings.json",
        ".MCP.JSON",
        // Letters the disk folds to ASCII, and marks it ignores.
        ".mcp.j\u{17f}on",
        ".\u{200c}git/config",
        ".gi\u{ff54}/config",
        // As Windows reads trailing dots and spaces, streams and short names.
        ".git./config",
        ".git /config",
        ".git::$INDEX_ALLOCATION/config",
        "GIT~1/config",
    ] {
        let path = under(rest);
        assert_eq!(
            judge("Write", &json!({ "file_path": path, "content": "" })),
            Err(PersonOnly::Outside),
            "{rest:?}"
        );
    }
    // Relative, as given.
    assert_eq!(
        judge("Edit", &json!({ "file_path": ".git/config" })),
        Err(PersonOnly::Outside)
    );
    // Names that only look alike.
    for rest in [".github/workflows/ci.yml", ".gitignore", "claude/notes.md"] {
        assert_eq!(
            judge("Edit", &json!({ "file_path": under(rest) })),
            Ok(()),
            "{rest:?}"
        );
    }
}

#[test]
fn a_secret_name_in_a_path_field_or_a_bash_token_is_the_persons() {
    for rest in [
        ".env",
        ".env.local",
        "config/.ENV",
        "id_rsa",
        "keys/server.pem",
        "credentials.json",
        ".ssh/config",
        // Folded to ASCII on the disk, or with a mark it ignores.
        "credential\u{17f}.json",
        ".\u{df}h/config",
        ".e\u{200b}nv",
    ] {
        assert_eq!(
            judge("Read", &json!({ "file_path": under(rest) })),
            Err(PersonOnly::Secret),
            "{rest:?}"
        );
    }
    assert_eq!(
        judge("Grep", &json!({ "pattern": "KEY", "glob": ".env*" })),
        Err(PersonOnly::Secret)
    );
    assert_eq!(
        judge("Glob", &json!({ "pattern": "**/*.pem" })),
        Err(PersonOnly::Secret)
    );
    for command in [
        "cat .env",
        "cat ~/.ssh/id_rsa",
        "cat \".env\"",
        "cat '.env'",
        "cat .e''nv",
        "cat .e\"\"nv",
        "cat \\.env",
        "cp id_ed25519 /tmp/x",
        "grep KEY config/.env.production",
        "source=.env ./run",
        "scp_like host:.env",
        "x=$(cat .env)",
        "tar c {a,.env}",
        "cat .ENV",
        "cat credential\u{17f}",
    ] {
        assert_eq!(bash(command), Err(PersonOnly::Secret), "{command:?}");
    }
    // A file of a name that only looks alike.
    assert_eq!(bash("cat environment.md"), Ok(()));
    assert_eq!(bash("echo 日本語のテスト"), Ok(()));
}

#[test]
fn an_input_is_listed_whole_up_to_the_most_and_else_not_at_all() {
    let small = json!({ "file_path": "a" });
    assert_eq!(
        listed(Some(&small), MAX_LISTED),
        (Some(small.clone()), false)
    );
    assert_eq!(listed(None, MAX_LISTED), (None, true));
    let length = serde_json::to_string(&small).unwrap().len();
    assert_eq!(listed(Some(&small), length), (Some(small.clone()), false));
    assert_eq!(listed(Some(&small), length - 1), (None, true));
    let big = json!({ "content": "x".repeat(MAX_LISTED) });
    assert_eq!(listed(Some(&big), MAX_LISTED), (None, true));
    // An input whose escapes make it longer as JSON than as text.
    let quoted = json!({ "content": "\"".repeat(MAX_LISTED / 2) });
    assert_eq!(listed(Some(&quoted), MAX_LISTED), (None, true));
}

#[test]
fn the_directory_passed_in_is_taken_as_given() {
    let dir = Path::new(if cfg!(windows) { r"C:\w" } else { "/w" });
    let input = json!({ "file_path": dir.join("a.rs").display().to_string() });
    assert_eq!(eligible("Read", Some(&input), false, dir, &all()), Ok(()));
}

pub(super) fn listing(tool: &str, input: Option<&Value>, card: bool, rules: &Rules) -> Value {
    let directory = Path::new(if cfg!(windows) { r"C:\w" } else { "/w" });
    let file = directory.join("a.rs").display().to_string();
    let input = input.map(|input| {
        let mut input = input.clone();
        if input.get("file_path").is_some_and(|path| path == "FILE") {
            input["file_path"] = json!(file);
        }
        input
    });
    let call = Call {
        tool_name: tool.to_owned(),
        tool_use_id: "t1".to_owned(),
        input,
    };
    super::listing(RequestId::default(), &call, card, 12, directory, rules)
}

#[test]
fn a_request_is_listed_with_its_input_and_whether_it_is_answerable() {
    let rules = Rules::new(&["Edit".to_owned()], &[]).unwrap();
    let edit = json!({ "file_path": "FILE", "old_string": "a", "new_string": "b" });
    let listed = listing("Edit", Some(&edit), false, &rules);
    assert_eq!(listed["id"], "r0");
    assert_eq!(listed["tool"], "Edit");
    assert_eq!(listed["input"]["old_string"], "a");
    assert_eq!(listed["input_cut"], false);
    assert_eq!(listed["shown_s"], 12);
    assert_eq!(listed["answerable"], true);
    assert_eq!(listed["person_only"], Value::Null);
    // Off: listed, not answerable, and why.
    let off = listing("Edit", Some(&edit), false, &Rules::default());
    assert_eq!(off["input"]["old_string"], "a");
    assert_eq!(off["answerable"], false);
    assert_eq!(off["person_only"], PersonOnly::Off.says());
}

#[test]
fn a_secret_or_a_cards_input_is_listed_as_null() {
    let rules = Rules::new(&["Read".to_owned(), "Bash".to_owned()], &[]).unwrap();
    for (tool, input) in [
        ("Read", json!({ "file_path": "/w/.env" })),
        ("Bash", json!({ "command": "cat ~/.ssh/id_rsa" })),
    ] {
        for rules in [&rules, &Rules::default()] {
            let listed = listing(tool, Some(&input), false, rules);
            assert_eq!(listed["input"], Value::Null, "{input}");
            assert_eq!(listed["input_cut"], false);
            assert_eq!(listed["answerable"], false);
            assert_eq!(listed["person_only"], PersonOnly::Secret.says());
        }
    }
    let plan = json!({ "plan": "the plan" });
    let listed = listing("ExitPlanMode", Some(&plan), true, &rules);
    assert_eq!(listed["input"], Value::Null);
    assert_eq!(listed["person_only"], PersonOnly::Card.says());
}

#[test]
fn an_input_past_the_most_is_listed_as_cut() {
    let rules = Rules::new(&["Write".to_owned()], &[]).unwrap();
    let big = json!({ "file_path": "FILE", "content": "x".repeat(MAX_LISTED) });
    let listed = listing("Write", Some(&big), false, &rules);
    assert_eq!(listed["input"], Value::Null);
    assert_eq!(listed["input_cut"], true);
    assert_eq!(listed["person_only"], PersonOnly::NotShown.says());
    let absent = listing("Write", None, false, &rules);
    assert_eq!(absent["input_cut"], true);
    assert_eq!(absent["person_only"], PersonOnly::NotShown.says());
}

#[test]
fn a_relative_path_or_a_search_with_no_path_is_the_persons_as_a_cd_moves_them() {
    // Claude Code takes them from its shell's directory, which a `cd` before changes.
    for (tool, input) in [
        ("Read", json!({ "file_path": "src/lib.rs" })),
        ("Grep", json!({ "pattern": "url" })),
        ("Glob", json!({ "pattern": "**/*.rs" })),
        ("Grep", json!({ "pattern": "url", "path": "src" })),
    ] {
        assert_eq!(judge(tool, &input), Err(PersonOnly::Outside), "{input}");
    }
    let grep = json!({ "pattern": "url", "path": under("src") });
    assert_eq!(judge("Grep", &grep), Ok(()));
}

#[test]
fn a_glob_that_may_match_a_secret_or_climb_is_the_persons() {
    let grep = |glob: &str| {
        judge(
            "Grep",
            &json!({ "pattern": "KEY", "path": under(""), "glob": glob }),
        )
    };
    for glob in ["*.{pem,key}", ".e?v", ".en[v]", ".e*", "id_*", "{a,.env}"] {
        assert_eq!(grep(glob), Err(PersonOnly::Secret), "{glob:?}");
    }
    assert!(grep("{..,src}/**").is_err());
    assert_eq!(grep("src/..x/../*"), Err(PersonOnly::Outside));
    for glob in ["*.rs", "**/*.rs", "src/**", "*.d.ts", "*"] {
        assert_eq!(grep(glob), Ok(()), "{glob:?}");
    }
}

#[test]
fn a_secret_in_any_string_of_the_input_withholds_it() {
    let bad = json!({ "command": 1, "file_path": "~/.ssh/id_rsa" });
    assert_eq!(judge("Bash", &bad), Err(PersonOnly::Secret));
    for (tool, input) in [
        (
            "Edit",
            json!({ "file_path": under("a.rs"), "paths": [under(".env")] }),
        ),
        (
            "Write",
            json!({ "file_path": under("a.rs"), "filename": "id_rsa" }),
        ),
        (
            "Edit",
            json!({ "file_path": under("a.rs"), "edits": [{ "file": "x.pem" }] }),
        ),
    ] {
        assert_eq!(judge(tool, &input), Err(PersonOnly::Secret), "{input}");
    }
    let listed = listing("Bash", Some(&bad), false, &all());
    assert_eq!(listed["input"], Value::Null);
}

#[test]
fn a_name_is_judged_whole_and_as_windows_cuts_it() {
    assert_eq!(
        judge("Read", &json!({ "file_path": under("certs/a:b.pem") })),
        Err(PersonOnly::Secret)
    );
    for part in [".. ", ". .", "..."] {
        assert_eq!(
            judge(
                "Read",
                &json!({ "file_path": under(&format!("{part}/a.rs")) })
            ),
            Err(PersonOnly::Outside),
            "{part:?}"
        );
    }
    assert_eq!(
        judge("Read", &json!({ "file_path": under("日本/a.rs") })),
        Ok(())
    );
}

#[test]
fn a_secret_among_the_words_or_keys_of_any_string_withholds_the_input() {
    let rules = rules(&["mcp__x__y"]);
    for input in [
        json!({ "args": "cat .env" }),
        json!({ "files": "a.rs .env" }),
        json!({ "map": { ".env": "x" } }),
    ] {
        assert_eq!(
            eligible("mcp__x__y", Some(&input), false, &directory(), &rules),
            Err(PersonOnly::Secret),
            "{input}"
        );
    }
}

#[test]
fn a_reply_summarizes_only_what_the_listing_could_show() {
    use super::shows_summary;
    assert!(shows_summary(Ok(())));
    for shown in [
        PersonOnly::Off,
        PersonOnly::Tool,
        PersonOnly::Outside,
        PersonOnly::Denied,
    ] {
        assert!(shows_summary(Err(shown)), "{shown:?}");
    }
    // Neither a card's, a secret's, nor an input not listed whole, which may name one past
    // its first 8 KiB.
    for withheld in [PersonOnly::Card, PersonOnly::Secret, PersonOnly::NotShown] {
        assert!(!shows_summary(Err(withheld)), "{withheld:?}");
    }
}
