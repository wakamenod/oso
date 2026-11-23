//! Asking a question in a review (ADR 0163): `Q` sends the person's question about the place at
//! point, or the lines the region spans, at once to the space's orchestrator, and shows its
//! terminal in the review layout's slot without selecting it, so that the person stays in the
//! review and reads the answer beside it.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or reads a
//! file.
//!
//! The question is composed by `review::ask` from the model's hunk or the rows the review shows,
//! and pasted and submitted through the sessions' delivery path (`paste_into_session`,
//! `submit_paste`), with its refusals.
//!
//! - **Where it goes** (`target`): to the space's orchestrator while the review is of one of the
//!   space's repositories; stopped with its terminal, the person is asked whether to resume it,
//!   and the question waits, one at a time, until it can take a paste, at most [`HOLD_FOR`].
//!   Else to the review's session, as `C-c C-c` sends (ADR 0122, step 12a).
//! - **Submitted (the one exception to ADR 0121's Q17):** the person typed the question and
//!   confirmed it with `RET` in the prompt. Claude Code's input box is read from the screen the
//!   UI holds (`sessions::send::input_box`): while it shows, the question is pasted, and Enter
//!   goes as a key of its own, with any text the box held (ADR 0163, "Submit once pasted, as
//!   amended"), at least [`ENTER_AFTER`] later and once the screen has changed since the paste,
//!   read again then and still showing the box, within [`ENTER_WAIT`]; with no box, as when a
//!   dialog, a picker or another mode shows, nothing is pasted or submitted.
//! - **To a native session** (ADR 0165, part 2f; ADR 0163, "A native session's question, as
//!   amended"): sent at once as a message, or queued behind the turn under way
//!   (`send_confirmed`), the composer's draft untouched. A stopped native orchestrator is
//!   offered a resume as a terminal's is, and the question held goes once its process runs
//!   (ADR 0176, part O-2); one whose resume did not start drops it. Its transcript shows in the
//!   slot as a terminal does.
//! - **Refused** as any send is, and then the session's terminal still shows in the slot, so
//!   the person sees why; the question is in the prompt's history, `M-p` there brings it back.
//! - A file that may hold secrets sends its place alone, judged again before a held question
//!   goes, and a question from a prompt a yank of a secret entry turned secret is refused by the
//!   command.

use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;
use oso_core::editor::ReviewId;
use oso_core::minibuffer::PromptId;
use oso_core::review::{Anchor, Review as Model, RowKind, Rows};
use oso_core::rope::Rope;

use super::super::App;
use super::super::sessions::{Orchestrator, Outgoing, Shown};
use crate::commands::agent_sessions::MAX_SEND_BYTES;
use crate::review::ask::{About, Where, compose, region_rows, row_text};
use crate::sessions::hosted::Run;
use crate::sessions::send::{InputBox, NoBox, input_box};
use crate::sessions::spaces::SpaceId;
use crate::sessions::worker::SessionKey;

/// How long after its paste a question's Enter may go, as a key of its own: a TUI may read a
/// paste and a key that comes with it in one read as one paste.
pub(super) const ENTER_AFTER: Duration = Duration::from_millis(150);
/// How long the Enter waits for a screen newer than the paste, as a terminal no window shows
/// is looked at only a few times a second: past it, the question stays unsubmitted.
pub(super) const ENTER_WAIT: Duration = Duration::from_secs(2);
/// How often the screen is looked at meanwhile.
const ENTER_POLL: Duration = Duration::from_millis(100);
/// How soon it is looked at again when a snapshot came: the terminal's thread builds the next
/// from what came since, so that the Enter judges the newest screen, not one it held.
const ENTER_RECHECK: Duration = Duration::from_millis(10);
/// How long a question waits for an orchestrator being resumed to take it.
pub(super) const HOLD_FOR: Duration = Duration::from_secs(120);

/// What `c` says on a file whose rows shown the model lags, as `review-ask` does too.
const BEHIND: &str = "This file is being read again: try in a moment";

/// What asking keeps: the Enter that waits, the resume offered, and a question held for it.
#[derive(Debug, Default)]
pub(in crate::app) struct Asking {
    enter: Option<Enter>,
    offer: Option<Offer>,
    held: Option<Held>,
}

/// An Enter for session `key`'s terminal `buffer`, whose screen was at `generation` when the
/// question was pasted: looked at from `next`, given up at `until`.
#[derive(Debug)]
struct Enter {
    key: SessionKey,
    buffer: BufferId,
    generation: Option<u64>,
    next: Instant,
    until: Instant,
}

/// A question composed, with what is judged again before it goes later.
#[derive(Debug)]
struct Question {
    review: ReviewId,
    path: Box<[u8]>,
    /// Whether its file was judged to hold secrets, so that its lines were left out.
    secret: bool,
    text: String,
    /// The space in front when it was asked, whose slot shows the answer.
    space: Option<SpaceId>,
}

/// The question asked: resume the orchestrator `key`, and ask it `question` then.
#[derive(Debug)]
struct Offer {
    prompt: PromptId,
    key: SessionKey,
    question: Question,
}

/// A question waiting for session `key` to take a paste, until `until`.
#[derive(Debug)]
struct Held {
    key: SessionKey,
    question: Question,
    until: Instant,
}

impl App {
    /// `review-ask`: sends `question` about the place at point of `review` to where it goes.
    pub(super) fn ask_question(&mut self, review: ReviewId, question: &str) {
        let question = match self.compose_question(review, question) {
            Ok(question) => question,
            Err(why) => return self.editor.message(why),
        };
        // One at a time: an Enter that waits goes now if it may, else it is given up.
        self.look_for_enter(Instant::now(), true);
        match self.review_orchestrator(review) {
            Some(Orchestrator::Running(key)) => self.deliver_question(key, question),
            // A native one too (ADR 0176, part O-2).
            Some(Orchestrator::Stopped { key, .. }) => self.offer_resume(key, question),
            Some(stopped @ Orchestrator::NoTerminal { .. }) => {
                if let Some(why) = self.not_running(&stopped, "nothing was sent") {
                    self.editor.message(why);
                }
            }
            None => match self.review_session(review) {
                Some(key) => self.deliver_question(key, question),
                None => {
                    let start = self.dispatcher.where_is(&self.editor, "claude");
                    self.editor.message(format!(
                        "No orchestrator of this repository's space, and no agent session runs \
                         in this worktree: {start} starts one; nothing was sent"
                    ));
                }
            },
        }
    }

    /// Pastes `question` into session `key`'s terminal, as its input box allows, its Enter
    /// due after [`ENTER_AFTER`], and shows the terminal in the review layout's slot,
    /// the review staying selected, while the space it was asked in is in front.
    fn deliver_question(&mut self, key: SessionKey, question: Question) {
        let name = self.review_session_name(key);
        let beside = question.space.is_some() && question.space == self.front_space_id();
        if self.hosting.natives.has(key) {
            return self.deliver_native_question(key, &name, beside, &question.text);
        }
        let buffer = match self.paste_target(key) {
            Ok(buffer) => buffer,
            Err(why) => {
                // So that the person sees what it asks.
                if beside {
                    self.show_session_beside(key);
                }
                return self.editor.message(format!(
                    "{why}; nothing was sent: M-p in the question's prompt brings it back"
                ));
            }
        };
        let boxed = self.input_box_of(buffer);
        if let InputBox::Other(why) = boxed {
            if beside {
                self.show_in_review_slot(buffer, false);
            }
            return self.editor.message(format!(
                "{}: nothing was sent; M-p in the question's prompt brings it back",
                why.said(&name)
            ));
        }
        let generation = self.screen_generation(buffer);
        let text = Outgoing::Composed(&question.text);
        if let Err(why) = self.paste_into_session(key, text, Shown::Nothing) {
            return self.editor.message(why);
        }
        let now = Instant::now();
        self.reviews.asking.enter = Some(Enter {
            key,
            buffer,
            generation,
            next: now + ENTER_AFTER,
            until: now + ENTER_WAIT,
        });
        let shown = beside && self.show_in_review_slot(buffer, false);
        let mut said = if shown {
            format!("Asked {name}: its answer shows beside the review")
        } else {
            let shows = self.dispatcher.where_is(&self.editor, "session-switch");
            format!("Asked {name}: {shows} shows its answer")
        };
        if boxed != InputBox::Empty {
            said.push_str("; what its input box held went with it");
        }
        self.editor.message(said);
        self.request_redraw();
    }

    /// [`App::deliver_question`] to native session `key`, named `name`: sent at once, or queued
    /// behind the turn under way, its composer's draft untouched (ADR 0165, step 2f, Q4 (a)),
    /// and its transcript shown in the slot while the space it was asked in is in front.
    fn deliver_native_question(&mut self, key: SessionKey, name: &str, beside: bool, text: &str) {
        let sent = self.send_confirmed(key, text);
        let transcript = (self.hosting.table().get(key)).and_then(|session| session.buffer);
        let shown =
            beside && transcript.is_some_and(|buffer| self.show_in_review_slot(buffer, false));
        let said = match sent {
            Err(why) => format!(
                "{name}: {why}; nothing was sent, and M-p in the question's prompt brings it back"
            ),
            Ok(how) if shown => format!("Asked {name}, {how}: its answer shows beside the review"),
            Ok(how) => {
                let shows = self.dispatcher.where_is(&self.editor, "session-switch");
                format!("Asked {name}, {how}: {shows} shows its answer")
            }
        };
        self.editor.message(said);
        self.request_redraw();
    }

    /// What Claude Code's input box in the terminal `buffer` holds, as the screen the UI has
    /// shows it.
    fn input_box_of(&self, buffer: BufferId) -> InputBox {
        (self.terminals.map.get(buffer)).map_or(InputBox::Other(NoBox::NoRules), |hosted| {
            input_box(&hosted.grid.row_texts())
        })
    }

    /// The id of the space in front.
    pub(super) fn front_space_id(&self) -> Option<SpaceId> {
        let spaces = self.watch.spaces();
        let at = spaces.current()?;
        Some(spaces.get(at)?.id().clone())
    }

    /// Shows session `key`'s terminal in the review layout's slot, the review staying
    /// selected, if it has one.
    fn show_session_beside(&mut self, key: SessionKey) {
        let buffer = (self.hosting.table().get(key))
            .and_then(|session| session.buffer)
            .filter(|buffer| self.editor.buffer(*buffer).is_some());
        if let Some(buffer) = buffer {
            self.show_in_review_slot(buffer, false);
        }
    }

    /// Asks whether to resume the stopped orchestrator `key`, `question` held for it on `y`.
    fn offer_resume(&mut self, key: SessionKey, question: Question) {
        let name = self.review_session_name(key);
        let asked = format!("Orchestrator {name} is stopped: resume it and ask it then? (y or n)");
        // A question asked anew drops the one before, and what it offered.
        match self.ask_confirm(asked) {
            Ok(prompt) => {
                self.reviews.asking.offer = Some(Offer {
                    prompt,
                    key,
                    question,
                });
            }
            Err(e) => self.editor.message(e.to_string()),
        }
    }

    /// `y` to the question asked last, if it was a review's resume offer: the orchestrator is
    /// resumed, behind the review, and the question waits for it. Whether it was one.
    pub(in crate::app) fn review_ask_confirmed(&mut self) -> bool {
        let Some(offer) = self.reviews.asking.offer.take() else {
            return false;
        };
        // Still the space's orchestrator, and still stopped.
        let still = self.review_orchestrator(offer.question.review);
        if !matches!(still, Some(Orchestrator::Stopped { key, .. }) if key == offer.key) {
            self.editor
                .message("Not asked: the space or its orchestrator changed meanwhile".to_owned());
            return true;
        }
        let name = self.review_session_name(offer.key);
        if let Err(why) = self.resume_hosted(offer.key, false) {
            self.editor.message(why);
            return true;
        }
        self.show_session_beside(offer.key);
        let minutes = HOLD_FOR.as_secs() / 60;
        self.editor.message(format!(
            "Resuming {name}: the question goes once it can take it, for {minutes} minutes"
        ));
        self.reviews.asking.held = Some(Held {
            key: offer.key,
            question: offer.question,
            until: Instant::now() + HOLD_FOR,
        });
        true
    }

    /// After an event: a resume offer whose question is no longer open, and was not answered
    /// `y` in it, is dropped, as the space's offers are.
    pub(in crate::app) fn forget_unanswered_review_offer(&mut self) {
        let open = self.editor.prompt().map(|prompt| prompt.id());
        let asked = self.reviews.asking.offer.as_ref().map(|offer| offer.prompt);
        if asked.is_some() && open != asked {
            self.reviews.asking.offer = None;
        }
    }

    /// Drops the review's resume offer, as a space's question asked anew does.
    pub(in crate::app) fn forget_review_offer(&mut self) {
        self.reviews.asking.offer = None;
    }

    /// After each event: the Enter that waits is looked for, and a held question goes once its
    /// orchestrator takes a paste at an input box, or a native one's process runs, or is
    /// dropped once [`HOLD_FOR`] passed, the session is gone, a native one's resume did not
    /// start, or its file may now hold secrets.
    pub(super) fn sync_review_ask(&mut self) {
        let now = Instant::now();
        self.look_for_enter(now, false);
        let Some(held) = &self.reviews.asking.held else {
            return;
        };
        let (key, until) = (held.key, held.until);
        if self.hosting.table().get(key).is_none() {
            self.reviews.asking.held = None;
            return self
                .editor
                .message("The orchestrator is gone: the question was not sent");
        }
        let ready = match self.native_run(key) {
            Some(Run::Running { .. }) => true,
            Some(Run::Starting) => false,
            Some(Run::Stopped | Run::Failed) => {
                self.reviews.asking.held = None;
                let name = self.review_session_name(key);
                return self.editor.message(format!(
                    "{name} did not resume: the question was not sent; M-p in the question's \
                     prompt brings it back"
                ));
            }
            None => self
                .paste_target(key)
                .is_ok_and(|buffer| !matches!(self.input_box_of(buffer), InputBox::Other(_))),
        };
        if ready {
            let Some(held) = self.reviews.asking.held.take() else {
                return;
            };
            if !held.question.secret && self.now_secret(&held.question) {
                return self.editor.message(
                    "The question's file may hold secrets now: it was not sent".to_owned(),
                );
            }
            self.deliver_question(held.key, held.question);
        } else if until <= now {
            self.reviews.asking.held = None;
            let name = self.review_session_name(key);
            let minutes = HOLD_FOR.as_secs() / 60;
            self.editor.message(format!(
                "{name} took no question in {minutes} minutes: it was not sent; M-p in the \
                 question's prompt brings it back"
            ));
        }
    }

    /// Whether `question`'s file may hold secrets now, by its name or an open buffer of it.
    fn now_secret(&self, question: &Question) -> bool {
        let Some(index) = self.reviews.index(question.review) else {
            return true;
        };
        let held = super::secrets::held_paths(&self.editor);
        (self.review_places(index).iter())
            .any(|root| super::secrets::withheld(root, &held, &question.path))
    }

    /// Sends the Enter that waits once its time came and the screen changed since the paste,
    /// as the TUI draws a paste at once, taking what the terminal sent first: only while the
    /// session still takes a paste and the box, read on that newer screen, still shows, so
    /// that it never goes to a dialog or a picker that came meanwhile, nor alone when the paste
    /// did not arrive. With `last`, as a question asked anew comes, it goes only if it may
    /// now, and is given up else; so it is once [`ENTER_WAIT`] passed.
    fn look_for_enter(&mut self, now: Instant, last: bool) {
        let Some(enter) = &self.reviews.asking.enter else {
            return;
        };
        let (key, buffer, generation, until) =
            (enter.key, enter.buffer, enter.generation, enter.until);
        let due = now >= enter.next;
        if !due && !last {
            return;
        }
        let name = self.review_session_name(key);
        let why = if due {
            let came = self.take_screen_now(buffer);
            let changed = self.screen_generation(buffer) != generation;
            let current = changed && !came;
            if !current && !last && now < until {
                let wait = if came { ENTER_RECHECK } else { ENTER_POLL };
                if let Some(enter) = &mut self.reviews.asking.enter {
                    enter.next = now + wait;
                }
                return;
            }
            self.reviews.asking.enter = None;
            match (current, self.input_box_of(buffer)) {
                (false, _) => Some("its screen showed nothing newer than the paste".to_owned()),
                (true, InputBox::Other(why)) => Some(why.said(&name)),
                (true, _) => self.submit_paste(key, buffer).err(),
            }
        } else {
            self.reviews.asking.enter = None;
            Some("another question came before it could go".to_owned())
        };
        if let Some(why) = why {
            self.editor.message(format!(
                "The question waits in {name}'s input box, not submitted: {why}; press Enter \
                 there"
            ));
        }
    }

    /// Takes the snapshot the terminal `buffer`'s thread holds, if it holds one, whether or not
    /// a frame or the pace for terminals no window shows would take it now; whether one came.
    /// None coming means the screen the UI holds is the newest the thread built.
    fn take_screen_now(&mut self, buffer: BufferId) -> bool {
        let Some(hosted) = self.terminals.map.get_mut(buffer) else {
            return false;
        };
        hosted.ready = true;
        self.take_terminal_snapshots();
        self.terminals
            .map
            .get(buffer)
            .is_some_and(|hosted| hosted.ready)
    }

    /// The generation of the screen the UI holds of the terminal `buffer`: another number
    /// means other rows.
    fn screen_generation(&self, buffer: BufferId) -> Option<u64> {
        (self.terminals.map.get(buffer)).map(|hosted| hosted.grid.generation())
    }

    /// Before the loop waits: once [`App::review_ask_deadline`] passed with no event, the
    /// Enter that waits is looked for and a held question past its end is dropped, as every
    /// deadline the loop wakes for is looked at there.
    pub(in crate::app) fn review_ask_due(&mut self) {
        if self
            .review_ask_deadline()
            .is_some_and(|deadline| deadline <= Instant::now())
        {
            self.sync_review_ask();
        }
    }

    /// When asking next wants to look: the Enter's next look, or the held question's end.
    pub(in crate::app) fn review_ask_deadline(&self) -> Option<Instant> {
        let asking = &self.reviews.asking;
        let enter = asking.enter.as_ref().map(|enter| enter.next);
        let held = asking.held.as_ref().map(|held| held.until);
        enter.into_iter().chain(held).min()
    }

    /// The question `question` about the place at point of `review` in the selected window,
    /// composed, or why there is none.
    fn compose_question(&self, review: ReviewId, question: &str) -> Result<Question, String> {
        let index = self.reviews.index(review).ok_or("Not in a review")?;
        let scope = self
            .review_scope(index)
            .ok_or("The review's project is gone")?;
        let worktree = (self.review_worktree(index))
            .ok_or("The review's project is gone")?
            .to_string_lossy()
            .into_owned();
        let (window, buffer) = self.editor.current();
        let rows = self
            .editor
            .review_rows_of(window.buffer())
            .ok_or("Not in a review")?;
        let model = self.editor.review(review).ok_or("Not in a review")?;
        let text = buffer.text();
        let selection = window.selections.primary();
        let region = window.selections.mark_active().is_active() && !selection.range().is_empty();
        let span = region.then(|| {
            let range = selection.range();
            let first = text.offset_to_point(range.start).row;
            let mut last = text.offset_to_point(range.end).row;
            // A region that ends where a line starts leaves that line out.
            if last > first && text.line_start(last) == range.end {
                last -= 1;
            }
            (first, last)
        });
        let at = text.offset_to_point(selection.cursor).row;
        let quoted = span.and_then(|(first, last)| region_rows(rows, text, first, last));
        let (path, about) = match quoted {
            Some(quoted) => quoted,
            None => point_about(model, rows, text, at)?,
        };
        // No line lands by a reading the model does not hold yet, as `c` refuses there.
        if self.editor.review_file_behind(review, &path) {
            return Err(BEHIND.to_owned());
        }
        let held = super::secrets::held_paths(&self.editor);
        let secret = (self.review_places(index).iter())
            .any(|root| super::secrets::withheld(root, &held, &path));
        let at = Where {
            scope: &scope,
            worktree: &worktree,
        };
        let text = compose(&at, &path, &about, secret, question);
        if text.len() > MAX_SEND_BYTES {
            return Err(format!(
                "The question is {} KiB with its lines, past the 256 KiB a session takes",
                text.len().div_ceil(1024)
            ));
        }
        Ok(Question {
            review,
            path,
            secret,
            text,
            space: self.front_space_id(),
        })
    }
}

/// What line `at` of a review's text is, for a question: a changed line or a hunk's header with
/// its hunk, an unchanged line, a comment's place, or a file.
fn point_about<'a>(
    model: &'a Model,
    rows: &Rows,
    text: &Rope,
    at: usize,
) -> Result<(Box<[u8]>, About<'a>), String> {
    let row = rows.at(at).ok_or("Nothing to ask about here")?;
    let path: Box<[u8]> = row.path.into();
    let hunk = |anchor: Anchor, path: Box<[u8]>| {
        let shape = model.file(&path).ok_or(BEHIND)?;
        Ok((path, About::Hunk { anchor, shape }))
    };
    match row.kind {
        RowKind::Lines { .. } => match row.line() {
            Some((side, number)) => hunk(Anchor::Line { side, number }, path),
            None => Ok((path, About::File)),
        },
        RowKind::Hunk { hunk: index } => hunk(Anchor::Hunk { index }, path),
        RowKind::Context { new, .. } => {
            let lines = vec![(' ', row_text(text, at))];
            let place = format!("L{new} (unchanged)");
            Ok((path, About::Rows { place, lines }))
        }
        RowKind::Label { comment } | RowKind::Card { comment } => {
            let comment = model.comment(comment).ok_or("Nothing to ask about here")?;
            let path: Box<[u8]> = comment.path().into();
            if comment.is_outdated() {
                Ok((path, About::File))
            } else {
                hunk(comment.anchor(), path)
            }
        }
        RowKind::File | RowKind::Note | RowKind::Message => Ok((path, About::File)),
        RowKind::Gap { .. } => Err("Ask on a line: = shows the lines here".to_owned()),
        RowKind::Filler => Err("Nothing to ask about here".to_owned()),
    }
}
