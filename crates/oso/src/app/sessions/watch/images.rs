//! `session-yank-image` carried out (ADR 0142): the clipboard's image is read on the clipboard's
//! threads, kept in a file by `sessions::images`' keeper thread, and an `@` reference to the file
//! is pasted into the session through [`App::paste_into_session`], the one delivery path, so that
//! a session that cannot take a paste refuses it as it refuses a send; a native session's goes
//! into its composer (ADR 0165, part 2f). It is shown only while the person is still in it. A session's images go when
//! it is archived or dropped, and old ones when the sessions are recalled at the start.
//!
//! Must not depend on: how a frame is drawn, or key presses.
//!
//! Nothing here touches the disk, and an image dropped here goes to the freer's thread. The
//! image itself never reaches the agent channel or a log, and the echo area names only its file,
//! not the directory: the full path is pasted into a terminal the channel reads only where the
//! person allowed it (ADR 0120, Q16). Input from the dev socket reads only a clipboard in
//! memory, as a yank does (ADR 0074): the dispatcher notes where each run came from.

use oso_core::buffer::BufferId;
use oso_platform::clipboard::{Image, ImageContents, MAX_IMAGE};

use crate::app::sessions::{Outgoing, Shown};
use crate::app::{App, UserEvent, clipboard};
use crate::sessions::identity::SessionId;
use crate::sessions::images::{self, Kept};
use crate::sessions::worker::SessionKey;

impl App {
    /// `session-yank-image` in the terminal `buffer`: starts reading the clipboard's image for
    /// its session, unless the session could not take the paste now, or an image is being read
    /// already. `from_socket` says whether the command came through the dev socket, as the
    /// dispatcher noted when it ran.
    pub(in crate::app) fn yank_image(&mut self, buffer: BufferId, from_socket: bool) {
        if let Err(why) = self.start_image_read(buffer, from_socket) {
            self.editor.message(why);
        }
        self.request_redraw();
    }

    fn start_image_read(&mut self, buffer: BufferId, from_socket: bool) -> Result<(), String> {
        let key = (self.hosting.table().by_buffer(buffer))
            .map(|session| session.key)
            .or_else(|| self.composer_session(buffer))
            .ok_or_else(|| "This buffer holds no session".to_owned())?;
        if self.watch.sending.reading_image_for.is_some() {
            return Err("Still reading the clipboard's image".to_owned());
        }
        // Checked first, so that no file is kept for a paste that would be refused; a native
        // session's composer takes it (ADR 0165, step 2f).
        if !self.hosting.natives.has(key) {
            self.paste_target(key)?;
        }
        let clipboard = self
            .clipboard
            .as_ref()
            .ok_or_else(|| "There is no clipboard: see the log".to_owned())?;
        if !clipboard::may_read(from_socket, clipboard::is_memory(clipboard)) {
            return Err("Input from the dev socket reads no system clipboard".to_owned());
        }
        clipboard.read_image();
        self.watch.sending.reading_image_for = Some(key);
        Ok(())
    }

    /// Pastes `reference`, a kept image's, into session `key`. The read and the save took a
    /// while: the session is shown only while the person is still in it, so that no key meant
    /// for another buffer lands in it. Whether it was shown.
    fn paste_kept_image(&mut self, key: SessionKey, reference: &str) -> Result<Shown, String> {
        let selected = self.editor.window().buffer();
        let session = self
            .hosting
            .table()
            .get(key)
            .and_then(|session| session.buffer);
        let shown = match session == Some(selected) || self.composer_session(selected) == Some(key)
        {
            true => Shown::Session,
            false => Shown::Nothing,
        };
        self.paste_into_session(key, Outgoing::Reference(reference), shown)?;
        Ok(shown)
    }

    /// The clipboard's threads answered the image read.
    pub(in crate::app) fn clipboard_image(&mut self, contents: ImageContents) {
        // No read of this instance's is out: an answer nobody waits for is dropped.
        let Some(key) = self.watch.sending.reading_image_for else {
            self.freer.free(contents);
            return;
        };
        let said = match contents {
            ImageContents::Image(image) => match self.keep_image(key, image) {
                Ok(()) => return,
                Err(why) => why,
            },
            ImageContents::Nothing => "No image on the clipboard".to_owned(),
            ImageContents::Concealed => {
                "The app that copied the image marked it secret; nothing is attached".to_owned()
            }
            ImageContents::TooLarge => format!(
                "The image on the clipboard is over {} MiB; nothing is attached",
                MAX_IMAGE >> 20
            ),
            ImageContents::NoAnswer => "The clipboard is busy or not answering".to_owned(),
            ImageContents::Unsupported => {
                "Images on the clipboard are not supported on this system yet".to_owned()
            }
        };
        self.watch.sending.reading_image_for = None;
        self.editor.message(said);
        self.request_redraw();
    }

    /// Hands `image` to the keeper for session `key`, which reports [`UserEvent::ImageKept`].
    /// An image that cannot be kept goes to the freer.
    fn keep_image(&mut self, key: SessionKey, image: Image) -> Result<(), String> {
        let id = self
            .hosting
            .table()
            .get(key)
            .map(|session| session.id.clone());
        let (Some(id), Some(root)) = (id, images::root()) else {
            self.freer.free(image);
            return Err("There is no session or cache directory to keep the image for".to_owned());
        };
        let waker = self.waker.clone();
        let done = Box::new(move |kept| {
            waker.send(UserEvent::ImageKept(key, Box::new(kept)));
        });
        let refused = self.watch.sending.keeper.save(root, id, image, done);
        refused.map_err(|image| {
            self.freer.free(image);
            "Cannot keep the image: see the log".to_owned()
        })
    }

    /// The image for session `key` was kept, or could not be: its reference is pasted into the
    /// session, which may have stopped taking pastes meanwhile. A file no paste refers to is
    /// removed.
    pub(in crate::app) fn image_kept(&mut self, key: SessionKey, kept: Result<Kept, String>) {
        self.watch.sending.reading_image_for = None;
        let said = match kept {
            Err(why) => why,
            Ok(kept) => match self.paste_kept_image(key, &kept.reference) {
                Ok(shown) => {
                    let name = kept
                        .path
                        .file_name()
                        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                    let png = if kept.converted { ", as PNG" } else { "" };
                    let hint = self.send_hint(key);
                    match shown {
                        Shown::Session => format!("Image saved as {name}{png}: {hint}"),
                        Shown::Nothing => {
                            let session = (self.hosting.table().get(key))
                                .map_or_else(String::new, |session| session.name.clone());
                            format!("Image saved as {name}{png}, put into {session}: {hint}")
                        }
                    }
                }
                Err(why) => {
                    self.watch.sending.keeper.drop_file(kept.path);
                    why
                }
            },
        };
        self.editor.message(said);
        self.request_redraw();
    }

    /// Session `id`'s images go, as it was archived or dropped (ADR 0142).
    pub(in crate::app) fn drop_session_images(&mut self, id: SessionId) {
        if let Some(root) = images::root() {
            self.watch.sending.keeper.drop_session(root, id);
        }
    }

    /// Images older than a week go, as oso starts.
    pub(in crate::app) fn sweep_images(&mut self) {
        if let Some(root) = images::root() {
            self.watch.sending.keeper.sweep(root);
        }
    }
}
