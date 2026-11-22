//! The scope a review request names, in the app (ADR 0122, "What can be reviewed"): what a
//! review's name calls it, whether two reviews review the same thing, a request's scope, a
//! pull request's number checked, and what the echo area says of a pull request (step 12b).
//!
//! Must not depend on: the editor's state, key presses or the threads; plain functions.

use oso_core::editor::{ScopeKind, row_text};

use crate::commands::review::{MAX_NAME, one_line, pull_number};
use crate::git::Id;
use crate::git::printable;
use crate::review::Scope;
use crate::review::pr::PrFound;

/// What a review's name and title call its scope.
pub(super) fn scope_label(scope: &Scope) -> String {
    let label = match scope {
        Scope::Session { .. } => "session".to_owned(),
        Scope::Uncommitted => "uncommitted".to_owned(),
        Scope::Unstaged => "unstaged".to_owned(),
        Scope::Staged => "staged".to_owned(),
        Scope::Branch { base } => format!("branch {base}"),
        // A step through a pull request's commits names one by its full id (ADR 0167).
        Scope::Commit { revision } if is_full_id(revision) => {
            format!("commit {}", &revision[..SHORT])
        }
        Scope::Commit { revision } => format!("commit {revision}"),
        Scope::Range { from, to } => format!("range {from}..{to}"),
        Scope::PullRequest { number } => format!("pull request #{number}"),
    };
    one_line(&label, MAX_NAME)
}

/// How many of a full id's digits a review's name shows.
const SHORT: usize = 12;

/// Whether `revision` is a whole object id, of 40 or 64 lower-case hex digits.
fn is_full_id(revision: &str) -> bool {
    matches!(revision.len(), 40 | 64)
        && revision
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Whether `a` and `b` review the same thing: the session scope whatever its baseline.
pub(super) fn same_scope(a: &Scope, b: &Scope) -> bool {
    matches!((a, b), (Scope::Session { .. }, Scope::Session { .. })) || a == b
}

/// The scope a request names, or why it cannot be reviewed.
pub(super) fn scope_of(
    scope: ScopeKind,
    revision: Option<String>,
    baseline: Option<Id>,
) -> Result<Scope, &'static str> {
    let revision = || revision.clone().ok_or("No revision was named");
    Ok(match scope {
        ScopeKind::Session => Scope::Session {
            baseline: baseline.ok_or("No baseline: B takes one")?,
        },
        ScopeKind::Uncommitted => Scope::Uncommitted,
        ScopeKind::Unstaged => Scope::Unstaged,
        ScopeKind::Staged => Scope::Staged,
        ScopeKind::Branch => Scope::Branch { base: revision()? },
        ScopeKind::Commit => Scope::Commit {
            revision: revision()?,
        },
        ScopeKind::Range => {
            let range = revision()?;
            let (from, to) = range.split_once("..").ok_or("A range is FROM..TO")?;
            Scope::Range {
                from: from.to_owned(),
                to: to.to_owned(),
            }
        }
        ScopeKind::PullRequest => Scope::PullRequest {
            number: pull_number(&revision()?).ok_or("A pull request is named by its number")?,
        },
    })
}

/// What the echo area says of a review's pull request: the fixed words first, then the branch
/// and the title, escaped, so that neither reads as the words oso says of it.
pub(super) fn found_said(found: &PrFound) -> String {
    let title = one_line(&row_text(&found.title), MAX_NAME);
    let branch = one_line(&row_text(&found.head_branch), MAX_NAME);
    let shown = if found.head_checked_out {
        "checked out here, so the work tree shows"
    } else {
        "its head commit shows"
    };
    format!(
        "Reviewing #{}, {shown} \u{b7} {branch} \u{b7} {title}",
        found.number
    )
}

/// What the echo area says while a review's pull request's commits are fetched.
pub(super) fn fetching_said(number: u64, remote: &str) -> String {
    let remote = printable(remote, 80);
    format!("Fetching #{number}'s commits from {remote}: closing the review stops it")
}
