//! A pull request's checks or review threads sent to an agent session (ADR 0127, step 6b),
//! through the same `*review message*` a review's comments go through (ADR 0122, step 12).
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs `gh`: the
//! repository's `gh` thread read and composed the message.
//!
//! - The session is the one the person picked for the pull request with `C-u C-c C-c`, while
//!   it runs and the pull request's buffer is shown, else the hosted session in the worktree
//!   its head branch is checked out in (Q4). With none, the message stays, as a review's does.
//! - The comments of others than the person and the repository's owners, members and
//!   collaborators are left out (Q13, amended); `C-c C-t` adds them to the message's end,
//!   quoted, keeping what the person wrote (Q2).
//! - Nothing on GitHub is marked: the message is dropped once pasted.

use std::path::PathBuf;

use oso_core::buffer::{Buffer, BufferId, Mode, Replacement};
use oso_core::project::ProjectId;

use super::super::super::App;
use super::{NAME, Origin, Waiting};
use crate::sessions::worker::SessionKey;
use crate::vcs::pr::work::PrSent;

/// What a pull request's message was made from.
#[derive(Clone, Debug)]
pub(super) struct FromPull {
    pub(super) project: ProjectId,
    pub(super) number: u64,
    /// The worktree its head branch is checked out in, if one holds its head.
    worktree: Option<PathBuf>,
    /// The comments left out, in the message's shape, for `C-c C-t`.
    others: String,
    left_out: usize,
}

impl App {
    /// A message of pull request `sent.number` of `project` came: it opens in its buffer, if
    /// none waits.
    pub(in crate::app) fn open_pull_message(&mut self, project: ProjectId, sent: PrSent) {
        if let Some(message) = &self.reviews.sending.message
            && self.editor.buffer(message.buffer).is_some()
        {
            let buffer = message.buffer;
            self.editor.show(buffer);
            return self
                .editor
                .message("A message waits: C-c C-c there sends it, C-c C-k drops it");
        }
        let PrSent {
            number,
            message,
            worktree,
        } = sent;
        let name = self.unique_name(NAME);
        let mut buffer = Buffer::from_text(name, &message.text);
        buffer.set_mode(Mode::ReviewMessage);
        let buffer = self.editor.add_buffer(buffer);
        self.editor.show(buffer);
        let from = FromPull {
            project,
            number,
            worktree,
            others: message.others,
            left_out: message.left_out,
        };
        let to = match self.pull_session(&from) {
            Some(key) => format!("to {}", self.review_session_name(key)),
            None => "to no session yet".to_owned(),
        };
        let left_out = match from.left_out {
            0 => String::new(),
            n => format!("; {n} comments by others left out, C-c C-t takes them in"),
        };
        let cut = match message.cut {
            0 => String::new(),
            n => format!("; {n} more cut, past 256 KiB"),
        };
        self.reviews.sending.message = Some(Waiting {
            buffer,
            origin: Origin::Pull(from),
            taken: Vec::new(),
        });
        self.editor.message(format!(
            "#{number}: {} {to}{left_out}{cut}. C-c C-c pastes it, C-u C-c C-c picks the \
             session, C-c C-k drops it",
            items(message.items)
        ));
    }

    /// `review-message-take`: adds the comments a pull request's message `buffer` left out to
    /// its end.
    pub(in crate::app) fn take_review_message(&mut self, buffer: BufferId) {
        let Some(waiting) =
            (self.reviews.sending.message.as_mut()).filter(|message| message.buffer == buffer)
        else {
            return self.editor.message("This is no message waiting to be sent");
        };
        let Origin::Pull(from) = &mut waiting.origin else {
            return self
                .editor
                .message("A review's message leaves out nothing to take in");
        };
        if from.others.is_empty() {
            return self.editor.message("No comment was left out");
        }
        let others = std::mem::take(&mut from.others);
        let taken = std::mem::take(&mut from.left_out);
        let Some(end) = self.editor.buffer(buffer).map(|b| b.text().len()) else {
            return;
        };
        let edit = Replacement {
            range: end..end,
            text: &others,
        };
        if let Err(e) = self.editor.apply_edits(buffer, &[edit]) {
            // What could not go in stays to take in again.
            if let Some(Waiting {
                origin: Origin::Pull(from),
                ..
            }) = self.reviews.sending.message.as_mut()
            {
                from.others = others;
                from.left_out = taken;
            }
            return self.editor.message(format!("They cannot go in: {e}"));
        }
        self.editor.message(format!(
            "{taken} comments by others are at the message's end: read them before C-c C-c"
        ));
    }

    /// The session a pull request's message goes to: the one picked for it while it runs, else
    /// the one in the worktree its head branch is checked out in.
    pub(super) fn pull_session(&self, from: &FromPull) -> Option<SessionKey> {
        let picked = (self.reviews.sending.picked_pulls)
            .get(&(from.project, from.number))
            .copied()
            .filter(|key| self.hosting.table().get(*key).is_some_and(|s| s.runs()));
        picked.or_else(|| {
            let worktree = from.worktree.as_deref()?;
            self.worktree_session(worktree)
        })
    }
}

/// "1 item", "3 items": checks and threads.
fn items(n: usize) -> String {
    if n == 1 {
        "1 item".to_owned()
    } else {
        format!("{n} items")
    }
}
