//! A rename on the UI thread (ADR 0138, step 14, as `eglot-rename`): the server checks the place,
//! the new name is read in a prompt that names the old one, the server's edit opens and is
//! judged through `applying`, and `*lsp-rename*` shows it before `y` applies it (`shown`).
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server or a
//! file.
//!
//! - Only the person renames (Q10): the commands are `Person`, and `*lsp-rename*` is withheld
//!   from agents.
//! - The place is the server's range if it checks one, else the identifier at point (Q4). The
//!   prompt starts empty, with the old name on `M-n` (Q5).
//! - The rename waits [`RENAME_WAIT`](oso_lsp::limits::RENAME_WAIT); `C-g` stops it, and the
//!   server is told (Q7), as it is when a newer `lsp-rename` drops it.
//! - The edit's files open as 13b opens them, unshown, judged before and once loaded, with no
//!   question: the preview is the question, built from those buffers' texts with the ranges `y`
//!   applies (the author's choice, 2026-11-17). `n`, `q` or `C-g` kills what was opened.
//! - One rename at a time. An answer, a failure or a preview counts only if it is the rename's
//!   own: its server, its buffer and its ask's serial.
//! - Names are shown with their controls and bidi and invisible characters written out, and
//!   never logged.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oso_core::buffer::BufferId;
use oso_core::editor::{AskAt, BufferEdit};
use oso_core::minibuffer::PromptId;
use oso_core::sum_tree::Bias;
use oso_lsp::server::{AskContext, AskFailure, AskKind, Prepared, ServerEdit};

use super::super::App;
use super::applying::EditFrom;
use super::asks::{Asker, Who};

mod names;
mod preview;
mod shown;

use names::{NEW_NAME_MAX, identifier_at};
use preview::shown;

/// The rename under way: its server, by its root and entry, the buffer asked about, and how far
/// it has got.
#[derive(Debug)]
pub(super) struct Renaming {
    server: (PathBuf, String),
    buffer: BufferId,
    stage: Stage,
}

/// How far a rename has got.
#[derive(Debug)]
enum Stage {
    /// The server checks the place, ask `serial`.
    Checking { serial: u64 },
    /// The prompt reads the new name of `old`, asked at `at`.
    Naming {
        at: AskAt,
        old: String,
        prompt: PromptId,
    },
    /// The server renames `old` to `new`, ask `serial`.
    Asked {
        serial: u64,
        old: String,
        new: String,
    },
    /// Its edit's files open and are judged (`applying`).
    Opening { old: String, new: String },
    /// Its preview is built, generation `generation`, from the buffers it opened, `opened`
    /// among them.
    Reading {
        generation: u64,
        old: String,
        new: String,
        opened: Vec<BufferId>,
    },
    /// Its preview shows in `preview`, from `back`: `parts`, which `y` applies.
    Shown {
        old: String,
        new: String,
        parts: PlacedEdits,
        opened: Vec<BufferId>,
        preview: BufferId,
        back: BufferId,
    },
}

/// A rename's edits placed in the buffers they change, which its preview shows and `y` applies.
/// `Debug` gives counts only: an edit's text may quote what the server read.
pub(crate) struct PlacedEdits(pub(in crate::app) Vec<BufferEdit>);

impl std::fmt::Debug for PlacedEdits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let edits: usize = self.0.iter().map(|part| part.edits.len()).sum();
        write!(f, "PlacedEdits({} buffers, {edits} edits)", self.0.len())
    }
}

/// The generation of the newest preview, which stops the build of an older one.
pub(super) type Generation = Arc<AtomicU64>;

impl App {
    /// `lsp-rename`: asks the server following `at`'s buffer to check the place, or says why
    /// not. A rename under way goes.
    pub(super) fn lsp_rename(&mut self, at: AskAt) {
        // Never the agent's: the commands are `Person`.
        if at.agent {
            return;
        }
        self.drop_rename(None);
        let Some(index) = self.server_to_ask(at.buffer) else {
            return;
        };
        let running = &self.servers.running[index];
        let server = (running.root.clone(), running.entry.name.clone());
        self.send_ask(
            index,
            (at, AskKind::PrepareRename, AskContext::None),
            Who::Person,
        );
        let serial = self.servers.serial;
        self.servers.rename = Some(Renaming {
            server,
            buffer: at.buffer,
            stage: Stage::Checking { serial },
        });
    }

    /// Whether `taken`, server `index`'s answer, is the rename's own ask of `serial`.
    fn rename_owns(&self, index: usize, taken: Option<&(BufferId, Asker)>, serial: u64) -> bool {
        let Some(renaming) = self.servers.rename.as_ref() else {
            return false;
        };
        self.servers.find(&renaming.server.0, &renaming.server.1) == Some(index)
            && taken
                .is_some_and(|(asked, asker)| *asked == renaming.buffer && asker.serial() == serial)
    }

    /// The server checked the place for who asked, `taken`: the prompt for the new name opens,
    /// if nothing moved since.
    pub(super) fn rename_checked(
        &mut self,
        index: usize,
        taken: Option<(BufferId, Asker)>,
        prepared: Prepared,
    ) {
        let serial = match self.servers.rename.as_ref().map(|renaming| &renaming.stage) {
            Some(Stage::Checking { serial }) => *serial,
            _ => return,
        };
        if !self.rename_owns(index, taken.as_ref(), serial) {
            return;
        }
        let Some((asked, asker)) = taken.filter(|(asked, asker)| self.still_there(*asked, asker))
        else {
            return self.drop_rename(None);
        };
        let name = self.servers.running[index].entry.name.clone();
        let at = asker.at(asked);
        let old = self.editor.buffer(asked).and_then(|buffer| {
            let text = buffer.text();
            match prepared {
                Prepared::At {
                    placeholder: Some(placeholder),
                    ..
                } => Some(placeholder.into_string()),
                Prepared::At { start, end, .. } => (start < end && end - start <= NEW_NAME_MAX)
                    .then(|| text.slice(start..end).to_string()),
                Prepared::Unasked => {
                    // Only the text around the cursor: a name is short.
                    let reach = NEW_NAME_MAX * 4;
                    let lo = text.clip_offset(at.offset.saturating_sub(reach), Bias::Left);
                    let hi = text.clip_offset((at.offset + reach).min(text.len()), Bias::Right);
                    let around = text.slice(lo..hi).to_string();
                    identifier_at(&around, at.offset.saturating_sub(lo))
                }
                Prepared::Nothing => None,
            }
        });
        let Some(old) = old.filter(|old| !old.is_empty()) else {
            self.drop_rename(None);
            self.editor
                .message(format!("{name}: nothing to rename here"));
            return self.request_redraw();
        };
        let label = format!("Rename \u{2018}{}\u{2019} to: ", shown(&old));
        let Some(prompt) = self
            .dispatcher
            .read_new_name(&mut self.editor, &label, &old)
        else {
            return self.drop_rename(None);
        };
        if let Some(renaming) = self.servers.rename.as_mut() {
            renaming.stage = Stage::Naming { at, old, prompt };
        }
        self.request_redraw();
    }

    /// `lsp-rename-to`: asks the server to rename what it checked to `name`, if the buffer is
    /// as it was then.
    pub(super) fn rename_to(&mut self, name: &str) {
        let Some(Renaming {
            server,
            buffer,
            stage: Stage::Naming { at, old, .. },
        }) = self.servers.rename.take()
        else {
            return;
        };
        let refused = if name.is_empty() {
            Some("no new name")
        } else if name == old {
            Some("the new name is the old one")
        } else if name.len() > NEW_NAME_MAX || name.chars().any(char::is_control) {
            Some("the new name is too long or holds a control character")
        } else if self.editor.buffer(at.buffer).map(|shown| shown.revision()) != Some(at.revision) {
            Some("the buffer changed since the place was checked")
        } else {
            None
        };
        let index = self.servers.find(&server.0, &server.1);
        let (Some(index), None) = (index, refused) else {
            let why = refused.unwrap_or("its server ended");
            self.editor
                .message(format!("{}: not renamed: {why}", server.1));
            return self.request_redraw();
        };
        let context = (at, AskKind::Rename, AskContext::None);
        self.send_ask_named(index, context, Who::Person, Some(name.into()));
        // The old name is the buffer's or the server's text: shown, not logged.
        self.editor.message_unlogged(format!(
            "{} renames \u{2018}{}\u{2019}… (C-g stops)",
            server.1,
            shown(&old)
        ));
        let serial = self.servers.serial;
        self.servers.rename = Some(Renaming {
            server,
            buffer,
            stage: Stage::Asked {
                serial,
                old,
                new: name.to_owned(),
            },
        });
        self.request_redraw();
    }

    /// The server's rename came for who asked, `taken`: its files open and are judged, then its
    /// preview is built (`shown`).
    pub(super) fn renamed(
        &mut self,
        index: usize,
        taken: Option<(BufferId, Asker)>,
        edit: ServerEdit,
    ) {
        let serial = match self.servers.rename.as_ref().map(|renaming| &renaming.stage) {
            Some(Stage::Asked { serial, .. }) => *serial,
            _ => return self.freer.free(edit),
        };
        if !self.rename_owns(index, taken.as_ref(), serial) {
            return self.freer.free(edit);
        }
        let Some(Renaming {
            server,
            buffer,
            stage: Stage::Asked { old, new, .. },
        }) = self.servers.rename.take()
        else {
            return self.freer.free(edit);
        };
        let unchanged = taken.is_some_and(|(asked, asker)| {
            self.editor.buffer(asked).map(|shown| shown.revision())
                == Some(asker.at(asked).revision)
        });
        let refused = if !unchanged {
            Some("the buffer changed while the server renamed")
        } else if edit.open.is_empty() && edit.unsent.is_empty() {
            Some("nothing to rename")
        } else {
            None
        };
        if let Some(why) = refused {
            self.freer.free(edit);
            self.editor
                .message(format!("{}: not renamed: {why}", server.1));
            return self.request_redraw();
        }
        let title = format!(
            "rename \u{2018}{}\u{2019} to \u{2018}{}\u{2019}",
            shown(&old),
            shown(&new)
        );
        self.servers.rename = Some(Renaming {
            server: server.clone(),
            buffer,
            stage: Stage::Opening { old, new },
        });
        // Opens what no buffer holds, judges it all, and hands the placed edits to
        // `rename_placed`, or says why not.
        self.apply_edit_of(&server, &title, edit, EditFrom::Rename);
        self.request_redraw();
    }

    /// `lsp-rename-cancel`, and `C-g` anywhere while a rename waits: it stops, and the server is
    /// told; the preview goes only if it is the window's.
    pub(super) fn cancel_rename(&mut self) {
        let Some(renaming) = self.servers.rename.as_ref() else {
            return;
        };
        match &renaming.stage {
            Stage::Checking { .. }
            | Stage::Asked { .. }
            | Stage::Opening { .. }
            | Stage::Reading { .. } => self.drop_rename(Some("stopped")),
            Stage::Shown { preview, .. } if self.editor.tiled().0.buffer() == *preview => {
                self.drop_rename(Some("not applied"));
            }
            // A `C-g` elsewhere leaves the preview; the prompt's own `C-g` closed it.
            Stage::Shown { .. } | Stage::Naming { .. } => {}
        }
        self.request_redraw();
    }

    /// After each sync and each answer: a rename whose prompt closed with no name, whose
    /// preview was killed, or whose files were refused, goes; and the dispatcher hears whether
    /// one waits, for `C-g`.
    pub(in crate::app) fn step_rename(&mut self) {
        let gone = match self.servers.rename.as_ref().map(|renaming| &renaming.stage) {
            Some(Stage::Naming { prompt, .. }) => !self
                .editor
                .prompts()
                .iter()
                .any(|open| open.id() == *prompt),
            Some(Stage::Shown { preview, .. }) => self.editor.buffer(*preview).is_none(),
            // `applying` said why, and killed what it opened.
            Some(Stage::Opening { .. }) => !self.rename_opening(),
            _ => false,
        };
        if gone {
            self.drop_rename(None);
        }
        let waits = matches!(
            self.servers.rename.as_ref().map(|renaming| &renaming.stage),
            Some(
                Stage::Checking { .. }
                    | Stage::Asked { .. }
                    | Stage::Opening { .. }
                    | Stage::Reading { .. }
            )
        );
        self.dispatcher.set_rename_waits(waits);
    }

    /// The server's check or rename failed: if it is the rename's own ask, the rename ends, and
    /// the echo area says why.
    pub(super) fn rename_failed(
        &mut self,
        index: usize,
        taken: Option<(BufferId, Asker)>,
        kind: AskKind,
        why: &AskFailure,
    ) {
        let serial = match self.servers.rename.as_ref().map(|renaming| &renaming.stage) {
            Some(Stage::Checking { serial }) if kind == AskKind::PrepareRename => *serial,
            Some(Stage::Asked { serial, .. }) if kind == AskKind::Rename => *serial,
            _ => return,
        };
        if !self.rename_owns(index, taken.as_ref(), serial) {
            return;
        }
        self.drop_rename(None);
        let name = self.servers.running[index].entry.name.clone();
        // The server's text may quote what it read: shown, not logged.
        self.editor
            .message_unlogged(super::places::failed(&name, kind, why));
        self.request_redraw();
    }

    /// Drops the rename under way, saying `said` of it if given: an ask that waits is cancelled
    /// and the server told, what it opened is killed, and its preview goes.
    fn drop_rename(&mut self, said: Option<&str>) {
        let Some(renaming) = self.servers.rename.take() else {
            return;
        };
        let name = renaming.server.1.clone();
        match renaming.stage {
            Stage::Checking { .. } | Stage::Asked { .. } => {
                let kind = match renaming.stage {
                    Stage::Checking { .. } => AskKind::PrepareRename,
                    _ => AskKind::Rename,
                };
                let found = self.servers.find(&renaming.server.0, &renaming.server.1);
                if let Some(index) = found {
                    let running = &mut self.servers.running[index];
                    running.server.cancel(renaming.buffer.get(), kind);
                    running.asked.remove(&(renaming.buffer, kind));
                }
            }
            Stage::Opening { .. } => self.drop_rename_opening("stopped"),
            Stage::Reading { opened, .. } => {
                self.servers
                    .preview_generation
                    .fetch_add(1, Ordering::Relaxed);
                self.kill_opened(&opened);
            }
            Stage::Shown {
                parts,
                opened,
                preview,
                back,
                ..
            } => {
                self.close_preview(preview, back);
                self.kill_opened(&opened);
                self.freer.free(parts);
            }
            Stage::Naming { .. } => {}
        }
        if let Some(said) = said {
            self.editor.message(format!("{name}: rename {said}"));
        }
    }
}

#[cfg(test)]
mod tests;
