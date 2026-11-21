//! Asking a question in a review (ADR 0163): `Q` sends the person's question about the place at
//! point, or the lines the region spans, at once to the space's orchestrator, and shows its
//! terminal in the review layout's slot without selecting it, so that the person stays in the
//! review and reads the answer beside it.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or reads a
//! file: the question is composed from the model's hunk or the rows the review shows, and the
//! paste is `send_to_session`'s, `paste_target`'s refusals included.
//!
//! - **Where it goes:** in a space whose orchestrator is recorded, to that orchestrator while it
//!   runs; stopped with its terminal, the person is asked whether to resume it, and the question
//!   waits, one at a time, until it can take a paste, at most [`HOLD_FOR`]. Outside such a
//!   space, to the review's session, as `C-c C-c` sends (ADR 0122, step 12a).
//! - **Submitted (the first exception to ADR 0121's Q17):** the person typed the question and
//!   confirmed it with `RET` in the prompt, which stands for the Enter a paste otherwise waits
//!   for. Enter goes [`ENTER_AFTER`] after the paste, as a key of its own, once the session
//!   still takes a paste; else the question stays in its input box, said so. Text the person
//!   had typed in that input box goes with it: oso cannot see it there.
//! - **Refused** as any send is, and then the session's terminal still shows in the slot, so
//!   the person sees why; the question is in the prompt's history, `M-p` there brings it back.
//! - A file that may hold secrets sends its place alone, and a question from a prompt a yank
//!   of a secret entry turned secret is refused by the command.

use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;
use oso_core::editor::{Editor, ReviewId};
use oso_core::minibuffer::PromptId;
use oso_core::review::{Anchor, Review as Model, RowKind, Rows, Side};
use oso_core::rope::Rope;

use super::super::App;
use super::scope_label;
use super::send::places;
use crate::commands::review::{MAX_NAME, one_line};
use crate::review::ask::{About, Review, compose};
use crate::review::message::{MAX_HUNK_LINES, MAX_LINE_CHARS};
use crate::sessions::worker::SessionKey;

/// How long after its paste a question's Enter goes, as a key of its own: a TUI may read a
/// paste and a key that comes with it in one read as one paste.
pub(super) const ENTER_AFTER: Duration = Duration::from_millis(150);
/// How long a question waits for an orchestrator being resumed to take it.
pub(super) const HOLD_FOR: Duration = Duration::from_secs(120);
/// The most rows of a region looked at, past which its lines are not named.
const MAX_REGION_ROWS: usize = 100_000;

/// What asking keeps: the Enter that waits, the resume offered, and a question held for it.
#[derive(Debug, Default)]
pub(in crate::app) struct Asking {
    enter: Option<Enter>,
    offer: Option<Offer>,
    held: Option<Held>,
}

/// An Enter to send to a session's terminal once `at` passed.
#[derive(Debug)]
struct Enter {
    key: SessionKey,
    buffer: BufferId,
    at: Instant,
}

/// The question asked: resume the orchestrator `held.key`, and ask it then.
#[derive(Debug)]
struct Offer {
    prompt: PromptId,
    held: Held,
}

/// A question waiting for session `key` to take a paste, until `until`.
#[derive(Debug)]
struct Held {
    key: SessionKey,
    text: String,
    until: Instant,
}

/// Where a question goes.
enum Target {
    /// The space's orchestrator, running.
    Orchestrator(SessionKey),
    /// The space's orchestrator, stopped, with its terminal.
    Stopped(SessionKey),
    /// The space's orchestrator, with no terminal: its name.
    Gone(String),
    /// Outside a space with an orchestrator: the review's session.
    Session(SessionKey),
    /// Outside one, with no session for the review.
    Nowhere,
}

impl App {
    /// `review-ask`: sends `question` about the place at point of `review` to where it goes.
    pub(super) fn ask_question(&mut self, review: ReviewId, question: &str) {
        let Some(index) = self.reviews.index(review) else {
            return;
        };
        let text = match self.question_text(index, question) {
            Ok(text) => text,
            Err(why) => return self.editor.message(why),
        };
        // One at a time: a question waiting for its Enter gets it now.
        self.press_enter_now();
        match self.ask_target(review) {
            Target::Orchestrator(key) | Target::Session(key) => self.deliver_question(key, text),
            Target::Stopped(key) => self.offer_resume(key, text),
            Target::Gone(name) => {
                let start = self
                    .dispatcher
                    .where_is(&self.editor, "space-orchestrator-start");
                self.editor.message(format!(
                    "Orchestrator {name} has no terminal: {start} starts it; nothing was sent"
                ));
            }
            Target::Nowhere => {
                let start = self.dispatcher.where_is(&self.editor, "claude");
                self.editor.message(format!(
                    "No orchestrator in this space, and no agent session runs in this \
                     worktree: {start} starts one; nothing was sent"
                ));
            }
        }
    }

    /// Where a question about `review` goes.
    fn ask_target(&self, review: ReviewId) -> Target {
        match self.review_orchestrator() {
            Some(Ok(key)) => Target::Orchestrator(key),
            Some(Err((_, Some(key)))) => Target::Stopped(key),
            Some(Err((name, None))) => Target::Gone(name),
            None => match self.review_session(review) {
                Some(key) => Target::Session(key),
                None => Target::Nowhere,
            },
        }
    }

    /// The orchestrator of the space in front, while one is recorded: its key while it runs,
    /// else its name and its key while it has a terminal. `None` outside such a space.
    pub(super) fn review_orchestrator(
        &self,
    ) -> Option<Result<SessionKey, (String, Option<SessionKey>)>> {
        let spaces = self.watch.spaces();
        let at = spaces.current()?;
        let stored = spaces.stored(at)?;
        stored.orchestrator.as_ref()?;
        if let Some(key) = self.live_orchestrator(at) {
            return Some(Ok(key));
        }
        Some(Err((stored.name.clone(), self.orchestrator_of(at))))
    }

    /// Pastes `text` into session `key`'s terminal, its Enter after [`ENTER_AFTER`], and
    /// shows the terminal in the review layout's slot, the review staying selected.
    fn deliver_question(&mut self, key: SessionKey, text: String) {
        let name = self.review_session_name(key);
        let buffer = match self.paste_target(key) {
            Ok(buffer) => buffer,
            Err(why) => {
                // So that the person sees what it asks.
                self.show_session_beside(key);
                return self.editor.message(format!(
                    "{why}; nothing was sent: M-p in the question's prompt brings it back"
                ));
            }
        };
        self.paste(buffer, text);
        self.reviews.asking.enter = Some(Enter {
            key,
            buffer,
            at: Instant::now() + ENTER_AFTER,
        });
        if self.show_in_review_slot(buffer, false) {
            self.editor
                .message(format!("Asked {name}: its answer shows beside the review"));
        } else {
            let shows = self.dispatcher.where_is(&self.editor, "session-switch");
            self.editor
                .message(format!("Asked {name}: {shows} shows its answer"));
        }
        self.request_redraw();
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

    /// Asks whether to resume the stopped orchestrator `key`, `text` held for it on `y`.
    fn offer_resume(&mut self, key: SessionKey, text: String) {
        let name = self.review_session_name(key);
        // A question asked anew drops the one before, and what it offered.
        self.forget_space_offers();
        let question =
            format!("Orchestrator {name} is stopped: resume it and ask it then? (y or n)");
        match self.editor.ask(question, "space-confirm") {
            Ok(prompt) => {
                self.reviews.asking.offer = Some(Offer {
                    prompt,
                    held: Held {
                        key,
                        text,
                        until: Instant::now(),
                    },
                });
            }
            Err(e) => self.editor.message(e.to_string()),
        }
    }

    /// `y` to the question asked last, if it was a review's resume offer: the orchestrator is
    /// resumed, behind the review, and the question waits for it. Whether it was one.
    pub(in crate::app) fn review_ask_confirmed(&mut self) -> bool {
        let Some(Offer { held, .. }) = self.reviews.asking.offer.take() else {
            return false;
        };
        // Still the space's orchestrator, and still stopped.
        if !matches!(self.review_orchestrator(), Some(Err((_, Some(key)))) if key == held.key) {
            self.editor
                .message("Not asked: the space or its orchestrator changed meanwhile".to_owned());
            return true;
        }
        let name = self.review_session_name(held.key);
        if let Err(why) = self.resume_hosted(held.key, false) {
            self.editor.message(why);
            return true;
        }
        self.show_session_beside(held.key);
        let minutes = HOLD_FOR.as_secs() / 60;
        self.editor.message(format!(
            "Resuming {name}: the question goes once it can take it, for {minutes} minutes"
        ));
        self.reviews.asking.held = Some(Held {
            until: Instant::now() + HOLD_FOR,
            ..held
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

    /// Drops the space's offers, as a question asked anew does.
    fn forget_space_offers(&mut self) {
        self.hosting.spaces.offer = None;
        self.hosting.spaces.replace_asked = None;
    }

    /// After each event: the Enter that waits goes once due, and a question held for an
    /// orchestrator goes once it takes a paste, or is dropped once [`HOLD_FOR`] passed or the
    /// session is gone.
    pub(super) fn sync_review_ask(&mut self) {
        let now = Instant::now();
        if self
            .reviews
            .asking
            .enter
            .as_ref()
            .is_some_and(|enter| enter.at <= now)
        {
            self.press_enter_now();
        }
        let Some(held) = &self.reviews.asking.held else {
            return;
        };
        let key = held.key;
        if self.hosting.table().get(key).is_none() {
            self.reviews.asking.held = None;
            return self
                .editor
                .message("The orchestrator is gone: the question was not sent");
        }
        if self.paste_target(key).is_ok() {
            if let Some(held) = self.reviews.asking.held.take() {
                self.deliver_question(held.key, held.text);
            }
        } else if held.until <= now {
            self.reviews.asking.held = None;
            let name = self.review_session_name(key);
            let minutes = HOLD_FOR.as_secs() / 60;
            self.editor.message(format!(
                "{name} took no question in {minutes} minutes: it was not sent; M-p in the \
                 question's prompt brings it back"
            ));
        }
    }

    /// Sends the Enter that waits now, if its session still takes a paste; else says the
    /// question waits in its input box.
    fn press_enter_now(&mut self) {
        let Some(enter) = self.reviews.asking.enter.take() else {
            return;
        };
        match self.paste_target(enter.key) {
            Ok(buffer) if buffer == enter.buffer => self.send_text(buffer, "\r"),
            Ok(_) => {}
            Err(why) => {
                let name = self.review_session_name(enter.key);
                self.editor.message(format!(
                    "The question waits in {name}'s input box, not submitted: {why}"
                ));
            }
        }
    }

    /// When asking next wants to look: the Enter's time, or the held question's end.
    pub(in crate::app) fn review_ask_deadline(&self) -> Option<Instant> {
        let asking = &self.reviews.asking;
        let enter = asking.enter.as_ref().map(|enter| enter.at);
        let held = asking.held.as_ref().map(|held| held.until);
        enter.into_iter().chain(held).min()
    }

    /// The text of `question` about the place at point of the review at `index`, in the
    /// selected window, or why there is none.
    fn question_text(&self, index: usize, question: &str) -> Result<String, String> {
        let running = &self.reviews.open[index];
        let review = running.id;
        let found = self
            .editor
            .project(running.project)
            .ok_or("The review's project is gone")?;
        let root = found.root();
        let scope = format!(
            "{}, in {}",
            scope_label(&running.scope),
            one_line(found.name(), MAX_NAME)
        );
        let worktree = places(running.pr.as_ref(), root)
            .next()
            .unwrap_or(root)
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
        let at = text.offset_to_point(selection.cursor).row;
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
        let quoted = span.and_then(|(first, last)| region_rows(rows, text, first, last));
        let (path, about) = match quoted {
            Some(quoted) => quoted,
            None => point_about(&self.editor, review, model, rows, text, at)?,
        };
        let held = super::secrets::held_paths(&self.editor);
        let secret = super::secrets::withheld(root, &held, &path);
        let place = Review {
            scope: &scope,
            root: &worktree,
        };
        Ok(compose(&place, &path, &about, secret, question))
    }
}

/// What line `at` of a review's text is, for a question: a changed line or a hunk's header with
/// its hunk, an unchanged line, a comment's place, or a file.
fn point_about<'a>(
    editor: &Editor,
    review: ReviewId,
    model: &'a Model,
    rows: &Rows,
    text: &Rope,
    at: usize,
) -> Result<(Box<[u8]>, About<'a>), String> {
    let row = rows.at(at).ok_or("Nothing to ask about here")?;
    let path: Box<[u8]> = row.path.into();
    let hunk = |anchor: Anchor, path: Box<[u8]>| {
        if editor.review_file_behind(review, &path) {
            return Err("This file is being read again: try in a moment".to_owned());
        }
        let shape = model
            .file(&path)
            .ok_or("This file is being read again: try in a moment")?;
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
        RowKind::File | RowKind::Note => Ok((path, About::File)),
        RowKind::Gap { .. } => Err("Ask on a line: = shows the lines here".to_owned()),
        RowKind::Filler => Err("Nothing to ask about here".to_owned()),
    }
}

/// The changed and unchanged lines of the rows from `first` to `last` of a review's text, of
/// the first such row's file, and their place: `None` with no such row.
fn region_rows(
    rows: &Rows,
    text: &Rope,
    first: usize,
    last: usize,
) -> Option<(Box<[u8]>, About<'static>)> {
    let mut path: Option<&[u8]> = None;
    let mut lines = Vec::new();
    let (mut new, mut old) = (None::<(u32, u32)>, None::<(u32, u32)>);
    let widen = |span: &mut Option<(u32, u32)>, number: u32| {
        *span = Some(span.map_or((number, number), |(a, b)| (a.min(number), b.max(number))));
    };
    for line in (first..=last).take(MAX_REGION_ROWS) {
        let Some(row) = rows.at(line) else {
            continue;
        };
        let sign = match row.kind {
            RowKind::Lines {
                side: Side::Old, ..
            } => '-',
            RowKind::Lines {
                side: Side::New, ..
            } => '+',
            RowKind::Context { .. } => ' ',
            _ => continue,
        };
        match path {
            None => path = Some(row.path),
            Some(path) if path != row.path => break,
            Some(_) => {}
        }
        match (row.kind, row.line()) {
            (
                RowKind::Lines {
                    side: Side::Old, ..
                },
                Some((_, number)),
            ) => widen(&mut old, number),
            (_, Some((_, number))) => widen(&mut new, number),
            (_, None) => {}
        }
        // One more than a whole hunk's, so that a cut is known.
        if lines.len() <= MAX_HUNK_LINES {
            lines.push((sign, row_text(text, line)));
        }
    }
    let path = path?;
    let (span, side) = match (new, old) {
        (Some(span), _) => (span, "new"),
        (None, Some(span)) => (span, "old"),
        (None, None) => return None,
    };
    let place = match span {
        (a, b) if a == b => format!("L{a} ({side})"),
        (a, b) => format!("L{a}-L{b} ({side})"),
    };
    Some((path.into(), About::Rows { place, lines }))
}

/// Row `line`'s text as the review shows it, its sign left out, at most
/// [`MAX_LINE_CHARS`] characters' bytes read.
fn row_text(text: &Rope, line: usize) -> String {
    let start = text.line_start(line);
    let end = text.line_end(line).min(start + MAX_LINE_CHARS * 4 + 8);
    let mut out = String::with_capacity(end - start);
    for chunk in text.chunks_in_range(start..end) {
        out.push_str(chunk);
    }
    // The sign oso wrote, one character.
    let mut chars = out.chars();
    chars.next();
    chars.as_str().to_owned()
}

#[cfg(test)]
mod tests;
