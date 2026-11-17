//! Sending a review's comments to an agent session (ADR 0122, step 12): the person's comments
//! that were not sent gathered into `*review message*`, which the person reads and may change,
//! then pasted through the one delivery path, and the comments it held marked sent. A pull
//! request's checks and review threads go through the same message (ADR 0127, step 6b,
//! [`pull`]): it holds one message at a time, of either [`Origin`].
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or reads a
//! file: the message is composed from the model's comments and readings, bounded at 256 KiB,
//! and the paste is `send_to_session`'s.
//!
//! - The session is the one the person picked for the review with `C-u C-c C-c`, while it
//!   runs, else, for a pull request, the hosted session in the worktree its head branch is
//!   checked out in (step 12b, Q11), else the one that runs in the review's root, the one shown
//!   last first.
//!   With none, the message stays and says how to pick or start one: nothing goes to the kill
//!   ring, as the message may hold an agent's words (ADR 0074, Q-G).
//! - A paste refused, as when the session asks for permission, keeps the message (Q6).
//! - The comments marked sent are those the message took that still say what they said when it
//!   was made, so that a comment changed or removed in the review meanwhile stays unsent (Q8),
//!   whatever the person changed in the message.
//! - A file whose name may hold secrets, or whose open buffer does, sends its place alone, and
//!   a message turned secret, as a yank of a secret entry turns it, is not sent.
//! - Which comments go, and in what order, is decided here, from the model and the rows the
//!   review buffer shows, as the order is the host's text's.
//! - One message at a time: `C-c C-c` in a review while one is open shows it.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::editor::ReviewId;
use oso_core::project::ProjectId;
use oso_core::review::{Author, Comment, CommentId, Review, RowKind, Rows, Seek};

use super::super::App;
use super::scope_label;
use crate::commands::agent_sessions::{MAX_SEND_BYTES, too_large};
use crate::commands::review::{MAX_NAME, one_line};
use crate::review::message::{Item, compose};
use crate::review::pr::PrFound;
use crate::sessions::worker::SessionKey;

/// What the buffer of a review's message is called.
const NAME: &str = "*review message*";

/// A message waiting in its buffer to be sent, what it was made from, and the comments it took.
pub(in crate::app) struct Waiting {
    buffer: BufferId,
    origin: Origin,
    taken: Vec<Comment>,
}

/// What a message was made from: a review's comments, or a pull request's checks or review
/// threads (ADR 0127, step 6b).
#[derive(Clone, Debug)]
enum Origin {
    Review(ReviewId),
    Pull(pull::FromPull),
}

/// What sending keeps: the message open, and the session the person picked for each review and
/// each pull request.
#[derive(Default)]
pub(in crate::app) struct Sending {
    message: Option<Waiting>,
    picked: HashMap<ReviewId, SessionKey>,
    picked_pulls: HashMap<(ProjectId, u64), SessionKey>,
}

impl App {
    /// `review-send`: gathers the person's comments of `review` not sent into a message, shown
    /// in its buffer.
    pub(super) fn send_review(&mut self, review: ReviewId) {
        if self.show_waiting_message() {
            return;
        }
        let Some(index) = self.reviews.index(review) else {
            return;
        };
        let (project, scope) = {
            let running = &self.reviews.open[index];
            (running.project, scope_label(&running.scope))
        };
        let Some(found) = self.editor.project(project) else {
            return;
        };
        let root = found.root().to_path_buf();
        let scope = format!("{scope}, in {}", one_line(found.name(), MAX_NAME));
        let (Some(model), rows) = (self.editor.review(review), self.editor.review_rows(review))
        else {
            return;
        };
        let ids = unsent(model, rows);
        if ids.is_empty() {
            return self.editor.message("No comment of yours waits to be sent");
        }
        let items: Vec<Item<'_>> = (ids.iter())
            .filter_map(|id| model.comment(*id))
            .map(|comment| Item {
                comment,
                parent: comment.reply_to().and_then(|to| model.comment(to)),
                shape: model.file(comment.path()).map(|shape| &**shape),
            })
            .collect();
        // A file may hold secrets by its name, or as an open buffer of it does (`secrets`).
        let held = super::secrets::held_paths(&self.editor);
        let composed = compose(&scope, &items, MAX_SEND_BYTES, |path| {
            super::secrets::withheld(&root, &held, path)
        });
        if composed.taken == 0 {
            return self
                .editor
                .message("Your first comment is too long to send");
        }
        let taken: Vec<Comment> = (items[..composed.taken].iter())
            .map(|item| item.comment.clone())
            .collect();
        let left = items.len() - composed.taken;
        let name = self.unique_name(NAME);
        let mut buffer = Buffer::from_text(name, &composed.text);
        buffer.set_mode(Mode::ReviewMessage);
        let buffer = self.editor.add_buffer(buffer);
        self.editor.show(buffer);
        let count = taken.len();
        let hidden = self.hidden_comments(review, &taken, |path| {
            super::secrets::withheld(&root, &held, path)
        });
        self.reviews.sending.message = Some(Waiting {
            buffer,
            origin: Origin::Review(review),
            taken,
        });
        let to = match self.review_session(review) {
            Some(key) => format!("to {}", self.review_session_name(key)),
            None => "to no session yet".to_owned(),
        };
        let more = match left {
            0 => String::new(),
            n => format!("; {n} more wait for the next, past 256 KiB"),
        } + &hidden;
        self.editor.message(format!(
            "{} {to}{more}. C-c C-c pastes it, C-u C-c C-c picks the session, C-c C-k drops it",
            comments(count)
        ));
    }

    /// What the echo says of `taken`'s comments on files the filter hides (step 6b), judged
    /// from the filter the editor holds, as the view judges them, not from the text it built
    /// last; a file `withheld` says may hold secrets by its path alone. Nothing while none is.
    /// O(the review's comments).
    fn hidden_comments(
        &self,
        review: ReviewId,
        taken: &[Comment],
        withheld: impl Fn(&[u8]) -> bool,
    ) -> String {
        let (Some(filter), Some(model)) = (
            self.editor.review_filter(review),
            self.editor.review(review),
        ) else {
            return String::new();
        };
        let mut texts: HashMap<&[u8], Vec<&str>> = (taken.iter())
            .map(|comment| (comment.path(), Vec::new()))
            .collect();
        for comment in model.comments() {
            if let Some(texts) = texts.get_mut(comment.path()) {
                texts.push(comment.text());
            }
        }
        let hidden: HashSet<&[u8]> = (texts.iter())
            .filter(|(path, texts)| {
                let texts: &[&str] = if withheld(path) { &[] } else { texts };
                !filter.matches(path, texts)
            })
            .map(|(path, _)| *path)
            .collect();
        let count = (taken.iter())
            .filter(|comment| hidden.contains(comment.path()))
            .count();
        match count {
            0 => String::new(),
            count => format!("; {count} on files the filter hides"),
        }
    }

    /// Whether a message waits to be sent, as one at a time does: it is shown, and said so.
    pub(in crate::app) fn show_waiting_message(&mut self) -> bool {
        let Some(buffer) = (self.reviews.sending.message.as_ref())
            .map(|message| message.buffer)
            .filter(|buffer| self.editor.buffer(*buffer).is_some())
        else {
            return false;
        };
        self.editor.show(buffer);
        self.editor
            .message("A message waits: C-c C-c there sends it, C-c C-k drops it");
        true
    }

    /// `review-message-send`: pastes the message `buffer` holds into its session, or into the
    /// one named `to`, and marks the comments a review's message took sent.
    pub(super) fn send_review_message(&mut self, buffer: BufferId, to: Option<String>) {
        let Some(origin) = (self.reviews.sending.message.as_ref())
            .filter(|message| message.buffer == buffer)
            .map(|message| message.origin.clone())
        else {
            return self
                .editor
                .message("This is no review's message: C-c C-c in a review makes one");
        };
        let Some(text) = self.message_to_send(buffer, &origin) else {
            return;
        };
        let key = match &to {
            Some(name) => match self.session_named(name) {
                Some(key) => key,
                None => return self.editor.message(format!("No session {name}")),
            },
            None => match self.message_session(&origin) {
                Some(key) => key,
                None => {
                    let start = self.dispatcher.where_is(&self.editor, "claude");
                    let place = match origin {
                        Origin::Review(_) => "in this worktree",
                        Origin::Pull(_) => "where the pull request's branch is checked out",
                    };
                    return self.editor.message(format!(
                        "No agent session runs {place}: C-u C-c C-c picks one, or {start} starts \
                         one"
                    ));
                }
            },
        };
        let name = self.review_session_name(key);
        if let Err(why) = self.send_to_session(key, text) {
            return self.editor.message(why);
        }
        let Some(message) = self.reviews.sending.message.take() else {
            return;
        };
        self.editor.kill_buffer(buffer);
        let review = match origin {
            Origin::Review(review) => review,
            Origin::Pull(from) => {
                if to.is_some() {
                    let picked = &mut self.reviews.sending.picked_pulls;
                    picked.insert((from.project, from.number), key);
                }
                return self
                    .editor
                    .message(format!("Pasted into {name}: press Enter there"));
            }
        };
        if to.is_some() {
            self.reviews.sending.picked.insert(review, key);
        }
        let (marked, changed) = self.mark_sent(review, &message.taken);
        let changed = match changed {
            0 => String::new(),
            n => format!("; {} changed meanwhile stay unsent", comments(n)),
        };
        self.editor.message(format!(
            "Pasted into {name}: press Enter there. {} marked sent{changed}",
            comments(marked)
        ));
    }

    /// The text of the message `buffer` holds, if it may be sent; else why not is said.
    fn message_to_send(&mut self, buffer: BufferId, origin: &Origin) -> Option<String> {
        let shown = self.editor.buffer(buffer)?;
        // A yank of a secret entry turns the message secret: what it holds stays here, as a
        // draft's does (step 3).
        if shown.is_secret() {
            self.editor.message(
                "The message holds secret text, which is not sent: take it out, or C-c C-k",
            );
            return None;
        }
        // Its length is looked at before anything is copied.
        let bytes = shown.text().len();
        if bytes > MAX_SEND_BYTES {
            let too = match origin {
                Origin::Review(_) => {
                    let send = self
                        .dispatcher
                        .where_is(&self.editor, "review-message-send");
                    too_large(bytes, &send)
                }
                Origin::Pull(_) => format!(
                    "The message is {} KiB, past the 256 KiB a session takes: take some out, or \
                     C-c C-k",
                    bytes.div_ceil(1024)
                ),
            };
            self.editor.message(too);
            return None;
        }
        let text = shown.text().to_string();
        if text.trim().is_empty() {
            self.editor
                .message("The message is empty: C-c C-k drops it");
            return None;
        }
        Some(text)
    }

    /// `review-message-drop`: drops the message `buffer` holds, and shows what it was made
    /// from again.
    pub(super) fn drop_review_message(&mut self, buffer: BufferId) {
        let origin = (self.reviews.sending.message.as_ref())
            .filter(|message| message.buffer == buffer)
            .map(|message| message.origin.clone());
        if origin.is_some() {
            self.reviews.sending.message = None;
        }
        // A review buffer is internal, which a kill's next buffer leaves out.
        let from = match &origin {
            Some(Origin::Review(review)) => self.editor.review_buffer(*review),
            Some(Origin::Pull(from)) => self.pr_buffer_of(from.project, from.number),
            None => None,
        };
        if let Some(shown) = from
            && self.editor.window().buffer() == buffer
        {
            self.editor.show(shown);
        }
        self.editor.kill_buffer(buffer);
        let left = match origin {
            Some(Origin::Pull(_)) => "nothing was sent",
            _ => "its comments stay unsent",
        };
        self.editor
            .message(format!("The message was dropped: {left}"));
    }

    /// After each event: forgets a message whose buffer was killed, and a picked session that
    /// is gone, or whose review closed or pull request is no longer shown.
    pub(super) fn sync_review_message(&mut self) {
        let shown: HashSet<(ProjectId, u64)> = (self.reviews.sending.picked_pulls.keys())
            .filter(|(project, number)| self.pr_buffer_of(*project, *number).is_some())
            .copied()
            .collect();
        let editor = &self.editor;
        let sending = &mut self.reviews.sending;
        if (sending.message.as_ref()).is_some_and(|message| editor.buffer(message.buffer).is_none())
        {
            sending.message = None;
        }
        let table = self.hosting.table();
        let open = &self.reviews.open;
        sending.picked.retain(|review, key| {
            table.get(*key).is_some() && open.iter().any(|running| running.id == *review)
        });
        (sending.picked_pulls)
            .retain(|pull, key| table.get(*key).is_some() && shown.contains(pull));
    }

    /// The session a message from `origin` goes to, if none is named.
    fn message_session(&self, origin: &Origin) -> Option<SessionKey> {
        match origin {
            Origin::Review(review) => self.review_session(*review),
            Origin::Pull(from) => self.pull_session(from),
        }
    }

    /// The session a message of `review` goes to: the one the person picked for it while it
    /// runs, else the one in each of [`places`], the one shown last first.
    fn review_session(&self, review: ReviewId) -> Option<SessionKey> {
        let picked = (self.reviews.sending.picked.get(&review).copied())
            .filter(|key| self.hosting.table().get(*key).is_some_and(|s| s.runs()));
        picked.or_else(|| {
            let index = self.reviews.index(review)?;
            let running = &self.reviews.open[index];
            let root = self.editor.project(running.project)?.root();
            places(running.pr.as_ref(), root).find_map(|place| self.worktree_session(place))
        })
    }

    /// Session `key`'s name, as the echo area shows it.
    fn review_session_name(&self, key: SessionKey) -> String {
        (self.hosting.table().get(key)).map_or_else(String::new, |s| one_line(&s.name, 40))
    }

    /// Marks sent each of `taken` that `review` still holds with the text it was taken with,
    /// and shows their files' cards again; how many were marked, and how many changed or went.
    fn mark_sent(&mut self, review: ReviewId, taken: &[Comment]) -> (usize, usize) {
        let Some(model) = self.editor.review_mut(review) else {
            return (0, taken.len());
        };
        let mut same = Vec::new();
        let mut paths: BTreeSet<&[u8]> = BTreeSet::new();
        for then in taken {
            let unchanged = (model.comment(then.id()))
                .is_some_and(|now| same_text(now, then) && now.author() == Author::Person);
            if unchanged {
                same.push(then.id());
                paths.insert(then.path());
            }
        }
        model.mark_sent(&same);
        let paths: Vec<Box<[u8]>> = paths.into_iter().map(Box::from).collect();
        if let Some(index) = self.reviews.index(review) {
            for path in paths {
                self.tell_cards(index, &path);
            }
        }
        (same.len(), taken.len() - same.len())
    }
}

/// Where a review's session is looked for, in order: a pull request's comments go to the
/// session where its head branch is checked out (step 12b, Q11), else to the review's own.
fn places<'a>(found: Option<&'a PrFound>, root: &'a Path) -> impl Iterator<Item = &'a Path> {
    let head = found.and_then(|found| found.worktree.as_deref());
    head.into_iter().chain(std::iter::once(root))
}

/// Whether `now` says what `then` said when the message was made from it: an edit to other
/// words leaves it unsent, and one to the same words changes nothing the message holds.
fn same_text(now: &Comment, then: &Comment) -> bool {
    now.text() == then.text()
}

/// "1 comment", "3 comments".
fn comments(n: usize) -> String {
    if n == 1 {
        "1 comment".to_owned()
    } else {
        format!("{n} comments")
    }
}

/// The person's comments of `model` not sent, outside threads they resolved, in the order the
/// review buffer shows them: by their labels in `rows`, then any it does not show, as past the
/// text's limit, by file and id.
pub(super) fn unsent(model: &Review, rows: Option<&Rows>) -> Vec<CommentId> {
    let resolved = resolved_threads(model);
    let wanted = |comment: &Comment| {
        comment.author() == Author::Person
            && !comment.is_sent()
            && !resolved.contains(&comment.id())
    };
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    if let Some(rows) = rows {
        let is_label = |at: usize| match rows.at(at).map(|row| row.kind) {
            Some(RowKind::Label { comment }) => Some(comment),
            _ => None,
        };
        let mut line = if is_label(0).is_some() {
            Some(0)
        } else {
            rows.seek(0, Seek::Comment, true)
        };
        while let Some(at) = line {
            if let Some(comment) = is_label(at)
                && seen.insert(comment)
                && model.comment(comment).is_some_and(wanted)
            {
                ids.push(comment);
            }
            line = rows.seek(at, Seek::Comment, true);
        }
    }
    let mut rest: Vec<&Comment> = (model.comments())
        .filter(|comment| !seen.contains(&comment.id()) && wanted(comment))
        .collect();
    rest.sort_by(|a, b| a.path().cmp(b.path()).then(a.id().cmp(&b.id())));
    ids.extend(rest.into_iter().map(Comment::id));
    ids
}

/// The comments in a thread someone resolved: each resolved, or replying, at any depth, to one
/// that is. One pass, oldest first: a parent comes before its replies, so it is judged first.
fn resolved_threads(model: &Review) -> HashSet<CommentId> {
    let mut under = HashSet::new();
    for comment in model.comments() {
        let parent = (comment.reply_to()).filter(|parent| *parent < comment.id());
        if comment.is_resolved() || parent.is_some_and(|parent| under.contains(&parent)) {
            under.insert(comment.id());
        }
    }
    under
}

#[path = "send/pull.rs"]
mod pull;

#[cfg(test)]
#[path = "send_tests.rs"]
mod tests;
