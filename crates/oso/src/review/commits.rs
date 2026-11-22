//! A pull request's commits and a commit's message, as a review reads them (ADR 0167): the
//! commits from where the pull request's head left its base to its head, a row each for its
//! commit list, and the message a commit's review shows under its title.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs git on the review's
//! own thread, after a pull request's sides resolved and any fetch ended, so that it asks no
//! `gh` and reaches no network.
//!
//! - The commits are `git log --topo-order` of `BASE..TIP`, newest first, so that a listing cut
//!   at [`MAX_LIST_BYTES`] loses the oldest, never the head; the newest [`MAX_COMMITS`] are
//!   kept, oldest first. Each name and subject is kept to [`MAX_WORDS`]
//!   characters, and a line whose short id is not hex or whose date is not `YYYY-MM-DD` is
//!   left out. Past any of these the list says it left some out. `TIP` is the head, or `HEAD` where the head branch
//!   is checked out and the work tree shows, so that commits not pushed yet are listed too,
//!   marked as such.
//! - A merge has more than one parent; a commit changes nothing when its tree is its parent's,
//!   as far as the parents listed and the base say. Both are listed and stepped over.
//! - A commit's message is `git log -1` of its id, at most [`MAX_MESSAGE_BYTES`], cut to
//!   [`MAX_MESSAGE_LINES`] lines, saying so.
//! - Every word is anyone's: subjects, names and the message are escaped on one line each as
//!   git's text is shown (`printable`), and nothing here logs them. No run shows a signature,
//!   which would run gpg, nor notes.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;

use super::Report;
use super::scope::{New, ScopeError, Sides};
use crate::git::{Git, GitError, Id, printable};

/// The most commits listed, the newest.
pub(crate) const MAX_COMMITS: usize = 1_000;
/// The most bytes of `git log` output a listing reads.
const MAX_LIST_BYTES: usize = 1024 * 1024;
/// The most bytes of a commit's message read.
pub(crate) const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// The most lines of a commit's message shown.
pub(crate) const MAX_MESSAGE_LINES: usize = 100;
/// The most characters of a subject, a name or a line of a message kept.
const MAX_WORDS: usize = 1024;
/// What separates a listing's fields; no field before the subject, the last, holds it.
const FIELD: u8 = 0;

/// Lists the commits of a pull request whose sides `sides` resolved, its head `head`, and
/// reports them: to `HEAD` when the head branch is `checked_out` here and the work tree shows.
/// A listing that fails is logged and leaves the review with no list; a stop, or the host gone,
/// is [`ScopeError::Stopped`].
pub(super) fn report_listing(
    git: &Git,
    sides: &Sides,
    head: &Id,
    checked_out: bool,
    stop: &dyn Fn() -> bool,
    out: &mut dyn FnMut(Report) -> bool,
) -> Result<(), ScopeError> {
    let Some(base) = &sides.old else {
        return Ok(());
    };
    let tip = match checked_out {
        true => git
            .resolve("HEAD", "commit")?
            .unwrap_or_else(|| head.clone()),
        false => head.clone(),
    };
    match list(git, base, &tip, head, stop) {
        Ok(listed) => match out(Report::Commits(Box::new(listed))) {
            true => Ok(()),
            false => Err(ScopeError::Stopped),
        },
        Err(ScopeError::Stopped) => Err(ScopeError::Stopped),
        Err(e) => {
            log::warn!(
                "a pull request's commits could not be listed: {}",
                super::work::said(&e)
            );
            Ok(())
        }
    }
}

/// Reports the message of the commit a review of one commit has as its new side, once its id
/// is known or moved from `last`, the one reported last; `false` once the host has gone. A
/// message that cannot be read is left out, the review going on.
pub(super) fn report_message(
    git: &Git,
    sides: &Sides,
    last: &mut Option<Id>,
    out: &mut dyn FnMut(Report) -> bool,
) -> bool {
    let New::Tree(id) = &sides.new else {
        return true;
    };
    if last.as_ref() == Some(id) {
        return true;
    }
    match message(git, id) {
        Ok(read) => {
            *last = Some(id.clone());
            out(Report::Message(std::sync::Arc::new(read)))
        }
        Err(e) => {
            log::debug!(
                "a commit's message could not be read: {}",
                super::work::said(&e)
            );
            true
        }
    }
}

/// A pull request's commits, oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Listed {
    pub(crate) commits: Vec<Listing>,
    /// Whether commits were left out: more than [`MAX_COMMITS`], or more output than read.
    pub(crate) more: bool,
}

/// One commit of a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Listing {
    pub(crate) id: Id,
    pub(crate) short: String,
    pub(crate) subject: String,
    pub(crate) author: String,
    /// Its author's date, `YYYY-MM-DD`.
    pub(crate) date: String,
    pub(crate) merge: bool,
    pub(crate) empty: bool,
    /// Whether the pull request's head does not hold it.
    pub(crate) unpushed: bool,
}

/// A commit's message, as its review shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Message {
    pub(crate) short: String,
    pub(crate) author: String,
    pub(crate) date: String,
    /// Its lines, the subject first, each escaped.
    pub(crate) lines: Vec<String>,
    /// How many lines past [`MAX_MESSAGE_LINES`] were left out, and whether its bytes were cut
    /// at [`MAX_MESSAGE_BYTES`].
    pub(crate) left_out: usize,
    pub(crate) cut: bool,
}

/// The commits from `base`, where the pull request's head left its base, to `tip`, its head or
/// `HEAD`; those `head` does not hold marked unpushed. It runs git no more once `stop` says so.
pub(crate) fn list(
    git: &Git,
    base: &Id,
    tip: &Id,
    head: &Id,
    stop: &dyn Fn() -> bool,
) -> Result<Listed, ScopeError> {
    let range = format!("{}..{}", base.as_str(), tip.as_str());
    let max = format!("--max-count={}", MAX_COMMITS + 1);
    let args = [
        "log",
        "--no-color",
        "--no-show-signature",
        "--no-notes",
        "--topo-order",
        &max,
        "--format=%H%x00%T%x00%P%x00%h%x00%an%x00%as%x00%s",
        "--end-of-options",
        &range,
        "--",
    ]
    .map(OsStr::new);
    let output = git.output(&args, &[], MAX_LIST_BYTES)?;
    if !output.status.success() && !output.cut {
        return Err(GitError::Failed {
            status: output.status.code(),
            stderr: output.stderr,
        }
        .into());
    }
    if stop() {
        return Err(ScopeError::Stopped);
    }
    let mut listed = read_listing(&output.stdout, output.cut);
    // A tree the base has, for its child's: one run.
    let base_tree = git.resolve(base.as_str(), "tree")?;
    let trees: HashMap<&str, &str> = listed
        .trees
        .iter()
        .map(|(id, tree)| (id.as_str(), tree.as_str()))
        .chain(
            base_tree
                .as_ref()
                .map(|tree| (base.as_str(), tree.as_str())),
        )
        .collect();
    for (commit, (_, tree)) in listed.listed.commits.iter_mut().zip(&listed.trees) {
        let parents = listed.parents.get(commit.id.as_str());
        commit.empty = !commit.merge
            && parents
                .and_then(|parents| parents.first())
                .and_then(|parent| trees.get(parent.as_str()))
                .is_some_and(|parent_tree| *parent_tree == tree.as_str());
    }
    if tip != head && !listed.listed.commits.is_empty() {
        if stop() {
            return Err(ScopeError::Stopped);
        }
        let unpushed = unpushed(git, head, tip)?;
        for commit in &mut listed.listed.commits {
            commit.unpushed = unpushed.contains(commit.id.as_str());
        }
    }
    Ok(listed.listed)
}

/// A listing read, with each commit's tree and parents for the caller to judge.
struct Read {
    listed: Listed,
    trees: Vec<(Id, Id)>,
    parents: HashMap<String, Vec<Id>>,
}

/// The commits in `git log`'s output, a line each, newest first, as the listing gives them
/// oldest first: those past [`MAX_COMMITS`], the oldest, and a line `cut` short are left out,
/// saying so, and a line not as asked is left out.
fn read_listing(out: &[u8], cut: bool) -> Read {
    let mut lines: Vec<&[u8]> = out.split(|b| *b == b'\n').collect();
    // The last is empty, or cut short.
    lines.pop();
    let mut read = Read {
        listed: Listed {
            commits: Vec::new(),
            more: cut || lines.len() > MAX_COMMITS,
        },
        trees: Vec::new(),
        parents: HashMap::new(),
    };
    lines.truncate(MAX_COMMITS);
    lines.reverse();
    for line in lines {
        let mut fields = line.splitn(7, |b| *b == FIELD);
        let (Some(id), Some(tree), Some(parents), Some(short), Some(author), Some(date)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        let subject = fields.next().unwrap_or_default();
        let (Some(id), Some(tree)) = (Id::parse(id), Id::parse(tree)) else {
            continue;
        };
        // Fields that are oso's to show as they are: a short id of hex digits, a date.
        let hex = !short.is_empty() && short.len() <= 64 && short.iter().all(u8::is_ascii_hexdigit);
        let dated = date.len() == 10
            && date.iter().enumerate().all(|(at, b)| {
                if at == 4 || at == 7 {
                    *b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            });
        if !hex || !dated {
            read.listed.more = true;
            continue;
        }
        let parents: Vec<Id> = parents
            .split(|b| *b == b' ')
            .filter_map(Id::parse)
            .collect();
        let words =
            |bytes: &[u8]| one_spaced(&printable(&String::from_utf8_lossy(bytes), MAX_WORDS));
        read.listed.commits.push(Listing {
            id: id.clone(),
            short: words(short),
            subject: words(subject),
            author: words(author),
            date: words(date),
            merge: parents.len() > 1,
            empty: false,
            unpushed: false,
        });
        read.trees.push((id.clone(), tree));
        read.parents.insert(id.as_str().to_owned(), parents);
    }
    read
}

/// `text` with each run of white space one space, its ends trimmed, so that no padding of
/// anyone's words pushes oso's own off a row.
fn one_spaced(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The commits `tip` holds that `head` does not, at most [`MAX_COMMITS`] and one.
fn unpushed(git: &Git, head: &Id, tip: &Id) -> Result<HashSet<String>, ScopeError> {
    let range = format!("{}..{}", head.as_str(), tip.as_str());
    let max = format!("--max-count={}", MAX_COMMITS + 1);
    let args = ["rev-list", &max, "--end-of-options", &range, "--"].map(OsStr::new);
    // 41 bytes an id at most, a few more for a longer hash.
    let out = git.run(&args, &[], (MAX_COMMITS + 1) * 72)?;
    Ok(out
        .split(|b| *b == b'\n')
        .filter_map(Id::parse)
        .map(|id| id.as_str().to_owned())
        .collect())
}

/// The message of the commit `id`, as its review shows it.
pub(crate) fn message(git: &Git, id: &Id) -> Result<Message, ScopeError> {
    let args = [
        "log",
        "-1",
        "--no-color",
        "--no-show-signature",
        "--no-notes",
        "--format=%h%x00%an%x00%as%x00%B",
        "--end-of-options",
        id.as_str(),
        "--",
    ]
    .map(OsStr::new);
    let output = git.output(&args, &[], MAX_MESSAGE_BYTES)?;
    if !output.status.success() && !output.cut {
        return Err(GitError::Failed {
            status: output.status.code(),
            stderr: output.stderr,
        }
        .into());
    }
    read_message(&output.stdout, output.cut)
        .ok_or_else(|| GitError::Unexpected("not a commit's message").into())
}

/// A commit's message in `git log`'s output, `cut` short or not.
fn read_message(out: &[u8], cut: bool) -> Option<Message> {
    let mut fields = out.splitn(4, |b| *b == FIELD);
    let (short, author, date) = (fields.next()?, fields.next()?, fields.next()?);
    let body = fields.next().unwrap_or_default();
    let words = |bytes: &[u8]| printable(&String::from_utf8_lossy(bytes), MAX_WORDS);
    let text = String::from_utf8_lossy(body);
    let text = text.trim_end();
    let mut lines = Vec::new();
    let mut left_out = 0;
    for line in text.lines() {
        if lines.len() == MAX_MESSAGE_LINES {
            left_out += 1;
            continue;
        }
        lines.push(printable(line, MAX_WORDS));
    }
    Some(Message {
        short: words(short),
        author: words(author),
        date: words(date),
        lines,
        left_out,
        cut,
    })
}

#[cfg(test)]
#[path = "commits_tests.rs"]
mod tests;
