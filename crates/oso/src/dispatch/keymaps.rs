//! The keymaps a dispatcher holds, and which of them are active at a moment: a prefix
//! argument's, the layer `C-x TAB` opens, a prompt's, a completion's, a review comment's
//! draft's or a question's, a jump's, isearch's, the `lsp` minor mode's in a buffer a language
//! server follows, a mode's, a session's terminal's with the user's `[keys.session]` over it,
//! the user's and the global one (ADR 0034, ADR 0072, ADR 0076, ADR 0082, ADR 0108, ADR 0115,
//! ADR 0116, ADR 0121, ADR 0122, ADR 0132, ADR 0138).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.
//!
//! The default keymaps come from `bindings`, the major modes' in `modes`. The user's keymap
//! from the configuration goes in here too, once no prefix key waits for the one before
//! (`user`).

use oso_core::buffer::{Buffer, ServerMark};
use oso_core::command::{CommandId, Prefix, Registry};
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::keymap::{CANCEL, Keymap, Layers, is_cancel};
use oso_core::minibuffer::Kind;

use crate::bindings;

use super::session_help::SessionHelp;

mod modes;
mod user;

use modes::Modes;

/// Every keymap a dispatcher holds.
pub(super) struct Keymaps {
    pub(super) global: Keymap,
    /// The user's keys from the configuration, just above the global ones (ADR 0108). Empty
    /// without any, and then not a layer.
    pub(super) user: Keymap,
    /// Active while the prefix argument reads digits, and a minus sign. They come
    /// first among the minor modes, as what is being typed is for them.
    pub(super) prefix: [Keymap; 2],
    /// The keys of a prompt that reads an answer, above the global ones (ADR 0072).
    pub(super) minibuffer: Keymap,
    /// The keys of a question, in place of the global ones.
    pub(super) question: Keymap,
    /// The keys of a prompt that completes, with the minibuffer's, above the global ones
    /// (ADR 0076).
    pub(super) completing: Keymap,
    /// The keys of a review comment's draft, above the global ones (ADR 0122).
    pub(super) draft: Keymap,
    /// The keys of a search under way, in place of the others (ADR 0082).
    pub(super) isearch: Keymap,
    /// The keys of a jump under way, in place of the others, isearch's too (ADR 0116).
    pub(super) jump: Keymap,
    /// The answers of a query-replace under way, in place of the others (ADR 0113).
    pub(super) replace: Keymap,
    /// The keys of the layer `C-x TAB` opens, first while it is open (ADR 0115).
    pub(super) rigid: Keymap,
    /// The commands the layer runs rather than closes for.
    pub(super) rigid_commands: Vec<CommandId>,
    /// The completion popup's keys while it is open, before its selection was moved and
    /// after, in the place of `C-x TAB`'s (ADR 0138, step 9).
    pub(super) popup: [Keymap; 2],
    /// The commands the popup stays open for: its own, and typing.
    pub(super) popup_commands: Vec<CommandId>,
    /// `self-insert-command`, the person's typing of which may complete by itself (step 9b).
    pub(super) self_insert: Option<CommandId>,
    /// `keyboard-quit`, which stops a rename waiting for its server (step 14, Q7).
    pub(super) keyboard_quit: Option<CommandId>,
    /// The keys of a server's description at point, in the layer's place while it shows
    /// (ADR 0138, Hover), and the commands it runs rather than closes for.
    pub(super) hover: Keymap,
    pub(super) hover_commands: Vec<CommandId>,
    /// The user's keys from `[keys.session]`, above `session` in a session's terminal
    /// (ADR 0132, step 2). Empty without any, and then not a layer.
    pub(super) user_session: Keymap,
    /// The `lsp` minor mode's keys, above the major mode's in a buffer a language server
    /// follows (ADR 0138, Keys in buffers a server follows).
    pub(super) lsp: Keymap,
    /// Every major mode's keymap.
    modes: Modes,
    /// What `session-help` says: session mode's keys, from its keymap (ADR 0132).
    pub(super) session_help: SessionHelp,
}

impl Keymaps {
    /// The default keymaps over the commands in `registry`, with `global` as the global keymap
    /// and no user keys.
    pub(super) fn new(registry: &Registry, global: Keymap) -> Self {
        let (digits, minus) = bindings::prefix_keymaps(registry);
        let [minibuffer, question, completing, draft] = bindings::prompt_keymaps(registry);
        let modes = Modes::new(registry);
        let (popup, popup_commands) = bindings::popup_keymaps(registry);
        Self {
            global,
            user: Keymap::new(),
            prefix: [digits, minus],
            minibuffer,
            question,
            completing,
            draft,
            isearch: bindings::isearch_keymap(registry),
            jump: bindings::jump_keymap(registry),
            replace: bindings::replace_keymap(registry),
            rigid: bindings::rigid_keymap(registry),
            rigid_commands: bindings::rigid_commands(registry),
            popup,
            popup_commands,
            self_insert: registry.lookup("self-insert-command"),
            keyboard_quit: registry.lookup("keyboard-quit"),
            hover: bindings::hover_keymap(registry),
            hover_commands: bindings::hover_commands(registry),
            session_help: SessionHelp::new(modes.session(), registry),
            modes,
            user_session: Keymap::new(),
            lsp: bindings::lsp_keymap(registry),
        }
    }

    /// Every keymap, active or not, the major modes' last. Each field is named here, so that a
    /// keymap added to the struct cannot be left out of what `M-x` judges commands by.
    pub(super) fn all(&self) -> impl Iterator<Item = &Keymap> {
        let Self {
            global,
            user,
            prefix: [digits, minus],
            minibuffer,
            question,
            completing,
            draft,
            isearch,
            jump,
            replace,
            rigid,
            rigid_commands: _,
            popup: [popup, popup_moved],
            popup_commands: _,
            self_insert: _,
            keyboard_quit: _,
            hover,
            hover_commands: _,
            user_session,
            lsp,
            modes,
            session_help: _,
        } = self;
        [
            global,
            user,
            digits,
            minus,
            minibuffer,
            question,
            completing,
            draft,
            isearch,
            jump,
            replace,
            rigid,
            popup,
            popup_moved,
            hover,
            user_session,
            lsp,
        ]
        .into_iter()
        .chain(modes.all())
    }

    /// The prefix argument's keymaps, the buffer's minor mode keys, the layer of `C-x TAB` and
    /// the minor mode keys again, as [`Keymaps::active`] takes them: in this order, so that each
    /// set it makes active is one slice, the layer of `C-x TAB` first among the minor modes
    /// while it is open (ADR 0115). The minor mode keys are the user's session keys in a
    /// session's terminal and the `lsp` layer's elsewhere: a session's terminal is never a
    /// buffer a server follows, so the two share a place.
    pub(super) fn reading(&self, editor: &Editor) -> [&Keymap; 5] {
        let minor = if editor.current().1.hosts_session() {
            &self.user_session
        } else {
            &self.lsp
        };
        // One place for a layer a key opens for a while: the popup's, a description's or
        // `C-x TAB`'s.
        let transient = match editor.completion_popup() {
            Some(popup) => &self.popup[usize::from(popup.moved())],
            None if editor.hover().is_some() => &self.hover,
            None => &self.rigid,
        };
        [&self.prefix[1], &self.prefix[0], minor, transient, minor]
    }

    /// The layers `key` resolves through, and the key to feed: [`Keymaps::active`]'s, but
    /// `C-g` is never the user's, Shift or not. With keys `pending` it cancels whatever is
    /// bound, and on its own it runs what the defaults bind (ADR 0108, ADR 0110). A key that
    /// runs and `describe-key` both resolve through this (ADR 0150).
    pub(super) fn for_key<'a>(
        &'a self,
        reading: &'a [&'a Keymap; 5],
        prefix: Option<Prefix>,
        pending: bool,
        editor: &Editor,
        key: KeyPress,
    ) -> (Layers<'a>, KeyPress) {
        let mut layers = self.active(reading, prefix, editor);
        let cancels = is_cancel(key);
        if cancels {
            layers.user = None;
        }
        (layers, if cancels && pending { CANCEL } else { key })
    }

    /// The keymaps active now. A question's keys take the global keymap's place, so that
    /// no other key reaches a command, and a prompt's go above it (ADR 0072), a completion's
    /// with them (ADR 0076), and a draft's in their place (ADR 0122). So do isearch's, and a
    /// key they do not bind ends the search first (ADR 0082). The user's keys go just above
    /// the global ones, so neither a question nor a search under way sees them (ADR 0108), and
    /// the user's session keys above a session's terminal's (ADR 0132), or the `lsp` layer in a
    /// buffer a server follows (ADR 0138). `reading` holds them as [`Keymaps::reading`] gives
    /// them.
    pub(super) fn active<'a>(
        &'a self,
        reading: &'a [&'a Keymap; 5],
        prefix: Option<Prefix>,
        editor: &Editor,
    ) -> Layers<'a> {
        if editor
            .prompt()
            .is_some_and(|prompt| prompt.kind().is_question())
        {
            return Layers::global(&self.question);
        }
        if editor.jump().is_some() {
            return Layers::global(&self.jump);
        }
        if editor.isearch().is_some() {
            return Layers::global(&self.isearch);
        }
        if editor.replace().is_some() {
            return Layers::global(&self.replace);
        }
        let buffer = editor.current().1;
        // The user's session keys, or the `lsp` layer's, come after the prefix argument's,
        // which bind no `C-c` or `C-M-` key.
        let minor_on = if buffer.hosts_session() {
            !self.user_session.is_empty()
        } else {
            lsp_on(buffer)
        };
        let minor_len = usize::from(minor_on && editor.prompt().is_none());
        let minor: &[&Keymap] = match prefix {
            Some(prefix) if prefix.reads_minus() => &reading[..2 + minor_len],
            Some(prefix) if prefix.reads_digits() => &reading[1..2 + minor_len],
            _ if editor.indent_rigidly_layer()
                || editor.completion_popup().is_some()
                || editor.hover().is_some() =>
            {
                &reading[3..4 + minor_len]
            }
            _ => &reading[2..2 + minor_len],
        };
        let major = if let Some(prompt) = editor.prompt() {
            match prompt.kind() {
                // `session-search`'s own keys over completion's (ADR 0145).
                Kind::Complete { .. } if editor.searches_conversations() => {
                    Some(self.modes.searching())
                }
                Kind::Complete { .. } => Some(&self.completing),
                // `RET` breaks a line in a draft, which `C-c C-c` keeps (ADR 0122).
                Kind::Draft => Some(&self.draft),
                _ => Some(&self.minibuffer),
            }
        } else {
            self.modes.keys(buffer)
        };
        Layers {
            minor,
            major,
            user: self.user(),
            ..Layers::global(&self.global)
        }
    }

    /// A native session's permission card's keys, some of which its composer takes (ADR 0180).
    pub(super) fn card(&self) -> &Keymap {
        self.modes.card()
    }

    /// The keymap of a terminal's buffer: session mode's over the terminal's in one that hosts
    /// a session (ADR 0132), else the terminal's.
    pub(super) fn terminal_keys(&self, buffer: &Buffer) -> &Keymap {
        self.modes.terminal_keys(buffer)
    }

    /// The user's keymap as a layer, none when it is empty, so that a key costs nothing more
    /// without user keys.
    pub(super) fn user(&self) -> Option<&Keymap> {
        Some(&self.user).filter(|user| !user.is_empty())
    }
}

/// Whether the `lsp` layer is on in `buffer`: a server follows it, open with it, as the
/// modeline's `lsp` says, and has not stopped for the session; one that waits to start again
/// after a crash keeps it (ADR 0138).
fn lsp_on(buffer: &Buffer) -> bool {
    buffer
        .server()
        .is_some_and(|mark| mark != ServerMark::Stopped)
}
