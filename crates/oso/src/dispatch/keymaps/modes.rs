//! The major modes' own keymaps, each above the global one in a buffer of its mode, and which
//! of them a buffer's mode reads: Markdown's, the emergency copies', the worktree and session
//! lists', a review's, a directory listing's, `*lsp-rename*`'s, git's, a terminal's and a
//! session's, and a native session's (ADR 0064, ADR 0120, ADR 0121, ADR 0122, ADR 0127,
//! ADR 0132, ADR 0134, ADR 0138, ADR 0145, ADR 0165).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.
//!
//! A new mode's keymap goes here: a field, its line in [`Modes::new`], in [`Modes::all`] and in
//! [`Modes::keys`].

use oso_core::buffer::{Buffer, Mode};
use oso_core::command::Registry;
use oso_core::keymap::Keymap;

use crate::bindings;

/// Every major mode's keymap.
pub(super) struct Modes {
    /// The keys of the list of emergency copies, and of a buffer showing one (ADR 0064).
    copy: [Keymap; 2],
    /// Markdown's own keys, above the global ones in a Markdown buffer.
    markdown: Keymap,
    /// A `*Brief*` buffer's own keys, and a `*Worktrees*` buffer's, above the global ones
    /// (ADR 0121).
    worktrees: [Keymap; 2],
    /// `*Spaces*`'s own keys, above the global ones in it (ADR 0121).
    sessions: Keymap,
    /// A review buffer's own keys, above the global ones (ADR 0122).
    review: Keymap,
    /// A directory listing's own keys, above the global ones (ADR 0134).
    dired: Keymap,
    /// `*lsp-rename*`'s own keys, above the global ones (ADR 0138, step 14).
    rename_preview: Keymap,
    /// A git status's own keys, above the global ones (ADR 0127).
    git: Keymap,
    /// A commit's message's own keys, above the global ones (ADR 0127, step 1b).
    git_commit: Keymap,
    /// The log of git's runs' own keys, above the global ones.
    git_log: Keymap,
    /// `session-search`'s prompt's, then `*conversation: NAME*`'s (ADR 0145). The prompt's is
    /// no mode's: [`Modes::searching`] gives it to the prompt's layer.
    conversations: [Keymap; 2],
    /// The list of what discards kept's own keys (ADR 0127, step 1c).
    git_discarded: Keymap,
    /// A graph's own keys, above the global ones (ADR 0127, step 4).
    git_graph: Keymap,
    git_pr: Keymap,
    git_issue: Keymap,
    git_pr_message: Keymap,
    review_message: Keymap,
    review_files: Keymap,
    review_commits: Keymap,
    /// A terminal's own keys, above the global ones in a terminal's buffer (ADR 0120).
    terminal: Keymap,
    /// A terminal's own keys and session mode's, in a terminal's buffer that hosts a session
    /// (ADR 0132).
    session: Keymap,
    /// A native session's buffer's own keys and session mode's (ADR 0165).
    native: Keymap,
    /// A native session's composer's keys, its permission card's (ADR 0165, step 2) and its
    /// plan's edit's (part 3d).
    composer: Keymap,
    card: Keymap,
    plan: Keymap,
}

impl Modes {
    /// The default keymaps of the major modes over the commands in `registry`.
    pub(super) fn new(registry: &Registry) -> Self {
        let (list, copy) = bindings::recovery_keymaps(registry);
        Self {
            copy: [list, copy],
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
            git_issue: bindings::git_issue_keymap(registry),
            git_pr_message: bindings::git_pr_message_keymap(registry),
            review_message: bindings::review_message_keymap(registry),
            review_files: bindings::review_files_keymap(registry),
            review_commits: bindings::review_commits_keymap(registry),
            terminal: bindings::terminal_keymap(registry),
            session: bindings::session_keymap(registry),
            native: bindings::native_session_keymap(registry),
            composer: bindings::composer_keymap(registry),
            card: bindings::card_keymap(registry),
            plan: bindings::plan_keymap(registry),
        }
    }

    /// Every major mode's keymap. Each field is named here, so that a keymap added to the
    /// struct cannot be left out of what `M-x` judges commands by.
    pub(super) fn all(&self) -> [&Keymap; 28] {
        let Self {
            copy: [list, copy],
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
            git_issue,
            git_pr_message,
            review_message,
            review_files,
            review_commits,
            terminal,
            session,
            native,
            composer,
            card,
            plan,
        } = self;
        [
            list,
            copy,
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
            git_issue,
            git_pr_message,
            review_message,
            review_files,
            review_commits,
            terminal,
            session,
            native,
            composer,
            card,
            plan,
        ]
    }

    /// The keys of `buffer`'s major mode, above the global ones, if it has its own.
    pub(super) fn keys<'a>(&'a self, buffer: &Buffer) -> Option<&'a Keymap> {
        match buffer.mode() {
            Mode::Fundamental
            | Mode::Rust
            | Mode::Toml
            | Mode::Go
            | Mode::TypeScript
            | Mode::Tsx
            | Mode::JavaScript
            | Mode::Html
            | Mode::Css
            | Mode::Json
            | Mode::Yaml
            | Mode::Latex
            | Mode::Swift
            | Mode::SessionMove => None,
            Mode::NativeSession => Some(&self.native),
            Mode::Composer => Some(&self.composer),
            Mode::Card => Some(&self.card),
            Mode::Plan => Some(&self.plan),
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
            Mode::GitIssue | Mode::GitIssueList => Some(&self.git_issue),
            Mode::GitPrMessage => Some(&self.git_pr_message),
            Mode::ReviewMessage => Some(&self.review_message),
            Mode::ReviewFiles => Some(&self.review_files),
            Mode::ReviewCommits => Some(&self.review_commits),
            Mode::Markdown => Some(&self.markdown),
            Mode::Brief => Some(&self.worktrees[0]),
            Mode::Worktrees => Some(&self.worktrees[1]),
            Mode::Sessions => Some(&self.sessions),
            Mode::CopyList => Some(&self.copy[0]),
            Mode::CopyView => Some(&self.copy[1]),
            Mode::Terminal => Some(self.terminal_keys(buffer)),
        }
    }

    /// A hosted session's terminal's keys, which `session-help` reads (ADR 0132).
    pub(super) fn session(&self) -> &Keymap {
        &self.session
    }

    /// A native session's permission card's keys.
    pub(super) fn card(&self) -> &Keymap {
        &self.card
    }

    /// `session-search`'s prompt's keys, over completion's (ADR 0145).
    pub(super) fn searching(&self) -> &Keymap {
        &self.conversations[0]
    }

    /// A native session's composer's keys, whatever buffer is current (ADR 0170).
    pub(super) fn composer(&self) -> &Keymap {
        &self.composer
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
}
