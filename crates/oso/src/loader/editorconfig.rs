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

use std::fs::Metadata;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use oso_core::display_map::TabWidth;
use oso_core::editor::ProjectRequest;
use oso_core::editorconfig::{Applied, Budget, File, MAX_FILE, Properties};
use oso_core::indent::{self, Settings};

use super::Shared;
use crate::project::{Above, ours, upward};

/// The name of the files read.
const NAME: &str = ".editorconfig";

/// What a visit's load needs to know of the indentation before it builds the display map.
#[derive(Clone, Debug, Default)]
pub struct Visit {
    /// The mode's indentation, as the configuration gives it.
    pub configured: Settings,
    /// Whether to read the EditorConfig files that apply to the file first.
    pub editorconfig: bool,
    /// The home directory, after which no EditorConfig file is read.
    pub home: Option<PathBuf>,
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
    pub(super) fn editorconfig(&self) -> std::sync::MutexGuard<'_, Option<Found>> {
        self.editorconfig
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The tab width to build the display map with: for a visit, after reading the
    /// EditorConfig files that apply to where the file is, which it keeps for the UI.
    pub(super) fn tab_width_for_visit(&self) -> TabWidth {
        let Some(visit) = &self.visit else {
            return self.tab_width;
        };
        let location = self
            .baseline()
            .as_ref()
            .map(|baseline| baseline.location.clone());
        let found = match location {
            Some(location) if visit.editorconfig => read(&location, visit.home.as_deref()),
            _ => Found::default(),
        };
        let tab_width = found.tab_width(visit.configured);
        *self.editorconfig() = Some(found);
        tab_width
    }
}

/// What the files found for a file give it, and what `*Messages*` should say of them.
#[derive(Debug, Default)]
pub struct Found {
    /// `None` if no file was read.
    pub applied: Option<Applied>,
    /// What was skipped, or not read, and why. They name no buffer text.
    pub notes: Vec<String>,
}

impl Found {
    /// The tab width the file is shown with: EditorConfig's, or the configured one.
    pub fn tab_width(&self, configured: Settings) -> TabWidth {
        match &self.applied {
            Some(applied) => {
                indent::with_editorconfig(configured, Default::default(), &applied.properties)
                    .0
                    .tab_width()
            }
            None => configured.tab_width(),
        }
    }
}

/// The EditorConfig files that apply to the file at `location`, the real place its path
/// leads, with `home` the home directory as the person's environment names it.
pub fn read(location: &Path, home: Option<&Path>) -> Found {
    let home = home.and_then(|home| std::fs::canonicalize(home).ok());
    let mut budget = Budget::default();
    let mut found = Found::default();
    let mut files: Vec<(&Path, File)> = Vec::new();
    let mut paths = Vec::new();
    for Above { dir, .. } in upward(location, home.as_deref()) {
        let path = dir.join(NAME);
        let text = match read_checked(&path) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(why) => {
                found.notes.push(format!(
                    "EditorConfig {} is not read: {why}",
                    path.display()
                ));
                continue;
            }
        };
        let file = File::parse(&text, &mut budget);
        for problem in file.problems() {
            found
                .notes
                .push(format!("EditorConfig {}: {problem}", path.display()));
        }
        let root = file.is_root();
        files.push((dir, file));
        paths.push(path);
        if root {
            break;
        }
    }
    if files.is_empty() {
        return found;
    }
    // Farthest first, so that a nearer file wins.
    let mut properties = Properties::default();
    for (source, (dir, file)) in files.iter().enumerate().rev() {
        properties.apply(file, source, &relative(location, dir));
    }
    found.applied = Some(Applied {
        properties,
        files: paths,
    });
    found
}

/// The path of `location` from `dir`, its parts joined by `/`, as EditorConfig's globs read it.
fn relative(location: &Path, dir: &Path) -> String {
    let parts = location.strip_prefix(dir).unwrap_or(location).iter();
    let parts: Vec<_> = parts.map(|part| part.to_string_lossy()).collect();
    parts.join("/")
}

/// The text of the `.editorconfig` at `path`, or `None` if there is none, or why it is not
/// read.
fn read_checked(path: &Path) -> Result<Option<String>, &'static str> {
    let (file, metadata) = match oso_platform::file_meta::open_regular_here(path) {
        Ok(Some(opened)) => opened,
        Ok(None) => return Err("it is a link, or not a regular file"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("it cannot be opened"),
    };
    if !ours(&metadata) || others_can_write(&metadata) {
        return Err("another user owns it or can change it");
    }
    let too_large = "it is larger than 64 KiB";
    if metadata.len() > MAX_FILE as u64 {
        return Err(too_large);
    }
    let mut bytes = Vec::new();
    if file
        .take(MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Err("it cannot be read");
    }
    if bytes.len() > MAX_FILE {
        return Err(too_large);
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "it is not UTF-8 text")
}

/// Whether every user may write what `metadata` describes. Windows has no modes to go by.
fn others_can_write(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    return std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o002 != 0;
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

/// What the threads reading after `C-x C-w` found, for the UI to take.
pub type Answers = Arc<Mutex<Vec<(ProjectRequest, Found)>>>;

/// Reads the EditorConfig of `request`'s file on a thread of its own, as `C-x C-w` named it,
/// puts what it found in `answers` and calls `wake`.
pub fn spawn_read(
    request: ProjectRequest,
    home: Option<PathBuf>,
    answers: &Answers,
    wake: impl FnOnce() + Send + 'static,
) {
    let answers = Arc::clone(answers);
    let started = std::thread::Builder::new()
        .name("oso-editorconfig".into())
        .spawn(move || {
            // A panic reads nothing, and the buffer keeps what it has.
            let found = std::panic::catch_unwind(|| read(&request.location, home.as_deref()));
            if let Ok(found) = found {
                answers
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((request, found));
                wake();
            }
        });
    if let Err(e) = started {
        log::warn!("cannot start reading EditorConfig: {e}");
    }
}

#[cfg(test)]
mod tests;
