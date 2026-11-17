//! Writing pull requests' texts (ADR 0127, step 5a): the status's head line for a branch's pull
//! request, `*pr: #N*` with its checks, review threads and conversation, and a repository's
//! list, with their faces and the table of what each line names.
//!
//! Must not depend on: `app`, `dispatch`, `gh` or the file system; pure functions of what
//! `gh` gave.
//!
//! - Everything GitHub gave is escaped, one row per line, each row and each body cut, so that
//!   no comment can draw a row of its own, reorder what is around it, or grow without bound.
//! - What a line names (a check's address, a thread's place, a listed pull request) is the
//!   table's, never read back from the text.

use std::sync::Arc;

use oso_core::load::LoadSnapshot;
use oso_core::syntax::{Highlight, Highlights};

use super::{Bucket, Check, Comment, Counts, Listed, PrHead, PrView, State, Thread};
use crate::git::printable;
use crate::vcs::layout::write::Out;

/// The most characters of one shown row.
const MAX_ROW: usize = 300;
/// The most rows of one text.
const MAX_ROWS: usize = 20_000;
/// The most rows of one body or comment.
const MAX_BODY_ROWS: usize = 60;
/// The most rows of a review comment's diff shown: its last ones, where the comment is.
const MAX_HUNK_ROWS: usize = 4;

/// What a line of a pull request's buffer, or of a list, names: an index into its table of
/// checks, threads or listed pull requests, so that a line costs no copy of what it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrRow {
    Other,
    /// A check, with its address: [`PrBuilt::links`].
    Check(usize),
    /// A review thread's file, from the repository's root, and line: [`PrBuilt::threads`].
    Thread(usize),
    /// A listed pull request: [`PrBuilt::pulls`].
    Pull(usize),
}

/// What a laid text's lines name, and the pull request it shows.
#[derive(Debug, Default)]
pub(crate) struct PrBuilt {
    pub(crate) rows: Vec<PrRow>,
    /// The review threads' files and lines.
    pub(crate) threads: Vec<(String, Option<u64>)>,
    /// The checks' addresses.
    pub(crate) links: Vec<String>,
    /// The checks' names, beside their addresses (step 6b).
    pub(crate) check_names: Vec<String>,
    /// The listed pull requests' numbers and addresses.
    pub(crate) pulls: Vec<(u64, String)>,
    /// The pull request's number and address, for a pull request's buffer.
    pub(crate) pull: Option<(u64, String)>,
    /// The line of the checks' heading.
    pub(crate) checks_line: usize,
}

impl PrBuilt {
    pub(crate) fn row(&self, line: usize) -> PrRow {
        self.rows.get(line).copied().unwrap_or(PrRow::Other)
    }
}

/// A text laid out off the UI thread, for it to put in at O(1).
pub(crate) struct PrLaid {
    pub(crate) text: LoadSnapshot,
    pub(crate) highlights: Highlights,
    pub(crate) built: Arc<PrBuilt>,
}

/// The parts of the status's `PR:` line for `head`, the branch's pull request, `stale` if its
/// head is not `tip`, the commit checked out.
pub(crate) fn head_parts(head: &PrHead, tip: Option<&str>) -> Vec<(String, Option<Highlight>)> {
    let mut parts = vec![
        (format!("#{}", head.number), Some(Highlight::GitHash)),
        (format!(" {}", printable(&head.title, 120)), None),
        (
            format!("  {}", head.state.word()),
            Some(state_face(head.state)),
        ),
    ];
    if let Some(review) = head.review {
        parts.push((format!(", {}", review.words()), Some(Highlight::GitDetail)));
    }
    parts.extend(counts_parts(head.counts));
    let stale = tip.is_some_and(|tip| !tip.is_empty() && !head.head.starts_with(tip));
    if stale {
        parts.push(("  stale".to_owned(), Some(Highlight::Outdated)));
    }
    parts
}

/// `view`'s text, its checks and threads with it.
pub(crate) fn view_text(view: &PrView) -> PrLaid {
    let mut page = Page::default();
    let head = &view.head;
    page.part(
        &format!("#{} ", head.number),
        Some(Highlight::GitSectionHeading),
    );
    page.part(
        &printable(&head.title, MAX_ROW),
        Some(Highlight::GitSectionHeading),
    );
    page.end(PrRow::Other);
    page.part("State:    ", None);
    page.part(head.state.word(), Some(state_face(head.state)));
    if let Some(review) = head.review {
        page.part(&format!(", {}", review.words()), Some(Highlight::GitDetail));
    }
    for (text, face) in counts_parts(head.counts) {
        page.part(&text, face);
    }
    page.end(PrRow::Other);
    page.part("Branches: ", None);
    page.part(
        &printable(&view.base, MAX_ROW),
        Some(Highlight::GitBranchRemote),
    );
    page.part(" ← ", None);
    page.part(
        &printable(&view.branch, MAX_ROW),
        Some(Highlight::GitBranchRemote),
    );
    let short = head.head.get(..7).unwrap_or(&head.head);
    page.part(
        &format!(" {}", printable(short, 7)),
        Some(Highlight::GitHash),
    );
    page.end(PrRow::Other);
    page.part(&format!("Author:   @{}", printable(&view.author, 80)), None);
    page.end(PrRow::Other);
    page.part("Address:  ", None);
    page.part(&printable(&head.url, MAX_ROW), Some(Highlight::Link));
    page.end(PrRow::Other);
    if !view.body.trim().is_empty() {
        page.blank();
        page.heading("Description");
        page.body(&view.body, "  ", PrRow::Other);
    }
    page.blank();
    let checks_line = page.rows.len();
    checks(&mut page, &view.checks);
    page.blank();
    threads(&mut page, &view.threads, view.more_threads);
    page.blank();
    page.heading(&format!("Conversation ({})", view.comments.len()));
    for comment in &view.comments {
        comment_rows(&mut page, comment, "  ", PrRow::Other);
    }
    page.laid(Some((head.number, head.url.clone())), checks_line)
}

/// The text of `repo`'s open pull requests, `items`.
pub(crate) fn list_text(repo: &str, items: &[Listed]) -> PrLaid {
    let mut page = Page::default();
    page.heading(&format!(
        "Open pull requests of {} ({})",
        printable(repo, 120),
        items.len()
    ));
    for item in items {
        page.part(&format!("#{:<5} ", item.number), Some(Highlight::GitHash));
        let state = if item.draft {
            State::Draft
        } else {
            State::Open
        };
        page.part(&format!("{:<6} ", state.word()), Some(state_face(state)));
        page.part(&printable(&item.title, 100), None);
        page.part(
            &format!("  @{}", printable(&item.author, 40)),
            Some(Highlight::GitDetail),
        );
        page.part(
            &format!("  {}", printable(&item.branch, 60)),
            Some(Highlight::GitBranchRemote),
        );
        if let Some(review) = item.review {
            page.part(&format!("  {}", review.words()), Some(Highlight::GitDetail));
        }
        for (text, face) in counts_parts(item.counts) {
            page.part(&text, face);
        }
        page.pulls.push((item.number, item.url.clone()));
        let row = PrRow::Pull(page.pulls.len() - 1);
        page.end(row);
    }
    page.laid(None, 0)
}

fn checks(page: &mut Page, checks: &[Check]) {
    page.heading(&format!("Checks ({})", checks.len()));
    for check in checks {
        let (mark, face) = match check.bucket {
            Bucket::Pass => ("✓", Highlight::DiffAdded),
            Bucket::Fail => ("✗", Highlight::DiffRemoved),
            Bucket::Pending => ("●", Highlight::GitDetail),
            Bucket::Skipping => ("-", Highlight::GitDetail),
            Bucket::Cancel => ("⊘", Highlight::DiffRemoved),
        };
        page.part(&format!("  {mark} "), Some(face));
        page.part(&printable(&check.name, 120), None);
        if !check.workflow.is_empty() {
            page.part(
                &format!("  {}", printable(&check.workflow, 60)),
                Some(Highlight::GitDetail),
            );
        }
        if let Some(took) = check.took {
            page.part(
                &format!("  {}m {}s", took / 60, took % 60),
                Some(Highlight::GitDetail),
            );
        }
        page.links.push(check.link.clone());
        page.check_names.push(check.name.clone());
        let row = PrRow::Check(page.links.len() - 1);
        page.end(row);
    }
}

fn threads(page: &mut Page, threads: &[Thread], more: usize) {
    let open = threads.iter().filter(|thread| !thread.resolved).count();
    page.heading(&format!(
        "Review threads ({}, {open} unresolved)",
        threads.len() + more
    ));
    for thread in threads {
        let row = page.thread(&thread.path, thread.line);
        page.part("  ", None);
        page.part(
            &printable(&thread.path, 200),
            Some(Highlight::DiffFileHeader),
        );
        if let Some(line) = thread.line {
            page.part(&format!(":{line}"), Some(Highlight::DiffFileHeader));
        }
        let (state, face) = if thread.resolved {
            ("  resolved", Highlight::GitDetail)
        } else {
            ("  unresolved", Highlight::Outdated)
        };
        page.part(state, Some(face));
        page.end(row);
        if let Some(first) = thread.comments.first() {
            hunk(page, &first.hunk, row);
        }
        for comment in &thread.comments {
            comment_rows(page, comment, "    ", row);
        }
        if thread.more > 0 {
            page.part(
                &format!("    and {} more comments", thread.more),
                Some(Highlight::GitDetail),
            );
            page.end(row);
        }
    }
    if more > 0 {
        page.part(
            &format!("  and {more} more threads"),
            Some(Highlight::GitDetail),
        );
        page.end(PrRow::Other);
    }
}

/// The last rows of a review comment's diff, the lines it is on.
fn hunk(page: &mut Page, hunk: &str, row: PrRow) {
    let lines: Vec<&str> = hunk.lines().collect();
    for line in &lines[lines.len().saturating_sub(MAX_HUNK_ROWS)..] {
        let face = match line.as_bytes().first() {
            Some(b'+') => Some(Highlight::DiffAdded),
            Some(b'-') => Some(Highlight::DiffRemoved),
            Some(b'@') => Some(Highlight::DiffHunkHeader),
            _ => None,
        };
        page.part("    │ ", Some(Highlight::GitDetail));
        page.part(&printable(line, MAX_ROW), face);
        page.end(row);
    }
}

fn comment_rows(page: &mut Page, comment: &Comment, indent: &str, row: PrRow) {
    page.part(indent, None);
    page.part(
        &format!("@{}", printable(&comment.author, 40)),
        Some(Highlight::CommentPerson),
    );
    if !comment.association.is_empty() && comment.association != "NONE" {
        let association = printable(&comment.association, 20);
        page.part(&format!(" ({association})"), Some(Highlight::GitDetail));
    }
    page.end(row);
    page.body(&comment.body, &format!("{indent}  "), row);
}

fn counts_parts(counts: Counts) -> Vec<(String, Option<Highlight>)> {
    let mut parts = Vec::new();
    if counts.pass + counts.fail + counts.pending > 0 {
        parts.push((format!("  ✓ {}", counts.pass), Some(Highlight::DiffAdded)));
        parts.push((format!(" ✗ {}", counts.fail), Some(Highlight::DiffRemoved)));
        parts.push((format!(" ● {}", counts.pending), Some(Highlight::GitDetail)));
    }
    parts
}

fn state_face(state: State) -> Highlight {
    match state {
        State::Open => Highlight::DiffAdded,
        State::Draft | State::Closed => Highlight::GitDetail,
        State::Merged => Highlight::GitBranchLocal,
    }
}

/// A text being written, as the status's is, and what each of its lines names; past
/// [`MAX_ROWS`] rows it says so and writes no more.
#[derive(Default)]
struct Page {
    out: Out,
    rows: Vec<PrRow>,
    threads: Vec<(String, Option<u64>)>,
    links: Vec<String>,
    check_names: Vec<String>,
    pulls: Vec<(u64, String)>,
    full: bool,
}

impl Page {
    /// Adds `text`, a part of one line, in `face`.
    fn part(&mut self, text: &str, face: Option<Highlight>) {
        if !self.full {
            self.out.part(text.as_bytes(), face);
        }
    }

    /// Ends the line, which names `row`.
    fn end(&mut self, row: PrRow) {
        if self.full {
            return;
        }
        self.out.end_line();
        self.rows.push(row);
        if self.rows.len() + 1 >= MAX_ROWS {
            self.out.part(
                b"The rest is cut: the address shows it all",
                Some(Highlight::GitDetail),
            );
            self.out.end_line();
            self.rows.push(PrRow::Other);
            self.full = true;
        }
    }

    fn blank(&mut self) {
        self.end(PrRow::Other);
    }

    fn heading(&mut self, text: &str) {
        self.part(text, Some(Highlight::GitSectionHeading));
        self.end(PrRow::Other);
    }

    /// The row that names a review thread at `path` and `line`, kept once.
    fn thread(&mut self, path: &str, line: Option<u64>) -> PrRow {
        self.threads.push((path.to_owned(), line));
        PrRow::Thread(self.threads.len() - 1)
    }

    /// `text`'s lines, each escaped and cut, at most [`MAX_BODY_ROWS`], after `indent`.
    fn body(&mut self, text: &str, indent: &str, row: PrRow) {
        let lines: Vec<&str> = text.lines().take(MAX_BODY_ROWS + 1).collect();
        for line in lines.iter().take(MAX_BODY_ROWS) {
            self.part(indent, None);
            self.part(&printable(line.trim_end_matches('\r'), MAX_ROW), None);
            self.end(row);
        }
        if lines.len() > MAX_BODY_ROWS {
            let more = text.lines().count() - MAX_BODY_ROWS;
            self.part(
                &format!("{indent}and {more} more lines"),
                Some(Highlight::GitDetail),
            );
            self.end(row);
        }
    }

    fn laid(self, pull: Option<(u64, String)>, checks_line: usize) -> PrLaid {
        let (text, highlights) = self.out.finish();
        PrLaid {
            text,
            highlights,
            built: Arc::new(PrBuilt {
                rows: self.rows,
                threads: self.threads,
                links: self.links,
                check_names: self.check_names,
                pulls: self.pulls,
                pull,
                checks_line,
            }),
        }
    }
}

mod issue;

pub(crate) use issue::{issue_list_text, issue_text};

#[cfg(test)]
mod tests;
