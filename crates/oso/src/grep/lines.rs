//! Searching a buffer's lines for `consult-line` (ADR 0154): a thread of its own, `oso-lines`,
//! runs each request, a snapshot of the buffer's text the editor made, and sends the rows it
//! finds.
//!
//! Must not depend on: `app`, winit or the UI thread; the caller supplies the wake-up. This reads
//! nothing from disk: the text is the editor's, shared.
//!
//! - A newer request moves the generation, which the search reads at each block of the text; a
//!   superseded one stops there and sends nothing more.
//! - The first row goes as soon as it is found, so the selected row shows at once; the rest go
//!   at most every [`BATCH_EVERY`], looked at between blocks too, and with the end. A search
//!   that ends at once sends two batches.
//! - Nothing here logs what is searched for or found; `Debug` gives a generation and a count.

use std::fmt;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Instant;

use oso_core::editor::{LineRow, LineSearchRequest, LinesEnd, WalkEnd};

use super::BATCH_EVERY;

/// A batch of rows a search of a buffer's lines found, and whether it ended. `Debug` gives its
/// generation and size only.
pub struct FoundLines {
    pub generation: u64,
    pub rows: Vec<LineRow>,
    pub end: Option<WalkEnd>,
}

impl fmt::Debug for FoundLines {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FoundLines")
            .field("generation", &self.generation)
            .field("rows", &self.rows.len())
            .field("end", &self.end)
            .finish()
    }
}

/// Hands a batch to the event loop; `false` once it is gone.
type SendLines = Arc<dyn Fn(FoundLines) -> bool + Send + Sync>;

/// The thread that searches a buffer's lines, started with the first request.
pub struct LineSearcher {
    requests: Option<Sender<LineSearchRequest>>,
    send: SendLines,
}

impl LineSearcher {
    /// A searcher that hands each batch to `send`.
    pub fn new(send: impl Fn(FoundLines) -> bool + Send + Sync + 'static) -> Self {
        Self {
            requests: None,
            send: Arc::new(send),
        }
    }

    /// Searches as `request` asks, on `oso-lines`, after any search before it, which a newer
    /// one has stopped. If the thread cannot start, the search ends at once with nothing found,
    /// so the prompt stops waiting.
    pub fn search(&mut self, request: LineSearchRequest) {
        if self.requests.is_none() {
            let (requests, queue) = mpsc::channel::<LineSearchRequest>();
            let send = Arc::clone(&self.send);
            let spawned = thread::Builder::new()
                .name("oso-lines".into())
                .spawn(move || serve(&queue, &*send));
            if spawned.is_ok() {
                self.requests = Some(requests);
            } else {
                log::warn!("the line searching thread could not start");
            }
        }
        let request = match &self.requests {
            Some(requests) => match requests.send(request) {
                Ok(()) => return,
                Err(failed) => failed.0,
            },
            None => request,
        };
        self.requests = None;
        (self.send)(ended(request.generation, false));
    }
}

/// The end of the search `generation`, with nothing more found.
fn ended(generation: u64, cut: bool) -> FoundLines {
    FoundLines {
        generation,
        rows: Vec::new(),
        end: Some(WalkEnd { cut }),
    }
}

/// Searches as each request that comes asks, in turn, until the searcher is dropped or the
/// event loop has gone.
fn serve(queue: &Receiver<LineSearchRequest>, send: &(dyn Fn(FoundLines) -> bool + Send + Sync)) {
    // A superseded request stops at its first block, sending nothing.
    while let Ok(request) = queue.recv() {
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| search(&request, send)));
        let gone = match outcome {
            Ok(gone) => gone,
            Err(_) => {
                log::warn!("a line search panicked");
                !send(ended(request.generation, false))
            }
        };
        if gone {
            return;
        }
    }
}

/// Runs `request`, sending its rows in batches: whether the event loop has gone. The first row
/// goes at once, the rest at most every [`BATCH_EVERY`] and with the end, a search keeping at
/// most [`MAX_ROWS`](oso_core::project::MAX_ROWS): the editor takes each batch in one append,
/// whose cost grows with the list, so fewer and larger batches cost it less.
fn search(request: &LineSearchRequest, send: &dyn Fn(FoundLines) -> bool) -> bool {
    let generation = request.generation;
    let mut rows = Vec::new();
    let mut sent: Option<Instant> = None;
    let mut gone = false;
    let ended_as = request.run(&mut |row| {
        let first = sent.is_none() && row.is_some();
        rows.extend(row);
        let due = sent.is_some_and(|sent| sent.elapsed() >= BATCH_EVERY);
        if (first || due) && !rows.is_empty() {
            sent = Some(Instant::now());
            let batch = std::mem::take(&mut rows);
            if !send(FoundLines {
                generation,
                rows: batch,
                end: None,
            }) {
                gone = true;
                return false;
            }
        }
        true
    });
    match ended_as {
        LinesEnd::Stopped => gone,
        LinesEnd::Ended(end) => !send(FoundLines {
            generation,
            rows,
            end: Some(end),
        }),
    }
}

#[cfg(test)]
#[path = "lines_tests.rs"]
mod tests;
