//! Reading the EditorConfig files that apply to a file (ADR 0115): from the file's directory
//! up, through the directories project detection walks too ([`upward`]), on the loading
//! thread before the display map is built, so the tab width costs the UI nothing, and on a
//! thread of its own after `C-x C-w` names the file anew.
//!
//! Must not depend on: `app`, `config` or the UI thread; this touches the file system.
//!
//! The walk stops after a file that says `root = true`, after the home directory, and where
//! [`upward`] ends. A `.editorconfig` is opened without following a link, and read only if it
//! is a regular file this user or root owns, that other users cannot write, of at most
//! [`MAX_FILE`] bytes of UTF-8 text, as the configuration file is (ADR 0108). The files of one
//! visit share a [`Budget`] of what they may compile. It reads in every project, trusted or
//! not, as EditorConfig can run nothing (security.md, T1).

use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use oso_core::display_map::TabWidth;
use oso_core::editorconfig::{Applied, Budget, File, MAX_FILE};
use oso_core::indent::Settings;

use oso_platform::file_meta::Directory;
use oso_platform::privacy;

use super::{LoadError, Shared};
use crate::project::{Above, ours, upward};
use crate::walk::real_home;

/// The name of the files read.
const NAME: &str = ".editorconfig";

/// What a visit's load needs to know of the indentation before it builds the display map.
#[derive(Clone, Debug, Default)]
pub struct Visit {
    /// The mode's indentation, as the configuration gives it.
    pub configured: Settings,
    /// Whether to read the EditorConfig files that apply to the file first: always for a
    /// visit; a test of loading may leave them out.
    pub editorconfig: bool,
    /// The home directory, after which no EditorConfig file is read.
    pub home: Home,
}

/// The home directory, as the person's environment names it, and its real place, which the
/// first read that needs it finds, off the UI thread, for all the reads that share it.
#[derive(Clone, Debug, Default)]
pub struct Home {
    spelled: Option<PathBuf>,
    /// Its real place, once found.
    real: Arc<Mutex<Option<Option<PathBuf>>>>,
}

impl Home {
    /// The home directory `spelled` so, if there is one.
    pub fn new(spelled: Option<PathBuf>) -> Self {
        Self {
            spelled,
            real: Arc::default(),
        }
    }

    /// Its real place, or its spelling if that cannot be found, which the walk still stops
    /// after.
    fn real(&self) -> Option<PathBuf> {
        let mut real = self.real.lock().unwrap_or_else(PoisonError::into_inner);
        real.get_or_insert_with(|| real_home(self.spelled.as_deref()))
            .clone()
    }
}

impl Shared {
    /// What a visit's load shares, which builds the display map with the configured tab width
    /// unless EditorConfig gives another.
    pub(super) fn visiting(visit: Visit) -> Self {
        Self {
            tab_width: visit.configured.tab_width(),
            visit: Some(visit),
            ..Self::default()
        }
    }

    /// Locks what EditorConfig gave, which is held only to swap an `Option`.
    pub(super) fn editorconfig(&self) -> std::sync::MutexGuard<'_, Option<EditorConfigFound>> {
        self.editorconfig
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The tab width to build the display map with, given how opening the file went and the
    /// most it may hold: for a visit that goes on to read the file, or makes a new one, after
    /// reading the EditorConfig files that apply to where the file is, which it keeps for the UI.
    pub(super) fn tab_width_for_load(
        &self,
        opened: &Result<(fs::File, Metadata), LoadError>,
        limit: u64,
    ) -> TabWidth {
        let goes_on = match opened {
            Ok((_, metadata)) => metadata.len() <= limit,
            Err(error) => matches!(error, LoadError::NotFound),
        };
        if goes_on {
            self.tab_width_for_visit()
        } else {
            self.tab_width
        }
    }

    /// The tab width to build the display map with: for a visit, after reading the
    /// EditorConfig files that apply to where the file is, which it keeps for the UI.
    fn tab_width_for_visit(&self) -> TabWidth {
        let Some(visit) = &self.visit else {
            return self.tab_width;
        };
        let location = self
            .baseline()
            .as_ref()
            .map(|baseline| baseline.location.clone());
        let found = match location {
            Some(location) if visit.editorconfig => read(&location, &visit.home, &|| {
                self.cancel.load(Ordering::Relaxed)
            }),
            _ => EditorConfigFound::default(),
        };
        let tab_width = found.tab_width(visit.configured);
        *self.editorconfig() = Some(found);
        tab_width
    }
}

/// What the files found for a file give it, and what `*Messages*` should say of them.
#[derive(Debug, Default)]
pub struct EditorConfigFound {
    /// `None` if no file was read.
    pub applied: Option<Applied>,
    /// What was skipped, or not read, and why. They name no buffer text.
    pub notes: Vec<String>,
}

impl EditorConfigFound {
    /// The tab width the file is shown with: EditorConfig's, or the configured one.
    pub fn tab_width(&self, configured: Settings) -> TabWidth {
        self.applied
            .as_ref()
            .and_then(|applied| applied.properties().tab_width())
            .map_or(configured.tab_width(), |(width, _)| {
                TabWidth::new(u32::from(width))
            })
    }
}

/// The most `.editorconfig` files one visit reads, nearest first.
const VISIT_FILES: usize = 32;

/// The most bytes of them one visit reads.
const VISIT_BYTES: u64 = 1 << 20;

/// The most notes one visit gives `*Messages*`; one more says how many were left out.
const VISIT_NOTES: usize = 8;

/// The EditorConfig files that apply to the file at `location`, the real place its path
/// leads, the walk ending after `home`, and early once `cancelled` says so.
pub fn read(location: &Path, home: &Home, cancelled: &dyn Fn() -> bool) -> EditorConfigFound {
    let mut budget = Budget::default();
    let mut notes = Notes::default();
    let mut files = Vec::new();
    let (mut seen, mut bytes) = (0, 0);
    let home = home.real();
    for Above { dir, metadata, .. } in upward(location, home.as_deref()) {
        // Once nothing more can be compiled, no file above adds anything.
        if cancelled() || budget.is_spent() {
            break;
        }
        let path = dir.join(NAME);
        let here = read_here(dir, &metadata, VISIT_BYTES.saturating_sub(bytes));
        if matches!(here, Here::Text(_) | Here::Refused(_)) && seen == VISIT_FILES {
            notes.push(format!(
                "EditorConfig {} is not read: a visit reads at most {VISIT_FILES} of them",
                path.display()
            ));
            break;
        }
        let text = match here {
            Here::Nothing => continue,
            Here::Refused(why) => {
                seen += 1;
                notes.push(format!(
                    "EditorConfig {} is not read: {why}",
                    path.display()
                ));
                continue;
            }
            Here::Text(text) => text,
            Here::PastBudget => {
                notes.push(format!(
                    "EditorConfig {} is not read: a visit reads at most {} KiB of them",
                    path.display(),
                    VISIT_BYTES >> 10
                ));
                break;
            }
        };
        seen += 1;
        bytes += text.len() as u64;
        let file = File::parse(&text, &mut budget);
        for problem in file.problems() {
            notes.push(format!("EditorConfig {}: {problem}", path.display()));
        }
        let root = file.is_root();
        files.push((path, file));
        if root {
            break;
        }
    }
    EditorConfigFound {
        applied: (!files.is_empty()).then(|| Applied::from_walk(location, files)),
        notes: notes.finish(),
    }
}

/// The notes of one visit, at most [`VISIT_NOTES`] of them.
#[derive(Default)]
struct Notes {
    kept: Vec<String>,
    left_out: usize,
}

impl Notes {
    fn push(&mut self, note: String) {
        if self.kept.len() < VISIT_NOTES {
            self.kept.push(note);
        } else {
            self.left_out += 1;
        }
    }

    fn finish(mut self) -> Vec<String> {
        if self.left_out > 0 {
            let more = self.left_out;
            self.kept.push(format!(
                "EditorConfig: {more} more notes of this visit left out"
            ));
        }
        self.kept
    }
}

/// What a directory holds of EditorConfig.
enum Here {
    Nothing,
    Text(String),
    Refused(String),
    /// It is larger than what the visit may still read.
    PastBudget,
}

/// The `.editorconfig` in `dir`, which the walk saw as `walked`, read from the directory
/// itself once it is the one the walk saw, if it is at most `left` bytes.
fn read_here(dir: &Path, walked: &Metadata, left: u64) -> Here {
    let name = OsStr::new(NAME);
    // A directory that cannot be listed holds nothing to read.
    let Ok(directory) = Directory::open(dir) else {
        return Here::Nothing;
    };
    // Its owner and mode as opened, once it is the directory the walk saw.
    let Some(opened) = opened_as_walked(&directory, walked) else {
        return Here::Refused("its directory changed as it was read".into());
    };
    if privacy::others_can_write(&opened) {
        return match directory.kind_of(name) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Here::Nothing,
            _ => Here::Refused("every user may write its directory".into()),
        };
    }
    let (file, metadata) = match directory.open_file(name) {
        Ok(Some(opened)) => opened,
        Ok(None) => return Here::Refused("it is a link, or not a regular file here".into()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Here::Nothing,
        Err(_) => return Here::Refused("it cannot be opened".into()),
    };
    if !ours(&metadata) || privacy::others_can_write(&metadata) {
        return Here::Refused("another user owns it or can change it".into());
    }
    if metadata.len() > MAX_FILE as u64 {
        return Here::Refused(format!("it is larger than {} KiB", MAX_FILE >> 10));
    }
    if metadata.len() > left {
        return Here::PastBudget;
    }
    // No more than it held when opened, so what a visit reads stays within its bound.
    let mut bytes = Vec::new();
    if file
        .take(metadata.len() + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Here::Refused("it cannot be read".into());
    }
    if bytes.len() as u64 > metadata.len() {
        return Here::Refused("it changed as it was read".into());
    }
    match String::from_utf8(bytes) {
        Ok(text) => Here::Text(text),
        Err(_) => Here::Refused("it is not UTF-8 text".into()),
    }
}

/// The metadata of `directory`, if it is the one the walk saw as `walked`, by device and inode.
/// Windows keeps no stable identity to compare, and its directories are opened by path.
fn opened_as_walked(directory: &Directory, walked: &Metadata) -> Option<Metadata> {
    let opened = directory.metadata().ok()?;
    #[cfg(unix)]
    if !super::same_file(&opened, walked) {
        return None;
    }
    #[cfg(not(unix))]
    let _ = walked;
    Some(opened)
}

mod reader;

pub use reader::EditorConfigReader;

#[cfg(test)]
mod tests;
