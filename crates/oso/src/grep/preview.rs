//! Reading a search's row's file for its preview (ADR 0114): a thread of its own, `oso-preview`,
//! reads the lines around the row's line and hands them to the editor as an excerpt.
//!
//! Must not depend on: `app`, winit or the UI thread; this reads the file system, and the caller
//! supplies the wake-up.
//!
//! - The file is reached as the search reached it: the root is opened by its path, never one
//!   that is or holds the home directory, by the real place of either and by device and inode,
//!   then each directory on the way and the file are opened from the one before by name,
//!   following no link and waiting for nothing. Only a regular file is read.
//! - A file that may hold secrets, by the name of its path from the root as given or as it
//!   leads, is never read; the editor never asks for one whose row says so either.
//! - The read starts at the row's line, as far as the search found it, and goes back for about
//!   half the preview's rows: at most [`BACK`] before the line and [`MAX_LINE`] in all.
//! - Each request has a generation, read before the file is opened, at each step on the way
//!   and before each part read; a newer selection stops it, and only the newest of those
//!   waiting is read.
//! - Nothing here logs what is read; `Debug` gives a generation.

use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use oso_core::buffer::Buffer;
use oso_core::editor::{Excerpt, ProjectPreviewRead, ProjectPreviewRequest};
use oso_core::project::MAX_LINE;
use oso_platform::file_meta::Directory;

use crate::walk::{self, MAX_DEPTH, MAX_PATH};

/// The most bytes read before the row's line, for the lines above it.
pub const BACK: usize = 64 * 1024;

/// How much is read at a time, the generation asked before each.
const PART: usize = 64 * 1024;

/// The most bytes past the end of the row's line read for each row after it, as an excerpt keeps
/// no more of a line.
const PER_ROW: usize = 4096 + 1;

/// What a preview's read found, for the editor. `Debug` gives its generation only.
pub struct Previewed {
    pub generation: u64,
    pub read: ProjectPreviewRead,
}

impl fmt::Debug for Previewed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Previewed")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// Hands a read to the event loop; `false` once it is gone.
type SendRead = Arc<dyn Fn(Previewed) -> bool + Send + Sync>;

/// The reading thread, started with the first request.
pub struct Previewer {
    requests: Option<Sender<ProjectPreviewRequest>>,
    send: SendRead,
    /// The home directory as the person's environment spells it, which the thread finds the
    /// real place of once.
    home: Option<PathBuf>,
}

impl Previewer {
    /// A reader that hands each read to `send`, and never reads under a root that holds `home`,
    /// as the search never searches one.
    pub fn new(
        home: Option<PathBuf>,
        send: impl Fn(Previewed) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            requests: None,
            send: Arc::new(send),
            home,
        }
    }

    /// Reads as `request` asks, on `oso-preview`, after any read before it, which a newer one
    /// has stopped. If the thread cannot start, the preview shows nothing.
    pub fn read(&mut self, request: ProjectPreviewRequest) {
        if self.requests.is_none() {
            let (requests, queue) = mpsc::channel::<ProjectPreviewRequest>();
            let send = Arc::clone(&self.send);
            let home = self.home.clone();
            let spawned = thread::Builder::new()
                .name("oso-preview".into())
                .spawn(move || serve(&queue, &*send, home.as_deref()));
            if spawned.is_ok() {
                self.requests = Some(requests);
            } else {
                log::warn!("the preview's thread could not start");
            }
        }
        if let Some(requests) = &self.requests
            && requests.send(request).is_err()
        {
            self.requests = None;
        }
    }
}

/// Reads as the newest request waiting asks, in turn, until the reader is dropped or the event
/// loop has gone.
fn serve(
    queue: &Receiver<ProjectPreviewRequest>,
    send: &(dyn Fn(Previewed) -> bool + Send + Sync),
    home: Option<&Path>,
) {
    let home = walk::real_home(home);
    while let Ok(mut request) = queue.recv() {
        // Only the newest waiting is read; the rest are stale.
        while let Ok(newer) = queue.try_recv() {
            request = newer;
        }
        let generation = request.generation;
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            read_preview(&request, home.as_deref())
        }));
        let read = match read {
            Ok(Some(read)) => read,
            Ok(None) => continue,
            Err(_) => {
                log::warn!("a preview's read panicked");
                ProjectPreviewRead::Unreadable
            }
        };
        if !send(Previewed { generation, read }) {
            return;
        }
    }
}

/// What `request` finds, or `None` once a newer request has stopped it.
pub(crate) fn read_preview(
    request: &ProjectPreviewRequest,
    home: Option<&Path>,
) -> Option<ProjectPreviewRead> {
    let live = || request.live.load(Ordering::Relaxed) == request.generation;
    if !live() {
        return None;
    }
    let root = &request.root;
    let path = Path::new(&*request.path);
    let secret = |root: &Path| Buffer::name_may_hold_secrets(&root.join(path));
    let real_root = std::fs::canonicalize(root).ok();
    if secret(root) || real_root.as_deref().is_some_and(secret) {
        return Some(ProjectPreviewRead::Withheld);
    }
    if walk::holds_home(root, home) {
        return Some(ProjectPreviewRead::Unreadable);
    }
    let Some((file, identity)) = open(root, path, home, &live) else {
        return live().then_some(ProjectPreviewRead::Unreadable);
    };
    let Some((bytes, first_line)) = read_around(file, request, &live) else {
        return live().then_some(ProjectPreviewRead::Unreadable);
    };
    let hit = (request.line, request.column);
    Some(ProjectPreviewRead::Text(Excerpt::new(
        &bytes, first_line, hit, identity,
    )))
}

/// The file at `path` under `root`, opened from the root a directory at a time, following no
/// link, with its device and inode where the system has them: none if any step is no directory,
/// a link or gone, if the path leaves the root or goes deeper than a walk, if the root is or
/// holds `home`, or once `live` says the read is stale.
fn open(
    root: &Path,
    path: &Path,
    home: Option<&Path>,
    live: &dyn Fn() -> bool,
) -> Option<(File, Option<(u64, u64)>)> {
    if path.as_os_str().len() > MAX_PATH {
        return None;
    }
    let mut names = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => names.push(name),
            // A row's path is the walk's, from the root and down only.
            _ => return None,
        }
    }
    let (file, directories) = names.split_last()?;
    if directories.len() > MAX_DEPTH {
        return None;
    }
    let mut directory = Directory::open(root).ok()?;
    if walk::opened_holds_home(&directory, home) {
        return None;
    }
    for name in directories {
        if !live() {
            return None;
        }
        directory = directory.open_directory(name).ok()??;
    }
    if !live() {
        return None;
    }
    let (file, metadata) = directory.open_file(file).ok()??;
    Some((file, identity(&metadata)))
}

#[cfg(unix)]
fn identity(metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn identity(_: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// The lines of `file` around the request's line, from its offset, and the number of the first:
/// at most [`BACK`] bytes of whole lines before it, for about half the rows, and from it as many
/// lines as fill the rest, within [`MAX_LINE`] in all. `None` if it cannot be read, the offset is
/// past its end, or once `live` says the read is stale.
fn read_around(
    mut file: File,
    request: &ProjectPreviewRequest,
    live: &dyn Fn() -> bool,
) -> Option<(Vec<u8>, usize)> {
    let before = request.rows / 2;
    let after = request.rows.saturating_sub(before).max(1);
    let back = usize::try_from(request.offset).map_or(BACK, |offset| offset.min(BACK));
    let start = request.offset - back as u64;
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::with_capacity(back + PART);
    (&mut file).take(back as u64).read_to_end(&mut bytes).ok()?;
    if bytes.len() < back {
        return None;
    }
    // Whole lines only: a read that starts within one leaves it out.
    let mut starts: Vec<usize> = bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .map(|(at, _)| at + 1)
        .filter(|&at| at < bytes.len())
        .collect();
    if start == 0 && !bytes.is_empty() {
        starts.insert(0, 0);
    }
    let kept = starts.len().min(before);
    let from = starts.len().checked_sub(kept).and_then(|at| starts.get(at));
    let from = from.copied().unwrap_or(bytes.len());
    bytes.drain(..from);
    let first_line = request.line.saturating_sub(kept);
    // Then the line and those after it, a part at a time.
    let mut breaks = 0;
    let mut line_end = None;
    let limit = MAX_LINE.saturating_sub(bytes.len());
    let mut read = 0;
    let mut part = vec![0; PART];
    loop {
        if !live() {
            return None;
        }
        // Past the row's line, an excerpt keeps no more of each line than [`PER_ROW`].
        let room = line_end.map_or(usize::MAX, |end| {
            ((after - breaks + 1) * PER_ROW).saturating_sub(bytes.len() - end)
        });
        let wanted = PART.min(limit - read).min(room);
        if wanted == 0 {
            break;
        }
        let count = file.read(&mut part[..wanted]).ok()?;
        if count == 0 {
            break;
        }
        read += count;
        for (at, &byte) in part[..count].iter().enumerate() {
            if byte == b'\n' {
                breaks += 1;
                if line_end.is_none() {
                    line_end = Some(bytes.len() + at);
                }
                if breaks == after {
                    bytes.extend_from_slice(&part[..=at]);
                    return Some((bytes, first_line));
                }
            }
        }
        bytes.extend_from_slice(&part[..count]);
    }
    if read == 0 && request.offset > 0 && bytes.is_empty() {
        return None;
    }
    Some((bytes, first_line))
}

#[cfg(test)]
mod tests;
