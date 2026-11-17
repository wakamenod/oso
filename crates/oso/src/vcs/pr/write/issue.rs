//! Writing issues' texts (ADR 0127, step 5b-1): a repository's open issues, `*issues: NAME*`,
//! and one with its comments, `*issue: #N*`, with their faces and what each line names.
//!
//! Must not depend on: `app`, `dispatch`, `gh` or the file system; pure functions of what
//! `gh` gave.
//!
//! - As a pull request's: everything GitHub gave escaped and cut, one row per line, and what a
//!   line names the table's, by index.

use oso_core::syntax::Highlight;

use super::super::issue::{Issue, IssueListed};
use super::{Page, PrLaid, PrRow, comment_rows};
use crate::git::printable;

/// The text of `repo`'s open issues, `items`.
pub(crate) fn issue_list_text(repo: &str, items: &[IssueListed]) -> PrLaid {
    let mut page = Page::default();
    page.heading(&format!(
        "Open issues of {} ({})",
        printable(repo, 120),
        items.len()
    ));
    for item in items {
        page.part(&format!("#{:<5} ", item.number), Some(Highlight::GitHash));
        page.part(&printable(&item.title, 100), None);
        for label in &item.labels {
            page.part(
                &format!("  [{}]", printable(label, 30)),
                Some(Highlight::GitTag),
            );
        }
        page.part(
            &format!("  @{}", printable(&item.author, 40)),
            Some(Highlight::GitDetail),
        );
        let day = item.updated.get(..10).unwrap_or(&item.updated);
        page.part(
            &format!("  {}", printable(day, 10)),
            Some(Highlight::GitDetail),
        );
        page.pulls.push((item.number, item.url.clone()));
        let row = PrRow::Pull(page.pulls.len() - 1);
        page.end(row);
    }
    page.laid(None, 0)
}

/// `issue`'s text, its comments with it.
pub(crate) fn issue_text(issue: &Issue) -> PrLaid {
    let mut page = Page::default();
    page.part(
        &format!("#{} ", issue.number),
        Some(Highlight::GitSectionHeading),
    );
    page.part(
        &printable(&issue.title, 300),
        Some(Highlight::GitSectionHeading),
    );
    page.end(PrRow::Other);
    page.part("State:    ", None);
    page.part(
        &printable(&issue.state.to_lowercase(), 20),
        Some(Highlight::DiffAdded),
    );
    for label in &issue.labels {
        page.part(
            &format!("  [{}]", printable(label, 30)),
            Some(Highlight::GitTag),
        );
    }
    page.end(PrRow::Other);
    page.part(
        &format!("Author:   @{}", printable(&issue.author, 80)),
        None,
    );
    page.end(PrRow::Other);
    page.part("Address:  ", None);
    page.part(&printable(&issue.url, 300), Some(Highlight::Link));
    page.end(PrRow::Other);
    if !issue.body.trim().is_empty() {
        page.blank();
        page.heading("Description");
        page.body(&issue.body, "  ", PrRow::Other);
    }
    page.blank();
    page.heading(&format!("Comments ({})", issue.comments.len()));
    for comment in &issue.comments {
        comment_rows(&mut page, comment, "  ", PrRow::Other);
    }
    page.laid(Some((issue.number, issue.url.clone())), 0)
}
