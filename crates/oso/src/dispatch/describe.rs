//! The key sequence `describe-key` reads, which resolves as a key that runs would, through a
//! terminal's routing first and then the active keymaps, runs nothing and sends nothing, and
//! is described in `*Help*` or the echo area (ADR 0150). Where secrets may be it reads nothing.
//!
//! Must not depend on: `app` or winit.

use oso_core::buffer::BufferId;
use oso_core::command::{CommandId, Effects, Registry};
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::keymap::{Resolution, display_keys};

use super::Dispatcher;
use super::lossage::typing_in_secret;
use super::shown::masked;
use crate::commands::help::{DESCRIBE_KEY, SECRET, show_internal};

/// The internal buffer `describe-key` describes in.
const HELP: &str = "*Help*";

/// `describe-key`, and the sequence it reads.
pub(super) struct DescribeKey {
    command: Option<CommandId>,
    /// The keys read so far, while it reads.
    reading: Option<Vec<KeyPress>>,
    buffer: Option<BufferId>,
}

impl DescribeKey {
    pub(super) fn new(registry: &Registry) -> Self {
        Self {
            command: registry.lookup(DESCRIBE_KEY),
            reading: None,
            buffer: None,
        }
    }
}

/// What a sequence `describe-key` read turned out to be.
enum Found {
    Runs(CommandId, bool),
    Says(&'static str),
    Quit,
}

impl Dispatcher {
    /// Whether `describe-key` is reading a key sequence.
    pub(crate) fn describing(&self) -> bool {
        self.describe.reading.is_some()
    }

    /// Starts reading if `id`, which ran without an error, is `describe-key`. Returns whether
    /// it was.
    pub(super) fn answer_describe_key(&mut self, id: CommandId) -> bool {
        if Some(id) != self.describe.command {
            return false;
        }
        self.describe.reading = Some(Vec::new());
        true
    }

    /// Stops reading, as text or a key where secrets may be does, with the keys read so far
    /// dropped. Nothing is said: what stopped it goes on as it would have.
    pub(super) fn stop_describing(&mut self) {
        if self.describe.reading.take().is_some() {
            self.resolver.reset();
        }
    }

    /// Takes `key` while `describe-key` reads: a prefix waits for the next key, and a whole
    /// sequence is described, with nothing run or sent. `None` when it reads none.
    pub(super) fn describe_takes(&mut self, editor: &mut Editor, key: KeyPress) -> Option<Effects> {
        let mut keys = self.describe.reading.take()?;
        if typing_in_secret(editor) {
            self.resolver.reset();
            editor.echo(SECRET);
            return Some(Effects::default());
        }
        keys.push(key);
        let shown = || display_keys(&keys).to_string();
        if let Some(goes) = self.terminal_takes(editor, key) {
            self.resolver.reset();
            // A character for the program may be a password the terminal has not seen asked
            // for, and the echo area is the agent channel's to read.
            editor.echo(format!("{} {goes}", masked(&keys, true)));
            return Some(Effects {
                redraw: true,
                ..Effects::default()
            });
        }
        let found = {
            let reading = self.keymaps.reading(editor);
            let pending = !self.resolver.pending().is_empty();
            let (layers, fed) = (self.keymaps).for_key(&reading, self.prefix, pending, editor, key);
            match self.resolver.feed(&layers, fed) {
                Resolution::Pending => None,
                Resolution::Command(id) => Some(Found::Runs(id, false)),
                Resolution::Shifted(id) => Some(Found::Runs(id, true)),
                Resolution::Menu(_) => Some(Found::Says("opens a menu")),
                Resolution::Unbound(_) => Some(Found::Says("is undefined")),
                Resolution::Cancelled => Some(Found::Quit),
            }
        };
        let Some(found) = found else {
            self.describe.reading = Some(keys);
            return Some(Effects::default());
        };
        match found {
            Found::Runs(id, shifted) => {
                let text = self.described(&shown(), id, shifted);
                show_internal(editor, &mut self.describe.buffer, HELP, &text, false);
            }
            Found::Says(what) => editor.echo(format!("{} {what}", shown())),
            Found::Quit => editor.echo("Quit"),
        }
        Some(Effects {
            redraw: true,
            ..Effects::default()
        })
    }

    /// What `*Help*` says of `keys`, which run `id`.
    fn described(&self, keys: &str, id: CommandId, shifted: bool) -> String {
        let Some(command) = self.registry.get(id) else {
            return format!("{keys} runs a command that is gone\n");
        };
        let name = command.name;
        let how = if shifted {
            ", bound to the key without Shift; Shift makes a motion select"
        } else {
            ""
        };
        format!("{keys} runs the command {name}{how}.\n\n{}\n", command.doc)
    }
}
