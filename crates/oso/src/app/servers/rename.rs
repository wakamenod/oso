//! A rename on the UI thread (ADR 0138, step 14, as `eglot-rename`): the server checks the place,
//! the new name is read in a prompt that names the old one, the server's edits are shown in
//! `*lsp-rename*`, and `y` there applies them through `applying`.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server or a
//! file.
//!
//! - Only the person renames (Q10): the commands are `Person`, and `*lsp-rename*` is withheld
//!   from agents.
//! - The place is the server's range if it checks one, else the identifier at point (Q4). The
//!   prompt starts empty, with the old name on `M-n` (Q5).
//! - The rename waits [`RENAME_WAIT`](oso_lsp::limits::RENAME_WAIT); `C-g` stops it and the
//!   server is told (Q7).
//! - Before anything shows, a file the edit would refuse once applied before it opens refuses
//!   it now (`applying::judge_unsent`), so that the preview never shows a file that may hold
//!   secrets or lies outside the project. The preview stands for 13b's question: `y` applies,
//!   opening the files no buffer holds, unshown and unsaved (Q9); `n`, `q` or `C-g` leaves all
//!   as it was. One rename at a time: a newer `lsp-rename` drops an older one.
//! - Every name shown has its controls and bidi and invisible characters written out.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::AskAt;
use oso_core::minibuffer::PromptId;
use oso_core::sum_tree::Bias;
use oso_lsp::server::{AskContext, AskFailure, AskKind, Prepared, ServerEdit};

use super::super::{App, UserEvent};
use super::applying::EditFrom;
use super::asks::{Asker, Who};

mod preview;
mod snapshot;

use preview::shown;
use snapshot::{NEW_NAME_MAX, identifier_at};

/// The name of the buffer that shows a rename's edits.
const PREVIEW_NAME: &str = "*lsp-rename*";

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
    /// The server checks the place.
    Checking,
    /// The prompt reads the new name of `old`, asked at `at`.
    Naming {
        at: AskAt,
        old: String,
        prompt: PromptId,
    },
    /// The server renames `old` to `new`.
    Asked { old: String, new: String },
    /// Its edits are read for the preview, generation `generation`.
    Reading {
        generation: u64,
        old: String,
        new: String,
        edit: ServerEdit,
    },
    /// The preview shows them in `preview`, from `back`.
    Shown {
        old: String,
        new: String,
        edit: ServerEdit,
        preview: BufferId,
        back: BufferId,
    },
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
        let context = AskContext::None;
        self.send_ask(index, (at, AskKind::PrepareRename, context), Who::Person);
        self.servers.rename = Some(Renaming {
            server,
            buffer: at.buffer,
            stage: Stage::Checking,
        });
    }

    /// The server checked the place for who asked, `taken`: the prompt for the new name opens,
    /// if nothing moved since.
    pub(super) fn rename_checked(
        &mut self,
        index: usize,
        taken: Option<(BufferId, Asker)>,
        prepared: Prepared,
    ) {
        let Some((asked, asker)) = taken.filter(|(asked, asker)| self.still_there(*asked, asker))
        else {
            self.servers.rename = None;
            return;
        };
        if !matches!(
            self.servers.rename.as_ref().map(|renaming| &renaming.stage),
            Some(Stage::Checking)
        ) {
            return;
        }
        let name = self.servers.running[index].entry.name.clone();
        let at = asker.at(asked);
        let old = self.editor.buffer(asked).and_then(|buffer| {
            let text = buffer.text();
            match prepared {
                Prepared::At {
                    start,
                    end,
                    placeholder,
                } => match placeholder {
                    Some(placeholder) => Some(placeholder.into_string()),
                    None => (start < end && end - start <= NEW_NAME_MAX)
                        .then(|| text.slice(start..end).to_string()),
                },
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
            self.servers.rename = None;
            self.editor
                .message(format!("{name}: nothing to rename here"));
            self.request_redraw();
            return;
        };
        let label = format!("Rename \u{2018}{}\u{2019} to: ", shown(&old));
        let Some(prompt) = self
            .dispatcher
            .read_new_name(&mut self.editor, &label, &old)
        else {
            self.servers.rename = None;
            return;
        };
        if let Some(renaming) = self.servers.rename.as_mut() {
            renaming.stage = Stage::Naming { at, old, prompt };
        }
        self.request_redraw();
    }

    /// `lsp-rename-to`: asks the server to rename what it checked to `name`, if the buffer is
    /// as it was then.
    pub(super) fn rename_to(&mut self, name: &str) {
        let Some(renaming) = self.servers.rename.take() else {
            return;
        };
        let Stage::Naming { at, old, .. } = renaming.stage else {
            self.servers.rename = Some(renaming);
            return;
        };
        let server_name = renaming.server.1.clone();
        let refused = if name.is_empty() {
            Some("no new name")
        } else if name == old {
            Some("the new name is the old one")
        } else if name.len() > NEW_NAME_MAX || name.chars().any(char::is_control) {
            Some("the new name is too long or holds a control character")
        } else if self
            .editor
            .buffer(at.buffer)
            .map(|buffer| buffer.revision())
            != Some(at.revision)
        {
            Some("the buffer changed since the place was checked")
        } else {
            None
        };
        if let Some(why) = refused {
            self.editor
                .message(format!("{server_name}: not renamed: {why}"));
            self.request_redraw();
            return;
        }
        let Some(index) = self.servers.find(&renaming.server.0, &renaming.server.1) else {
            self.editor
                .message(format!("{server_name}: not renamed: its server ended"));
            return;
        };
        let context = (at, AskKind::Rename, AskContext::None);
        self.send_ask_named(index, context, Who::Person, Some(name.into()));
        self.editor.message(format!(
            "{server_name} renames \u{2018}{}\u{2019}… (C-g stops)",
            shown(&old)
        ));
        self.servers.rename = Some(Renaming {
            stage: Stage::Asked {
                old,
                new: name.to_owned(),
            },
            ..renaming
        });
        self.request_redraw();
    }

    /// The server's rename came for who asked, `taken`: its files judged, and its preview built
    /// off the UI thread.
    pub(super) fn renamed(
        &mut self,
        index: usize,
        taken: Option<(BufferId, Asker)>,
        edit: ServerEdit,
    ) {
        let asked = taken.filter(|(asked, asker)| {
            self.editor.buffer(*asked).map(|buffer| buffer.revision())
                == Some(asker.at(*asked).revision)
        });
        let Some(renaming) = self.servers.rename.take() else {
            self.freer.free(edit);
            return;
        };
        let Stage::Asked { old, new } = renaming.stage else {
            self.freer.free(edit);
            return;
        };
        let name = renaming.server.1.clone();
        let refused = if asked.is_none() {
            Some("the buffer changed while the server renamed")
        } else if edit.open.is_empty() && edit.unsent.is_empty() {
            Some("nothing to rename")
        } else {
            None
        };
        if let Some(why) = refused {
            self.freer.free(edit);
            self.editor.message(format!("{name}: not renamed: {why}"));
            self.request_redraw();
            return;
        }
        let snapshot = match self.preview_snapshot(index, (&old, &new), &edit) {
            Ok(snapshot) => snapshot,
            Err(why) => {
                self.freer.free(edit);
                self.editor.message(format!("{name}: not renamed: {why}"));
                self.request_redraw();
                return;
            }
        };
        let generation = self
            .servers
            .preview_generation
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        let live = Arc::clone(&self.servers.preview_generation);
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name("oso-lsp-rename".into())
            .spawn(move || {
                let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    preview::build(&snapshot, &|| live.load(Ordering::Relaxed) == generation)
                }));
                let built = built.unwrap_or_else(|_| {
                    log::warn!("the build of a rename's preview panicked");
                    None
                });
                drop(snapshot);
                if let Some(text) = built {
                    waker.send(UserEvent::RenamePreviewed(generation, text));
                }
            });
        if spawned.is_err() {
            log::warn!("the thread that builds a rename's preview could not start");
            self.freer.free(edit);
            self.editor.message(format!(
                "{name}: not renamed: its preview could not be built"
            ));
            return;
        }
        self.servers.rename = Some(Renaming {
            stage: Stage::Reading {
                generation,
                old,
                new,
                edit,
            },
            ..renaming
        });
    }

    /// The preview of `generation` was built: shown in `*lsp-rename*`, if its rename still
    /// waits for it.
    pub(in crate::app) fn rename_previewed(&mut self, generation: u64, text: String) {
        let waits = matches!(
            self.servers.rename.as_ref().map(|renaming| &renaming.stage),
            Some(Stage::Reading { generation: built, .. }) if *built == generation
        );
        let Some(renaming) = self.servers.rename.take().filter(|_| waits) else {
            return;
        };
        let Stage::Reading { old, new, edit, .. } = renaming.stage else {
            return;
        };
        let back = self.editor.window().buffer();
        let preview = self
            .editor
            .add_internal_buffer(PREVIEW_NAME, Mode::RenamePreview);
        if let Some(buffer) = self.editor.buffer_mut(preview) {
            buffer.withhold_from_agents();
        }
        self.editor.set_internal_text(preview, &text);
        self.editor.show(preview);
        self.servers.rename = Some(Renaming {
            stage: Stage::Shown {
                old,
                new,
                edit,
                preview,
                back,
            },
            ..renaming
        });
        self.request_redraw();
    }

    /// `lsp-rename-apply`, the `y` of `*lsp-rename*`: applies the rename it shows.
    pub(super) fn apply_rename(&mut self) {
        let shown_now = self.editor.window().buffer();
        let Some(renaming) = self.servers.rename.take() else {
            return;
        };
        let Stage::Shown {
            old,
            new,
            edit,
            preview,
            back,
        } = renaming.stage
        else {
            self.servers.rename = Some(renaming);
            return;
        };
        if shown_now != preview {
            self.servers.rename = Some(Renaming {
                stage: Stage::Shown {
                    old,
                    new,
                    edit,
                    preview,
                    back,
                },
                ..renaming
            });
            return;
        }
        self.close_preview(preview, back);
        let title = format!(
            "renamed \u{2018}{}\u{2019} to \u{2018}{}\u{2019}",
            shown(&old),
            shown(&new)
        );
        self.apply_edit_of(&renaming.server, &title, edit, EditFrom::Rename);
        self.request_redraw();
    }

    /// `lsp-rename-cancel`, and `C-g` anywhere: a rename waiting for its server stops, and the
    /// server is told; the preview, if shown in the selected window, goes.
    pub(super) fn cancel_rename(&mut self) {
        let Some(renaming) = self.servers.rename.as_ref() else {
            return;
        };
        match &renaming.stage {
            Stage::Checking | Stage::Asked { .. } => {
                let kind = match renaming.stage {
                    Stage::Checking => AskKind::PrepareRename,
                    _ => AskKind::Rename,
                };
                if let Some(index) = self.servers.find(&renaming.server.0, &renaming.server.1) {
                    let buffer = renaming.buffer;
                    self.servers.running[index]
                        .server
                        .cancel(buffer.get(), kind);
                    self.servers.running[index].asked.remove(&(buffer, kind));
                }
                self.drop_rename(Some("stopped"));
            }
            Stage::Shown { preview, .. } if self.editor.window().buffer() == *preview => {
                self.drop_rename(Some("not applied"));
            }
            // A `C-g` elsewhere leaves the preview; the prompt's own `C-g` closed it.
            Stage::Shown { .. } | Stage::Naming { .. } => {}
            Stage::Reading { .. } => self.drop_rename(Some("stopped")),
        }
        self.request_redraw();
    }

    /// Tells the dispatcher whether a rename waits for its server or its preview, which `C-g`
    /// stops.
    fn note_rename_waits(&mut self) {
        let waits = matches!(
            self.servers.rename.as_ref().map(|renaming| &renaming.stage),
            Some(Stage::Checking | Stage::Asked { .. } | Stage::Reading { .. })
        );
        self.dispatcher.set_rename_waits(waits);
    }

    /// After each sync and each answer: a rename whose prompt closed with no name, or whose
    /// preview was killed, goes.
    pub(in crate::app) fn step_rename(&mut self) {
        let gone = match self.servers.rename.as_ref().map(|renaming| &renaming.stage) {
            Some(Stage::Naming { prompt, .. }) => !self
                .editor
                .prompts()
                .iter()
                .any(|open| open.id() == *prompt),
            Some(Stage::Shown { preview, .. }) => self.editor.buffer(*preview).is_none(),
            _ => false,
        };
        if gone {
            self.drop_rename(None);
        }
        self.note_rename_waits();
    }

    /// The server's check or rename failed: the rename ends, and the echo area says why.
    pub(super) fn rename_failed(
        &mut self,
        index: usize,
        taken: Option<(BufferId, Asker)>,
        kind: AskKind,
        why: &AskFailure,
    ) {
        if taken.is_none() || self.servers.rename.is_none() {
            return;
        }
        self.servers.rename = None;
        let name = self.servers.running[index].entry.name.clone();
        // The server's text may quote what it read: shown, not logged.
        self.editor
            .message_unlogged(super::places::failed(&name, kind, why));
        self.request_redraw();
    }

    /// Drops the rename under way, its preview with it, saying `said` of it if given.
    fn drop_rename(&mut self, said: Option<&str>) {
        let Some(renaming) = self.servers.rename.take() else {
            return;
        };
        let name = renaming.server.1.clone();
        match renaming.stage {
            Stage::Reading { edit, .. } => {
                self.servers
                    .preview_generation
                    .fetch_add(1, Ordering::Relaxed);
                self.freer.free(edit);
            }
            Stage::Shown {
                edit,
                preview,
                back,
                ..
            } => {
                self.close_preview(preview, back);
                self.freer.free(edit);
            }
            Stage::Checking | Stage::Naming { .. } | Stage::Asked { .. } => {}
        }
        if let Some(said) = said {
            self.editor.message(format!("{name}: rename {said}"));
        }
    }

    /// Kills the preview, showing `back` where it was.
    fn close_preview(&mut self, preview: BufferId, back: BufferId) {
        if self.editor.window().buffer() == preview && self.editor.buffer(back).is_some() {
            self.editor.show(back);
        }
        self.editor.kill_buffer(preview);
    }
}

#[cfg(test)]
mod tests;
