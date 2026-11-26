//! Keeping reviews across restarts (ADR 0122, "Persistence", step 6), on the UI thread: a
//! review opened reads what was saved of its project and scope while its own thread lists and
//! reads, and is saved as it changes, when it closes, and before oso goes or restarts.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here reads or writes a
//! file: the store's thread does (`store`), and the model it built comes in at O(1).
//!
//! - A review's own thread opens at once, but what it reports waits until the saved review was
//!   read and its model went in, so that the first readings carry the saved comments and marks
//!   as any refresh does.
//! - A change of what a save holds is saved once [`QUIET`] passed with no other, or
//!   [`AT_MOST`] after the first: the snapshot is references to the model's readings and
//!   comments, taken then, and written on the store's thread, which judges what may hold
//!   secrets. A review closed is saved at once, from the model the editor hands back, if it
//!   changed.
//! - What may hold secrets is withheld as the review message withholds it (`secrets`), by the
//!   buffers seen to hold secrets at any time while a review was open, not only at the save.
//! - A saved review that could not be read is never written over: the review says so, and
//!   `review-forget` removes it.
//! - While the store keeps a review, its comments not sent are not lost when it closes or oso
//!   goes, and nothing asks about them ([`Editor::set_review_kept`], Q-G). A write that fails,
//!   that of a review closed too, says so, and a review whose store ended is kept no longer.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_core::editor::{ReviewChoice, ReviewId};
use oso_core::review::{Author, Filter, Review};

use super::ReviewEvent;
use super::state::Running;
use super::store::{self, Found, Snapshot, Store, Stored, Told};
use crate::app::{App, UserEvent};
use crate::commands::review::one_line;
use crate::review::saved::{self, Header, Place, SavedView};
use crate::review::{Report, Scope};

mod left;
mod list;
mod place;

pub(super) use left::Continue;

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
    /// Whether the saved review is being read: its thread's reports wait in `held`.
    pub(super) waiting: bool,
    held: Vec<(u64, Report)>,
    /// The model's revision and the view's choices last handed to the store, and as last seen.
    handed: Option<(u64, Choices)>,
    seen: Option<(u64, Choices)>,
    /// When it first changed since, and last.
    first: Option<Instant>,
    last: Option<Instant>,
    /// Whether a write failed, said once until one succeeds.
    failed: bool,
    /// Where the cursor was when it was saved, put back once a text shows it.
    pub(super) place: Option<Place>,
    /// Where the cursor was as it was last handed to the store, which a review closed keeps.
    place_handed: Option<Place>,
    /// The files with restored comments the view was not told of yet.
    pub(super) untold: HashSet<Box<[u8]>>,
    /// When the person left it last, in milliseconds since the epoch, in which space, and the
    /// stamp last handed to the store (ADR 0184).
    pub(super) left: u64,
    pub(super) space: Option<String>,
    left_handed: u64,
    /// Where the cursor was when the selected window last showed it, or when it was last
    /// handed over, which a review closed or continued puts back.
    place_seen: Option<Place>,
    /// The pull request whose review opens instead, if this commit's review, continued from
    /// its saved file, finds the commit gone (ADR 0184).
    pub(super) instead: Option<u64>,
}

/// What a review's view keeps besides its cursor: its layout chosen, agent cards folded, its
/// filter, and whether its files pane shows (step 6b). A filter's clone is O(1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Choices {
    choice: ReviewChoice,
    folded: bool,
    filter: Option<Filter>,
    files: bool,
    /// How often a pull request's last review moved, which is saved when it does (ADR 0167,
    /// step 2).
    checkpoint: u64,
}

/// The app's side of saved reviews: the store's thread, once started, the saved reviews as
/// last listed, and the files seen held as secret while a review was open.
#[derive(Default)]
pub(super) struct Keep {
    store: Option<Store>,
    /// Whether starting the store was tried: it is not tried again.
    tried: bool,
    /// Each saved review as last listed: its candidate's text, its file's name and header.
    listed: Vec<(String, String, Header)>,
    /// The files of buffers seen to hold secrets, or withheld from agents, while a review was
    /// open: once seen, always withheld from what is saved, in this run.
    held: Arc<HashSet<PathBuf>>,
    /// The review closed in this run that was left last, the review the selected window
    /// showed at the last event, and whether the store listed the saved ones yet (ADR 0184).
    left: Option<left::Left>,
    front: Option<left::Front>,
    pub(super) listed_once: bool,
}

impl App {
    /// The store, started the first time it is asked for.
    fn review_store(&mut self) -> Option<&Store> {
        if !self.reviews.keep.tried {
            self.reviews.keep.tried = true;
            let waker = self.waker.clone();
            let tell = move |stored| {
                waker.send(UserEvent::Review(ReviewEvent::Stored(stored)));
            };
            match super::state().map(|state| Store::start(&state, tell)) {
                Some(Ok(store)) => self.reviews.keep.store = Some(store),
                Some(Err(e)) => log::warn!("reviews will not be saved: {e}"),
                None => log::info!("no state directory: reviews will not be saved"),
            }
        }
        self.reviews.keep.store.as_ref()
    }

    /// Starts the store after the first event, so that its sweep runs as oso starts, once it is
    /// idle, and `review-resume` has its list.
    pub(super) fn start_review_store(&mut self) {
        if !self.reviews.keep.tried {
            self.review_store();
        }
    }

    /// The review at `index` was just made, its own thread asked to open: what was saved of
    /// `scope` is read meanwhile, and what the thread reports waits for it.
    pub(super) fn open_saved(&mut self, index: usize, scope: &Scope) {
        let root = self.reviews.open[index].root.path().to_owned();
        let id = self.reviews.open[index].id;
        let name = root.to_str().map(|_| saved::file_name(&root, scope));
        let asked = Header {
            root,
            scope: scope.clone(),
            opened: store::now(),
            comments: 0,
            unsent: 0,
            pull: None,
            checkpoint: None,
            left: None,
            space: None,
        };
        let read = match name.clone() {
            Some(name) => (self.review_store()).is_some_and(|store| store.read(id, name, asked)),
            None => false,
        };
        let saving = &mut self.reviews.open[index].saving;
        saving.opened = store::now();
        if read {
            saving.name = name;
            saving.waiting = true;
        }
    }

    /// Keeps `report` of the review at `index` while its saved review is being read; false
    /// when it is not, and the report is the caller's to carry out.
    pub(super) fn hold_review_report(
        &mut self,
        index: usize,
        serial: u64,
        report: Report,
    ) -> Result<(), Report> {
        let saving = &mut self.reviews.open[index].saving;
        if !saving.waiting {
            return Err(report);
        }
        saving.held.push((serial, report));
        Ok(())
    }

    /// The store found or did something.
    pub(super) fn review_stored(&mut self, stored: Stored) {
        match stored {
            Stored::Read { review, found } => self.saved_review_read(review, found),
            Stored::Written { told, written } => self.review_written(told, written),
            Stored::Listed(listed) => self.saved_reviews_listed(listed),
        }
    }

    /// A write `told` is about went to disk, or did not.
    fn review_written(&mut self, told: Told, written: bool) {
        match told {
            Told::Review(review) => {
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
            }
            Told::Closed { unsent } if !written && unsent > 0 => {
                let comments = if unsent == 1 { "comment" } else { "comments" };
                self.editor.message(format!(
                    "The review closed could not be saved: {unsent} unsent {comments} of yours \
                     are lost, as the log says why"
                ));
            }
            Told::Closed { .. } => {}
        }
    }

    /// What was saved of the review `id`'s scope came back: its model goes in, and what its own
    /// thread reported meanwhile is carried out, its first readings carrying the model.
    fn saved_review_read(&mut self, id: ReviewId, found: Result<Option<Box<Found>>, String>) {
        let Some(index) = self.reviews.index(id) else {
            return self.freer.free(found);
        };
        if !self.reviews.open[index].saving.waiting {
            return self.freer.free(found);
        }
        self.reviews.open[index].saving.waiting = false;
        match found {
            Ok(found) => {
                self.editor.set_review_kept(id, true);
                if let Some(found) = found {
                    self.restore_review(index, *found);
                }
            }
            Err(why) => {
                // A file that may not be read, or is another review's, is not written over.
                self.reviews.open[index].saving.name = None;
                self.editor.message(format!(
                    "The saved review could not be read, and is not saved over: {}; \
                     M-x review-forget removes it",
                    one_line(&why, 120)
                ));
            }
        }
        let held = std::mem::take(&mut self.reviews.open[index].saving.held);
        for (serial, report) in held {
            self.review_report(id, serial, report);
        }
        self.request_redraw();
    }

    /// Puts `found`'s model in the review at `index`, which holds nothing yet, with the view's
    /// choices it kept.
    fn restore_review(&mut self, index: usize, found: Found) {
        let id = self.reviews.open[index].id;
        let Found {
            review,
            view,
            left_out,
            pull,
            checkpoint,
            left,
            space,
        } = found;
        // A pull request's review: the person's last review of it (step 2).
        if checkpoint.is_some() {
            self.reviews.open[index].checkpoint = checkpoint;
        }
        // A commit's review stepped to from a pull request: linked again once that pull
        // request's commits are listed (ADR 0167).
        if pull.is_some() {
            self.want_pull(index, pull);
        }
        let comments = review.comments().count();
        let Some(model) = self.editor.review_mut(id) else {
            return self.freer.free(review);
        };
        let empty = std::mem::replace(model, review);
        self.freer.free(empty);
        // The view draws a file's cards from what it is told of its comments: each file's once,
        // as its first reading comes, or once a listing says it left (`comments`).
        let untold: HashSet<Box<[u8]>> = (self.editor.review(id).into_iter())
            .flat_map(Review::comments)
            .map(|comment| Box::from(comment.path()))
            .collect();
        self.reviews.open[index].saving.untold = untold;
        self.editor.choose_review_layout(id, view.choice);
        self.editor.fold_agents(id, view.folded);
        self.editor.set_review_filter(id, view.filter.clone());
        if view.files {
            self.show_files_again(id);
        }
        let checkpoint = self.reviews.open[index].checkpoint_moves;
        // Nothing to save until it changes.
        let revision = self.editor.review(id).map_or(0, Review::revision);
        let saving = &mut self.reviews.open[index].saving;
        saving.place_handed.clone_from(&view.place);
        saving.place = view.place;
        let choices = Choices {
            choice: view.choice,
            folded: view.folded,
            filter: view.filter,
            files: view.files,
            checkpoint,
        };
        saving.handed = Some((revision, choices));
        // When it was left before, unless it was left since it opened (ADR 0184).
        if let Some(left) = left.filter(|left| *left > saving.left) {
            saving.left = left;
            saving.space = space;
            saving.left_handed = left;
        }
        let noun = if comments == 1 { "comment" } else { "comments" };
        if left_out > 0 {
            log::info!("{left_out} lines of a saved review were left out");
            self.editor.message(format!(
                "{comments} saved {noun} brought back; {left_out} lines of the saved review \
                 could not be read and are left out"
            ));
        } else if comments > 0 {
            self.editor
                .message(format!("{comments} saved {noun} brought back"));
        }
    }

    /// After each event: notes which reviews changed since they were last handed to the store,
    /// and the buffers that hold secrets now. Costs a look at each review's revision and
    /// choices, and at the buffers while a review is open.
    pub(super) fn sync_review_saves(&mut self) {
        if !self.reviews.open.is_empty() {
            self.note_held_buffers();
            self.sync_review_front();
        }
        let now = Instant::now();
        for index in 0..self.reviews.open.len() {
            let state = self.saving_state(index);
            let saving = &mut self.reviews.open[index].saving;
            // A stamp of leaving alone is saved too (ADR 0184).
            let stamped = saving.left != saving.left_handed;
            if saving.name.is_none() || saving.waiting || (saving.handed == state && !stamped) {
                saving.first = None;
                saving.last = None;
                saving.seen = state;
                continue;
            }
            if saving.seen != state || (stamped && saving.first.is_none()) {
                saving.seen = state;
                saving.first.get_or_insert(now);
                saving.last = Some(now);
            }
        }
    }

    /// Adds the files of buffers that hold secrets now to those seen to.
    fn note_held_buffers(&mut self) {
        let held = super::secrets::held_paths(&self.editor);
        if held
            .iter()
            .all(|path| self.reviews.keep.held.contains(path))
        {
            return;
        }
        let mut seen = (*self.reviews.keep.held).clone();
        seen.extend(held);
        self.reviews.keep.held = Arc::new(seen);
    }

    /// The review at `index`'s model revision and view choices, as they would be handed over.
    fn saving_state(&self, index: usize) -> Option<(u64, Choices)> {
        let id = self.reviews.open[index].id;
        let model = self.editor.review(id)?;
        let mut choices = self.choices(id);
        choices.checkpoint = self.reviews.open[index].checkpoint_moves;
        Some((model.revision(), choices))
    }

    /// The review `id`'s view choices now.
    fn choices(&self, id: ReviewId) -> Choices {
        Choices {
            choice: self.editor.review_choice(id),
            folded: self.editor.agents_folded(id),
            filter: self.editor.review_filter(id).cloned(),
            files: self.editor.review_files(id).is_some(),
            checkpoint: 0,
        }
    }

    /// Shows the files pane of the review `id` brought back at the left of the first window
    /// showing it, selecting nothing; none while no window shows it or there is no room.
    fn show_files_again(&mut self, id: ReviewId) {
        let review = self.editor.review_buffer(id);
        let window = (self.editor.windows())
            .find(|window| Some(window.buffer()) == review)
            .map(|window| window.id());
        if let Some(window) = window {
            let _ = self.editor.show_review_files(id, window);
        }
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

    /// Hands the review at `index`, as it is now, to the store; whether it was handed.
    fn save_review(&mut self, index: usize) -> bool {
        let state = self.saving_state(index);
        let id = self.reviews.open[index].id;
        let Some(name) = self.reviews.open[index].saving.name.clone() else {
            return false;
        };
        let Some(snapshot) = self.snapshot(&self.reviews.open[index], self.editor.review(id))
        else {
            return false;
        };
        let place = snapshot.view.place.clone();
        let handed = match self.review_store() {
            Some(store) => store.write(Told::Review(id), name, snapshot),
            None => Err(Box::new(snapshot)),
        };
        let saving = &mut self.reviews.open[index].saving;
        saving.first = None;
        saving.last = None;
        match handed {
            Ok(()) => {
                saving.handed = state;
                saving.left_handed = saving.left;
                if place.is_some() {
                    saving.place_seen.clone_from(&place);
                }
                saving.place_handed = place;
                true
            }
            Err(snapshot) => {
                // The store ended: nothing keeps the review any more, nor is tried again for a
                // stamp alone.
                saving.left_handed = saving.left;
                self.editor.set_review_kept(id, false);
                self.freer.free(snapshot);
                false
            }
        }
    }

    /// What `running`, whose model is `model`, is saved as, if it is saved. Costs references to
    /// the model's readings and comments; what may hold secrets is judged on the store's
    /// thread.
    fn snapshot(&self, running: &Running, model: Option<&Review>) -> Option<Snapshot> {
        let saving = &running.saving;
        if saving.name.is_none() || saving.waiting {
            return None;
        }
        let saved = model?.save();
        // A commit's review a step opened that holds nothing, and was never saved, is not
        // written: walking a long pull request keeps no file a commit (ADR 0167).
        let pull = self.pull_number(running);
        if pull.is_some()
            && saving.handed.is_none()
            && saved.comments.is_empty()
            && saved.files.is_empty()
        {
            return None;
        }
        let unsent = (saved.comments.iter())
            .filter(|comment| comment.author == Author::Person && !comment.sent)
            .count();
        let root = running.root.path().to_owned();
        let header = Header {
            root: root.clone(),
            scope: running.scope.clone(),
            opened: saving.opened,
            comments: saved.comments.len(),
            unsent,
            pull,
            checkpoint: running.checkpoint.clone(),
            left: (saving.left > 0).then_some(saving.left),
            space: saving.space.clone(),
        };
        let choices = self.choices(running.id);
        let view = SavedView {
            choice: choices.choice,
            folded: choices.folded,
            place: self.review_place(running.id),
            filter: choices.filter,
            files: choices.files,
        };
        Some(Snapshot {
            header,
            saved,
            view,
            root,
            held: Arc::clone(&self.reviews.keep.held),
            listing: Arc::clone(&running.decisions.listing),
        })
    }

    /// The review `running` closed: what it held is saved at once, if it changed since it was
    /// last handed over, from the model the editor hands back, which is freed off the UI
    /// thread.
    pub(super) fn save_closed_review(&mut self, running: &Running) {
        let model = self.editor.take_closed_review(running.id);
        let saving = &running.saving;
        // What changed, or where it was left and its cursor (ADR 0184).
        let changed = model.as_ref().map(Review::revision) != saving.handed.as_ref().map(|h| h.0)
            || saving.left != saving.left_handed
            || (saving.place_seen.is_some() && saving.place_seen != saving.place_handed);
        let name = running.saving.name.clone();
        let snapshot = match (changed, name) {
            (true, Some(name)) => self
                .snapshot(running, model.as_ref())
                .map(|snapshot| (name, snapshot)),
            _ => None,
        };
        if let Some((name, mut snapshot)) = snapshot {
            // The view went with the review: its choices and cursor as last handed stand.
            if let Some((_, choices)) = &running.saving.handed {
                snapshot.view.choice = choices.choice;
                snapshot.view.folded = choices.folded;
                snapshot.view.filter.clone_from(&choices.filter);
                snapshot.view.files = choices.files;
            }
            snapshot.view.place =
                (running.saving.place_seen.clone()).or_else(|| running.saving.place_handed.clone());
            let unsent = snapshot.header.unsent;
            let told = Told::Closed { unsent };
            let handed = match self.review_store() {
                Some(store) => store.write(told, name, snapshot),
                None => Err(Box::new(snapshot)),
            };
            if let Err(snapshot) = handed {
                self.freer.free(snapshot);
                self.review_written(told, false);
            }
        }
        self.freer.free(model);
    }

    /// Hands every review that changed to the store, before oso goes or a restart's new
    /// instance reads them; whether any was. [`App::wait_review_saves`] then waits for them.
    pub(in crate::app) fn hand_review_saves(&mut self) -> bool {
        // The review in front is left as oso goes (ADR 0184).
        self.note_front_left();
        let mut handed = false;
        for index in 0..self.reviews.open.len() {
            let saving = &self.reviews.open[index].saving;
            // The cursor too, which alone changes nothing saved as it goes.
            let moved = self.review_place(self.reviews.open[index].id) != saving.place_handed;
            let stamped = saving.left != saving.left_handed;
            if moved || stamped || self.saving_state(index) != saving.handed {
                handed |= self.save_review(index);
            }
        }
        handed
    }

    /// Waits at most `wait` until what was handed to the store is written.
    pub(in crate::app) fn wait_review_saves(&self, wait: Duration) {
        if let Some(store) = &self.reviews.keep.store
            && !store.flush(wait)
        {
            log::warn!("the reviews were not saved within {wait:?}");
        }
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
