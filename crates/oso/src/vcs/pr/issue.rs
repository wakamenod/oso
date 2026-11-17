//! Issues as `gh` gives them (ADR 0127, step 5b-1): a repository's open issues and one with its
//! comments, read by shape, the branch a worktree session from one gets, the only address `o`
//! opens, and the brief the session starts from.
//!
//! Must not depend on: `app`, `dispatch`, processes or the file system; pure functions over
//! what `gh` wrote.
//!
//! - What an issue says is anyone's words (Q13). The brief puts it under the line that tells the
//!   agent so, escapes every line as the display would box it, keeps only the comments of the
//!   issue's author, the person and the repository's owners, members and collaborators, counts
//!   the others, and is cut at [`MAX_BRIEF`]. The person reads it in `*Brief*` before it goes.

use serde_json::Value;

pub(crate) use super::UNTRUSTED;
use super::{Comment, END, MAX_ITEMS, comment_of, is_login, login, text, trusted};
use crate::git::{GhRepo, printable};

/// The fields of `gh issue list --json`.
pub(crate) const LIST_FIELDS: &str = "number,title,labels,author,updatedAt,url";
/// The fields of `gh issue view --json`.
pub(crate) const VIEW_FIELDS: &str = "number,title,body,comments,labels,state,author,url";
/// The most bytes of a brief made from an issue.
pub(crate) const MAX_BRIEF: usize = 64 * 1024;
/// The most characters of a branch name's slug.
const MAX_SLUG: usize = 40;
/// What a cut brief says where it was cut.
const CUT: &str = "> [the rest is cut]\n";
/// The room a brief leaves for the header a session's first prompt adds (ADR 0121).
const HEADER_ROOM: usize = 4 * 1024;

/// An issue in a repository's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IssueListed {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) labels: Vec<String>,
    pub(crate) author: String,
    /// When it was last changed, as GitHub writes it.
    pub(crate) updated: String,
    pub(crate) url: String,
}

/// One issue, with its comments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Issue {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) state: String,
    pub(crate) author: String,
    pub(crate) labels: Vec<String>,
    pub(crate) url: String,
    pub(crate) comments: Vec<Comment>,
}

/// The issues in `gh issue list --json` [`LIST_FIELDS`]'s answer.
pub(crate) fn read_issue_list(json: &[u8]) -> Option<Vec<IssueListed>> {
    let Value::Array(items) = serde_json::from_slice::<Value>(json).ok()? else {
        return None;
    };
    let listed = |item: &Value| {
        Some(IssueListed {
            number: item.get("number")?.as_u64()?,
            title: text(item.get("title")?)?,
            labels: labels(item.get("labels")),
            author: login(item.get("author")),
            updated: item.get("updatedAt").and_then(text).unwrap_or_default(),
            url: item.get("url").and_then(text).unwrap_or_default(),
        })
    };
    Some(items.iter().take(MAX_ITEMS).filter_map(listed).collect())
}

/// The issue in `gh issue view --json` [`VIEW_FIELDS`]'s answer.
pub(crate) fn read_issue(json: &[u8]) -> Option<Issue> {
    let value = serde_json::from_slice::<Value>(json).ok()?;
    let comments = (value.get("comments")?.as_array()?.iter())
        .take(MAX_ITEMS)
        .filter_map(|comment| comment_of(comment, None))
        .collect();
    Some(Issue {
        number: value.get("number")?.as_u64()?,
        title: text(value.get("title")?)?,
        body: value.get("body").and_then(text).unwrap_or_default(),
        state: value.get("state").and_then(text).unwrap_or_default(),
        author: login(value.get("author")),
        labels: labels(value.get("labels")),
        url: value.get("url").and_then(text).unwrap_or_default(),
        comments,
    })
}

/// Whether `url` is the address of issue `number` of `repo`: the only address `o` opens.
pub(crate) fn is_issue_address(url: &str, repo: &GhRepo, number: u64) -> bool {
    url.eq_ignore_ascii_case(&repo.issue_address(number))
}

/// The branch a worktree session from issue `number` titled `title` gets: `issue-N-SLUG`, the
/// slug the title's letters and digits in lower case, joined by `-`.
pub(crate) fn branch_name(number: u64, title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= MAX_SLUG {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        format!("issue-{number}")
    } else {
        format!("issue-{number}-{slug}")
    }
}

/// The brief a worktree session from `issue` starts from: what it says under [`UNTRUSTED`], each
/// of its lines quoted with `> ` so that none can pass for oso's or the person's, with the
/// comments of its author, of `person`, the login `gh` is logged in as, and of the repository's
/// owners, members and collaborators; how many others are left out comes first, and the end of
/// what GitHub said is marked. A login that is empty or a deleted account's matches no one.
pub(crate) fn brief_text(issue: &Issue, person: &str) -> String {
    let trusted =
        |comment: &&Comment| trusted(comment, person) || is_login(&comment.author, &issue.author);
    let kept: Vec<&Comment> = issue.comments.iter().filter(trusted).collect();
    let left_out = issue.comments.len() - kept.len();
    let mut out = format!("{UNTRUSTED}\n\n");
    let title = printable(&issue.title, 300);
    out.push_str(&format!(
        "Issue #{} on GitHub, quoted: {title}\n",
        issue.number
    ));
    if left_out > 0 {
        out.push_str(&format!(
            "{left_out} comments by others than the issue's author, you and the repository's \
             members are left out.\n"
        ));
    }
    out.push('\n');
    let room = MAX_BRIEF - END.len() - CUT.len() - HEADER_ROOM;
    let mut cut = !push_quoted(&mut out, &issue.body, room);
    for comment in kept {
        if cut {
            break;
        }
        let author = printable(&comment.author, 40);
        let association = printable(&comment.association, 20);
        out.push_str(&format!(
            "\nComment by @{author} ({association}), quoted:\n"
        ));
        cut = !push_quoted(&mut out, &comment.body, room);
    }
    if cut {
        out.push_str(CUT);
    }
    out.push_str(END);
    out
}

/// The login in `gh api user`'s answer, if it names one.
pub(crate) fn read_login(json: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(json).ok()?;
    text(value.get("login")?).filter(|login| !login.is_empty())
}

/// Adds `text`'s lines to `out`, each quoted and escaped as the display would box it, while
/// `out` stays within `room` bytes; whether all of them went in.
fn push_quoted(out: &mut String, text: &str, room: usize) -> bool {
    for line in text.lines() {
        let line = printable(line.trim_end_matches('\r'), 2000);
        if out.len() + line.len() + 3 > room {
            return false;
        }
        out.push_str("> ");
        out.push_str(&line);
        out.push('\n');
    }
    true
}

fn labels(value: Option<&Value>) -> Vec<String> {
    (value.and_then(Value::as_array).into_iter().flatten())
        .take(20)
        .filter_map(|label| text(label.get("name")?))
        .collect()
}

#[cfg(test)]
mod tests;
