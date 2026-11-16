//! `session-yank-image` carried out (ADR 0142): the clipboard's image is read on the clipboard's
//! threads, kept in a file on a thread of its own, and an `@` reference to the file is pasted
//! into the session through [`App::send_to_session`], the one delivery path, so that a session
//! that cannot take a paste refuses it as it refuses a send.
//!
//! Must not depend on: how a frame is drawn, or key presses.
//!
//! Nothing here touches the disk: the file is written on a thread of its own, and removed by
//! the sessions worker.
//!
//! The image itself never reaches the agent channel or a log: only its file's path is pasted,
//! into a terminal the channel reads only where the person allowed it (ADR 0120, Q16). Input
//! from the dev socket reads only a clipboard in memory, as a yank does (ADR 0074).

use std::path::Path;
use std::thread;
use std::time::SystemTime;

use oso_core::buffer::BufferId;
use oso_core::editor::SessionRequest;
use oso_platform::clipboard::{Image, ImageContents, MAX_IMAGE};

use crate::app::{App, UserEvent, clipboard};
use crate::sessions::images::{self, Report};
use crate::sessions::worker::{ImageJob, Job, SessionKey};

impl App {
    /// After a command: reads the clipboard's image for each `session-yank-image` it asked for,
    /// while the dispatcher still knows whether the input came through the dev socket. The
    /// other session requests stay for `sync_sessions`, in their order.
    pub(in crate::app) fn start_image_yanks(&mut self) {
        let requests = self.editor.take_session_requests();
        if requests.is_empty() {
            return;
        }
        let from_socket = self.dispatcher.input_from_socket();
        for request in requests {
            match request {
                SessionRequest::YankImage(buffer) => self.yank_image(buffer, from_socket),
                other => self.editor.request_session(other),
            }
        }
    }

    /// Starts reading the clipboard's image for the session whose terminal is `buffer`, unless
    /// it could not take the paste now, or an image is being read already.
    fn yank_image(&mut self, buffer: BufferId, from_socket: bool) {
        let said = self.start_image_read(buffer, from_socket);
        if let Err(why) = said {
            self.editor.message(why);
        }
        self.request_redraw();
    }

    fn start_image_read(&mut self, buffer: BufferId, from_socket: bool) -> Result<(), String> {
        let key = self
            .hosting
            .table()
            .by_buffer(buffer)
            .map(|session| session.key)
            .ok_or_else(|| "This buffer holds no session".to_owned())?;
        if self.watch.sending.image.is_some() {
            return Err("Still reading the clipboard's image".to_owned());
        }
        // Checked first, so that no file is kept for a paste that would be refused.
        self.paste_target(key)?;
        let clipboard = self
            .clipboard
            .as_ref()
            .ok_or_else(|| "There is no clipboard: see the log".to_owned())?;
        if !clipboard::may_read(from_socket, clipboard::is_memory(clipboard)) {
            return Err("Input from the dev socket reads no system clipboard".to_owned());
        }
        clipboard.read_image();
        self.watch.sending.image = Some(key);
        Ok(())
    }

    /// The clipboard's threads answered the image read.
    pub(in crate::app) fn clipboard_image(&mut self, contents: ImageContents) {
        // No read of this instance's is out: an answer nobody waits for is dropped.
        let Some(key) = self.watch.sending.image else {
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
            ImageContents::NoAnswer => "The clipboard is not answering".to_owned(),
            ImageContents::Unsupported => {
                "Images on the clipboard are not supported on this system yet".to_owned()
            }
        };
        self.watch.sending.image = None;
        self.editor.message(said);
        self.request_redraw();
    }

    /// Keeps `image` for session `key` on a thread of its own, which reports
    /// [`UserEvent::ImageKept`].
    fn keep_image(&mut self, key: SessionKey, image: Image) -> Result<(), String> {
        let id = self
            .hosting
            .table()
            .get(key)
            .map(|session| session.id.clone())
            .ok_or_else(|| "That session is gone".to_owned())?;
        let root = images::root()
            .ok_or_else(|| "There is no cache directory to keep the image in".to_owned())?;
        let waker = self.waker.clone();
        thread::Builder::new()
            .name("oso-image".into())
            .spawn(move || {
                let kept = images::save(&root, &id, &image, SystemTime::now());
                waker.send(UserEvent::ImageKept(Box::new(Report { key, kept })));
            })
            .map(drop)
            .map_err(|e| format!("Cannot keep the image: {e}"))
    }

    /// The image for a session was kept, or could not be: its reference is pasted into the
    /// session, which may have stopped taking pastes meanwhile. A file no paste refers to is
    /// removed.
    pub(in crate::app) fn image_kept(&mut self, report: Report) {
        self.watch.sending.image = None;
        let said = match report.kept {
            Err(why) => why,
            Ok(kept) => match self.send_to_session(report.key, kept.reference) {
                Ok(()) => {
                    let shown = shown(&kept.path);
                    if kept.converted {
                        format!("Image saved to {shown} as PNG: press Enter there")
                    } else {
                        format!("Image saved to {shown}: press Enter there")
                    }
                }
                Err(why) => {
                    if let Some(worker) = &self.sessions {
                        worker.send(Job::Images(ImageJob::File(kept.path)));
                    }
                    why
                }
            },
        };
        self.editor.message(said);
        self.request_redraw();
    }

    /// Session `id`'s images go, as it was archived or dropped (ADR 0142).
    pub(in crate::app) fn drop_session_images(&self, id: crate::sessions::identity::SessionId) {
        if let (Some(worker), Some(root)) = (&self.sessions, images::root()) {
            worker.send(Job::Images(ImageJob::Session(root, id)));
        }
    }
}

/// `path` with the home directory written as `~`.
fn shown(path: &Path) -> String {
    let home = oso_platform::privacy::home();
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}
