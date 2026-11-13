//! The text of `*Worktrees*` (ADR 0121, `worktree-list`): for each linked worktree of a
//! repository, its path, branch, uncommitted and untracked counts, whether its branch is
//! merged and how that was found, the sessions in it with their states, the oso buffers in it
//! with unsaved changes, its space, and whether `worktree-remove` would take it or why not.
//!
//! Must not depend on: `app`, `dispatch` or threads; this is a plain function over
//! what the jobs found and what the UI thread holds.
//!
//! Paths, branch names and session names are untrusted text: each is shown with its control
//! characters escaped and its length capped.

use std::path::{Path, PathBuf};

use super::checks::{self, Held, Looked};
use super::porcelain::printable;

/// The longest a field of a row is shown.
const MAX_FIELD: usize = 120;

/// One worktree as the list shows it.
#[derive(Clone, Debug)]
pub struct Row {
    pub looked: Looked,
    pub held: Held,
    /// The hosted sessions in it, each as `name (state)`, of any project.
    pub sessions: Vec<String>,
    /// The name of its space, when it has one.
    pub space: Option<String>,
}

/// The list's text, and the line each row starts on, with its worktree's path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    pub text: String,
    pub rows: Vec<(usize, PathBuf)>,
}

/// The text for `rows` of the repository at `main`, `pending` more still being looked at.
#[cfg(test)]
pub fn render(main: &Path, rows: &[Row], pending: usize) -> Shown {
    let texts: Vec<RowText> = rows.iter().map(|row| row_lines(main, row)).collect();
    assemble(main, &texts, pending)
}

/// One row, formatted.
#[derive(Clone, Debug)]
pub struct RowText {
    /// Its worktree's path.
    pub path: PathBuf,
    pub lines: Vec<String>,
}

/// The text for rows already formatted, each by [`row_lines`], of the repository at `main`,
/// `pending` more still being looked at.
pub fn assemble(main: &Path, rows: &[RowText], pending: usize) -> Shown {
    let mut text = format!(
        "Worktrees of {}    g: list again   D: remove the one at point\n\n",
        printable(&main.display().to_string(), MAX_FIELD)
    );
    let mut line = 2;
    let mut starts = Vec::with_capacity(rows.len());
    if rows.is_empty() && pending == 0 {
        text.push_str("No linked worktrees.\n");
    }
    for row in rows {
        starts.push((line, row.path.clone()));
        for row_line in &row.lines {
            text.push_str(row_line);
            text.push('\n');
            line += 1;
        }
        text.push('\n');
        line += 1;
    }
    if pending > 0 {
        text.push_str(&format!("{pending} more being looked at…\n"));
    }
    Shown { text, rows: starts }
}

/// The lines of one row: a heading, then its facts indented.
pub fn row_lines(main: &Path, row: &Row) -> RowText {
    let looked = &row.looked;
    let listed = &looked.listed;
    let path = listed
        .path
        .strip_prefix(main)
        .unwrap_or(&listed.path)
        .display()
        .to_string();
    let branch = match (&listed.branch, listed.detached) {
        (Some(branch), _) => printable(branch, MAX_FIELD),
        (None, true) => "(detached)".to_owned(),
        (None, false) => "(no branch)".to_owned(),
    };
    let verdict = match checks::removable(looked, &row.held) {
        Ok(()) => "removable".to_owned(),
        Err(check) => format!("kept: {}", check.why()),
    };
    let mut lines = vec![format!("{}  {branch}", printable(&path, MAX_FIELD))];
    let changes = looked.status.map_or_else(
        || "changes: not known".to_owned(),
        |status| match (status.changed, status.untracked) {
            (0, 0) => "clean".to_owned(),
            (changed, untracked) => format!("{changed} changed, {untracked} untracked"),
        },
    );
    let merged = looked
        .merged
        .as_ref()
        .map_or_else(|| "merge: not known".to_owned(), |m| m.how());
    lines.push(format!("  {changes}; {}", printable(&merged, MAX_FIELD)));
    if !row.sessions.is_empty() {
        let names: Vec<String> = row.sessions.iter().map(|s| printable(s, 60)).collect();
        lines.push(format!("  sessions: {}", names.join(", ")));
    }
    if !looked.external.is_empty() {
        lines.push(format!(
            "  sessions oso did not start: {}",
            looked.external.join(", ")
        ));
    }
    if row.held.unsaved > 0 {
        lines.push(format!("  unsaved buffers: {}", row.held.unsaved));
    }
    if let Some(space) = &row.space {
        lines.push(format!("  space: {}", printable(space, MAX_FIELD)));
    }
    lines.push(format!("  {}", printable(&verdict, MAX_FIELD * 2)));
    RowText {
        path: listed.path.clone(),
        lines,
    }
}

/// The row whose lines hold `line`, by its worktree's path.
pub fn row_at(shown: &Shown, line: usize) -> Option<&Path> {
    shown
        .rows
        .iter()
        .take_while(|(start, _)| *start <= line)
        .last()
        .map(|(_, path)| path.as_path())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Row, render, row_at};
    use crate::sessions::worktree::checks::{Held, Looked};
    use crate::sessions::worktree::merged::Merged;
    use crate::sessions::worktree::porcelain::{Listed, Status};

    fn row(name: &str, status: Status, merged: Merged) -> Row {
        Row {
            looked: Looked {
                listed: Listed {
                    path: PathBuf::from(format!("/r/.claude/worktrees/{name}")),
                    head: Some("a".repeat(40)),
                    branch: Some(name.to_owned()),
                    bare: false,
                    detached: false,
                    locked: false,
                    prunable: false,
                },
                linked: Ok(()),
                status: Some(status),
                merged: Some(merged),
                ignored: Vec::new(),
                external: Vec::new(),
                busy: Vec::new(),
                gh_pending: None,
            },
            held: Held::default(),
            sessions: Vec::new(),
            space: None,
        }
    }

    #[test]
    fn rows_say_what_removal_would_do() {
        let clean = Status {
            changed: 0,
            untracked: 0,
        };
        let dirty = Status {
            changed: 2,
            untracked: 1,
        };
        let rows = [
            row("a", clean, Merged::Ancestor("origin/main".into())),
            row("b", dirty, Merged::No("not merged".into())),
        ];
        let shown = render(Path::new("/r"), &rows, 0);
        assert!(
            shown.text.contains(
                ".claude/worktrees/a  a\n  clean; merged (in origin/main)\n  removable\n"
            )
        );
        assert!(shown.text.contains("2 changed, 1 untracked; not merged"));
        assert!(
            shown
                .text
                .contains("kept: 2 changed and 1 untracked files are not committed")
        );
        assert_eq!(row_at(&shown, 2), Some(Path::new("/r/.claude/worktrees/a")));
        assert_eq!(row_at(&shown, 4), Some(Path::new("/r/.claude/worktrees/a")));
        let second = shown.rows[1].0;
        assert_eq!(
            row_at(&shown, second),
            Some(Path::new("/r/.claude/worktrees/b"))
        );
        assert_eq!(row_at(&shown, 0), None);
    }

    #[test]
    fn a_row_names_its_space_and_the_busy_sessions_the_job_saw() {
        let clean = Status::default();
        let mut with = row("a", clean, Merged::Ancestor("origin/main".into()));
        with.space = Some("r/a".into());
        with.looked.busy = vec!["a".into()];
        let shown = render(Path::new("/r"), &[with], 0);
        assert!(shown.text.contains("\n  space: r/a\n"), "{}", shown.text);
        assert!(shown.text.contains("kept: session a in it is busy"));
        let without = row("b", clean, Merged::Ancestor("origin/main".into()));
        let shown = render(Path::new("/r"), &[without], 0);
        assert!(!shown.text.contains("space:"));
    }

    #[test]
    fn untrusted_names_are_escaped() {
        let mut odd = row("x", Status::default(), Merged::No("not merged".into()));
        odd.looked.listed.branch = Some("evil\u{1b}[2Jname".into());
        let shown = render(Path::new("/r"), &[odd], 1);
        assert!(!shown.text.contains('\u{1b}'));
        assert!(shown.text.contains("1 more being looked at"));
    }

    #[test]
    fn an_empty_list_says_so() {
        assert!(
            render(Path::new("/r"), &[], 0)
                .text
                .contains("No linked worktrees.")
        );
    }
}
