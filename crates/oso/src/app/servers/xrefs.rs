//! The list of a server's places (ADR 0138, step 8b), on the UI thread: several definitions
//! `M-.` found, or the references `M-?` found, listed as `consult-xref` lists them. Its rows are
//! built on `oso-xrefs` (`rows`); the list opens once they come, or after [`OPEN_AFTER`] waiting
//! for them, and `RET` goes to the row's place as an answer's jump goes.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server or a
//! file, and nothing here reads the file system.
//!
//! - Until it opens, the list waits only while nothing moved since the ask, as an answer does:
//!   a person who moved on is not shown it. A newer ask, or the list's end, stops the build.
//! - The agent never gets a list (step 8's Q5): its answers go to the first place.
//! - The last list's server is kept until the next list, so that `RET`'s row goes to it however
//!   late the host takes the row: a command a macro replays runs before the host looks.
//! - At most [`READING_MAX`] builds read files at once; a build past them gives rows with no
//!   text of a file no buffer holds, as one stuck on a file system that hangs never ends.

mod rows;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;
use oso_core::editor::{ListedHit, XrefKind, XrefRows};
use oso_core::minibuffer::PromptId;
use oso_lsp::server::{AskKind, Location};

use super::super::{App, UserEvent};
use super::asks::Asker;
use super::places::{column, line_and_column};

/// How long a list waits for its rows before it opens without them (step 8's Q4).
pub(super) const OPEN_AFTER: Duration = Duration::from_secs(1);

/// The most builds that read files at once.
const READING_MAX: usize = 2;

/// The list being built or shown, the newest list's number, which stops an older build, the
/// last list's server, and how many builds read files now.
#[derive(Debug, Default)]
pub(super) struct Lists {
    list: Option<List>,
    live: Arc<AtomicU64>,
    server: Option<(PathBuf, String)>,
    reading: Arc<AtomicUsize>,
}

/// One list: its number (the ask's serial), what it lists, who asked and where, the server's
/// root as a URI reads back, and its rows until it opens.
#[derive(Debug)]
struct List {
    generation: u64,
    kind: XrefKind,
    asked: BufferId,
    asker: Asker,
    root: PathBuf,
    due: Instant,
    rows: Option<XrefRows>,
    prompt: Option<PromptId>,
}

impl Lists {
    /// Stops the build of the list there is, and gives it back, to be freed off this thread.
    fn stop(&mut self) -> Option<List> {
        self.live.store(0, Ordering::Relaxed);
        self.list.take()
    }

    /// Stops the list that waits for its rows, if one does, and gives it back: one that opened
    /// stays.
    pub(super) fn stop_waiting(&mut self) -> Option<impl Send + 'static> {
        if self.list.as_ref().is_some_and(|list| list.prompt.is_none()) {
            return self.stop();
        }
        None
    }
}

/// Counts a build that reads files, until it ends.
struct Reading(Arc<AtomicUsize>);

impl Drop for Reading {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl App {
    /// Lists the `places` server `index` answered to `asker`'s ask of `kind` about `asked`, on
    /// a thread of its own; the list opens once the rows come or [`OPEN_AFTER`] has passed.
    pub(super) fn list_places(
        &mut self,
        index: usize,
        (kind, asked, asker): (AskKind, BufferId, Asker),
        places: &[Location],
    ) {
        let kind = match kind {
            AskKind::Definition => XrefKind::Definitions,
            AskKind::References => XrefKind::References,
            // Completions and code actions are no places, and a description goes to its float:
            // none comes here.
            AskKind::Completion
            | AskKind::Resolve
            | AskKind::Hover
            | AskKind::CodeAction
            | AskKind::ResolveAction
            | AskKind::ExecuteCommand => return,
        };
        let Some(running) = self.servers.running.get(index) else {
            return;
        };
        let root = super::read_back(&running.root).unwrap_or_else(|| running.root.clone());
        let server = (running.root.clone(), running.entry.name.clone());
        let generation = asker.serial();
        let reading = Arc::clone(&self.servers.xrefs.reading);
        let reads = reading.fetch_add(1, Ordering::Relaxed) < READING_MAX;
        // Counted from here, so that a thread that cannot start lets go too.
        let counted = Reading(reading);
        let mut snapshot = self.xref_snapshot(index, places, generation, root.clone());
        if !reads {
            snapshot.read_max = 0;
        }
        if let Some(old) = self.servers.xrefs.stop() {
            self.freer.free(old);
        }
        self.servers.xrefs.server = Some(server);
        let live = Arc::clone(&self.servers.xrefs.live);
        live.store(generation, Ordering::Relaxed);
        self.servers.xrefs.list = Some(List {
            generation,
            kind,
            asked,
            asker,
            root,
            due: Instant::now() + OPEN_AFTER,
            rows: None,
            prompt: None,
        });
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name("oso-xrefs".into())
            .spawn(move || {
                let _counted = counted;
                let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // Found here, as it reads the file system.
                    snapshot.home.real = crate::walk::real_home(snapshot.home.spelled.as_deref());
                    rows::build(&snapshot, super::super::greps::leads, &|| {
                        live.load(Ordering::Relaxed) == generation
                    })
                }));
                let built = built.unwrap_or_else(|_| {
                    log::warn!("the build of a list of a server's places panicked");
                    Some(XrefRows::new(generation, Vec::new(), false))
                });
                // The snapshot's texts and paths are freed here too.
                drop(snapshot);
                if let Some(built) = built {
                    waker.send(UserEvent::XrefsListed(generation, built));
                }
            });
        if spawned.is_err() {
            log::warn!("the thread that lists a server's places could not start");
            self.xrefs_listed(generation, XrefRows::new(generation, Vec::new(), false));
        }
    }

    /// What the rows of the list `generation` of server `index`'s `places` are built from: each
    /// place in an open document by its buffer, with the buffer's text, any other by its line,
    /// and the open buffers that withhold their text, whose files a row never reads.
    fn xref_snapshot(
        &self,
        index: usize,
        places: &[Location],
        generation: u64,
        root: PathBuf,
    ) -> rows::Snapshot {
        let opened = self.open_places(index, places);
        let mut texts = HashMap::new();
        let spots = (places.iter().zip(opened))
            .map(|(place, opened)| {
                let held =
                    opened.and_then(|(buffer, _)| Some((buffer, self.editor.buffer(buffer)?)));
                match (opened, held) {
                    (Some((buffer, offset)), Some((_, held))) => {
                        texts.entry(buffer).or_insert_with(|| held.text().clone());
                        rows::Spot::Open {
                            path: place.path.clone(),
                            buffer,
                            offset,
                            revision: held.revision(),
                            secret: held.is_secret() || held.verdict().is_withheld(),
                        }
                    }
                    _ => {
                        let (start, encoding) = line_and_column(&place.target);
                        rows::Spot::File {
                            path: place.path.clone(),
                            line: start.line as usize + 1,
                            column: column(start.character as usize, encoding),
                        }
                    }
                }
            })
            .collect();
        let withheld = (self.editor.buffers())
            .filter(|(_, held)| held.is_secret() || held.verdict().is_withheld())
            .filter_map(|(_, held)| {
                let path = held.path().and_then(super::read_back);
                let identity = held.baseline().and_then(|baseline| baseline.file?.identity);
                (path.is_some() || identity.is_some()).then_some(rows::Withheld { path, identity })
            })
            .collect();
        rows::Snapshot {
            generation,
            root,
            home: rows::Home {
                spelled: super::super::home(),
                real: None,
            },
            spots,
            texts,
            withheld,
            read_max: rows::READ_MAX,
            cut: places.len() >= oso_lsp::limits::LOCATIONS_MAX,
        }
    }

    /// The rows of a list of places came: the list opens with them, or takes them if it opened
    /// waiting, unless they are another list's, it went, or the person moved on.
    pub(in crate::app) fn xrefs_listed(&mut self, generation: u64, rows: XrefRows) {
        let Some(list) = &mut self.servers.xrefs.list else {
            self.freer.free(rows);
            return;
        };
        if list.prompt.is_some() {
            // The editor drops another list's.
            self.editor.xref_listed(rows);
            self.request_redraw();
            return;
        }
        if generation != list.generation {
            self.freer.free(rows);
            return;
        }
        list.rows = Some(rows);
        self.open_xref_list();
    }

    /// Before the loop waits: a list that waits opens once its time has come, and one whose
    /// person moved on, or whose prompt closed, goes.
    pub(in crate::app) fn xrefs_if_due(&mut self) {
        let Some(list) = &self.servers.xrefs.list else {
            return;
        };
        let gone = match list.prompt {
            // Its end stops the build; the server stays for `RET`'s row.
            Some(id) => !self.editor.prompts().iter().any(|prompt| prompt.id() == id),
            None => !self.still_there(list.asked, &list.asker),
        };
        if gone {
            if let Some(old) = self.servers.xrefs.stop() {
                self.freer.free(old);
            }
        } else if list.prompt.is_none() && Instant::now() >= list.due {
            self.open_xref_list();
        }
    }

    /// When the list that waits for its rows opens without them, for the loop's wake.
    pub(in crate::app) fn xrefs_deadline(&self) -> Option<Instant> {
        let list = self.servers.xrefs.list.as_ref()?;
        list.prompt.is_none().then_some(list.due)
    }

    /// Opens the list that waits, with its rows if they came, while nothing moved since the
    /// ask; otherwise it goes.
    fn open_xref_list(&mut self) {
        let Some(list) = &self.servers.xrefs.list else {
            return;
        };
        let (kind, generation, root) = (list.kind, list.generation, list.root.clone());
        let prompt = if self.still_there(list.asked, &list.asker) {
            self.dispatcher
                .list_xrefs(&mut self.editor, kind, generation, root)
        } else {
            None
        };
        let Some(prompt) = prompt else {
            if let Some(old) = self.servers.xrefs.stop() {
                self.freer.free(old);
            }
            self.request_redraw();
            return;
        };
        if let Some(list) = &mut self.servers.xrefs.list {
            list.prompt = Some(prompt);
            if let Some(rows) = list.rows.take() {
                self.editor.xref_listed(rows);
            }
        }
        self.request_redraw();
    }

    /// The name of the server of the list of places open, or of the last one, for the agent's
    /// `state` (step 15).
    pub(in crate::app) fn xrefs_server(&self) -> Option<&str> {
        let (_, name) = self.servers.xrefs.server.as_ref()?;
        Some(name)
    }

    /// `lsp-go-to-xref`: goes to the row the list took, as an answer's jump goes, the file of a
    /// place outside any project joining the server that gave it.
    pub(super) fn go_to_xref(&mut self) {
        let Some(hit) = self.editor.take_xref_hit() else {
            self.editor.message("No place was picked");
            self.request_redraw();
            return;
        };
        if let Some(old) = self.servers.xrefs.stop() {
            self.freer.free(old);
        }
        let index = (self.servers.xrefs.server.as_ref())
            .and_then(|(root, name)| self.servers.find(root, name));
        match hit {
            ListedHit::Buffer { id, offset } => self.editor.xref_to_place(id, offset),
            ListedHit::File { path, line, column } => {
                self.visit_place(index, path, line, column);
            }
        }
        self.request_redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_past_the_most_that_read_is_counted_and_lets_go_when_it_ends() {
        let reading = Arc::new(AtomicUsize::new(READING_MAX));
        let counted = Reading(Arc::clone(&reading));
        drop(counted);
        assert_eq!(reading.load(Ordering::Relaxed), READING_MAX - 1);
    }
}
