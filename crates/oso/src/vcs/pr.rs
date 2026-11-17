//! Pull requests as `gh` gives them (ADR 0127, step 5a): what its JSON answers say of a branch's
//! pull request, one pull request with its checks, review threads and conversation, and a
//! repository's open ones, read by shape into plain values.
//!
//! Must not depend on: `app`, `dispatch`, processes or the file system; these are pure
//! functions over the bytes `gh` wrote, which its thread reads ([`work`]) and the text is made
//! from ([`write`]).
//!
//! - What GitHub says is anyone's words (Q13): a title, a body, a comment, a path or a check's
//!   name is kept as text and only escaped where it is shown. Nothing here acts on it.
//! - Every answer is untrusted: an entry missing a field, or of another shape, is left out, an
//!   answer of the wrong shape reads as `None`, and every list has a bound.

pub(crate) mod work;
pub(crate) mod write;

use serde_json::Value;

use crate::git::GhRepo;

/// The most checks, comments or listed pull requests kept of one answer.
pub(crate) const MAX_ITEMS: usize = 500;
/// The most review threads asked for, and the most comments of each.
pub(crate) const MAX_THREADS: usize = 100;
pub(crate) const MAX_THREAD_COMMENTS: usize = 50;
/// The most bytes of one title, body, name or path kept.
const MAX_TEXT: usize = 64 * 1024;

/// The fields of `gh pr view --json` the status's head line reads.
pub(crate) const HEAD_FIELDS: &str =
    "number,title,state,isDraft,reviewDecision,statusCheckRollup,url,headRefOid,headRefName";
/// The fields of `gh pr view --json` a pull request's buffer reads.
pub(crate) const VIEW_FIELDS: &str = "number,title,state,isDraft,reviewDecision,\
statusCheckRollup,url,headRefOid,author,baseRefName,headRefName,body,comments";
/// The fields of `gh pr checks --json`.
pub(crate) const CHECK_FIELDS: &str = "name,state,bucket,link,workflow,startedAt,completedAt";
/// The fields of `gh pr list --json`.
pub(crate) const LIST_FIELDS: &str =
    "number,title,headRefName,author,isDraft,reviewDecision,statusCheckRollup,url";

/// The review threads of a pull request, one fixed query, its owner, name and number given as
/// variables, never written into it.
pub(crate) const THREADS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100) {
        totalCount
        nodes {
          isResolved
          path
          line
          originalLine
          comments(first: 50) {
            totalCount
            nodes { author { login } authorAssociation body diffHunk }
          }
        }
      }
    }
  }
}";

/// Where a pull request is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Open,
    Draft,
    Merged,
    Closed,
}

impl State {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Draft => "draft",
            Self::Merged => "merged",
            Self::Closed => "closed",
        }
    }
}

/// What the reviews decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Review {
    Approved,
    ChangesRequested,
    Required,
}

impl Review {
    pub(crate) fn words(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes requested",
            Self::Required => "review required",
        }
    }
}

/// How many checks passed, failed and are still going.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) pass: usize,
    pub(crate) fail: usize,
    pub(crate) pending: usize,
}

/// A branch's pull request, as the status's head line shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrHead {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) state: State,
    pub(crate) review: Option<Review>,
    pub(crate) counts: Counts,
    pub(crate) url: String,
    /// The pull request's head commit, whole.
    pub(crate) head: String,
    /// Its head branch, which must be the branch it was asked for.
    pub(crate) branch: String,
}

/// A check's outcome, as `gh pr checks` buckets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bucket {
    Pass,
    Fail,
    Pending,
    Skipping,
    Cancel,
}

/// One check of a pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Check {
    pub(crate) name: String,
    pub(crate) bucket: Bucket,
    pub(crate) workflow: String,
    pub(crate) link: String,
    /// How long it ran, in seconds, once it ended.
    pub(crate) took: Option<u64>,
}

/// A comment, in a review thread or the conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Comment {
    pub(crate) author: String,
    /// `OWNER`, `MEMBER`, `COLLABORATOR`, `CONTRIBUTOR`, `NONE`…, as GitHub says.
    pub(crate) association: String,
    pub(crate) body: String,
    /// The lines of the diff a review comment is on.
    pub(crate) hunk: String,
}

/// A review thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Thread {
    pub(crate) path: String,
    /// The line on the new side, or the original one if the thread is outdated.
    pub(crate) line: Option<u64>,
    pub(crate) resolved: bool,
    pub(crate) comments: Vec<Comment>,
    /// How many comments it has past those read.
    pub(crate) more: usize,
}

/// One pull request, as its buffer shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrView {
    pub(crate) head: PrHead,
    pub(crate) author: String,
    pub(crate) base: String,
    pub(crate) branch: String,
    pub(crate) body: String,
    pub(crate) comments: Vec<Comment>,
    pub(crate) checks: Vec<Check>,
    pub(crate) threads: Vec<Thread>,
    /// How many threads it has past those read.
    pub(crate) more_threads: usize,
}

/// A pull request in a repository's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Listed {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) branch: String,
    pub(crate) author: String,
    pub(crate) draft: bool,
    pub(crate) review: Option<Review>,
    pub(crate) counts: Counts,
    pub(crate) url: String,
}

/// The branch's pull request in `gh pr view --json` [`HEAD_FIELDS`]'s answer.
pub(crate) fn read_head(json: &[u8]) -> Option<PrHead> {
    head_of(&serde_json::from_slice::<Value>(json).ok()?)
}

/// A pull request in `gh pr view --json` [`VIEW_FIELDS`]'s answer, with no checks or threads.
pub(crate) fn read_view(json: &[u8]) -> Option<PrView> {
    let value = serde_json::from_slice::<Value>(json).ok()?;
    let comments = (value.get("comments")?.as_array()?.iter())
        .take(MAX_ITEMS)
        .filter_map(|comment| comment_of(comment, None))
        .collect();
    Some(PrView {
        head: head_of(&value)?,
        author: login(value.get("author")),
        base: text(value.get("baseRefName")?)?,
        branch: text(value.get("headRefName")?)?,
        body: value.get("body").and_then(text).unwrap_or_default(),
        comments,
        checks: Vec::new(),
        threads: Vec::new(),
        more_threads: 0,
    })
}

/// The checks in `gh pr checks --json` [`CHECK_FIELDS`]'s answer.
pub(crate) fn read_checks(json: &[u8]) -> Option<Vec<Check>> {
    let Value::Array(items) = serde_json::from_slice::<Value>(json).ok()? else {
        return None;
    };
    let check = |item: &Value| {
        let bucket = match item.get("bucket")?.as_str()? {
            "pass" => Bucket::Pass,
            "fail" => Bucket::Fail,
            "pending" => Bucket::Pending,
            "skipping" => Bucket::Skipping,
            "cancel" => Bucket::Cancel,
            _ => return None,
        };
        let at = |field: &str| item.get(field).and_then(Value::as_str).and_then(seconds);
        let took = at("completedAt")
            .zip(at("startedAt"))
            .and_then(|(end, start)| end.checked_sub(start));
        Some(Check {
            name: text(item.get("name")?)?,
            bucket,
            workflow: item.get("workflow").and_then(text).unwrap_or_default(),
            link: item.get("link").and_then(text).unwrap_or_default(),
            took,
        })
    };
    Some(items.iter().take(MAX_ITEMS).filter_map(check).collect())
}

/// The review threads, unresolved first, and how many there are past them, in the answer to
/// [`THREADS_QUERY`].
pub(crate) fn read_threads(json: &[u8]) -> Option<(Vec<Thread>, usize)> {
    let value = serde_json::from_slice::<Value>(json).ok()?;
    let threads = value
        .get("data")?
        .get("repository")?
        .get("pullRequest")?
        .get("reviewThreads")?;
    let nodes = threads.get("nodes")?.as_array()?;
    let total = count(threads.get("totalCount")).max(nodes.len());
    let thread = |node: &Value| {
        let comments = node.get("comments")?;
        let read: Vec<Comment> = (comments.get("nodes")?.as_array()?.iter())
            .take(MAX_THREAD_COMMENTS)
            .filter_map(|comment| comment_of(comment, comment.get("diffHunk")))
            .collect();
        let line = (node.get("line").and_then(Value::as_u64))
            .or_else(|| node.get("originalLine").and_then(Value::as_u64));
        Some(Thread {
            path: text(node.get("path")?)?,
            line,
            resolved: node.get("isResolved")?.as_bool()?,
            more: count(comments.get("totalCount")).saturating_sub(read.len()),
            comments: read,
        })
    };
    let mut read: Vec<Thread> = nodes.iter().take(MAX_THREADS).filter_map(thread).collect();
    read.sort_by_key(|thread| thread.resolved);
    let more = total.saturating_sub(read.len());
    Some((read, more))
}

/// The pull requests in `gh pr list --json` [`LIST_FIELDS`]'s answer.
pub(crate) fn read_list(json: &[u8]) -> Option<Vec<Listed>> {
    let Value::Array(items) = serde_json::from_slice::<Value>(json).ok()? else {
        return None;
    };
    let listed = |item: &Value| {
        Some(Listed {
            number: item.get("number")?.as_u64()?,
            title: text(item.get("title")?)?,
            branch: text(item.get("headRefName")?)?,
            author: login(item.get("author")),
            draft: item.get("isDraft").and_then(Value::as_bool) == Some(true),
            review: review_of(item.get("reviewDecision")),
            counts: counts_of(item.get("statusCheckRollup")),
            url: item.get("url").and_then(text).unwrap_or_default(),
        })
    };
    Some(items.iter().take(MAX_ITEMS).filter_map(listed).collect())
}

/// The address, number and head branch in `gh pr view --json number,url,headRefName`'s answer.
pub(crate) fn read_address(json: &[u8]) -> Option<(String, u64, String)> {
    let value = serde_json::from_slice::<Value>(json).ok()?;
    let url = text(value.get("url")?)?;
    Some((
        url,
        value.get("number")?.as_u64()?,
        text(value.get("headRefName")?)?,
    ))
}

/// Whether `path`, a review thread's file as GitHub names it, is a plain path under the root:
/// relative, of names only, none `.` or `..`.
pub(crate) fn is_plain_path(path: &str) -> bool {
    use std::path::{Component, Path};
    !path.is_empty()
        && !path.contains(['\\', ':'])
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

/// Whether `url` is the address of pull request `number` of `repo`, as GitHub writes it: the
/// only address `o` opens (Q5).
pub(crate) fn is_pull_address(url: &str, repo: &GhRepo, number: u64) -> bool {
    url.eq_ignore_ascii_case(&repo.pull_address(number))
}

/// Whether `url` is a plain `https` address, of the characters an address is written with but
/// no quote, space, comma or semicolon, at most 2 KiB: what `w` copies, as a link is anyone's
/// words.
pub(crate) fn is_plain_link(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let plain = |b: u8| b.is_ascii_alphanumeric() || b"-._~:/?#=%&".contains(&b);
    url.len() <= 2048
        && rest
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && url.bytes().all(plain)
}

fn head_of(value: &Value) -> Option<PrHead> {
    let state = match (value.get("state")?.as_str()?, value.get("isDraft")) {
        ("OPEN", Some(Value::Bool(true))) => State::Draft,
        ("OPEN", _) => State::Open,
        ("MERGED", _) => State::Merged,
        ("CLOSED", _) => State::Closed,
        _ => return None,
    };
    Some(PrHead {
        number: value.get("number")?.as_u64()?,
        title: text(value.get("title")?)?,
        state,
        review: review_of(value.get("reviewDecision")),
        counts: counts_of(value.get("statusCheckRollup")),
        url: value.get("url").and_then(text).unwrap_or_default(),
        head: value.get("headRefOid").and_then(text).unwrap_or_default(),
        branch: value.get("headRefName").and_then(text).unwrap_or_default(),
    })
}

fn review_of(value: Option<&Value>) -> Option<Review> {
    match value?.as_str()? {
        "APPROVED" => Some(Review::Approved),
        "CHANGES_REQUESTED" => Some(Review::ChangesRequested),
        "REVIEW_REQUIRED" => Some(Review::Required),
        _ => None,
    }
}

/// The counts of `statusCheckRollup`: check runs by their status and conclusion, and commit
/// statuses by their state.
fn counts_of(value: Option<&Value>) -> Counts {
    let mut counts = Counts::default();
    let Some(items) = value.and_then(Value::as_array) else {
        return counts;
    };
    for item in items.iter().take(MAX_ITEMS) {
        let field = |name: &str| item.get(name).and_then(Value::as_str);
        let outcome = match (field("status"), field("conclusion"), field("state")) {
            (Some(status), _, _) if status != "COMPLETED" => Some(Bucket::Pending),
            (Some(_), Some("SUCCESS" | "NEUTRAL" | "SKIPPED"), _) => Some(Bucket::Pass),
            (Some(_), Some(_), _) => Some(Bucket::Fail),
            (None, _, Some("SUCCESS")) => Some(Bucket::Pass),
            (None, _, Some("PENDING" | "EXPECTED")) => Some(Bucket::Pending),
            (None, _, Some(_)) => Some(Bucket::Fail),
            _ => None,
        };
        match outcome {
            Some(Bucket::Pass) => counts.pass += 1,
            Some(Bucket::Pending) => counts.pending += 1,
            Some(_) => counts.fail += 1,
            None => {}
        }
    }
    counts
}

fn comment_of(value: &Value, hunk: Option<&Value>) -> Option<Comment> {
    Some(Comment {
        author: login(value.get("author")),
        association: (value.get("authorAssociation").and_then(text)).unwrap_or_default(),
        body: text(value.get("body")?)?,
        hunk: hunk.and_then(text).unwrap_or_default(),
    })
}

/// A login, or `ghost` for an account that is gone, as GitHub shows it.
fn login(author: Option<&Value>) -> String {
    (author.and_then(|author| author.get("login")).and_then(text))
        .unwrap_or_else(|| "ghost".to_owned())
}

fn count(value: Option<&Value>) -> usize {
    value
        .and_then(Value::as_u64)
        .map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX))
}

/// A string's text, at most [`MAX_TEXT`] bytes of it, cut at a character.
fn text(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    let mut end = text.len().min(MAX_TEXT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(text[..end].to_owned())
}

/// The seconds since 1970 of an RFC 3339 time in UTC, `YYYY-MM-DDTHH:MM:SSZ`, as GitHub writes
/// them; `None` for any other.
fn seconds(at: &str) -> Option<u64> {
    let b = at.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[19] != b'Z' {
        return None;
    }
    let n = |from: usize, to: usize| -> Option<u64> { at.get(from..to)?.parse().ok() };
    let (year, month, day) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (hour, minute, second) = (n(11, 13)?, n(14, 16)?, n(17, 19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    // Days from the civil date, as Howard Hinnant's algorithm counts them.
    let (y, m) = if month <= 2 {
        (year.checked_sub(1)?, month + 9)
    } else {
        (year, month - 3)
    };
    let era = y / 400;
    let of_era = y % 400;
    let of_year = (153 * m + 2) / 5 + day - 1;
    let of_era_days = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    let days = (era * 146_097 + of_era_days).checked_sub(719_468)?;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second.min(60))
}

#[cfg(test)]
mod tests;
