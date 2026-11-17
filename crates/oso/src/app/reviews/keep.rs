//! Keeping reviews across restarts (ADR 0122, "Persistence", step 6), on the UI thread: a
//! review opened reads what was saved of its project and scope before its own thread lists
//! anything, and is saved as it changes, when it closes, and before oso goes or restarts.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here reads or writes a
//! file: the store's thread does (`store`), and the model it built comes in at O(1).
//!
//! - A review's own thread is asked to open only once the saved review was read, so that the
//!   first readings carry the saved comments and marks as any refresh does.
//! - A change is saved once [`QUIET`] passed with no other, or [`AT_MOST`] after the first:
//!   the snapshot is references to the model's readings and comments, taken then, and written
//!   on the store's thread. A review closed is saved at once, from the model the editor hands
//!   back.
//! - What may hold secrets is withheld from the snapshot as the review message withholds it
//!   (step 12): a file whose name may, or whose open buffer may or is withheld from agents.
//! - A saved review that could not be read is never written over: the review says so, and
//!   `review-forget` removes it.
//! - While the store keeps a review, its comments not sent are not lost when it closes or oso
//!   goes, and nothing asks about them ([`Editor::set_review_kept`], Q-G).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_core::complete::Candidate;
use oso_core::editor::{ReviewChoice, ReviewId, ScopeKind};
use oso_core::review::{Author, Review};

use super::state::Running;
use super::store::{self, Found, Snapshot, Store, Stored};
use super::{ReviewEvent, scope_label};
use crate::app::{App, UserEvent};
use crate::commands::review::{MAX_NAME, RESTRICTED, SAVED_LIST, one_line};
use crate::git::TrustedRoot;
use crate::review::saved::{self, Header, Kept};
use crate::review::{Request, Scope};

/// How long after the last change a review is saved.
const QUIET: Duration = Duration::from_secs(1);
/// How long after the first change it is saved, however it goes on changing.
const AT_MOST: Duration = Duration::from_secs(5);

/// What a review keeps of its saving.
#[derive(Debug, Default)]
pub(super) struct Saving {
    /// The file it is saved as; `None` when it is not saved, as no store runs, or the file
    /// could not be read and must not be written over.
    pub(super) name: Option<String>,
    /// When it was opened, in milliseconds since the epoch.
    pub(super) opened: u64,
    /// The scope its own thread opens once the saved review was read.
    pub(super) waiting: Option<Scope>,
    /// The model's revision and the view's choices last handed to the store, and as last seen.
    handed: Option<(u64, ReviewChoice, bool)>,
    seen: Option<(u64, ReviewChoice, bool)>,
    /// When it first changed since, and last.
    first: Option<Instant>,
    last: Option<Instant>,
    /// Whether a write failed, said once until one succeeds.
    failed: bool,
}

/// The app's side of saved reviews: the store's thread, once started, and the saved reviews
/// as last listed.
#[derive(Default)]
pub(super) struct Keep {
    store: Option<Store>,
    /// Whether starting the store was tried: it is not tried again.
    tried: bool,
    /// Each saved review as last listed: its candidate's text, its file's name and header.
    listed: Vec<(String, String, Header)>,
}

impl App {
    /// The store, started the first time it is asked for, with a list of the saved reviews.
    fn review_store(&mut self) -> Option<&Store> {
        if !self.reviews.keep.tried {
            self.reviews.keep.tried = true;
            let waker = self.waker.clone();
            let tell = move |stored| {
                waker.send(UserEvent::Review(ReviewEvent::Stored(stored)));
            };
            match super::state().map(|state| Store::start(&state, tell)) {
                Some(Ok(store)) => {
                    store.list();
                    self.reviews.keep.store = Some(store);
                }
                Some(Err(e)) => log::warn!("reviews will not be saved: {e}"),
                None => log::info!("no state directory: reviews will not be saved"),
            }
        }
        self.reviews.keep.store.as_ref()
    }

    /// Starts the store after the first event, so that its sweep runs at start and
    /// `review-resume` has its list.
    pub(super) fn start_review_store(&mut self) {
        if !self.reviews.keep.tried {
            self.review_store();
        }
    }

    /// The review at `index` was just made: what was saved of its scope is read first, and
    /// its own thread asked to open `scope` once it is.
    pub(super) fn open_saved(&mut self, index: usize, scope: Scope) {
        let root = self.reviews.open[index].root.path().to_owned();
        let id = self.reviews.open[index].id;
        let name = root.to_str().map(|_| saved::file_name(&root, &scope));
        let asked = Header {
            root,
            scope: scope.clone(),
            opened: store::now(),
            comments: 0,
            unsent: 0,
        };
        let read = match name.clone() {
            Some(name) => (self.review_store()).is_some_and(|store| store.read(id, name, asked)),
            None => false,
        };
        let saving = &mut self.reviews.open[index].saving;
        saving.opened = store::now();
        if read {
            saving.name = name;
            saving.waiting = Some(scope);
        } else {
            self.ask_review(index, Request::Open(scope));
        }
    }

    /// The store found or did something.
    pub(super) fn review_stored(&mut self, stored: Stored) {
        match stored {
            Stored::Read { review, found } => self.saved_review_read(review, found),
            Stored::Written { review, written } => {
                let Some(index) = self.reviews.index(review) else {
                    return;
                };
                let saving = &mut self.reviews.open[index].saving;
                let say = !written && !saving.failed;
                saving.failed = !written;
                self.editor.set_review_kept(review, written);
                if say {
                    self.editor.message(
                        "The review could not be saved: C-x k and leaving oso ask about its comments",
                    );
                }
                if written {
                    self.list_saved_reviews();
                }
            }
            Stored::Listed(listed) => self.saved_reviews_listed(listed),
        }
    }

    /// What was saved of the review `id`'s scope came back: its model goes in, and its own
    /// thread opens the scope, whose first readings carry it.
    fn saved_review_read(&mut self, id: ReviewId, found: Result<Option<Box<Found>>, String>) {
        let Some(index) = self.reviews.index(id) else {
            return self.freer.free(found);
        };
        let Some(scope) = self.reviews.open[index].saving.waiting.take() else {
            return self.freer.free(found);
        };
        match found {
            Ok(found) => {
                self.editor.set_review_kept(id, true);
                if let Some(found) = found {
                    self.restore_review(index, *found);
                }
            }
            Err(why) => {
                // A file that may not be read must not be written over.
                self.reviews.open[index].saving.name = None;
                self.editor.message(format!(
                    "The saved review could not be read, and is not saved over: {}; \
                     M-x review-forget removes it",
                    one_line(&why, 120)
                ));
            }
        }
        self.ask_review(index, Request::Open(scope));
        self.request_redraw();
    }

    /// Puts `found`'s model in the review at `index`, which holds nothing yet, with the view's
    /// choices it kept.
    fn restore_review(&mut self, index: usize, found: Found) {
        let id = self.reviews.open[index].id;
        let Found {
            review,
            kept,
            left_out,
        } = found;
        if left_out > 0 {
            log::info!("{left_out} lines of a saved review were left out");
        }
        let comments = review.comments().count();
        let Some(model) = self.editor.review_mut(id) else {
            return self.freer.free(review);
        };
        let empty = std::mem::replace(model, review);
        self.freer.free(empty);
        self.editor.choose_review_layout(id, kept.choice);
        self.editor.fold_agents(id, kept.folded);
        // Nothing to save until it changes.
        let revision = self.editor.review(id).map_or(0, Review::revision);
        self.reviews.open[index].saving.handed = Some((revision, kept.choice, kept.folded));
        if comments > 0 {
            let noun = if comments == 1 { "comment" } else { "comments" };
            self.editor
                .message(format!("{comments} saved {noun} brought back"));
        }
    }

    /// After each event: notes which reviews changed since they were last handed to the store.
    /// Costs a look at each review's revision and choices.
    pub(super) fn sync_review_saves(&mut self) {
        let now = Instant::now();
        for index in 0..self.reviews.open.len() {
            let state = self.saving_state(index);
            let saving = &mut self.reviews.open[index].saving;
            if saving.name.is_none() || saving.waiting.is_some() || saving.handed == state {
                saving.first = None;
                saving.last = None;
                saving.seen = state;
                continue;
            }
            if saving.seen != state {
                saving.seen = state;
                saving.first.get_or_insert(now);
                saving.last = Some(now);
            }
        }
    }

    /// The review at `index`'s model revision and view choices, as they would be handed over.
    fn saving_state(&self, index: usize) -> Option<(u64, ReviewChoice, bool)> {
        let id = self.reviews.open[index].id;
        let model = self.editor.review(id)?;
        Some((
            model.revision(),
            self.editor.review_choice(id),
            self.editor.agents_folded(id),
        ))
    }

    /// When the next review is due to be saved.
    pub(super) fn review_save_deadline(&self) -> Option<Instant> {
        (self.reviews.open.iter())
            .filter_map(|running| due_at(&running.saving))
            .min()
    }

    /// Saves each review whose changes settled.
    pub(super) fn review_saves_due(&mut self) {
        let now = Instant::now();
        for index in 0..self.reviews.open.len() {
            if due_at(&self.reviews.open[index].saving).is_some_and(|due| due <= now) {
                self.save_review(index);
            }
        }
    }

    /// Hands the review at `index`, as it is now, to the store.
    fn save_review(&mut self, index: usize) {
        let state = self.saving_state(index);
        let id = self.reviews.open[index].id;
        let Some(snapshot) = self.snapshot(&self.reviews.open[index], self.editor.review(id))
        else {
            return;
        };
        let Some(name) = self.reviews.open[index].saving.name.clone() else {
            return;
        };
        let handed =
            (self.review_store()).is_some_and(|store| store.write(Some(id), name, snapshot));
        let saving = &mut self.reviews.open[index].saving;
        saving.first = None;
        saving.last = None;
        if handed {
            saving.handed = state;
        }
    }

    /// What `running`, whose model is `model`, is saved as, if it is saved.
    fn snapshot(&self, running: &Running, model: Option<&Review>) -> Option<Snapshot> {
        let saving = &running.saving;
        if saving.name.is_none() || saving.waiting.is_some() {
            return None;
        }
        let model = model?;
        let saved = model.save();
        let root = running.root.path();
        // A file may hold secrets by its name, or as an open buffer of it does, as the review
        // message judges it (step 12).
        let held: HashSet<PathBuf> = (self.editor.buffers())
            .filter(|(_, buffer)| buffer.is_secret() || buffer.verdict().is_withheld())
            .filter_map(|(_, buffer)| buffer.path().map(PathBuf::from))
            .collect();
        let withheld: HashSet<Box<[u8]>> = (saved.files.iter().map(|file| &*file.path))
            .chain(saved.comments.iter().map(|comment| &*comment.path))
            .filter(|path| {
                crate::git::may_hold_secrets(root, path)
                    || crate::git::os_path(path).is_ok_and(|path| held.contains(&root.join(path)))
            })
            .map(Box::from)
            .collect();
        let unsent = (saved.comments.iter())
            .filter(|comment| comment.author == Author::Person && !comment.sent)
            .count();
        let header = Header {
            root: root.to_owned(),
            scope: running.scope.clone(),
            opened: saving.opened,
            comments: saved.comments.len(),
            unsent,
        };
        let kept = Kept {
            choice: self.editor.review_choice(running.id),
            folded: self.editor.agents_folded(running.id),
            place: None,
        };
        Some(Snapshot {
            header,
            saved,
            kept,
            withheld,
        })
    }

    /// The review `running` closed: what it held is saved at once, from the model the editor
    /// handed back, which the store's thread then frees.
    pub(super) fn save_closed_review(&mut self, running: &Running) {
        let model = self.editor.take_closed_review(running.id);
        let snapshot = self.snapshot(running, model.as_ref());
        // The view's choices went with the review: those last handed stand.
        let snapshot = snapshot.map(|mut snapshot| {
            if let Some((_, choice, folded)) = running.saving.handed {
                snapshot.kept.choice = choice;
                snapshot.kept.folded = folded;
            }
            snapshot
        });
        let changed = model.as_ref().map(Review::revision) != running.saving.handed.map(|h| h.0);
        if let (Some(snapshot), Some(name), true) = (snapshot, running.saving.name.clone(), changed)
            && let Some(store) = self.review_store()
        {
            store.write(None, name, snapshot);
        }
        self.freer.free(model);
    }

    /// Hands every review that changed to the store, before oso goes or a restart's new
    /// instance reads them; [`App::wait_review_saves`] then waits for them.
    pub(in crate::app) fn hand_review_saves(&mut self) {
        for index in 0..self.reviews.open.len() {
            if self.saving_state(index) != self.reviews.open[index].saving.handed {
                self.save_review(index);
            }
        }
    }

    /// Waits at most `wait` until what was handed to the store is written.
    pub(in crate::app) fn wait_review_saves(&self, wait: Duration) {
        if let Some(store) = &self.reviews.keep.store
            && !store.flush(wait)
        {
            log::warn!("the reviews were not saved within {wait:?}");
        }
    }

    /// Asks the store for the saved reviews again.
    fn list_saved_reviews(&mut self) {
        if let Some(store) = &self.reviews.keep.store {
            store.list();
        }
    }

    /// The saved reviews, as the store listed them, are `review-resume`'s candidates: each its
    /// scope and root, with its comments and when it was opened.
    fn saved_reviews_listed(&mut self, listed: Vec<(String, Header)>) {
        let now = store::now();
        let mut candidates = Vec::with_capacity(listed.len());
        let mut kept = Vec::with_capacity(listed.len());
        for (name, header) in listed {
            let root = one_line(&header.root.to_string_lossy(), 2 * MAX_NAME);
            let text = format!("{} {root}", scope_label(&header.scope));
            let comments = match header.comments {
                1 => "1 comment".to_owned(),
                n => format!("{n} comments"),
            };
            let days = now.saturating_sub(header.opened) / (24 * 60 * 60 * 1000);
            let age = match days {
                0 => "today".to_owned(),
                1 => "1 day ago".to_owned(),
                n => format!("{n} days ago"),
            };
            let annotation = format!("{comments}, {} unsent \u{b7} {age}", header.unsent);
            candidates.push(Candidate::new(text.as_str(), "", annotation));
            kept.push((text, name, header));
        }
        self.editor.set_listed(SAVED_LIST, Arc::from(candidates));
        self.reviews.keep.listed = kept;
    }

    /// `review-resume` picked `pick`: the saved review opens again in its project, once oso
    /// knows the project and trusts it.
    pub(super) fn resume_review(&mut self, pick: &str) {
        let Some((_, _, header)) =
            (self.reviews.keep.listed.iter()).find(|(text, ..)| text == pick)
        else {
            return self.editor.message("No such saved review");
        };
        let header = header.clone();
        let project = (self.editor.buffers())
            .filter_map(|(_, buffer)| buffer.project())
            .chain(self.reviews.open.iter().map(|running| running.project))
            .find(|project| {
                (self.editor.project(*project)).is_some_and(|known| known.root() == header.root)
            });
        let Some(project) = project else {
            return self.editor.message(format!(
                "oso does not know that project yet: open a file of {} first",
                one_line(&header.root.to_string_lossy(), 2 * MAX_NAME)
            ));
        };
        if TrustedRoot::of(&self.editor, project).is_none() {
            return self.editor.message(RESTRICTED);
        }
        // A session review compares with the baseline taken in this run, else the saved one.
        if let Scope::Session { baseline } = &header.scope
            && self.reviews.baseline(project).is_none()
        {
            self.reviews.baselines.push((project, baseline.clone()));
        }
        let (kind, revision) = kind_and_revision(&header.scope);
        self.open_review(project, kind, revision);
    }

    /// `review-forget` picked `pick`: the saved review is removed now.
    pub(super) fn forget_review(&mut self, pick: &str) {
        let Some(index) = (self.reviews.keep.listed.iter()).position(|(text, ..)| text == pick)
        else {
            return self.editor.message("No such saved review");
        };
        let (_, name, _) = self.reviews.keep.listed.remove(index);
        if let Some(store) = &self.reviews.keep.store {
            store.remove(name);
            store.list();
        }
        self.editor.message("The saved review was removed");
    }
}

/// What a review request names `scope` by: its kind, and the revision as the person gave it.
fn kind_and_revision(scope: &Scope) -> (ScopeKind, Option<String>) {
    match scope {
        Scope::Session { .. } => (ScopeKind::Session, None),
        Scope::Uncommitted => (ScopeKind::Uncommitted, None),
        Scope::Staged => (ScopeKind::Staged, None),
        Scope::Unstaged => (ScopeKind::Unstaged, None),
        Scope::Branch { base } => (ScopeKind::Branch, Some(base.clone())),
        Scope::Commit { revision } => (ScopeKind::Commit, Some(revision.clone())),
        Scope::Range { from, to } => (ScopeKind::Range, Some(format!("{from}..{to}"))),
    }
}

/// When `saving`'s changes are due to be saved, if any wait.
fn due_at(saving: &Saving) -> Option<Instant> {
    let (first, last) = (saving.first?, saving.last?);
    Some((last + QUIET).min(first + AT_MOST))
}

#[cfg(test)]
#[path = "keep_tests.rs"]
mod tests;
