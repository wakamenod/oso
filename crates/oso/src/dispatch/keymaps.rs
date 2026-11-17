//! The keymaps a dispatcher holds, and which of them are active at a moment: a prefix
//! argument's, the layer `C-x TAB` opens, a prompt's, a completion's, a review comment's
//! draft's or a question's, a jump's, isearch's, the `lsp` minor mode's in a buffer a language
//! server follows, a mode's, a session's terminal's with the user's `[keys.session]` over it,
//! the user's and the global one (ADR 0034, ADR 0072, ADR 0076, ADR 0082, ADR 0108, ADR 0115,
//! ADR 0116, ADR 0121, ADR 0122, ADR 0132, ADR 0138).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.
//!
//! The default keymaps come from `bindings`. The user's keymap from the configuration goes in
//! here too, once no prefix key waits for the one before.

use oso_core::buffer::{Buffer, Mode, ServerMark};
use oso_core::command::{CommandId, Prefix, Registry};
use oso_core::editor::Editor;
use oso_core::keymap::{Keymap, Layers};
use oso_core::minibuffer::Kind;

use crate::bindings;
use crate::terminal::route;

use super::Dispatcher;
use super::session_help::SessionHelp;

/// Every keymap a dispatcher holds.
pub(super) struct Keymaps {
    pub(super) global: Keymap,
    /// The user's keys from the configuration, just above the global ones (ADR 0108). Empty
    /// without any, and then not a layer.
    pub(super) user: Keymap,
    /// Active while the prefix argument reads digits, and a minus sign. They come
    /// first among the minor modes, as what is being typed is for them.
    pub(super) prefix: [Keymap; 2],
    /// The keys of the list of emergency copies, and of a buffer showing one (ADR 0064).
    pub(super) copy: [Keymap; 2],
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
    /// Markdown's own keys, above the global ones in a Markdown buffer.
    pub(super) markdown: Keymap,
    /// A `*Brief*` buffer's own keys, and a `*Worktrees*` buffer's, above the global ones
    /// (ADR 0121).
    pub(super) worktrees: [Keymap; 2],
    /// `*Spaces*`'s own keys, above the global ones in it (ADR 0121).
    pub(super) sessions: Keymap,
    /// A review buffer's own keys, above the global ones (ADR 0122).
    pub(super) review: Keymap,
    /// A directory listing's own keys, above the global ones (ADR 0134).
    pub(super) dired: Keymap,
    /// `*lsp-rename*`'s own keys, above the global ones (ADR 0138, step 14).
    pub(super) rename_preview: Keymap,
    /// A git status's own keys, above the global ones (ADR 0127).
    pub(super) git: Keymap,
    /// A commit's message's own keys, above the global ones (ADR 0127, step 1b).
    pub(super) git_commit: Keymap,
    /// The log of git's runs' own keys, above the global ones.
    pub(super) git_log: Keymap,
    /// `session-search`'s prompt's, then `*conversation: NAME*`'s (ADR 0145).
    pub(super) conversations: [Keymap; 2],
    /// The list of what discards kept's own keys (ADR 0127, step 1c).
    pub(super) git_discarded: Keymap,
    /// A graph's own keys, above the global ones (ADR 0127, step 4).
    pub(super) git_graph: Keymap,
    pub(super) git_pr: Keymap,
    pub(super) git_pr_message: Keymap,
    pub(super) review_message: Keymap,
    /// A terminal's own keys, above the global ones in a terminal's buffer (ADR 0120).
    pub(super) terminal: Keymap,
    /// A terminal's own keys and session mode's, in a terminal's buffer that hosts a session
    /// (ADR 0132).
    pub(super) session: Keymap,
    /// The user's keys from `[keys.session]`, above `session` in a session's terminal
    /// (ADR 0132, step 2). Empty without any, and then not a layer.
    pub(super) user_session: Keymap,
    /// The `lsp` minor mode's keys, above the major mode's in a buffer a language server
    /// follows (ADR 0138, Keys in buffers a server follows).
    pub(super) lsp: Keymap,
    /// What `session-help` says: session mode's keys, from its keymap (ADR 0132).
    pub(super) session_help: SessionHelp,
}

impl Keymaps {
    /// The default keymaps over the commands in `registry`, with `global` as the global keymap
    /// and no user keys.
    pub(super) fn new(registry: &Registry, global: Keymap) -> Self {
        let (digits, minus) = bindings::prefix_keymaps(registry);
        let (list, copy) = bindings::recovery_keymaps(registry);
        let [minibuffer, question, completing, draft] = bindings::prompt_keymaps(registry);
        let session = bindings::session_keymap(registry);
        let (popup, popup_commands) = bindings::popup_keymaps(registry);
        Self {
            global,
            user: Keymap::new(),
            prefix: [digits, minus],
            copy: [list, copy],
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
            markdown: bindings::markdown_keymap(registry),
            worktrees: bindings::worktree_keymaps(registry),
            sessions: bindings::sessions_keymap(registry),
            review: bindings::review_keymap(registry),
            dired: bindings::dired_keymap(registry),
            rename_preview: bindings::rename_preview_keymap(registry),
            git: bindings::git_status_keymap(registry),
            git_commit: bindings::git_commit_keymap(registry),
            git_log: bindings::git_log_keymap(registry),
            conversations: bindings::conversation_keymaps(registry),
            git_discarded: bindings::git_discarded_keymap(registry),
            git_graph: bindings::git_graph_keymap(registry),
            git_pr: bindings::git_pr_keymap(registry),
            git_pr_message: bindings::git_pr_message_keymap(registry),
            review_message: bindings::review_message_keymap(registry),
            terminal: bindings::terminal_keymap(registry),
            session_help: SessionHelp::new(&session, registry),
            session,
            user_session: Keymap::new(),
            lsp: bindings::lsp_keymap(registry),
        }
    }

    /// Every keymap, active or not. Each field is named here, so that a keymap added to the
    /// struct cannot be left out of what `M-x` judges commands by.
    #[expect(
        clippy::too_many_lines,
        reason = "one line for each keymap, named twice so that none is left out"
    )]
    pub(super) fn all(&self) -> [&Keymap; 38] {
        let Self {
            global,
            user,
            prefix: [digits, minus],
            copy: [list, copy],
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
            markdown,
            worktrees: [brief, worktrees],
            sessions,
            review,
            dired,
            rename_preview,
            git,
            git_commit,
            git_log,
            conversations: [searching, conversation],
            git_discarded,
            git_graph,
            git_pr,
            git_pr_message,
            review_message,
            terminal,
            session,
            user_session,
            lsp,
            session_help: _,
        } = self;
        [
            global,
            user,
            digits,
            minus,
            list,
            copy,
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
            markdown,
            brief,
            worktrees,
            sessions,
            review,
            dired,
            rename_preview,
            git,
            git_commit,
            git_log,
            searching,
            conversation,
            git_discarded,
            git_graph,
            git_pr,
            git_pr_message,
            review_message,
            terminal,
            session,
            user_session,
            lsp,
        ]
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
                    Some(&self.conversations[0])
                }
                Kind::Complete { .. } => Some(&self.completing),
                // `RET` breaks a line in a draft, which `C-c C-c` keeps (ADR 0122).
                Kind::Draft => Some(&self.draft),
                _ => Some(&self.minibuffer),
            }
        } else {
            match buffer.mode() {
                Mode::Fundamental
                | Mode::Rust
                | Mode::Toml
                | Mode::Go
                | Mode::TypeScript
                | Mode::Tsx
                | Mode::JavaScript
                | Mode::Latex
                | Mode::Swift => None,
                Mode::Review => Some(&self.review),
                Mode::Directory => Some(&self.dired),
                Mode::RenamePreview => Some(&self.rename_preview),
                Mode::GitStatus => Some(&self.git),
                Mode::GitCommit => Some(&self.git_commit),
                Mode::GitLog => Some(&self.git_log),
                Mode::Conversation => Some(&self.conversations[1]),
                Mode::GitDiscarded => Some(&self.git_discarded),
                Mode::GitGraph => Some(&self.git_graph),
                Mode::GitPr | Mode::GitPrList => Some(&self.git_pr),
                Mode::GitPrMessage => Some(&self.git_pr_message),
                Mode::ReviewMessage => Some(&self.review_message),
                Mode::Markdown => Some(&self.markdown),
                Mode::Brief => Some(&self.worktrees[0]),
                Mode::Worktrees => Some(&self.worktrees[1]),
                Mode::Sessions => Some(&self.sessions),
                Mode::CopyList => Some(&self.copy[0]),
                Mode::CopyView => Some(&self.copy[1]),
                Mode::Terminal => Some(self.terminal_keys(buffer)),
            }
        };
        Layers {
            minor,
            major,
            user: self.user(),
            ..Layers::global(&self.global)
        }
    }

    /// The keymap of a terminal's buffer: session mode's over the terminal's in one that hosts
    /// a session (ADR 0132), else the terminal's.
    pub(super) fn terminal_keys(&self, buffer: &Buffer) -> &Keymap {
        if buffer.hosts_session() {
            &self.session
        } else {
            &self.terminal
        }
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

/// The user's keymap from the configuration, which waits for a pending prefix key to go in
/// (ADR 0108).
impl Dispatcher {
    /// Puts in the user's keymap that waited for a prefix key, once none is pending. The one
    /// it replaces waits in turn, for [`Dispatcher::take_replaced_keymap`].
    pub(super) fn take_waiting_user(&mut self) {
        if !self.resolver.pending().is_empty() {
            return;
        }
        self.menus.take_waiting();
        if let Some(user) = self.user_waiting.take() {
            let old = std::mem::replace(&mut self.keymaps.user, user);
            self.replaced.push(old);
        }
        if let Some((keys, help)) = self.session_waiting.take() {
            let old = self.put_session_keys(keys, help);
            self.replaced.push(old);
        }
    }

    /// Puts the user's keys from the configuration just above the global keymap, in place of
    /// those before; an empty keymap takes them away (ADR 0108). While a prefix key is
    /// pending, they wait for it to resolve, so that its next key reads the keymaps it
    /// started in.
    ///
    /// Returns the keymap this replaces now, the one in force or one that was waiting, for the
    /// caller to drop elsewhere: a large one takes milliseconds to drop. One that a waiting
    /// keymap replaces later is kept for [`Dispatcher::take_replaced_keymap`].
    pub fn set_user_keymap(&mut self, user: Keymap) -> Option<Keymap> {
        if self.resolver.pending().is_empty() {
            if let Some(stale) = self.user_waiting.take() {
                self.replaced.push(stale);
            }
            Some(std::mem::replace(&mut self.keymaps.user, user))
        } else {
            self.user_waiting.replace(user)
        }
    }

    /// Puts the user's `[keys.session]` above a session's terminal's keys, in place of those
    /// before, with `help`, what `C-c ?` says of them, or `None` for the default line
    /// (ADR 0132). They wait for a pending prefix key as [`Dispatcher::set_user_keymap`]'s do,
    /// and the keymap this replaces now is returned in the same way.
    ///
    /// Keys that would take `C-c C-c` or `C-c C-k` from the terminal are not put in, as the
    /// configuration refuses them: `C-c` would no longer reach the program, or char mode
    /// would not switch. The default line is said then.
    pub fn set_session_keys(&mut self, keys: Keymap, help: Option<String>) -> Option<Keymap> {
        let takes = |key| keys.lookup(&[route::C_C, key]).is_some();
        let (keys, help) = if takes(route::C_C) || takes(route::C_K) {
            log::warn!("session keys that bind C-c C-c or C-c C-k were not put in");
            (Keymap::new(), None)
        } else {
            (keys, help)
        };
        if self.resolver.pending().is_empty() {
            if let Some((stale, _)) = self.session_waiting.take() {
                self.replaced.push(stale);
            }
            Some(self.put_session_keys(keys, help))
        } else {
            self.session_waiting
                .replace((keys, help))
                .map(|(stale, _)| stale)
        }
    }

    /// Puts in the user's session keys and their line, and returns the keys they replace.
    fn put_session_keys(&mut self, keys: Keymap, help: Option<String>) -> Keymap {
        self.keymaps.session_help.set(help);
        std::mem::replace(&mut self.keymaps.user_session, keys)
    }

    /// A user's keymap that a waiting one replaced, once its prefix key resolved, for the
    /// caller to drop elsewhere. Call it until it gives `None`.
    pub fn take_replaced_keymap(&mut self) -> Option<Keymap> {
        self.replaced.pop()
    }
}

/// What the dispatcher is reading now: a prefix key, a prefix argument, or text as typed.
impl Dispatcher {
    /// The keys of the pending prefix, empty if none is pending.
    pub fn pending(&self) -> &[oso_core::key::KeyPress] {
        self.resolver.pending()
    }

    /// Whether keys are in progress at all: a prefix key, or a prefix argument, even one that
    /// takes no more digits, which the next command would take.
    pub fn keys_in_progress(&self) -> bool {
        !self.resolver.pending().is_empty() || self.prefix.is_some()
    }

    /// Whether the next key is read as a key rather than typed: a prefix key is
    /// pending, a prefix argument reads digits or a minus, or a menu is open. The input method
    /// stays off then, as a letter or a digit would start a composition (ADR 0069, ADR 0117).
    pub fn reads_keys(&self) -> bool {
        self.menu_open()
            || !self.resolver.pending().is_empty()
            || self
                .prefix
                .is_some_and(|prefix| prefix.reads_digits() || prefix.reads_minus())
    }

    /// Whether text would go in as typed now: no prefix key or prefix argument waits, no
    /// question or menu is open, and what is active takes text. A commit the window layer makes
    /// unasked waits otherwise, so that it neither takes a prefix argument nor is lost
    /// (ADR 0077).
    pub fn types_text(&self, editor: &Editor) -> bool {
        let question = editor
            .prompt()
            .is_some_and(|prompt| prompt.kind().is_question());
        if question
            || self.prefix.is_some()
            || !self.resolver.pending().is_empty()
            || self.menu_open()
        {
            return false;
        }
        let reading = self.keymaps.reading(editor);
        let active = self.keymaps.active(&reading, self.prefix, editor);
        active.text_command().is_some()
    }
}
