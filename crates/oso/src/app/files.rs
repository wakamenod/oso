//! The prompt that reads a path, served from the lister's threads, and the files commands asked
//! to visit (ADR 0087).
//!
//! Must not depend on: key presses or how a frame is drawn. Directories are listed on the
//! lister's threads, never here.

use std::ffi::OsString;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use oso_core::complete::{Name, NameKind};
use oso_core::editor::Looked;

use super::{App, UserEvent, Waker};
use crate::lister::{Answer, Kind, LeadsTo, Lister, Look};

/// The lister for the app, so that its limits hold for the process, waking the event loop with
/// [`UserEvent::Listed`].
pub(super) fn lister(waker: &Waker) -> Lister {
    let waker = Mutex::new(waker.clone());
    Lister::new(move || {
        waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(UserEvent::Listed)
    })
}

impl App {
    /// Starts what commands asked for files: the directories the prompt that reads a path needs
    /// listed, and the files to visit.
    pub(super) fn serve_files(&mut self) {
        for directory in self.editor.take_file_requests() {
            // A generation is not needed: listings are kept by directory for the prompt's life.
            self.lister.list(directory, 0);
        }
        for (directory, names) in self.editor.take_file_looks() {
            let names = names.into_iter().map(OsString::from).collect();
            self.lister.look(directory, names, 0);
        }
        for (path, limit) in self.editor.take_visits() {
            if let Some(buffer) = self.visit(path, limit) {
                self.editor.show(buffer);
            }
            self.request_redraw();
        }
    }

    /// The lister answered: gives the prompt what it found.
    pub(super) fn listed(&mut self) {
        let mut changed = false;
        let now = SystemTime::now();
        for answer in self.lister.take() {
            let (directory, entries, truncated) = match answer {
                Answer::Listed {
                    directory,
                    entries,
                    truncated,
                    ..
                } => (directory, entries, truncated),
                Answer::Looked {
                    directory,
                    looks,
                    missed,
                    ..
                } => {
                    // A name that could not be looked at is answered as found to be no directory,
                    // with nothing to say of it, so that nothing waits for it (ADR 0114). Its 0
                    // bytes only mean that `C-x C-f` does not ask first: the loader compares the
                    // size of the file it opened with the limit before it reads any of it, so a
                    // file past it still stops there and asks (ADR 0087).
                    let missed = missed.iter().map(|name| {
                        let looked = Looked {
                            annotation: String::new(),
                            bytes: 0,
                            directory: false,
                        };
                        (name.to_string_lossy().into_owned(), looked)
                    });
                    let looks = looks
                        .iter()
                        .map(|look| looked(look, now))
                        .chain(missed)
                        .collect();
                    self.editor.file_looked(&directory, looks);
                    changed = true;
                    continue;
                }
            };
            let names = entries.ok().map(|entries| {
                // A name that is not UTF-8 is left out, as a path made from a lossy copy of it
                // would name another file.
                entries
                    .iter()
                    .filter_map(|entry| Some((entry, entry.name.to_str()?)))
                    .map(|(entry, name)| Name {
                        name: name.to_owned(),
                        kind: match entry.kind {
                            Kind::File => NameKind::File,
                            Kind::Directory => NameKind::Directory,
                            Kind::Link => NameKind::Link,
                            Kind::Other => NameKind::Other,
                        },
                    })
                    .collect::<Arc<[Name]>>()
            });
            self.editor.file_listed(directory, names, truncated);
            changed = true;
        }
        self.serve_files();
        if changed {
            self.request_redraw();
        }
    }

    /// The prompt that read a path has closed: what waits for its listings goes.
    pub(super) fn forget_listings(&mut self) {
        // Only once it has closed: a question over it must not drop what it waits for.
        if !self.editor.file_prompt_open() {
            self.lister.forget();
        }
    }
}

/// What a row shows of a name looked at, as marginalia's annotation for a file: the mode as
/// `ls` writes it, the size, the age, whether another user owns it, and where a link leads.
fn looked(look: &Look, now: SystemTime) -> (String, Looked) {
    let mut annotation = format!(
        "{}  {:>5}  {}",
        mode_text(look.mode),
        crate::dispatch::size(usize::try_from(look.len).unwrap_or(usize::MAX)),
        look.modified
            .map_or_else(String::new, |modified| age(now, modified)),
    );
    if look.others {
        annotation.push_str("  another user's");
    }
    if let Some(target) = &look.target {
        annotation.push_str(&format!("  → {}", target.display()));
        if look.leads_to == Some(LeadsTo::Missing) {
            annotation.push_str(" (missing)");
        }
    }
    let name = look.name.to_string_lossy().into_owned();
    let directory = look.mode & 0o170_000 == 0o040_000 || look.leads_to == Some(LeadsTo::Directory);
    let looked = Looked {
        annotation,
        bytes: look.len,
        directory,
    };
    (name, looked)
}

/// Unix mode bits as `ls -l` writes them, such as `drwxr-xr-x`.
fn mode_text(mode: u32) -> String {
    let kind = match mode & 0o170_000 {
        0o040_000 => 'd',
        0o120_000 => 'l',
        0o010_000 => 'p',
        0o020_000 => 'c',
        0o060_000 => 'b',
        0o140_000 => 's',
        _ => '-',
    };
    let bits = ['r', 'w', 'x'];
    let mut text = String::with_capacity(10);
    text.push(kind);
    for shift in (0..9).rev() {
        let on = mode & (1 << shift) != 0;
        text.push(if on { bits[(8 - shift) % 3] } else { '-' });
    }
    text
}

/// How long ago `then` was, as marginalia says it: `now`, `5 min ago`, `3 days ago`.
fn age(now: SystemTime, then: SystemTime) -> String {
    let seconds = now.duration_since(then).unwrap_or(Duration::ZERO).as_secs();
    let (count, unit) = match seconds {
        0..60 => return "now".to_owned(),
        60..3600 => (seconds / 60, "min"),
        3600..86_400 => (seconds / 3600, "hour"),
        86_400..2_592_000 => (seconds / 86_400, "day"),
        2_592_000..31_536_000 => (seconds / 2_592_000, "month"),
        _ => (seconds / 31_536_000, "year"),
    };
    let plural = if count == 1 || unit == "min" { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mode_reads_as_ls_writes_it() {
        assert_eq!(mode_text(0o040_755), "drwxr-xr-x");
        assert_eq!(mode_text(0o100_644), "-rw-r--r--");
        assert_eq!(mode_text(0o120_777), "lrwxrwxrwx");
    }

    #[test]
    fn an_age_reads_as_marginalia_says_it() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100_000_000);
        let ago = |seconds| age(now, now - Duration::from_secs(seconds));
        assert_eq!(ago(10), "now");
        assert_eq!(ago(300), "5 min ago");
        assert_eq!(ago(3600), "1 hour ago");
        assert_eq!(ago(3 * 86_400), "3 days ago");
        assert_eq!(
            age(now, now + Duration::from_secs(5)),
            "now",
            "a time ahead"
        );
    }
}
