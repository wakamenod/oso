//! Migemo's dictionary (ADR 0116): the `oso-migemo` thread that reads the file
//! `[search] migemo-dictionary` names, at the first search that would use it, and taking in
//! the table it builds, which the editor keeps for the session.
//!
//! Must not depend on: key presses or how a frame is drawn. The file is read and its table
//! built on the thread, never here.
//!
//! - Nothing is read at start-up: the editor says when a search would have used a
//!   dictionary ([`oso_core::editor::Editor::take_migemo_dictionary_wanted`]), and searches
//!   find kana alone until the table is in.
//! - A change of the setting reads the new file at once if a table is in, which serves until
//!   the new one is, else at the next search that wants it; with none named, or a new file
//!   that cannot be read, the table goes. A file that could not be read is tried again only
//!   when the configuration file is read again or the setting changes.
//! - The file is the person's, checked as `config.toml` is (ADR 0108): through links, by who
//!   can change it and the way to it, at most `oso_core::migemo::MAX_BYTES` of UTF-8. A path
//!   from `~/` needs an absolute home directory, so it never names a file in a project.
//! - `*Messages*` says how many readings came, or why none did, and never a word of the
//!   dictionary nor its path.
//! - [`MigemoReader`] decides; the app carries out what it says ([`Effects`]), and drops an
//!   old table on the freeing thread.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;

use oso_core::migemo::{Dictionary, MAX_BYTES};

use super::{App, UserEvent, Waker};

/// What the thread read for a generation of the setting.
#[derive(Debug)]
pub struct DictionaryRead {
    generation: u64,
    /// The table, and whether the users of a group can change the file or the way to it; or
    /// why it was not read.
    read: Result<(Arc<Dictionary>, bool), String>,
}

/// Where a generation's dictionary is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Stage {
    /// Not read yet: the next search that wants it reads it.
    #[default]
    Unread,
    Reading,
    /// The editor has it.
    Read,
    /// It could not be read; not tried again until the configuration says so.
    Failed,
}

/// What the setting names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Named {
    File(PathBuf),
    /// A path from `~/`, with no absolute home directory known.
    NoHome,
    /// A path from `~/` that the home directory makes one on another computer or a device.
    Elsewhere,
}

/// What the app is to do after the reader took something in.
#[derive(Debug, Default)]
struct Effects {
    /// The table the editor is to hold now, when that changes.
    give: Option<Option<Arc<Dictionary>>>,
    /// A table no longer used, or read for an older setting, dropped on the freeing thread.
    free: Option<Arc<Dictionary>>,
    /// A file to send to the thread, for a generation.
    read: Option<(u64, PathBuf)>,
    /// What `*Messages*` says.
    notes: Vec<String>,
}

/// The setting, what became of its file, the table handed over, and the thread's queue once
/// it has started.
#[derive(Default)]
pub(super) struct MigemoReader {
    named: Option<Named>,
    /// Counts the setting's changes: what the thread read for an older one is dropped.
    generation: u64,
    stage: Stage,
    /// The table the editor holds, maybe an older setting's: the app's own handle, so a table
    /// replaced is dropped on the freeing thread.
    held: Option<Arc<Dictionary>>,
    jobs: Option<Sender<(u64, PathBuf)>>,
}

/// What `written`, as `[search] migemo-dictionary` holds it, names, with `~/` taken as `home`
/// if that is absolute. The setting itself is an absolute path, or `~/` and a relative one,
/// and not one on another computer; the path `home` makes is judged again.
fn named(written: &str, home: Option<&Path>) -> Named {
    let file = match written.strip_prefix("~/") {
        Some(rest) => match home.filter(|home| home.is_absolute()) {
            Some(home) => home.join(rest),
            None => return Named::NoHome,
        },
        None => PathBuf::from(written),
    };
    if elsewhere(&file) {
        Named::Elsewhere
    } else if file.is_absolute() {
        Named::File(file)
    } else {
        Named::NoHome
    }
}

/// Whether `file` is on another computer or a Windows device: it starts with `//` or `\\`,
/// or with a prefix other than a drive's.
fn elsewhere(file: &Path) -> bool {
    let text = file.as_os_str().to_string_lossy();
    let prefix = match file.components().next() {
        Some(Component::Prefix(prefix)) => !matches!(prefix.kind(), Prefix::Disk(_)),
        _ => false,
    };
    text.starts_with("//") || text.starts_with("\\\\") || prefix
}

/// Reads `file` and builds its table, on the thread.
fn read(file: &Path) -> Result<(Arc<Dictionary>, bool), String> {
    let max = u64::try_from(MAX_BYTES).unwrap_or(u64::MAX);
    let (text, group) = crate::config::read_checked(file, max)?;
    let built = catch_unwind(AssertUnwindSafe(|| Dictionary::new(text)));
    let dictionary = built.map_err(|_| "building its table failed".to_owned())?;
    Ok((Arc::new(dictionary), group))
}

impl MigemoReader {
    /// Takes in the setting as the configuration file now gives it, `named` from what it
    /// holds, on each look at the file.
    fn configure(&mut self, named: Option<Named>) -> Effects {
        let mut effects = Effects::default();
        if named == self.named {
            if self.stage == Stage::Failed && matches!(named, Some(Named::File(_))) {
                self.stage = Stage::Unread;
            }
            return effects;
        }
        self.named = named;
        self.generation += 1;
        self.stage = Stage::Unread;
        match &self.named {
            Some(Named::File(file)) if self.held.is_some() => {
                effects.read = Some((self.generation, file.clone()));
                self.stage = Stage::Reading;
            }
            Some(Named::File(_)) => {}
            Some(Named::NoHome) => {
                self.stage = Stage::Failed;
                effects.notes.push(
                    "Migemo dictionary not read: no absolute home directory is known for ~/"
                        .to_owned(),
                );
                self.drop_held(&mut effects);
            }
            Some(Named::Elsewhere) => {
                self.stage = Stage::Failed;
                effects.notes.push(
                    "Migemo dictionary not read: the home directory puts it on another computer"
                        .to_owned(),
                );
                self.drop_held(&mut effects);
            }
            None => self.drop_held(&mut effects),
        }
        effects
    }

    /// A search would have used a dictionary: the file to read, if one is named and not read
    /// or being read yet.
    fn wanted(&mut self) -> Effects {
        let mut effects = Effects::default();
        if let (Stage::Unread, Some(Named::File(file))) = (self.stage, &self.named) {
            effects.read = Some((self.generation, file.clone()));
            self.stage = Stage::Reading;
        }
        effects
    }

    /// The thread read a dictionary, or could not.
    fn finished(&mut self, event: DictionaryRead) -> Effects {
        let mut effects = Effects::default();
        if event.generation != self.generation || self.stage != Stage::Reading {
            effects.free = event.read.ok().map(|(dictionary, _)| dictionary);
            return effects;
        }
        match event.read {
            Ok((dictionary, group)) => {
                self.stage = Stage::Read;
                if group {
                    let note = crate::config::group_writes_note("Migemo dictionary");
                    effects.notes.push(note);
                }
                effects.notes.push(format!(
                    "Migemo dictionary: {} readings",
                    grouped(dictionary.readings())
                ));
                effects.free = self.held.replace(Arc::clone(&dictionary));
                effects.give = Some(Some(dictionary));
            }
            Err(why) => {
                self.stage = Stage::Failed;
                // The setting no longer names the table held, if any.
                self.drop_held(&mut effects);
                effects
                    .notes
                    .push(format!("Migemo dictionary not read: {why}"));
            }
        }
        effects
    }

    /// The thread could not take a file: none is read until the configuration says so.
    fn could_not_send(&mut self) {
        self.stage = Stage::Failed;
        self.jobs = None;
    }

    /// Takes the table away from the editor, if it holds one.
    fn drop_held(&mut self, effects: &mut Effects) {
        if let Some(held) = self.held.take() {
            effects.give = Some(None);
            effects.free = Some(held);
        }
    }

    /// Sends `file` to the thread, starting it the first time. Fails if it cannot start or
    /// has gone.
    fn send(&mut self, generation: u64, file: PathBuf, waker: &Waker) -> Result<(), ()> {
        if self.jobs.is_none() {
            let (jobs, queue) = mpsc::channel::<(u64, PathBuf)>();
            let waker = waker.clone();
            let spawned = thread::Builder::new()
                .name("oso-migemo".into())
                .spawn(move || {
                    while let Ok(mut job) = queue.recv() {
                        // Only the newest setting's file is worth reading.
                        while let Ok(newer) = queue.try_recv() {
                            job = newer;
                        }
                        let (generation, file) = job;
                        let read = read(&file);
                        let event = DictionaryRead { generation, read };
                        if !waker.send(UserEvent::Migemo(Box::new(event))) {
                            return;
                        }
                    }
                });
            if spawned.is_err() {
                return Err(());
            }
            self.jobs = Some(jobs);
        }
        let sent = self.jobs.as_ref().map(|jobs| jobs.send((generation, file)));
        if matches!(sent, Some(Ok(()))) {
            Ok(())
        } else {
            Err(())
        }
    }
}

impl App {
    /// Takes in `[search] migemo-dictionary` as the configuration file now gives it, on each
    /// look at the file.
    pub(super) fn set_migemo_dictionary(&mut self, written: Option<&str>) {
        let named = written.map(|written| named(written, super::home().as_deref()));
        let effects = self.migemo_reader.configure(named);
        self.carry_out_migemo(effects);
    }

    /// Reads the dictionary named if a search would have used it, and it is not read or being
    /// read yet. Called before the event loop waits.
    pub(super) fn read_migemo_dictionary_if_wanted(&mut self) {
        if self.editor.take_migemo_dictionary_wanted() {
            let effects = self.migemo_reader.wanted();
            self.carry_out_migemo(effects);
        }
    }

    /// The thread read a dictionary, or could not: the editor takes the table, and the
    /// search on screen looks again with it.
    pub(super) fn migemo_dictionary_read(&mut self, event: DictionaryRead) {
        let effects = self.migemo_reader.finished(event);
        self.carry_out_migemo(effects);
    }

    /// Does what the reader said: sends a file, gives the editor its table, says what came
    /// of it, and drops an old table on the freeing thread once the editor let it go.
    fn carry_out_migemo(&mut self, effects: Effects) {
        let Effects {
            give,
            free,
            read,
            notes,
        } = effects;
        if let Some((generation, file)) = read
            && self
                .migemo_reader
                .send(generation, file, &self.waker)
                .is_err()
        {
            self.migemo_reader.could_not_send();
            log::warn!("the migemo dictionary's thread could not start");
        }
        if let Some(dictionary) = give {
            self.editor.set_migemo_dictionary(dictionary);
            self.request_redraw();
        }
        for note in notes {
            self.editor.note(note);
        }
        if let Some(free) = free {
            self.freer.free(free);
        }
    }
}

/// `count` with a comma between each three digits: `137,025`.
fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests;
