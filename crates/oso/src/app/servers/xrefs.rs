//! The list of a server's places (ADR 0138, step 8b), on the UI thread: several definitions
//! `M-.` found, or the references `M-?` found, listed as `consult-xref` lists them. Its rows are
//! built on `oso-xrefs` (`rows`); the list opens once they come, or after [`OPEN_AFTER`] waiting
//! for them, and `RET` goes to the row's place as an answer's jump goes.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server or a
//! file.
//!
//! - Until it opens, the list waits only while nothing moved since the ask, as an answer does:
//!   a person who moved on is not shown it. A newer ask, or the list's end, stops the build.
//! - The agent never gets a list (step 8's Q5): its answers go to the first place.

mod rows;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::{DiagnosticHit, XrefKind, XrefRows};
use oso_core::minibuffer::PromptId;
use oso_lsp::position::Encoding;
use oso_lsp::server::{AskKind, Location};

use super::super::{App, UserEvent};
use super::places::{Asker, line_and_column};

/// How long a list waits for its rows before it opens without them (step 8's Q4).
pub(super) const OPEN_AFTER: Duration = Duration::from_secs(1);

/// The list being built or shown, and the newest list's number, which stops an older build.
#[derive(Debug, Default)]
pub(super) struct Listing {
    list: Option<List>,
    live: Arc<AtomicU64>,
}

/// One list: its number (the ask's serial), what it lists, who asked and where, the server's
/// root and entry, and its rows until it opens.
#[derive(Debug)]
struct List {
    generation: u64,
    kind: XrefKind,
    asked: BufferId,
    asker: Asker,
    server: (PathBuf, String),
    root: PathBuf,
    due: Instant,
    rows: Option<XrefRows>,
    prompt: Option<PromptId>,
}

impl Listing {
    /// Stops the build of the list there is, and forgets it.
    fn stop(&mut self) {
        self.live.store(0, Ordering::Relaxed);
        self.list = None;
    }

    /// Stops the list that waits for its rows, if one does: one that opened stays.
    pub(super) fn stop_waiting(&mut self) {
        if self.list.as_ref().is_some_and(|list| list.prompt.is_none()) {
            self.stop();
        }
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
        let Some(running) = self.servers.running.get(index) else {
            return;
        };
        // In the form a URI reads back as, as the places' paths are.
        let root = oso_lsp::uri::from_path(&running.root)
            .and_then(|uri| oso_lsp::uri::to_path(&uri))
            .unwrap_or_else(|| running.root.clone());
        let server = (running.root.clone(), running.entry.name.clone());
        let mut texts = std::collections::HashMap::new();
        let spots = (places.iter())
            .map(|place| match self.open_place(index, place) {
                Some((buffer, offset)) => {
                    let held = self.editor.buffer(buffer);
                    if let Some(held) = held {
                        texts.entry(buffer).or_insert_with(|| held.text().clone());
                    }
                    rows::Spot::Open {
                        path: place.path.clone(),
                        buffer,
                        offset,
                        revision: held.map_or(0, Buffer::revision),
                        secret: held.is_none_or(Buffer::is_secret),
                    }
                }
                None => {
                    let (start, encoding) = line_and_column(&place.target);
                    rows::Spot::File {
                        path: place.path.clone(),
                        line: start.line as usize + 1,
                        column: column(start.character as usize, encoding),
                    }
                }
            })
            .collect();
        let generation = asker.serial();
        let snapshot = rows::Snapshot {
            generation,
            root: root.clone(),
            home: (
                super::super::home(),
                crate::walk::real_home(super::super::home().as_deref()),
            ),
            spots,
            texts,
            cut: places.len() >= oso_lsp::limits::LOCATIONS_MAX,
        };
        let kind = match kind {
            AskKind::Definition => XrefKind::Definitions,
            AskKind::References => XrefKind::References,
        };
        self.servers.xrefs.stop();
        let live = Arc::clone(&self.servers.xrefs.live);
        live.store(generation, Ordering::Relaxed);
        self.servers.xrefs.list = Some(List {
            generation,
            kind,
            asked,
            asker,
            server,
            root,
            due: Instant::now() + OPEN_AFTER,
            rows: None,
            prompt: None,
        });
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name("oso-xrefs".into())
            .spawn(move || {
                let built = rows::build(&snapshot, super::super::greps::leads, &|| {
                    live.load(Ordering::Relaxed) == generation
                });
                // The snapshot's texts and paths are freed here too.
                drop(snapshot);
                if let Some(built) = built {
                    waker.send(UserEvent::XrefsListed(built));
                }
            });
        if spawned.is_err() {
            log::warn!("the thread that lists a server's places could not start");
            self.xrefs_listed(XrefRows::new(generation, Vec::new(), false));
        }
    }

    /// The rows of a list of places came: the list opens with them, or takes them if it opened
    /// waiting, unless it went or the person moved on.
    pub(in crate::app) fn xrefs_listed(&mut self, rows: XrefRows) {
        let Some(list) = &mut self.servers.xrefs.list else {
            self.freer.free(rows);
            return;
        };
        if list.prompt.is_some() {
            self.editor.xref_listed(rows);
            self.request_redraw();
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
        match list.prompt {
            Some(id) => {
                // Its end stops the build. `RET`'s row was gone to before: the host takes it
                // right after the command, before this.
                if !self.editor.prompts().iter().any(|prompt| prompt.id() == id) {
                    self.servers.xrefs.stop();
                }
            }
            None if !self.still_there(list.asked, &list.asker) => self.servers.xrefs.stop(),
            None if Instant::now() >= list.due => self.open_xref_list(),
            None => {}
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
        if !self.still_there(list.asked, &list.asker) {
            self.servers.xrefs.stop();
            return;
        }
        let (kind, generation, root) = (list.kind, list.generation, list.root.clone());
        if !self
            .dispatcher
            .list_xrefs(&mut self.editor, kind, generation, root)
        {
            self.servers.xrefs.stop();
            self.request_redraw();
            return;
        }
        let prompt = self.editor.prompt().map(|prompt| prompt.id());
        if let Some(list) = &mut self.servers.xrefs.list {
            list.prompt = prompt;
            if let Some(rows) = list.rows.take() {
                self.editor.xref_listed(rows);
            }
        }
        self.request_redraw();
    }

    /// `lsp-go-to-xref`: goes to the row the list took, as an answer's jump goes, the file of a
    /// place outside any project joining the server that gave it.
    pub(super) fn go_to_xref(&mut self) {
        let Some(hit) = self.editor.take_xref_hit() else {
            return;
        };
        let server = (self.servers.xrefs.list.as_ref()).map(|list| list.server.clone());
        self.servers.xrefs.stop();
        let index = server.and_then(|(root, name)| {
            (self.servers.running.iter())
                .position(|running| running.root == root && running.entry.name == name)
        });
        match hit {
            DiagnosticHit::Buffer { id, offset } => self.editor.xref_to_place(id, offset),
            DiagnosticHit::File { path, line, column } => {
                if let Some(index) = index
                    && !Buffer::name_may_hold_secrets(&path)
                {
                    self.join(index, path.clone());
                }
                self.editor
                    .xref_visit(path, oso_core::load::UNASKED, line, column);
                // The command ran before the host took the hit: the visit starts here.
                self.serve_files();
            }
        }
        self.request_redraw();
    }
}

/// `column` as the visits count it, in the server's `encoding`.
pub(super) fn column(column: usize, encoding: Encoding) -> oso_core::editor::VisitColumn {
    match encoding {
        Encoding::Utf8 => oso_core::editor::VisitColumn::Utf8(column),
        Encoding::Utf16 => oso_core::editor::VisitColumn::Utf16(column),
    }
}
