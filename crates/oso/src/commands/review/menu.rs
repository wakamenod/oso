//! What the review menu says of each head as it opens and as its counts come (ADR 0117 "Live
//! content", ADR 0122): each scope's count of files, the last scope reviewed, the branch's
//! base, the line under the title, and the heads that cannot run here and why.
//!
//! Must not depend on: `bindings`, `dispatch`, winit or the host's threads; this reads only the
//! editor's state, and never a buffer's text.
//!
//! - A note keeps one width whatever its count says, so that a count coming in never moves the
//!   float: the counts are right-aligned to [`COUNT_CELLS`], then ` (last)` or as many spaces.
//! - Heads are known by their keys, which the built-in menu's table gives them; a test keeps
//!   [`HEADS`] in step with it. A head of the person's added in `config.toml` gets no note.
//! - A project's name and the branch's base are cut to [`MAX_NAME`] characters on one line.

use std::time::{SystemTime, UNIX_EPOCH};

use oso_core::editor::{Count, Editor, ScopeKind};
use oso_core::key::KeySequence;
use oso_core::menu::{Note, Notes, Open};

use super::{MAX_NAME, one_line, review_project};

/// The cells a count takes, the widest being `>10000 files`.
const COUNT_CELLS: usize = 12;
/// What follows the last scope's count.
const LAST: &str = " (last)";
/// Why every head is unavailable outside a project.
const NO_REPOSITORY: &str = "not in a git repository";
/// Why every head is unavailable in a restricted project (ADR 0122, "Restricted projects").
const RESTRICTED: &str =
    "reviewing runs git, which this project is not trusted to run: M-x trust-project";

/// What each head's keys stand for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Head {
    Scope(ScopeKind),
    Baseline,
}

/// The heads of the built-in review menu, by their keys, and their commands.
const HEADS: [(&str, &str, Head); 8] = [
    ("D", "review-session", Head::Scope(ScopeKind::Session)),
    (
        "w",
        "review-uncommitted",
        Head::Scope(ScopeKind::Uncommitted),
    ),
    ("u", "review-unstaged", Head::Scope(ScopeKind::Unstaged)),
    ("s", "review-staged", Head::Scope(ScopeKind::Staged)),
    ("b", "review-branch", Head::Scope(ScopeKind::Branch)),
    ("c", "review-commit", Head::Scope(ScopeKind::Commit)),
    ("r", "review-range", Head::Scope(ScopeKind::Range)),
    ("B", "review-mark-baseline", Head::Baseline),
];

/// The keys and command of each head [`HEADS`] knows, for the test that keeps it in step with
/// the built-in table.
#[cfg(test)]
pub(crate) fn menu_heads() -> impl Iterator<Item = (&'static str, &'static str)> {
    HEADS.iter().map(|(keys, command, _)| (*keys, *command))
}

fn head_of(keys: &KeySequence) -> Option<Head> {
    HEADS.iter().find_map(|(text, _, head)| {
        let parsed: KeySequence = text.parse().ok()?;
        (parsed == *keys).then_some(*head)
    })
}

/// A count as its note says it, right-aligned.
fn count_text(count: Option<Count>) -> String {
    let text = match count {
        None | Some(Count::Running) => "\u{2026}".to_owned(),
        Some(Count::Files(0)) => "nothing".to_owned(),
        Some(Count::Files(1)) => "1 file".to_owned(),
        Some(Count::Files(n)) => format!("{n} files"),
        Some(Count::More) => ">10000 files".to_owned(),
        Some(Count::Failed) => "?".to_owned(),
    };
    format!("{text:>COUNT_CELLS$}")
}

/// How long ago `at`, in seconds since the Unix epoch, was, as the title's line says it.
fn ago(at: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(at, |now| now.as_secs());
    let minutes = now.saturating_sub(at) / 60;
    match minutes {
        0 => "just now".to_owned(),
        m if m < 60 => format!("{m} min ago"),
        m if m < 48 * 60 => format!("{} h ago", m / 60),
        m => format!("{} days ago", m / (24 * 60)),
    }
}

/// The review menu's notes: built in, so it reads `editor` alone.
pub fn review_menu(editor: &Editor, open: &Open) -> Notes {
    let definition = open.definition();
    let project = review_project(editor);
    let trusted = project.is_some_and(|id| editor.project_trusted(id));
    let menu = project.and_then(|id| editor.scope_menu(id));
    let last = menu.and_then(|menu| menu.last());
    let baseline = menu.and_then(|menu| menu.baseline_taken());
    let mut notes = Notes::default();
    if let Some(id) = project
        && let Some(found) = editor.project(id)
    {
        let root = super::super::shown_place(found.root(), editor.home(), 40);
        let name = one_line(found.name(), MAX_NAME);
        let mut line = format!("Review \u{b7} {name} \u{b7} {root}");
        if let Some(at) = baseline {
            line.push_str(&format!(" \u{b7} baseline {}", ago(at)));
        }
        line.push_str(" \u{b7} comments go to the kill ring until sending exists");
        notes.line = Some(line);
    }
    let ret = last.unwrap_or(ScopeKind::Uncommitted);
    for (index, head) in definition.heads().iter().enumerate() {
        let Some(what) = head_of(&head.keys) else {
            continue;
        };
        let mut note = Note::default();
        match what {
            Head::Scope(scope) => {
                let counted = trusted
                    && !matches!(scope, ScopeKind::Commit | ScopeKind::Range)
                    && (scope != ScopeKind::Session || baseline.is_some());
                if !counted {
                    note.text = format!("{:COUNT_CELLS$}", "");
                } else {
                    note.text = count_text(menu.and_then(|menu| menu.count(scope)));
                }
                note.text
                    .push_str(if last == Some(scope) { LAST } else { "       " });
                if scope == ScopeKind::Branch
                    && let Some(base) = menu.and_then(|menu| menu.base())
                {
                    note.text.push_str(" \u{b7} ");
                    note.text.push_str(&one_line(base, MAX_NAME));
                }
                if scope == ScopeKind::Session && baseline.is_none() {
                    note.unavailable = Some("no baseline: B takes one".into());
                }
                if scope == ret {
                    notes.ret_head = Some(index);
                }
            }
            Head::Baseline => {}
        }
        if project.is_none() {
            note.unavailable = Some(NO_REPOSITORY.into());
        } else if !trusted {
            note.unavailable = Some(RESTRICTED.into());
        }
        notes.heads.push((index, note));
    }
    notes
}
