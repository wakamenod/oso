//! The user configuration file in the app: what its thread found is carried out on the UI
//! thread, the user's keys between keys, and what is wrong is said in `*Messages*`
//! (ADR 0108, ADR 0110). `config::outcome` decides what that is. A setting applies at start,
//! and again only when its value in the file changes, so a choice made at run time stands until
//! then: the settings in force are kept, and each look compares the file's with them
//! (`config::InForce`).
//!
//! Must not depend on: winit or the GPU; this only carries out what the thread brings.

use std::path::PathBuf;

use oso_core::keymap::Keymap;

use super::{App, UserEvent, theme_names};
use crate::config::{InForce, Reader, Settings, Source, outcome};

/// The configuration's thread, what has been said of the file, and the settings in force.
pub(super) struct Configuration {
    reader: Option<Reader>,
    /// The configuration directories the agent channel never saves into.
    directories: Vec<PathBuf>,
    /// What problems are said to be in: the file's name.
    name: String,
    /// Whether the last look found something wrong, so that a good one after says so.
    troubled: bool,
    /// The settings in force: the compiled-in ones until a look applies the file's, with the
    /// default theme remembered with the theme shown last.
    in_force: InForce,
    /// Whether the launch's look has come: only it starts the agent channel.
    looked: bool,
    /// Whether a look has applied settings.
    applied: bool,
    /// A dev restart's handoff set the theme and the aids before any look applied settings: the
    /// next takes the file's as those in force without applying them (ADR 0108).
    #[cfg(feature = "dev")]
    handed_over: bool,
}

impl Configuration {
    /// No configuration yet, with `themes`, the built-in themes, the default first.
    pub(super) fn new(themes: &[(&'static str, &'static str)]) -> Self {
        let names: Vec<&str> = themes.iter().map(|&(name, _)| name).collect();
        Self {
            reader: None,
            directories: Vec::new(),
            name: String::new(),
            troubled: false,
            in_force: InForce::compiled(&names),
            looked: false,
            applied: false,
            #[cfg(feature = "dev")]
            handed_over: false,
        }
    }

    /// The configuration's default theme in force: the one remembered with the theme shown
    /// last, until a look applies the file's.
    pub(super) fn default_theme(&self) -> &str {
        &self.in_force.theme
    }

    /// Takes `name`, remembered with the theme shown last, as the default theme in force,
    /// for the file's to be compared with.
    pub(super) fn set_default_theme(&mut self, name: String) {
        if !self.applied {
            self.in_force.theme = name;
        }
    }

    /// A dev restart's handoff set the theme and the aids: they win over the file's.
    #[cfg(feature = "dev")]
    pub(super) fn hand_over(&mut self) {
        if !self.applied {
            self.handed_over = true;
        }
    }
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
        self.configuration.reader = reader;
        self.configuration.directories = source.directories();
        self.configuration.name = name;
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

    /// Carries out what the configuration's thread found, if it found something new. Not
    /// before the window opens, which takes the remembered theme first, for the file's default
    /// to be compared with: what the thread found waits for it.
    pub(super) fn configuration_reported(&mut self) {
        let configuration = &self.configuration;
        if self.view.is_none() {
            return;
        }
        let Some(report) = configuration.reader.as_ref().and_then(Reader::take) else {
            return;
        };
        let outcome = outcome(report, &configuration.name, configuration.troubled);
        if let Some(keys) = outcome.keys {
            self.replace_user_keymap(keys);
        }
        let launch = !std::mem::replace(&mut self.configuration.looked, true);
        let later = outcome
            .settings
            .and_then(|settings| self.apply_settings(&settings, launch));
        for note in outcome.notes {
            self.editor.note(note);
        }
        match (outcome.said, later) {
            (Some(said), later) => {
                if let Some(later) = later {
                    self.editor.note(later);
                }
                self.editor.message(said);
            }
            (None, Some(later)) => self.editor.message(later),
            (None, None) => {}
        }
        self.configuration.troubled = outcome.troubled;
    }

    /// Applies each of `settings` whose value differs from the one in force, and starts the
    /// agent channel if this is the `launch`'s look and they say so. Returns what to say of a
    /// change that applies only at the next start.
    fn apply_settings(&mut self, settings: &Settings, launch: bool) -> Option<String> {
        let themes = theme_names();
        let names: Vec<&str> = themes.iter().map(|&(name, _)| name).collect();
        let index = |name: &str| names.iter().position(|&theme| theme == name);
        let now = InForce::of(settings, &names);
        let pair = now.pair.each_ref().map(|name| index(name));
        let theme = (now.theme != self.configuration.in_force.theme)
            .then(|| index(&now.theme))
            .flatten();
        let configuration = &mut self.configuration;
        let before = std::mem::replace(&mut configuration.in_force, now);
        configuration.applied = true;
        #[cfg(feature = "dev")]
        let handed_over = std::mem::take(&mut configuration.handed_over);
        #[cfg(not(feature = "dev"))]
        let handed_over = false;
        let now = &configuration.in_force;
        if !handed_over {
            let aids = now.aids_over(&before, self.editor.visual_aids());
            if aids != self.editor.visual_aids() {
                self.editor.set_visual_aids(aids);
            }
            // Quietly: the person changed the file, or set the theme to start in.
            if let Some(theme) = theme {
                self.editor.set_themes(themes, theme);
            }
        }
        if let [Some(first), Some(second)] = pair {
            self.editor.set_theme_pair([first, second]);
        }
        let start = now.agent_channel_start;
        let changed = start != before.agent_channel_start;
        self.remember_theme();
        self.request_redraw();
        if launch {
            if start {
                let name = self.configuration.name.clone();
                self.start_agent_channel_as_configured(&name);
            }
            return None;
        }
        changed.then(|| {
            format!(
                "{}: [agent-channel] start = {start} applies from the next start",
                self.configuration.name
            )
        })
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
