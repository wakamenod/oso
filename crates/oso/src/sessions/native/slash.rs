//! A native session's slash commands (ADR 0165, step 2d, the author's (a) on Q1 to Q3): those
//! its CLI offers, which the composer completes over after a `/`, which a send runs as a
//! command, and which oso refuses to send.
//!
//! Must not depend on: threads, the stream or the UI; the caller gives what the CLI listed.
//!
//! - The list is the last `initialize` answer of the session's processes, with each command's
//!   description, argument hint and aliases; until a process gives one, as on tier 1, each
//!   `system/init`'s names. A new process keeps the old list until its own comes.
//! - A text runs a command only when it starts with `/` itself and its first word is one the
//!   CLI lists (measured, CLI 2.1.330): the CLI then replays nothing a person wrote, so the
//!   person's row is shown at once. Any other text is a message, replayed as one. Words end at
//!   whitespace as the CLI's do, a no-break space and U+FEFF included.
//! - `/clear` and its aliases are refused (Q2 (a)): in `-p` they start a conversation with a
//!   new session id, which oso's record would not follow, so a resume or a fork would reach the
//!   conversation before it. Once the list says which commands are the CLI's own, a word any
//!   listed `clear` of the CLI's answers to is, whatever comes first, and not a skill of the
//!   project's named so; until then, the names alone are. They are left out of completion.
//! - The commands only a terminal's TUI runs are left out of completion, but go if typed
//!   (Q3 (a)): those `system/init` lists, and until it does, those measured.

use super::wire::slash::SlashCommand;

/// `/clear`'s names, refused before the CLI lists its own.
const CLEARS: [&str; 3] = ["clear", "reset", "new"];

/// The commands only a terminal's TUI runs, as measured, until `system/init` lists its own.
const TERMINAL_ONLY: [&str; 4] = ["doctor", "color", "focus", "reload-plugins"];

/// Why `/clear` is refused.
pub const CLEAR_REFUSED: &str =
    "/clear would start a conversation oso does not follow: start a new session instead";

/// One session's slash commands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlashCommands {
    listed: Vec<SlashCommand>,
    /// The list is an `initialize` answer's, not names alone.
    described: bool,
    /// This process's `initialize` answered with one.
    answered: bool,
    /// `system/init`'s terminal-only names, once it listed them.
    terminal: Option<Vec<String>>,
    /// Counts each change, so a caller knows when to show the list again.
    version: u64,
}

impl SlashCommands {
    /// A new process starts: the list stays until its own comes.
    pub fn new_process(&mut self) {
        self.answered = false;
    }

    /// `initialize` answered `commands`: they are the list.
    pub fn initialized(&mut self, commands: Vec<SlashCommand>) {
        self.listed = commands;
        self.described = true;
        self.answered = true;
        self.version += 1;
    }

    /// `system/init` listed `names`, and `terminal` as only a terminal's TUI runs: the names
    /// are the list while this process's `initialize` gave none.
    pub fn system_init(&mut self, names: Vec<String>, terminal: Vec<String>) {
        let mut changed = self.terminal.as_ref() != Some(&terminal);
        if changed {
            self.terminal = Some(terminal);
        }
        let same = (self.listed.iter().map(|command| &command.name)).eq(names.iter());
        if !self.answered && !same {
            self.listed = (names.into_iter())
                .map(|name| SlashCommand {
                    name,
                    description: String::new(),
                    hint: String::new(),
                    aliases: Vec::new(),
                    builtin: false,
                })
                .collect();
            self.described = false;
            changed = true;
        }
        if changed {
            self.version += 1;
        }
    }

    /// Counts each change of what [`SlashCommands::offered`] gives.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Whether `text`, sent, runs a command the CLI lists.
    pub fn runs(&self, text: &str) -> bool {
        first_word(text).is_some_and(|word| self.find(word).is_some())
    }

    /// Whether the list says which commands are the CLI's own: an `initialize` answer's that
    /// marks one so. Names alone, or a CLI that marks none, do not.
    pub fn says_whose(&self) -> bool {
        self.described && self.listed.iter().any(|command| command.builtin)
    }

    /// Why `text` is not sent, if it is not: when any command it names is the CLI's own
    /// `clear`, or, while the list does not say whose a command is, when it is one of its
    /// names.
    pub fn refused(&self, text: &str) -> Option<&'static str> {
        let word = first_word(text)?;
        let named = |command: &&SlashCommand| {
            command.name == word || command.aliases.iter().any(|alias| alias == word)
        };
        let clears = match self.says_whose() {
            true => self.listed.iter().filter(named).any(clears),
            false => CLEARS.contains(&word),
        };
        clears.then_some(CLEAR_REFUSED)
    }

    /// The commands completion offers, in the CLI's order.
    pub fn offered(&self) -> impl Iterator<Item = &SlashCommand> {
        self.listed.iter().filter(|command| {
            let cleared = match self.says_whose() {
                true => clears(command),
                false => CLEARS.contains(&command.name.as_str()),
            };
            let terminal = match &self.terminal {
                Some(terminal) => terminal.contains(&command.name),
                None => TERMINAL_ONLY.contains(&command.name.as_str()),
            };
            !cleared && !terminal
        })
    }

    /// The command named or aliased `word`.
    fn find(&self, word: &str) -> Option<&SlashCommand> {
        (self.listed.iter())
            .find(|command| command.name == word || command.aliases.iter().any(|a| a == word))
    }
}

/// Whether `command` is the CLI's own `/clear`.
fn clears(command: &SlashCommand) -> bool {
    command.builtin && command.name == "clear"
}

/// The word after the `/` that starts `text`, if it starts with one.
pub fn first_word(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    let word = rest.split(ends_word).next()?;
    (!word.is_empty()).then_some(word)
}

/// Whether `c` ends a command's word, as the CLI reads it.
pub fn ends_word(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

#[cfg(test)]
mod tests;
