//! The dev socket's `run-command` (ADR 0044): a command by its name, with a count or a number
//! for its argument and never a prompt for one, refused while a passphrase prompt is open or a
//! prompt that completes is open beside a buffer that may hold secrets.
//!
//! Must not depend on: the socket or `app`; the host hands over the name and the argument.

use oso_core::command::{ArgSpec, Effects, Invocation};
use oso_core::editor::Editor;

use super::{Dispatcher, Step, args};

impl Dispatcher {
    /// Runs the command named `name`, as the control socket's `run-command`
    /// does, with `arg` as its numeric argument, or else a prefix argument left by
    /// the keys before it. Returns what the host should do, which matters even if
    /// the command failed: a motion that hits the buffer end still moves as far as
    /// it can.
    pub fn run_command(
        &mut self,
        editor: &mut Editor,
        name: &str,
        arg: Option<isize>,
    ) -> (Effects, Result<(), String>) {
        if self.waits_for_clipboard() {
            return (
                Effects::default(),
                Err("a yank is waiting for the clipboard".into()),
            );
        }
        self.as_input(editor, |this, editor| this.run_named(editor, name, arg))
    }

    fn run_named(
        &mut self,
        editor: &mut Editor,
        name: &str,
        arg: Option<isize>,
    ) -> (Effects, Result<(), String>) {
        let found = self
            .registry
            .lookup(name)
            .and_then(|id| Some((id, self.registry.get(id)?)));
        let Some((id, command)) = found else {
            return (
                Effects::default(),
                Err(format!("no command named {name:?}")),
            );
        };
        // A reply could say something of what is typed there.
        // Any open one, also beneath a prompt opened from it.
        let passphrase = editor
            .prompts()
            .iter()
            .any(|prompt| prompt.kind() == oso_core::minibuffer::Kind::Passphrase);
        if passphrase && name != "keyboard-quit" {
            return (
                Effects::default(),
                Err("a passphrase prompt is open; only keyboard-quit runs".into()),
            );
        }
        // A command from the socket drops a key held for a result, as a key does, and does not
        // take a candidate of the rows of an older input (ADR 0096).
        self.completion_held = None;
        if super::held::waits(editor, name) {
            return (
                Effects::default(),
                Err(
                    "the list or a listing is still on its way; wait until get-state says nothing is \
                     pending"
                        .into(),
                ),
            );
        }
        // Whether a command found a match would say something of an answer withheld beside a
        // buffer that may hold secrets (ADR 0076).
        let completing = editor
            .prompts()
            .iter()
            .any(|prompt| prompt.candidates().is_some());
        if completing && editor.around_secret() && name != "keyboard-quit" {
            return (
                Effects::default(),
                Err("a prompt that completes is open beside a secret buffer; only keyboard-quit runs".into()),
            );
        }
        // The socket never opens a prompt that waits for a person: it gives the answer.
        let given = arg.map_or(args::Given::Nothing, args::Given::Count);
        let mapped = match args::map(name, command.args, given, self.prefix) {
            Ok(mapped) => mapped,
            Err(refused) => return (Effects::default(), Err(refused)),
        };
        // The prefix the keys left is the command's now, or was replaced by the argument.
        self.prefix = None;
        let invocation = Invocation {
            answer: mapped.answer.as_deref(),
            ..Invocation::default()
        };
        let args = mapped.args;
        // A command isearch's keys do not run ends a search first, as a key would
        // (ADR 0082).
        let ended_search = !self.isearch_commands.contains(&id) && editor.isearch_exit();
        // And one its answers do not run ends a query-replace (ADR 0113).
        let ended_search =
            ended_search | (!self.replace_commands.contains(&id) && editor.replace_exit());
        // A yank waits for the clipboard as it does from a key, and replies at once.
        let (effects, result) = if command.args == ArgSpec::CountAndClipboard {
            self.run_or_read(editor, id, args, invocation)
        } else {
            self.execute(editor, id, args, invocation, Step::MayJoin)
        };
        let mut effects = effects.union(self.after_prompts(editor));
        effects.redraw |= ended_search;
        (effects, result.map_err(|e| e.to_string()))
    }
}
