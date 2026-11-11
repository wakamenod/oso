//! The user configuration file in the app: what its thread found is carried out on the UI
//! thread, the user's keys between keys, and what is wrong is said in `*Messages*`
//! (ADR 0108, ADR 0110). `config::outcome` decides what that is; the settings come in a later
//! step.
//!
//! Must not depend on: winit or the GPU; this only carries out what the thread brings.

use std::path::PathBuf;

use oso_core::keymap::Keymap;

use super::{App, UserEvent};
use crate::config::{Reader, Source, outcome};

/// The configuration's thread and what has been said of the file.
#[derive(Default)]
pub(super) struct Configuration {
    reader: Option<Reader>,
    /// The configuration directories the agent channel never saves into.
    directories: Vec<PathBuf>,
    /// What problems are said to be in: the file's name.
    name: String,
    /// Whether the last look found something wrong, so that a good one after says so.
    troubled: bool,
}

impl App {
    /// Takes the configuration from `source`, which `reader` reads, and has it wake the event
    /// loop when it finds something new. Called once, before the window opens.
    pub fn set_configuration(&mut self, source: &Source, reader: Option<Reader>) {
        if let Some(reader) = &reader {
            let waker = self.waker.clone();
            reader.set_wake(move || {
                let _ = waker.send(UserEvent::Configuration);
            });
        }
        let name = source
            .file()
            .and_then(|file| {
                file.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        self.configuration = Configuration {
            reader,
            directories: source.directories(),
            name,
            troubled: false,
        };
    }

    /// The configuration directories the agent channel never saves into (ADR 0108).
    pub(super) fn configuration_directories(&self) -> Vec<PathBuf> {
        self.configuration.directories.clone()
    }

    /// Asks the configuration's thread to look at the file now: oso gained the focus, or saved.
    pub(super) fn look_at_configuration_again(&self) {
        if let Some(reader) = &self.configuration.reader {
            reader.look_again();
        }
    }

    /// Carries out what the configuration's thread found, if it found something new.
    pub(super) fn configuration_reported(&mut self) {
        let configuration = &self.configuration;
        let Some(report) = configuration.reader.as_ref().and_then(Reader::take) else {
            return;
        };
        let outcome = outcome(report, &configuration.name, configuration.troubled);
        if let Some(keys) = outcome.keys {
            self.replace_user_keymap(keys);
        }
        for note in outcome.notes {
            self.editor.note(note);
        }
        if let Some(said) = outcome.said {
            self.editor.message(said);
        }
        self.configuration.troubled = outcome.troubled;
    }

    /// Puts in the user's keys, and drops those they replace on the freeing thread: a large
    /// keymap takes milliseconds to drop.
    fn replace_user_keymap(&mut self, keys: Keymap) {
        if let Some(old) = self.dispatcher.set_user_keymap(keys) {
            self.freer.free(old);
        }
    }

    /// Drops on the freeing thread the user's keymaps that ones waiting for a prefix key replaced.
    pub(super) fn free_replaced_keymap(&mut self) {
        while let Some(old) = self.dispatcher.take_replaced_keymap() {
            self.freer.free(old);
        }
    }
}
