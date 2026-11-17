//! A rename's preview on the UI thread (ADR 0138, step 14, Q3): once its edits are placed in the
//! buffers they change, opened for it if none held them, the preview is built off the UI thread
//! from those texts, shown in `*lsp-rename*`, and `y` applies exactly those edits.
//!
//! Must not depend on: the window system or the drawing; nothing here reads a file.
//!
//! - `y` applies only while every buffer is at the revision the preview was built from
//!   (`Editor::apply_server_edit` checks each), and what it changes is left unsaved (Q9).
//! - What was opened for it is killed if it is not applied.
//! - The edits go to the preview's thread and come back with its text, so nothing large is
//!   copied on the UI thread.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::BufferEdit;

use super::super::super::{App, UserEvent};
use super::super::actions::said_of;
use super::names::shown_path;
use super::preview::{self, Snapshot, shown};
use super::{PlacedEdits, Renaming, Stage};

/// The name of the buffer that shows a rename's edits.
const PREVIEW_NAME: &str = "*lsp-rename*";

impl App {
    /// The rename's edits, `parts`, placed by `applying` in the buffers they change, `opened`
    /// for it among them: its preview is built off the UI thread, or the rename is refused.
    pub(in crate::app::servers) fn rename_placed(
        &mut self,
        server: &(PathBuf, String),
        parts: Vec<BufferEdit>,
        opened: Vec<BufferId>,
    ) {
        let waits = matches!(
            self.servers.rename.as_ref(),
            Some(Renaming {
                server: renaming,
                stage: Stage::Opening { .. },
                ..
            }) if renaming == server
        );
        let Some(Renaming {
            server,
            buffer,
            stage: Stage::Opening { old, new },
        }) = self.servers.rename.take().filter(|_| waits)
        else {
            self.kill_opened(&opened);
            return self.freer.free(parts);
        };
        let snapshot = match self.rename_snapshot(&server, (&old, &new), &parts, opened.len()) {
            Ok(snapshot) => snapshot,
            Err(why) => {
                self.kill_opened(&opened);
                self.freer.free(parts);
                self.editor
                    .message(format!("{}: not renamed: {why}", server.1));
                return self.request_redraw();
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
                    preview::build(&snapshot, &parts, &|| {
                        live.load(Ordering::Relaxed) == generation
                    })
                }));
                let built = built.unwrap_or_else(|_| {
                    log::warn!("the build of a rename's preview panicked");
                    Some(Err("its preview could not be built"))
                });
                // The texts are freed here too.
                drop(snapshot);
                if let Some(built) = built {
                    waker.send(UserEvent::RenamePreviewed(
                        generation,
                        built,
                        PlacedEdits(parts),
                    ));
                }
            });
        if spawned.is_err() {
            log::warn!("the thread that builds a rename's preview could not start");
            self.kill_opened(&opened);
            self.editor.message(format!(
                "{}: not renamed: its preview could not be built",
                server.1
            ));
            return self.request_redraw();
        }
        self.servers.rename = Some(Renaming {
            server,
            buffer,
            stage: Stage::Reading {
                generation,
                old,
                new,
                opened,
            },
        });
    }

    /// What the preview of `parts`, `server`'s rename of `old` to `new`, is built from: each
    /// buffer's text, shared, and its path from the root; or why it is refused.
    fn rename_snapshot(
        &self,
        server: &(PathBuf, String),
        (old, new): (&str, &str),
        parts: &[BufferEdit],
        opened: usize,
    ) -> Result<Snapshot, &'static str> {
        let index = (self.servers.find(&server.0, &server.1)).ok_or("its server ended")?;
        let running = &self.servers.running[index];
        let root = super::super::read_back(&running.root).unwrap_or_else(|| running.root.clone());
        let mut files = Vec::with_capacity(parts.len());
        for part in parts {
            let buffer = (self.editor.buffer(part.buffer)).ok_or("a buffer it edits is gone")?;
            if buffer.revision() != part.revision {
                return Err("a buffer it edits changed since");
            }
            if buffer.is_secret() {
                return Err("it edits a buffer that may hold secrets");
            }
            let path = shown_path(buffer.path(), &root, buffer.name());
            files.push((path, buffer.text().clone()));
        }
        Ok(Snapshot {
            server: server.1.clone(),
            old: old.to_owned(),
            new: new.to_owned(),
            opened,
            files,
        })
    }

    /// The preview of `generation` was built, `built`, from `parts`: shown in `*lsp-rename*` if
    /// its rename still waits for it, or the rename refused.
    pub(in crate::app) fn rename_previewed(
        &mut self,
        generation: u64,
        built: Result<String, &'static str>,
        parts: PlacedEdits,
    ) {
        let waits = matches!(
            self.servers.rename.as_ref().map(|renaming| &renaming.stage),
            Some(Stage::Reading { generation: built, .. }) if *built == generation
        );
        let Some(Renaming {
            server,
            buffer,
            stage: Stage::Reading {
                old, new, opened, ..
            },
        }) = self.servers.rename.take().filter(|_| waits)
        else {
            return self.freer.free(parts);
        };
        let text = match built {
            Ok(text) => text,
            Err(why) => {
                self.kill_opened(&opened);
                self.freer.free(parts);
                self.editor
                    .message(format!("{}: not renamed: {why}", server.1));
                return self.request_redraw();
            }
        };
        let back = self.editor.tiled().0.buffer();
        let preview = self
            .editor
            .add_internal_buffer(PREVIEW_NAME, Mode::RenamePreview);
        if let Some(shown) = self.editor.buffer_mut(preview) {
            shown.withhold_from_agents();
        }
        // At most `preview::PREVIEW_BYTES_MAX`.
        self.editor.set_internal_text(preview, &text);
        self.editor.show(preview);
        self.servers.rename = Some(Renaming {
            server,
            buffer,
            stage: Stage::Shown {
                old,
                new,
                parts,
                opened,
                preview,
                back,
            },
        });
        self.request_redraw();
    }

    /// `lsp-rename-apply`, the `y` of `*lsp-rename*`: applies exactly what it shows, only while
    /// every buffer is still at the text it was built from.
    pub(in crate::app::servers) fn apply_rename(&mut self) {
        let showing = matches!(
            self.servers.rename.as_ref().map(|renaming| &renaming.stage),
            Some(Stage::Shown { preview, .. }) if self.editor.tiled().0.buffer() == *preview
        );
        let Some(Renaming {
            server,
            stage:
                Stage::Shown {
                    old,
                    new,
                    parts,
                    opened,
                    preview,
                    back,
                },
            ..
        }) = self.servers.rename.take().filter(|_| showing)
        else {
            return;
        };
        self.close_preview(preview, back);
        let title = format!(
            "renamed \u{2018}{}\u{2019} to \u{2018}{}\u{2019}",
            shown(&old),
            shown(&new)
        );
        match self.editor.apply_server_edit(parts.0) {
            Ok(applied) => {
                let edits = if applied.edits == 1 { "edit" } else { "edits" };
                let buffers = if applied.buffers == 1 {
                    "buffer"
                } else {
                    "buffers"
                };
                // As Eglot leaves them (Q9).
                let said = format!(
                    "{} {edits} in {} {buffers}, unsaved; C-x s saves",
                    applied.edits, applied.buffers
                );
                self.editor
                    .message_unlogged(said_of(&server.1, &said, &title));
            }
            Err(why) => {
                self.kill_opened(&opened);
                self.editor
                    .message_unlogged(said_of(&server.1, why, &title));
            }
        }
        self.request_redraw();
    }

    /// Kills the preview, the window that shows it going back to `back`.
    pub(super) fn close_preview(&mut self, preview: BufferId, back: BufferId) {
        let window = self.editor.tiled().0;
        if window.buffer() == preview && self.editor.buffer(back).is_some() {
            let window = window.id();
            self.editor.show_in(window, back);
        }
        self.editor.kill_buffer(preview);
    }
}
