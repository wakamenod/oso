//! Reviews in the app (ADR 0122): what review commands ask of the host, carried out on the UI
//! thread. Each open review has its own `oso-review` thread, which runs git and diffs, and its
//! own `oso-review-view` thread, which lays out the review buffer's text; the review menu's
//! counts come from `oso-review-count`, and saved reviews are read and written on
//! `oso-review-store` (`keep`, `store`, step 6). What they report is put in here, in order.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git, reads or
//! writes a file, diffs or builds a review's text: a text comes whole from its view thread,
//! goes in at O(1), and the text it replaces is freed off the UI thread. Nothing here logs a
//! file's text.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oso_core::editor::{ReviewId, ReviewRequest, ScopeKind};
use oso_core::project::ProjectId;

use super::{App, UserEvent};
use crate::commands::review::{MAX_NAME, RESTRICTED, one_line};
use crate::git::TrustedRoot;
use crate::review::count::Counted;
use crate::review::pr::PrFound;
use crate::review::view::{Tell, Text, View};
use crate::review::{Asked, Report, Request, Reviewer, Scope};
use scope::{fetching_said, found_said, same_scope, scope_label, scope_of};
pub(in crate::app) use state::Reviews;
use state::{Reopen, Running};

/// The most characters of a failure the echo area shows.
const MAX_MESSAGE: usize = 200;

/// What a review's threads sent the UI thread.
pub(crate) enum ReviewEvent {
    /// A review's own thread reported, as its `serial`th start.
    Report {
        review: ReviewId,
        serial: u64,
        report: Report,
    },
    /// The `serial`th thread taking a baseline of `project` reported.
    Baseline {
        project: ProjectId,
        serial: u64,
        report: Report,
    },
    /// A review's view thread built a text.
    Text { review: ReviewId, text: Box<Text> },
    /// The menu's counting threads counted.
    Counted(Counted),
    /// The saved reviews' store found or did something (step 6).
    Stored(store::Stored),
}

impl fmt::Debug for ReviewEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Report { review, report, .. } => f
                .debug_struct("Report")
                .field("review", &review.get())
                .field("report", report)
                .finish_non_exhaustive(),
            Self::Baseline { report, .. } => f
                .debug_struct("Baseline")
                .field("report", report)
                .finish_non_exhaustive(),
            Self::Text { review, .. } => f
                .debug_struct("Text")
                .field("review", &review.get())
                .finish_non_exhaustive(),
            Self::Counted(counted) => f.debug_tuple("Counted").field(counted).finish(),
            Self::Stored(stored) => f.debug_tuple("Stored").field(stored).finish(),
        }
    }
}

/// oso's own directory in the per-user state directory, where reviews keep their index
/// copies. Asks the environment and not the disk. A session's start baseline is taken there
/// too (ADR 0121).
pub(in crate::app) fn state() -> Option<PathBuf> {
    oso_platform::privacy::state_dir()
}

/// Seconds since the Unix epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |now| now.as_secs())
}

impl App {
    /// After commands and after each event: revokes the roots of projects no longer trusted,
    /// first, so that nothing asked runs git for one, then carries out what review commands
    /// asked, starts or stops the menu's counts, and puts in the texts that wait while nothing
    /// holds them.
    pub(super) fn sync_reviews(&mut self) {
        self.start_review_store();
        self.revoke_untrusted_reviews();
        // Before the requests, so that a head that closed the menu, as `b` does, stops the
        // menu's counts before a request starts its own.
        self.sync_review_menu();
        for request in self.editor.take_review_requests() {
            self.review_request(request);
        }
        // What a restart carried, once its projects are known (step 6).
        self.reopen_handed_reviews();
        self.put_in_review_texts();
        // A file a reject opened may have loaded.
        self.go_on_reverting();
        self.sync_review_layouts();
        // The files pane's width and the filter (step 6b).
        self.sync_review_files();
        self.sync_review_sides();
        self.sync_review_held();
        self.sync_review_watches();
        self.sync_review_comments();
        self.sync_review_message();
        self.sync_review_saves();
    }

    fn review_request(&mut self, request: ReviewRequest) {
        match request {
            ReviewRequest::Open {
                project,
                scope,
                revision,
            } => self.open_review(project, scope, revision),
            ReviewRequest::Baseline { project } => self.take_baseline(project),
            ReviewRequest::Refresh(id) => self.refresh_review(id, true),
            ReviewRequest::Close(id) => {
                self.close_review(id);
                self.review_closed_layout(id);
            }
            ReviewRequest::Layout { menu } => self.review_layout_key(menu),
            ReviewRequest::Quit => self.review_quit_layout(),
            ReviewRequest::Redraw { review, path } => self.redraw_comments(review, &path),
            ReviewRequest::Context { review, path, show } => {
                if let Some(index) = self.reviews.index(review) {
                    (self.reviews.open[index].view).tell(Tell::Context { path, show });
                }
            }
            ReviewRequest::Send(id) => self.send_review(id),
            ReviewRequest::MessageSend { buffer, to } => self.send_review_message(buffer, to),
            ReviewRequest::MessageDrop { buffer } => self.drop_review_message(buffer),
            ReviewRequest::Resume(pick) => self.resume_review(&pick),
            ReviewRequest::Forget(pick) => self.forget_review(&pick),
            ReviewRequest::MessageTake { buffer } => self.take_review_message(buffer),
            ReviewRequest::Visit {
                review,
                path,
                side,
                number,
                old_side,
            } => self.visit_review_line(review, &path, (side, number), old_side),
            decision => self.decision_request(decision),
        }
    }

    /// Opens a review of `scope` in `project`, or shows the one open and reads it again.
    pub(super) fn open_review(
        &mut self,
        project: ProjectId,
        kind: ScopeKind,
        revision: Option<String>,
    ) {
        if kind == ScopeKind::Branch && revision.is_none() {
            // The base is not known yet: it is found first, and the review opens with it.
            return self.find_branch_base(project);
        }
        let baseline = self.reviews.baseline(project);
        let scope = match scope_of(kind, revision, baseline) {
            Ok(scope) => scope,
            Err(why) => return self.editor.message(why),
        };
        self.editor.set_last_scope(project, kind);
        self.open_scope(project, scope, true);
    }

    /// Opens a review of `scope` in `project`, or shows the one open and reads it again: as a
    /// request names it, or as a saved review or a restart kept it, a session review with its
    /// own baseline (step 6). With `laid`, as a person's open, it shows in the review layout
    /// (ADR 0160, Q2); else, as a restart opens it again, in the selected window.
    pub(super) fn open_scope(&mut self, project: ProjectId, scope: Scope, laid: bool) {
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            return self.editor.message(RESTRICTED);
        };
        if matches!(scope, Scope::Session { .. }) {
            self.session_review_opened(root.path());
        }
        let window = self.editor.tiled().0.id();
        let open =
            self.reviews.open.iter().position(|running| {
                running.project == project && same_scope(&running.scope, &scope)
            });
        if let Some(index) = open {
            let id = self.reviews.open[index].id;
            if !(laid && self.show_review_laid(id))
                && let Some(buffer) = self.editor.review_buffer(id)
            {
                self.editor.show_in(window, buffer);
            }
            let running = &mut self.reviews.open[index];
            let reopen = running.reopen(&scope);
            // A new baseline: the session review compares with it from now on.
            running.scope = scope.clone();
            match reopen {
                // A revoked review starts over with the scope, as `g` would.
                Reopen::Refresh | Reopen::Restart => self.refresh_review(id, true),
                Reopen::Open => self.ask_review(index, Request::Open(scope)),
            }
            return self.request_redraw();
        }
        let name = self
            .editor
            .project(project)
            .map_or_else(String::new, |project| one_line(project.name(), MAX_NAME));
        let label = scope_label(&scope);
        let (id, buffer) = self
            .editor
            .add_review(project, format!("*review: {label} {name}*"));
        let waker = self.waker.clone();
        let view = View::start(
            format!("Review \u{b7} {label} \u{b7} {name}"),
            move |text| {
                waker.send(UserEvent::Review(ReviewEvent::Text {
                    review: id,
                    text: Box::new(text),
                }))
            },
        );
        let view = match view {
            Ok(view) => view,
            Err(e) => {
                self.editor.remove_review(id);
                return self.editor.message(format!("Cannot start the review: {e}"));
            }
        };
        // The top its files' names are judged under, before highlighting is ever on.
        let top = root.path().to_path_buf();
        view.tell(Tell::Highlighter {
            highlighter: None,
            top,
        });
        let running = Running::new(id, project, scope.clone(), root, view);
        self.reviews.open.push(running);
        if !(laid && self.show_review_laid(id)) {
            self.editor.show_in(window, buffer);
        }
        let index = self.reviews.open.len() - 1;
        self.start_reviewer(index);
        // What was saved of it is read meanwhile, and what its thread reports waits for it
        // (step 6).
        self.open_saved(index, &scope);
        self.ask_review(index, Request::Open(scope));
        self.request_redraw();
    }

    /// Starts the review's own thread over its root, dropping the one before.
    fn start_reviewer(&mut self, index: usize) {
        // What the thread before was carrying is carried on the view thread; what it was
        // staging is answered by no one.
        self.carries_failed(index, true);
        self.stages_lost(index);
        let serial = self.reviews.serial();
        let running = &mut self.reviews.open[index];
        running.serial = serial;
        running.reviewer = None;
        let review = running.id;
        let waker = self.waker.clone();
        let started = Reviewer::start(running.root.clone(), state(), move |report| {
            waker.send(UserEvent::Review(ReviewEvent::Report {
                review,
                serial,
                report,
            }))
        });
        match started {
            Ok(reviewer) => running.reviewer = Some(reviewer),
            Err(e) => {
                let why = format!("cannot start the review's thread: {e}");
                running.view.tell(Tell::Said(vec![why.clone()]));
                self.editor.message(why);
            }
        }
    }

    /// Asks the review's own thread for `request`.
    fn ask_review(&mut self, index: usize, request: Request) {
        let running = &self.reviews.open[index];
        let asked = running
            .reviewer
            .as_ref()
            .is_some_and(|reviewer| reviewer.ask(request));
        if !asked {
            self.editor
                .message("The review's thread has ended: C-x k, then open it again");
        }
    }

    /// Reads the review's files again, those its windows show first; `person` if the person
    /// asked, which alone asks `gh` again for a pull request. A review whose project was trusted
    /// again starts over from a new root.
    fn refresh_review(&mut self, id: ReviewId, person: bool) {
        let Some(index) = self.reviews.index(id) else {
            return;
        };
        let running = &mut self.reviews.open[index];
        if running.revoked {
            let Some(root) = TrustedRoot::of(&self.editor, running.project) else {
                return self.editor.message(RESTRICTED);
            };
            running.root = root;
            running.revoked = false;
            running.view.tell(Tell::Said(Vec::new()));
            let scope = running.scope.clone();
            self.start_reviewer(index);
            return self.ask_review(index, Request::Open(scope));
        }
        if !self.editor.project_trusted(running.project) {
            return self.editor.message(RESTRICTED);
        }
        let mut first = std::mem::take(&mut self.reviews.shown);
        self.editor.shown_review_files(id, &mut first);
        self.ask_review(index, Request::Refresh { first, person });
        self.refresh_started(index);
    }

    /// Stops a closed review's threads, and frees the texts it left waiting.
    fn close_review(&mut self, id: ReviewId) {
        // Its sides no window shows go with it; one shown stays until the person moves on.
        if let Some(index) = self.reviews.index(id) {
            self.drop_unshown_sides(index, None);
        }
        match self.reviews.close(id) {
            Some(running) => {
                // What it held is saved at once (step 6).
                self.save_closed_review(&running);
                self.freer.free(running.texts);
            }
            // A review that never started its threads: its model is only freed.
            None => self.freer.free(self.editor.take_closed_review(id)),
        }
    }

    /// The project oso knows whose root is `root`, by a buffer in it or an open review.
    pub(super) fn project_with_root(&self, root: &std::path::Path) -> Option<ProjectId> {
        (self.editor.buffers())
            .filter_map(|(_, buffer)| buffer.project())
            .chain(self.reviews.open.iter().map(|running| running.project))
            .find(|project| {
                (self.editor.project(*project)).is_some_and(|known| known.root() == root)
            })
    }

    /// A review's threads sent something.
    pub(super) fn review_reported(&mut self, event: ReviewEvent) {
        match event {
            ReviewEvent::Report {
                review,
                serial,
                report,
            } => self.review_report(review, serial, report),
            ReviewEvent::Baseline {
                project,
                serial,
                report,
            } => self.baseline_report(project, serial, report),
            ReviewEvent::Text { review, text } => match self.reviews.index(review) {
                Some(index) => {
                    self.reviews.open[index].texts.push_back(text);
                    self.put_in_review_texts();
                }
                None => self.freer.free(text),
            },
            ReviewEvent::Counted(counted) => self.scope_counted(counted),
            ReviewEvent::Stored(stored) => self.review_stored(stored),
        }
    }

    fn review_report(&mut self, review: ReviewId, serial: u64, report: Report) {
        let Some(index) = self.reviews.current(review, serial) else {
            return self.freer.free(report);
        };
        // While its saved review is read, the model may not take a reading yet (step 6).
        let Err(report) = self.hold_review_report(index, serial, report) else {
            return;
        };
        let tell = match report {
            Report::Listed {
                entries, more, cut, ..
            } => {
                let whole = more == 0 && !cut;
                self.reviews.open[index].view.tell(Tell::Listed {
                    entries: Arc::clone(&entries),
                    more,
                    cut,
                });
                self.reviews.open[index].decisions.listing = Arc::clone(&entries);
                return self.forget_unlisted(index, &entries, whole);
            }
            Report::File { reading, .. } => return self.tell_file(index, reading),
            Report::Carried { carried, cut_short } => {
                return self.review_carried(index, *carried, cut_short);
            }
            Report::Read { stopped, .. } => {
                self.refresh_ended(index);
                self.reviews.open[index].read_once |= !stopped;
                Tell::Read
            }
            Report::Stale => return self.mark_review_stale(index),
            Report::Staged { ticket, staged } => return self.review_staged(index, ticket, staged),
            // A look nobody asked for tells nothing of its failure: the next `g` says why.
            Report::Failed {
                asked: Asked::Look,
                why,
            } => return log::debug!("a review's look failed: {}", one_line(&why, MAX_MESSAGE)),
            Report::Failed { asked, why } => {
                if matches!(asked, Asked::Open | Asked::Refresh) {
                    self.refresh_ended(index);
                }
                if asked == Asked::Carry {
                    self.carries_failed(index, false);
                } else {
                    // What `gh` said may be stale now: comments go to the review's own session.
                    self.reviews.open[index].pr = None;
                }
                let why = one_line(&why, MAX_MESSAGE);
                self.editor.message(format!("The review failed: {why}"));
                Tell::Said(vec![why])
            }
            Report::BaselineGone => {
                self.refresh_ended(index);
                let project = self.reviews.open[index].project;
                self.forget_baseline(project);
                let gone = "the session's baseline is gone: B takes another";
                self.editor.message(gone);
                Tell::Said(vec![gone.to_owned()])
            }
            Report::Baseline { .. } => return,
            Report::PrFound(found) => return self.review_pr_found(index, found),
            Report::Fetching { number, remote } => {
                self.editor.message(fetching_said(number, &remote));
                return self.request_redraw();
            }
            Report::Jumped(jumped) => return self.review_jumped(index, jumped),
        };
        self.reviews.open[index].view.tell(tell);
    }

    /// Keeps what `gh` said of a review's pull request, saying it once what is said of it
    /// changed.
    fn review_pr_found(&mut self, index: usize, found: PrFound) {
        let running = &mut self.reviews.open[index];
        let said = found_said(&found);
        let new = running
            .pr
            .as_ref()
            .is_none_or(|was| found_said(was) != said);
        running.pr = Some(found);
        if new {
            self.editor.message(said);
            self.request_redraw();
        }
    }

    /// Puts in each review's waiting texts, in order, while nothing holds them.
    fn put_in_review_texts(&mut self) {
        let mut put = false;
        for index in 0..self.reviews.open.len() {
            while !self.editor.review_waits() {
                let Some(text) = self.reviews.open[index].texts.pop_front() else {
                    break;
                };
                self.put_in_review_text(index, *text);
                put = true;
            }
        }
        if put {
            self.request_redraw();
        }
    }

    fn put_in_review_text(&mut self, index: usize, text: Text) {
        #[cfg(feature = "dev")]
        let began = std::time::Instant::now();
        let id = self.reviews.open[index].id;
        let Some(buffer) = self.editor.review_buffer(id) else {
            return self.freer.free(text);
        };
        #[cfg(feature = "dev")]
        let highlighted = text.highlighted;
        let Text {
            text,
            rows,
            highlights,
            panes,
            files,
            carried,
            reading,
            folded,
            pane_paint,
            ..
        } = text;
        self.reviews.open[index].shown_folded = folded;
        // Kept, at O(1), so that the text it replaces is freed off the UI thread, with the rows
        // and highlights it replaces.
        let before = self
            .editor
            .buffer(buffer)
            .map(|shown| (shown.text().clone(), shown.display()));
        let lines = watch::Lines::new(self.editor.review(id), &carried);
        let moved = |path: &[u8], side, number| lines.line_now(path, side, number);
        let left = match text {
            Some(text) => (self.editor).set_review_text(id, text, rows, highlights, &moved),
            None => self
                .editor
                .set_review_paint(id, rows, highlights)
                .unwrap_or_else(|left| {
                    log::warn!("a review's paint did not cover its text");
                    left
                }),
        };
        self.freer.free((before, left));
        self.put_in_review_panes(index, panes, pane_paint, &moved);
        if let Some(files) = files {
            self.put_in_review_files(index, files);
        }
        // The view builds the next text only now.
        self.reviews.open[index].view.taken();
        for (reading, carried) in carried {
            let Some(model) = self.editor.review_mut(id) else {
                break;
            };
            let applied = model.apply(carried);
            // The readings it let go are freed off the UI thread (step 4).
            let released = model.take_released();
            self.freer.free(released);
            self.carry_applied(index, reading, &applied);
        }
        self.editor.set_review_reading(id, reading);
        // The cursor saved goes back once a text shows its file (step 6).
        self.put_back_place(index);
        #[cfg(feature = "dev")]
        self.timings.review_text(crate::dev::ReviewText::of(
            reading,
            highlighted.map(|progress| (progress.shown, progress.whole)),
            began.elapsed(),
        ));
    }

    /// Revokes the roots of the reviews, baselines and counts of projects no longer trusted.
    fn revoke_untrusted_reviews(&mut self) {
        let editor = &self.editor;
        self.reviews
            .revoke_untrusted(|project| editor.project_trusted(project));
        self.revoke_untrusted_session_baselines();
        // And the looks for a session's changes (ADR 0121, step 3).
        self.revoke_untrusted_looks();
    }
}

mod baseline;
mod comments;
mod decide;
mod files;
mod handed;
mod highlight;
mod judge;
mod keep;
mod layout;
mod menu;
mod scope;
mod secrets;
mod send;
mod slot;
mod state;
mod store;
#[cfg(test)]
#[path = "reviews_tests.rs"]
mod tests;
mod visit;
mod watch;
