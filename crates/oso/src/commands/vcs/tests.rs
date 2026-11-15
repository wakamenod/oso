//! Tests of source control's commands: what they ask of the host, moving between headings with
//! every cursor, and refusals outside a status or a trusted project.
//!
//! Must not depend on: `app` or winit.

use oso_core::command::{Args, Prefix};
use oso_core::editor::{Editor, GitHeading, GitLines, GitRequest, GitShown};
use oso_core::load::decode_all;
use oso_core::syntax::Highlights;

use super::{next_head, next_sibling_head, parent_head, previous_head, previous_sibling_head};
use crate::commands::tests::{cursor, editor, run};

const TEXT: &str = "Head:     main\n\
\n\
Untracked files (1)\n\
?  a.txt\n\
\n\
Unstaged changes (2)\n\
M  b.rs\n\
@@ -1 +1 @@\n\
-x\n\
+y\n\
M  c.rs\n";

fn heads() -> Vec<GitHeading> {
    let head = |line, depth| GitHeading { line, depth };
    vec![
        head(2, 0),
        head(3, 1),
        head(5, 0),
        head(6, 1),
        head(7, 2),
        head(10, 1),
    ]
}

/// An editor showing a status of `TEXT`, point on line `line`.
fn shown(line: usize) -> (Editor, oso_core::buffer::BufferId) {
    let (mut editor, project) = crate::commands::review::tests::in_project(false);
    let id = editor.add_git_status(project, "*git: p*");
    editor.show(id);
    editor.set_git_text(
        id,
        decode_all(TEXT.as_bytes(), Default::default()),
        GitShown {
            highlights: Highlights::new(TEXT.len()),
            outline: heads(),
            line,
            lines: Vec::new(),
        },
    );
    (editor, id)
}

fn line_start(line: usize) -> usize {
    TEXT.split_inclusive('\n').take(line).map(str::len).sum()
}

#[test]
fn moving_between_headings_follows_the_outline() {
    let heads = heads();
    assert_eq!(next_head(&heads, 0), Some(2));
    assert_eq!(next_head(&heads, 8), Some(10));
    assert_eq!(next_head(&heads, 10), None);
    assert_eq!(previous_head(&heads, 9), Some(7));
    assert_eq!(previous_head(&heads, 2), None);
    // Siblings stay in their section: c.rs follows b.rs, nothing follows c.rs.
    assert_eq!(next_sibling_head(&heads, 6), Some(10));
    assert_eq!(next_sibling_head(&heads, 9), None);
    assert_eq!(next_sibling_head(&heads, 2), Some(5));
    assert_eq!(next_sibling_head(&heads, 3), None);
    assert_eq!(previous_sibling_head(&heads, 10), Some(6));
    assert_eq!(previous_sibling_head(&heads, 6), None);
    // Before any heading, the first; up from a hunk's line, its file, then its section.
    assert_eq!(next_sibling_head(&heads, 0), Some(2));
    assert_eq!(parent_head(&heads, 8), Some(6));
    assert_eq!(parent_head(&heads, 6), Some(5));
    assert_eq!(parent_head(&heads, 5), None);
}

#[test]
fn n_and_p_move_every_cursor_with_a_count() {
    let (mut editor, _) = shown(0);
    let (result, _) = run(&mut editor, "git-next-section", Args::NONE);
    assert!(result.is_ok());
    assert_eq!(cursor(&editor), line_start(2));
    let _ = run(
        &mut editor,
        "git-next-section",
        Args::with_prefix(Some(Prefix::Number(3))),
    );
    assert_eq!(cursor(&editor), line_start(6));
    let _ = run(&mut editor, "git-next-sibling", Args::NONE);
    assert_eq!(cursor(&editor), line_start(10));
    let _ = run(&mut editor, "git-previous-section", Args::NONE);
    assert_eq!(cursor(&editor), line_start(7));
    let _ = run(&mut editor, "git-up-section", Args::NONE);
    assert_eq!(cursor(&editor), line_start(6));
    let _ = run(&mut editor, "git-previous-sibling", Args::NONE);
    assert_eq!(cursor(&editor), line_start(6));
}

#[test]
fn the_commands_ask_the_host_with_the_cursors_lines() {
    let (mut editor, id) = shown(8);
    for name in [
        "git-refresh",
        "git-toggle",
        "git-stage",
        "git-unstage",
        "git-stage-all",
        "git-unstage-all",
        "git-visit",
    ] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_ok(), "{name}");
    }
    let here = GitLines {
        first: 8,
        last: 8,
        region: false,
    };
    assert_eq!(
        editor.take_git_requests(),
        [
            GitRequest::Refresh { buffer: id },
            GitRequest::Toggle {
                buffer: id,
                line: 8
            },
            GitRequest::Stage {
                buffer: id,
                lines: vec![here]
            },
            GitRequest::Unstage {
                buffer: id,
                lines: vec![here]
            },
            GitRequest::StageAll { buffer: id },
            GitRequest::UnstageAll { buffer: id },
            GitRequest::Visit {
                buffer: id,
                line: 8,
                column: 0
            },
        ]
    );
}

#[test]
fn a_region_sends_its_lines_and_is_hidden_after() {
    let (mut editor, id) = shown(6);
    let _ = run(&mut editor, "set-mark-command", Args::NONE);
    let _ = run(&mut editor, "next-line", Args::NONE);
    let _ = run(&mut editor, "next-line", Args::NONE);
    let _ = run(&mut editor, "git-stage", Args::NONE);
    assert_eq!(
        editor.take_git_requests(),
        [GitRequest::Stage {
            buffer: id,
            lines: vec![GitLines {
                first: 6,
                last: 7,
                region: true
            }]
        }]
    );
    assert!(!editor.window().selections.mark_active().is_active());
}

#[test]
fn outside_a_status_the_commands_say_so() {
    let mut editor = editor("text");
    for name in ["git-stage", "git-toggle", "git-next-section", "git-refresh"] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        let error = result.unwrap_err();
        assert_eq!(error.to_string(), "Not in a git status", "{name}");
    }
    assert!(editor.take_git_requests().is_empty());
}

#[test]
fn the_status_needs_a_trusted_project() {
    let mut editor = editor("text");
    let (result, _) = run(&mut editor, "git-status", Args::NONE);
    assert_eq!(result.unwrap_err().to_string(), "Not in a project");
    // In the status itself, its project; refused while it is not trusted.
    let (mut editor, _) = shown(0);
    let (result, _) = run(&mut editor, "git-status", Args::NONE);
    assert_eq!(result.unwrap_err().to_string(), super::RESTRICTED);
    assert!(editor.take_git_requests().is_empty());
    // Unix only from here, as the review's trust test is: without a drive, the path is not
    // absolute on Windows, and a record of it trusts nothing there.
    if cfg!(not(unix)) {
        return;
    }
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let (result, _) = run(&mut editor, "git-status", Args::NONE);
    assert!(result.is_ok());
    assert_eq!(editor.take_git_requests(), [GitRequest::Status { project }]);
}

#[test]
fn the_keys_list_names_every_key_with_its_commands_summary() {
    let text = super::keys_text();
    for (keys, name) in super::KEYS {
        assert!(text.contains(keys) && text.contains(name), "{keys} {name}");
        let own = (super::COMMANDS.iter().chain(super::commit::COMMANDS))
            .any(|command| command.name == *name);
        assert!(own || *name == "quit-window", "{name} has no summary");
    }
}

#[test]
fn each_command_has_the_reach_it_needs() {
    use oso_core::command::Reach;
    for command in super::COMMANDS.iter().chain(super::commit::COMMANDS) {
        let looks = matches!(
            command.name,
            "git-next-section"
                | "git-previous-section"
                | "git-next-sibling"
                | "git-previous-sibling"
                | "git-up-section"
                | "git-status-keys"
        );
        // Anything that makes the host run git or open a file is the person's (ADR 0085).
        let want = if looks { Reach::Look } else { Reach::Person };
        assert_eq!(command.reach, want, "{}", command.name);
    }
}
