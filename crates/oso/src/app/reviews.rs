//! Reviews in the app (ADR 0122): what review commands ask of the host, carried out on the UI
//! thread. Each open review has its own `oso-review` thread, which runs git and diffs, and its
//! own `oso-review-view` thread, which lays out the review buffer's text; the review menu's
//! counts come from `oso-review-count`. What they report is put in here, in order.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git, reads a
//! file, diffs or builds a review's text: a text comes whole from its view thread, goes in at
//! O(1), and the text it replaces is freed off the UI thread. Nothing here logs a file's text.

use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oso_core::editor::{ReviewId, ReviewRequest, ScopeKind};
use oso_core::project::ProjectId;

use super::{App, UserEvent};
use crate::commands::review::{MAX_NAME, RESTRICTED, one_line};
use crate::review::count::Counted;
use crate::review::view::{Tell, Text, View};
use crate::review::{Asked, Content, Id, Reading, Report, Request, Reviewer, Scope, TrustedRoot};
pub(in crate::app) use state::Reviews;
use state::{Baseliner, Reopen, Running};

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
        }
    }
}

/// oso's own directory in the per-user state directory, where reviews keep their index
/// copies. Asks the environment and not the disk. A session's start baseline is taken there
/// too (ADR 0121).
pub(in crate::app) fn state() -> Option<PathBuf> {
    oso_platform::privacy::state_home().map(|home| home.join("oso"))
}

/// Seconds since the Unix epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |now| now.as_secs())
}

/// What a review's name and title call its scope.
fn scope_label(scope: &Scope) -> String {
    let label = match scope {
        Scope::Session { .. } => "session".to_owned(),
        Scope::Uncommitted => "uncommitted".to_owned(),
        Scope::Unstaged => "unstaged".to_owned(),
        Scope::Staged => "staged".to_owned(),
        Scope::Branch { base } => format!("branch {base}"),
        Scope::Commit { revision } => format!("commit {revision}"),
        Scope::Range { from, to } => format!("range {from}..{to}"),
    };
    one_line(&label, MAX_NAME)
}

/// Whether `a` and `b` review the same thing: the session scope whatever its baseline.
fn same_scope(a: &Scope, b: &Scope) -> bool {
    matches!((a, b), (Scope::Session { .. }, Scope::Session { .. })) || a == b
}

/// The scope a request names, or why it cannot be reviewed.
fn scope_of(
    scope: ScopeKind,
    revision: Option<String>,
    baseline: Option<Id>,
) -> Result<Scope, &'static str> {
    let revision = || revision.clone().ok_or("No revision was named");
    Ok(match scope {
        ScopeKind::Session => Scope::Session {
            baseline: baseline.ok_or("No baseline: B takes one")?,
        },
        ScopeKind::Uncommitted => Scope::Uncommitted,
        ScopeKind::Unstaged => Scope::Unstaged,
        ScopeKind::Staged => Scope::Staged,
        ScopeKind::Branch => Scope::Branch { base: revision()? },
        ScopeKind::Commit => Scope::Commit {
            revision: revision()?,
        },
        ScopeKind::Range => {
            let range = revision()?;
            let (from, to) = range.split_once("..").ok_or("A range is FROM..TO")?;
            Scope::Range {
                from: from.to_owned(),
                to: to.to_owned(),
            }
        }
        ScopeKind::PullRequest => return Err("Pull requests cannot be reviewed yet"),
    })
}

impl App {
    /// After commands and after each event: revokes the roots of projects no longer trusted,
    /// first, so that nothing asked runs git for one, then carries out what review commands
    /// asked, starts or stops the menu's counts, and puts in the texts that wait while nothing
    /// holds them.
    pub(super) fn sync_reviews(&mut self) {
        self.revoke_untrusted_reviews();
        // Before the requests, so that a head that closed the menu, as `b` does, stops the
        // menu's counts before a request starts its own.
        self.sync_review_menu();
        for request in self.editor.take_review_requests() {
            self.review_request(request);
        }
        self.put_in_review_texts();
        self.sync_review_comments();
    }

    fn review_request(&mut self, request: ReviewRequest) {
        match request {
            ReviewRequest::Open {
                project,
                scope,
                revision,
            } => self.open_review(project, scope, revision),
            ReviewRequest::Baseline { project } => self.take_baseline(project),
            ReviewRequest::Refresh(id) => self.refresh_review(id),
            ReviewRequest::Close(id) => self.close_review(id),
            ReviewRequest::Redraw { review, path } => self.redraw_comments(review, &path),
            // Visits come with a later step of ADR 0122.
            ReviewRequest::Visit { .. } => {
                log::debug!("a review request this build does not carry out yet");
            }
        }
    }

    /// Opens a review of `scope` in `project`, or shows the one open and reads it again.
    fn open_review(&mut self, project: ProjectId, kind: ScopeKind, revision: Option<String>) {
        if kind == ScopeKind::Branch && revision.is_none() {
            // The base is not known yet: it is found first, and the review opens with it.
            return self.find_branch_base(project);
        }
        let baseline = self.reviews.baseline(project);
        let scope = match scope_of(kind, revision, baseline) {
            Ok(scope) => scope,
            Err(why) => return self.editor.message(why),
        };
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            return self.editor.message(RESTRICTED);
        };
        self.editor.set_last_scope(project, kind);
        let window = self.editor.tiled().0.id();
        let open =
            self.reviews.open.iter().position(|running| {
                running.project == project && same_scope(&running.scope, &scope)
            });
        if let Some(index) = open {
            let id = self.reviews.open[index].id;
            if let Some(buffer) = self.editor.review_buffer(id) {
                self.editor.show_in(window, buffer);
            }
            let running = &mut self.reviews.open[index];
            let reopen = running.reopen(&scope);
            // A new baseline: the session review compares with it from now on.
            running.scope = scope.clone();
            match reopen {
                // A revoked review starts over with the scope, as `g` would.
                Reopen::Refresh | Reopen::Restart => self.refresh_review(id),
                Reopen::Open => self.ask_review(index, Request::Open(scope)),
            }
            self.request_redraw();
            return;
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
        self.editor.show_in(window, buffer);
        self.reviews.open.push(Running {
            id,
            project,
            scope: scope.clone(),
            root,
            serial: 0,
            reviewer: None,
            view,
            texts: VecDeque::new(),
            revoked: false,
            carrying: VecDeque::new(),
            folded: false,
        });
        let index = self.reviews.open.len() - 1;
        self.start_reviewer(index);
        self.ask_review(index, Request::Open(scope));
        self.request_redraw();
    }

    /// Starts the review's own thread over its root, dropping the one before.
    fn start_reviewer(&mut self, index: usize) {
        // What the thread before was carrying is carried on the view thread.
        self.carries_failed(index, true);
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

    /// Reads the review's files again, those its windows show first. A review whose project
    /// was trusted again starts over from a new root.
    fn refresh_review(&mut self, id: ReviewId) {
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
        self.ask_review(index, Request::Refresh { first });
    }

    /// Stops a closed review's threads, and frees the texts it left waiting.
    fn close_review(&mut self, id: ReviewId) {
        if let Some(running) = self.reviews.close(id) {
            self.freer.free(running.texts);
        }
    }

    /// Takes a baseline of `project`'s work tree on a thread of its own.
    fn take_baseline(&mut self, project: ProjectId) {
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            return self.editor.message(RESTRICTED);
        };
        let waker = self.waker.clone();
        let serial = self.reviews.serial();
        let started = Reviewer::start(root.clone(), state(), move |report| {
            waker.send(UserEvent::Review(ReviewEvent::Baseline {
                project,
                serial,
                report,
            }))
        });
        match started {
            Ok(reviewer) => {
                reviewer.ask(Request::Baseline);
                self.reviews.add_baseliner(Baseliner {
                    project,
                    serial,
                    root,
                    _reviewer: reviewer,
                });
            }
            Err(e) => self.editor.message(format!("Cannot take a baseline: {e}")),
        }
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
        }
    }

    fn review_report(&mut self, review: ReviewId, serial: u64, report: Report) {
        let Some(index) = self.reviews.current(review, serial) else {
            return self.freer.free(report);
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
                return self.forget_unlisted(index, &entries, whole);
            }
            Report::File { reading, .. } => return self.tell_file(index, reading),
            Report::Carried { carried, cut_short } => {
                return self.review_carried(index, carried, cut_short);
            }
            Report::Read { .. } => Tell::Read,
            Report::Failed { asked, why } => {
                if asked == Asked::Carry {
                    self.carries_failed(index, false);
                }
                let why = one_line(&why, MAX_MESSAGE);
                self.editor.message(format!("The review failed: {why}"));
                Tell::Said(vec![why])
            }
            Report::BaselineGone => {
                let project = self.reviews.open[index].project;
                self.forget_baseline(project);
                let gone = "the session's baseline is gone: B takes another";
                self.editor.message(gone);
                Tell::Said(vec![gone.to_owned()])
            }
            Report::Baseline { .. } => return,
        };
        self.reviews.open[index].view.tell(tell);
    }

    fn baseline_report(&mut self, project: ProjectId, serial: u64, report: Report) {
        // A report of a thread replaced or dropped since, or taken as the trust was taken
        // back, is not used.
        if !self.reviews.take_baseliner(project, serial) || !self.editor.project_trusted(project) {
            return self.freer.free(report);
        }
        match report {
            Report::Baseline { tree, left_out } => {
                self.keep_baseline(project, tree);
                self.editor.message(if left_out == 0 {
                    "Baseline taken: D reviews what changed since".to_owned()
                } else {
                    format!(
                        "Baseline taken, {left_out} files left out: D reviews what changed since"
                    )
                });
            }
            Report::Failed { why, .. } => {
                let why = one_line(&why, MAX_MESSAGE);
                self.editor
                    .message(format!("Cannot take a baseline: {why}"));
            }
            other => self.freer.free(other),
        }
        self.request_redraw();
    }

    /// The comments of the review's file to carry to `reading`, for a text reading.
    fn carry_for(
        &self,
        review: ReviewId,
        reading: &Arc<Reading>,
    ) -> Option<oso_core::review::Carry> {
        let Content::Text { shape, .. } = &reading.content else {
            return None;
        };
        let model = self.editor.review(review)?;
        Some(model.carry(&reading.entry.path, Arc::clone(shape)))
    }

    /// `tree` is `project`'s baseline from now on: the session review compares with it.
    fn keep_baseline(&mut self, project: ProjectId, tree: Id) {
        self.reviews.baselines.retain(|(id, _)| *id != project);
        self.reviews.baselines.push((project, tree));
        self.editor.set_baseline_taken(project, Some(now()));
        self.count_session(project);
    }

    /// A hosted session's start thread took `tree` as `project`'s baseline before its
    /// `claude` ran (ADR 0121, ADR 0122): the review takes it as if `B` had, while the
    /// project is still trusted.
    pub(in crate::app) fn session_baseline_taken(&mut self, project: ProjectId, tree: &str) {
        if !self.editor.project_trusted(project) {
            return;
        }
        if let Some(tree) = Id::parse(tree.as_bytes()) {
            self.keep_baseline(project, tree);
        }
    }

    fn forget_baseline(&mut self, project: ProjectId) {
        self.reviews.baselines.retain(|(id, _)| *id != project);
        self.editor.set_baseline_taken(project, None);
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
        let id = self.reviews.open[index].id;
        let Some(buffer) = self.editor.review_buffer(id) else {
            return self.freer.free(text);
        };
        let Text {
            text,
            rows,
            highlights,
            carried,
            reading,
        } = text;
        // Kept, at O(1), so that the text it replaces is freed off the UI thread, with the rows
        // and highlights it replaces.
        let before = self
            .editor
            .buffer(buffer)
            .map(|shown| (shown.text().clone(), shown.display()));
        let left = match text {
            Some(text) => self.editor.set_review_text(id, text, rows, highlights),
            None => self
                .editor
                .set_review_paint(id, rows, highlights)
                .unwrap_or_else(|left| {
                    log::warn!("a review's paint did not cover its text");
                    left
                }),
        };
        self.freer.free((before, left));
        // The view builds the next text only now.
        self.reviews.open[index].view.taken();
        for (reading, carried) in carried {
            let Some(model) = self.editor.review_mut(id) else {
                break;
            };
            let applied = model.apply(carried);
            let again = applied.again;
            // The view placed the file's cards by this carry, as the model now does, unless
            // some went outdated, which the model may say otherwise, or came meanwhile.
            if applied.outdated > 0 || again {
                self.tell_cards(index, &reading.entry.path);
            }
            if again {
                self.carry_again(index, reading);
            }
        }
        self.editor.set_review_reading(id, reading);
        #[cfg(feature = "dev")]
        self.timings.review_text(crate::dev::ReviewText {
            read: reading.map_or(0, |(read, _)| read),
            listed: reading.map_or(0, |(_, listed)| listed),
            done: reading.is_none(),
        });
    }

    /// Revokes the roots of the reviews, baselines and counts of projects no longer trusted.
    fn revoke_untrusted_reviews(&mut self) {
        let editor = &self.editor;
        self.reviews
            .revoke_untrusted(|project| editor.project_trusted(project));
        self.revoke_untrusted_session_baselines();
    }
}

mod comments;
mod menu;
mod state;
#[cfg(test)]
#[path = "reviews_tests.rs"]
mod tests;
