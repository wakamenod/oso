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
//!   the new one is, else at the next search that wants it; with none named, the table goes.
//!   A file that could not be read is tried again only when the configuration file is read
//!   again or the setting changes.
//! - The file is the person's, checked as `config.toml` is (ADR 0108): through links, by who
//!   can change it and the way to it, at most `oso_core::migemo::MAX_BYTES` of UTF-8.
//! - `*Messages*` says how many readings came, or why none did, and never a word of the
//!   dictionary nor its path.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Not read yet: the next search that wants it reads it.
    Unread,
    Reading,
    /// The editor has it.
    Read,
    /// It could not be read; not tried again until the configuration says so.
    Failed,
}

/// The setting, what became of its file, and the thread's queue once it has started.
pub(super) struct MigemoReader {
    /// The file the setting names, `~/` taken as the home directory; `None` for none, or for a
    /// `~/` path with no home directory known.
    named: Option<PathBuf>,
    /// Counts the setting's changes: what the thread read for an older one is dropped.
    generation: u64,
    stage: Stage,
    /// Whether the editor holds a table, maybe an older setting's.
    holds: bool,
    jobs: Option<Sender<(u64, PathBuf)>>,
}

impl Default for MigemoReader {
    fn default() -> Self {
        Self {
            named: None,
            generation: 0,
            stage: Stage::Unread,
            holds: false,
            jobs: None,
        }
    }
}

/// `written`, as `[search] migemo-dictionary` holds it, with `~/` taken as `home`.
fn expand(written: &str, home: Option<&Path>) -> Option<PathBuf> {
    match written.strip_prefix("~/") {
        Some(rest) => home.map(|home| home.join(rest)),
        None => Some(PathBuf::from(written)),
    }
}

/// Reads `file` and builds its table, on the thread.
fn read(file: &Path) -> Result<(Arc<Dictionary>, bool), String> {
    let max = u64::try_from(MAX_BYTES).unwrap_or(u64::MAX);
    let (text, group) = crate::config::read_named(file, max)?;
    let built = catch_unwind(AssertUnwindSafe(|| Dictionary::new(text)));
    let dictionary = built.map_err(|_| "building its table failed".to_owned())?;
    Ok((Arc::new(dictionary), group))
}

impl MigemoReader {
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
        if !matches!(sent, Some(Ok(()))) {
            self.jobs = None;
            return Err(());
        }
        Ok(())
    }
}

impl App {
    /// Takes in `[search] migemo-dictionary` as the configuration file now gives it, on each
    /// look at the file: a file that could not be read is tried again, and a change reads
    /// the new one at once if a table is in, or drops the table if none is named.
    pub(super) fn set_migemo_dictionary(&mut self, written: Option<&str>) {
        let named = written.and_then(|written| expand(written, super::home().as_deref()));
        let reader = &mut self.migemo_reader;
        if named == reader.named {
            if reader.stage == Stage::Failed {
                reader.stage = Stage::Unread;
            }
            return;
        }
        reader.named = named;
        reader.generation += 1;
        reader.stage = Stage::Unread;
        if reader.named.is_none() {
            if std::mem::take(&mut reader.holds) {
                self.editor.set_migemo_dictionary(None);
                self.request_redraw();
            }
        } else if reader.holds {
            self.read_migemo_dictionary();
        }
    }

    /// Reads the dictionary named if a search would have used it, and it is not read or being
    /// read yet. Called before the event loop waits.
    pub(super) fn want_migemo_dictionary(&mut self) {
        if self.editor.take_migemo_dictionary_wanted() && self.migemo_reader.stage == Stage::Unread
        {
            self.read_migemo_dictionary();
        }
    }

    /// Sends the file named to the thread.
    fn read_migemo_dictionary(&mut self) {
        let reader = &mut self.migemo_reader;
        let Some(file) = reader.named.clone() else {
            return;
        };
        let generation = reader.generation;
        if reader.send(generation, file, &self.waker).is_err() {
            reader.stage = Stage::Failed;
            log::warn!("the migemo dictionary's thread could not start");
            return;
        }
        reader.stage = Stage::Reading;
    }

    /// The thread read a dictionary, or could not: the editor takes the table, and the
    /// search on screen looks again with it.
    pub(super) fn migemo_dictionary_read(&mut self, event: DictionaryRead) {
        let reader = &mut self.migemo_reader;
        if event.generation != reader.generation || reader.stage != Stage::Reading {
            return;
        }
        match event.read {
            Ok((dictionary, group)) => {
                reader.stage = Stage::Read;
                reader.holds = true;
                if group {
                    self.editor.note(
                        "Migemo dictionary: other users in a group can change it, or the way to \
                         it (on macOS, a group such as staff can hold every account)",
                    );
                }
                self.editor.note(format!(
                    "Migemo dictionary: {} readings",
                    grouped(dictionary.readings())
                ));
                self.editor.set_migemo_dictionary(Some(dictionary));
            }
            Err(why) => {
                reader.stage = Stage::Failed;
                // The setting no longer names the table held, if any.
                if std::mem::take(&mut reader.holds) {
                    self.editor.set_migemo_dictionary(None);
                }
                self.editor
                    .note(format!("Migemo dictionary not read: {why}"));
            }
        }
        self.request_redraw();
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
mod tests {
    use std::path::{Path, PathBuf};

    use super::{expand, grouped};

    #[test]
    fn a_path_from_home_is_taken_from_the_home_directory() {
        let home = Path::new("/home/me");
        assert_eq!(
            expand("~/dict/migemo-dict", Some(home)),
            Some(PathBuf::from("/home/me/dict/migemo-dict"))
        );
        assert_eq!(expand("~/dict", None), None);
        assert_eq!(
            expand("/usr/share/dict", Some(home)),
            Some(PathBuf::from("/usr/share/dict"))
        );
    }

    #[test]
    fn counts_have_a_comma_between_each_three_digits() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(137_025), "137,025");
        assert_eq!(grouped(1_048_576), "1,048,576");
    }
}
