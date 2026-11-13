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
use oso_core::menu::Menus;

use super::{App, UserEvent};
use crate::config::{InForce, Look, MODES, Reader, Report, Settings, Source, outcome};

/// The configuration's thread, what has been said of the file, and the settings in force.
pub(super) struct Configuration {
    reader: Option<Reader>,
    /// The configuration directories the agent channel never saves into.
    directories: Vec<PathBuf>,
    /// What problems are said to be in: the file's name.
    name: String,
    /// Whether the last look found something wrong, so that a good one after says so.
    troubled: bool,
    /// The built-in themes' names, the default first, which settings name.
    names: Vec<&'static str>,
    /// The settings in force: the compiled-in ones until a look applies the file's, with the
    /// default theme remembered with the theme shown last.
    in_force: InForce,
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
        let names: Vec<&'static str> = themes.iter().map(|&(name, _)| name).collect();
        Self {
            reader: None,
            directories: Vec::new(),
            name: String::new(),
            troubled: false,
            in_force: InForce::compiled(&names),
            names,
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

    /// `[sessions] claude` in force: the executable a hosted session runs, by absolute path, if
    /// the file names one (ADR 0121).
    pub(super) fn session_claude(&self) -> Option<&std::path::Path> {
        self.in_force.sessions.claude.as_deref()
    }

    /// `[sessions] env` in force: the variables a hosted session takes from oso's environment.
    #[expect(
        dead_code,
        reason = "the session's terminal passes them (ADR 0120 step 4)"
    )]
    pub(super) fn session_env(&self) -> &[String] {
        &self.in_force.sessions.env
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

    /// Whether a handoff set the theme and the aids before a look applied settings, once.
    #[cfg(feature = "dev")]
    fn take_handed_over(&mut self) -> bool {
        std::mem::take(&mut self.handed_over)
    }

    /// Only a dev build takes a handoff.
    #[cfg(not(feature = "dev"))]
    #[expect(
        clippy::unused_self,
        reason = "the dev build's takes the handoff's mark"
    )]
    fn take_handed_over(&mut self) -> bool {
        false
    }

    /// Where the theme called `name` is among the built-in ones.
    fn theme_index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|&theme| theme == name)
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

    /// Asks the configuration's thread to look at the file now, and the thread of the record of
    /// trusted projects to look at that one, beside it: oso gained the focus, or saved.
    pub(super) fn look_at_configuration_again(&self) {
        if let Some(reader) = &self.configuration.reader {
            reader.look_again();
        }
        if let Some(trust_file) = &self.trust_file {
            trust_file.look_again();
        }
    }

    /// Carries out what the configuration's thread found, if it found something new. Not
    /// before the window opens, which takes the remembered theme first, for the file's default
    /// to be compared with: what the thread found waits for it.
    pub(super) fn configuration_reported(&mut self) {
        if self.view.is_none() {
            return;
        }
        let rules = self
            .configuration
            .reader
            .as_ref()
            .and_then(Reader::take_rules);
        if let Some(rules) = rules {
            self.agent_rules_reported(rules);
        }
        let configuration = &self.configuration;
        let Some((report, launch)) = configuration.reader.as_ref().and_then(Reader::take) else {
            return;
        };
        let mut report = report;
        // The menus go in with the keys that name them; with no file, the built-in ones.
        let menus = match &mut report {
            Report::Read { loaded, .. } => loaded.menus.take(),
            Report::Missing | Report::Refused(_) => None,
        };
        let mut outcome = outcome(report, &configuration.name, configuration.troubled);
        if let Some(keys) = outcome.keys.take() {
            self.replace_user_keymap(keys, menus);
        }
        if let Some(settings) = outcome.settings.take() {
            let look = Look {
                launch,
                handed_over: self.configuration.take_handed_over(),
                group_writes: outcome.group_writes,
            };
            if let Some(later) = self.apply_settings(&settings, look) {
                outcome.say_later(later);
            }
        }
        for note in outcome.notes {
            self.editor.note(note);
        }
        if let Some(said) = outcome.said {
            self.editor.message(said);
        }
        self.configuration.troubled = outcome.troubled;
    }

    /// Carries out what `settings` come to on this `look`, against those in force
    /// (`InForce::changes`). Returns what to say of a change that applies only at the next
    /// start.
    fn apply_settings(&mut self, settings: &Settings, look: Look) -> Option<String> {
        let configuration = &mut self.configuration;
        let now = InForce::of(settings, &configuration.names);
        let changes = now.changes(
            &configuration.in_force,
            self.editor.visual_aids(),
            look,
            &configuration.name,
        );
        configuration.in_force = now;
        configuration.applied = true;
        self.editor
            .set_trim_whitespace(configuration.in_force.trim_whitespace);
        self.editor.set_migemo(configuration.in_force.migemo);
        // On every look, as neither has a run-time switch (ADR 0116).
        let jump = &configuration.in_force.jump;
        self.editor.set_jump_keys(jump.keys.clone());
        self.jump_timer.set_timeout(jump.timeout);
        if let Some(on) = changes.electric_indent {
            self.editor.set_electric_indent(on);
        }
        if changes.mode_indents {
            let indents = configuration.in_force.mode_indents;
            self.editor.set_mode_indents(std::array::from_fn(|index| {
                let (settings, origins) = indents[index];
                (MODES[index].1, settings, origins)
            }));
        }
        if let Some(aids) = changes.aids {
            self.editor.set_visual_aids(aids);
        }
        let configuration = &self.configuration;
        // Quietly. Taking the themes again sets the pair back to the first two, and the
        // configured pair is set after it, on every look.
        if let Some(index) = changes
            .theme
            .and_then(|name| configuration.theme_index(&name))
        {
            self.editor.set_themes(self.themes.clone(), index);
        }
        let pair = &configuration.in_force.pair;
        if let (Some(first), Some(second)) = (
            configuration.theme_index(&pair[0]),
            configuration.theme_index(&pair[1]),
        ) {
            self.editor.set_theme_pair([first, second]);
        }
        self.remember_theme();
        let dictionary = self.configuration.in_force.migemo_dictionary.clone();
        self.set_migemo_dictionary(dictionary.as_deref());
        self.request_redraw();
        if changes.start_channel {
            let name = self.configuration.name.clone();
            self.start_agent_channel_as_configured(&name);
        }
        changes.later
    }

    /// Puts in the user's keys, and drops those they replace on the freeing thread: a large
    /// keymap takes milliseconds to drop.
    fn replace_user_keymap(&mut self, keys: Keymap, menus: Option<Menus>) {
        if let Some(old) = self.dispatcher.set_menus(menus) {
            self.freer.free(old);
        }
        if let Some(old) = self.dispatcher.set_user_keymap(keys) {
            self.freer.free(old);
        }
    }

    /// Drops on the freeing thread the user's keymaps that ones waiting for a prefix key replaced.
    pub(super) fn free_replaced_keymap(&mut self) {
        while let Some(old) = self.dispatcher.take_replaced_keymap() {
            self.freer.free(old);
        }
        while let Some(old) = self.dispatcher.take_replaced_menus() {
            self.freer.free(old);
        }
    }
}
